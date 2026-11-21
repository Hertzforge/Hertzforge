# Modal Test

A Modal Test measures frequency response functions (FRFs) at every point of a structure, for the
modal fit. The force comes from an impact hammer with a force gauge, or from one or more shakers
driven open loop. Each point is averaged: hits in impact mode, frames in shaker mode. The test
estimates each point's FRF and coherence as the averages come in, saves every hit or frame taken
with the reason it was kept or rejected, and leaves a run folder that opens in the analysis side,
in the modal fit. Before anything is armed, the control service checks the setup against the
channel table: the rate, the band, the estimator, the references and responses, and every point's
DOFs.

This guide is written for the test engineer running the test. It follows the order of a test: the
channels and DOFs, impact testing step by step, shaker testing, the windows, what the test checks
and what it does not, what a run saves, and opening the test in the fit. The Modal Test shares its
run bar, station panel and channel table with the other test types.

## What the Test Does

1. **Check.** The control service reads the setup and the channel table's DOFs and checks them
   together. A setup that fails is refused, with the field and the numbers; a warning is shown but
   does not stop the test.
2. **Start.** The station opens at the setup's rate. In impact mode no drive row is opened.
3. **Measure.** At each point, every hit (or every frame of the shakers' drive) is checked and
   counted. Each accepted one goes into the point's average, and the FRF and coherence are drawn
   as the average builds. When a point has its averages, its FRF and coherence are saved, and you
   move to the next point.
4. **End.** You end the test with **End Test** once the points you want are measured, or stop or
   abort it, or a protection ends it.

## Opening the Test

In Acquire & Control, choose **Modal Test** from the test types, under Specialized. The screen has
two steps, **Setup** and **Run**. The run bar stays in view on both, with the current point and
the points measured once the test is live. The summary beside every setup section says what the
setup holds, what the control service made of it (the rate, the block, the line spacing, the band,
the estimator, the plan and the trigger), every warning and refusal in the service's own words,
and what is still left to fix, each a click from its section.

**Proceed to Run** applies the setup, with any channel table edits first, and shows the Measure
screen. **Save Setup**, **Open Setup** and **Open Setup From Run** keep and reopen the setup in the
open project, as on every test screen.

## Channels and DOFs

The station panel and the channel table are shared with the other test types. Each measured row
needs a DOF: a node number and a direction, such as `3` and `Z+`. The **DOFs** section shows the
table's input rows with their node, direction and role, and edits them in the table itself; there
is no separate list of DOFs. Each row is one of:

- **Reference**: the force. In impact mode, the hammer's force gauge, which must be the one
  reference. In shaker mode, the force gauge of each shaker, one reference a shaker.
- **Response**: an accelerometer, or any other motion transducer.
- **Monitor**: recorded, but not used in the FRFs.
- **Not used**: not part of the test.

The units come from each row's unit in the table (N or lbf for a force, g or m/s² for an
acceleration), and the FRFs are saved in them. A row with no node or no direction is refused
before the test starts.

## Impact Step by Step

1. **Set up the channels.** In Station & Channels, add the hammer and the accelerometers, each
   with its unit and sensitivity. In DOFs, make the hammer row the reference and each
   accelerometer a response, and give each its node and direction.
2. **Choose the mode and the acquisition.** In Mode & Acquisition, choose **Impact**, the sample
   rate, the block (which sets the line spacing), the band, the estimator (H1, H2 or Hv) and the
   averages: the hits taken at each point.
3. **Plan the points.** In Roving, turn on **Rove over points** and choose the rows that move.
   Rove the hammer row for a roving hammer survey: the accelerometers stay put and the hammer
   visits every point. Rove the responses for a roving accelerometer survey. Give the points as a
   start, a step and a count, or typed or pasted, a node a line (a direction after the node, such
   as `4 Z+`, when one row roves). The plan lists every point's DOFs. With roving off, the test
   measures one point, with the DOFs as the table gives them.
4. **Set the trigger and the checks.** In Impact: the trigger level, as a percent of the hammer
   row's range; the pretrigger, the part of the block kept before the trigger; the holdoff, the
   blocks after a hit before the next can trigger; the double hit threshold; and the tip warning.
   Choose **Automatic** or **Manual** accept, and the windows (see Windows).
5. **Set a force target, if you want one.** **Flag each hit against a target peak** with a peak
   and a tolerance in percent. A hit outside it is flagged High or Low with its percent; it is
   never rejected for that.
