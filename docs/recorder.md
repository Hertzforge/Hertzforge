# Recorder

A Recorder test records the station's inputs and drives nothing. The run opens the station with no
outputs, acquires every enabled input row of the channel table, shows each row's level, time history
and spectrum live, and saves recordings of those rows: from the run's first frame, when the operator
asks, or each time a row crosses a level. Monitoring saves nothing; only a recording does.

This guide is written for the test engineer running the test. It follows the order of a run: the
station and channels, the rate and the trigger, the analysis, the records, then the run itself and
what a recording holds. The Recorder shares its run bar, station panel and channel table with the
other test types.

## What the Test Does

1. **Start.** The station opens at the rate asked, with no outputs, and the run adopts the rate it
   programs. Acquisition starts at station frame 0. In "From the Start" mode the first recording
   begins with that frame.
2. **Monitoring.** The service reads the station in batches of two blocks (512 frames, 40 ms, at
   12,800 Hz on the NI station) and, between batches, shows every recorded row's rms, peak, time
   history and spectrum, and the FRF of one pair of rows when the setup names one.
3. **Recording.** A recording is a folder of its own in the run folder: the samples of every
   recorded row, at the rate the station runs at, and the recording's facts. Recordings are written
   by the run's own record thread, never by the thread that reads the station.
4. **End.** The operator stops or aborts the run, or the run ends because it cannot write, the disk
   falls behind, the station fails, or nobody holds control. Every end finishes every recording with
   every frame acquired; nothing is discarded.

## Opening the Test

In Acquire & Control, choose **Recorder** from the test types, under Data Acquisition. The screen has
two steps, **Setup** and **Run**. The run bar (where the data comes from, the run's state, the station
time, the rate, the recording in progress, the recordings saved, who holds control, and Stop and Abort
once a run is live) stays in view on both. The summary beside every setup section says what the setup
holds, what the control service worked out from it, and what is still missing.

**Proceed to Run** applies the setup (with any channel table edits first) and shows the run.
**Start** opens the station and starts acquisition, in one step.

## Station and Channels

The station panel chooses the simulator or the NI hardware, as for the other test types.

- **Recorded rows.** Every enabled input row (control, monitor or reference) is recorded, in table
  order, and each recording holds one column per row in that order. At least one is needed.
- **Drive rows** of the table are not opened and not recorded. The setup warns that they are present
  and nothing else changes, so the one table serves the drive tests and the Recorder alike. The
  service states the outputs as "none", and the run's log says which drive rows it left alone.
- **Abort limits** do not stop a Recorder run. A row passing one is marked and recording goes on (see
  "Marks and the Overload Indicator").

**On the simulator.** The simulator's test setup moves only when something drives it, and a Recorder
drives nothing, so a Recorder run on the simulator carries a stated source on its inputs, on the
simulator's small noise, in each row's unit, repeating every 2.000 s from the run's first frame. The
screen names it whenever the run is on the simulator:

| Recorded row | Signal |
|---|---|
| 1st | Markers: a half-sine of 0.5 peak lasting 10 ms, beginning 0.5 s into every 2.000 s |
| 2nd | 250 Hz, 0.2 peak |
| 3rd | 433 Hz, 0.1 peak |
| 4th | 1000 Hz, 0.15 peak |

A fifth recorded row and after repeat the four. Nothing on the screen changes the source.

## The Rate

**Rate asked of the station:** 12,800 Hz until you change it, from 1000 to 204,800 Hz. A station may
program a nearby rate instead: an NI input module runs only at the rates its clock divides to, so on
a module whose rates are 51,200 Hz over a whole number, 8192 Hz asked runs at 8533.33 Hz. The run
adopts the rate the station programs, records at it, derives every number from it, and shows both:
the run bar and the card read the rate the station runs at, with the rate asked beside it when they
differ, and every recording states both. A recording is always at the station's rate, whatever the
analysis does.

## The Trigger

**Starts a recording** chooses the mode:

| Mode | What starts a recording | What ends it |
|---|---|---|
| **From the Start** | The run's first frame (station frame 0), with no pre-window | Its length, or Cut, or the run's end |
| **Manual** | **Record**; armed, the recording begins with the pre-window it holds | Its length, or Cut |
| **Level** | A row crossing a level, with the pre-window before it | Its length (required) |

- **Pre-window:** the frames before the trigger a recording begins with, 0 to 10 s (usually 1 s). The
  pre-window fills once the run is armed, from the next batch on, and holds at most its length.
- **Length after the trigger:** each recording holds exactly this after its trigger, plus its
  pre-window, to the frame: at 12,800 Hz a 1.0 s length with a 0.5 s pre-window is 19,200 frames. In
  "From the Start" and "Manual" modes it may be left open ("Until cut"): the recording then runs until
  **Cut** or the run's end.

**A level trigger** needs a row, a level and a length:

