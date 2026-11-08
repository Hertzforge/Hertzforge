# A Multiple-Output Control Law, From Copy to Hardware

This folder is a complete multiple-output Random control law: the open-loop pseudo-inverse,
the same method as the built-in `pinv_open`, written out as the example to start from. Copy
the folder, rename the package in `Cargo.toml`, and change `src/law.rs`.

## Build It

You need Rust with the WebAssembly target (`rustup target add wasm32-wasip2`). In this folder:

```
cargo build --target wasm32-wasip2 --release
```

The law file is `target/wasm32-wasip2/release/my_mimo_law.wasm` (the package name, with
underscores).

## Check It

Before a law may drive hardware it has to pass the check: it runs on six simulated test
articles (square and non-square modal plants, an ill-conditioned plant, a rigid plate, a plant
with measurement noise, a level ladder from -20 dB) under the controller's own guards. Any law
runs on the simulator; only a law whose exact file passed may arm on hardware.

In the application, import the file from the law picker and choose "Check before hardware".
From a checkout of the repository, the same check runs from the command line:

```
cargo run --release -p sds-control --example law_check -- \
    crates/hertzforge-law-sdk/template/mimo-open-loop/target/wasm32-wasip2/release/my_mimo_law.wasm
```

It imports the file into the local data folder, prints each case, records the result against
the file's SHA-256, and says whether the file is cleared for hardware. Rebuilding the law
changes its hash, so a changed law is checked again.

## What to Change

- `describe()`: the name, version, author, summary, the parameters (each becomes an input box),
  the shapes the law supports, the optional inputs it reads (`needs`) and its diagnostics.
- `new()`: read the parameters; refuse a setup the law cannot run.
- `seed()`: the first drive, after identification.
- `iterate()`: the drive for each loop at full level.

The full interface, every input and output and the controller's checks, is in
`docs/control-law-spec.md`; the guide is `docs/control-laws.md`.
