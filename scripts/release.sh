#!/bin/bash
# Release Hertzforge in one command, start to finish:
#
#     scripts/release.sh 0.1.17 notes.md
#
# notes.md is the version pull request's body: what the release brings, by
# pull request number. The script
#
#   1. bumps the version in every file that states it (scripts/bump_version.py)
#      on a branch tooling/version-X.Y.Z cut from origin/main, commits it and
#      pushes it to $PUSH_REMOTE (default `fork`);
#   2. opens the pull request "Version X.Y.Z" against origin's main, reruns a
#      failed job twice at most, and merges it (--admin) once every one of the
#      $REQUIRED_CHECKS required checks (default 17) has passed;
#   3. signs the tag vX.Y.Z on the merged main and pushes it to origin, which
#      starts the desktop workflow (build, sign, notarize, publish the release);
#   4. watches that run, rerunning its failed jobs twice at most (a Mac runner's
#      `hdiutil: Resource busy` fails a job now and then and passes on a rerun);
#   5. waits for the release workflow's PyPI publish and checks the release is
#      published with its 11 assets and PyPI serves the version.
#
# It refuses uncommitted work in this checkout, a version that is not after the
# current one, and a tag that already exists. Each step says what it is doing; a
# step that cannot finish stops the script with the reason, and running it again
# picks up where it stopped (an open pull request, a merge done, a tag pushed).
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=${1:?usage: scripts/release.sh X.Y.Z notes.md}
NOTES=${2:?usage: scripts/release.sh X.Y.Z notes.md}
REPO=${REPO:-Hertzforge/Hertzforge}
PUSH_REMOTE=${PUSH_REMOTE:-fork}
REQUIRED_CHECKS=${REQUIRED_CHECKS:-17}
ASSETS=${ASSETS:-11}
TAG=v$VERSION
BRANCH=tooling/version-$VERSION
TITLE="Version $VERSION"

say() { printf '\n== %s\n' "$*"; }
die() { printf 'release: %s\n' "$*" >&2; exit 1; }

[ -f "$NOTES" ] || die "no notes file at $NOTES"
[ -z "$(git status --porcelain --untracked-files=no)" ] || die 'this checkout has uncommitted changes'
git fetch -q origin --tags
git rev-parse -q --verify "refs/tags/$TAG" >/dev/null && ! git ls-remote --exit-code origin "refs/tags/$TAG" >/dev/null 2>&1 \
    && die "$TAG exists here but not on origin; delete it or push it"