- **Row:** a recorded row; the level is read on it.
- **Level:** in the row's unit, within its range either side of zero.
- **Slope:** **Rising** fires on the first sample at or over the level; **Falling** on the first at or
  under it. That sample is the recording's trigger.
- **Hysteresis:** before the detector arms, the signal must have been below the level less the
  hysteresis (rising) or above the level plus it (falling). Usually 1% of the row's range: at a range
  of 5 g, 0.05 g, so a 0.25 g rising trigger arms only after the signal has been under 0.20 g. A
  signal hovering at the level fires at most once.
- **Recordings an arm saves:** the count. After each recording the trigger re-arms by itself until
  it has saved this many since **Arm**, then the run goes back to monitoring. "Until the run ends"
  keeps it re-arming.
- **Holdoff:** the least time from one trigger to the next, counted in station time from the
  trigger. A re-armed trigger also waits for a full pre-window of frames acquired after the last
  recording ended, so no frame is ever in two recordings. With markers 2 s apart and a 3 s holdoff,
  the marker 2 s after a trigger is skipped and the one 4 s after it fires.
- The first trigger after **Arm** may come before the pre-window has filled. The recording then
  holds the pre-window it had, and says so (its pre-window held less than asked).

## The Analysis

The live spectra and FRF; nothing here changes a recording.

- **Block:** the analysis block, frames at the analysis rate, a power of two from 256 to 65,536
  (usually 2048). The line spacing is the analysis rate over the block, and the lines run to the
  analysis rate over 2.56.
- **Window:** Hann, flat top, rectangular or Blackman-Harris; **Overlap:** the window's own (Hann 50%,
  flat top 75%, rectangular 0%, Blackman-Harris 66.7%) unless you give one, 0 to 95%.
- **Averaging:** linear, exponential or peak hold; **Averages:** the blocks in a linear average, or
  the time constant in blocks of an exponential one.
- **Decimation:** a power of two from 1 to 256. The analysis rate is the station's rate over it.
- **FRF:** one pair of recorded rows, a reference and a response, for a live H1 with its phase and the
  coherence.

At 12,800 Hz with a block of 2048, Hann and decimation 1: lines every 6.25 Hz, 800 of them, to
5000 Hz; 8 averages are complete 0.72 s after start, 16 after 1.36 s. The summary shows the
service's own numbers for the setup as it stands.

## Run Records

- **Title:** the run's name.
- **Run folder:** a plain name, under the program's local data folder (the folder named by
  `HERTZFORGE_DATA_ROOT`, else `Hertzforge_Data` in your home folder). A name already taken gets the
  first free `<name>-<n>`, so no run writes over another. The run's log names the folder at start, and
  the card names it at the end.
- **Keep free on the data volume:** the free space the run leaves, in GiB, at least 1. With a timed
  length the floor is also at least twice one recording's bytes. A start with less free space than
  the floor is refused, naming both numbers, and makes nothing.
- **Kept with the run:** fields you name, written with the run.

## Running the Test

**Start** answers once acquisition runs. The card says "Monitoring: nothing is being saved", or in
"From the Start" mode that the first recording is being saved.

The run bar keeps the Recorder's own verbs beside **Stop** and **Abort**, as the mode offers them:

- **Arm** (Manual and Level): the pre-window starts filling, and in Level mode the detector waits.
- **Disarm:** back to monitoring; the pre-window is dropped.
- **Record** (Manual): a recording begins at the first frame of the next batch, with the pre-window
  held when armed and none when not. The answer names the recording and its trigger's station frame.
- **Cut** (From the Start and Manual): the recording in progress ends with the batch the request is
  taken in. The answer names its last station frame.

A verb that cannot run says why. Every verb but Stop and Abort needs control.

The **card** says what the run is doing, in plain words:

- "Monitoring: nothing is being saved"
- "Armed: waiting for row 1 to rise through 0.250 g (pre-window 0.50 s of 0.50 s)", the pre-window
  held of the pre-window asked; "holding off" while a holdoff runs
- "Recording rec_0002: 0.42 s of 1.00 s", the seconds after the trigger of the length
- "Saved rec_0002: 1.50 s, no gaps", the last recording saved
- "Writing 0.3 s behind the inputs", when the disk is more than a quarter second behind
- "Could not write: <the fault's words>. rec_0003 holds 48.2 s."
- "Stopped: 6 recordings in <folder>"

Under it: the rate the station runs at, the station time, the recordings saved, the marks, the
triggers fired and how many of the count are left, the time of recording left above the free-space
floor, the group delay, the backlog, and each recorded row's rms over the last second, its peak
since start, its marks and its overload indicator.

The pages:

- **Time:** every recorded row's last two seconds on the station's clock, drawn as each interval's
  largest and smallest sample, so a short event is never lost between points. Limit marks are drawn
  as thin lines where they fell.
- **Spectra:** every recorded row's PSD, the average so far.
- **FRF:** the pair's magnitude, phase and coherence, when the setup names a pair.
- **Readings:** the run in numbers, and every mark in words.
- **Recordings:** each recording's folder and its facts, and the one being written with its frames on
  disk.

