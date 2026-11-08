# Control laws

A control law is the math the controller runs once per loop to decide the next drive. Hertzforge
loads control laws from files, so a law can be written, built and shared without rebuilding
Hertzforge. This page is a guide for someone writing one, for the test types with a law
interface: single-output Random, and multiple-output (MIMO) Random, classical shock and SRS shock,
which each have their own chapter below. The requirements a law must meet, what it is given and what it must return are in the
specification, [`control-law-spec.md`](control-law-spec.md); where this guide and the
specification differ, the specification governs.

## What a law is

A law file is a WebAssembly component that implements one world of the interface package
`hertzforge:control-law@0.1.0`, written in WIT, WebAssembly's interface language, in
[`wit/control-law/`](../wit/control-law/): `random-single` for single-output Random, or
`random-mimo` for multiple-output Random. One law file runs on every platform Hertzforge runs on.
The rest of this section, and the sections up to the MIMO chapter, describe single-output laws.

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

## MIMO Random laws

A multiple-output law decides the next drive spectral density matrix (SDM) for `n_d` drives so
that `n_c` control responses match a reference SDM: every control's power spectral density and the
cross spectral density, coherence and phase, of every pair. Its world is `random-mimo`, and the
specification's section 7 states it in full. What differs from a single-output law:

- **Matrices, one call for the whole grid.** The law gets the reference SDM (`n_c` by `n_c` per
  line), the measured response SDM, the plant estimate (the FRF, `n_c` by `n_d`), each control's
  multiple coherence and its alarm and abort lines, and returns the drive SDM (`n_d` by `n_d`). A
  spectral matrix is `E[X X^H]`, so a drive `G` gives the response `H G H^H`.
- **Full level.** The law works entirely at full level: the measured response has been referred to
  its full-level equivalent, and the drive it returns is the full-level drive. The controller owns
  the level schedule, so there is no `retarget-level`, and a law cannot raise the level.
- **A manifest with three more things:** the shapes the law supports (as many controls as drives,
  more, fewer), the optional inputs it needs (the last drive played, this loop's response-to-drive
  cross spectrum, the identification response, drive and noise floor; each is large, so the
  controller passes only the declared ones) and the layout of each diagnostic (a scalar, or a value
  per line, per control per line or per drive per line).
- **What is checked.** The host refuses a drive of the wrong size, with a value that is not finite,
  that is not Hermitian (an imaginary part on a diagonal), or that is not zero outside the band, and
  diagnostics that are not as declared. The controller then repairs a drive with a small negative
  eigenvalue (and counts it), refuses one with a large one, and applies its caps and limits by
  scalings that keep the matrix positive semidefinite and every pair's coherence.

### Writing a MIMO law in Rust

The SDK carries both worlds. A MIMO law implements `RandomMimoLaw` and exports it with
`export_mimo_law!`. It is written against the controller's own matrix types, re-exported in
`hertzforge_law_sdk::mimo`: `MimoSetup`, `MimoLoopInputs`, `MimoLoopOutputs`, `HermitianStack`
(one Hermitian matrix per line, stored as its upper triangle) and `MatrixStack` (one matrix per
line), with the controller's per-line linear algebra as `mimo::linalg` (eigen and singular value
decompositions, the regularized pseudo-inverse, congruences, positive semidefinite repair). The SDK
unpacks what the interface carries into these types and packs the drive back, so the same code can
run natively and as a law file. `MimoManifestBuilder` builds the manifest; parameters are declared
exactly as a single-output law declares them.

The worked example is a complete law crate to copy: `crates/hertzforge-law-sdk/template/mimo-open-loop`.
It is the open-loop pseudo-inverse, the same method as the built-in `pinv_open`: each loop it
returns `G = H^+ G_ref H^+^H` per line, with the operator's choice of truncated or Tikhonov
regularization. With more controls than drives this is the least-squares drive; with fewer, the
minimum-norm one. Copy the folder, rename the package in its `Cargo.toml`, and change
`src/law.rs`. Its heart is one function:

