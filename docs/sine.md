# Sine

A Sine test drives one tone and closes the loop on its amplitude every control tick: the tone sweeps
across a band at a rate, holds one frequency, or steps across a band, while the control law decides
how much drive the tone needs for the control point to read the profile at the present frequency
and level. The frequency, the sweep and its direction, the tone's phase, the level schedule and every
abort are the controller's, never the law's. The drive is in volts peak throughout, held under a
drive ceiling and an abort that the operator gives.

This guide is written for the test engineer running the test. It follows the order of a run: the
station and channels, the profile, the test, the schedule and identification, control, limits and
capture, then the run itself. Sine shares its run bar, station panel, channel table and control-law
panel with the other test types, and its level and ladder controls with Random.

## What the Test Does

A run goes through these steps in order:

1. **Arm.** The station opens, adopts its sample rate, converts the control tick to whole samples at
   that rate and passes its gate. Nothing is driven. When a shaker is given, the profile is checked
   against its ratings first; a profile over them blocks the run until the operator arms past it in
   their own words.
2. **Identification.** A white-noise burst under full level. The controller measures how each
   control channel answers across the band, how well (the coherence), and the delay between the
   drive and the response. A band coherence under the one asked for holds the run before anything is
   driven at test level.
3. **Review.** With the review hold on, the run waits with the identified gain and its coherence on
   screen. Nothing drives until the operator proceeds.
4. **Engage.** The control law is built from the identification, and the tone starts at the first
   level of the schedule. Its first drive is at most the identification's prediction there plus
   6 dB, and never over the drive ceiling.
5. **Each tick.** The controller reads every input at the tone's frequency, combines the control
   channels, holds the limit channels under their bounds, and asks the law for one amplitude. It then
   limits how far that amplitude may move in a tick and in a second, keeps it under the notch and the
   drive ceiling, and judges the control point against the profile's alarm and abort bands at the
   present frequency.
6. **End.** The test reaches its end (a count of sweeps, a test time, a dwell's length or the last
   step) and the drive ramps down; or the run is stopped, aborted, held at identification, or the
   law gives no usable drive.

The controller holds every law to the same checks, whichever law it is.

## Opening the Test

In Acquire & Control, choose **Sine** from the test types. The screen has two steps, **Setup** and
**Run**. The run bar (where the data comes from, the run's state, elapsed time, level, the
frequency, the sweep under way, who holds control, and Stop and Abort once a run is live) stays in
view on both. The setup's sections are tabs; the summary beside every section says what the setup
holds, what the control service worked out from it, and what is still missing, each a click from
its section.

**Proceed to Run** applies the setup (with any channel table edits first) and shows the run.
**Start** arms if needed, then runs identification. The setup is checked as it is edited: every
problem shows in the section it belongs to, and a setup with errors is never accepted.

## Station and Channels

The station panel chooses the simulator or the NI hardware, as for Random. The run controls on the
channel table's enabled **control** rows (combined as set under Control) and drives on its first
enabled **drive** row. Other enabled inputs can be watched on the Responses page or held under a
bound as limit channels.

## The Profile

- **Title:** the run's name. **Profile name** is optional.
- **Rows:** each a frequency and a value in its own unit: acceleration in g peak, velocity in in/s
  peak, or displacement in in peak to peak. A row holds from its frequency to the next row's.
  Neighbours of one unit are joined on log-log axes; where the unit changes, the left row's value
  holds up to the next row, and a jump of more than 0.5 dB there is a warning. The rows must cover
  the test's band.
- **Bands:** how far below and above the profile the control point may read, in dB: past the alarm
  band the run warns, past the abort band it stops. The usual bands alarm at 1.5 dB and abort at
  3 dB either side. A row with bands of its own uses them from that row to the next.
- **Paste or read rows:** rows pasted as text, or read from a file by its full path on the computer
  that runs the test. A header row names the frequency column and the unit columns (for example
  `freq_hz, disp_pp_in, accel_g`), with one value given in each row; tabs, commas, semicolons or
  spaces separate them, and `#` starts a comment. The control service reads the rows and says what
  it found; a row it cannot read is named, and the table keeps what it had. The rows replace the
  table; the bands stay.

Beside the table the control service draws the profile at full level as acceleration, velocity and
displacement, says where it changes unit, and gives its peaks over the band: a computation, never a
measurement. With a shaker given under Limits, it also says whether the profile is within the
shaker's ratings.

## The Test

- **Sweep:** the frequency moves across the band, at octaves a minute or hertz a second, up and
  down. **Start at** is the band's low edge unless given; **First leg** says which way it starts.
  The run ends after a count of one-way sweeps, a test time, or whichever comes first, counted over
  the whole run from engage at every level.
