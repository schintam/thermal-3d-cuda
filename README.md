# Thermal 3D CUDA Simulator

GPU-accelerated 3D heat diffusion simulator for stacked dies and 3D IC packaging. The solver uses a CuPy backend when CUDA is available (ideal for DGX-class machines) and falls back to NumPy on CPU. A lightweight web UI (three.js + Pico.css) lets you configure geometry, materials, and hotspots and view temperature fields as a rotating voxel volume.

## Running locally

1. Install dependencies (use the CuPy build that matches your CUDA stack):

   ```bash
   python -m venv .venv
   source .venv/bin/activate
   pip install fastapi uvicorn[standard] numpy
   # GPU: pick a wheel such as cupy-cuda12x
   pip install cupy-cuda12x  # adjust suffix for your CUDA toolkit
   ```

2. Start the service:

   ```bash
   uvicorn backend.main:app --reload --port 8000
   ```

3. Open http://localhost:8000 in a browser. Update grid size, material constants, and hotspots, then click **Run simulation**. Results show the backend used (GPU vs CPU) and temperature range.

## API

- `GET /api/example`: seed values for UI.
- `POST /api/simulate`: run a simulation. Body fields:
  - `nx, ny, nz` (ints): grid dimensions.
  - `iterations` (int): finite-difference steps.
  - `diffusion` (float): diffusion coefficient (m²/s).
  - `dt`, `dx` (floats): timestep and cell pitch.
  - `ambient` (float): baseline temperature in Kelvin.
  - `hotspots` (list): `{x, y, z, temperature}` entries.

Response returns flatten temperature array plus min/max and backend identifier.

## Notes

- Simulation uses explicit finite differences with simple padding boundary conditions. For stability, ensure `diffusion * dt / dx^2` stays small (< ~0.1).
- The renderer uses instanced meshes for performance; large grids render best on desktop GPUs. On mobile, try reducing grid size.
