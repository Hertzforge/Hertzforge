# Third-Party Notices

Attribution for permissively licensed third-party components shipped in or consulted by
Hertzforge. Extend it whenever a dependency or ported reference is added. The enforcement is
mechanical: `cargo deny check` (`deny.toml`) for Rust crates and `tools/check_tree.py` for npm
packages. This file is the human-readable attribution the licenses require in distributed
binaries.

## Rust crates shipped in the control service and the controller crates

| Crate | License | Use |
|---|---|---|
| serde / serde_json | MIT OR Apache-2.0 | serialization throughout |
| dirs | MIT OR Apache-2.0 | data-root resolution (eigen-capture) |
| option-ext | MPL-2.0 | reached only through `dirs`; used unmodified |
| libloading | ISC | NI-DAQmx dynamic loading |
| tracing family | MIT | logging |
| tokio / tokio-tungstenite / futures-util | MIT | eigen-backendd daemon and the control service |
| hound | Apache-2.0 | WAV read/write (eigen-formats, Insight). NOTICE: Apache-2.0 only, not dual |

## npm packages shipped in the shell

| Package | License | Use |
|---|---|---|
| uPlot | MIT | plot rendering |
| Svelte / SvelteKit / Vite toolchain (dev) | MIT | shell build |

## Format specifications and reference material (consulted, not linked)

Parsers in `eigen-formats` are written from public format specifications: UFF/UNV (UC-SDRL
publications), MATLAB MAT-File Format (MathWorks, R2026a), MTS RPC-3 specification (MTS
Systems), Abaqus `.fil` documentation (Dassault Systemes public documentation), ASAM ODS base
model (highQSoft public publication), NI TDMS internal-structure documentation (National
Instruments). The specifications themselves are not part of this repository. No third-party
source code was ported into this repository. Behavioral-oracle projects consulted as
documentation only: pyuff (MIT), pyNastran (BSD-3-Clause), MYSTRAN (MIT).

## Explicitly excluded

GPL, AGPL and SSPL components are excluded by policy and enforced by the license allowlist in
`deny.toml`. Its name bans also refuse the known copyleft picks in this field by name (`wav`,
`nastran`, `nastran-pch2csv`, all Gmsh bindings).
