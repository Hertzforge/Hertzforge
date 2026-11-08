# Classical Shock

Classical shock fires one pulse at a time, of a standard shape such as a half-sine, at a set peak
and duration, and closes the loop between pulses: after each pulse is measured and judged, the
control law proposes the next drive and the controller checks it before it plays. The drive is in
volts throughout, held under a drive ceiling and an abort that the operator gives.

This guide is written for the test engineer running the test. It follows the order of a run: the
station and channels, the pulse, the standard, the schedule, identification, control and limits,
trains and capture, then the run itself. Classical shock shares its run bar, station panel,
channel table and control-law panel with Random and MIMO Random, so an engineer who has run one
will find the others familiar.

## What the Test Does

A run goes through these steps in order:

1. **Arm.** The station opens, adopts its sample rate and passes its gate. Nothing is driven. When
   a shaker is given, the pulse is checked against its ratings first; a pulse over them blocks the
   run until the operator arms past it in their own words.
2. **Identification.** Band-limited random at a stated drive in volts rms, its peak held under the
   drive ceiling. The controller estimates how the system answers (its frequency response and
   coherence) on the pulse's own grid, and marks the lines it trusts. A mean coherence under the
   floor, or trusted lines that cover too little of the reference, holds the run before any pulse.
3. **Review.** With the review hold on, the run waits with the identified system on screen. Nothing
   fires until the operator proceeds.
4. **Seed.** The control law builds the first drive from the identified system. The controller
   predicts the full-level drive and its response; a drive predicted over the drive abort ends the
   run before anything fires.
5. **Pulses.** For each pulse: wait out the time between pulses; check the drive (its volts, its
   predicted peak and kinematics, at rest at both ends); fire; measure and judge it against the
   standard; let the law propose the next drive; check the law's drive; move the ladder.
6. **End.** The schedule completes, or the run stops, aborts, fails to converge, or the law gives
   no usable drive.

The law works at full level. Every drive it proposes is checked by the controller before it plays,
whichever law it is.

## Opening the Test

In Acquire & Control, choose **Classical Shock** from the test types. The screen has two steps,
**Setup** and **Run**. The run bar (where the data comes from, the run's state, elapsed time,
level, the pulses fired and passed, who holds control, and Stop and Abort once a run is live)
stays in view on both. The setup's sections are tabs; the summary beside every section says what
the setup holds, what the control service worked out from it, and what is still missing, each a
click from its section.

**Proceed to Run** applies the setup (with any channel table edits first) and shows the run.
**Start** arms if needed, then runs identification. The setup is checked as it is edited: every
problem shows in the section it belongs to, and a setup with errors is never accepted.

## Station and Channels

The station panel chooses the simulator or the NI hardware, as for Random. The run controls on the
channel table's first enabled **control** row and drives its first enabled **drive** row; the
section names both. Any other enabled row can be monitored (Limits).

## The Pulse

- **Shape:** half-sine, haversine, terminal-peak or initial-peak sawtooth, trapezoid (with its rise
  and fall as shares of the duration) or rectangle.
- **Peak** in g and **duration** in ms, at full level.
- **Direction:** positive or negative. The standards ask for both directions; negative turns the
  reference over and the pulse is judged on the same terms.
- **Compensation.** A shaker cannot end a pulse still moving. Shoulders before and after the main
  pulse take the velocity and the displacement back to zero. Their shape is rectangular, rounded
  rectangular or half-sine, and their length a multiple of the duration, the same both sides or
  set apart. The usual choice is rectangular shoulders four times the duration.

Beside the pulse, the control service draws it at full level: the reference with its tolerance
band, its velocity and displacement, the velocity change, the peak-to-peak displacement, and how
far the shoulders reach into the standard's quiet window. Shoulders that leave the quiet window are
a warning, not an error: the test may run, and longer, lower shoulders bring it inside.

## The Standard

- **Judged by** MIL-STD-810, Method 516 (amplitude within 15% of the peak around the reference,
  velocity change within 10%) or IEC 60068-2-27 (20% and 15%).
- **A failed pulse** aborts at full level only (below it, the run raises an alarm and the law
  corrects the next pulse), or at every level. A pulse that comes in low never aborts by itself;
  the limit on tries at one level ends a run that does not converge.
- **Overshoot abort:** a measured peak over the commanded peak by more than this share aborts at
  any level. Usual: twice the standard's amplitude tolerance.
- **Measurement:** the sample rate asked of the station (the run reports the rate it used), and each
  pulse's shock response spectrum (SRS), its band, bands per octave and an informational band about
  the reference's SRS. The SRS is shown on its own page; the run is judged on the pulse.

## The Schedule

- **Automatic** fires the schedule by itself, level by level. **Manual** waits for **Fire**; the
  operator steps the level, goes to a level of the table, and fires trains and bursts. The mode is
  fixed for the run.
