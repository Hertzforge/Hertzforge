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
| libloading | ISC | NI-DAQmx dynamic loading |
| tracing family | MIT | logging |
| tokio / tokio-tungstenite / futures-util | MIT | hertzforge-backendd daemon and the control service |
| hound | Apache-2.0 | WAV read/write (hertzforge-formats, Insight). NOTICE: Apache-2.0 only, not dual |
| num-complex | MIT OR Apache-2.0 | complex numbers in the spectral density matrix stacks (hertzforge-sdm) |
| faer, faer-traits | MIT | per-line dense linear algebra (Hermitian eigendecomposition, SVD, Cholesky, matrix products) in hertzforge-sdm |
| faer's kernels and helpers: gemm, gemm-common, gemm-c32, gemm-c64, gemm-f32, gemm-f64, nano-gemm, nano-gemm-core, nano-gemm-c32, nano-gemm-c64, nano-gemm-f32, nano-gemm-f64, private-gemm-x86, pulp, pulp-wasm-simd-flag, dyn-stack, dyn-stack-macros, equator, equator-macro, reborrow, qd, raw-cpuid, sysctl | MIT | matrix-multiply kernels, SIMD dispatch, workspace and CPU detection under faer |
| generativity, num_cpus, hermit-abi, paste, seq-macro, enum-as-inner | MIT OR Apache-2.0 | helpers in faer's dependency tree |
| bytemuck | Zlib OR Apache-2.0 OR MIT | plain-data casts in faer's dependency tree |
| walkdir, same-file | Unlicense OR MIT (used under MIT) | sysctl's dependencies under faer's matrix kernels |
| sha2, digest, block-buffer, crypto-common, generic-array, typenum, cpufeatures | MIT OR Apache-2.0 (generic-array MIT) | the content hash of the native reference file (hertzforge-reference) |
| zip | MIT | reading NumPy `.npz` specification files (hertzforge-reference) |
| flate2, miniz_oxide, crc32fast | MIT OR Apache-2.0 (miniz_oxide also Zlib) | inflating deflated `.npz` entries, pure Rust (hertzforge-reference) |
| adler2, simd-adler32 | 0BSD OR MIT OR Apache-2.0 (simd-adler32 MIT) | checksums under miniz_oxide |
| indexmap, hashbrown, equivalent, memchr, typed-path, getrandom, nix, bitflags, libc, cfg-if | MIT OR Apache-2.0 (memchr Unlicense OR MIT, used under MIT; nix MIT) | helpers in zip's dependency tree |

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
