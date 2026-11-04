# Control law plug-ins: specification

**Status: draft for comment.** Interface version 0.1, unstable. Comments are wanted on every part,
and most of all on the MIMO Random interface (section 7) and the open questions (section 10).

## 1. Purpose

Hertzforge lets the math that closes a vibration control loop be changed without rebuilding the
program. Each test type ships a default control law and may ship others; an operator chooses one per
test, and can import a law of their own as a single file. This document states what a law file must
be, what the controller guarantees around it, and the interface for each test type.

Single-output Random is the pilot. MIMO Random is next, and its interface is drafted here so it can
be reviewed before any of it is built.

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
| R3 | **Manifest.** The law describes itself: name, version (semantic versioning), author, a one-paragraph summary, its declared parameters and diagnostics, and for MIMO the channel shapes it supports (section 7). |
| R4 | **No system access.** The controller grants a law no files, no network, no environment and no arguments. Its clock and random source are fixed, so a law must not depend on them. Anything it prints is discarded. |
| R5 | **Deterministic.** The same inputs and parameter values give the same outputs. This is what makes a recorded run replayable. |
| R6 | **Budgets.** A law gets a memory cap (256 MiB by default) and a time budget per call (by default a quarter of one loop's acquisition time). Exceeding either is a failed call. |
| R7 | **Errors.** A law that cannot compute a sound drive returns an error with a message instead of a value. A trap (for example a panic) is a failed call. |
| R8 | **Valid outputs.** Outputs have the stated dimensions and are finite. A single-output drive spectrum is non-negative; a MIMO drive SDM is Hermitian and positive semidefinite. Lines outside the band are zero. An invalid output is a failed call. |
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

A law may declare named diagnostics: a scalar per loop or a series per line, each with a unit. They
are shown to the operator and can be plotted like any other quantity. They are recorded with the run.

### Calls

Every world has the same life cycle:

1. `describe()`: return the manifest. Called when the file is imported.
2. Construct the law's state with the test setup and the parameter values. Called once per run.
3. `seed(inputs)`: called once after system identification; returns the first drive request.
4. `iterate(inputs)`: called once per control loop with a fresh estimate; returns the next drive request.
5. `retarget-level(level-db, ...)`: called when the level schedule moves to a new level; returns the drive request at the new level, checked like any other. `level-db` is the new level relative to the full reference. Single output also passes `target-rms`, the reference's in-band RMS at that level. A failed call keeps the drive in force, which the core carries to the new level itself.

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

## 7. MIMO Random (`random-mimo`), draft

`n_c` control channels, `n_d` drives, `N` lines in the full grid.

**Conventions.**

- One call covers the whole grid, not one line, so a law may work across lines (for example,
  smoothing its correction in frequency).
- SDMs are one-sided. Complex values are pairs of 64-bit floats (real, imaginary). Each line's
  matrix is stored row-major, and lines are in order. Hermitian matrices are passed in full.
- FRFs are response over drive, `n_c` by `n_d`.
- Cross terms, proposed (open question 11). The off-diagonal entry `(i, j)` of an SDM is
  `S_ij = E[Z_i conj(Z_j)]`, where `Z` is the spectrum under the textbook forward transform,
  `Z[k] = sum_n z[n] exp(-j 2 pi k n / N)`. The angle of `S_ij` is then the phase of channel `i`
  minus the phase of channel `j`, and a response SDM follows from a drive SDM as
  `S_yy = H S_vv H^H`, with `H` response over drive and `H^H` its conjugate transpose. The
  response-to-drive cross spectrum has entries `E[Y_c conj(V_d)]`, so `H = S_yv inv(S_vv)`.
- The other form, `E[conj(Z_i) Z_j]`, is the conjugate of this one, and under it the same relation
  reads `S_yy = conj(H) S_vv transpose(H)`. Rattlesnake and SDynPy hold the proposed form, so a law
  ported from Rattlesnake needs no conjugation. Hertzforge's analysis side stores the other form in
  its files, and so does the core's estimator, because its FFT has a positive exponent, which
  conjugates every spectrum (see `h_estimate_hv` in `hertzforge-dsp`). The core would convert at
  this boundary; a law should not.

**Inputs, per call.**

| Input | Shape per line | Meaning |
|---|---|---|
| Reference SDM | `n_c` x `n_c` | The specification at full level (0 dB) |
| Current level | scalar | dB re full |
| Measured response SDM | `n_c` x `n_c` | The latest control estimate |
| Degrees of freedom | scalar | Of the measured estimate |
| FRF | `n_c` x `n_d` | The core's current estimate, response over drive (EU/V) |
| Measured cross spectrum | `n_c` x `n_d` | Response to drive, `E[Y_c conj(V_d)]`, this loop, so a law can adapt its own FRF estimate |
| Last drive SDM | `n_d` x `n_d` | The drive actually played last loop, after the core's limiting (V²/Hz) |
| Multiple coherence | `n_c` | Of each control channel on all drives |
| Noise SDM | `n_c` x `n_c` | Control responses measured with the drives off, during system identification |
| Identification response and drive SDMs | `n_c` x `n_c`, `n_d` x `n_d` | Measured during system identification; used by laws that match the identification's coherence and phase |
| Line spacing, band, loop index | scalars | As in section 6 |

**Output.** The next drive SDM, `n_d` x `n_d` per line (V²/Hz), Hermitian and positive
semidefinite, plus diagnostics (for example, the condition number or the singular values kept per
line).

**Shapes.** The manifest declares which shapes the law supports: square (`n_c` = `n_d`), more
controls than drives, fewer controls than drives. The controller refuses a law for a test whose
shape it does not support.

**What the core does with the output** (proposed; see the open questions):

- Rejects a matrix that is not Hermitian to within a tolerance. Small negative eigenvalues (relative
  to the largest at that line) are clamped to zero and counted; larger ones reject the output.
- Applies the per-loop change cap to each drive's PSD (the diagonal).
- Keeps each drive within its ceiling by scaling the whole drive SDM, which preserves its coherence
  and phase.
- Limits response channels by predicting each limit channel's PSD from the FRF and the drive SDM,
  and scaling that line's drive SDM down where a prediction exceeds its ceiling.
- Synthesizes the drive by factoring each line's SDM and applying random phase, then overlap-adds
  in time.
- Judges the alarm and abort tolerances on each control channel's PSD (the diagonal of the
  measured response SDM).

