# Control law plug-ins: specification

**Status: draft for comment.** Interface version 0.1. Both Random worlds, single-output
(section 6) and MIMO (section 7), are implemented: the host loads them and the built-in laws run
through them. While the interface is 0.x it may still change (R15). Comments are wanted on every
part, and on the questions still open (section 10).

## 1. Purpose

Hertzforge lets the math that closes a vibration control loop be changed without rebuilding the
program. Each test type ships a default control law and may ship others; an operator chooses one per
test, and can import a law of their own as a single file. This document states what a law file must
be, what the controller guarantees around it, and the interface for each test type.

Single-output Random was the pilot. MIMO Random is the second test type with a law interface, and
its world is frozen in this version (section 7).

## 2. Terms

| Term | Meaning |
|---|---|
| Law | The code that, once per control loop, turns the latest measurements into the next drive request. |
| Core | Everything else in the controller: acquisition, spectral estimation, system identification, drive synthesis, the level schedule, protection and records. |
| Law file | A WebAssembly component (`.wasm`) that implements one law for one test type. |
| Line | One frequency line of the control grid, spacing `df`. |
| Band | The half-open range of lines `[lo, hi)` the test controls. |
| SDM | Spectral density matrix: one-sided, per line, Hermitian. Its diagonal holds the power spectral densities, its off-diagonal terms the cross spectral densities. |

## 3. Who does what

A law only computes the next drive request. The core keeps every protection, and applies it to every
law, built-in or imported.

| The law | The core |
|---|---|
| Computes the next drive spectrum (single output) or drive SDM (MIMO) from the inputs it is given | Acquires data, windows, averages, and estimates spectra and FRFs |
| Keeps its own state between loops | Runs system identification before control starts |
| Declares its parameters and diagnostics | Synthesizes the drive time history from the law's request (random phase, overlap-add) |
| Reports diagnostics each loop | Runs the level schedule, soft start and ramps |
| | Enforces the drive ceiling and drive abort, the per-loop change cap, limit notching, the envelope alarm and abort, divergence, kinematic and overload checks |
| | Checks every output, and enforces the time and memory budget |
| | Records which law ran, with what parameters, in every run |

The core may reduce a law's request to satisfy a protection. It never makes a request more
aggressive than the law asked for. A law's own diagnostics can add a trip (for example, a law that
reports divergent lines), but never suppress one.

## 4. Requirements for a law file

