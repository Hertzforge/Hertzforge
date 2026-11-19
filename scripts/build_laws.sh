#!/usr/bin/env bash
# Build every guest law to a WebAssembly component, in release:
#
#   laws/*                                  control laws
#   crates/hertzforge-law-host/guests/*     the law host's test fixtures
#
# Each lands at <target>/wasm32-wasip2/release/<crate_name>.wasm, where
# <target> is cargo's target directory (CARGO_TARGET_DIR when it is set). The
# law host's tests read the files from there and fail, naming this script,
# when one is missing. scripts/rust_tests.sh runs this first.
#
# Anything given is passed to `cargo build`. The target comes from
# rust-toolchain.toml; `rustup target add wasm32-wasip2` installs it by hand.
#
# Written for bash 3.2, the bash macOS ships.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

pkgs=()
n=0
for manifest in laws/*/Cargo.toml crates/hertzforge-law-host/guests/*/Cargo.toml; do
  [ -f "$manifest" ] || continue
  name=$(sed -e 's/\r$//' "$manifest" | sed -n 's/^name = "\(.*\)"$/\1/p' | head -n 1)
  if [ -z "$name" ]; then
    echo "build_laws: no package name in $manifest" >&2
    exit 1
  fi
  pkgs+=(-p "$name")
  n=$((n + 1))
done
if [ "$n" -eq 0 ]; then
  echo "build_laws: no guest crates under laws/ or crates/hertzforge-law-host/guests/" >&2
  exit 1
fi

echo "build_laws: $n guest crates for wasm32-wasip2"
cargo build --locked --release --target wasm32-wasip2 "${pkgs[@]}" "$@" </dev/null
