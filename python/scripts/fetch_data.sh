#!/usr/bin/env bash
# Extract the visualdynamics plate test data at the pinned commit into
# python/.data/vd. From a local checkout when it holds the commit (read
# only: no checkout, no fetch), otherwise from GitHub's archive of that
# commit, which is how CI gets it.
#
#   python/scripts/fetch_data.sh            # from anywhere
#   VD_REPO=/path/to/visualdynamics python/scripts/fetch_data.sh
set -euo pipefail
COMMIT=38cc476f7ef38fe0dae5ed4545378c7b2f954727
PYTHON_DIR="$(cd "$(dirname "$0")/.." && pwd)"
VD_REPO="${VD_REPO:-$HOME/dev/visualdynamics}"
DEST="$PYTHON_DIR/.data/vd"
mkdir -p "$DEST"
if git -C "$VD_REPO" cat-file -e "$COMMIT^{commit}" 2>/dev/null; then
    git -C "$VD_REPO" archive "$COMMIT" testdata/plate | tar -x -C "$DEST"
else
    URL="https://codeload.github.com/visualdynamics/visualdynamics/tar.gz/$COMMIT"
    curl -fsSL "$URL" | tar -xz -C "$DEST" --strip-components=1 "visualdynamics-$COMMIT/testdata/plate"
fi
echo "fetch_data: plate test data at $DEST/testdata/plate"
