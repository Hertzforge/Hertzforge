#!/usr/bin/env bash
# The analysis service's Python, for the desktop app: a relocatable CPython from
# python-build-standalone (github.com/astral-sh/python-build-standalone, the
# interpreter's own PSF license and its libraries' permissive ones), with the
# hertzforge package from this checkout and its dependencies installed into it as
# published wheels. The app starts it as `python -I -B -m hertzforge serve`.
#
#     desktop/packaging/bundle_python.sh <output dir> [<target triple>]
#
# The output dir gets `python/` (the runtime) and `testdata/plate/` (the plate data
# the catalog's demonstration datasets read, from scripts/fetch_testdata.sh). The
# target triple defaults to this machine's. Nothing here runs at the app's run time:
# the installed app downloads nothing.
#
# Why not a PyInstaller freeze, as Visual Dynamics ships: its bootloader is
# GPL-2.0 with a linking exception, and this runtime avoids the question. The
# runtime is also an ordinary Python, so `-m hertzforge serve` is the same command a
# developer runs. docs/desktop-app.md has the comparison.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:?usage: bundle_python.sh <output dir> [<target triple>]}"
TRIPLE="${2:-$(rustc -vV | sed -n 's/^host: //p')}"
# The interpreter build, named like the Rust toolchain in rust-toolchain.toml: bumped
# by hand, checked against the release's own checksum. 3.13 is the version the CI
# analysis job tests on every desktop platform.
PBS_TAG="${PBS_TAG:-20261026}"
PY_VERSION="${PY_VERSION:-3.13.18}"
PY_MINOR="${PY_VERSION%.*}"

case "$TRIPLE" in
  aarch64-apple-darwin|x86_64-apple-darwin|x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) EXE=bin/python3 ;;
  x86_64-pc-windows-msvc|aarch64-pc-windows-msvc) EXE=python.exe ;;
  *) echo "bundle_python: no runtime for $TRIPLE" >&2; exit 1 ;;
esac
ASSET="cpython-${PY_VERSION}+${PBS_TAG}-${TRIPLE}-install_only_stripped.tar.gz"
URL="https://github.com/astral-sh/python-build-standalone/releases/download/${PBS_TAG}/${ASSET}"

mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
WORK="$OUT/.work"
rm -rf "$WORK" "$OUT/python" "$OUT/testdata"
mkdir -p "$WORK"
trap 'rm -rf "$WORK"' EXIT

echo "bundle_python: $ASSET"
curl -fsSL -o "$WORK/$ASSET" "$URL"
curl -fsSL -o "$WORK/SHA256SUMS" \
  "https://github.com/astral-sh/python-build-standalone/releases/download/${PBS_TAG}/SHA256SUMS"
want="$(awk -v a="$ASSET" '$2 == a { print $1 }' "$WORK/SHA256SUMS")"
[[ -n "$want" ]] || { echo "bundle_python: $ASSET is not in the release's SHA256SUMS" >&2; exit 1; }
if command -v sha256sum >/dev/null; then
  have="$(sha256sum "$WORK/$ASSET" | cut -d' ' -f1)"
else
  have="$(shasum -a 256 "$WORK/$ASSET" | cut -d' ' -f1)"
fi
[[ "$want" == "$have" ]] || { echo "bundle_python: checksum mismatch for $ASSET" >&2; exit 1; }
tar -xzf "$WORK/$ASSET" -C "$OUT"
rm -f "$WORK/$ASSET"
PY="$OUT/python/$EXE"
"$PY" -V

# The package as a wheel built from this checkout, then it and its dependencies as
# published wheels only: nothing is compiled from source for the bundle.
"$PY" -m pip install -q --disable-pip-version-check --no-warn-script-location \
  --upgrade pip
"$PY" -m pip wheel -q --disable-pip-version-check --no-deps -w "$WORK/wheels" "$ROOT"
"$PY" -m pip install -q --disable-pip-version-check --no-warn-script-location \
  --only-binary=:all: "$WORK"/wheels/hertzforge-*.whl

