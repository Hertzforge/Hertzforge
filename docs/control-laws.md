# Control laws

A control law is the math the controller runs once per loop to decide the next drive. Hertzforge
loads control laws from files, so a law can be written, built and shared without rebuilding
Hertzforge. This page is a guide for someone writing one, for single-output Random, the first test
type with a law interface. The requirements a law must meet, what it is given and what it must
return are in the specification, [`control-law-spec.md`](control-law-spec.md); where this guide
and the specification differ, the specification governs.

## What a law is

A law file is a WebAssembly component that implements the `random-single` world of the interface
package `hertzforge:control-law@0.1.0`, written in WIT, WebAssembly's interface language, in
[`wit/control-law/`](../wit/control-law/). One law file runs on every platform Hertzforge runs on.

A law has four parts:

- **A manifest**, returned by `describe`: the law's name, version, author, a summary, the
  parameters it takes and the diagnostics it reports.
- **A constructor**, called once per run with the setup and the parameter values. It may refuse
  the run with a reason.
- **`seed` and `iterate`**. `seed` is called once with the system-identification measurement and
  returns the starting drive. `iterate` is called once per control loop with the latest
  measurement and returns the next drive.
- **`retarget-level`**, called when the scheduled level moves, with the new level in dB and
  the target's in-band RMS at that level. It returns the drive at the new level.

The law keeps whatever state it needs between calls. Calls never overlap.

## What the controller guards, and what the law does not

The law proposes the drive; the controller decides what is played. The drive ceiling and abort,
limit notching, the envelope alarm and abort, divergence, the cap on how far the drive may change
in one loop, the checks on every output and the time budget on every call all belong to the
controller and apply to every law, built in or imported. A law's own diagnostics, including the
lines it calls divergent, are shown and recorded but never relied on, so a law that reports
nothing is no less safe than one that reports everything. The specification lists each guard and
each output rule.

A law runs in a sandbox and reaches nothing but the numbers it is handed: no files, no network,
no environment, a clock that does not move and random sources with fixed seeds, under a memory cap.
A law that traps, runs past its time budget or fails past its memory cap is stopped and not called
again. Standard output and standard error are kept in a small buffer, and what a law wrote to
standard error during a call that failed is shown with the failure.

## Parameters

A law declares its parameters in its manifest, and the parameter form is drawn from the
declaration: a law ships no user interface. Every value is checked against its declaration before
the law sees it, and the law always receives every parameter it declared, in the order it declared
them, with defaults for the ones not given. Keys are lower-case letters, digits and underscores
starting with a letter; saved test setups refer to them, so a key should not change once a law is
in use. The kinds:

| Kind | The operator enters | Declared with |
|---|---|---|
| `number` | a real number | unit, minimum, maximum, step, default |
| `integer` | a whole number | unit, minimum, maximum, step, default |
| `toggle` | on or off | default |
| `choice` | one of a set of named options | the options, the default option |
| `curve` | a value against frequency, as breakpoints | unit, minimum and maximum value, default breakpoints |
| `per-channel` | one number per control channel | unit, minimum, maximum, step, default |

The step is a hint for the input box. A curve's interpolation between breakpoints is the law's
choice, and its help text should say which. Diagnostics are declared with a key, a label, a unit
and help text, and are reported as numbers.

## Writing a law in Rust

Rust is the supported language. Other languages that build WebAssembly components can implement
the same interface from the WIT files, but are not documented or tested here.

A law is a library crate that depends on `hertzforge-law-sdk`, implements its `RandomLaw` trait
and exports it with `export_law!`:

```toml
[package]
name = "my-law"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
hertzforge-law-sdk = { path = "../hertzforge/crates/hertzforge-law-sdk" }
```

```rust
use hertzforge_law_sdk::*;

pub struct MyLaw {
    // the law's state
}

impl RandomLaw for MyLaw {
    fn describe() -> Manifest { /* name, version, parameters, diagnostics */ }
    fn new(setup: Setup, params: Params) -> Result<Self, String> { /* read params.number("gain") ... */ }
    fn seed(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, String> { /* starting drive */ }
    fn iterate(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, String> { /* next drive */ }
    fn retarget_level(&mut self, level_db: f64, target_rms: f64) -> Result<LoopOutputs, String> {
        /* the drive at the new level */
    }
}

export_law!(MyLaw);
```

