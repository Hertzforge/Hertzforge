# Hertzforge

Hertzforge is one toolset for structural dynamics testing, from acquiring
the data to analyzing it. It joins Kevin Cross's data acquisition software
with Brandon Zwink's structural dynamics analysis.

The analysis core is under `src/hertzforge/`. The `hertzforge` package on PyPI
at 0.0.1 predates it and installs only a command that prints its version;
0.1.0 is the release that carries the core, and the desktop app of the same
number is on [hertzforge.com](https://hertzforge.com).

If you used Visual Dynamics, your `.vdyn` projects open here.

```
pip install hertzforge
hertzforge --version
```

## The First Vertical Slice

The rest of this repository is the first slice through every layer of the
merged toolset. What still carries the working name `sds` is the
`crates/sds-control` crate, the protocol id `sds/1` and the shell's saved
theme setting.

| Path | Language | Owns |
|---|---|---|
| `src/hertzforge/` | Python | The `hertzforge` package: the analysis core, its readers and writers, reports, and the `hertzforge` command |
| `src/hertzforge/service/` | Python | The analysis service: the core behind the protocol, started by `hertzforge serve` |
| `schema/` | JSON | The protocol (`PROTOCOL.md`, `protocol.schema.json`) and golden frames every codec is tested against |
| `crates/sds-control/` | Rust | The control service. Runs closed-loop Random on the simulator or on NI-DAQmx hardware, on loopback |
| `crates/hertzforge-*/`, `modules/`, `abi/` | Rust, C header | The controller: control laws, the hardware abstraction with its Sim and NI backends, data formats, and the module ABI |
| `ui/` | TypeScript, Svelte 5 | The shell: workspaces, command palette, the viz kit, and panels built from operation descriptors |
| `desktop/` | Rust | The desktop app: the shell in a window of its own, with both services and a bundled Python |
| `tools/demo-video/` | JavaScript | A scripted walkthrough of the shell, recorded to video |
| `tools/e2e-sim/` | JavaScript | Closed-loop Random on the simulator, end to end through the real control service and the shell |
| `scripts/` | shell | Developer entry points |
| `site/` | HTML | The hertzforge.com page, deployed by `.github/workflows/site.yml` when it changes on `main` |

The control service and the controller crates are one Cargo workspace
(`cargo test --workspace`). The NI backend loads the NI-DAQmx driver at run
time, so building and testing need no NI software installed.

### Layer Rules

- Rust owns anything the real-time loop or the simulator executes, the hardware backends, and kernels
  shared by control and analysis.
- Python owns everything offline: analysis, modal fitting, FEM, specification tools, reports, and the
  scripting API.
- TypeScript owns every pixel and no numerics. A view asks a service for a result and draws it.
- Spectra are plotted the way they are calculated: a density per line is drawn flat across its bin,
  one df wide. The service that computed the data says how it is drawn.
- The control loop and abort path never depend on the GUI, the network, or Python.
- A simulated run is always marked `source: "SIM"` and is never presented as a hardware result.
- No employer, client, laboratory or partner names anywhere in the tree.
- Dependencies are permissively licensed (MIT, BSD, Apache-2.0, ISC, Zlib; OFL-1.1 for bundled fonts;
  MPL-2.0 reviewed case by case). No copyleft packages. The one exception is Intel's oneMKL, which
  the sparse modal solve factors with on x86-64 Windows and Linux (`THIRD_PARTY_NOTICES.md`).
- The app makes no network requests beyond its own services: fonts and assets are bundled, because
  test labs run on closed networks.

### Run

```bash
pip install -e .
scripts/fetch_testdata.sh
scripts/dev.sh
```

The first line installs the package with the analysis service into the active
environment, the second fetches the plate test data the tests read (and
`scripts/make_examples.py` turns into the example projects on hertzforge.com).
`scripts/dev.sh` then starts the analysis service on 8791, the control service on 8792 and the shell on
http://127.0.0.1:5191/. `?mock=1` runs the shell alone, its control side on synthetic data.

The desktop app (`desktop/`) carries the shell, both services and its own Python, so it needs
nothing else installed. `desktop/packaging/build.sh` builds it and its installers;
`docs/desktop-app.md` says what they contain and how the app stops a run when it quits.

The end-to-end check drives Random on the simulator through the real control service, in the
installed Chrome. It starts both services itself on free ports, takes about three minutes, and is not
part of `npm --prefix ui test`:

```bash
npm --prefix tools/e2e-sim ci
node tools/e2e-sim/random_sim.mjs --shots <folder for screenshots>
```

### Guides

| Guide | What it covers |
|---|---|
| [`docs/shock.md`](docs/shock.md) | Classical Shock: a compensated pulse on one shaker |
| [`docs/srs.md`](docs/srs.md) | SRS: a shock response spectrum on one shaker |
| [`docs/sine.md`](docs/sine.md) | Sine: one tone, its amplitude controlled as it sweeps |
| [`docs/mixed.md`](docs/mixed.md) | Mixed Mode: sine or narrow-band random over a random floor |
| [`docs/twr.md`](docs/twr.md) | TWR: time waveform replication |
| [`docs/mimo-random.md`](docs/mimo-random.md) | MIMO Random: random control with several shakers |
| [`docs/mimo-shock.md`](docs/mimo-shock.md) | MIMO Shock: classical pulses and SRS with several shakers |
| [`docs/explorer.md`](docs/explorer.md) | Explorer: one sine tone steered by hand |
| [`docs/recorder.md`](docs/recorder.md) | Recorder: the station's inputs recorded, nothing driven |
| [`docs/control-laws.md`](docs/control-laws.md), [`docs/control-law-spec.md`](docs/control-law-spec.md) | Writing a control law as a law file, and the interface it is written against |
| [`docs/reference-file.md`](docs/reference-file.md), [`docs/reference-from-field-data.md`](docs/reference-from-field-data.md) | Reference files, and references from field data |
| [`docs/hforge-format.md`](docs/hforge-format.md) | The `.hforge` run file |
| [`docs/desktop-app.md`](docs/desktop-app.md) | The desktop app |
| [`docs/documentation.md`](docs/documentation.md) | Documentation and typing conventions for contributors |

## License

Hertzforge is released under the MIT License (`LICENSE`). The source is at
https://github.com/hertzforge/hertzforge.
