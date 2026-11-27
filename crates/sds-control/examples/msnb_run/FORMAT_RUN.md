# MIMO Sine Benchmark Runner: Case and Run Formats

`msnb_run` runs one MIMO Sine case of the Open Benchmark Set through the shipped MIMO Sine engine
(`hertzforge-mimo-sine`: its identification, loop-speed rule, guards, level ladder and records)
on the simulator, and writes one run folder. This file describes the case file it reads
(`msnb-case/1`), the reference file the case names (`msnb-reference/1`) and the run folder it
writes (`msnb-run/1`). The machine-readable form of the case is `msnb-case-1.schema.json`
beside this file.

The run format is meant to be written by more than one producer: this program (the Hertzforge
engine) and a closed-loop harness in Python. A key is either **common** (every producer writes
it; the scorer may rely on it) or **engine** (this program writes it; another producer may
leave it out or write `null`, and a scorer must not need it). Every table below marks which.
Where a common key depends on how the loop is built (what an update is, which updates are
judged, how the history is thinned), the rule is stated here exactly so another producer can
follow it.

```text
cargo build --release -p sds-control --example msnb_run
msnb_run <case.json> --root <problem-root> --out <run-dir> [--ideal-frf] [--record] [--no-true-modes] [--whole-history]
```

- `<problem-root>`: the folder every path in the case is relative to (for the benchmark, the
  folder holding the problem folders). The run file never holds an absolute path.
- `<run-dir>`: made by the program; it must not exist yet or must be empty (a run folder is
  never overwritten).
- `--ideal-frf`: hand the law the structure's small-signal frequency response in place of the
  engine's identification (see "Identification" below). An option for the suite to separate
  the identification's error from the law's; not the default.
- `--record`: also write the raw samples of the engaged run (see "Recording"). For checks, not
  for the study: a 400 s run at 8192 Hz is about 26 MB a channel.
- `--no-true-modes`: do not follow the modes' frequencies (`true_modes.tracked` is false). On
  a 6x6 realistic structure, 1 octave a minute over 20 to 2000 Hz, following them took 24 % of
  the run's processor time.
- `--whole-history`: a stepped sine or a dwell keeps every update (see "Thinning" under
  `history`); a sweep is thinned as always. For a diagnosis, not for the study.

One run uses one core: the program starts no thread of its own and does no network or screen
work.

Exit status: `0` the run completed; `1` the run ended any other way (aborted, stopped, law
fault; the run folder is written); `2` the case was refused by the runner (unreadable, a key
unknown, a value out of range, a grid that does not cover the test; nothing is written); `3`
the engine refused the setup (for instance more than 16 controls or 8 drives), the law could not
be resolved or built, or the identification's coherence gate held the run (the run folder is
written with that outcome).

## Units, and Where They Are Converted

The benchmark structures give every output in m/s^2 and every drive in volts. The engine works
in acceleration in g (its profiles and its tracked values are "g peak"). The runner converts
in one module (`units` in `main.rs`, `G0 = 9.80665` m/s^2 per g) and only there:

- in: every reference amplitude and limit bound of the case (m/s^2 peak) is divided by `G0`
  when the engine's profiles are built; every sample the simulator returns (m/s^2) is divided
  by `G0` at the station's edge (`read_input` of the runner's station tap), so the engine reads
  g as it would from an accelerometer channel;
- out: every response, reference, limit value and identification value the run folder holds
  is multiplied by `G0`, so it is in m/s^2 (responses) and m/s^2 per volt (identification),
  like the structure.

The simulator's measurement noise is added inside the simulator, before the station's edge, so
`noise.rms_m_s2` is given to it in m/s^2 unchanged. Drives are volts everywhere; nothing is
converted.

## The Case File, `msnb-case/1`