# No GPL package in the bundle, and every import of the shipped code provided by an
# installed package: tools/check_licenses.py's two checks, run on the environment the
# app ships (the second over src/ only, since the tests' own tools are not shipped).
(cd "$ROOT" && "$PY" - <<'PY'
import sys
from importlib import metadata
sys.path.insert(0, 'tools')
import check_licenses as c
files = c.tracked_python()
shipped = [p for p in files if p.relative_to(c.ROOT).parts[0] == 'src']
findings = c.check_installed() + c.check_imports(shipped, c.first_party(files, c.ROOT))
print(*findings, sep='\n')
count = sum(1 for _ in metadata.distributions())
print(f'check_licenses: the bundled runtime: {count} installed packages, '
      f'{len(shipped)} shipped files, {len(findings)} findings')
sys.exit(1 if findings else 0)
PY
)

# oneMKL, PARDISO's library (core.sparse_solve), on x86-64 Windows and Linux, where
# pypardiso is a dependency: only what PARDISO loads, threaded through TBB, as Visual
# Dynamics' builds carry it. The runtime, the core and its interface, the TBB threading
# layer, every processor's kernels and their vector math (another machine loads another
# one), and TBB with its allocator and thread manager. Not the OpenMP runtime, which is
# under Intel's developer-tools EULA and which TBB threading does not need, nor
# ScaLAPACK, BLACS or CDFT, nor the wheels' unversioned second copy of each library.
# `hertzforge check-solver` below is what proves the bundle still factors with it.
if [[ $TRIPLE == x86_64-pc-windows-msvc || $TRIPLE == x86_64-unknown-linux-gnu ]]; then
  "$PY" -m pip uninstall -q -y intel-openmp intel-cmplr-lib-ur umf 2>/dev/null || true
  "$PY" - <<'TRIM'
import re
from importlib.metadata import PackageNotFoundError, distribution
from pathlib import Path

PARTS = (r'rt|core|intel_lp64|tbb_thread|def|mc3|avx2|avx512|avx10'
         r'|vml_(?:def|mc3|avx2|avx512|avx10|cmpt)')
KEEP = re.compile(rf'(?:libmkl_(?:{PARTS})|libtbb|libtbbmalloc|libtcm)\.so\.\d+'
                  rf'|(?:mkl_(?:{PARTS})\.\d+|tbb\d+|tbbmalloc|tcm)\.dll')
LIBRARY = re.compile(r'.*\.(?:so(?:\.\d+)*|dll)$')
removed = kept = 0
for name in ('mkl', 'tbb', 'tcmlib'):
    try:
        files = distribution(name).files or ()
    except PackageNotFoundError:
        continue
    for entry in files:
        path = Path(entry.locate())
        if not path.is_file() or not LIBRARY.match(path.name.lower()):
            continue
        if KEEP.fullmatch(path.name.lower()):
            kept += 1
        else:
            path.unlink()
            removed += 1
print(f'bundle_python: oneMKL trimmed to what PARDISO loads: {kept} libraries kept, '
      f'{removed} removed')
TRIM
fi

# The solver the service will factor with, from the bundle itself: Accelerate on a
# Mac, PARDISO on x86-64 Windows and Linux. A bundle that fell back to SuperLU would
# still pass every test and solve a large model many times slower.
"$PY" -m hertzforge check-solver

# What the service never uses: the installer, headers, command scripts, the test
# suites, the Tk GUI kit, and the dbm module (whose library on some platforms carries
# a license this project does not take; `dbm` falls back to its pure-Python form).
if [[ $EXE == python.exe ]]; then
  LIB="$OUT/python/Lib"; DYNLOAD="$OUT/python/DLLs"
else
  LIB="$OUT/python/lib/python$PY_MINOR"; DYNLOAD="$LIB/lib-dynload"
