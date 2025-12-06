"""3D thermal diffusion simulator with optional CUDA acceleration.

The solver implements a simple explicit finite-difference method for the
heat equation in a 3D grid. If CuPy is available, computations run on the GPU;
otherwise NumPy is used as a CPU fallback.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import List, Optional

import numpy as _np

try:  # Prefer GPU if available
    import cupy as _cp
except Exception:  # pragma: no cover - environment may not have CUDA
    _cp = None

ArrayModule = _np.ndarray


@dataclass
class Hotspot:
    x: int
    y: int
    z: int
    temperature: float


@dataclass
class SimulationConfig:
    nx: int
    ny: int
    nz: int
    iterations: int
    diffusion: float = 1e-5
    dt: float = 1e-2
    dx: float = 1e-3
    ambient: float = 300.0
    hotspots: Optional[List[Hotspot]] = None

    def validate(self) -> None:
        if min(self.nx, self.ny, self.nz) <= 0:
            raise ValueError("Grid dimensions must be positive")
        if self.iterations <= 0:
            raise ValueError("Iterations must be positive")
        if self.dt <= 0 or self.dx <= 0:
            raise ValueError("Time-step and cell size must be positive")
        if self.diffusion <= 0:
            raise ValueError("Diffusion constant must be positive")
        if self.hotspots:
            for hs in self.hotspots:
                if not (
                    0 <= hs.x < self.nx
                    and 0 <= hs.y < self.ny
                    and 0 <= hs.z < self.nz
                ):
                    raise ValueError(f"Hotspot {hs} outside grid bounds")


class ThermalSimulator:
    """Run 3D thermal diffusion on CPU or GPU."""

    def __init__(self) -> None:
        self.xp = _cp if _cp is not None else _np
        self.backend = "cupy" if _cp is not None else "numpy"

    def _allocate_grid(self, cfg: SimulationConfig):
        xp = self.xp
        grid = xp.full((cfg.nz, cfg.ny, cfg.nx), cfg.ambient, dtype=xp.float32)
        if cfg.hotspots:
            for hs in cfg.hotspots:
                if not (0 <= hs.x < cfg.nx and 0 <= hs.y < cfg.ny and 0 <= hs.z < cfg.nz):
                    raise ValueError(f"Hotspot {hs} outside grid bounds")
                grid[hs.z, hs.y, hs.x] = hs.temperature
        return grid

    def _laplacian(self, grid):
        xp = self.xp
        padded = xp.pad(grid, 1, mode="edge")
        center = padded[1:-1, 1:-1, 1:-1]
        lap = (
            padded[2:, 1:-1, 1:-1]
            + padded[:-2, 1:-1, 1:-1]
            + padded[1:-1, 2:, 1:-1]
            + padded[1:-1, :-2, 1:-1]
            + padded[1:-1, 1:-1, 2:]
            + padded[1:-1, 1:-1, :-2]
            - 6 * center
        )
        return lap

    def run(self, cfg: SimulationConfig) -> dict:
        cfg.validate()
        xp = self.xp

        grid = self._allocate_grid(cfg)
        alpha = cfg.diffusion
        dt_dx2 = cfg.dt / (cfg.dx ** 2)
        for _ in range(cfg.iterations):
            lap = self._laplacian(grid)
            grid = grid + alpha * dt_dx2 * lap

        # Move results to CPU
        if _cp is not None and isinstance(grid, _cp.ndarray):
            grid_cpu = _cp.asnumpy(grid)
        else:
            grid_cpu = grid

        return {
            "backend": self.backend,
            "temperature": grid_cpu,
            "min": float(grid_cpu.min()),
            "max": float(grid_cpu.max()),
        }


def example_configuration() -> SimulationConfig:
    return SimulationConfig(
        nx=32,
        ny=32,
        nz=16,
        iterations=100,
        diffusion=1e-5,
        dt=5e-3,
        dx=1e-3,
        ambient=300.0,
        hotspots=[
            Hotspot(x=16, y=16, z=8, temperature=400.0),
            Hotspot(x=10, y=22, z=4, temperature=380.0),
        ],
    )