Pages can be added, renamed and opened in a window of their own for another monitor; that window
shows the run and keeps Stop and Abort.

## Time Zero, the Pre-Window and the Group Delay

Time is the station's. Every frame number the Recorder shows or writes is a **station frame**, counted
from the run's first frame (station frame 0) at the rate the station programmed: the station time on
the run bar, the answers to Record and Cut, each recording's first frame and trigger, the marks, and
the time plots. Nothing is timed by when a frame reached the computer. The only number on the
service's own clock is the countdown with nobody in control.

In a recording, **t = 0 is the trigger**: the file frame at the pre-window's end. A frame's time from
the trigger is its file frame less the pre-window held, over the rate. Its station frame is the
recording's first station frame plus its file frame (plus any frames missing before it).

**The group delay.** A station's inputs pass through filters that delay the signal by a fixed number
of frames. On the NI input path it is 38.4 frames plus 3.2 millionths of the rate: 38.44 frames
(3.00 ms) at 12,800 Hz, 38.56 at 51,200 Hz; on the simulator it is 0. An event that reaches the input at
station time t is recorded at frame t times the rate plus the group delay. Every recording states the
group delay, so its t = 0 can be placed against another instrument's clock.

## Marks and the Overload Indicator

A Recorder drives nothing, so stopping it protects nothing and would lose the event. A row's **abort
limits are marks**: acceleration is checked on every sample of a row in g, velocity and displacement
on the row's derived motion. A row passing a limit is marked at the station frame of its first sample
past it, with the value and the limit, in the log, the marks list, and the recording it fell in; the
same row and limit is marked again only after it has stayed within the limit for one second of station
time. Recording goes on.

Each row also has an **overload indicator**: it latches the first time a sample's magnitude reaches 0.9
of the row's range, holds for the rest of the run, and is marked once. The card shows it on the row,
as a shape and the word "Overload". The station's own overload words, where it says any, are in the
log.

## What Ends a Run

- **Stop** and **Abort** end acquisition at the next batch and finish every recording with every
  frame acquired. Only the outcome differs: "Stopped by the Operator" or "Aborted by the Operator".
  Any connected screen may stop or abort.
- **A write that fails** (the disk refuses it, an I/O error, or the free space falls under the floor)
  ends the run "Stopped: A Write Failed", with the fault's own words. The recording is finished with what
  reached the disk and names the fault.
- **A disk that falls behind.** The thread that reads the station hands each batch to the record
  thread through a queue that holds 2 s of frames. A disk so far behind that a batch cannot be queued
  ends the recording there, with the missing frames recorded as a gap, and the run "Stopped: The
  Disk Fell Behind". The card shows the backlog while it builds.
- **The station:** an input buffer overflow, a lost device, an input stall or a failure of the
  service's loop ends the run on a fault, in the station's words, every recording finished.
- **Nobody in control:** the run goes on unchanged, recordings included, for the timeout shown in the
  run bar's countdown, then stops with every recording finished. A service shutting down does the
  same.

## What a Recording Holds

The run folder:

```
<run folder>/run.json
             log.jsonl
             marks.jsonl
             end.json
             recordings/rec_0001/telemetry.f32
                                 capture.json
                                 recorder.json
```

- `run.json`, written at start: the configuration as sent, the station, the rate asked and the rate
  programmed, the group delay, the outputs ("none"), each recorded row (its index, description, role,
  unit and range) in column order, the service's derived numbers at the programmed rate, and the
  overload indicator's definition.
- `log.jsonl` and `marks.jsonl`: a line for every log entry and every mark.
- `end.json`, written when the run finishes: how it ended, the frames acquired, and each recorded
  row's mean, rms, peak, crest factor and kurtosis over the whole run.
- `recordings/rec_NNNN/`, numbered from `rec_0001` in the order they began:
  - `telemetry.f32`: no header; little-endian 32-bit floats, one frame after another, each frame one
    value per recorded row in the run's row order, in the row's unit, at the rate the station
    programmed. The file's size is the authority on its length: its frames are its size over 4 times
    the rows.
  - `capture.json`: the writer's own record: the channel table at capture time, the programmed rate,
    the frames written, the chunks dropped, the flushes, the free-space floor, whether it is complete
    and any fault.
  - `recorder.json`: the recording's facts: its name and trigger mode, the rate asked and programmed,
    the group delay, the rows and their units, its first station frame and its trigger's, the
    pre-window held and asked, the frames written, every gap (where in the file and how many frames),
    the marks inside it, any fault, and `complete`.

At 12,800 Hz four rows write 204,800 bytes a second, about 737 MB an hour. The record thread flushes
and syncs the data every 5 s, so a crash of the computer or the service loses at most the last 5 s. A
recording found with `complete` false was cut short that way; its file holds what reached the disk.