```rust
fn drive(&self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, String> {
    // H+ per line, and a report of the singular values it kept.
    let (hp, report) = linalg::pinv(inputs.frf, self.regularization).map_err(|e| e.to_string())?;
    // H+ G_ref H+^H per line: a congruence, so the drive is positive
    // semi-definite whenever the reference is.
    let mut drive = linalg::congruence(&hp, inputs.reference).map_err(|e| e.to_string())?;
    // The controller refuses a drive that is not zero outside the band.
    linalg::zero_out_of_band(&mut drive, self.band).map_err(|e| e.to_string())?;
    let kept = report.kept.iter().map(|&k| k as f64).collect();
    // ... the condition number per line, likewise ...
    Ok(MimoLoopOutputs {
        drive,
        diagnostics: vec![DiagnosticValue {
            key: "kept".into(),
            data: DiagnosticData::PerLine(kept),
        }],
    })
}
```

and `describe()` declares the rest: two parameters (the regularization and its amount), all three
shapes, no optional inputs, and two per-line diagnostics. `seed()` and `iterate()` both return
`drive()`: an open-loop law gives the same drive every loop for the same plant estimate. The crate
root is two lines, `pub mod law;` and `hertzforge_law_sdk::export_mimo_law!(law::OpenLoop);`.

In the template's folder, one command builds the law file:

```
cargo build --target wasm32-wasip2 --release
```

and it lands at `target/wasm32-wasip2/release/my_mimo_law.wasm`. The template's law is also
compiled natively and tested as part of the SDK (`crates/hertzforge-law-sdk/tests/template.rs`),
so the code a law author copies builds and returns the drive its documentation states.

A drive built as a congruence of a positive semidefinite matrix, as here, or as the Gram matrix
`B B^H` of some `B`, is positive semidefinite by construction; that is the easiest way for a law to
meet the controller's rule.

### Checking a MIMO law before hardware

Any law runs on the simulator. Before an imported MIMO law may drive hardware it has to pass the
check, and the pass is recorded against the exact file. The check runs the law on six simulated
test articles, each a whole run (identification, the level schedule, then up to 20 loops at full
level) inside the controller, under its own guards, against a reference the plant can reach:

| Case | Plant | Asks for |
|---|---|---|
| modal 4 x 4 | Well-conditioned 4 x 4 modal plant | Converge: every control within +/- 1 dB |
| modal 12 x 6 | Well-conditioned 12 x 6 modal plant (more controls than drives) | Converge, or settle |
| ill-conditioned 12 x 6 | Two drives acting through nearly the same shape; fails the identification gate, which the check overrides | Survive |
| rigid plate 12 x 6 | A plate on a soft suspension, in its rigid-body band | Converge, or settle |
| measurement noise 4 x 4 | The 4 x 4 plant with noise 30 dB under the response | Converge |
| level ladder 12 x 6 | The 12 x 6 plant on a ladder from -20 dB to full level | Converge, or settle |

