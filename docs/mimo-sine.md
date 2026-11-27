# MIMO Sine

MIMO Sine plays one sine tone at a time on several shakers at once, each drive with its own
amplitude and phase, and controls the response at several control points, each against its own
reference amplitude and, when you give one, its own reference phase. The test is a sweep, a dwell
or a stepped sine, on a level schedule. Every control update a control law decides one complex
amplitude per drive; the frequency, the sweep, the level schedule, every protection and every
abort are the controller's. Drives are in volts peak throughout, each held under a ceiling and an
abort that you give.

This guide is for a test engineer who has run single-shaker sine. It covers what a MIMO sine test
holds, how to set one up, the five built-in control laws, their limits and how to choose one, limit
channels and the backstop notch, the guards every law is held to, what the readings on the run
screen mean, how to write your own law, and what has been shown on the simulator only. It shares
its channel table, run bar and control-law panel with the other test types, and its
schedule, tracking, hold, resume and frequency verbs with Sine (see [sine.md](sine.md)).

## What a MIMO Sine Test Holds

- **Each control point's own reference.** A reference amplitude against frequency at full level,
  in acceleration, velocity or displacement, with its own alarm and abort bands, as Sine's
  profile. And a reference phase against frequency, in degrees relative to the tone.
- **Or amplitudes only.** For the whole run you may give amplitudes only; the law then chooses the
  phases between the control points. A law that cannot choose them is not offered.
- **One tone at a time.** Every drive plays the same frequency; only the amplitude and phase of each
  drive differ.
- **One schedule for every control.** The levels scale every reference together.

The first version runs up to 24 control points, 12 drives and 8 limit channels.

## What a Run Does

1. **Arm.** The station opens with every drive row as an output and every control and limit row as
   an input, on one clock, and adopts its sample rate. Nothing is driven.
2. **Identification.** Independent random plays on every drive at once, at a level under the
   references, and the controller estimates the response of every control and limit channel to
   every drive over the test's band, with each channel's coherence and the frequencies it can
   trust. A control whose band-mean coherence is under the gate ends the run held.
3. **Review.** With the review hold on, the run waits with the identification on screen and, for a
   sweep, a warning for each resonance the sweep passes too fast for (see "The Sweep-Rate
   Warning"). Nothing drives until you proceed; your words, if you give them, go in the log.
4. **Engage.** The law is built from the identification and the tone starts at the first level. The
   first drive is bounded by the identification's own prediction; a settle window keeps the band
   judgements off while the responses come onto their references.
5. **Every update**, in this order: read the inputs; track every control and limit channel at the
   tone; judge every control against its bands; step the frequency and the level; call the law
   with the drive in force; check its answer; cap each drive's change; scale the whole drive vector
   for the limit channels and for a drive over its ceiling; ramp every drive to the result.
6. **End.** The schedule finishes, or the run stops, aborts or is held, and says why in plain words.

## Setting One Up

Open Acquire & Control and choose **MIMO Sine** under MIMO. The screen opens on Setup, in
sections, with a summary beside every section that lists what is left to fix, and every refusal
and warning the control service gives, by name and in its own words; each opens its section. The
setup is checked by the control service as you edit (a dry run that changes nothing). **Proceed to
Run** applies the setup and shows the run. A setup saves to and opens from the project with the
setup bar, as every test's does.

### Channels

The station, the simulator or the NI hardware, is set for every test type in the Configure view,
opened from **Configure** at the upper right of Acquire & Control. The channel table holds the rows.
Every enabled control row is a control unless you take it out of the controls; a control row taken
out, or a monitor row, can then be a limit channel. Every enabled drive row is a drive, in the
table's order, each with its own **ceiling** and **abort** in volts peak. There is no default for
either: they are your amplifier inputs. On the simulator, the **structure** the run is played on is
chosen in Configure, under **Simulator Models**; its size must match the controls and drives, and by
default it is "Sized to the Channel Table", which takes the linear structure of the table's size.
The simulator's sine structures are a linear one in three sizes (4 controls by 4 drives, 6 by 3 and
3 by 6), three 4 by 4 structures whose resonances move with level (softening mildly, softening
moderately, hardening), a base and article with two limit channels (6 by 4), a 4 by 4 that is nearly
singular at one frequency, a 4 by 4 of weakly coupled pairs (each drive's cross paths at least 21.9
dB under its direct one), and a 4 by 4 loopback (four outputs wired to four inputs: unity gain, a
0.8 ms delay, no coupling), the simulator's twin of the bench's wire loopback.