Build it with the WebAssembly target (once: `rustup target add wasm32-wasip2`):

```sh
cargo build --target wasm32-wasip2 --release
```

The law file is `target/wasm32-wasip2/release/my_law.wasm`.

`Params` has a getter for each kind (`number`, `integer`, `toggle`, `choice`, `curve`,
`per_channel`), and the SDK has three small helpers: `band_range`, `inband_rms` and
`no_diagnostics`. Because `RandomLaw` is an ordinary trait, a law's math can be tested natively
with `cargo test` by calling its methods directly, before it is ever built to WebAssembly.

[`laws/ratio/`](../laws/ratio/) is a complete law to start from: per-line ratio correction. Each
in-band line whose coherence is at or above a floor moves its drive by a fraction (the gain) of the
measured error in dB, limited to a largest step per loop; lines below the floor hold their drive.
It declares three number parameters and two diagnostics. Its math is a plain Rust crate,
[`crates/hertzforge-law-ratio-math`](../crates/hertzforge-law-ratio-math/), and `laws/ratio` only
carries values across the interface: a pattern worth copying, because the math can then be used and
tested anywhere.

## Built-in laws

Hertzforge has two single-output Random laws built in: `hinv`, the default (an H-inverse starting
drive, a level servo and a per-line shape trim), and `ratio`, the law above. Each is described by
the same kind of manifest a law file declares, so the form for a built-in law is drawn the way it is
for an imported one. Each also exists as a law file built from the same code
([`laws/hinv/`](../laws/hinv/), [`laws/ratio/`](../laws/ratio/)), and the tests run each law both
ways through a whole test on the simulator and hold the two to the same result.

## Importing a law

A law file is imported from a path. Hertzforge loads it (which checks that it is a single-output
Random law with a sound manifest) and copies it into the `laws` folder of the local data root,
named by the SHA-256 of its bytes: `laws/<sha256>.wasm`. The data root is the folder Hertzforge
keeps its runs in, on the local disk.

The simulator runs any law. Hardware runs a built-in law, or an imported law only when a check
record for its exact file says it passed: `laws/<sha256>.check.json`, beside the law file. The
record names the file by its hash, so changing a single byte of a law file sends it back to the
simulator.

## Responsibility for imported laws

Hertzforge stands behind the controller's protections and the laws it ships. A law imported from a
file is its author's work and its user's choice: Hertzforge checks that the law's output is a
well-formed drive and keeps the drive inside the controller's limits, but does not check that a law
controls well. Run a new law on the simulator before it drives hardware.

Laws are separate files loaded at run time, so their authors can license them as they choose.

## The interface version

The interface is `hertzforge:control-law@0.1.0`. While it is at 0.x it may change between
releases, and a law must be built against the version the host loads. The
`random-mimo` world in the same package is a draft for multiple-output Random, marked unstable in
its file. Nothing loads it yet, and it will change.

## Building and testing in this repository

The workspace holds the law host (`crates/hertzforge-law-host`), the SDK
(`crates/hertzforge-law-sdk`), the laws (`laws/*`), the host's test fixtures
(`crates/hertzforge-law-host/guests/*`): small laws that trap, loop forever, grow memory, try to
open files, sleep or return a bad drive on purpose, and the Random law catalog
(`crates/hertzforge-random-laws`), which lists the built-in and imported laws, resolves the
operator's choice of law, and runs a law file as a Random control law under the controller's
guards. Its tests run the misbehaving fixtures through a whole Random run on the simulator and
show each ends in the controller's controlled stop. The laws and fixtures are workspace members,
so `cargo clippy --workspace` and `cargo test --workspace` build and check them natively, and the
one lockfile and `cargo deny` cover their dependencies.

`scripts/build_laws.sh` builds every law and fixture to WebAssembly into
`<target>/wasm32-wasip2/release/`. `scripts/rust_tests.sh` runs it before the tests, and the host's
tests read the files from there. A test that needs a law file that has not been built fails and
names the script; none is skipped.