- **Levels:** from the first level to full level, 0 dB, the last, each with its pulses.
- **When a level is done:** on passing pulses (the usual choice: the law keeps correcting until a
  level's pulses pass, and too many tries at one level end the run as not converged), or after its
  pulses, passed or not.
- **Time between pulses:** at least the largest of the cooling, settling and computing times,
  from the moment the station starts one pulse to the moment it starts the next. It also sets the
  control law's time budget, a share of it.

## Identification

The drive level in V rms (its peak is held under the drive ceiling), the length, the block length
of the estimate, the band, the mean coherence it must reach, and the review hold. With the review
hold on, the run waits after identification with the identified system on the Identification page;
**Proceed** in the run bar goes on.

## Control

The **control law** panel lists the laws of the classical world from the control service: the
built-in law first, then imported law files. It shows the chosen law's author, file hash, check
record and whether it may drive hardware, and its parameters, drawn from the law's own manifest.
**Import Law** reads a law file by its full path on the computer that runs the test. An imported law
runs on the simulator at once; it drives hardware only once its check has passed. The law and its
parameters are fixed for the run.

**What the correction trusts:** its band, the coherence floor (a line is corrected only where
identification measured it well), the inverse floor, the reference floor, and the share of the
reference's energy the trusted lines must cover.

**Guards** the controller holds every law to: the time budget of a call and the misses in a row that
stop the run (a miss fires the drive in force again), the run's pulse cap, the caps on the drive's
peak and on any line's rise from one pulse to the next at full level, the first drive's bound, and
the pulses in a row with a growing error that end the run.

## Limits

- **Drive ceiling** and **drive abort**, in volts peak at the amplifier's input. There is no default
  for either; give the values for your amplifier. A drive over the ceiling is scaled down to it,
  and the run says so. A drive predicted over the abort is never played: the run aborts first.
- **Peak acceleration ceiling:** a predicted response peak over it scales the pulse down before it
  fires.
- **Monitored channels:** any enabled row with a peak ceiling, checked after each pulse or predicted
  before it. A monitor over its ceiling aborts the run.
- **Shaker and moving mass:** the shaker record, the moving masses and the residual velocity and
  displacement the pulse may leave. Checked at arm; the shaker's ratings are also the run's
  kinematic aborts. Without them, feasibility is not evaluated and the run says so.

## Trains and Capture

On the manual ladder, a **train** fires a count of pulses at the present level, at least the time
between pulses apart (its first pulse too, from the pulse before it), and a **gunfire burst** fires
rounds at a rate. Each fires the drive in force,
with no law call inside it, and every pulse is judged. The capture can start when the run arms;
fields kept with the run (a run number, an operator) are listed here.

## The Run

The run bar carries the verbs that apply now: **Apply Setup**, **Arm**, **Start**, **Proceed** at the
review hold, **New Run** once a run has ended, and **Stop** and **Abort**, which any connected screen
may use. **Stop** lets the pulse in flight finish and fires nothing more; **Abort** takes the drive
down at once. On the manual ladder the bar adds **Fire**, a 1 dB step down and up, **Go to level**,
**Train** and **Burst**. A verb that cannot run says why when it is focused or hovered: "The next
pulse may fire in 0.6 s", "A pulse is playing", "At the last level of the table".

The level card beside the pages says what the run is doing in plain words ("Pulse 3 of 7 at
-6.0 dB", "Next pulse in 0.6 s", "Holding for review"), how the last pulse went ("Pulse 2 was 25%
low; correcting", "Pulse 4 passed, 1.9% error"), its commanded and measured peak, error, velocity
change and drive, its three verdicts (envelope, velocity change, substitution), and the predicted
full-level drive against the ceiling and the abort. When the run ends, the card says how, and for
a protection what tripped and on which pulse; a trip found on the prediction says that nothing was
driven for it.

The pages, each drawn from the control service's views:

- **Pulse:** the reference at the level fired with its tolerance band, the measured pulse and the
  drive played.
- **SRS:** the measured pulse's SRS against the reference's, with the informational band.
- **Kinematics:** velocity and displacement, measured and reference.
- **Identification:** the identified system's magnitude, and the coherence with the trusted lines
  marked; the pulse's spectra.
- **Pulses:** every pulse fired: where it came from, its level, drive, peak, error, velocity change,
  verdicts, result, SRS bands and the law call that made its drive. Pick pulses here (up to six) to
  lay them over the Pulse and SRS pages.
- **Law:** the control law's reported values on its last call, and the run in numbers.

Pages can be added, renamed and removed, and a page opens in a window of its own for another
monitor; every window keeps Stop and Abort, and Cmd or Ctrl and period aborts from any of them.

When nobody holds control, firing pauses at once. A screen that takes control within the timeout
resumes the run; otherwise it ends with a safe stop.

## How a Run Ends

| Outcome | Meaning |
|---|---|
| Completed | The schedule finished |
| Stopped by the operator | Stop: the pulse in flight finished, nothing more fired |
| Aborted by the operator | Abort |
| Stopped: nobody held control | No screen held control for the timeout, or the service shut down |
| Aborted by a protection | Tolerance, overshoot, kinematics, drive, peak acceleration, a monitor, or a growing error |
| Stopped: the pulse did not converge | Too many pulses at one level without a pass, or the run's pulse cap |
| Held at identification | The coherence, or the trusted lines' share of the reference, was too low |
| Stopped: no usable drive from the control law | The law missed too many calls in a row, or its first drive was refused |
