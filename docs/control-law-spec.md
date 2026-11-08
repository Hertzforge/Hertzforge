# Control law plug-ins: specification

**Status: draft for comment.** Interface version 0.1. Both Random worlds, single-output
(section 6) and MIMO (section 7), classical shock (section 8), SRS shock (section 9) and
single-output Sine (section 10) are implemented: the host loads them and the built-in laws run
through them. While the interface is 0.x it may still change (R15). Comments are wanted on every
part, and on the questions still open (section 13).

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
| R13 | **Check before hardware.** `law check` runs a law against a fixed set of simulated test articles (one mode, two close modes, a deep anti-resonance with low coherence, measurement noise, a level ladder) and records a pass against the file's hash. The simulator accepts any law; hardware arms only with a law that has passed. A multiple-output law is checked on its own set: the well-conditioned 4 x 4 and 12 x 6 modal plants, an ill-conditioned 12 x 6, a rigid plate, a plant with measurement noise and a level ladder from -20 dB, each a whole run under the core's guards (`laws.check`; the guide's "Checking a MIMO law before hardware"). A classical shock law is checked on seven simulated test articles, each a whole run under the core's guards (section 8; the guide's "Checking a classical shock law before hardware"), an SRS shock law on six (section 9; the guide's "Checking an SRS shock law before hardware"), and a Sine law on seven (section 10; the guide's "Checking a Sine law before hardware"). |
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

- **Spectral matrix.** An SDM is `G = lim (1/T) E[X X^H]` per line, with `X` the spectrum under the
  negative-exponent forward transform, `X[k] = sum_n x[n] exp(-j 2 pi k n / N)`. The off-diagonal
  entry `(i, j)` is `E[X_i conj(X_j)]`, its angle the phase of channel `i` minus the phase of
  channel `j`, and a response `Y = H X` has `G_yy = H G_xx H^H`, with `H` response over drive and
  `H^H` its conjugate transpose; the response-to-drive cross spectrum has entries
  `E[Y_c conj(V_d)]`, so `H = S_yv inv(S_vv)`. Rattlesnake and SDynPy hold this form, so a law
  ported from either needs no conjugation. It is the complex conjugate of the Bendat and Piersol
  definition (`G_xy = E[X* Y]`) that Hertzforge's analysis files and much other software hold,
  under which the same relation reads `S_yy = conj(H) S_vv transpose(H)`; mixing the two silently
  conjugates every cross-spectral phase. The core forms the boundary's form directly: its estimator
  conjugates the workspace FFT's positive-exponent spectra (`SdmEstimator` in
  `hertzforge-mimo-random`, pinned by a known-delay test), and every reference import converts
  where its source holds the other form (`docs/reference-file.md`). A law never converts.
- **One-sided.** The integral of a PSD over frequency is the signal's variance.
- **Whole grid.** One call covers every line, in line order, so a law may work across lines (for
  example, smoothing its correction in frequency). Lines outside the band carry no drive.
- **Complex values** are a record of two f64, `re` and `im`.
- **Rectangular stacks** (the FRF, the response-to-drive cross spectrum) are passed in full,
  row-major within a line, lines in order: element `(r, c)` of line `l` is
  `data[(l * rows + r) * cols + c]`. The FRF is response over drive, `n_c` by `n_d`.
- **Hermitian stacks** (every SDM) are passed packed: the upper triangle of each line, row-major,
  lines in order. For `r <= c`, element `(r, c)` of line `l` is
  `data[l * n * (n + 1) / 2 + r * (2 * n - r + 1) / 2 + (c - r)]`; an element below the diagonal
  is the conjugate of its mirror, and a diagonal value is real (its imaginary part is zero). This
  halves the largest copies.
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
| Shapes | Which shapes the law supports: square (`n_c` = `n_d`), more controls than drives (least squares), fewer controls than drives (minimum norm). At least one, each at most once. The controller refuses a test of another shape. |
| Needs | The optional inputs the law reads (below), each at most once. The core passes only these; the others arrive as `none`. |
| Diagnostic layouts | For each declared diagnostic, exactly one: a scalar (one number for the loop), one value per line (`N` values), one per control per line (`N * n_c` values, line after line), or one per drive per line (`N * n_d` values, line after line). |

A per-channel parameter holds one value per control channel.

**Calls.** The constructor takes the setup (`n_c`, `n_d`, `N`, line spacing, band, the reference
SDM and the tolerance lines) and the parameter values: every declared parameter, already checked
against the manifest, in manifest order, with defaults for the values the operator left out. An
error refuses the run with the reason given. `seed(inputs)` is called once after system
identification with the identification response as the measured response, and returns the first
drive. `iterate(inputs)` is called once per control loop.

**Inputs, per call.**

| Input | Shape per line | Meaning |
|---|---|---|
| Reference SDM | `n_c` x `n_c`, packed | The specification at full level |
| Alarm and abort lines | `n_c`, four of them (alarm upper and lower, abort upper and lower) | Each control's tolerance lines as PSDs at full level, so a law can weight its effort by where the tolerance is tight; NaN where a control has none (`N * n_c` values each, line after line) |
| Measured response SDM | `n_c` x `n_c`, packed | The latest control estimate, referred to full level |
| Degrees of freedom | scalar | Of the measured estimate |
| FRF | `n_c` x `n_d` | The plant estimate the core is using, held or updated (question 2) |
| Multiple coherence | `n_c` | Of each control channel on all drives, 0 to 1 (`N * n_c` values, line after line) |
| Line spacing, band, loop index | scalars | As in section 6; the loop index is 0 at the seed, then 1, 2, ... for each iteration |

**Optional inputs**, passed only when the manifest declares them (at full scale each is tens of
megabytes a loop, so a law that does not read one never pays for it):

| Need | Shape per line | Meaning |
|---|---|---|
| Last drive | `n_d` x `n_d`, packed | The drive played last loop, after the core's limits, at full level |
| Response-drive cross spectrum | `n_c` x `n_d` | This loop's measurement, `E[Y_c conj(V_d)]`, so a law can adapt its own FRF estimate as `H = S_yv inv(S_vv)` |
| Identification response | `n_c` x `n_c`, packed | The control SDM measured during system identification; used by laws that match the identification's coherence and phase |
| Identification drive | `n_d` x `n_d`, packed | The drive SDM of system identification |
| Identification noise | `n_c` x `n_c`, packed | The control SDM measured with the drives off |

**Output.** The next full-level drive SDM, `n_d` x `n_d` per line, packed (V²/Hz): finite, Hermitian
(real diagonal), positive semidefinite on every line, and zero on every line outside the band. Plus
the declared diagnostics, each at most once, finite, in its declared layout; they are shown and
recorded, never relied on for protection.

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
| `min_drive` | Minimum drive | The reference's autospectra, as far as a trade-off against drive power allows, with the cross terms of the drive that needs the least power to produce them: per line the drive minimizing the autospectrum residuals plus the trade-off times the drive power over the positive semi-definite cone, and the plant's predicted response to it as the target (K. Coletti, R. B. Davis and R. Schultz, "Pareto-optimal target definition for multi-axis random vibration testing", 2026; D. O. Smallwood, "Minimum Input Trace for Multiple Input Multiple Output Linear Systems", Journal of the IEST 56, 2013, at the exact-control end); then the default law's closed loop |

Each declares the condition number and the singular values kept per line and the largest drive
change of the loop as diagnostics; the closed-loop laws also count the clamped factor diagonals.

**Versioning.** The package stays at 0.1.0. Freezing this world changed only `random-mimo`, which
was published as an unstable draft no host loaded and no law could be built against;
`random-single` is unchanged, so every single-output law file keeps loading (R15).

## 8. Classical shock (`shock-classical`)

A classical shock law closes the loop between pulses: it proposes the next drive record, and the
controller checks it, scales it to the scheduled level, plays it, measures and judges the pulse,
and calls the law again. The world is `wit/control-law/shock-classical.wit`.

**When it runs.** Once after system identification (`seed`), then once after each fired, measured
and judged pulse (`iterate`), on the run's thread and never while a pulse plays. A train or a
gunfire burst fires the drive in force with no call inside it. Each call has a budget of a fraction
of the minimum time between pulses (`law_budget`, default 0.25: 250 ms at the default 1.0 s floor);
a law file's host deadline is the same number.

**What it sees.** Fixed for the run: the rate and the record length (a power of two), the
correction band, the full-level reference on the grid (the compensated pulse with its quiet
margins; the main pulse's first sample and length), the identified plant on the grid's bins
(`H` in EU per volt, coherence, the controller's regularized inverse
`conj(H) / (|H|^2 + epsilon)` with its epsilon, and the bins the controller trusts: in the band,
coherence at or over its floor, reference within its dynamic-range floor) and the drive ceiling.
Each call: the pulse index, the last response referred to full level by the scale the controller
actually commanded, the drive as played referred the same way, the controller's verdict (passed,
RMS error in percent, peak ratio, velocity-change ratio) and whether the controller scaled that
pulse down. Spectra are one-sided under the negative-exponent transform; the controller forms the
plant in that convention.

**What it returns.** The next full-level drive in volts, `record-len` finite samples, and up to
eight values its manifest declares. It does not decide the level, the ladder, when a pulse fires,
the spacing, a train, the polarity, the verdict or any abort.

**What the controller enforces, for every law.**

| Guard | Rule |
|---|---|
| Drive in volts | From the identified plant. Identification is band-limited random at a stated V rms, its peak held under the drive ceiling |
| Drive ceiling and abort | On the commanded record before it plays: over the abort it is refused and the run aborts (`drive_vpk`); over the ceiling it is scaled to it and the action logged. At proceed, the drive at the highest level the run is scheduled to reach over the abort ends the run before any pulse (`predicted`); a level move that would pass it is refused |
| Predicted response | Through `H` before every pulse: its peak against the drive limit, its velocity and displacement against the shaker's ratings (`kinematic`, `predicted`) |
| At rest | The outer half of each quiet margin tapers to zero, so every record starts and ends at 0 V |
| Output checks | Length, finite, at most eight declared values |
| Change caps | At full level, the drive's peak and any bin may rise at most `max_peak_rise_db` and `max_bin_rise_db` (default 10 dB each) over the drive in force, never tighter than the built-in's own limit plus 1 dB; bins under -40 dB of the largest count as that small |
| First drive | The seed's predicted response peak at most `first_drive_ratio` (default 2) times the reference's |
| Misses | A late, failed, crashed or refused call is a miss; the next pulse refires the drive in force. `law_misses` (default 3) in a row take the controlled stop (`law_fault`); a seed miss fires nothing |
| Measured pulse | Overshoot over `1 + overshoot_abort` times commanded aborts at any level (default twice the standard's amplitude tolerance); a tolerance failure aborts at full level (`full_level`, the default) or at every level (`every_level`), never for an undershoot |
| Convergence | `max_pulses_per_level` (default 5) pulses at one level without a pass, or failing in a row at one manual level, end the run `not_converged`, as does the run's `max_pulses` (default 100); the error rising more than 1 dB on `divergence_pulses` (default 3) pulses in a row at one level trips `divergence` |

**The built-in law, `fd_correct`.** The frequency-domain correction time waveform replication
uses, applied to the compensated pulse: `D0 = G R` on the trusted bins, then `D += beta G (R - Y)`
after each pulse, each bin's growth limited per pulse; the drive is the inverse transform of `D`.
Parameters `beta` (0.05 to 1, default 0.5), `max_db_per_pulse` (1 to 9, default 6) and
`initial_backoff_db` (0 to 12, default 0). Its seed and correction are time waveform replication's
bin for bin; it runs natively and as the law file `laws/shock-fdc`, and the two give the same run.

**Records.** At the seed the run folder gets `law.json` (the law's identity and parameters, the
guards and the caps); every call goes to `law_pulses` (an index line per call: pulse, seed or
iteration, used or the miss reason, call time, capped bins, verdict, values; the response, the
drive in and the drive out as `f32` in the payload), written between pulses. The report carries
the law's name, version and short hash and the guard counts.

**The check.** Seven simulated test articles, each a whole run under these guards: a wire (every
pulse passes), one mode (400 Hz, Q 10), the simulated station's plant (200 and 550 Hz, Q 2.5) with
a 6 ms haversine, one mode behind a 2 ms delay (each a pass by pulse 3 and every full-level pulse),
an identification error (the identified gain +3 dB: a pass within eight pulses at the first rung),
measurement noise (a pass by pulse 4) and a 30 dB notch at 300 Hz under the coherence floor
(survive). Every case asks every call answered in time, with no miss and no refused output.

## 9. SRS shock (`shock-srs`)

An SRS shock law closes the loop between pulses on a synthesized pulse: the controller synthesizes
every pulse from a table of components (odd-half-cycle wavelets or damped sines, one per band by
default), and the law proposes one gain per component. The controller applies the gains, caps
them, checks the drive, plays it, measures and judges the pulse, and calls the law again. A law
never writes a drive record, so a pulse's zero net velocity and displacement hold whatever it
returns. The world is `wit/control-law/shock-srs.wit`.

**When it runs.** Once after system identification and before the controller plans its limit notch
(`seed`), then once after each fired, measured and judged pulse (`iterate`), on the run's thread
and never while a pulse plays. Each call has a budget of a fraction of the time between pulses
(`law_budget`, default 0.25: 250 ms at the default 1.0 s floor); a law file's host deadline is the
same number.

**What it sees.** Fixed for the run: the band centres every spectrum is on, the components'
frequencies in table order (a separate grid, of any length), the full-level target SRS, the Q every
SRS is computed at, and the identified gain at each component, `|H|` in EU per volt floored by the
controller (absent when nothing was identified). Each call: the pulse index, the target at the scale
the last pulse was commanded at (the level times the controller's safety trims, so a law aims at
what was asked of that pulse and never fights a limiter), the last pulse's measured SRS (zeros at
the seed) and that scale.

**What it returns.** One multiplicative gain per component, finite and above zero (at the seed, the
starting gains relative to the table), and up to eight values its manifest declares. It does not
decide the level, the ladder, when a pulse fires, the spacing, the envelope verdict or any abort.

**What the controller enforces, for every law.**

| Guard | Rule |
|---|---|
| Drive in volts | From the identified gain. Identification plays the synthesized pulse at its level (`identification.level_db`, default -20 dB), its peak held at `identification.max_vpk` (default a tenth of the drive ceiling) |
| Coherence hold | A mean in-band coherence under `identification.min_mean_coherence` (default 0.8) ends the run `held` before any pulse |
| Drive ceiling and abort | On the synthesized drive before it plays: over the abort it is refused and the run aborts (`drive_vpk`); over the ceiling it is scaled to it and the action logged. At proceed, the drive at the highest level the run is scheduled to reach over the abort ends the run before any pulse (`predicted`); a level move that would pass it is refused |
| Predicted response | Through the identified gain before every pulse: its peak against the drive limit, its acceleration, velocity and displacement against the run's kinematic aborts and the shaker's ratings (`kinematic`, `predicted`) |
| Output checks | One gain per component, every number finite, every gain above zero, at most eight declared values |
| Gain cap | No gain may rise more than `max_gain_rise_db` (default 6 dB) in one pulse; an output that asks for more is refused (`gain_rise`). It never binds on the built-in at its default |
| Limit cap | After the law, each gain is held to `min(law gain, cap)`: the cap is the tightest allowed-over-predicted ratio over the limit channels and the bands within half an octave of the component, from the prediction for the last drive referred to the highest level the run is scheduled to reach; a rise toward the bound is damped to the square root, a binding cap is cut a further 0.5 dB, and nothing rises without a prediction. A falling gain always wins. A band the limit holds down (a band the arm-time notch cut, or within half an octave of one) that reads low does not fail a pulse |
| Misses | A late, failed, crashed or refused call is a miss; the next pulse refires the table in force. `law_misses` (default 3) in a row take the controlled stop (`law_fault`); a seed miss fires nothing |
| Measured pulse | The envelope at the level fired (alarm and abort bands), the measured kinematics and the drive monitors |
| Convergence and pacing | With `advance: on_pass` a pulse passes when no band is outside the alarm band; `max_pulses_per_level` (default 5) pulses at one level without a pass, or failing in a row at one manual level, end the run `not_converged`, as does the run's `max_pulses` (default 100). Pulses start no closer than the time between pulses |

**The built-in law, `srs_ratio`.** The classic per-band SRS iteration: the seed is `1 / |H(fc)|`
per component (1 when nothing was identified); after each pulse each component is scaled by the
target over the measured SRS at the band nearest its frequency, held at 1 on a band with no
response or no target, limited to `max_db_per_pulse` (0.5 to 5 dB, default 3) either way. It
reports the worst band's error and frequency and the components at its limit. It runs natively and
as the law file `laws/shock-srs-ratio`, and the two give the same run.

**Records.** At the seed the run folder gets `law.json` (the law's identity and parameters, the
guards and the gain cap); every call goes to `law_pulses` (an index line per call: pulse, seed or
iteration, used or the miss reason, call time, the commanded scale, the components the limit cap
held, values; the target and measured SRS as `f32` and the gains returned and applied as `f64` in
the payload), written between pulses. The report carries the law's name, version and short hash and
the guard counts.

**The check.** Six simulated test articles, each a whole run under these guards on a flat 1/3-octave
target from 100 to 2000 Hz at Q 10: one mode (400 Hz, Q 10), two close modes (240 and 260 Hz,
Q 30) and one mode with measurement noise (each every band within 3 dB by the sixth full-level
pulse and on every one after), a level ladder of -12, -6 and 0 dB with manual moves (every band
within 3 dB after three pulses at each level), one mode with the identified gain +3 dB (within 3 dB
by the eighth full-level pulse), and a wire with a limit channel whose supplied gain is 4 at 500 Hz
under a flat bound (the prediction at most 0.5 dB over the bound on every pulse, and the control
within 3 dB outside the bands the limit holds down). Every case asks every call answered in time,
with no miss and no refused output.

## 10. Single-output Sine (`sine-single`)

A Sine law decides one number per control tick: the amplitude of the one tone the controller plays,
in drive units peak (volts peak on hardware). The controller keeps the tone's frequency, the sweep
and its direction, its phase (continuous), the level schedule, the second reference output and
every protection. The world is `wit/control-law/sine-single.wit`.

**When it runs.** Once per control tick on the controller's real-time thread: `engage` once, after
system identification and before anything is driven at test level, for the starting drive; `tick`
on every tick after it. The tick is `tick_ms` (default 10 ms), whole frames at the rate the station
adopted; a built-in law may run at a 2 ms tick, a law file at no less than 8 ms. Each call has a
budget of `law_budget` (default 0.25) of the tick, 2.5 ms at 10 ms; a law file's host deadline is
the same number, checked on each of the host's 1 ms epoch ticks, so a late call is stopped one or
two epoch ticks past its budget, inside its control tick. The law is not called while the drive
ramps down. A number parameter the manifest lists as `live` can be changed by the operator during a
run (`adjust`): the controller checks the key and the value's declared range first, and the value
applies from the next tick.

**What it sees.** Fixed for the run: the control channels in the composite, the tick, the band, the
drive ceiling, and the identification on its own grid (`|H|` in control units peak per drive unit
peak, coherence, the controller's trust mask, the full-level target at each grid frequency). Each
tick: its index (0 at engage), the time since the last call, the frequency, the sweep rate (signed;
0 at a dwell or while held), the scheduled level, the target at this frequency and level, the
measured composite, each control channel's tracked amplitude and phase against the drive, the drive
in force as played, the prediction (`prediction-db`: the controller's plant-inverse drive at this
frequency at full level, dB relative to one drive unit; absent where the identification is not
trusted), why the controller holds the drive (`free`, `notch`, `probe`), the notch ceiling, and
whether the settle window is open.

**What it returns.** The drive, finite and at least zero, its own fault judgement (shown and
recorded, never relied on), and up to eight values its manifest declares. It does not decide the
frequency, the sweep, the phase, the level schedule, the reference output or any abort.

**What the controller enforces, for every law.**

| Guard | Rule |
|---|---|
| Output checks | A drive that is not a finite number at least zero, a value that is not finite or not declared: refused, a miss |
| Time budget | Each call timed against `law_budget` of the tick; a late call is a miss |
| Misses | A late, failed, panicked or refused call is a miss: the drive in force is held. A usable call resets the count; `law_misses` (default 3) in a row take the controlled stop (120 dB/s of station time, then the drain) and the run ends `law_fault`. The law is not called again |
| Change cap | At most `max_step_db` (default 1 dB, 1 to 3) from the drive in force in one tick, either way |
| Rise cap | At most `max_rise_db_per_s` (default 240 dB/s) up |
| Divergence | The change or rise cap binding for `divergence_s` (default 1 s) without a break takes the same controlled stop, trip `divergence` |
| First drive | At most the prediction at the start frequency and level plus 6 dB, and never over the ceiling; without a trusted prediction there, at most 0.05 times the start level's scale |
| Holds | While a limit channel holds the drive (`notch`) the notch ceiling applies after the law, so the law cannot raise it; while a resonance search maps the plant (`probe`) the law's drive is not used |
| Notch | A limit channel is held at its limit (the level its profile states); its abort edge above that ends the run |
| Drive ceiling and abort, the alarm and abort bands, the kinematic monitor, the servo fault, the raw watchdog | As for every Sine run, after the law and the notch |

**The built-in law, `servo_blend`.** An amplitude servo (proportional-integral on the error in dB,
`kp` default 0.3, `ki` and its rate limit from `compression`: slow 2 and 20 dB/s, fast 6 and
60 dB/s, or custom) moves its own level each tick; the blend adds `blend_w` (default 0.5, live)
times the prediction's change since the last tick and clamps the total to 1 dB a tick; the drive is
the drive in force moved by that total. The servo is held while the controller holds the drive, and
where the drive in force is under what the law last asked (a cap, a ceiling or the notch held it
down), the servo's level follows it down, so a long clamp does not wind the servo up. The first drive is the prediction at the start level (0.05 drive units at the start level without one).
It reports the servo's step, the prediction's step, the total and the servo's own level, dB. It runs
natively and as the law file `laws/sine-servo`, and the two give the same run.

**Records.** At engage, with a capture running, the run folder gets `law.json` (the law's identity,
parameters, values, live parameters and the guards); every call goes to `law_ticks.bin`, 136-byte
little-endian records described by `law_ticks.json` beside it (tick, frequency, target, measured,
the drive in force, the prediction, the hold, used or the miss reason, capped, the call's
microseconds, the law's drive, the drive the controller took, the eight values), written between
batches, never on the loop thread. The report carries the law's name, version and short hash and
the guard counts.

**The check.** Seven simulated test articles, each a whole run under these guards at 16384 Hz and
a 10 ms tick, a flat 1 g target, identification at -20 dB, the abort band 12 dB either way:

| Case | Plant and run | Asks for |
|---|---|---|
| slow sweep | One mode, 250 Hz, Q 50; a log sweep 100 to 800 Hz at 1 oct/min | Within 1.5 dB through the resonance (every judged tick within 30 Hz of it) |
| fast sweep | The same at 6 oct/min | Within 5 dB through the resonance, within 2 dB more than 30 Hz from it |
| close modes | 240 and 260 Hz, Q 30; 2 oct/min | Within 2 dB |
| anti-resonance | A flat plant cut 30 dB at 300 Hz, its coherence there under the trust floor; 2 oct/min | Survive: the run completes or ends on the controller's own divergence trip, the drive never over the ceiling |
| measurement noise | One mode; noise 30 dB under the full-level response; a 30 s dwell at 250 Hz | Within 1.5 dB |
| level ladder | One mode; -20, -12, -6 and 0 dB moved by hand, 10 s each, at a 250 Hz dwell, one live change to the middle of a live parameter's range | Every rung within 1 dB after its settle window |
| limit channel | One mode, and a limit channel reading up to 4 times the control at the mode under a flat 2 g limit; 2 oct/min | The limit channel at most 0.5 dB over its limit; the control at most 1 dB over its target after the notch releases |

A judged tick is one at full level (or, for the ladder, at a rung) outside the settle window and not
held by the notch. Every case also asks that every call is answered in time with no miss and no
refused output, and reports the slowest call and the capped ticks.

## 11. Later test types

Each test type gets its own world, built on the same mechanism.

| Test type | When the law runs | Note |
|---|---|---|
| Classical shock | Between pulses | Built: section 8 |
| SRS shock | Between pulses | Built: section 9 |
| Time waveform replication | Between passes | Slack time budget |
| Sine | Every control tick | Built: section 10 |
| MIMO variants of the above | As above | After MIMO Random |

## 12. Writing a law

Hertzforge publishes a Rust crate for law authors (the interface bindings for both worlds, plus
helpers such as the default level servo and, for MIMO, the controller's own matrix types and
per-line linear algebra), a template project, and a worked example for each world. A law builds with
`cargo build --target wasm32-wasip2 --release`. Any language that can build a WebAssembly component
against the WIT files can produce a law file (C and C++ can, for example); only Rust is supported
for now. A law should pass `law check` before it is used on hardware.

## 13. Open questions for reviewers

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
11. **Cross-term form.** *Answered:* the boundary carries `S_ij = E[Z_i conj(Z_j)]` under the
    negative-exponent transform (section 7), so `S_yy = H S_vv H^H` and a law ported from
    Rattlesnake needs no conjugation. The core forms it directly and converts where a file holds
    the other form; the conversion is the core's, never a law's.
