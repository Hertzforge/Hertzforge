# MIMO Shock

MIMO Shock fires one shock at a time on several shakers at once and judges it at several control
points. Every control point has its own target: a classical pulse (a half-sine, a sawtooth, a
trapezoid and so on, at its own peak, duration and polarity), or a shock response spectrum (SRS).
Between pulses a control law builds every drive for the next pulse, and the controller checks
those drives before they play. Drives are in volts throughout, each held under a ceiling and an
abort that the operator gives.

This guide is for a test engineer who has run single-shaker shock. It covers what a MIMO shock test
holds and does not hold, how to set one up, the control laws and when to pick each, what the judge
checks, the level ladder, what stops a run and why, and the limits of this version. It shares its
station panel, channel table, run bar and control-law panel with the other test types, and its run
screen with Classical Shock and SRS (see [shock.md](shock.md) and [srs.md](srs.md)).

## What a MIMO Shock Test Holds

- **Each control point's own pulse or SRS.** Each control is judged against its own target, by the
  same rules a single-shaker test uses: a classical pulse against its standard's tolerance band,
  an SRS against its tolerance in every band.
- **One kind of target for every control.** A run is all classical pulses or all SRS.
- **Timing between points is free unless you hold it.** By default each control is judged after its
  own best-fit delay is taken out, so two points may peak a fraction of a millisecond apart and
  both pass. The delays are reported with every pulse, and a spread of the delays wider than the
  timing window is a warning. When the timing between points matters to the test, hold it: give
  each control the delay it must arrive at, and a control outside its window fails on timing.
- **Not a rigid-body motion.** The test holds what each control point sees, not how the structure
  moves between them.

The first version runs up to 16 control points and 8 drives.

## What a Run Does

1. **Arm.** The station opens with every drive row as an output and every control and limit row
   as an input, on one clock, and adopts its sample rate. Nothing is driven.
2. **Identification.** One shot of independent band-limited random plays on every drive at once,
   at a level found by a short probe and held under every drive's ceiling. The controller
   estimates the structure, controls by drives, on the pulse's own spectral lines, so the travel
   delay of every path is in it. A control measured with a band-mean coherence under 0.8, or whose
   target has under 90% of its energy on lines that can be trusted, holds the run before any pulse.
3. **Reachable floors.** From the identified structure the controller works out, for every
   control, the best response the drives can give it: the share of its target the drives can reach
   at all, and how far that best response still is from the target (its floor). With every control
   judged on its own, a control whose floor is outside its own tolerance holds the run before the
   first pulse, whatever the review setting says (see "The Hold Before the First Pulse").
4. **Review.** With the review hold on, the run waits with the identified gains, the coherence and
   every control's floor on screen. Nothing fires until you proceed.
5. **Seed.** The control law builds the first drives at the first level of the ladder. The
   controller predicts every control's response to them; a first drive predicted over its bound
   fires nothing and ends the run.
6. **Pulses.** For each pulse, in this order: check the drives (each drive's ceiling and abort, the
   predicted response at every control and limit), fire every drive in one shot, measure every
   input, judge each control, check the measured response against the abort lines, move the
   ladder, watch for divergence, call the law for the next pulse at the next level, and check its
   answer.
7. **End.** Full level passes its pulses, or the run stops, aborts or is held, and says why in
   plain words.

## Opening the Test

Open Acquire & Control and choose **MIMO Shock** under MIMO. The screen opens on Setup, in
sections, with a summary beside every section that lists what is left to fix; each item opens its
section. Take control to edit and run; an observer sees the run the service holds. The setup is
kept across reloads, and checked by the control service as you edit (a dry run that changes
nothing). **Proceed to Run** applies the setup and shows the run.

## Setting One Up

### Station and Channels

The channel table is the one every test type shares. MIMO Shock reads its enabled rows:

- **Controls.** Every enabled control row is a control, in the table's order, each with its own
  target. A control row can be taken out of the controls, and then it can be a limit.
- **Drives.** Every enabled drive row drives the run, in the table's order. Each drive has its own
  **ceiling** (a drive over it is scaled down to it) and its own **abort** (a drive over it is
  refused and the run aborts), both in volts peak. Neither has a default: they are your amplifier
  inputs.
- **Limits.** An enabled monitor row, or a control row taken out of the controls, can be a limit
  with the peak, in its own unit, that ends the run. A limit is watched, not controlled.

