#!/usr/bin/env bash
# Run the Rust workspace's tests in the two passes CI runs:
#
#   scripts/rust_tests.sh parallel   every test except the timing tests, at
#                                    cargo's normal parallelism
#   scripts/rust_tests.sh timing     the timing tests, one at a time
#   scripts/rust_tests.sh [all]      both, in that order
#
# Anything after the pass name goes to `cargo test` before its `--`, such as
# `--no-fail-fast` or `--release`.
#
# The timing tests are listed in scripts/rust_timing_tests.txt, which says
# what belongs there. Each pass checks its count against that list, so an
# entry that no longer names exactly one test (renamed, removed, listed under
# the wrong target, or a name another test binary shares) fails the run
# instead of leaving a test that neither pass runs.
#
# Written for bash 3.2, the bash macOS ships.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LIST="$ROOT/scripts/rust_timing_tests.txt"
cd "$ROOT" || exit 1

fail() {
  echo "rust_tests: $*" >&2
  exit 1
}

# The list as "package target test" lines: comments, blank lines and carriage
# returns (a Windows checkout) dropped. A line that is not three fields is an
# error, not something to guess at.
entries() {
  sed -e 's/\r$//' -e 's/#.*//' "$LIST" | awk '
    NF == 3 { print $1, $2, $3; next }
    NF != 0 { print "malformed line " NR ": " $0 > "/dev/stderr"; bad = 1 }
    END { exit bad }'
}

# Sum one count ("passed", "failed", "ignored" or "filtered out") over every
# "test result:" line of a cargo test log.
total() {
  sed -e 's/\r$//' "$2" | grep '^test result:' |
    sed -E "s/.* ([0-9]+) $1.*/\\1/" | awk '{ s += $1 } END { print s + 0 }'
}

entries >/dev/null || fail "fix $LIST"
LOG="$(mktemp)"
trap 'rm -f "$LOG"' EXIT

parallel() {
  local skips=() n=0 pkg target name
  while read -r pkg target name; do
    skips+=(--skip "$name")
    n=$((n + 1))
  done < <(entries)
  [ "$n" -gt 0 ] || fail "$LIST lists no tests"
  echo "rust_tests: the workspace, without the $n timing tests"
  cargo test --workspace --locked ${CARGO_ARGS[@]+"${CARGO_ARGS[@]}"} -- --exact "${skips[@]}" \
    </dev/null 2>&1 | tee "$LOG"
  local status=${PIPESTATUS[0]}
  [ "$status" -eq 0 ] || return "$status"
  local skipped
  skipped=$(total "filtered out" "$LOG")
  [ "$skipped" -eq "$n" ] ||
    fail "the list names $n timing tests but $skipped were left out of this pass;" \
      "an entry no longer names exactly one test in the workspace"
  echo "rust_tests: $(total passed "$LOG") passed, $(total ignored "$LOG") ignored," \
    "$skipped left for the timing pass"
}

timing() {
  local status=0 n=0 passed=0 failed=0 ignored=0 broken="" pkg target name names sel s p f i
  # One cargo invocation per package and test target, in the order listed. A
  # failing target does not stop the ones after it.
  while read -r pkg target <&3; do
    names=()
    while read -r name; do
      names+=("$name")
    done < <(entries | awk -v p="$pkg" -v t="$target" '$1 == p && $2 == t { print $3 }')
    case "$target" in
      lib) sel=(--lib) ;;
      test:*) sel=(--test "${target#test:}") ;;
      bin:*) sel=(--bin "${target#bin:}") ;;
      *) fail "unknown target '$target' for $pkg in $LIST (lib, test:NAME or bin:NAME)" ;;
    esac
    echo "rust_tests: ${#names[@]} timing tests in $pkg ${sel[*]}, one at a time"
    cargo test --locked -p "$pkg" "${sel[@]}" ${CARGO_ARGS[@]+"${CARGO_ARGS[@]}"} \
      -- --exact --test-threads=1 "${names[@]}" </dev/null 2>&1 | tee "$LOG"
    s=${PIPESTATUS[0]}
    p=$(total passed "$LOG")
    f=$(total failed "$LOG")
    i=$(total ignored "$LOG")
    if [ "$s" -ne 0 ]; then
      status=$s
      broken="$broken
  $pkg ${sel[*]}: exit $s"
    elif [ $((p + f + i)) -ne "${#names[@]}" ]; then
      status=1
      broken="$broken
  $pkg ${sel[*]}: the list names ${#names[@]} tests, $((p + f + i)) ran"
    fi
    n=$((n + ${#names[@]}))
    passed=$((passed + p))
    failed=$((failed + f))
    ignored=$((ignored + i))
  done 3< <(entries | awk '!seen[$1 " " $2]++ { print $1, $2 }')
  echo "rust_tests: the $n timing tests: $passed passed, $failed failed, $ignored ignored"
  [ -z "$broken" ] || echo "rust_tests: failed:$broken" >&2
  return "$status"
}

PASS="${1:-all}"
[ $# -eq 0 ] || shift
CARGO_ARGS=("$@")
case "$PASS" in
  parallel) parallel ;;
  timing) timing ;;
  all) parallel && timing ;;
  *) fail "usage: scripts/rust_tests.sh [parallel|timing] [cargo test options]" ;;
esac