**Candidate built-in laws**, each written from the published literature:

- Regularized pseudo-inverse, open loop (least squares when there are more controls than drives;
  minimum norm when there are fewer).
- Closed-loop error correction of the drive SDM.
- Shape-constrained control, which keeps the drive in the dominant singular directions of the FRF.
- Control that matches the coherence and phase seen during system identification.

## 8. Later test types

Each test type gets its own world, built on the same mechanism.

| Test type | When the law runs | Note |
|---|---|---|
| Shock, SRS | Between pulses | Slack time budget |
| Time waveform replication | Between passes | Slack time budget |
| Sine | Every servo update | Far more frequent than Random, so the budget is tight; to be measured before deciding how much of the sine servo can be changed |
| MIMO variants of the above | As above | After MIMO Random |

## 9. Writing a law

Hertzforge publishes a Rust crate for law authors (the interface bindings plus helpers such as the
default level servo), a template project, and a worked example. A law builds with
`cargo build --target wasm32-wasip2 --release`. Any language that can build a WebAssembly component
against the WIT files can produce a law file (C and C++ can, for example); only Rust is supported
for now. A law should pass `law check` before it is used on hardware.

## 10. Open questions for reviewers

1. **MIMO inputs.** Does a law need anything not in section 7? For example: per-frame spectra rather
   than averaged SDMs, more than one FRF estimator (H1, H2, Hv), an uncertainty on the FRF, or time
   histories.
2. **The FRF during the test.** Should the core keep updating its FRF estimate during the test and
   pass the current one, or hold the identification FRF fixed and leave adaptation to laws (which
   receive this loop's cross spectrum for that purpose)?
3. **MIMO limiting.** Is per-line uniform scaling of the drive SDM the right floor for the core, with
   laws free to limit more cleverly within it?
4. **MIMO tolerances.** Should alarm and abort judge only each control channel's PSD, or also the
   cross terms (coherence and phase tolerances)?
5. **Repair or reject.** Should a drive SDM with small negative eigenvalues be repaired (clamped) or
   rejected outright?
6. **References that change.** Some methods derive the reference during the test (for example a
   minimum-PSD reference). Should a law be allowed to change the reference, or should that be a
   separate kind of plug-in with its own checks?
7. **Who owns the level in MIMO.** In single output the law owns the whole drive, level included.
   For MIMO, should the core own the overall level and the law only the matrix structure?
8. **Unchecked laws on hardware.** Block until `law check` passes (as R13 proposes), or warn and
   allow?
9. **Transport.** Pass Hermitian matrices in full (as proposed), or the upper triangle only?
10. **Cadence.** One call per control loop. Does any law need a different cadence?
11. **Cross-term form.** Section 7 proposes `S_ij = E[Z_i conj(Z_j)]` under the negative-exponent
    transform, so that `S_yy = H S_vv H^H` and a law ported from Rattlesnake needs no conjugation.
    Hertzforge's analysis files and the core's estimator hold the conjugate form, so the core would
    convert at this boundary. Is the boundary the right place for that conversion, or should it
    carry Hertzforge's stored form and leave the conjugation to laws?
