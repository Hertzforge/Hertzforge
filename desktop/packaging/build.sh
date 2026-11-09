#!/usr/bin/env bash
# Build the Hertzforge desktop app and its installers for this machine.
#
#     desktop/packaging/build.sh [--bundles <list>] [--e2e]
#
# 1. the shell (ui/, a static build the app serves to its window);
# 2. the control service, release (`sds-control`, shipped beside the app);
# 3. the Python runtime with the hertzforge package, and the plate test data
#    (packaging/bundle_python.sh), shipped as resources;
# 4. the app and its installers (`tauri build`): on macOS the .app and a .dmg, on
#    Windows an NSIS installer, on Linux a .deb and an AppImage, unless --bundles
#    names others (comma-separated, as `tauri build --bundles` takes them).
#
# Build output goes under $CARGO_TARGET_DIR (default target/ in the checkout): the
# staged parts in desktop-stage/, the installers in release/bundle/. Set
# HERTZFORGE_KEEP_PYTHON=1 to reuse a staged runtime from an earlier build.
#
# --e2e builds with the `e2e` feature, for the local proof only (packaging/e2e/);
# never ship that build.
#
# Unsigned. packaging/sign_macos.sh is the macOS signing and notarization step
# (docs/desktop-app.md).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DESKTOP="$ROOT/desktop"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
STAGE="$TARGET_DIR/desktop-stage"
TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
BUNDLES=""
FEATURES=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --bundles) BUNDLES="$2"; shift 2 ;;
    --e2e) FEATURES=(--features e2e); shift ;;
    *) echo "build.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
EXE=""
[[ "$TRIPLE" == *windows* ]] && EXE=".exe"
# Paths as the Tauri command line reads them (a Windows path under Git Bash).
native() { if command -v cygpath >/dev/null; then cygpath -m "$1"; else echo "$1"; fi; }

echo "build.sh: the shell"
[[ -d "$ROOT/ui/node_modules" ]] || npm --prefix "$ROOT/ui" ci --no-audit --no-fund
npm --prefix "$ROOT/ui" run build

echo "build.sh: the control service"
(cd "$ROOT" && cargo build --release --locked -p sds-control)
mkdir -p "$STAGE/bin"
cp "$TARGET_DIR/release/sds-control$EXE" "$STAGE/bin/sds-control-$TRIPLE$EXE"

if [[ -n "${HERTZFORGE_KEEP_PYTHON:-}" && -d "$STAGE/resources/python" ]]; then
  echo "build.sh: reusing the staged Python runtime"
else
  echo "build.sh: the Python runtime"
  bash "$DESKTOP/packaging/bundle_python.sh" "$STAGE/resources" "$TRIPLE"
fi

echo "build.sh: the app"
[[ -d "$DESKTOP/node_modules" ]] || npm --prefix "$DESKTOP" ci --no-audit --no-fund
npm --prefix "$DESKTOP" run icons >/dev/null 2>&1
S="$(native "$STAGE")"
cat > "$STAGE/tauri.stage.json" <<JSON
{
  "bundle": {
    "externalBin": ["$S/bin/sds-control"],
    "resources": {
      "$S/resources/python/": "python/"
    }
  }
}
JSON
args=(build --config "$(native "$STAGE/tauri.stage.json")" ${FEATURES[@]+"${FEATURES[@]}"})
[[ -n "$BUNDLES" ]] && args+=(--bundles "$BUNDLES")
# The AppImage's linuxdeploy follows every library in the app to what it needs, through
# ldd. oneMKL's kernels need its core beside them in the bundled Python's lib/, where ldd
# does not look, so it stops there. They are already in the bundle and load from it, so
# linuxdeploy is told to leave them alone rather than copy them a second time.
[[ "$TRIPLE" == *linux* ]] && export LINUXDEPLOY_EXCLUDED_LIBRARIES='libmkl_*;libtbb*;libtcm*'
# On a runner, what the bundlers print: linuxdeploy says nothing of why it failed otherwise.
[[ -n "${CI:-}" ]] && args+=(--verbose)
(cd "$DESKTOP" && npx --no-install tauri "${args[@]}")
echo "build.sh: installers in $TARGET_DIR/release/bundle/"
ls "$TARGET_DIR/release/bundle/"* 2>/dev/null || true