6. **Proceed to Run, then Start.** The station opens and the counter shows "0 of 5" at the first
   point.
7. **Hit the structure.** Each hit is drawn: the force and the response in time, with the force
   window, and the force spectrum. Its tile says Accepted, Rejected or Waiting, with its peak, its
   flags and its reason. In automatic mode, an overload or a double hit is rejected and any other
   hit accepted; in manual mode, each hit waits for you.
8. **Accept or reject.** Press **Enter** or **A** to accept the latest hit, **Backspace** or **R**
   to reject it, **U** to undo the last accepted one. In automatic mode these override the
   service's verdict on the latest hit until the next one lands.
9. **Move on.** When the counter reaches "5 of 5", the point's FRF and coherence are saved. Press
   the **Right arrow** for the next point and the **Left arrow** for the previous one, or click a
   point in the list. With **Move to the next point when a point has its hits** on, the test moves
   on by itself. A point measured again replaces the earlier measurement of it; both are kept in
   the run folder.
10. **End the test.** **End Test** ends it once the points you want are measured. **Open in
    Analysis** then opens the run in the modal fit.

The keys work wherever the focus is, so you can keep the hammer in one hand. They never fire while
a text field has the focus, nor with Ctrl, Cmd or Alt held, nor on a held key's repeats; Enter and
Space on a focused button are left to the button. **Space** arms and disarms the trigger.

**Use This Hit as the Target** sets the force target from the latest hit's peak, when you have
landed one you like.

Every count on the screen (the counter, the tiles, the points measured) is the control service's
own: the screen keeps no count of its own. On the simulator, **Simulated Hit** makes the next hit
clean, double or overloaded, to try the checks without a hammer.

<!-- Depends on the control service sending the hit's own block and the force spectrum with each hit. -->
The force plot shows the latest hit's own block. The force spectrum shows how much of the band the
tip reaches: a spectrum that falls by more than the tip warning across the band is flagged, and a
harder tip reaches higher.

## Shaker

In shaker mode, each shaker plays a broadband source open loop, at its own level in V RMS, on its
own drive row. Choose **Shaker** in Mode & Acquisition. In the Shaker section, add a drive row for
each shaker (drive rows are added in Station & Channels), its level, the source and the drive
ceiling. On the Measure screen, **Start Driving** ramps the drive up and averages frames until the
point has its averages; the drive then ramps down, 40 dB over 3 s, and goes to zero. **Stop
Driving** stops it sooner.
<!-- Depends on the control service sending the reference autospectra in the live frame. -->
While the shakers drive, the Measure screen draws the forces, the responses and the drives in
time, the reference autospectra, and the FRF with its phase and coherence as the average builds.

| Source | What it plays | Analysis window | Several shakers |
|---|---|---|---|
| Random | A new random record every frame, overlapped by 0 or 50% | Hann | Yes |
| Burst Random | Random for part of each frame (the burst), then silence, so the response dies out inside the frame | None | Yes |
| Pseudo Random | One random frame, repeated | None | No |
| Periodic Random | A new periodic record each frame, played after the delay frames so the response is steady | None | Yes |
| Chirp (Periodic) | A sweep repeated every frame | None | No |
| Burst Chirp | A sweep for part of each frame, then silence | None | No |

A random source is clipped at a crest of 4, so no sample exceeds four times its RMS level; a
chirp's crest is a sine's, √2. The check multiplies each level by the source's crest and refuses a
level that would reach past the drive ceiling. It refuses a ceiling over the drive row's range.

With several shakers the test refuses:

- **Pseudo random.** It repeats one frame, so every average sees the same forces. A repeated
  pseudo-random frame gives a rank-one force matrix, which cannot be inverted to separate the
  shakers.
- **A chirp.** The chirps drive one shaker only in this version.
- **Too few averages.** The averages must be at least twice the number of shakers.
- **H2 or Hv.** With several references the matrix H1 is used.

## Windows

**The force window**, on the hammer's force only, holds one over the pretrigger and the pulse,
then falls to zero by a half-cosine and stays zero for the rest of the block. Set its width as a
percent of the block after the trigger, and its fall as a percent of the width. A width of 5 to
10% is usual. It keeps the pulse and clears the noise after it. It does not cure a double hit: the
second impact lies inside the pulse's own time, and a window short enough to cut it would cut the
pulse too. A double hit is rejected and the hit taken again. A hit whose pulse has not fallen
before the window ends is flagged.

