# MIMO Sine Benchmark Runner: Case and Run Formats

`msnb_run` runs one MIMO Sine case of the Open Benchmark Set through the shipped MIMO Sine engine
(`hertzforge-mimo-sine`: its identification, loop-speed rule, guards, level ladder and records)
on the simulator, and writes one run folder. This file describes the case file it reads
(`msnb-case/1`), the reference file the case names (`msnb-reference/1`) and the run folder it
writes (`msnb-run/1`). The machine-readable form of the case is `msnb-case-1.schema.json`
beside this file.

```text
cargo build --release -p sds-control --example msnb_run
msnb_run <case.json> --root <problem-root> --out <run-dir> [--ideal-frf] [--record]
```

- `<problem-root>`: the folder every path in the case is relative to (for the benchmark, the
  folder holding the problem folders). The run file never holds an absolute path.
- `<run-dir>`: made by the program; it must not exist yet or must be empty (a run folder is
  never overwritten).
- `--ideal-frf`: hand the law the structure's small-signal frequency response at the controls
  in place of the engine's identification (see "Identification" below). An option for the
  suite to separate the identification's error from the law's; not the default.
- `--record`: also write the raw samples of the engaged run (see "Recording"). For checks, not
  for the study: a 400 s run at 8192 Hz is about 40 MB a drive.

One run uses one core: the program starts no thread of its own and does no network or screen
work.

Exit status: `0` the run completed; `1` the run ended any other way (aborted, stopped, law
fault; the run folder is written); `2` the case was refused by the runner (unreadable, a key
unknown, a value out of range; nothing is written); `3` the engine refused the setup, the law
could not be resolved or built, or the identification's coherence gate held the run (the run
folder is written with that outcome).

## Units, and Where They Are Converted

The benchmark structures give every output in m/s^2 and every drive in volts. The engine works
in acceleration in g (its profiles and its tracked values are "g peak"). The runner converts
in one module (`units` in `main.rs`, `G0 = 9.80665` m/s^2 per g) and only there:

- in: every reference amplitude and limit bound of the case (m/s^2 peak) is divided by `G0`
  when the engine's profiles are built; every sample the simulator returns (m/s^2) is divided
  by `G0` at the station's edge (`read_input` of the runner's station tap), so the engine reads
  g as it would from an accelerometer channel;
- out: every response, reference, limit value and identification value the run file holds is
  multiplied by `G0`, so the run file is in m/s^2 (responses) and m/s^2 per volt
  (identification), like the structure.

The simulator's measurement noise is added inside the simulator, before the station's edge, so
`noise.rms_m_s2` is given to it in m/s^2 unchanged. Drives are volts everywhere; nothing is
converted.

## The Case File, `msnb-case/1`