On the simulator you may name the simulated structure the run is played on; its size must match
the controls and drives. The capture switch writes every pulse, the identification and every law
call to disk when the run ends.

### Targets

Choose the kind of target for every control: **Classical pulse** or **Shock response spectrum**.
Then give each control its own target, one control at a time; **Copy Control 1 to Every Control**
starts the others from the first.

- **Classical pulse.** Shape, amplitude, duration, polarity and the shoulders that bring the
  velocity and displacement back to rest, as in Classical Shock. Every control is judged by the
  standard you choose. A negative polarity plays that control's pulse upside down.
- **SRS.** The band grid and the tolerance are the same for every control. Each control has its
  own breakpoints (typed, or opened from a breakpoint table file with the File menu) and its own
  synthesis: the control service builds a full-level waveform for each control's SRS, so a law that
  works on waveforms can run an SRS test too.

Beside the target, the control service draws the control's target at full level. It is a
computation, never a measurement.

### Timing and Verdict

- **Timing.** Free (the default) or held, with the window (2 ms unless you change it). Held, give
  each control its delay.
- **Verdict.** **Every control** (the default): a pulse passes when every control is inside its own
  tolerance. **Global SRS error** (an SRS target only): a pulse passes when the energy-weighted SRS
  error over every control is inside the tolerance and no control is over its abort line. The
  global error is shown with every SRS pulse either way.

### Ladder and Identification

- **Levels.** The ladder climbs from its first level to full level, 0 dB. The usual ladder is
  -12, -9, -6, -3, -1.5 and 0 dB: 3 dB steps with a half step before full level.
- **Firing.** Automatic fires each pulse once the time between pulses has passed; On Fire waits for
  you to press **Fire**. In both, the ladder alone sets the level.
- **Pulses a level.** A level below full level moves on at its first passing pulse; a level that
  fires this many pulses (3 unless you change it) without a pass ends the run. Full level completes
  on its own count of passing pulses (also 3 unless you change it).
- **Time between pulses.** The largest of the cooling, settling and law times.
- **Pulse grid.** The sample rate requested of the station (12 800 Hz unless you change it), the
  record length (the shortest that holds every target unless you choose one, at most 16 384
  samples), the band identification excites and the law corrects in, and where each target starts
  in its record. An SRS target is synthesized as a waveform that rings down from its lowest band, so
  it needs a longer record than a classical pulse: at 51 200 Hz no record holds one from 50 Hz, and
  the setup says so.
- **Identification.** The most each drive plays in the shot, the records averaged, the coherence
  floor under which a line is not trusted, and whether the run holds for review afterwards.

### Control Law

The law panel lists every MIMO Shock law the control service's catalog holds that accepts the kind
of target you chose, built-in laws first. Choosing a law shows its summary, who wrote it, its check
record, whether it may drive hardware, the target kinds it accepts, and its parameters as its own
manifest declares them, each with its range. A new setup opens on the law the control service names
the default for its kind of target: `inverse` for classical pulses, `shape` for SRS. Changing the
target kind moves a default law to the new kind's default; a law you chose stays when it accepts the
new kind, and otherwise gives way to the new kind's default.

Under the law are the guards the controller holds every law to, whichever law it is: how far a
drive's peak, or any third-octave band of a drive, may grow from one pulse to the next beyond the
level step; how many pulses in a row with the error rising, and how far a drive may grow at one
level, before the divergence watch stops the run; the bound on the first drives; the law's time a
call; the misses in a row that stop the run; and the pulses in the run at most.

### Limits and Aborts