A run set up and then given other simulator models in Configure is refused at arm until it is
set up again.

### References

One panel per control: its amplitude rows and bands, then its phase rows. **Copy to Every
Control** gives every other control the same reference. Turn **Reference phases** off for
amplitudes only.

### Test

- **Sweep**: a band, logarithmic in octaves a minute or linear in hertz a second, up or down first,
  ended by a count of one-way sweeps, a test time, or whichever comes first.
- **Dwell**: one frequency, for a time or a number of cycles at full level.
- **Stepped sine**: steps on a grid over a band (a number an octave, logarithmic or linear) or on a
  list of frequencies played in the order you give, with a dwell at each step in seconds or
  cycles (on a grid, also one computed per step).

Then the sample rate asked of the station and the control tick.

### Schedule and Identification

The levels in dB under the references, rising to full level; the level the run starts at; and in
the automatic mode how long each level is held (one-way sweeps in a sweep, else seconds). In the
manual mode you step the levels. Identification: its level under the references, the records it
averages, the coherence under which a frequency is not trusted, the gate every control must
reach, and whether the run holds for review after it.

### Control

The law, chosen from the laws the catalog lists that accept the setup, with the parameters its own
manifest declares. A law that does not accept the test type, the numbers of controls and drives, or
amplitudes only is listed apart under **Not Offered for This Setup**, each with the reason, and
cannot be chosen. Then the tracking filter, the guards (see "Guards on Any Law"), and the switch
for the law's requests.

### Limits

Limit channels with their bounds; the notch's settings: the ticks each limit reading is smoothed
over, the release rate (10 dB a second by default), the release hysteresis (0 to 6 dB, 1 by
default) and how near its bound a limit channel must be for a law's own lowered reference to be
taken (see "Limit Channels and the Backstop Notch"); and optionally each shaker's rating record and
moving masses for the feasibility check and the kinematic monitor.

## The Control Laws

Six laws are built in. Each states which test types it accepts, and the controller refuses a
setup that asks a law for one it does not.

| Law | Accepts | What it does |
|---|---|---|
| `matrix` (the default) | sweep, dwell, stepped sine | Corrects the drive through the regularized inverse of the identification every update, at a gain the loop-speed rule sets, and in a sweep carries the drive along the model as the frequency moves |
| `paired` | dwell, stepped sine | Pairs each control with one drive and moves that drive by the ratio of wanted to measured amplitude. No model of the test article |
| `steepest_descent` | dwell | Moves the drive down the gradient of the control error through the identification |
| `secant` | dwell | Finds a gain per control from two successive drives and their responses |
| `ratio` | dwell, stepped sine | Multiplies each drive by a compressed ratio of wanted to measured amplitude on its paired control |
| `followed_poles` | sweep | As `matrix`, on a modal model whose resonances it refits to the readings as the sweep moves them. Needs the station's delay; the simulator only |

### `matrix`

Use it unless you have a reason not to. Its parameters:

- **Carry weight** (0 to 1, default 0.67): how much of the drive the model predicts for the next
  frequency is added as the sweep moves. A correction alone cannot hold a fast sweep through a
  light resonance; carrying the drive along the model removes most of that error on a linear
  structure. 0.67 leaves at most a third of the sweep error whether the damping at test level is
  the same as identified or up to three times it. Lower it if a sweep overshoots a resonance.