fi
SITE="$LIB/site-packages"
"$PY" -m pip uninstall -q -y pip
rm -rf "$LIB/ensurepip" "$LIB/idlelib" "$LIB/tkinter" "$LIB/turtledemo" "$LIB/test" \
  "$LIB/lib2to3" "$OUT/python/tcl" "$OUT/python/include" "$OUT/python/share" \
  "$OUT/python/Scripts" "$OUT/python/libs" "$DYNLOAD"/_tkinter* "$DYNLOAD"/_dbm* \
  "$DYNLOAD"/tcl* "$DYNLOAD"/tk*
if [[ $EXE == bin/python3 ]]; then
  # Tcl/Tk and its thread extension sit in lib/ beside the standard library's own
  # folder. Never on Windows: there lib/ IS the standard library (Lib/, and the file
  # system ignores case), and lib/thread* took threading.py with it (2026-11-02).
  rm -rf "$OUT/python/lib/"tcl* "$OUT/python/lib/"tk* "$OUT/python/lib/"libtcl* \
    "$OUT/python/lib/"libtk* "$OUT/python/lib/"itcl* "$OUT/python/lib/"thread* \
    "$OUT/python/lib/pkgconfig"
  # The unversioned libpythonX.Y.so is a link for linking extensions at build time;
  # the interpreter loads libpythonX.Y.so.1.0 by its versioned name.
  find "$OUT/python/lib" -maxdepth 1 -type l -name 'libpython*.so' -delete
fi
if [[ $EXE == bin/python3 ]]; then
  # One interpreter file and no links (an installer copies a link as a second copy,
  # or not at all): bin/python3, the file bin/python3.N pointed at.
  real="$(cd "$OUT/python/bin" && readlink python3 || echo python3)"
  mv "$OUT/python/bin/$real" "$WORK/python3"
  rm -rf "$OUT/python/bin"
  mkdir "$OUT/python/bin"
  mv "$WORK/python3" "$OUT/python/bin/python3"
fi
if [[ -n "$(find "$OUT/python" -type l | head -1)" ]]; then
  echo "bundle_python: links left in the runtime:" >&2
  find "$OUT/python" -type l >&2
  exit 1
fi
# (`numpy.testing` and `pandas._testing` stay: they are imported by the libraries.)
for pkg in numpy scipy pandas h5py netCDF4 pint openpyxl; do
  [[ -d "$SITE/$pkg" ]] && find "$SITE/$pkg" -depth -type d -name tests -exec rm -rf {} +
done
find "$OUT/python" -type d -name __pycache__ -prune -exec rm -rf {} +
# The trimmed runtime still starts the service: an over-eager removal fails here, by
# name, rather than as a missing module deep in a later step.
"$PY" -I -B -c 'import threading, logging, concurrent.futures, hertzforge.service'
# Bytecode for everything, checked by hash and not by the source's time stamp: the
# installers copy files with new times, and the app starts Python with -B, so a
# bundle that is read only (and later signed) never has bytecode written into it.
"$PY" -m compileall -q -j 0 --invalidation-mode unchecked-hash "$LIB" >/dev/null

# The plate test data the demonstration datasets read (the app points
# HERTZFORGE_TESTDATA here).
if [[ ! -f "$ROOT/testdata/plate/random.nc4" ]]; then
  bash "$ROOT/scripts/fetch_testdata.sh"
fi
mkdir -p "$OUT/testdata"
cp -R "$ROOT/testdata/plate" "$OUT/testdata/plate"

# Proof it runs from where it is: the package imports and says its version.
"$PY" -I -B -c "import hertzforge, hertzforge.service.server as s; s.preload(); print('hertzforge', hertzforge.__version__, 'imports')"
echo "bundle_python: $(du -sh "$OUT/python" | cut -f1) runtime, $(du -sh "$OUT/testdata" | cut -f1) test data, in $OUT"