- **Dwell:** the tone holds one frequency for some seconds or cycles.
- **Step:** the tone holds each step across the band in turn, so many steps an octave, evenly in
  octaves or in hertz, each for as long as it needs, or for some seconds or cycles.
- **Rate:** the samples a second asked of the station; the usual rate is 16384 Hz. The band ends
  under 0.45 times the rate.

The control service gives the test's length in test time, the identification burst's length, and
whether the tracking filter can follow the sweep: a sweep too fast for it is warned of, or refused
when the readout would fall far behind the tone.

## The Schedule and Identification

- **Automatic** moves the level by itself: each level but the last is held for some one-way sweeps
  (a sweep test only) or seconds, then full level for its own. **Manual** holds the level until the
  operator steps it during the run. The ladder can be handed over between the two during a run.
- **Levels:** relative to the profile, in dB, rising; the last is full level. **Engage at** is a
  level of the schedule.
- The run itself ends at the test's end. A test that would end before the ladder has held full level
  is refused.
- **Burst level under full:** how far under the full-level profile the identification burst plays.
- **Coherence it must reach:** over the band. Under it the run is held and never drives at test
  level.
- **Hold for review:** the run waits after identification with the identified gain and its
  coherence on the Identification page; **Proceed** in the run bar goes on.

## Control

The **control law** panel lists the laws of the Sine world from the control service: the built-in
law first, then imported law files. It shows the chosen law's author, file hash, check record and
whether it may drive hardware, and its parameters, drawn from the law's own manifest. A parameter
the law declares **live** can be changed while the run runs, from the run's parameter card; every
other parameter, and the law itself, is fixed once the run starts. **Import Law** reads a law file
by its full path on the computer that runs the test. An imported law runs on the simulator at once;
it drives hardware only once its check has passed.

The built-in law is an amplitude servo on the error in dB, blended with the drive the
identification predicts at the present frequency. Its prediction weight is live.

- **Control point:** the control channels combine by a weighted average (each control row's
  weight), the lowest or the highest.
- **Tracking filter:** each input is read at the tone's frequency through a narrow filter that
  follows the tone: a share of the frequency, capped, or a fixed bandwidth. Narrow is steady and
  slow to follow a fast sweep; wide follows fast and lets more noise in.
- **Control tick:** how often the law decides, from 2 ms for a built-in law and 8 ms for a law
  file. The usual tick is 10 ms.
- **Where the identification is trusted:** the law is told the prediction only where the coherence
  reaches a figure and the gain is not too far under its peak.
- **Guards** the controller holds every law to: the largest change a tick and the largest rise a
  second, how long those limits may hold the law back before the run stops, the time a call may take
  and the unusable calls in a row that stop the run, and how many ticks the control point may read
  far under its target before the run stops for a lost control point. A law gives one amplitude a
  tick, a number at or above zero; anything else, or a late answer, is unusable, and the drive in
  force holds.

## Limits

- **Drive ceiling** and **drive abort**, in volts peak at the amplifier's input. There is no default
  for either; give the values for your amplifier. The drive is held at or under the ceiling, whatever
  the law asks. A drive at or over the abort aborts the run.
- **Controlled stop:** the drive ramps down at the controller's own rate, 120 dB a second of station
  time; the section shows how long that takes from the drive ceiling. The ramp is not set here.
- **Kinematic monitor:** with a shaker given, the run stops when the response passes the shaker's
  velocity or displacement rating.
- **Limit channels:** an input whose response must stay under a bound whatever the control point
  asks for, such as a fixture's resonance. Each has a bound in the profile's form, a value at each
  frequency; a bound can be named for several channels to share. Where a limit channel would pass
  its bound the drive is held down there (the notch) and the control point may read under its
  profile; a limit channel outside its own abort band stops the run. The notch's settings (the
  drive under which a channel's gain is not estimated, its smoothing, and how fast the drive comes
  back once it releases) are under **Show**.
- **Reference output:** a constant-amplitude copy of the tone, locked to it, on a second output: a
  sine, a square or a steady level. It needs a station with two outputs and a second drive row; the
  simulator has one, so it plays none. It ramps down with the drive at a stop.
- **Shaker and moving mass:** the shaker record and the moving masses the profile is checked
  against at arm. Without them, feasibility is not evaluated, the kinematic monitor is off, and the
  run says so.

## Capture

