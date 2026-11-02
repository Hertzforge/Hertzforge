#!/usr/bin/env bash
# Start the analysis service, the control service and the shell together.
# The analysis service is the hertzforge package (pip install -e . first).
# Ctrl-C stops all three.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
trap 'kill 0' EXIT INT TERM
(cd "$ROOT" && hertzforge serve --port 8791) &
(cd "$ROOT" && cargo run -q -p sds-control -- --port 8792) &
npm --prefix ui run dev -- --port 5191 --strictPort
