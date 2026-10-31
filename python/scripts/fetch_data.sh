#!/usr/bin/env bash
# Extract the visualdynamics plate test data at the pinned commit into
# python/.data/vd. Reads the local repository without changing it (no
# checkout, no fetch): the commit must already be present there.
#
#   python/scripts/fetch_data.sh            # from anywhere
#   VD_REPO=/path/to/visualdynamics python/scripts/fetch_data.sh
set -euo pipefail
COMMIT=38cc476
PYTHON_DIR="$(cd "$(dirname "$0")/.." && pwd)"
VD_REPO="${VD_REPO:-$HOME/dev/visualdynamics}"
DEST="$PYTHON_DIR/.data/vd"
if ! git -C "$VD_REPO" cat-file -e "$COMMIT^{commit}" 2>/dev/null; then
    echo "fetch_data: $VD_REPO has no commit $COMMIT; fetch upstream main there first" >&2
    exit 1
fi
mkdir -p "$DEST"
git -C "$VD_REPO" archive "$COMMIT" testdata/plate | tar -x -C "$DEST"
echo "fetch_data: plate test data at $DEST/testdata/plate"
