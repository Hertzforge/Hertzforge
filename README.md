# Hertzforge

Hertzforge will be one toolset for structural dynamics testing, from
acquiring the data to analyzing it. It is being built by merging two
existing projects: Eigen, Kevin Cross's data acquisition software, and
Visual Dynamics, Brandon Zwink's structural dynamics toolset
(https://visualdynamics.org).

The `hertzforge` package on PyPI (0.0.1) does not contain that code yet.
It installs a `hertzforge` command that prints its version and a short
help message, and nothing else. Later releases will bring the acquisition
and analysis features across.

```
pip install hertzforge
hertzforge --version
```

## The First Vertical Slice

The rest of this repository is the first slice through every layer of the
merged toolset, built as the foundation rather than a demo. Its code still
carries the working name `sds`; renaming it into the `hertzforge` package
layout is a mechanical change that comes next.

| Path | Language | Owns |
|---|---|---|
| `src/hertzforge/` | Python | The `hertzforge` command and package published to PyPI |
| `schema/` | JSON | The protocol (`PROTOCOL.md`, `protocol.schema.json`) and golden frames every codec is tested against |
| `python/` | Python 3.13 | The analysis service. Wraps the `visualdynamics` package unchanged |
| `crates/` | Rust | The control service. Runs the controller's Random control law on its Sim backend |
| `ui/` | TypeScript, Svelte 5 | The shell: workspaces, command palette, the viz kit, and panels built from operation descriptors |
| `tools/demo-video/` | JavaScript | A scripted walkthrough of the shell, recorded to video |
| `scripts/` | shell | Developer entry points |
| `site/` | HTML | The hertzforge.com page, deployed by `.github/workflows/site.yml` when it changes on `main` |

The control service builds against the controller's crates, which are not
in this repository yet. Until they are imported, it builds only beside a
checkout of the controller workspace at `../eigen`.

### Layer Rules

- Rust owns anything the real-time loop or the simulator executes, the hardware backends, and kernels
  shared by control and analysis.
- Python owns everything offline: analysis, modal fitting, FEM, specification tools, reports, and the
  scripting API.
- TypeScript owns every pixel and no numerics. A view asks a service for a result and draws it.
- Spectra are plotted the way they are calculated: a density per line is drawn flat across its bin,
  one df wide, as Visual Dynamics draws it. The service that computed the data says how it is drawn.
- The control loop and abort path never depend on the GUI, the network, or Python.
- A simulated run is always marked `source: "SIM"` and is never presented as a hardware result.
- No employer, client, laboratory or partner names anywhere in the tree.
- Dependencies are permissively licensed (MIT, BSD, Apache-2.0, ISC, Zlib; OFL-1.1 for bundled fonts;
  MPL-2.0 reviewed case by case). No copyleft packages.
- The app makes no network requests beyond its own services: fonts and assets are bundled, because
  test labs run on closed networks.

### Run

```bash
scripts/dev.sh
```

Starts the analysis service on 8791, the control service on 8792 and the shell on
http://127.0.0.1:5191/. `?mock=1` runs the shell alone on synthetic data.

## License

Hertzforge is released under the MIT License (`LICENSE`). The source is at
https://github.com/hertzforge/hertzforge.
