# Control laws

A control law is the math the controller runs once per loop to decide the next drive. Hertzforge
loads control laws from files, so a law can be written, built and shared without rebuilding
Hertzforge. This page is a guide for someone writing one, for the test types with a law
interface: single-output Random, and multiple-output (MIMO) Random, classical shock, SRS shock,
Sine and time waveform replication, which each have their own chapter below. The requirements a law must meet, what it is given and what it must return are in the
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
against its budget. The five built-in laws from the literature pass. The band-average law passes
four cases and settles on the two that ask a 4 x 4 plant to converge: it controls band averages,
so each control's line-by-line rms error holds steady at 1.5 dB there, with every call answered in
time and no trip. The zero-drive law fails every case (the envelope
ends each run), and a law whose drive is refused fails with that reason.

In the application, import the file and choose "Check before hardware" (`laws.check` on the
protocol). From a checkout of the repository the same check runs from the command line:

```
cargo run --release -p sds-control --example law_check -- path/to/law.wasm
```

It imports the file into the local data folder, prints each case, records the result, and says
whether the file is cleared for hardware.

### The built-in MIMO laws

Six multiple-output laws are built in, each running natively and also built as a law file from
the same source (`laws/mimo-pinv`, `laws/mimo-open-loop`, `laws/mimo-shape`, `laws/mimo-buzz`,
`laws/mimo-min-drive`, `laws/mimo-band-average`, all on the crate
`crates/hertzforge-mimo-laws`). The first five are from the published literature; the sixth is
Brandon Zwink's. There are tests
that hold every law file to its native law within `1e-12`, relative:

