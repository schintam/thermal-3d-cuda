from __future__ import annotations

import json
from pathlib import Path
from typing import List

from fastapi import FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse
from pydantic import BaseModel, Field, validator

from .simulation import Hotspot, SimulationConfig, ThermalSimulator, example_configuration

app = FastAPI(title="Thermal 3D CUDA Simulator")
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

static_dir = Path(__file__).resolve().parent.parent / "frontend"
index_file = static_dir / "index.html"


class HotspotModel(BaseModel):
    x: int
    y: int
    z: int
    temperature: float = Field(..., gt=0)


class SimulationRequest(BaseModel):
    nx: int = Field(32, gt=0)
    ny: int = Field(32, gt=0)
    nz: int = Field(16, gt=0)
    iterations: int = Field(100, gt=0, le=5000)
    diffusion: float = Field(1e-5, gt=0)
    dt: float = Field(5e-3, gt=0)
    dx: float = Field(1e-3, gt=0)
    ambient: float = Field(300.0)
    hotspots: List[HotspotModel] = Field(default_factory=list)

    @validator("hotspots")
    def validate_hotspots(cls, hotspots, values):
        nx = values.get("nx") or 0
        ny = values.get("ny") or 0
        nz = values.get("nz") or 0
        for hs in hotspots:
            if not (0 <= hs.x < nx and 0 <= hs.y < ny and 0 <= hs.z < nz):
                raise ValueError("Hotspot outside simulation bounds")
        return hotspots


@app.get("/")
def read_index():
    if not index_file.exists():
        raise HTTPException(status_code=404, detail="Frontend not found")
    return FileResponse(index_file)


@app.get("/api/example")
def example_config():
    cfg = example_configuration()
    return json.loads(json.dumps(cfg.__dict__, default=lambda o: o.__dict__))


@app.post("/api/simulate")
def simulate(body: SimulationRequest):
    cfg = SimulationConfig(
        nx=body.nx,
        ny=body.ny,
        nz=body.nz,
        iterations=body.iterations,
        diffusion=body.diffusion,
        dt=body.dt,
        dx=body.dx,
        ambient=body.ambient,
        hotspots=[Hotspot(**hs.dict()) for hs in body.hotspots],
    )
    simulator = ThermalSimulator()
    result = simulator.run(cfg)
    temperatures = result["temperature"].reshape(-1).tolist()
    return {
        "backend": result["backend"],
        "nx": body.nx,
        "ny": body.ny,
        "nz": body.nz,
        "min": result["min"],
        "max": result["max"],
        "temperature": temperatures,
    }


@app.get("/health")
def health_check():
    return {"status": "ok"}


# Local dev entrypoint: uvicorn backend.main:app --reload --port 8000
