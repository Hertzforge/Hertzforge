# MIMO Mixed Mode

MIMO Mixed Mode plays sine tones and narrowband random on a random floor, on several shakers
controlled together. It is Mixed Mode on more than one drive: the floor is MIMO Random's, a full
spectral density matrix across the control points; each tone has its own amplitude at every control
and, when you state them, its own phases between the controls; each narrowband is the floor raised,
or a density added, in a band that stands still, sweeps or follows another part. Every part is judged
against its own alarm and abort bands at every control, and one level schedule moves every part by
the same dB. The drives are shared: the floor and every tone play through the same drives, and each
drive's peak ceiling is a budget they share.

This guide is for a test engineer who has run single-output Mixed Mode and MIMO Random. It covers
what the test holds, how to set one up, the parts and the five laws that run them, the shared drive
budget and what the screen means by "limiting", what the floor's law is shown on the lines the parts
occupy, what the setup refuses and why, and what is not built yet. It shares its station panel,
channel table, run bar and control-law panels with the other test types, its floor, identification
and gate with MIMO Random (see [mimo-random.md](mimo-random.md)), its tones' law with MIMO Sine (see
[mimo-sine.md](mimo-sine.md)), and its parts, schedule, hold and part switches with Mixed Mode (see
[mixed.md](mixed.md)).

Control laws for this test are an open topic. Nobody has settled how a floor law, a tone law and the
servo of a narrowband should share several drives, and the module is built for trying laws: the floor
and the tones each take a law from the catalog or a law file, and the three rules that join them are
choices on the setup, not fixed.

## What the Test Holds

- **A floor.** MIMO Random's reference: one spectral density per control at full level, and one
  coherence and phase between every pair of controls, from breakpoints or from a reference file. Its
  band, from the lowest breakpoint to the highest, is the test's band.
- **Tones.** Up to 12. Each stands still, sweeps or follows another part, as in Mixed Mode. Each has
  a peak amplitude at every control, and either its phases at every control against the first, or
  none, when the tones' law chooses them.
- **Narrowbands.** Up to 12. Each has a width (in hertz or as a share of its center) and a
  statement at every control: either a **raise**, the floor lifted by so many dB in the band, or an
  **added** density, with one coherence between the controls.
- **Drives.** Up to 8. Each has an rms ceiling on its random part and a peak ceiling on its summed
  drive, in volts, and may have an rms abort of its own. There is no default for either ceiling:
  they are your amplifier's input.

The first version runs up to 16 controls, 8 drives and 8 limit channels.

## What a Run Does

1. **Arm.** The station opens with every drive row as an output and every control and limit row as
   an input, on one clock. Nothing is driven.
2. **Identification.** MIMO Random's: independent random on every drive at a level under the
   reference, giving the model from every drive to every control, each control's multiple coherence
   and the model's condition across the band.
3. **The gate.** The worst multiple coherence is checked against its floor and the model's condition
   against its limit. A gate that does not pass holds the run.
4. **The prediction.** From the model, the service forecasts what each drive needs at full level for
   the floor and every tone together, against its ceilings. A forecast that cannot fit holds the run.
5. **Review.** The run waits with the identification, the gate and the prediction on screen. With
   the gate passed and the forecast in budget, **Proceed** goes on. Past a failed gate or a forecast
   over a ceiling, only **Proceed With Your Words** goes on, and your words go in the log; or abort.
6. **The floor alone.** The floor engages alone at the first level and plays until it has equalized.
7. **The parts come in.** Parts time starts, and each tone and narrowband whose window is open and
   that is switched on ramps in over the part ramp.
8. **Each loop.** Each tone is tracked at every control and taken out of the control signal; the
   floor's law is called on what is left; each narrowband is servoed; the tones' law is called for
   every tone; the ceiling rule fits the sum to each drive's budget; every part is judged.
9. **End.** The schedule finishes and every part ramps down together, or the run is stopped, aborted
   or held, and says why in plain words.

## Setting One Up