- **Gain rule**: the loop-speed rule (the default) sets the gain every update from the tracking
  filter's delay and the identification's group delay near the tone; the fixed gain is the printed
  one.
- **Phase with amplitudes only**: with an amplitude-only reference, take every control in phase
  (the default) or take each control's phase from the measurement.

Its limit, stated plainly: this law assumes the test article responds at test level as it did
during the low-level identification. Where a resonance has moved by less than a quarter of its
half-power bandwidth, expect the same control as on a linear article. Between a quarter and one
bandwidth, expect errors over 3 dB at that resonance. Past about one bandwidth the loop can be lost
and the test will abort. A Q of 50 puts one bandwidth at 2 percent of frequency. If your test
article is one whose resonances move with level, run the identification as near test level as is
safe, climb the schedule in small steps, and watch the error at each resonance on the first levels.

### The Other Four

They are the published methods, offered as published with the loop-speed rule as their default
gain, and they are the baselines a comparison of laws needs. Their limits, from a research testbed
on a linear 4 by 4 structure:

- `paired` held none of the dwells on that coupled structure. Expect it to fail where the shakers
  are strongly coupled. It is the simplest law and the template for writing your own. Its check
  runs its dwells, noise, stepped and ladder cases on the weakly coupled pairs, the setting it is
  for, and each such case names that structure.
- `steepest_descent` held every dwell, two stepped sines in five, and no sweep.
- `secant` held four dwells in five (with either carry), and no stepped sine or sweep. Its
  parameter **Carry** chooses how the gain is carried to the next frequency (`factor` or
  `plain`).
- `ratio` held every dwell and every stepped sine with the reference pairing, one dwell in five
  with the drive pairing, and three sweeps in twenty. Its parameter **Pairing** pairs each drive
  with a control by the reference (the default) or by the drive.

### `followed_poles`

A research law, on the simulator only: a hardware station refuses it. It carries and corrects the
drive as `matrix` does, but on a modal model of the structure whose resonances it follows: every
second update it refits the frequency and damping of the modes near the tone to the last 40
readings, held to the identification's by a prior, and moves a mode only where the readings show
it. While the followed model predicts the readings worse than the identification's own, it uses
the identification's inverse.

Its model's modes are fitted with the station's delay taken out, so it needs **Station delay**
(`station_delay_s`) set: the delay from the controller's writing a drive sample to its reading the
response, the structure's own response left out (the controller's output queue, the identification's
frames ahead plus one block, the output and acquisition chain and any transport delay). Without it
the law is refused. On the simulator's sine structures it is 266 samples, 16.2 ms at 16384 Hz.

Its limits, from the research benchmark's screen of its research version against `matrix` on
simulated frames: on the nonlinear frames it completed more sweeps on 5 of 6 groups and had the
lower 95th-percentile error on 30 of 33 structures, but no sweep of either law ended within
tolerance; on the strongly nonlinear frames it drove harder and over-tested by more, 0.4 to 7.2 dB
in the paired medians. In the law check it over-tests a steep reference by 2.59 dB.

### Choosing a Law

- A sweep: `matrix`.
- A dwell or a stepped sine on a linear test article: `matrix`; the others to compare against it.
- A test article whose resonances move with level: `matrix` with the limit above in mind. No built-in
  law has been shown to hold such an article through a resonance that moves more than its own
  bandwidth.

## The Sweep-Rate Warning

Before you proceed, the review lists every resonance the identification found in the band where
the sweep spends fewer than two of the resonance's own time constants inside its half-power
bandwidth. There the law's error through the resonance is expected to be large, whatever the law;
a slower sweep is the remedy. A warning never holds the run.

## Limit Channels and the Backstop Notch