The abort lines and the kinematic limits at every control (see "Abort Lines: Absolute and
Scaled").

## The Control Laws

**Where the laws stand.** Control laws for several shakers firing shock are young. The laws built
in are a starting point, not a finished answer. On a structure whose resonances shift with level,
which is common where there are bolted joints, a correction through a model of the structure can
grow instead of settle, and these laws can leave a control point outside its tolerance. When that
happens the controller stops the run and says so, instead of climbing to a level the run has not
earned. Better laws are an active area of work. A new law is written as a law file against the same
interface the built-in laws use, and is added without changing anything else in the test; the
law-writing guide ([control-laws.md](control-laws.md)) starts from `paired`, the simplest of them.

The catalog names the laws; the three built in at present are below.

**`inverse`** builds the first drives by passing each control's target through the regularized
inverse of the identified structure, line by line, and after each pulse corrects the drives by a
share of that inverse times the waveform error. A line whose error grew has its correction cut, and
it is restored slowly. It accepts both kinds of target. Pick it for classical pulses on a structure
that behaves the same at every level. On a structure that changes with level the line cuts slow
the growth but do not stop it, and the divergence watch is what ends the run.

**`shape`** never forms a waveform error. It keeps a smooth gain for each control, shapes that
control's target by the ratio of target SRS to measured SRS after each pulse, with a weight that
rises with level, and drives the shaped targets through the regularized inverse. Each control's
delay is re-estimated from its response. It accepts SRS targets only. It is the several-drive form
of the single-drive practice of correcting the model's magnitude from the SRS ratio and leaving its
phase alone. Pick it for SRS tests; it stays bounded where a waveform correction grows, but on a
strongly coupled structure it can stall short of the tolerance.

**`paired`** pairs each control with one drive (the first control with the first drive, and so on)
and scales that drive's spectrum by the smoothed ratio of wanted to measured magnitude at its
control. It uses no model of how one drive moves the other controls. It accepts both kinds of
target. Pick it when the drives barely affect each other's control points, or to see what a law
without a coupling model does on your structure. On a coupled structure the pairs work against
each other. It is also the template for a new law.

**How a law's model follows the structure.** `inverse` and `shape` build every drive through a
model of the structure, and both have a `model` option for how that model is kept current as the
structure changes with level: `frozen` (the default) keeps the identification; `secant` reproduces
the pulses of the level being played; `track` and `track_secant` fit the structure's modes and
follow their frequencies and damping from pulse to pulse. The options other than `frozen` are off
unless you choose them. On the simulator's jointed structures, whose joints soften within a pulse,
none of them did better than `frozen`; [control-laws.md](control-laws.md) gives the numbers and
what each costs.

An imported law must pass the law check before it may drive hardware; any law runs on the
simulator. The law picker shows each law's check record.

## What the Judge Checks

After every pulse each control is judged on its own, and the verdict grid on the run screen shows
every control on every pulse with a mark and a word: a check and **Pass**, or an octagon with the
reasons it failed, by name:

- **Tolerance** (Tol): the pulse outside its standard's tolerance band, or the SRS outside its
  tolerance in some band.
- **Timing** (Time): with timing held, the arrival outside its window.
- **Polarity** (Pol): the pulse upside down.
- **Duration** (Dur): for an SRS target, the response's effective duration outside 0.8 to 1.2 times
  its synthesized waveform's.

Every control also reports its **signed error**, in dB: positive is an over-test (the peak over the
target's peak, or the SRS above its target), negative an under-test. The pulse's detail shows each
control's error, its delay, and how far it stood from its scaled and its absolute abort line.

Reported, not judged: the energy and rms duration against the target's (a warning outside their
stated tolerance), and a spread of the controls' arrivals wider than the timing window.

The **verdict mode** decides what the ladder is fed: every control inside its own tolerance, or the
global SRS error.

## The Hold Before the First Pulse

Some structures cannot give every control point its target with the drives available, most often
when there are more control points than drives. With every control judged on its own, a control
whose reachable floor is outside its own tolerance would fail every pulse however good the law, so
the run holds before the first pulse, names the controls and shows each control's floor.

To go on, write in your own words why the run should continue (for example, that a point is known
to be short and the test accepts it) and press **Proceed With This Acknowledgement**. Your words go
in the run's log. Or stop, and either change the setup or judge the run by the global SRS error,
under which the floors are shown and nothing holds.

## The Level Ladder

The ladder starts at its first level and moves up one level on a passing pulse. The level card
shows every level with the pulses fired and passed at it and marks the level in force; the half
step before full level is marked as such. A level spends at most its pulse allowance; spent
without a pass, the run ends there. The ladder never climbs on a failing pulse.

When the first pulses predict an over-test above the upper tolerance at the next level, the
controller may take a smaller step than the table's. The level card then shows the halved step,
and the pulse that caused it says which level was planned, which was taken, and the over-test
predicted at each.

## Abort Lines: Absolute and Scaled

Each control has two abort lines, both from one margin over its target's peak (twice the standard's
amplitude tolerance for a classical pulse, 6 dB for an SRS, unless you set it):