What the run records beyond its report: nothing (`off`, the default) or the entire test
(`entire_test`); recording only what is started and stopped by hand, or the first seconds at each
level, is refused when the setup is applied. Also here: how often to save a snapshot, and the run
folder's name under the program's local data folder (`sine` unless named); when that name is
taken, the run gets the next free one (`sine-2`, `sine-3`, ...). Fields kept with the run (a run
number, an operator) are listed here.

The entire test is recorded from the moment the tone starts until the run ends. The run folder
holds:

- the capture description (`capture.json`): the channel table, the station rate and the run's
  settings. Its sample stream (`telemetry.f32`) is empty: a Sine run records no time history.
- the control law's record (`law.json`) and one record per control tick (`law_ticks.bin`,
  described by `law_ticks.json`): the tone's frequency, the target, the measured amplitude, the
  drive in force, the law's output and what was applied.
- with a save interval set, a snapshot of the run's state at each interval of test time
  (`snapshot_<time>.json`): the frequency, level, target and measured amplitude, each channel's
  tracked and broadband amplitude, the distortion and coherence.

## The Run

The run bar carries the verbs that apply now: **Apply Setup**, **Arm**, **Start**, **Proceed** at the
review hold, **New Run** once a run has ended, and **Stop** and **Abort**, which any connected screen
may use. **Stop** ramps the drive down; **Abort** takes it down at once. Beside them:

- **Hold** freezes the test's progress: the sweep, the dwell and the level clocks stop, and the tone
  stays at its frequency and level, still under control. **Resume** starts them again.
- On a sweep, **Reverse** turns the sweep, **Jog** moves the frequency by 1, 10 or 100 Hz (stopping
  at the band's edge), and **Go to** moves it to a frequency inside the band. The tone moves without
  a break in its phase.
- The **Ladder** switches between automatic and manual. In manual the level steps down and up by the
  step chosen, or goes to a level of the schedule; the drive follows within the controller's limits.

A verb that cannot run says why when it is focused or hovered: "Once the tone is driving at the
schedule", "Take control to turn the sweep", "Switch the ladder to manual to step the level".

The level card beside the pages says what the run is doing in plain words ("Sweeping up at 4 oct/min
at -6.0 dB", "Holding at 251.3 Hz", "Limiting on channel 3", "Ramping to -6.0 dB at 100.0 Hz",
"Holding for review"), shows the tone's frequency large with the sweep's direction, which sweep is
under way ("Sweep 2 of 4, up"), the level the drive is at against the level commanded, the target
against the measured amplitude with the difference in dB, how far through the schedule the run is,
and the drive. It also shows the law's unusable calls and the ticks a limit held it back, and the
identification (the coherence against the one asked for, the share of the band it trusts, the
delay it measured). When the run ends, the card says how, how long the drive took to ramp down, and
for a protection what tripped, at which frequency and level.

The **parameter card** lists the run's law's parameters: a live one can be changed once the tone is
driving at the schedule (the law is built when the run goes on from identification), and applies from
the next tick; the others show the value they were set to, fixed for the run.

The pages, each drawn from the control service's views, against frequency in a sweep or a step test
and against time (the last minute) in a dwell:

- **Control:** the control amplitude against the profile at the present level, with the alarm and
  abort bands, and the drive as played.
- **Limits:** each limit channel against its bound.
- **Responses:** every input's amplitude at the tone's frequency, and its phase against the drive.
- **Identification:** the identified gain with its trust floor, and the coherence with the
  frequencies the identification trusts.
- **Law:** the control law's values on its last call, and the run in numbers.

**Phase**, **Distortion** and **Time History** pages can be added. Pages can be added, renamed and
removed, and a page opens in a window of its own for another monitor; every window keeps Stop and
Abort, and Cmd or Ctrl and period aborts from any of them.

When nobody holds control, the run goes on; if no screen takes control within the timeout, it ends
with the controlled stop.

## How a Run Ends

| Outcome | Meaning |
|---|---|
| Completed | The test reached its end; the drive ramped down |
| Stopped by the operator | Stop: the drive ramped down |
| Aborted by the operator | Abort |
| Stopped: nobody held control | No screen held control for the timeout, or the service shut down; the drive ramped down |
| Aborted by a protection | The control point outside its abort band, the shaker's kinematics, a limit channel outside its abort band, the drive abort, a lost control point, an input's own watchdog, or a frequency the station cannot play; or the change limits holding the law back too long, which ramps the drive down as Stop does |
| Held at the coherence check | The identification's coherence was too low |
| Stopped: no usable drive from the control law | The law missed too many calls in a row, or could not be built; the drive in force was held through the misses, then ramped down |
| Stopped on a fault | The loop or the station failed |