A limit channel is watched, not controlled. The notch has a depth: the share of the reference the
limit channels allow, which the controls are judged against and every law is told, so the law aims
for the lowered reference instead of pushing against the limit. The depth goes deeper at once when
a limit channel demands it. It comes back up only once the limit channels allow more than the
release hysteresis above where it is (1 dB by default), and then at most at the release rate (10 dB
a second by default), and while the notch is in the whole drive vector rises no faster than that
rate either. So a limit reading that wanders near its bound does not take the notch in and out,
and the controls follow it out without overshooting their reference.

Behind the depth is the backstop: where a limit channel's predicted or measured response would
still pass its bound on an update, the controller scales the whole drive vector down, its direction
kept, and every control falls with it. The backstop needs no model to be right.

The notch is in while the depth holds the reference down or the backstop cuts the drive. The
**Notch** on the run card says which, and by how much: the reference held down so many dB (the
deepest control's), the drive cut to a scale (and in dB) on the last update, or both; the Run
reading shows the cut. Per control, the controls table gives how far under its reference it sits,
and each control's amplitude plot draws the reference as held down, with its bands around it.
After the run the card gives, per limit channel, its worst margin to its bound and where, and the
time the notch was in. A limit channel outside its own abort band ends the run.

A law may also return a reference of its own, lowered at some controls. The controller takes it
only when it lowers every control's reference and a limit channel is within the "near the bound"
setting of its bound, or the notch is releasing; it then judges the controls against it and
reports the depth at each. No built-in law does this in the first version.

## Guards on Any Law

Whatever the law answers, the controller holds it to:

- **The answer checks.** One finite amplitude per drive and a well-formed request; an answer that
  fails is a miss and the drive in force is held. Three misses in a row (by default) end the run
  with the controlled stop.
- **The time budget.** A call that takes longer than its share of the tick is a miss.
- **A change cap per drive** in dB a tick and **a rise cap** in dB a second.
- **A rotation cap**, the most each drive's phase may turn in one tick: off by default. The largest
  turn the law asked for is reported either way, so a value can be set from a run.
- **The divergence stop.** A cap that binds without a break for a second (by default) ends the run
  with the controlled stop.
- **A drive over its ceiling** scales the whole drive vector down, direction kept, and is counted;
  a drive at its abort ends the run.
- **Each control's alarm and abort bands**, outside a settle window; three ticks in a row outside
  its abort band (by default) end the run.
- **The kinematic and shaker checks**, per shaker, with the shaker ratings given.

### The Law's Requests

A law may ask the controller to hold or slow the sweep. Every request is shown on the run screen
and counted. It is acted on only with **Act on the law's requests** switched on: a hold lasts at
most 2 seconds without a break, and a slow-down never goes under a tenth of the scheduled rate.

## What the Readings Mean

The run card and the Readings plot show these; they are never judged:

- **Opposition**: the drive vector's size over the smallest drive that would give the same control
  response by the identification. 1 means the shakers are not working against each other; a large
  value means they are, and the test is spending drive to cancel itself, often near a frequency
  where the controls are nearly dependent.
- **Largest share**: the largest share of the drive vector carried by one shaker. Near 1, one
  shaker is doing the test alone.
- **Largest turn**: the largest turn of a drive's phase in the last tick.

The Law card shows the law's own declared values on its last call (for `matrix`: the gain, the
group delay, the condition number at the tone, the share carried, the turn asked for, the share of
the drive the controls cannot see, and the model's modes), its unusable calls in a row, the ticks a
guard bound, the ticks a drive was over its ceiling and its requests.

## The Run Screen

The pages until you change them:

- **Controls**: each control's amplitude against its reference (as held down while the notch is
  in) with its alarm and abort bands around it, and its phase against its reference phase.
- **Error**: every control's error in dB with the alarm and abort band lines, the notch depth while
  one is taken, and the readings.