JSON. Unknown keys are refused at every level. Optional keys may be left out or set to `null`.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `schema` | `"msnb-case/1"` | required | |
| `case_id` | string | required | Free text, echoed in the run file |
| `structure.file` | string | required | `structure.json` of the problem, relative to `--root`; no absolute path, no `..`. Kind `jointed` (the realistic and strong tiers, with their level rule) or `modal` (the linear tier, run as the Random study's runner ran it: the fitted discretization of its `simulator` block) |
| `structure.sha256` | hex string | none | Checked against the file's bytes when given |
| `controls` | list of output names | every output with role `control`, in file order | The control channels, in the order of every per-control list |
| `fs_hz` | number | `8192` | The rate asked of the simulated station, Hz (the Open Benchmark Set's grid rate) |
| `tick_s` | number | `0.01` | The control update, s (the engine's default); the engine rounds it to whole samples |
| `law.id` | string | required | A built-in law: `matrix`, `paired`, `ratio`, `steepest_descent`, `secant` |
| `law.params` | object | `{}` | Values for the law's declared parameters by key; the rest take the law's defaults. A key the law does not declare, or a value not of its kind, is refused |
| `test` | object | required | One of the three forms below |
| `reference.file` | string | one of `file`, `inline` | A `msnb-reference/1` file, relative to `--root` |
| `reference.sha256` | hex string | none | Checked when given |
| `reference.inline` | object | | The body of a reference file, in the case itself |
| `reference.phase` | `"specified"`, `"amplitude_only"`, `"auto"` | `"auto"` | Whether the reference's phases are part of the test. `auto`: amplitude only when the law's manifest says it takes an amplitude-only reference, specified otherwise |
| `full_level_scale` | number > 0 | `1` | Multiplies every reference amplitude (the full level) |
| `ladder.levels_db` | list of numbers, ascending, each -60 to 12 | `[0]` (the control service's default: one level, full) | The level ladder, dB against full level; the last is the judged level |
| `ladder.rung` | `{"seconds": s}` or `{"sweeps": n}` | the control service's: `{"sweeps": 1}` for a sweep, `{"seconds": 10}` otherwise | How long each level but the last is held; the last is held to the end of the test |
| `drives.ceiling_vpk` | number or list (one per drive) | `9` | Each drive's ceiling, V peak (engine: above 0, at most 10) |
| `drives.abort_vpk` | number or list | `10` | Each drive's abort, V peak (above the ceiling, at most 10) |
| `drives.gain` | number > 0 | `1` | The amplifier's gain: the structure's input in volts per engine volt. The runner multiplies every drive sample by it at the station's edge (`Edge::write_drive`, beside the units conversion); the engine, the law, the ceilings, the aborts and every drive in the run file are in engine volts, and the identification the law is built on is per engine volt (with `--ideal-frf`, the small-signal response times the gain). The run file states it as `settings.drive_gain`. The MIMO Sine suite sets it so each problem's ideal full-level drive peak is 1 V (its `FORMAT_SINE.md` section 7) |
| `noise.rms_m_s2` | number >= 0 | `0` | Measurement noise on every simulated output the run reads, m/s^2 rms per sample |
| `noise.seed` | integer >= 0 | `1` | The seed of the simulator's noise |
| `identification_seed` | integer >= 0 | `1592590339` (`0x5EED0003`) | The engine's identification seed; the default is the control service's own |
| `limits` | list | `[]` | Limit channels, at most 8, see below |
| `protective` | `"default"` or `"widened"` | `"default"` | The protective settings, see below |
| `max_test_s` | number > 0 | 1.5 times (the test's own length plus every lower level's time) plus 60 | The runner's own cap on test time; a run still going is stopped (`runner_stopped`) |

### `test`

| `type` | Keys | Engine plan |
|---|---|---|
| `"sweep"` | `band_hz` `[lo, hi]`; `rate_oct_per_min` > 0; `direction` `"up"` (default) or `"down"` | One logarithmic one-way sweep (`BidirSweepConfig`, `Sweeps(1)`): up starts at `lo`, down at `hi` |
| `"stepped"` | `band_hz` `[lo, hi]`; `steps_per_octave` > 0; `step_s` > 0 | A stepped sine on a log grid (`StepPlan::Grid`, `StepSpacing::Log`), each step held `step_s` after its settle window |
| `"dwell"` | `freq_hz` > 0; `dwell_s` > 0 | A dwell (`DwellPlan`, `DwellFor::Seconds`) |

A down-only sweep is the engine's own: the scheduler starts at the top of the band and sweeps
down once (`check_msnb_run.py`, check 6). A reversal inside one run is not available.

### `limits[k]`

| Key | Type | Meaning |
|---|---|---|
| `output` | string | The output's name in `structure.json` (not a control) |
| `bound.frequency_hz` | list, ascending, at least 2 | The bound's grid, Hz; it must cover the frequencies the test visits |
| `bound.amplitude_m_s2_pk` | list, same length, each > 0 | The bound, m/s^2 peak, log-log between points (the engine's profile form); not scaled by `full_level_scale` |

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
- The tracking filter is the engine's default (noise bandwidth 10 percent of the frequency, at
  most 100 Hz).
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
| `frequency_hz` | list, ascending, at least 2 | The grid, Hz; it must cover the frequencies the test visits (the runner refuses it otherwise: the engine's profile gives no value outside its grid) |
| `amplitude_m_s2_pk` | list of lists, `(lines, controls)` | Each control's reference at full level, m/s^2 peak, each > 0 |
| `phase_deg` | list of lists, `(lines, controls)` | Each control's phase, degrees, relative to the tone, in (-180, 180]; required, ignored when the run is amplitude only |

How it becomes the engine's own form (`ControlChannel`): each control's amplitude becomes a
profile of acceleration rows, one row per grid point, read log-log between rows
(`SineProfile::target_accel_g_pk`); each phase becomes phase points, read by the engine linearly
in phase against log frequency without unwrapping, and held at the first or last point outside
the grid. Where two neighbouring phases differ by more than 180 degrees (a wrap), the runner
splits the segment where the short way crosses +/-180 degrees (at the log-frequency fraction
where the unwrapped phase reaches it) into two points a relative 1e-9 apart, 180 on one side
and -180 + 1e-9 on the other, so the engine's interpolation follows the short way everywhere
except inside that gap. The run file records how many splits were made. Another producer
should interpolate the same way: log-log in amplitude, linear in unwrapped phase against log
frequency along the short way.

## The Run Folder, `msnb-run/1`

| File | Deterministic | Kind | What |
|---|---|---|---|
| `run.json` | yes: the same case, root, options and program give the same bytes | common | The run file, below |
| `law_identification.f8` | yes | common | The frequency response the law was built on, below |
| `timing.json` | no (clock times) | engine | Clock time of each phase, law call times |
| `record/` | yes (with `--record` only) | engine | The engaged run's raw samples, for the player check |

### `run.json`

Compact JSON. Numbers are rounded to 9 significant digits; a number that is not finite is
`null`. Complex values are given as separate real and imaginary arrays. Every per-point array is
indexed by point; per-control arrays are `(points, controls)`, per-drive `(points, drives)`,
per-limit `(points, limits)`.

| Key | Kind | What |
|---|---|---|
| `schema` | common | `"msnb-run/1"` |
| `producer` | common | `{"program", "engine", "simulator"}`: who wrote the file (here `msnb_run`, `hertzforge-mimo-sine`, `hertzforge-hal-sim`) |
| `case_id` | common | From the case |
| `case_sha256` | common | SHA-256 of the case file's bytes |
| `runner_sha256` | engine | SHA-256 of the running program's file |
| `code_commit` | engine | The commit the program was built from (`HF_BUILD_COMMIT` at build time), or `"unstated build"` |
| `options` | common | `{"ideal_frf", "record", "true_modes", "whole_history"}`, booleans |
| `structure` | common | `file` (as in the case), `sha256`, `structure_id`, `variant`, `seed`, `controls`, `limits` and `drives` (names), `n_modes`, `fs_hz` (as configured), `tick_s` (as rounded to whole samples), `tick_frames` |
| `law` | common: `id`, `params`, `phase_specified`; engine: `name`, `version`, `amplitude_only` | `params` as given; `amplitude_only` the manifest's; `phase_specified` as run |
| `test` | common | The test as run: `type`, and `band_hz`, `rate_oct_per_min`, `direction`, `start_hz`, `sweeps` (sweep); `band_hz`, `steps_per_octave`, `spacing`, `step_s` (stepped); `freq_hz`, `dwell_s` (dwell) |
| `reference` | common | `source` (`file` or `inline`), `file`, `sha256` (of the file, or null), `lines`, `full_level_scale`, `phase_splits` |
| `settings` | common: `protective`, `alarm_db`, `abort_db`, `abort_ticks`, `limit_alarm_db`, `limit_abort_db`, `ceiling_vpk`, `abort_vpk`, `drive_gain`, `ladder_db`, `rung`, `noise_rms_m_s2`, `noise_seed`, `max_test_s`; engine: the rest | Every protective and drive value in force; engine-only: `divergence_s`, `max_step_db`, `max_rise_db_per_s`, `rotation_cap_deg`, `law_budget_fraction`, `law_misses`, `law_call_deadline_s`, `identification_seed`, `tracking`, `identification` |
| `identification` | below | |
| `outcome` | below | |
| `events` | engine | The engine's timeline, below |
| `events_dropped` | engine | Counts by kind of events not kept |
| `history` | below | The updates, thinned (below) |
| `true_modes` | below | The simulator's mode frequencies along the run |
| `summary` | common: `updates`, `judged_updates`, `largest_drive_vpk`; engine: `records_dropped` (0 expected), `largest_sample_v`, `over_ceiling_samples`, `drive_is_zero` (at the end) | |
| `record` | engine | With `--record`: `drive_frames`, `input_frames`, `channels`, `drives` |

#### `identification`

| Key | Kind | What |
|---|---|---|
| `source` | common | `"engine"` (the law was built on the engine's own identification), or `"ideal_frf"` (on the small-signal response; the engine's own was still measured and is what its guards use) |
| `law_given` | common | The identification the law was built on, in `law_identification.f8`: `file`, `sha256`, `lines`, `controls`, `limits`, `drives`, `builds` (how many times the law was built; the file holds the first), `columns` (the file's layout, below) |
| `level_db` | engine | The level the identification was taken at, dB against full level, by response (the engine's reading) |
| `lag_frames`, `frames_ahead` | engine | The drive-to-response lag taken out, and the frames queued ahead of the first drive frame, samples |
| `record_len`, `realizations`, `line_spacing_hz`, `duration_s` | engine | The measurement as made |
| `band_coherence` | engine | Each control's mean coherence over the test band |
| `gate_passed` | engine | Whether every control reached the gate |
| `resonances` | engine | The resonances the engine's modal fit finds in the identification over the test band (for a dwell, the dwell frequency /1.25 to x1.25): `f_hz` and `q` each (the sweep-rate warning's own fit; for a sweep, `time_constants` and `flagged` too) |
| `ideal_vs_measured` | engine | The measured identification against the small-signal response turned as below (on every run, with or without `--ideal-frf`): `elements` compared (trusted lines, elements at least 1/1000 of their line's largest), `median_db`, `largest_db`, `median_deg`, `largest_deg` of the measured over the small-signal; `fitted_delay_samples` and `fitted_angle_deg`, the straight line that best fits the angle left against frequency (both near 0 when the turn is right); `delay_turn_samples`, the turn used |

#### `law_identification.f8`

Little-endian float64, one row a line of the identification's grid (ascending), the columns in
the order `identification.law_given.columns` lists them: `frequency_hz` (1); `response_re`,
`response_im` (controls x drives each, control-major: control 0's drives, then control 1's);
`limit_response_re`, `limit_response_im` (limits x drives each); `coherence` (one per control);
`limit_coherence` (one per limit); `trusted` (1 or 0). Responses are m/s^2 peak per volt peak,
in the engine's convention: drive `D` plays `Re{D exp(j phi(t))}`, a channel's complex value
is its response's phase minus the tone's with the identification's lag taken out, so a linear
structure in steady state gives `C = H D` with the `H` of this file and the `C` and `D` of the
history. With `--ideal-frf` the file holds the small-signal response the law was handed (every
line trusted, coherence 1). 9937 lines for the benchmark's 20 to 2000 Hz band (16 to 2500 Hz
at 0.25 Hz): 6.4 MB for 6x6, 12.6 MB for 12x6.

#### `outcome` (common)

| Key | What |
|---|---|
| `class` | `completed`; `aborted` (a guard ended the run with `widened` protective settings); `stopped_by_protective_default` (a guard ended the run with `default` protective settings); `law_fault`; `station_fault`; `runner_stopped` (the runner's `max_test_s`); `refused` (the engine refused the setup or the law); `held_by_identification` (the coherence gate held the run) |
| `engine_outcome` | Engine: the engine's own name (`completed`, `control_abort`, `limit_abort`, `drive_abort`, `kinematic`, `law_fault`, `divergence`, `frequency_out_of_range`, `run_thread_failure`, `station`, `stopped`), or null |
| `guard` | For a guard's end: `control_abort`, `limit_abort`, `drive_abort`, `divergence`, `kinematic`, `frequency_out_of_range`; else null |
| `channel` | The control, limit or drive it named, by output or drive name; else null |
| `frequency_hz`, `time_s`, `level_db` | Where the stop began (the engine's `stopping` event); null for a completed run |
| `message` | The engine's or the runner's words for a refusal, a hold or a fault; else null |

#### `events` (engine)

The engine's timeline in order, each `{"time_s", "index", "frequency_hz", "level_db",
"kind", "detail"}`. `kind` is the engine's name (`engaged`, `level`, `notch_in`, `notch_out`,
`notched_reference_refused`, `alarm`, `alarm_cleared`, `law_miss`, `over_ceiling`, `stopping`,
`ended`, `held`, `resumed`); `detail` holds the event's own fields (the control, error, depth,
scale, reason or outcome), channels by name. `alarm` and `alarm_cleared` are kept for the
first 200 of each and then counted only (`events_dropped`).

#### `history`

**What an update is.** The loop runs once every `structure.tick_s` (`tick_frames` samples) from
engage. Update `k` (`index`, 0 at engage) does, in this order: (1) reads every channel's tracked
value at the end of its interval (`time_s`, the engine's own seconds since engage at the end of
the update); (2) forms the reference at the
update's frequency and level; (3) judges the controls and limit channels against their abort
bands and the drives against their aborts; if a guard ends the run here, the update is recorded
with the law not called and the run stops; (4) schedules the next interval (frequency, level,
settle window); (5) calls the law with this update's readings; its answer, after the engine's
caps and ceilings, is `drive`, played from the next interval on. So `control` at update `k`
answers the drives of earlier updates, through the structure and its delay.

**Tracked values.** `control` and `limit` are each channel's complex amplitude at the tone,
peak, in the convention of `law_identification.f8`. Here they are the engine's tone trackers:
synchronous demodulation against the tone's own phase, a cascade of 4 one-pole low-pass
sections whose noise bandwidth is the tracking width (10 % of the frequency, at most 100 Hz),
read at the end of the update. Another producer states its own tracker; a scorer compares
values, not filters.

**Settle windows.** At engage, after a level change and after a step (stepped sine), the engine
opens a settle window of 2 tracker settle times (1.3 over the tracking width each) plus 3 times
the structure's group delay at that frequency (from the identification, over twice the tracking
width) plus the identification's lag. `settling` is true inside it.

**Judged.** An update is judged when it is outside a settle window (`settling` false), the law's
answer was not refused (`miss` null), and it is at the last level of the ladder
(|`level_db` - last level| < 1e-6 dB): the law check's rule
(`hertzforge_mimo_sine_laws::check`). The update on which a guard ended the run is judged by the
same rule (its readings are real; the law was not called on it).

**Thinning.** A stepped sine or a dwell (`decimation` `"every judged update"`) keeps every judged
update as a point standing for itself. Of the updates that are not judged it keeps the first and
the last of each run of consecutive updates with the same key (the level rounded to 1e-6 dB, the
frequency, `settling`, `miss`); a point stands for the updates from it up to the next point.
Every update a scorer selects is judged, so it reads a stepped test's judged updates whole; the
other updates are read through their counts (`bin_updates`) and the drive and mode extremes of
each point's span (`bin_drive_vpk_max`, `bin_drive_norm_max`, `true_modes.frequency_hz_min` and
`_max`), each null where the point stands for one update (its own values are its extremes). With `--whole-history`
a stepped sine or a dwell keeps every update (`decimation` `"every update"`, no `bin_` fields, as
before the thinning; for a diagnosis). Measured on the pilot's 27 stepped-sine and 45 dwell cases:
the thinned files are 33.5 % of the whole ones' bytes on a stepped sine and 47.1 % on a
dwell, and every reading of the suite's scorer is the same. A
sweep (`decimation` `"1/48 octave"`) keeps one point per bin and level: with `lo` the band's
lower edge, an update's bin is `floor(48 log2(f / lo) + 1e-9)`; its key is (its level rounded to
1e-6 dB, its bin); a point is the first update of each run of consecutive updates with the same
key; and the last update of the run is a point too if it is not one already. A point stands for
the updates from it up to the next point (the last update stands for itself alone). Bins are
counted from the lower edge whichever way the sweep goes, so a frequency falls in the same bin
up or down.

| Key | Kind | What |
|---|---|---|
| `decimation` | common | `"1/48 octave"` (sweep), `"every judged update"` (stepped sine, dwell), or `"every update"` (stepped sine, dwell, with `--whole-history`) |
| `points` | common | How many points |
| `index` | common | The update's index (0 at engage) |
| `time_s` | common | Seconds since engage at the end of the update |
| `frequency_hz` | common | The frequency the channels were tracked at |
| `level_db` | common | The scheduled level, dB against full level |
| `judged` | common | As above |
| `settling` | common | A settle window was open |
| `miss` | engine | null, or why the law's answer was refused |
| `control_re`, `control_im` | common | Each control's tracked value, m/s^2 peak |
| `reference_re`, `reference_im` | common | Each control's reference at this frequency and level, m/s^2 peak |
| `judged_reference_re`, `judged_reference_im` | engine | The reference the controls were judged against (a notched one where the engine accepted it); present only when the case has limit channels |
| `error_db` | common | Each control's error, 20 log10 of the tracked value's size over the judged reference's (the engine's; a reading of zero gives about -6000 dB, the engine's floor of 1e-300 on both) |
| `drive_re`, `drive_im` | common | Each drive played from the next interval on, after every guard, V peak |
| `limit_re`, `limit_im`, `limit_bound` | common, with limit channels only | Each limit channel's tracked value and its bound here, m/s^2 peak |
| `notch_depth`, `ceiling_scale`, `cap_bound`, `opposition_ratio` | engine | The engine's readings of the update |
| `bin_updates` | common, thinned | How many updates the point stands for |
| `bin_judged` | common, thinned | How many of those are judged |
| `bin_error_db_min`, `bin_error_db_max` | common, sweep only | Each control's smallest and largest error over the judged updates the point stands for (`(points, controls)`; null where none is judged) |
| `bin_drive_vpk_max` | common, stepped sine and dwell thinned | Each drive's largest size over the updates the point stands for, V peak (`(points, drives)`), from the drive as written (nine digits); null where the point stands for one update |
| `bin_drive_norm_max` | common, stepped sine and dwell thinned | The drive vector's largest norm over those updates, V peak; null where the point stands for one update |

#### `true_modes` (common; `resolution_samples` and `configures_seen` engine)

What the simulator's level rule says each mode's frequency is at the moment of each history
point: the structure's state as tested.

| Key | What |
|---|---|
| `tracked` | Whether the modes were followed (false for a structure with no level rule, whose modes do not move, and with `--no-true-modes`) |
| `modes` | The modes followed: `index` (in `structure.json`), `f0_hz` (small-signal) |
| `frequency_hz` | `(points, modes)`: each mode's frequency at the point |
| `frequency_hz_min`, `frequency_hz_max` | A stepped sine or a dwell thinned, tracked: `(points, modes)`, each mode's smallest and largest frequency over the updates the point stands for; a null row where the point stands for one update |
| `resolution_samples` | The envelope is read at the end of each block the station is read in (256 samples at the simulator's block); a point takes the last read at or before round(`time_s` x fs) samples from the engaged start |
| `configures_seen` | The station's configures seen (2: the identification's, the engaged run's) |

The modes followed are those whose small-signal frequency lies in the identification's band
(`ident::grid_band` of the test band). The runner steps a copy of those modes alone on the exact
drive the simulator played (read from a second, drive-only simulator fed the same calls), so
the frequencies are the simulator's own rule applied to the simulator's own state. Checked
against the open-loop player's envelopes put through `reference_model.level_state`: a relative
3.5e-9.

### `timing.json` (engine)

`identify_s`, `engage_s` (the law's build and the core), `run_s` (the engaged loop), `write_s`,
`total_s` (clock seconds, wall); `law_calls`, `law_median_s`, `law_slowest_s`,
`law_over_setup_budget` (calls over a quarter of the update), `setup_law_budget_s`, `test_s`;
and `law_s` per history point (null where the law was not called).

### Recording (`--record`, engine)

`record/drive.f8`: the drive the simulator played to the structure in the engaged run, every
sample, `(samples, drives)` little-endian float64 row-major, read from the drive-only
simulator. `record/inputs.f8`: every sample the station returned in the engaged run, before
the conversion to g, `(samples, controls + limits)` float64 (each the float32 the station
gave). `record/structure.json`: the structure the simulator ran, in the research runner's keys
(case schema `mm-x4-case/2` for a jointed structure, the controls and limit channels as its
outputs, with the structure's step; `mm-x4-case/1`'s modal object for the linear tier). For a
jointed structure, `obs_play record/structure.json --drive record/drive.f8 --out <f8> --fs
<fs_hz>` gives the same samples with no noise: equal to `inputs.f8` once rounded to float32
(`check_msnb_run.py`, check 2). The player does not step a modal structure. Sample 0 of both
files is the first sample of the engaged run, so update `k`'s readings end at sample
round(`time_s` x fs).

## Identification

By default the engine's own (what a user gets). With `--ideal-frf` the law's build is handed,
on the identification's own grid, the structure's small-signal frequency response at the
controls and limit channels (the simulator's own exact response of the discrete structure,
`analytic_frf_at`; for a jointed structure every mode at zero envelope; at the station's rate, through
the structure's step and delays), turned by the delay the identification carries,
`exp(j w (lag_frames - frames_ahead - 256) / fs)` (256 is the service's simulator block,
`SIM_BLOCK_FRAMES`; found by fitting, and fitted again on every run as
`ideal_vs_measured.fitted_delay_samples`), converted to g per volt, with every line trusted and
coherence 1. The engine's guards still use its measured identification; only the law sees the
ideal one.