| Id | Law | What it does |
|---|---|---|
| `pinv_closed` | Pseudo-inverse, closed loop (the default) | Starts from the pseudo-inverse drive, then each loop corrects a lower-triangular factor of the reference from the measured error, autospectra first, each drive held to a per-loop change (by default +3.5 dB and -6.0 dB). Parameters: regularization and its amount, convergence factor, the per-loop rise and fall, rank tolerance, error weighting |
| `pinv_open` | Pseudo-inverse, open loop | The pseudo-inverse drive every loop, the method of the template above, with optional error weighting as well |
| `shape_svd` | Shape-constrained (SVD) | Holds the drive on each line to the `m` directions the plant responds to most, for much lower drive; `m` fixed, or set by a singular-value ratio; closed loop inside the constraint |
| `buzz_match` | Buzz-test matched | Keeps the reference's autospectra and takes each pair's coherence and phase from the identification response, the structure's own response to uncorrelated drive; pairs below a coherence floor are left independent; then the default law's closed loop |
| `min_drive` | Minimum drive | Takes the reference's cross terms from the drive that needs the least power to produce its autospectra: on each line the drive minimizing the autospectrum residuals plus a trade-off times the drive power, over the positive semi-definite cone, and the plant's predicted response to it as the target (K. Coletti, R. B. Davis and R. Schultz, "Pareto-optimal target definition for multi-axis random vibration testing", 2026; Smallwood's minimum input trace at the exact-control end). The trade-off is picked at the knee of drive against residual, or set; closed loop by the default law's correction on that target |
| `band_average` | Band average | Solves the inverse in fractional-octave bands: one flat correlated drive per band whose band-averaged response matches the band-averaged reference, with a threshold per band chosen by a sweep at the first solve; then a gain correction band by band from the measured response, and optionally a new solve from the updated FRF. It controls band averages, not lines |

The band-average law's arithmetic is `crates/hertzforge-law-band-average-math`, a port of
Brandon Zwink's Python law that the crate's `parity` example holds to it call for call. It leaves
to the core what the core does: limit channels, the drive ceilings (its own RMS limit is off unless
set) and the plant estimate (with its FRF update on, it solves again from the FRF and multiple
coherence the core hands it, so the test's FRF mode must be update). Its buzz cross terms are the
buzz-test law's reference. It reports each band's threshold and gain correction, and counts of the
channels its check has left out, the FRF updates it has accepted and the bands it has held. Its
first solve runs the sweep: 0.5 to 3 s on a test of 48 controls, 12 drives and 8193 lines, which
is over the law budget of a 2 s loop, and milliseconds on the law check's plants.

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

## Sine laws

A Sine law decides one number every control tick (10 ms by default): the amplitude of the one tone
the controller plays, in drive units peak (volts peak on hardware). The controller keeps the
frequency, the sweep and its direction, the tone's phase, the level schedule and the second
reference output; the law is called once for the starting drive after identification (`engage`)
and then once a tick (`tick`), on the controller's real-time thread, with a quarter of the tick as
its budget. The specification's section 10 states the world in full.

What differs from the other worlds:

- **One number per tick, on the real-time thread.** A Sine law runs a hundred times a second, and a
  late call is not used. Keep a tick's work small and bounded; do the heavy work in the
  constructor, which is given the whole identification.
- **The prediction comes with every tick.** `prediction_db` is the controller's plant-inverse drive
  at this frequency at full level, from the identification, in dB relative to one drive unit; it
  is absent where the controller does not trust the identification. The built-in law follows its
  change from tick to tick; a law may use it or not.
- **The target is at the present level.** A level change is a change of target, and a law follows
  it within the controller's caps.
- **The response arrives late.** On hardware a new drive's response reaches `measured` several
  ticks after it is played (the converters and the output buffer), and the tracking filter, whose
  corner scales with the frequency, takes a third of a second more to read it at 20 Hz. After the
  engage the reading can stall and dip on its way up. An error read before the response has
  arrived is one the law did not cause, and integrating it climbs blind. Hold the starting drive
  until `measured` is near what the prediction says the drive in force should give, as the
  template does; a reading that has stopped rising has not necessarily come up.
- **`drive` is what was played.** It can be under what the law asked on the tick before: the caps,
  the ceilings and the notch hold the drive down without saying so in `hold`. A law that keeps a
  level of its own should follow `drive` down, as the built-in law does, or a long clamp winds that
  level up.
- **Holds.** `hold` is `notch` while a limit channel holds the drive down (the controller holds it
  at the notch ceiling, so a law cannot raise it) and `probe` while a resonance search maps the
  plant (the law is still called, and its drive is not used).
- **Live parameters.** A number parameter the manifest lists as `live` can be changed while the
  test runs; the controller checks the value against its declared range and calls `adjust`, and
  the value applies from the next tick.

### Writing a Sine law in Rust

Implement `SineSingleLaw` (`describe`, `new`, `engage`, `tick`, and `adjust` for live parameters)
and export it with `export_sine_law!`; build the manifest with `SineManifestBuilder`, which takes
the live keys. The template is `crates/hertzforge-law-sdk/template/sine-pi`: a
proportional-integral loop on the error in dB that follows the prediction from tick to tick. It
holds its starting drive until the measurement has come up: at least five ticks, then until the
measured amplitude is within 2 dB of what the identification predicts for that drive (or over
it), at most a second, and the second without a trusted prediction.

| Key | What it does |
|---|---|
| `kp` | Proportional gain on the change of the error |
| `ki` | Integral gain on the error, per second; live |
| `follow` | How much of the prediction's change from tick to tick the drive follows |
| `max_step_db` | The most the law moves the drive in one tick |
| `start_offset_db` | Scales the law's own first drive, for seeing how the controller's guards respond: +4 dB starts 4 dB high, and the law brings it back |

Build it with `cargo build --target wasm32-wasip2 --release`. A law written against the
controller's own native seam (`hertzforge_sine::law`) can be carried across the interface with
`sine::native` in the SDK, as the built-in law's file is.

### What the controller does with a Sine law's drive

Every one of these holds whatever the law returns:

- a drive that is not a finite number at least zero, or a value that is not finite or not
  declared, is refused;
- a call past its budget, a failed call, a panic or a refused output is a miss and the drive in
  force is held; three in a row end the run with the controlled stop (the 120 dB/s ramp);
- the drive moves at most 1 dB a tick and rises at most 240 dB a second (by default); a cap that
  binds for a second without a break ends the run (`divergence`);
- the first drive is at most the prediction plus 6 dB, or 0.05 of the start level's scale without
  one;
- the notch ceiling holds the drive under the limit channels (each held at its limit, its abort edge
  above), and the drive ceiling, the drive abort, the alarm and abort bands and the kinematic
  monitor apply as on every Sine run.

### Checking a Sine law before hardware

As for the other worlds, an imported Sine law must pass the check before it may drive hardware.
Each case is a whole run on a simulated station at 16384 Hz and a 10 ms tick: a flat 1 g target,
identification at -20 dB, the abort band 12 dB either way.

| Case | Plant and run | Asks for |
|---|---|---|
| slow sweep | One mode, 250 Hz, Q 50; a log sweep 100 to 800 Hz at 1 oct/min | Within 1.5 dB through the resonance (every judged tick within 30 Hz of it) |
| fast sweep | The same at 6 oct/min | Within 5 dB through the resonance; within 2 dB more than 30 Hz from it |
| close modes | 240 and 260 Hz, Q 30; 2 oct/min | Within 2 dB |
| anti-resonance | A flat plant cut 30 dB at 300 Hz, its coherence there under the trust floor; 2 oct/min | The run completes, or ends on the controller's own divergence trip; the drive never over the ceiling |
| measurement noise | One mode; noise 30 dB under the response; a 30 s dwell at 250 Hz | Within 1.5 dB |
| level ladder | -20, -12, -6 and 0 dB moved by hand at a 250 Hz dwell, 10 s each, and one live change | Every rung within 1 dB after its settle window |
| limit channel | One mode, and a limit channel reading up to 4 times the control at the mode under a flat 2 g limit | The limit channel at most 0.5 dB over its limit; the control at most 1 dB over its target after the notch releases |
| transport delay | A flat wire that plays the drive 50 ms late (a hardware station's measured lag, rounded up); engaged at -6 dB, a 6 s dwell at 100 Hz | The run completes, and every tick from 2 s after the engage on is within 0.5 dB, settle window or not |

Only the transport delay case's station has a lag: on the others a new drive's response is read on
the next tick, on a hardware station several ticks later. Every case also asks that every call is
answered in time with no miss and no refused output. The built-in law passes every case; a law that
misbehaves fails.

### The built-in Sine law

`servo_blend`: an amplitude servo (proportional-integral on the error in dB) moves its own level
each tick, and the drive moves by that step plus `blend_w` (default 0.5, live) times the
prediction's change, at most 1 dB a tick. `compression` sets the servo's speed: slow (integral gain
2, at most 20 dB/s, the default), fast (6, 60 dB/s) or custom. A change of level is taken by the
servo alone (the prediction follows the frequency), so the compression sets how fast a level step
lands: slow takes a 1 dB step to within 0.1 dB in about 1.3 s, fast in under half a second. Fast is
quicker than a hardware station's own measurement, which trails the drive by the converters, the
output buffer and the tracking filter, and it overshoots there by several dB on a level step at
20 Hz; slow does not. It runs natively, and is also the law file `laws/sine-servo`; the two give the
same run.

## Time waveform replication laws

A time waveform replication (TWR) law closes the loop between whole records. The controller
identifies the system with band-limited random in volts, inverts the identified plant and holds for
review; when the run proceeds it builds the law and calls it: once for the first drive, then after
each correction play that did not meet its gate (and after each playback play when adaptation
during playback is on). The law returns the next drive record at full level, in volts; the
controller scales it to the commanded level, checks it, brings it to rest at both ends and plays
it. The specification's section 11 states the world in full.

What differs from a classical shock law:

- **The record is the replicated section between quiet pads**, `record_len` samples (a power of
  two) at `fs_hz`, often tens of thousands of samples. A call's lists are long, and copying them in
  and out is part of every call's time.
- **The controller calls the law only when it wants a correction.** A play that meets the gate is
  carried to the next level by the controller alone, with no call.
- **When the controller clipped the last drive**, `limited` is set and `drive` is the drive that
  played. A law that corrects from that record, rather than from the one it asked for, measures
  what really played; the built-in does, and so does the template.
- The plant has the classical world's shape, and the same conventions: one-sided spectra under the
  negative-exponent transform; `twr::rfft_half` and `twr::irfft_half` in the SDK are that pair.

### Writing a TWR law in Rust

Implement `TwrSingleLaw` (`describe`, `new`, `seed`, `iterate`) and export it with
`export_twr_law!`. The template is `crates/hertzforge-law-sdk/template/twr`: the frequency-domain
correction written plainly, with three parameters.

| Key | What it does |
|---|---|
| `beta` | The fraction of each play's error corrected before the next play |
| `correct` | Off: the first drive is held for the whole run |
| `model_error_db` | Scales the law's own copy of the plant's inverse, for seeing how the controller's guards respond: -6 dB plays half the target (with `correct` off, the run ends not converged), -4 dB leaves 0.685 of the error after each correction at `beta` 0.5, +8 dB starts 2.5 times hot (the first-drive bound) |

Build it with `cargo build --target wasm32-wasip2 --release`.

### What the controller does with a TWR law's drive

Every one of these holds whatever the law returns:

- the drive's peak in volts against the drive ceiling, on the record as it will play, refused (and
  the run aborted) if over; in AUTO, at proceed, the controller's own full-level drive against the
  ceiling; in MANUAL, a level move that would pass it is refused;
- the length, finite values and declared values of every output;
- at full level, the drive's peak and every bin may rise at most 10 dB a call (by default) over the
  drive last played; the drive's predicted response peak at most 1.5 times the level target's
  (with the default envelope); the first drive's at most twice;
- every record tapers to rest at both ends;
- a late, failed, crashed or refused call is a miss and the next play replays the drive in force;
  three in a row end the run;
- the response is watched as each block arrives, and a play that leaves the abort band for eight
  samples in a row is cut short with the emergency stop;
- each play's lag is fitted against the identification's; a drift of more than a frame ends the
  run before anything is corrected from that play;
- a level that does not meet its gate in its iterations ends the run (or holds), as does the error
  rising on three plays in a row.

### Checking a TWR law before hardware

An imported TWR law must pass the check before it may drive hardware. Each case is a whole run on
a simulated station at 12 800 Hz: a 2 s field-like reference peaking at 1 g, AUTO from -12 dB to
full level with a 10% gate and at most eight correction plays a rung, then one playback play.

| Case | Plant | Asks for |
|---|---|---|
| flat | A wire | Every rung gates on its first play; full level within 5% |
| one mode | 400 Hz, Q 10 | Every rung gates within its eight; full level within 10% |
| station plant | 200 and 550 Hz, Q 2.5 | As one mode |
| transport delay | One mode behind 2 ms | As one mode |
| deep queue | A wire behind 41 samples, the station keeping its output 512 frames ahead in 256-frame blocks | As flat, and the run aligned on the lag the identification measured |
| identification error | A wire; the identified gain scaled +3 dB before inversion | The first rung gated by play 4, every later rung on its first. A law that does not correct sits 29% low and cannot pass |
| measurement noise | One mode, noise 30 dB under the first rung's response | Full level within 10% |
| anti-resonance | A 30 dB notch at 300 Hz under the coherence floor | Survive: complete, or be ended by the controller's own judgement (not converged, divergence, the coverage floor); never over the drive ceiling, never an envelope abort |

Every case also asks that every call is answered in time with no miss and no refused output. The
built-in law and the template pass every case; a law that holds its first drive fails the
identification-error case. A check record clears a law file only at the check in force; a file
checked by an earlier one is checked again.

### The built-in TWR law

`twr_correct`, the frequency-domain correction: the plant's inverse applied to the reference, then
after each correction play a fraction `beta` (default 0.5) of the inverse times the error, on the
bins the controller trusts, each bin's rise held to `max_db_per_iter` (default 6 dB), the first
play optionally `initial_backoff_db` under the prediction; after a clipped play it corrects from
the drive that played. It runs natively, and is also the law file `laws/twr-correct`; the two give
the same run, sample for sample.

## Mixed Mode

A Mixed Mode test (tones and narrow bands of random on a random floor) has no
law world of its own. It takes two laws you already know:

- **The floor law** is a single-output Random law (the `single` world;
  default `hinv`). It is told the floor's target alone, at full level, and the
  floor's estimate made on the response with every tracked tone subtracted.
  On a line it could not read cleanly (beside a dwelling tone, or under a band
  and three lines either side of it), it is given the error of the nearest
  clean lines either side, so it neither chases a part nor loses its mean.
- **The tone law** is a Sine law (the `sine` world; default `servo_blend`),
  built once per tone from one choice. Each instance sees its own tone: its
  tracked amplitude at full level as `measured`, its own commanded amplitude as
  `drive`, and the prediction from the identified plant at its frequency.

Narrow bands are not a law: the controller shapes each band over the
identified plant at the center it will have when the drive plays, and moves a
capped gain on it from the band's level read in the band's own moving frame.

### What the controller does around the laws

Every guard of each world applies to its law as it does in that world: the
floor law's output checks, change cap, time budget and controlled stop on
repeated misses; each tone law's per-tick cap, rise limit, budget and
divergence time. On top of them the controller judges the sum: the drive's
peak per sample against the ceiling and abort, the floor on clean lines, each
tone and each band against its own tolerance, and a run that trips names the
part ("Tone t3 abort").

The level schedule moves every part by the same dB as one ramp. A law sees its
part at full level throughout: its target, its measurement and its drive are
all stated at 0 dB, so a level step never reads to a law as an error.

A tone is read through a narrow tracker (0.5 Hz by default), whose delay is
the tone loop's reading delay. A tone law that corrects faster than that delay
allows will ring. The built-in tone law therefore takes a gentler compression
in a Mixed Mode run (custom, integral gain 0.5, 20 dB/s) unless you choose
one.

## MIMO Shock laws

A MIMO Shock law drives several shakers at once to put a pulse at several control points: a
classical waveform at every control, or a shock response spectrum (SRS) at every control. The
controller identifies the structure on the pulse's own spectral lines with every drive playing at
once, then builds the law and calls it between pulses: once for the first pulse, at the first level
of the ladder, then after each pulse with what was played and measured and the level of the next
pulse. The law returns every drive's whole record for that next pulse, in volts, at that level. The
specification's section 15 states the world in full.

How a law for several drives should work is an open question. A correction through a model of the
structure can diverge when the structure's resonances move with level, and with several drives the
SRS can look better for a few pulses while the waveform and the drives grow. The built-in laws are
starting points, each with a known weakness (below), and the check is written for the law that
would have none. Trying a new law is meant to be quick.

What differs from the single-output shock laws:

- **The law builds the drive at the level it is played.** The drives and responses it is given are
  at the level played; `level_scale` turns a level into its amplitude, and the law divides by it to
  work at full level and multiplies by the next level's to play.
- **The plant is a stack of matrices**, controls by drives on every line, with each path's delay in
  it. A law takes its own transforms; `spectrum::rows_rfft` and `spectrum::rows_irfft` in the SDK
  go between records and spectra and scale in the same step.
- **One law can run both kinds of test.** For an SRS test the controller synthesizes a waveform
  for each control's SRS and hands it over as the reference, so a waveform law runs an SRS test as
  it runs a classical one; an SRS law also gets each control's target SRS and the measured SRS
  after every pulse. The manifest says which kinds a law accepts.

### Writing a MIMO Shock law in Rust

Implement `ShockMimoLaw` (`describe`, `new`, `seed`, `iterate`) from
`hertzforge_law_sdk::shock_mimo` and export it with `export_shock_mimo_law!`. Build the manifest
with `ManifestBuilder`: name, version, summary, the target kinds, the parameters and the values.
The SDK hands the law plain Rust: `Setup` (one row per control for the reference, the plant as a
`MatrixStack`, the band, the ceilings, the ladder), `Inputs` (one row per drive and per control)
and takes `Outputs` back. `shock_mimo::math` is the mathematics the built-in laws run on: the
regularized inverse per line, the SRS, delay estimation and the built-in laws' own steps.

Start from `laws/mimo-shock-paired`. It is the whole of `paired` written out in one file, with
comments for an engineer who knows control and has not seen this code: its parameters, its state,
the seed and the iteration, and nothing else.

### What the controller does with a MIMO Shock law's drives

Every one of these holds whatever the law returns:

- one record of the run's length per drive, every sample finite, at most eight declared values,
  delays (if any) one per control; anything else is refused by name and is a miss;
- each drive's peak against its ceiling: over it the drive is scaled down, past its abort the run
  aborts;
- each drive's growth from one pulse to the next, beyond the level step, is capped;
- the drives' predicted response is checked against the abort lines before a pulse is fired;
- an error that rises on consecutive pulses at one level ends the run in the controlled stop;
- a late, failed, crashed or refused call is a miss; three in a row end the run; a law file that
  traps, runs past its budget or asks for too much memory stays stopped for the rest of the run.

### Checking a MIMO Shock law before hardware

An imported MIMO Shock law must pass the check before it may drive hardware. Each case is a whole
run on the simulator, from -12 dB to full level in 3 dB steps with three pulses a level, for every
target kind the law accepts, with an SRS tolerance of -1.5 and +3 dB.

| Case | Plant | What it tests |
|---|---|---|
| linear 4 x 4 | Four drives, four controls, coupled, linear | The base case |
| jointed 4 x 4 moderate | The same structure, joints softening and damping moderately with level | A plant that moves with level |
| jointed 4 x 4 severe | The same, strongly | Where a model correction diverges |
| jointed 6 x 3 | Six controls on three drives | More controls than drives |
| delay error | Linear 4 x 4, the law's model 0.5 ms late on every path | A wrong delay in the model |
| identification error | Moderate 4 x 4 identified 20 dB under the reference, the model 2 dB high | A wrong model |
| measurement noise | Moderate 4 x 4, noise at 3 % of the target rms on every response | Noise |
| limit channel | Linear 4 x 4 with a monitored output that binds from -3 dB | A run held under full level |

Every case asks the same six things of the run: every answer of the law was used and it never
faulted; the run did not abort or stop on a guard; it reached full level (the limit case excepted);
the last pulse of the last level passed at every control; the worst control's error on that pulse
was within 1 dB of its best at that level, so a law that found the target and drifted off fails;
and no drive's peak grew more than 6 dB over that level's pulses. A failed case is recorded with
the requirement it broke and the number that broke it.

The requirements are what a good law should do, written before any law was run against them. Each
built-in law is known to fail some of them, and its facts in the law picker say which.

### The built-in MIMO Shock laws

**`inverse`** (the default for a waveform target; accepts both kinds). The first drives are each
control's target through the regularized inverse of the identified plant, line by line. After each
pulse the drives are corrected by a fraction of the inverse times the waveform error, with a gain
per line that is halved on a line whose smoothed error grew and restored slowly. Known weakness: on
a structure whose resonances move with level, the line gains slow the divergence but do not stop
it.

| Key | What it does |
|---|---|
| `lambda_rel` | Regularization of the inverse on each line, relative to that line's largest singular value (default 0.001) |
| `gain` | The fraction of each pulse's error corrected (default 1) |
| `watchdog` | The per-line gain cut (default on) |
| `refresh` | After every pulse, move the model the least amount that reproduces that pulse (default off) |
| `refresh_mu` | How far the refresh moves the model (default 0.5) |
| `align` | Remove each control's best-fit delay before the error is formed (default off) |
| `align_max_ms` | The largest delay removed (default 2 ms) |
| `model`, `modes`, `track_from_identification` | How the model follows the plant (below; default `frozen`) |
| `predicted_step` | At a level step, correct the error the next level is predicted to have, from the tracked model's change between the two levels, instead of the error just measured; needs `model` `track` or `track_secant` (default off) |

**`shape`** (the default for an SRS target; SRS only). A smooth real gain per control shapes that
control's target by its SRS ratio after every pulse, with a weight that rises with level (0.5 at
-12 dB to 0.85 from -3 dB), and the drives are the regularized inverse applied to the shaped
targets: no waveform error is ever formed. Each control's delay is re-estimated from its response.
It is the several-drive form of the published single-drive practice. Known weakness: bounded, but
it stalls short of tolerance on a strongly coupled structure.

| Key | What it does |
|---|---|
| `lambda_rel` | As for `inverse` (default 0.001) |
| `max_step_db` | The largest change of a control's gain in one pulse (default 6 dB) |
| `delay_relax` | How far each delay moves toward the measured one per pulse; 0 holds them (default 0.5) |
| `delay_bound_ms` | Each control's delay stays this close to the first control's (default 2 ms) |
| `anchor` | `target` (default) or `measured`: shape the measured response with its own phase; needs `refresh` |
| `refresh`, `refresh_mu` | As for `inverse` (default off, 1) |
| `seed_timing` | `zero` (default) or `least_effort`: start the delays where the least drive and unreachable energy are needed |
| `model`, `modes`, `track_from_identification` | How the model follows the plant (below; default `frozen`) |
| `gain_clamp_db` | Each control's accumulated gain stays within this many dB of one; 0 leaves it free (default 0) |
| `stall_tol_db` | A point is reported stalled (the `stalled_points` value) when its gain is at the clamp, or its error is above this and has not fallen in two pulses at one level (default 1.5 dB) |

**How the model follows the plant (`model`).** `inverse` and `shape` build every drive through a
model of the structure. A jointed structure softens as it is driven harder, so the model measured
at identification is wrong by full level. `model` chooses how the law keeps its model current:

| `model` | What it does | Cost |
|---|---|---|
| `frozen` (default) | The identification, never changed | Nothing |
| `secant` | After each pulse, the least change of the model that reproduces every pulse played at the current level | Small, every pulse |
| `track` | Fits a modal model (common resonances and damping, a delay per control) to the identification before the first pulse; after each pulse re-fits the resonances and damping from that pulse, and predicts the first pulse of each new level from their trend with level. When the tracked model explains the pulses worse than `secant` would, `secant`'s model is used until it does better again | A fit before the first pulse (seconds), and a small re-fit every pulse |
| `track_secant` | `track`, with the pulses of the level being played reproduced exactly on top of the tracked model | As `track` |

`modes` sets the number of modes fitted; 0 (the default) counts the identified plant's resonance
peaks in the band, then adds two modes at a time until the fit is within 1 % of the
identification. A fit further than 5 % from the identification is not trusted: the run starts on
`secant`'s model, and the `model_source` value says 3. `track_from_identification` takes the
identification as the first point of the level trend, at the level the controller measured it at
(the identification's peak over the target's peak, the largest among the controls).

Where they help and where they do not. On a plant whose modes soften with the pulse's level but
not within one pulse, `track` with the identification as a level point takes the first full-level
pulse from about 10 dB over the SRS to under 1 dB. On the simulator's jointed plants, whose joints
soften within each pulse and recover as it decays, no option helps: no single linear model
reproduces one of those pulses (the best modal re-fit of a pulse leaves 8 % of it unexplained at
-30 dB and 47 % at -18 dB), and the tracked model is set aside for `secant`'s within a pulse or two.
There, on targets that drive the joints hard, `frozen` reached full level where every other option
stopped at a lower level, and on milder ones the options did no better than `frozen`. That is why
the default is `frozen`. A structure's joints decide which of the two it is like, and a run on it says which:
`model_prediction_residual` is how far the model missed the pulse it built.

**`paired`** (both kinds). Each control is paired with one drive (control 1 with drive 1, and so
on); that drive's spectrum is scaled by the smoothed ratio of wanted to measured magnitude at its
control. No cross path of the plant is used, so on a coupled structure the pairs work against each
other; it is there to show that, and as the template. Parameters `weight` (the power on the ratio,
default 0.8), `smooth_lines` (default 15) and `max_step_db` (default 6 dB).

Each runs natively and as a law file (`laws/mimo-shock-inverse`, `laws/mimo-shock-shape`,
`laws/mimo-shock-paired`), from the same mathematics and the same constructor; the law file's
drives match the native law's within 1e-12 of each record's peak, for every option.

**Time.** A law is built and seeded before the first pulse, under `control.seed_budget_s` (60 s by
default): that is where `track` fits its modal model. Each pulse's call has the call budget, a
quarter of the time between pulses by default. Before the first pulse the controller times one
iteration of a second copy of the law on the identified plant's predicted response, so a law that
cannot answer in time is named (`law_too_slow`) before the run, not in the middle of it.

### Trying a new idea

1. Copy the template and give it a name:

   ```sh
   cp -R laws/mimo-shock-paired laws/mimo-shock-myidea
   ```

   In `laws/mimo-shock-myidea/Cargo.toml` change the package name to
   `hertzforge-law-mimo-shock-myidea`. In `src/lib.rs` change the name in `describe`, then change
   `new`, `seed` and `iterate`. Delete `src/tests.rs` and the `mod tests;` line at the end of
   `src/lib.rs`: they check that the template matches the built-in `paired`. Then add the new
   crate to the lockfile:

   ```sh
   cargo update --workspace
   ```

2. Build it:

   ```sh
   bash scripts/build_laws.sh
   ```

   The law file is `<target>/wasm32-wasip2/release/hertzforge_law_mimo_shock_myidea.wasm`, where
   `<target>` is `target` or `CARGO_TARGET_DIR`.

3. Run it on the reference bench beside the built-in it should beat:

   ```sh
   cargo run --release -p hertzforge-mimo-shock-laws --features reference-plant \
       --example try_mimo_shock_law -- \
       target/wasm32-wasip2/release/hertzforge_law_mimo_shock_myidea.wasm srs
   ```

   `srs` or `waveform` picks the target kind; `name=value` arguments after it set parameters
   (`weight=0.5`). The bench is a 4 x 4 coupled structure whose resonances fall 5 % and whose
   damping triples by full level. It prints each pulse's level, waveform error, worst SRS error
   and drive rms for your law, then for `shape` (SRS) or `inverse` (waveform) on the same pulses.

4. Import the law file in the law panel, run its check on the simulator, and read the record: each
   case passed, or the requirement it broke with its number. A new law is worth keeping when it
   passes a case the built-ins fail, or passes the same cases with less drive.

## Importing a law

A law file is imported from a path. Hertzforge loads it (which finds the world it implements and
checks its manifest) and copies it into the `laws` folder of the local data root, named by the
SHA-256 of its bytes: `laws/<sha256>.wasm`. Single-output Random, multiple-output Random,
classical shock, SRS shock and Sine laws are listed apart, each on its own test type's screens, and a law chosen for
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

The interface is `hertzforge:control-law@0.1.0`, with seven worlds: `random-single`,
`random-mimo`, `shock-classical`, `shock-srs`, `sine-single`, `twr-single` and `shock-mimo`. While it is at 0.x it may change between releases, and a law must be built against
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