# ---- 1 and 2: the version pull request, merged ----------------------------------------
merged_version=$(git show origin/main:pyproject.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
python3 -c "import sys; a, b = (tuple(map(int, v.split('.'))) for v in sys.argv[1:]); sys.exit(a < b)" \
    "$VERSION" "$merged_version" || die "$VERSION is before $merged_version, the version on origin/main"
if [ "$merged_version" != "$VERSION" ]; then
    pr=$(gh pr list --repo "$REPO" --head "$BRANCH" --state open --json number --jq '.[0].number // empty')
    if [ -z "$pr" ]; then
        say "bumping to $VERSION on $BRANCH"
        git checkout -q -B "$BRANCH" origin/main
        python3 scripts/bump_version.py "$VERSION"
        git commit -qam "$TITLE"
        git push -q -f "$PUSH_REMOTE" "$BRANCH"
        owner=$(git remote get-url "$PUSH_REMOTE" | sed -E 's#.*[:/]([^/]+)/[^/]+(\.git)?$#\1#')
        gh pr create --repo "$REPO" --base main --head "$owner:$BRANCH" --title "$TITLE" --body-file "$NOTES"
        pr=$(gh pr list --repo "$REPO" --head "$BRANCH" --state open --json number --jq '.[0].number')
        sleep 60   # right after a push GitHub reports no checks at all, which is not a pass
    fi
    say "waiting for the checks on pull request $pr"
    reruns=0
    while :; do
        [ "$(gh pr view "$pr" --repo "$REPO" --json state --jq .state)" = MERGED ] && break
        checks=$(gh pr checks "$pr" --repo "$REPO" 2>/dev/null || true)
        pass=$(printf '%s\n' "$checks" | awk -F'\t' '$2=="pass"' | wc -l | tr -d ' ')
        pend=$(printf '%s\n' "$checks" | awk -F'\t' '$2=="pending"' | wc -l | tr -d ' ')
        fail=$(printf '%s\n' "$checks" | awk -F'\t' '$2=="fail"' | wc -l | tr -d ' ')
        if [ "$fail" -gt 0 ]; then
            [ "$reruns" -ge 2 ] && die "pull request $pr still fails after $reruns reruns: $(printf '%s\n' "$checks" | awk -F'\t' '$2=="fail" {print $1}' | tr '\n' ' ')"
            for run in $(gh pr checks "$pr" --repo "$REPO" --json state,link --jq '.[] | select(.state=="FAILURE") | .link' \
                         | sed -E 's#.*/runs/([0-9]+)/.*#\1#' | sort -u); do
                while [ "$(gh run view "$run" --repo "$REPO" --json status --jq .status)" != completed ]; do sleep 30; done
                gh run rerun "$run" --repo "$REPO" --failed && echo "reran run $run"
            done
            reruns=$((reruns + 1)); sleep 90; continue
        fi
        if [ "$pend" -eq 0 ] && [ "$pass" -ge "$REQUIRED_CHECKS" ]; then
            gh pr merge "$pr" --repo "$REPO" --merge --admin && echo "merged $pr"
            sleep 10; continue
        fi
        sleep 30
    done
    git fetch -q origin
fi
merged_version=$(git show origin/main:pyproject.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
[ "$merged_version" = "$VERSION" ] || die "origin/main says $merged_version, not $VERSION"

# ---- 3: the signed tag ------------------------------------------------------------------
if ! git ls-remote --exit-code origin "refs/tags/$TAG" >/dev/null 2>&1; then
    say "signing $TAG on origin/main and pushing it"
    git tag -s "$TAG" -m "Hertzforge $VERSION" origin/main
    git push -q origin "$TAG"
    sleep 30
fi

# ---- 4: the desktop build, rerun where a job fails --------------------------------------
run_of() {   # the newest run of workflow $1 for the tag
    gh run list --repo "$REPO" --workflow "$1" --limit 20 --json databaseId,headBranch \
        --jq "[.[] | select(.headBranch==\"$TAG\")][0].databaseId // empty"
}
finished() {   # wait for run $1 to finish, through the API's passing errors; print its conclusion
    local status
    while :; do
        status=$(gh run view "$1" --repo "$REPO" --json status,conclusion --jq '.status + " " + .conclusion' 2>/dev/null || echo 'unknown ')
        case $status in completed\ *) echo "${status#completed }"; return ;; esac
        sleep 60
    done
}
desktop=''
for _ in $(seq 1 20); do desktop=$(run_of desktop.yml); [ -n "$desktop" ] && break; sleep 15; done
[ -n "$desktop" ] || die "no desktop run started for $TAG"
say "watching the desktop run $desktop"
for attempt in 0 1 2; do
    conclusion=$(finished "$desktop")
    [ "$conclusion" = success ] && break
    [ "$attempt" -eq 2 ] && die "the desktop run $desktop ends $conclusion after two reruns"
    echo "desktop run $conclusion; rerunning its failed jobs"
    gh run rerun "$desktop" --repo "$REPO" --failed
    sleep 30
done

# ---- 5: the PyPI publish and the checks -------------------------------------------------
publish=''
for _ in $(seq 1 40); do publish=$(run_of release.yml); [ -n "$publish" ] && break; sleep 15; done
[ -n "$publish" ] || die "no release workflow run started for $TAG"
say "waiting for the PyPI publish, run $publish"
conclusion=$(finished "$publish")
[ "$conclusion" = success ] || die "the PyPI publish run $publish ends $conclusion"

say "checking what was published"
read -r draft assets < <(gh release view "$TAG" --repo "$REPO" --json isDraft,assets --jq '"\(.isDraft) \(.assets | length)"')
[ "$draft" = false ] || die "the $TAG release is still a draft"
[ "$assets" -eq "$ASSETS" ] || die "the $TAG release has $assets assets, not $ASSETS"
for _ in $(seq 1 20); do
    code=$(curl -s -o /dev/null -w '%{http_code}' "https://pypi.org/pypi/hertzforge/$VERSION/json")
    [ "$code" = 200 ] && break
    sleep 15
done
[ "$code" = 200 ] || die "PyPI answers $code for hertzforge $VERSION"
echo "Hertzforge $VERSION released: $TAG signed, the release published with $assets assets, PyPI serves $VERSION"