JSON. Unknown keys are refused at every level. Optional keys may be left out or set to `null`.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `schema` | `"msnb-case/1"` | required | |
| `case_id` | string | required | Free text, echoed in the run file |
| `structure.file` | string | required | `structure.json` of the problem, relative to `--root`; no absolute path, no `..` |
| `structure.sha256` | hex string | none | Checked against the file's bytes when given |
| `controls` | list of output names | every output with role `control`, in file order | The control channels, in the order of every per-control list |
| `fs_hz` | number | `8192` | The rate asked of the simulated station, Hz (the Open Benchmark Set's grid rate) |
| `tick_s` | number | `0.01` | The control update, s (the engine's default); the engine rounds it to whole samples |
| `law.id` | string | required | A built-in law: `matrix`, `paired`, `ratio`, `steepest_descent`, `secant` |
| `law.params` | object | `{}` | Values for the law's declared parameters by key; the rest take the law's defaults. Checked as the control service checks them |
| `test` | object | required | One of the three forms below |
| `reference.file` | string | one of `file`, `inline` | A `msnb-reference/1` file, relative to `--root` |
| `reference.sha256` | hex string | none | Checked when given |
| `reference.inline` | object | | The body of a reference file, in the case itself |
| `reference.phase` | `"specified"`, `"amplitude_only"`, `"auto"` | `"auto"` | Whether the reference's phases are part of the test. `auto`: amplitude only when the law's manifest accepts an amplitude-only reference (as the law check does), specified otherwise |
| `full_level_scale` | number > 0 | `1` | Multiplies every reference amplitude (the full level) |
| `ladder.levels_db` | list of numbers, ascending, each -60 to 12 | `[0]` | The level ladder, dB against full level; the last is the judged level |
| `ladder.rung` | `{"seconds": s}` or `{"sweeps": n}` | `{"seconds": 2}` | How long each level but the last is held; the last is held to the end of the test |
| `drives.ceiling_vpk` | number or list (one per drive) | `9` | Each drive's ceiling, V peak (engine: above 0, at most 10) |
| `drives.abort_vpk` | number or list | `10` | Each drive's abort, V peak (above the ceiling, at most 10) |
| `noise.rms_m_s2` | number >= 0 | `0` | Measurement noise on every simulated output, m/s^2 rms per sample |
| `noise.seed` | integer >= 0 | `1` | The seed of the simulator's noise |
| `identification_seed` | integer >= 0 | `1592590339` (`0x5EED0003`) | The engine's identification seed; the default is the control service's own |
| `limits` | list | `[]` | Limit channels, at most 8, see below |
| `protective` | `"default"` or `"widened"` | `"default"` | The protective settings, see below |
| `max_test_s` | number > 0 | the test's own length times 1.5 plus 60 | The runner's own cap on test time; a run still going is stopped (`runner_stopped`) |

### `test`

| `type` | Keys | Engine plan |
|---|---|---|
| `"sweep"` | `band_hz` `[lo, hi]`; `rate_oct_per_min` > 0; `direction` `"up"` (default) or `"down"` | One logarithmic one-way sweep (`BidirSweepConfig`, `Sweeps(1)`): up starts at `lo`, down at `hi` |
| `"stepped"` | `band_hz` `[lo, hi]`; `steps_per_octave` > 0; `step_s` > 0 | A stepped sine on a log grid (`StepPlan::Grid`, `StepSpacing::Log`), each step held `step_s` after its settle window |
| `"dwell"` | `freq_hz` > 0; `dwell_s` > 0 | A dwell (`DwellPlan`, `DwellFor::Seconds`) |

A down-only sweep is the engine's own: the scheduler starts at the top of the band and sweeps
down once. A reversal inside one run is not available.

### `limits[k]`

| Key | Type | Meaning |
|---|---|---|
| `output` | string | The output's name in `structure.json` (not a control) |
| `bound.frequency_hz` | list, ascending, at least 2 | The bound's grid, Hz; it must cover the test band |
| `bound.amplitude_m_s2_pk` | list, same length, each > 0 | The bound, m/s^2 peak, interpolated in log-log between points (the engine's profile form); not scaled by `full_level_scale` |

### `protective`

| Setting | `default` (what a user's run gets) | `widened` |
|---|---|---|
| Control alarm band | +/-1.5 dB | +/-1.5 dB |
| Control abort band | +/-3 dB | +/-20 dB |
| Updates outside the abort band before the abort (`abort_ticks`) | 3 | 100 |
| Limit channel alarm and abort, above the bound | 1.5 dB, 3 dB | 1.5 dB, 20 dB |
| Seconds a change cap may bind before the divergence stop (`divergence_s`) | 1 | 60 |
| Drive abort | the case's `drives.abort_vpk` | 10 V (the engine's largest) |
| Change cap (`max_step_db`), rise cap (`max_rise_db_per_s`), rotation cap, law budget and misses, notch settings | engine defaults (1 dB, 240 dB/s, none, a quarter of the update, 3) | unchanged |

The caps that shape the drive are not widened: they are part of what the law's answer becomes.
Only the checks that end a run are. The run file states every value in force.

### What the runner sets that a user would set on the screen

- The identification runs at the engine's defaults (`IdentificationSettings::default()`:
  -20 dB, 0.25 Hz lines, `max(24, 6 x drives)` records, trust and gate coherence 0.5) with the
  review answered "proceed" at once.
- The tracking filter is the engine's default (10 percent of the frequency, at most 100 Hz).
- Law requests to hold or slow the sweep are not honoured (the engine's default); they are
  recorded.
- The channel table holds the controls (role control), the limit channels (role monitor) and
  the drives, nothing else. The simulator is given only those outputs of the structure; the
  modes, their level rules and their drive shapes are the structure's own, so the outputs
  given behave exactly as in the whole structure.
- Every law call is held to `hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET` (1 s) in place of
  the setup's budget (a quarter of the update), as the law check's deterministic harness does,
  so a busy machine cannot refuse a late answer and change the run. The call times are still
  measured against the setup's budget and reported in `timing.json`. This is the one
  departure from a user's simulator run, made so the same case gives the same bytes.

### The Reference File, `msnb-reference/1`

| Key | Type | Meaning |
|---|---|---|
| `schema` | `"msnb-reference/1"` | |
| `controls` | list of output names | Must equal the case's controls, in order |
| `frequency_hz` | list, ascending, at least 2 | The grid, Hz; it must cover the test band |
| `amplitude_m_s2_pk` | list of lists, `(lines, controls)` | Each control's reference at full level, m/s^2 peak, each > 0 |
| `phase_deg` | list of lists, `(lines, controls)` | Each control's phase, degrees, relative to the tone, in (-180, 180]; required, ignored when the run is amplitude only |

How it becomes the engine's own form (`ControlChannel`): each control's amplitude becomes a
profile of acceleration rows, one row per grid point (log-log between rows, held flat beyond
the ends only where the engine holds it); each phase becomes phase points, interpolated by the
engine linearly in phase against log frequency without unwrapping. Where two neighbouring
phases differ by more than 180 degrees (a wrap), the runner splits the segment where the
unwrapped phase crosses +/-180 degrees into two points 1e-9 of the frequency apart, so the
engine's interpolation follows the short way everywhere except inside that gap. The run file
records how many splits were made.

## The Run Folder, `msnb-run/1`

| File | Deterministic | What |
|---|---|---|
| `run.json` | yes: the same case, root, options and runner give the same bytes | The run file, below |
| `timing.json` | no (clock times) | Clock time of each phase, law call times |
| `record/` | yes (with `--record` only) | The engaged run's raw samples, for the player check |

### `run.json`

Numbers are rounded to 9 significant digits. Complex values are given as separate real and
imaginary arrays. Every per-point array is indexed by point; per-control arrays are
`(points, controls)`, per-drive `(points, drives)`, per-limit `(points, limits)`.

| Key | What |
|---|---|
| `schema` | `"msnb-run/1"` |
| `case_id` | From the case |
| `case_sha256` | SHA-256 of the case file's bytes |
| `runner_sha256` | SHA-256 of the running program's file |
| `code_commit` | The commit the program was built from (`HF_BUILD_COMMIT` at build time), or `"unstated build"` |
| `options` | `{"ideal_frf": bool, "record": bool}` |
| `structure` | `file` (as in the case), `sha256`, `structure_id`, `variant`, `controls`, `limits` and `drives` (names), `n_modes`, `fs_hz` (as configured), `tick_s` (as rounded), `tick_frames` |
| `law` | `id`, `name`, `params` (as given), `amplitude_only` (the manifest's), `phase_specified` (as run) |
| `test` | The test as the engine was given it: `type`, and `band_hz`, `rate_oct_per_min`, `direction`, `start_hz` (sweep); `band_hz`, `steps_per_octave`, `step_s` (stepped); `freq_hz`, `dwell_s` (dwell) |
| `reference` | `source` (`file` or `inline`), `sha256` (of the file, or null), `lines`, `full_level_scale`, `phase_splits` |
| `settings` | Every protective and drive value in force (`protective`, `alarm_db`, `abort_db`, `abort_ticks`, `limit_alarm_db`, `limit_abort_db`, `divergence_s`, `max_step_db`, `max_rise_db_per_s`, `rotation_cap_deg`, `law_budget_fraction`, `law_misses`, `ceiling_vpk`, `abort_vpk`, `ladder_db`, `rung`, `noise_rms_m_s2`, `noise_seed`, `identification_seed`, `max_test_s`) |
| `identification` | Below |
| `outcome` | Below |
| `events` | The engine's timeline, below |
| `history` | The judged updates, decimated, below |
| `true_modes` | The simulator's mode frequencies along the run, below |
| `summary` | `updates` (records received), `records_dropped` (the engine's count; 0 expected), `judged_updates`, `largest_drive_vpk`, `over_ceiling_samples`, `drive_is_zero` (at the end), `engaged_at_s` |

#### `identification`

| Key | What |
|---|---|
| `source` | `"engine"` (the engine's own identification), or `"ideal_frf"` (the law was handed the small-signal response; the engine's own was still measured and is what its guards use) |
| `level_db` | The level it was taken at, dB against full level, by response (the engine's reading) |
| `lag_frames`, `frames_ahead` | The drive-to-response lag taken out, and the frames queued ahead of the first drive frame, samples |
| `record_len`, `realizations`, `line_spacing_hz`, `duration_s` | The measurement as made |
| `band_coherence` | Each control's mean coherence over the test band |
| `gate_passed` | Whether every control reached the gate |
| `resonances` | The resonances the engine's modal fit finds in the identification over the test band (for a dwell, the dwell frequency /1.25 to x1.25): `f_hz` and `q` each (the sweep-rate warning's own fit; for a sweep, `time_constants` and `flagged` too) |
| `ideal_vs_measured` | With `--ideal-frf`: the median and largest difference between the measured identification and the response handed to the law over trusted lines in the test band, dB and degrees |

#### `outcome`

| Key | What |
|---|---|
| `class` | `completed`; `aborted` (a guard ended the run with `widened` protective settings); `stopped_by_protective_default` (a guard ended the run with `default` protective settings); `law_fault`; `station_fault`; `runner_stopped` (the runner's `max_test_s`); `refused` (the engine refused the setup or the law); `held_by_identification` (the coherence gate held the run) |
| `engine_outcome` | The engine's own name (`completed`, `control_abort`, `limit_abort`, `drive_abort`, `kinematic`, `law_fault`, `divergence`, `frequency_out_of_range`, `run_thread_failure`, `station`, `stopped`), or null |
| `guard` | For a guard's end: the guard (`control_abort`, `limit_abort`, `drive_abort`, `divergence`, `kinematic`, `frequency_out_of_range`); else null |
| `channel` | The control, limit or drive it named, by output or drive name; else null |
| `frequency_hz`, `time_s`, `level_db` | Where the stop began (the engine's `stopping` event); null for a completed run |
| `message` | The engine's or the runner's words for a refusal, a hold or a fault; else null |

#### `events`

The engine's timeline in order, each `{"time_s", "index", "frequency_hz", "level_db",
"kind", "detail"}`. `kind` is the engine's name (`engaged`, `level`, `notch_in`, `notch_out`,
`notched_reference_refused`, `alarm`, `alarm_cleared`, `law_miss`, `over_ceiling`, `stopping`,
`ended`, `held`, `resumed`); `detail` holds the event's own fields (the control, error, depth,
scale, reason or outcome), channels by name. `alarm` and `alarm_cleared` are kept for the
first 200 of each and then counted only (`events_dropped` gives the counts by kind).

#### `history`

| Key | What |
|---|---|
| `decimation` | `"1/48 octave"` for a sweep, `"every update"` for a stepped sine or a dwell |
| `points` | How many points |
| `index` | The update's index (0 at engage) |
| `time_s` | Seconds since engage at the end of the update |
| `frequency_hz` | The frequency the controls were measured at |
| `level_db` | The scheduled level |
| `judged` | Whether the update is judged: outside a settle window, the law's answer used, at the last level of the ladder (the law check's rule) |
| `settling` | A settle window was open |
| `miss` | null, or why the law's answer was not used |
| `control_re`, `control_im` | Each control's tracked response, m/s^2 peak |
| `reference_re`, `reference_im` | Each control's reference at this frequency and level, m/s^2 peak |
| `judged_reference_re`, `judged_reference_im` | The reference the controls were judged against (a notched one where the engine accepted it); present only when the case has limit channels |
| `error_db` | Each control's error, tracked over judged reference, dB (the engine's) |
| `drive_re`, `drive_im` | Each drive played from this update on, after every guard, V peak |
| `limit_re`, `limit_im`, `limit_bound` | Each limit channel's tracked response and its bound here, m/s^2 peak; present only with limit channels |
| `notch_depth`, `ceiling_scale`, `cap_bound`, `opposition_ratio` | The engine's readings of the update |
| `bin_updates` | Sweep only: how many updates the point stands for (its 1/48-octave bin at its level) |
| `bin_judged` | Sweep only: how many of those are judged |
| `bin_error_db_min`, `bin_error_db_max` | Sweep only: each control's smallest and largest error over the judged updates of the bin (`(points, controls)`; null where none is judged) |

In a sweep a point is the first update of each 1/48-octave bin (bins counted from the band's
lower edge, so the same frequency falls in the same bin up or down) at each level, and the last
update of the run is always a point. The bin columns let a scorer find the worst excursion
anywhere in the sweep without the full history.

#### `true_modes`

What the simulator's level rule says each mode's frequency is at the moment of each history
point: the structure's state as tested.

| Key | What |
|---|---|
| `tracked` | Whether the modes were followed (false for a structure with no level rule: its modes do not move) |
| `modes` | The modes followed: `index` (in `structure.json`), `f0_hz` (small-signal) |
| `frequency_hz` | `(points, modes)`: each mode's frequency at the point |
| `resolution_samples` | The envelope is read at the end of each block the station is read in (256 samples at the simulator's block); a point takes the last read at or before its time |

The modes followed are those whose small-signal frequency lies in the identification's band
(the test band /1.25 to x1.25). The runner steps a copy of those modes alone on the exact
drive the simulator played (read from a second, drive-only simulator fed the same calls), so
the frequencies are the simulator's own rule applied to the simulator's own state.

### `timing.json`

`identify_s`, `engage_s` (the law's build and the core), `run_s` (the engaged loop), `write_s`,
`total_s` (clock seconds, wall); `law_calls`, `law_median_s`, `law_slowest_s`,
`law_over_setup_budget` (calls over a quarter of the update), `setup_law_budget_s`; and
`law_s` per history point (null where the law was not called).

### Recording (`--record`)

`record/drive.f8`: the drive the simulator played to the structure in the engaged run, every
sample, `(samples, drives)` little-endian float64 row-major, read from the drive-only
simulator. `record/inputs.f8`: every sample the station returned in the engaged run, before
the conversion to g, `(samples, controls + limits)` float64 (each the float32 the station
gave). `record/structure.json`: the structure the simulator ran, in the player's keys (case
schema `mm-x4-case/2`: the controls and limit channels as its outputs, the structure's step).
`obs_play record/structure.json --drive record/drive.f8 --out <f8> --fs <fs_hz>` gives the
same samples with no noise.

## Identification

By default the engine's own (what a user gets). With `--ideal-frf` the law's build is handed,
on the identification's own grid, the structure's small-signal frequency response at the
controls and limit channels (`JointedPlant::frf_at_envelope` with every envelope zero, at the
station's rate, through the structure's step and delays), turned by the station's delay the
same way the identification is (`exp(j w (lag_frames - frames_ahead) / fs)`), converted to g
per volt, with every line trusted and coherence 1. The engine's guards still use its measured
identification; only the law sees the ideal one.