Open Acquire & Control and choose **MIMO Mixed Mode** under MIMO. The screen opens on Setup, in
sections, with a summary beside every section that lists what is left to fix and every refusal and
warning the control service gives, by name. The setup is checked by the control service as you edit
(a dry run that changes nothing). **Proceed to Run** applies the setup and shows the run.

### Station and Channels

The station and channel table are shared with every test type. Every enabled control row is a
control unless you switch it off here; every enabled drive row is a drive, in the table's order. Each
drive takes its rms ceiling, its peak ceiling (at least the rms ceiling) and, if you want one, its rms
abort.

### Floor Reference

As MIMO Random's: a breakpoint table for each control, with one coherence and phase for every pair,
or a reference file. The reference the service builds is drawn under the tables, and a reference the
service had to repair is used only when you accept the repair.

### Parts

The tones' table and the narrowbands' table, each row a part with its id, name, switch, motion and
window, and under it a row for every control:

- **A tone:** its peak amplitude in g at that control and, when **Phases stated** is on, its phase
  in degrees against the first control. A tone's amplitude is a constant at each control; the
  amplitude-against-frequency profile of single-output Mixed Mode is not offered here yet.
- **A narrowband:** its raise in dB at that control, or its added density in g²/Hz, with one
  coherence for the band.

Beside the tables are the tracking bandwidth (usual 0.5 Hz) and the width left out of the floor's
average around each standing tone (usual 2 Hz). The panel says how late the tracker reads each tone
at the bandwidth given (see "What Is Not Built").

### Schedule and Identification

The level schedule is Mixed Mode's: levels in dB under full level, each held for its dwell, then
full level for its time, in parts time. Identification takes its level, its averages, the
coherence floor and the condition limit of the gate, and whether the run holds for review.

### Laws

Five choices, each with what it means beside it.

- **The floor's law** is chosen from the service's list of laws in the MIMO world, the same list
  MIMO Random offers, with its parameter form. Nothing in the screen names a law: the list is the
  service's.
- **The tones' law** is chosen from the service's list in the MIMO sine world, as MIMO Sine offers,
  with its parameter form. It decides each tone's complex amplitude on every drive.
- **The band law:** **Automatic** (one servo per band for a raise, one per control for an added
  density), **One Servo per Band** (one gain at every control, keeping the floor's shape between
  them), or **One Servo per Control**.
- **The ceiling rule,** for a drive out of room (see "The Shared Drive Budget"): **Common Scale** or
  **In Turn**.
- **The fill rule,** for the lines a part occupies (see "What the Floor's Law Sees Under a Part"):
  **No Error** or **Fill From the Model**.

### Limits and Protections

Limit channels with their ceilings and abort margins, the floor's envelope and divergence aborts,
the overload share, the loop's ramps and guards, and the run record's words.

### Prediction and Budget

Before identification there is no model, so this section says the prediction comes after it. Once
the run has identified, the prediction shows each drive's need against its ceilings as a bar, the
peak need of the floor and the tones together against the peak ceiling, and each control's
predicted levels. Refusals and warnings are listed here by name, each with the part, control or
drive it concerns.

## The Shared Drive Budget

Every drive plays the floor and every tone at once, so each drive's peak ceiling is one budget for
all of them: the floor's peak plus every tone's amplitude on that drive. When a drive would go over its peak ceiling, the ceiling rule decides what
gives:

- **Common Scale.** Every tone is scaled down by the same factor, so the tones keep their
  proportions between themselves and between the controls. The run card shows the factor ("Tones at
  86 %").
- **In Turn.** Each tone is judged in order against what the others already play, as single-output
  Mixed Mode does. The last tones lose first, and the outcome depends on the order of the tones.

The floor is never scaled for the tones. While a tone is scaled down, the run is **limiting**: the
header says Limiting, the parts table marks every tone held under its target, and the drives card
shows each drive's need against its ceiling and the common scale. A limiting tone is under its
target by the factor shown, and its error at every control shows it. Limiting is not a fault: it is the budget doing its work,
and the log says each time a drive runs out of room.

## What the Floor's Law Sees Under a Part

A tone or a servoed narrowband occupies the floor's lines in its band. The floor's law is not
shown those lines as measured, because it would chase the part. The fill rule says what it is
shown instead:

- **No Error.** On every occupied line, the floor's law is shown the reference as if measured. It
  sees no error there, and its drive on those lines stays where it was when the part came in.
- **Fill From the Model.** On those lines, the floor's law is shown what the model predicts the
  floor alone would read from the drive in force. Its drive there keeps moving with the model.

## What Is Refused and Why

The service refuses a setup it cannot run, by name, with the part, control or drive it concerns. The
names you may see:

| Refusal | Why |
|---|---|
| Part outside the band | A tone or narrowband leaves the floor's band somewhere in its motion |
| Parts too close | Two tones come within two tracking widths of each other |
| Too many tones, too many narrowbands, controls, drives or limits | Over the first version's counts |
| Two parts with one id, follows an unknown part, follows itself, followers in a loop | The parts list does not hold together |
| Wrong number of values per control | A per-control list does not match the controls |
| Band not additive | A narrowband would take from the floor: a raise under 0 dB, or an added density that is not a valid spectral matrix at some control or line |
| Band as a full matrix: not built | A narrowband stated as a full matrix between the controls |
| Tone law refuses the tone | The chosen tones' law cannot run a tone as stated (for example, phases left to a law that cannot choose them) |
| Budget cannot fit | The forecast needs more than a drive's peak ceiling with every tone at its target |
| Model update: not built | The setup asks for the model to be updated during the run |
| Unknown band law, ceiling rule or fill rule; law not in the catalog | A choice the service does not know |
| Reference not accepted | The floor's reference cannot be used |

A forecast over budget is said before the run and again at review: you may still proceed with your
words, and the ceiling rule then holds the tones down for the whole run.

A sweep faster than the tracker follows is a warning, not a refusal: the part's row says how fast it
moves and how late the tracker reads it.

## What Is Not Built

- **Updating the model during the run.** The model from identification is held for the whole run;
  a setup that asks for it to be updated is refused as `frf_update_not_built`.
- **Narrowbands that take from the floor, and narrowbands as a full matrix.** A narrowband must add
  to the floor at every control; a band stated as a full matrix between the controls is refused.
- **Tones that sweep faster than the tracker can follow.** Each tone is read through a tracking
  filter, and at the default 0.5 Hz bandwidth the tracker reads the tone about 1.25 s late (the delay
  is about 0.625 divided by the bandwidth). Through a resonance a tone's response changes faster than
  that: a sweep of a few Hz/s moves several hertz in 1.25 s, more than the width of a lightly damped
  resonance, so the tracker loses the tone and the run aborts. Sweeps of a few Hz/s through a
  resonance are beyond it. A wider tracking bandwidth shortens the delay but lets more of the floor
  into each tone's reading.
- **A tone amplitude profile against frequency.** Each tone's amplitude is constant at each control.

## The Run Screen

The run card carries the verbs that apply now: **Arm**, **Start**, **Proceed** at review, **Proceed
With Your Words** past a failed gate or a forecast over a ceiling, **Hold** and **Resume**, the level
steps in the manual schedule, **Stop** and **Abort**. Every part's switch is in the parts table; the
floor has none.

- **The parts table:** for every part its id, kind, switch and frequency, whether it is held, whether
  it is limiting, and at every control its target, its measured value, the error in dB and its
  alarm.
- **The drives card:** for every drive the random part's rms, the held peak, the tones' sum,
  the headroom left under its peak ceiling, and the common scale.
- **The pages:** the control spectra, the residual with the tones taken out and the cross terms,
  each with its alarm and abort lines; the drives; each tone's amplitude and phase history at every
  control; each narrowband's level; the identification and its coherence; and the laws' values.
- **The log:** every state, level, hold, part switch, limit, alarm and trip, with your words.

## What a Run Saves

<!-- PLACEHOLDER FOR THE LEAD: the run folder's contents and the Open in Analysis table, once the run record is settled. -->

**Placeholder:** what a MIMO Mixed Mode run saves is to be written here, once the run record is
settled.
