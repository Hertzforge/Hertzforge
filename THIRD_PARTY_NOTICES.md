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
| dirs | MIT OR Apache-2.0 | data-root resolution (hertzforge-capture) |
| option-ext | MPL-2.0 | reached only through `dirs`; used unmodified |
| libloading | ISC | NI-DAQmx dynamic loading |
| tracing family | MIT | logging |
| tokio / tokio-tungstenite / futures-util | MIT | hertzforge-backendd daemon and the control service |
| hound | Apache-2.0 | WAV read/write (hertzforge-formats, Insight). NOTICE: Apache-2.0 only, not dual |

## npm packages shipped in the shell

| Package | License | Use |
|---|---|---|
| uPlot | MIT | plot rendering |
| Svelte / SvelteKit / Vite toolchain (dev) | MIT | shell build |

## Python packages the hertzforge package depends on

The `hertzforge` package on PyPI does not include these: an install fetches each from PyPI under
its own license. `tools/check_licenses.py` reads every installed package's license in CI.

| Package | License | Use |
|---|---|---|
| numpy, scipy, h5py, pandas, pint, threadpoolctl | BSD-3-Clause | arrays, signal processing, HDF5 files, tables, units, BLAS threads |
| netCDF4, openpyxl | MIT | Rattlesnake `.nc4` files, Excel channel tables |
| certifi (netCDF4's dependency) | MPL-2.0 | used unmodified; source at github.com/certifi/python-certifi |
| cadquery-ocp-novtk (the `OCP` bindings) | Apache-2.0 | solids, booleans and STEP/IGES import |
| OpenCASCADE Technology 8.0.1, inside that wheel | LGPL-2.1 with the Open CASCADE exception | shipped as shared libraries, unmodified and replaceable; source at github.com/Open-Cascade-SAS/OCCT, tag `V8_0_1` |

Hertzforge makes use of facilities provided by the Open CASCADE Technology software
(https://dev.opencascade.org); this is the notice the Open CASCADE exception asks for.

The numpy and scipy wheels carry libgfortran and libgcc (GPL-3.0 with the GCC Runtime Library
Exception, which permits this use) and libquadmath (LGPL-2.1-or-later, a separate shared library).
The OpenCASCADE wheel carries image and compression libraries under their own permissive licenses
(FreeType, libpng, libtiff, zlib and others). None of these are redistributed by this repository
or the PyPI package. A packaged desktop build would ship them, and Visual Dynamics' `NOTICE.md`
lists each one with its license text; that list moves here when Hertzforge ships a package.

## Code and data from Visual Dynamics

The analysis core under `src/hertzforge/`, its tests and `testdata/` were moved from Visual
Dynamics 0.1.0a82 (github.com/visualdynamics/visualdynamics, MIT, Copyright (c) 2026 Brandon
Zwink) by its author.

`src/hertzforge/io/escdf_specifications/` holds the Engineering Sciences Common Data Format's
specification files, which the ESCDF reader parses for the format's types. They are BSD-3-Clause
and their `LICENSE` is kept beside them, unmodified, as that license requires.

## Format specifications and reference material (consulted, not linked)

Parsers in `hertzforge-formats` are written from public format specifications: UFF/UNV (UC-SDRL
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
