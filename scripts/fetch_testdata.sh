#!/usr/bin/env bash
# Fetch the plate test data (55 MB of runs and models) into testdata/plate,
# from Visual Dynamics' public repository at the release its code was copied
# from. Too large for this repository's history, so it is fetched, not
# committed. From a local checkout when it holds the tag (read only: no
# checkout, no fetch), otherwise from GitHub's archive of the tag.
#
#   scripts/fetch_testdata.sh
#   VD_REPO=/path/to/visualdynamics scripts/fetch_testdata.sh
set -euo pipefail
TAG=v0.1.0a82
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VD_REPO="${VD_REPO:-$HOME/visualdynamics-shared}"
DEST="$ROOT/testdata"
if git -C "$VD_REPO" rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
    git -C "$VD_REPO" archive "$TAG" testdata/plate | tar -x -C "$ROOT"
else
    URL="https://codeload.github.com/visualdynamics/visualdynamics/tar.gz/refs/tags/$TAG"
    curl -fsSL "$URL" | tar -xz -C "$ROOT" --strip-components=1 "visualdynamics-${TAG#v}/testdata/plate"
fi
echo "fetch_testdata: plate test data at $DEST/plate"