| # | Requirement |
|---|---|
| R1 | **Format.** A law file is a WebAssembly component that implements exactly one world of the package `hertzforge:control-law` at a version the controller supports. The WIT files under `wit/control-law/` are the authoritative interface. |
| R2 | **One file for every platform.** The same file runs on every operating system Hertzforge supports. |
| R3 | **Manifest.** The law describes itself: name, version (semantic versioning), author, a one-paragraph summary, its declared parameters and diagnostics, and for MIMO the channel shapes it supports, the optional inputs it needs and the layout of each diagnostic (section 7). |
| R4 | **No system access.** The controller grants a law no files, no network, no environment and no arguments. Its clock and random source are fixed, so a law must not depend on them. Anything it prints is discarded. |
| R5 | **Deterministic.** The same inputs and parameter values give the same outputs. This is what makes a recorded run replayable. |
| R6 | **Budgets.** A law gets a memory cap (256 MiB by default) and a time budget per call (by default a quarter of one loop's acquisition time). Exceeding either is a failed call. |
| R7 | **Errors.** A law that cannot compute a sound drive returns an error with a message instead of a value. A trap (for example a panic) is a failed call. |
| R8 | **Valid outputs.** Outputs have the stated dimensions and are finite. A single-output drive spectrum is non-negative; a MIMO drive SDM is Hermitian and positive semidefinite (the core repairs a small negative eigenvalue and refuses a large one, section 7). Lines outside the band are zero. An invalid output is a failed call. |
| R9 | **Failure handling (core).** On a failed call the core holds the previous drive. Three failed calls in a row (configurable) end the run with a controlled stop, recorded with the reason. |
| R10 | **State.** A law keeps its state in the resource the core constructs for each run. Nothing carries over between runs. |
| R11 | **Parameters.** Only the declared kinds in section 5. The core validates every value against the declaration before the law is constructed. There are no free-form strings for a law to parse. |
| R12 | **Identity and records.** A law file is identified by the SHA-256 of its bytes. Every run records the law's name, version, hash and parameter values, the law file itself, and each loop's law inputs and outputs. |
| R13 | **Check before hardware.** `law check` runs a law against a fixed set of simulated test articles (one mode, two close modes, a deep anti-resonance with low coherence, measurement noise, a level ladder) and records a pass against the file's hash. The simulator accepts any law; hardware arms only with a law that has passed. |
| R14 | **Licensing.** A law file is a separate work loaded at run time; its author chooses its license. Laws shipped with Hertzforge are MIT. |
| R15 | **Versioning.** While the interface is 0.x it may change. From 1.0, minor versions only add, and the controller loads the current and the previous major version. |

## 5. Common types

### Parameters

A law declares each parameter with a key, a label, help text, and one of these kinds. The UI draws
the input from the declaration; a law ships no UI code.

| Kind | Declares | Example |
|---|---|---|
| Number | Unit, minimum, maximum, step, default | Correction gain, 0 to 1 |
| Integer | Minimum, maximum, step, default | Singular values kept per line |
| Toggle | Default | Use the trust gate |
| Choice | The options; the first is the default | Regularization method |
| Curve | A value against frequency, as a breakpoint table, with unit and default | Gain against frequency |
| Per channel | One number per control channel, with unit, range and default | Control channel weights |

### Diagnostics

A law may declare named diagnostics, each with a unit. A single-output law reports numbers; a MIMO
law declares the layout of each one: a scalar per loop, a value per line, per control per line or per
drive per line. They are shown to the operator and can be plotted like any other quantity. They are
recorded with the run.

### Calls

Every world has the same life cycle:

1. `describe()`: return the manifest. Called when the file is imported.
2. Construct the law's state with the test setup and the parameter values. Called once per run.
3. `seed(inputs)`: called once after system identification; returns the first drive request.
4. `iterate(inputs)`: called once per control loop with a fresh estimate; returns the next drive request.
5. Single output only: `retarget-level(level-db, target-rms)`, called when the level schedule moves to a new level; returns the drive request at the new level, checked like any other. `level-db` is the new level relative to the full reference, `target-rms` the reference's in-band RMS at that level. A failed call keeps the drive in force, which the core carries to the new level itself. A MIMO law works at full level and has no such call (section 7).

## 6. Single-output Random (`random-single`)

One drive, one or more control channels. The core combines the control channels into one composite
spectrum by the operator's rule (weighted average, minimum or maximum per line) before the law sees
it.

**Inputs, per call.** All arrays have one value per line of the full grid.

| Input | Meaning | Unit |
|---|---|---|
| Target PSD | The reference at full level (0 dB) | EU²/Hz |
| Measured PSD | The composite control spectrum from the latest estimate | EU²/Hz |
| Plant gain | \|H\|², drive to composite control response | (EU/V)² |
| Coherence | Ordinary coherence, drive to composite control | 0 to 1 |
| Current level | The scheduled level | dB re full |
| Line spacing | `df` | Hz |
| Band | `[lo, hi)` | lines |
| Notched lines | Lines the core's limiting held down last loop; a law must not try to raise them | flags |
| Degrees of freedom | Of the measured estimate | count |
| Loop index | Loops since control started | count |

**Outputs.** The drive PSD (V²/Hz per line, before the core's limiting), the level the law is
commanding (dB re full), and its diagnostics.

The single-output law supplies magnitude only. The drive synthesizer draws a random phase per line.

## 7. MIMO Random (`random-mimo`)

`n_c` control channels, `n_d` drives, `N` lines in the full grid. The world is in
[`wit/control-law/random-mimo.wit`](../wit/control-law/random-mimo.wit), which states these
conventions in its comments too.

**Conventions.**

- **Spectral matrix.** An SDM is `G = lim (1/T) E[X X^H]` per line, so a response `Y = H X` has
  `G_yy = H G_xx H^H`. This is the complex conjugate of the Bendat and Piersol definition
  (`G_xy = E[X* Y]`) that much analysis software uses; mixing the two silently conjugates every
  cross-spectral phase. A law that brings in cross spectra from elsewhere conjugates them first.
- **One-sided.** The integral of a PSD over frequency is the signal's variance.
- **Whole grid.** One call covers every line, in line order, so a law may work across lines (for
  example, smoothing its correction in frequency). Lines outside the band carry no drive.
- **Complex values** are pairs of 64-bit floats (real, imaginary).
- **Rectangular stacks** (the FRF, the response-to-drive cross spectrum) are passed in full,
  row-major within a line, lines in order: element `(r, c)` of line `l` is
  `data[(l * rows + r) * cols + c]`. The FRF is response over drive, `n_c` by `n_d`.
- **Hermitian stacks** (every SDM) are passed packed: the upper triangle of each line, row-major,
  lines in order. For `r <= c`, element `(r, c)` of line `l` is
  `data[l * n (n + 1) / 2 + r (2 n - r + 1) / 2 + (c - r)]`; an element below the diagonal is the
  conjugate of its mirror, and a diagonal value is real. This halves the largest copies.
- **Units.** A control SDM in EU²/Hz, a drive SDM in V²/Hz, the FRF in EU/V.
- **Full level.** Everything a law sees and returns is at full level (0 dB). The reference is the
  full-level specification, the measured response has been referred to its full-level equivalent
  by the core, and the drive the law returns is the full-level drive. The core applies the level
  schedule. A law never sees the level, cannot raise it, and has no `retarget-level` call; an
  error-correcting law still sees the plant's level dependence through the measured response.
- **Spaces.** The controls are the channels the reference is written in (physical channels, or
  virtual degrees of freedom after the response transformation); the drives are the drives the law
  commands (physical outputs, or virtual drives before the drive transformation). The core
  transforms both ways.

**Manifest.** `describe()` returns the manifest every law has (name, version, author, summary,
parameters, diagnostics) and three things more:

| Field | Meaning |
|---|---|
| Shapes | Which shapes the law supports: square (`n_c` = `n_d`), more controls than drives, fewer controls than drives. At least one. The host refuses a run of another shape. |
| Needs | The optional inputs the law reads (below). The core passes only these. |
| Diagnostic layouts | For each declared diagnostic, exactly one: a scalar, one value per line, per control per line, or per drive per line. |

A per-channel parameter holds one value per control channel.

**Calls.** The constructor takes the setup (`n_c`, `n_d`, `N`, line spacing, band, the reference
SDM and the tolerance lines) and the parameter values. `seed(inputs)` is called once after system
identification with the identification response as the measured response, and returns the first
drive. `iterate(inputs)` is called once per control loop.

**Inputs, per call.**

| Input | Shape per line | Meaning |
|---|---|---|
| Reference SDM | `n_c` x `n_c`, packed | The specification at full level |
| Alarm and abort lines | `n_c`, four of them (alarm upper and lower, abort upper and lower) | Each control's tolerance lines as PSDs at full level, so a law can weight its effort by where the tolerance is tight; NaN where a control has none |
| Measured response SDM | `n_c` x `n_c`, packed | The latest control estimate, referred to full level |
| Degrees of freedom | scalar | Of the measured estimate |
| FRF | `n_c` x `n_d` | The plant estimate the core is using, held or updated (question 2) |
| Multiple coherence | `n_c` | Of each control channel on all drives |
| Line spacing, band, loop index | scalars | As in section 6; the loop index is 0 at the seed |

**Optional inputs**, passed only when the manifest declares them (at full scale each is tens of
megabytes a loop, so a law that does not read one never pays for it):

| Need | Shape per line | Meaning |
|---|---|---|
| Last drive | `n_d` x `n_d`, packed | The drive played last loop, after the core's limits, at full level |
| Response-drive cross spectrum | `n_c` x `n_d` | This loop's measurement, so a law can adapt its own FRF estimate |
| Identification response | `n_c` x `n_c`, packed | The control SDM measured during system identification; used by laws that match the identification's coherence and phase |
| Identification drive | `n_d` x `n_d`, packed | The drive SDM of system identification |
| Identification noise | `n_c` x `n_c`, packed | The control SDM measured with the drives off |

**Output.** The next full-level drive SDM, `n_d` x `n_d` per line, packed (V²/Hz), plus the
declared diagnostics in their declared layouts.

**What the host checks.** Every output, before the core sees it; a failure is a failed call that
names the rule:

| Rule | Check |
|---|---|
| Dimensions | `N` lines of `n_d` x `n_d`, packed |
| Finite | Every value finite |
| Hermitian | `||G - G^H|| / ||G||` (Frobenius, per line) at most `1e-9`. Packed, the only way to break it is an imaginary part on a diagonal |
| Out of band | Every line outside the band exactly zero |
| Diagnostics | Each declared, reported at most once, in its declared layout and length, finite |

The host passes a well-formed drive that is not positive semidefinite on to the core, which judges
it (below). The inputs are checked too (dimensions, finiteness, declared needs present); a failure
there is the caller's, never the law's.

**What the core does with the output.** The law proposes; the core disposes, for every law alike.

- **Positive semidefinite.** Eigenvalues below `-1e-6` times the largest at that line are clamped
  to zero, the diagonal is restored to the requested drive PSDs by a positive diagonal congruence,
  and the clamps are counted and shown; a negative eigenvalue larger in magnitude than `1e-3` times
  the largest fails the call.
- **Per-loop change cap** on each drive's PSD, by default +3.5 dB up and -6.0 dB down per loop,
  applied as a diagonal congruence `D^(1/2) G D^(1/2)`, which keeps the matrix positive
  semidefinite and every pair's coherence.
- **Drive ceilings** per drive, by the same congruence.
- **Response limiting.** Each limit channel's PSD is predicted from the FRF and the drive SDM, and
  where a prediction exceeds its ceiling the line's whole drive SDM is scaled down: the provable
  floor. Laws may limit more cleverly within it.
- **Synthesis.** Each line's SDM is factored (by its eigen square root, which handles the
  rank-deficient lines a MIMO reference often has), random phases are applied, and the drives are
  overlap-added in time.
- **Tolerances.** Alarm and abort judge each control channel's PSD, the diagonal of the measured
  response SDM. Coherence and phase errors are shown and recorded.

**Built-in laws**, each from the published literature, each run natively and also built as a law
file from the same source and held to the native law in the tests:

| Id | Law | Method |
|---|---|---|
| `pinv_closed` (default) | Pseudo-inverse, closed loop | The regularized pseudo-inverse drive `H^+ G_ref H^+^H`, then each loop a correction of a lower-triangular factor of the reference from the measured error (D. O. Smallwood, "Multiple Shaker Random Vibration Control: An Update", Proc. IEST 1999): autospectra first in channel order, an amplitude-only form for a control above its reference, each factor diagonal and each drive PSD held to the per-loop cap, no more independent sources than the plant carries. Every drive is positive semidefinite by construction. With fewer sources than controls the correction runs in the coordinates of the plant's reachable subspace and settles at the least-squares response `P G_ref P^H`, `P = H H^+` |
| `pinv_open` | Pseudo-inverse, open loop | `H^+ G_ref H^+^H` every loop, truncated-SVD or Tikhonov regularization (R. Schultz, "A Demonstration of Force Estimation and Regularization Methods for Multi-Shaker Testing", IMAC proceedings), optionally weighting each control's error by its reference PSD: least squares with more controls than drives, minimum norm with fewer |
| `shape_svd` | Shape-constrained (SVD) | The drive held to the `m` leading right singular directions of the plant on each line, `m` fixed or set by a singular-value ratio (R. Schultz and P. Avitabile, "Shape-Constrained Input Estimation for Efficient Multi-shaker Vibration Testing", Experimental Techniques 44, 2020); closed loop by the default law's correction inside the constraint |
| `buzz_match` | Buzz-test matched | The reference's autospectra with each pair's coherence and phase taken from the identification response, pairs below a coherence floor left independent (after P. M. Daborn, C. Roberts, D. J. Ewins and P. R. Ind, "Next-Generation Random Vibration Tests", IMAC XXXII, 2014, as described by Schultz); then the default law's closed loop |

Each declares the condition number and the singular values kept per line and the largest drive
change of the loop as diagnostics; the closed-loop laws also count the clamped factor diagonals.

**Versioning.** The package stays at 0.1.0. Freezing this world changed only `random-mimo`, which
was published as an unstable draft no host loaded and no law could be built against;
`random-single` is unchanged, so every single-output law file keeps loading (R15).

## 8. Later test types

Each test type gets its own world, built on the same mechanism.

| Test type | When the law runs | Note |
|---|---|---|
| Shock, SRS | Between pulses | Slack time budget |
| Time waveform replication | Between passes | Slack time budget |
| Sine | Every servo update | Far more frequent than Random, so the budget is tight; to be measured before deciding how much of the sine servo can be changed |
| MIMO variants of the above | As above | After MIMO Random |

## 9. Writing a law

Hertzforge publishes a Rust crate for law authors (the interface bindings for both worlds, plus
helpers such as the default level servo and, for MIMO, the controller's own matrix types and
per-line linear algebra), a template project, and a worked example for each world. A law builds with
`cargo build --target wasm32-wasip2 --release`. Any language that can build a WebAssembly component
against the WIT files can produce a law file (C and C++ can, for example); only Rust is supported
for now. A law should pass `law check` before it is used on hardware.

## 10. Open questions for reviewers

1. **MIMO inputs.** *Answered:* the inputs of section 7. The large optional ones (the last drive,
   the response-drive cross spectrum, the identification response, drive and noise) are declared in
   the manifest and passed only to a law that declares them. Per-frame spectra, other FRF
   estimators, an uncertainty on the FRF and time histories are not passed in this version.
2. **The FRF during the test.** *Answered:* the operator's choice, per test. Hold (the default)
   keeps the identification FRF; Update lets the core re-estimate it every loop from the played drive
   and the measured response, blended with the identification estimate. A law receives whichever the
   setting selects, and a law that adapts on its own declares the response-drive cross spectrum.
3. **MIMO limiting.** *Answered:* yes. The core predicts each limit channel's PSD from the FRF and
   the drive SDM and scales the line's whole drive SDM down where a prediction exceeds its ceiling;
   laws may limit more cleverly within that floor.
4. **MIMO tolerances.** Should alarm and abort judge only each control channel's PSD, or also the
   cross terms (coherence and phase tolerances)?
5. **Repair or reject.** *Answered:* both, by size. Eigenvalues below `-1e-6` times the largest
   are clamped to zero, the requested drive PSDs restored and the clamps counted; one larger in
   magnitude than `1e-3` times the largest fails the call. The core does this, not the host, so every
   law is judged alike (section 7).
6. **References that change.** Some methods derive the reference during the test (for example a
   minimum-PSD reference). Should a law be allowed to change the reference, or should that be a
   separate kind of plug-in with its own checks?
7. **Who owns the level in MIMO.** *Answered:* the core owns the level and the law the matrix. A
   MIMO law works entirely at full level; the core scales its drive to the scheduled level, so every
   law behaves alike under the level schedule and no law can raise the level.
8. **Unchecked laws on hardware.** Block until `law check` passes (as R13 proposes), or warn and
   allow?
9. **Transport.** *Answered:* Hermitian matrices travel packed, the upper triangle row-major;
   rectangular matrices in full. The SDK unpacks them.
10. **Cadence.** One call per control loop. Does any law need a different cadence?