- **Limits**: each limit channel against its bound.
- **Drives**: every drive's amplitude and phase as played.
- **Identification**: the response from every drive to a control (in the control's unit per
  volt), its phase, and every channel's coherence with the trust line. The review hold brings this
  page forward; proceeding brings the Controls page back.
- **Law**: the law's values and the run in numbers.

In a sweep or a stepped sine the plots are against frequency; in a dwell, against the last minute
of test time. The progress counts the one-way sweeps or steps completed: a sweep counts once it
reaches the band's edge, so a finished one-sweep run reads 1 of 1, and one stopped part way 0. The
run card holds the verbs: Start, Proceed, Hold and Resume, and in a sweep Jog and
Go To, the level steps in the manual mode, Stop and Abort. A sweep turns only at the band's edges:
to sweep the other way from where the tone is, go to a frequency, or stop and start a sweep in the
other direction. Stop ramps every drive down together; Abort ends the run
within one batch.

## What a Run Saves

Every run that arms leaves a run folder under the data folder, named
`<date>-<time>_mimo_sine_<run id>`, written while the run goes and readable at every moment. From
the arm: the configuration as accepted and what it resolved to (every control's reference and
phase, every limit channel's bound, the test, the schedule, the guards, the identification's
settings, the law and its parameters, the request switch, the notch's settings, and the
simulator's structure on the simulator), the channel table as it was used, and the law with what it accepts. After the
identification: every control's and limit channel's response to every drive, the coherences, the
lines trusted, the level it was taken at and the loop's delays the law was told. Then, tick by
tick, every control's, limit channel's and drive's tracked amplitude and phase with the tone's
frequency, the level, each control's reference and error, the notch and the two readings, and
every call of the law with what the guards did with it. The run's log holds every state, level,
hold, notch, alarm, unusable call, request and trip, and the end holds how the run ended and its
report. Every input as read and every drive as played are kept as time histories unless the
setup switches them off. The run is labelled SIM or HARDWARE from the station it ran on.

**Open in Analysis** turns the folder into a project:

| In the run folder | In the project |
|---|---|
| `streams/inputs`, `streams/drives` | Time histories, in SI |
| `results/reference` | `Specification`: one tone with each control's own amplitude, phase and bands (a sweep or a dwell) |
| `results/channel_levels` | `Sine Levels`: every control, limit channel and drive over the last one-way pass at the highest level (a sweep or a stepped sine) |
| `results/identification_frf` | `identification_frf`: an FRF of every control and limit channel against every drive |
| `results/identification_coherence` | `identification_coherence`: each one's multiple coherence |
| `results/identification_trust`, `results/control_trace`, `results/limit_ch<n>`, the rest of `channel_levels`, the log and the end | The `Run Record`, as named arrays |

The project also holds the setup the run was accepted with, which opens again as a MIMO Sine
setup. A stepped sine's references and a dwell's levels have no analysis object yet and stay in the
run record. The file-by-file list is in [run-folder.md](run-folder.md), "What a MIMO Sine Run Saves".

## Writing Your Own Law

A law is a WebAssembly component of the `sine-mimo` world. Start from the template in
`crates/hertzforge-law-sdk/template/sine-mimo/` (the `paired` law, the simplest), and read the
world's section of [control-law-spec.md](control-law-spec.md) for what the law is told every
update and what it may answer. Import the law file with **Import Law** in the Control section; an
imported law runs on the simulator at once, and drives hardware only once its check has passed.

## Shown on the Simulator Only

- **Coupling between shakers and behaviour that changes with level** are shown on the simulator's
  structures only. On hardware the module is checked on a wire loopback: the tone's coherence and
  phase between channels, the timing of the sweep, the dwell and the step, the drive ceilings, the
  guards, stop and abort, and a law file arming. A loopback does not show a structure.
- **The limits stated for each law** come from a research testbed, on a linear structure, and are
  provisional until the bench has run.
- **The simulator's structures are models.** The softening and hardening structures change with
  level in a stated way; a real test article can do more, and less.
