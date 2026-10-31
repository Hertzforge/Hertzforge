# sds: Structural Dynamics Suite (Working Name)

The seed of the combined suite: one shell for test control, acquisition, analysis, modal, FEM,
specification and simulation work. `sds` is a placeholder until the name is chosen; renaming it is
a mechanical find and replace.

This repository starts as the first vertical slice through every layer, built as the real
foundation rather than a throwaway demo. Each piece here is the piece the full build keeps.

## Layout

| Path | Language | Owns |
|---|---|---|
| `schema/` | JSON | The protocol (`PROTOCOL.md`, `protocol.schema.json`) and golden frames every codec is tested against |
| `python/` | Python 3.13 | The analysis service. Wraps the `visualdynamics` package unchanged; adds no numerics of its own beyond glue |
| `crates/` | Rust | The control service. Runs the control engine and its Sim backend; builds on the controller crates by path |
| `ui/` | TypeScript, Svelte 5 | The shell: workspaces, command palette, the viz kit, and panels built from operation descriptors |
| `scripts/` | shell | Developer entry points |

## Layer Rules

- Rust owns anything the real-time loop or the simulator executes, the hardware backends, and kernels
  shared by control and analysis.
- Python owns everything offline: analysis, modal fitting, FEM, specification tools, reports, and the
  scripting API.
- TypeScript owns every pixel and no numerics. A view asks a service for a result and draws it.
- The control loop and abort path never depend on the GUI, the network, or Python.
- A simulated run is always marked `source: "SIM"` and is never presented as a hardware result.
- No employer, client, laboratory or partner names anywhere in the tree.
- Dependencies are permissively licensed (MIT, BSD, Apache-2.0, ISC, Zlib; OFL-1.1 for bundled fonts; MPL-2.0 reviewed case by
  case). No copyleft packages.
- The app makes no network requests beyond its own services: fonts and assets are bundled, because
  test labs run on closed networks.

## Run

```bash
scripts/dev.sh
```

Starts the analysis service on 8791, the control service on 8792 and the shell on
http://127.0.0.1:5191/.

## License

MIT is intended. The LICENSE file and its copyright line are added at release.