**The exponential window**, w(t) = exp(-t/τ), goes on every channel, the force and the responses
alike. Set it by the value left at the block's end. It makes a lightly damped response die out
inside the block, at a price: it adds damping. Each mode's measured damping ratio is high by
1/(2π f_n τ), where f_n is the mode's natural frequency in Hz. The check warns with the amount at
half the band's top frequency, and the run records τ, so the fit can take it out.

The correction (the fitted damping ratio less 1/(2π f_n τ)) is for light damping. It can be badly
off when the damping is very small and poorly estimated, since the window's share is then most of
what was fitted. A longer block, which captures more of the decay, is better than correcting for a
short τ.

In shaker mode there is no force or exponential window: the random source uses a Hann window, and
the others need none.

## What It Checks

Each hit is checked before it is counted:

- **Overload.** Any channel at 98% of its range or more, with the channel named.
- **Double hit.** A second force over the double hit threshold, as a percent of the peak, after
  the first pulse has fallen below 5% of its peak.
- **No response.** A response that does not rise well above its level before the trigger is
  flagged. Without a pretrigger this is not judged, and the check warns.
- **Tip.** The force spectrum's drop from its largest value to the band's top line, against the
  tip warning, is flagged.
- **Force target.** A peak outside the target is flagged High or Low, with its percent. It never
  rejects.
- **Force window.** A pulse that has not fallen before the force window ends is flagged.

In automatic mode an overload or a double hit is rejected as it lands; in manual mode every hit
waits for you, with these on its tile. The others are flags only, never rejections. The flags are
shown as words, and in color. The setup's check also warns when there is no roving plan, when
there is one average (the coherence is then 1 at every line and says nothing), and with the
exponential window's added damping.

## What It Does Not Check

- Whether the structure is linear and reciprocal. A roving hammer survey's FRFs are turned round
  for the fit by reciprocity, which assumes it.
- Whether the DOFs are where the table says they are, or point the way it says.
- Whether the hammer tip suits the band, beyond the tip warning.
- The shakers' attachment, or a force gauge's alignment with the shaker.
- In shaker mode, there is no control loop: each drive plays at its level, whatever the response.

## Stop, Abort and Protections

- **End Test** ends the test; in shaker mode, through the stop ramp.
- **Stop** and **Abort** are on the run bar, as on every test screen. Abort takes any drive to zero
  at once.
- In shaker mode, an input row past one of its abort limits, an input overload on a row with abort
  limits, a sample over the drive ceiling, a drive not made in time and a failure of the drive
  source end the run with the drive at zero.

## What a Run Saves

Each run writes a run folder under the program's local data folder: the setup as accepted with its
check, the channel table with every DOF, the inputs as the station read them (in shaker mode the
drives as played too), a line for every hit or frame taken and each change of its status, each
point's FRFs, coherence and reference autospectra as it is measured, and how the test ended. The
files and their fields are in `docs/run-folder.md`, "What a Modal Test Run Saves"; what each
becomes in a project is in `docs/modal-test-analysis.md`. A test cut off part way keeps every point
finished before it.

## Opening the Test in the Fit

**Open in Analysis**, once the test has ended, opens the run in the analysis side's modal fit. The
FRFs are there with their DOFs, coherence and windows, and each measurement's accepted frames as a
time history. A roving hammer survey's FRFs are also turned round by reciprocity into the fixed
reference form a modal fit needs, each record marked so. A fit of FRFs taken with the exponential
window notes the added damping on each mode (see `docs/modal-test-analysis.md`, "The Exponential
Window's Damping").

**Estimate Again** estimates one measurement's FRFs and coherence again from its saved frames,
with another estimator, other windows, or other frames. The results are added beside the frames;
the run's own FRFs are not changed.

## References

- Trethewey, M. W., and Cafeo, J. A. (1992). "Tutorial: Signal Processing Aspects of Structural
  Impact Testing." *International Journal of Analytical and Experimental Modal Analysis* 7(2),
  129-149. The exponential window (equation 22) and the damping correction (equation 27), with
  its caution for very light damping.
- Fladung, W. A. (1997). "Windows Used for Impact Testing." *Proceedings of IMAC XV*. The force
  window of 5 to 10% with a cosine taper, and the exponential window applied to the force and the
  response alike.
- Avitabile, P. (2002). "Modal Space: Which window is most appropriate for the different types
  of modal tests performed?" *Experimental Techniques*, February 2002. The force window of about
  10%, never to be used to remove a double impact; burst random, of 50 to 80% of the frame,
  needing no window.
