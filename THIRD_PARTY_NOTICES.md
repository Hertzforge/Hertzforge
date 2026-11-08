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

| Package | License | Notes |
|---|---|---|
| pypardiso, the sparse modal solve's link to PARDISO (x86-64 Windows and Linux only) | BSD-3-Clause | keep the copyright notice |
| Intel oneMKL and TBB, which pypardiso brings: `mkl`, `tbb`, `tcmlib` and, on Windows, `onemkl-license`, the license text as a package (x86-64 Windows and Linux only) | Proprietary: the Intel Simplified Software License | the desktop app carries only the libraries PARDISO loads, Intel's unmodified binaries, with this notice; reverse engineering, decompiling or disassembling them is not permitted. Taken on November 3, 2026, so a large model solves in minutes rather than an hour |
| `intel-openmp`, `intel-cmplr-lib-ur` and `umf`, which a pip install of oneMKL brings as well | Proprietary: the Intel End User License Agreement for Developer Tools | not in the desktop app: oneMKL is threaded through TBB, which needs none of them. A pip install downloads them from Intel's own packages; nothing here redistributes them |

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
The OpenCASCADE wheel carries image and compression libraries under their own licenses
(FreeType, libpng, libtiff, zlib and others). None of these are redistributed by this repository
or the PyPI package. The desktop app does ship them; see below.

## The desktop app

The desktop app (`desktop/`, `docs/desktop-app.md`) ships the shell, the control service, and a
Python runtime with the packages above installed as their published wheels.

### Rust crates in the app itself

| Crate | License | Use |
|---|---|---|
| tauri, tauri-build, tao, wry, muda and the rest of the Tauri family | Apache-2.0 OR MIT | the window, the menu, serving the shell |
| tauri-plugin-dialog, rfd | Apache-2.0 OR MIT, MIT | the quit question and error messages |
| tungstenite | MIT OR Apache-2.0 | asking the control service whether a run is live |

`cargo deny --manifest-path desktop/Cargo.toml --config desktop/deny.toml check` checks every
crate in the app's own lockfile against the same license policy as the controller's. On Linux the
app uses the system's WebKitGTK and GTK (LGPL-2.1-or-later, loaded as shared libraries); on Windows,
the system's WebView2 runtime; on macOS, the system's WebKit.

The Tauri command line (`@tauri-apps/cli`, Apache-2.0 OR MIT) builds the app and is not shipped.

### The Python runtime

CPython 3.13 from python-build-standalone (github.com/astral-sh/python-build-standalone), under
the Python Software Foundation License (`python/lib/python3.13/LICENSE.txt` in the app). Linked
into it: OpenSSL 3.5 (Apache-2.0), SQLite (public domain), libffi (MIT), mpdecimal (BSD-2-Clause),
Expat (MIT), bzip2 (bzip2-1.0.6) and XZ's liblzma (0BSD); on Linux and Windows also zlib (Zlib), and
on Linux libedit (BSD-3-Clause), ncurses (X11) and libuuid (BSD-3-Clause), which the macOS build
takes from the system. `readline` is libedit, not GNU readline. The build script removes the Tcl/Tk
GUI kit, the `dbm` module (its library on some platforms carries a license this project does not
take; `dbm` falls back to its pure-Python form), pip, the headers and the test suites.

### Libraries inside the Python wheels

Shipped as the wheels ship them, as separate shared libraries (macOS arm64 build, 2026-11-02):

| Wheel | Libraries | Licenses |
|---|---|---|
| scipy | libgfortran, libgcc_s, libquadmath | GPL-3.0 with the GCC Runtime Library Exception; LGPL-2.1-or-later |
| h5py | HDF5, zlib | BSD-3-Clause style (HDF5), Zlib |
| netCDF4 | netCDF-C, HDF5, HDF4, libcurl, OpenSSL, Kerberos, libssh2, nghttp2, libxml2, libiconv, the AWS C libraries and s2n, libzip, Blosc, Snappy, LZ4, libaec, zstd, bzip2, zlib, libjpeg, libc++ | libiconv is LGPL-2.1-or-later; the rest are permissive (BSD style for netCDF and HDF, curl, Apache-2.0, MIT and MIT style, bzip2-1.0.6, Zlib, IJG) |
| cadquery-ocp-novtk | OpenCASCADE Technology (above), FreeImage, LibRaw, OpenEXR and Imath, FreeType, libpng, libtiff, libjpeg, JPEG XR, OpenJPEG, OpenJPH, libwebp, Little CMS, LERC, liblzma, zstd, libdeflate, fmt, zlib, libc++ | FreeImage is GPL-2.0 or GPL-3.0 or the FreeImage Public License; LibRaw is LGPL-2.1 or CDDL-1.0; FreeType is the FreeType License or GPL-2.0; the rest are permissive (BSD style, MIT, Apache-2.0, libpng, libtiff, IJG, Zlib) |

The netCDF4 and OpenCASCADE wheels do not include these libraries' license texts, so the app does
not yet either. FreeImage's choice
among its licenses (the FreeImage Public License is an MPL-1.1 style license, not on this project's
list as written) is for the maintainers to rule on.

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
