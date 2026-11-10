#!/usr/bin/env bash
# Run Hertzforge from this checkout in the app's own window, without packaging it.
#
#     scripts/dev_app.sh
#
# The window, menus and file dialogs are the real app's (Tauri's development mode); the
# shell comes from the development server, so a change saved under ui/ shows at once;
# and both services run from the checkout. A change to the Python package or to the
# control service takes effect the next time this runs: quit the app and run it again.
# The first run builds the control service and the app, a few minutes; later runs
# rebuild only what changed.
#
# The analysis service runs on HERTZFORGE_DEV_PYTHON, or else on the `python3` this
# folder gets, which must import hertzforge from this checkout's src/
# (`pip install -e .`).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
PORT=5191

# rustup from Homebrew puts no cargo on the PATH
if ! command -v cargo >/dev/null && command -v rustup >/dev/null; then
  PATH="$(dirname "$(rustup which cargo)"):$PATH"
fi

PY="${HERTZFORGE_DEV_PYTHON:-$(command -v python3)}"
# the interpreter itself: a pyenv shim chooses its Python by the folder it runs in, and
# the app starts the service from another folder
PY="$("$PY" -c 'import sys; print(sys.executable)')"
FROM="$("$PY" -I -c 'import hertzforge, pathlib; print(pathlib.Path(hertzforge.__file__).resolve().parent.parent)' 2>/dev/null || true)"
if [[ "$FROM" != "$ROOT/src" ]]; then
  echo "dev_app.sh: $PY does not import hertzforge from $ROOT/src${FROM:+ (it imports it from $FROM)}." >&2
  echo "dev_app.sh: install it for development there (pip install -e \"$ROOT\"), or name another Python in HERTZFORGE_DEV_PYTHON." >&2
  exit 1
fi

echo "dev_app.sh: the control service"
cargo build --release --locked -p sds-control
CONTROL="${CARGO_TARGET_DIR:-$ROOT/target}/release/sds-control"

[[ -d ui/node_modules ]] || npm --prefix ui ci --no-audit --no-fund
[[ -d desktop/node_modules ]] || npm --prefix desktop ci --no-audit --no-fund
[[ -f desktop/icons/icon.icns ]] || npm --prefix desktop run icons >/dev/null

# the shell's development server, unless one is already serving
if ! nc -z 127.0.0.1 "$PORT" 2>/dev/null; then
  echo "dev_app.sh: the shell's development server on port $PORT"
  npm --prefix ui run dev -- --port "$PORT" --strictPort >"${TMPDIR:-/tmp}/hertzforge-dev-shell.log" 2>&1 &
  SHELL_PID=$!
  trap 'kill "$SHELL_PID" 2>/dev/null || true' EXIT
  for _ in $(seq 1 120); do
    nc -z 127.0.0.1 "$PORT" 2>/dev/null && break
    kill -0 "$SHELL_PID" 2>/dev/null || { cat "${TMPDIR:-/tmp}/hertzforge-dev-shell.log" >&2; exit 1; }
    sleep 0.5
  done
fi

echo "dev_app.sh: the app, with its services from $ROOT"
export HERTZFORGE_DEV_PYTHON="$PY" HERTZFORGE_DEV_CONTROL="$CONTROL"
cd desktop && npx --no-install tauri dev