- **The absolute line** is that margin over the target at **full level**. A measured peak over it
  ends the run at any level. It is always on; it cannot be turned off.
- **The scaled line** is the same margin over the target at **the level played**. With scaled aborts
  on (the default), a pulse over it stops the run. With them off, the pulse is judged failing, the
  run flags every control that crossed its scaled line and by how much, and goes on without
  climbing; a standing notice on every page of the run says the scaled aborts are off.

Turn the scaled lines off when the law is expected to correct an over-test at a low level, on a
structure that changes with level, and stopping at the first over-test would end the run before the
law could correct it. The absolute lines still protect the article at full level. The switch is
locked once a run is armed.

## What Stops a Run and Why

The run's end is shown in plain words on the level card and in the Run card. The guards are there
for you: each one stops the run before the drives, the structure or the article are taken somewhere
the run has not earned.

| The run says | What happened |
|---|---|
| Completed | Full level passed its pulses |
| A level's pulse allowance was spent | A level fired its allowance without every control passing; the ladder did not climb on it |
| The divergence watch stopped the run | The error rose on consecutive pulses at one level, or a drive grew past its allowance at one level: the law was making it worse |
| The predicted response was over an abort line | The next drives were checked before firing and were not fired |
| The run reached its cap on pulses | The pulses in the run at most |
| An abort line, scaled to the level played | A measured peak over its scaled line, with scaled aborts on |
| The absolute abort line | A measured peak over the full-level line, at any level |
| A drive went over its abort | A drive's peak past its abort voltage |
| A limit channel, or a kinematic limit | A monitored peak, or a control's velocity or displacement, over its limit |
| The control law gave no usable drives | The law's answers were unusable (late, failed, or refused by a guard) too many times in a row, or it failed outright; the reason is named |
| Held at identification | Too little coherence, or too little of a target on trusted lines |
| Stopped or aborted by the operator | Stop finishes the pulse in flight; Abort takes every drive down at once |
| Nobody held control | Firing pauses when nobody holds control, and the run stops if nobody claims it in time |

## The Run Screen

The run uses the pulse test screen: pages of plots, with **+ Plot** to add any quantity the service
draws, and the control dock beside them. Its pages to start with:

- **Pulses**: each control's measured pulse over its reference at the level fired, moved by its
  delay, with its tolerance band. Traces differ by line style as well as color.
- **SRS**: each control's SRS over its target and tolerance (an SRS target).
- **Drives**: every drive as played.
- **Verdicts**: the verdict grid (controls down, pulses across) and the latest judgement as a table.
  Pick a pulse in the grid to see its detail.
- **Ladder**: the levels and the run's numbers.
- **Identification**: the identified gain from every drive to a control, every control's
  coherence, and the reachable floors.
- **Law**: the law's named values on its last call.

The dock holds the Run card (the verbs, Fire in On Fire mode, and the acknowledgement when the run
holds for one), the level card (the law, the ladder, the floors before the first pulse, the end),
the verdict grid, the pulse in detail and the station. A page opens in a window of its own for
another monitor; every window keeps Stop and Abort.

## Limits of This Version

- **The laws are young.** Control of several shakers firing shock is not a settled practice, and
  few published methods for it were found. The built-in laws can leave a control point outside
  tolerance on a structure that changes with level; the controller stops instead of climbing.
- **A structure whose resonances shift inside a pulse can stop a run low.** Where bolted joints
  soften while the pulse is ringing and recover as it decays, no linear model reproduces the pulse,
  and the controller may stop the run at a low level of the ladder: its guards refuse a law's
  answer that grows a drive past its caps, or a level spends its pulses without passing. The run
  says which, on the pulse and at its end.
- **Nothing here has run on a shaker.** Every result in this guide was measured on the simulator.
- **The simulator's structures are models.** The jointed structures the laws are checked on soften
  and gain damping with level in a stated way. A real joint can do more, and less, than the model.
- **The hardware proof so far is a wire loopback.** It shows the timing, the channel sync, the drive
  path, the ceilings, the guards and the stops. It does not show a structure: coupling between
  shakers and behaviour that changes with level are shown on the simulator only.
- **The level is the ladder's alone.** There are no trains, bursts or manual level moves.