Every case first asks that every call is answered, in time (a quarter of the loop's acquisition),
with a drive the checks accept, and that no protection trips. Then:

- **Converge:** every control's in-band error within +/- 1 dB within 20 loops at full level.
- **Settle:** converge, or hold a steady drive: no control's error moving more than 0.5 dB from
  loop to loop over the last three loops, with every control within 6 dB. With more controls than
  drives a reference is not exactly reachable in general, and the documented methods aim at
  different compromises (least squares, the plant's leading directions, the structure's own
  coherence), so the check asks for a steady drive near the reference rather than one compromise.
- **Survive:** as settle, or the run ended by the controller's own judgement that the loop was
  diverging or asking for more change than it allows, which the case reports. What the
  ill-conditioned plant tests is that a nearly singular plant estimate never makes the law miss a
  call: a NaN, an exception, or an answer too late.

Each case reports the loops it took to converge, the trip if one ended the run, the calls not used
and the drives refused, the drive-lines the controller repaired or held back, and the slowest call
against its budget. The five built-in laws pass; the zero-drive law fails every case (the envelope
ends each run), and a law whose drive is refused fails with that reason.

In the application, import the file and choose "Check before hardware" (`laws.check` on the
protocol). From a checkout of the repository the same check runs from the command line:

```
cargo run --release -p sds-control --example law_check -- path/to/law.wasm
```

It imports the file into the local data folder, prints each case, records the result, and says
whether the file is cleared for hardware.

### The built-in MIMO laws

Five multiple-output laws are built in, each from the published literature, each running natively
and also built as a law file from the same source (`laws/mimo-pinv`, `laws/mimo-open-loop`,
`laws/mimo-shape`, `laws/mimo-buzz`, `laws/mimo-min-drive`, all on the crate
`crates/hertzforge-mimo-laws`), with tests
that hold every law file to its native law within `1e-12`, relative:

| Id | Law | What it does |
|---|---|---|
| `pinv_closed` | Pseudo-inverse, closed loop (the default) | Starts from the pseudo-inverse drive, then each loop corrects a lower-triangular factor of the reference from the measured error, autospectra first, each drive held to a per-loop change (by default +3.5 dB and -6.0 dB). Parameters: regularization and its amount, convergence factor, the per-loop rise and fall, rank tolerance, error weighting |
| `pinv_open` | Pseudo-inverse, open loop | The pseudo-inverse drive every loop, the method of the template above, with optional error weighting as well |
| `shape_svd` | Shape-constrained (SVD) | Holds the drive on each line to the `m` directions the plant responds to most, for much lower drive; `m` fixed, or set by a singular-value ratio; closed loop inside the constraint |
| `buzz_match` | Buzz-test matched | Keeps the reference's autospectra and takes each pair's coherence and phase from the identification response, the structure's own response to uncorrelated drive; pairs below a coherence floor are left independent; then the default law's closed loop |
| `min_drive` | Minimum drive | Takes the reference's cross terms from the drive that needs the least power to produce its autospectra: on each line the drive minimizing the autospectrum residuals plus a trade-off times the drive power, over the positive semi-definite cone, and the plant's predicted response to it as the target (K. Coletti, R. B. Davis and R. Schultz, "Pareto-optimal target definition for multi-axis random vibration testing", 2026; Smallwood's minimum input trace at the exact-control end). The trade-off is picked at the knee of drive against residual, or set; closed loop by the default law's correction on that target |

The `zero drive` law (`laws/mimo-zero`) returns the zero drive and one diagnostic of each layout.
It is not for running a test; it is the smallest complete MIMO law, and the tests use it to prove
the path a law file takes.

## Classical shock laws

A classical shock law closes the loop between pulses. The controller identifies the system with
band-limited random in volts, inverts the identified plant, and calls the law: once for the first
drive, then after each pulse has been fired, measured and judged. The law returns the next drive
record at full level, in volts; the controller scales it to the scheduled level, checks it, brings
it to rest at both ends and plays it. The specification's section 8 states the world in full.

What differs from a Random law:

- **The drive is a whole record**, phase included, not a spectrum the controller randomizes. Every
  record is `record_len` samples (a power of two) at `fs_hz`, starting with the quiet lead-in.
- **Everything is at full level.** The reference is the full-level compensated pulse; the response
  the law sees is the last pulse referred to full level by the scale the controller commanded,
  limits included. A law never sees the level and cannot change it.
- **The plant comes with the setup**: `H`, coherence, the controller's regularized inverse and the
  bins it trusts. A law may use the inverse as it is, or build its own from `H`.
- **Spectra are one-sided under the negative-exponent transform.** `shock::rfft_half` and
  `shock::irfft_half` in the SDK are that pair.

### Writing a classical law in Rust

Implement `ShockClassicalLaw` (`describe`, `new`, `seed`, `iterate`) and export it with
`export_classical_law!`. The template is `crates/hertzforge-law-sdk/template/shock-classical`: the
frequency-domain correction written plainly, with three parameters.

| Key | What it does |
|---|---|
| `beta` | The fraction of each pulse's error corrected before the next pulse |
| `correct` | Off: the first drive is held for the whole run |
| `model_error_db` | Scales the law's own copy of the plant's inverse, for seeing how the controller's guards respond: +4 dB drives 1.6 times hot (the overshoot abort), +8 dB 2.5 times (the first-drive bound), -6 dB half as hard (with `correct` off, the run ends not converged) |

Build it with `cargo build --target wasm32-wasip2 --release`.

### What the controller does with a classical law's drive

Every one of these holds whatever the law returns:

- the drive's peak in volts against the drive ceiling (scaled to it) and the drive abort (refused),
  before every pulse, and at proceed for the highest level the run is scheduled to reach;
- the response the plant predicts against the drive limit and the shaker's velocity and
  displacement ratings, before every pulse;
- the length, finite values and declared values of every output;
- at full level, the drive's peak and every bin may rise at most 10 dB a pulse (by default) over the
  drive in force; the first drive's predicted response at most twice the reference's peak;
- every record tapers to rest at both ends;
- a late, failed, crashed or refused call is a miss and the next pulse refires the drive in force;
  three in a row end the run;
- a measured overshoot over twice the standard's amplitude tolerance aborts at any level; a failed
  pulse at full level aborts, except an undershoot; five pulses at one level without a pass, or the
  error rising on three pulses in a row, end the run.

### Checking a classical shock law before hardware

As for a MIMO law, an imported classical law must pass the check before it may drive hardware.
Each case is a whole run on a simulated station: AUTO from -12 dB to full level, one pulse a rung
and three at full level, advancing on a passing pulse, MIL-STD-810 Method 516, a 1 g half-sine of
11 ms unless the case says otherwise.

| Case | Plant | Asks for |
|---|---|---|
| flat | A wire | Every pulse passes, the first included |
| one mode | 400 Hz, Q 10 | A pass by pulse 3 at the first rung; every full-level pulse passes |
| station plant | 200 and 550 Hz, Q 2.5; a 6 ms haversine | As one mode |
| transport delay | One mode behind 2 ms | As one mode |
| identification error | A wire; the identified gain scaled +3 dB before inversion | A pass within eight pulses at the first rung. A law that does not correct sits 29% low and cannot pass |
| measurement noise | One mode, noise 30 dB under the first rung's pulse | A pass by pulse 4; every full-level pulse passes |
| anti-resonance | A 30 dB notch at 300 Hz under the coherence floor | Survive: complete, or be ended by the controller's own judgement (not converged, divergence); never over the drive ceiling, never an overshoot |

Every case also asks that every call is answered in time with no miss and no refused output. The
built-in law passes every case; a law that holds its first drive fails the identification-error
case.

### The built-in classical law

`fd_correct`, the frequency-domain correction: the plant's inverse applied to the reference, then
after each pulse a fraction `beta` (default 0.5) of the inverse times the error, on the bins the
controller trusts, each bin's rise held to `max_db_per_pulse` (default 6 dB), the first pulse
optionally `initial_backoff_db` under the prediction. It is time waveform replication's iteration
bin for bin, runs natively, and is also the law file `laws/shock-fdc`; the two give the same run.

## SRS shock laws

An SRS shock law closes the loop between pulses on a synthesized pulse. The controller synthesizes
every pulse from a table of components (odd-half-cycle wavelets or damped sines, one per band by
default), identifies the system with that pulse at a low level in volts, and calls the law: once
for the starting gains, then after each pulse has been fired, measured and judged. The law returns
one gain per component; the controller applies the gains to its table, caps them, checks the drive
and plays it. The specification's section 9 states the world in full.

What differs from a classical law:

- **A law returns gains, not a record.** One multiplicative gain per component, in table order,
  each finite and above zero. The controller synthesizes the drive, so every pulse ends at rest
  whatever the gains.
- **Two grids.** Every spectrum is a shock response spectrum on the band centres (`band_hz`); the
  components are on their own grid (`component_hz`). `srs::nearest_band` in the SDK maps a
  component to the band nearest it.
- **The target follows the pulse.** The target a law sees on each call is at the scale the last
  pulse was commanded at, so a gain computed from it aims at what was asked of that pulse.
- **The identified gain comes with the setup**: `|H|` at each component, floored by the controller,
  or nothing when the run identified nothing.

### Writing an SRS law in Rust

Implement `ShockSrsLaw` (`describe`, `new`, `seed`, `iterate`) and export it with
`export_srs_law!`. The template is `crates/hertzforge-law-sdk/template/shock-srs`: the per-band
ratio update written plainly, with three parameters.

| Key | What it does |
|---|---|
| `max_db_per_pulse` | The most any component may rise or fall from one pulse to the next |
| `correct` | Off: the starting gains are held for the whole run, and a run that starts wrong ends not converged |
| `seed_error_db` | Scales the law's starting gains, for seeing how the controller's guards respond: -4 dB starts every band 4 dB low |

Build it with `cargo build --target wasm32-wasip2 --release`.

### What the controller does with an SRS law's gains

Every one of these holds whatever the law returns:

- one gain per component, finite, above zero, with declared values, or the call is refused;
- no gain may rise more than 6 dB in a pulse (by default);
- each gain is held to what the limit channels can take at the highest level the run is scheduled
  to reach (a falling gain always wins), and a band the limit holds down may read low without
  failing a pulse;
- the drive's peak in volts against the drive ceiling (scaled to it) and the drive abort (refused),
  before every pulse, and at proceed for the highest level the run is scheduled to reach;
- the response predicted through the identified gain against the drive limit, the kinematic aborts
  and the shaker's ratings, before every pulse;
- pulses start no closer than the time between pulses;
- a late, failed, crashed or refused call is a miss and the next pulse refires the table in force;
  three in a row end the run;
- five pulses at one level without a pass, or the run's pulse cap, end the run not converged.

### Checking an SRS shock law before hardware

As for the other worlds, an imported SRS law must pass the check before it may drive hardware.
Each case is a whole run on a simulated station: a flat 1 g target on a 1/3-octave grid from 100
to 2000 Hz at Q 10, synthesized from wavelets; AUTO -12, -6 and 0 dB with two pulses at each lower
level and the case's pulses at full level, advancing on a passing pulse (every band inside 3 dB),
at most 12 pulses a level.

| Case | Plant | Asks for |
|---|---|---|
| one mode | 400 Hz, Q 10 | Every band within 3 dB by the sixth full-level pulse, and on every one after |
| close modes | 240 and 260 Hz, Q 30 | As one mode |
| measurement noise | One mode, noise 30 dB under the identification's pulse | As one mode |
| level ladder | A wire, manual moves: a jump from -12 to -6 dB, then six 1 dB steps to 0 dB | Every band within 3 dB after three pulses at each level |
| identification error | One mode; the identified gain scaled +3 dB before the seed | Every band within 3 dB by the eighth full-level pulse. A law that does not correct cannot pass |
| limit channel | A wire, and a limit channel whose supplied gain is 4 at 500 Hz under a flat bound | The prediction at most 0.5 dB over the bound on every pulse; outside the bands the limit holds down, every band within 3 dB |

Every case also asks that every call is answered in time with no miss and no refused output. The
built-in law passes every case; a law that holds its starting gains fails the identification-error
case.

### The built-in SRS law

`srs_ratio`, the classic per-band SRS iteration: each component starts at one over the identified
gain at its frequency, then after each pulse is scaled by the target over the measured response at
the band nearest it, limited to `max_db_per_pulse` (default 3 dB) either way. It runs natively, and
is also the law file `laws/shock-srs-ratio`; the two give the same run.

## Importing a law

A law file is imported from a path. Hertzforge loads it (which finds the world it implements and
checks its manifest) and copies it into the `laws` folder of the local data root, named by the
SHA-256 of its bytes: `laws/<sha256>.wasm`. Single-output Random, multiple-output Random and
classical shock laws are listed apart, each on its own test type's screens, and a law chosen for
another test type is refused. The data
root is the folder Hertzforge keeps its runs in, on the local disk.

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

The interface is `hertzforge:control-law@0.1.0`, with four worlds: `random-single`,
`random-mimo`, `shock-classical` and `shock-srs`. While it is at 0.x it may change between releases, and a law must be built against
the version the host loads.

## Building and testing in this repository

The workspace holds the law host (`crates/hertzforge-law-host`), the SDK
(`crates/hertzforge-law-sdk`), the laws (`laws/*`), the host's test fixtures
(`crates/hertzforge-law-host/guests/*`): small laws that trap, loop forever, grow memory, try to
open files, sleep or return a bad drive on purpose (and a multiple-output one that returns a NaN,
a matrix that is not Hermitian, the wrong dimensions, a drive outside the band or an indefinite
matrix, spins or traps), the built-in MIMO laws (`crates/hertzforge-mimo-laws`), and the Random
law catalog (`crates/hertzforge-random-laws`), which lists the built-in and imported laws of both
test types, resolves the operator's choice of law, and runs a law file as a Random control law
under the controller's guards. Its tests run the misbehaving fixtures through a whole Random run on the simulator and
show each ends in the controller's controlled stop. The laws and fixtures are workspace members,
so `cargo clippy --workspace` and `cargo test --workspace` build and check them natively, and the
one lockfile and `cargo deny` cover their dependencies.

`scripts/build_laws.sh` builds every law and fixture to WebAssembly into
`<target>/wasm32-wasip2/release/`. `scripts/rust_tests.sh` runs it before the tests, and the host's
tests read the files from there. A test that needs a law file that has not been built fails and
names the script; none is skipped.
