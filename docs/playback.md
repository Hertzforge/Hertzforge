# Playback

A Playback test plays a drive you made: volts at each drive output, one signal a drive, of any
length, on as many drives as the station has. Nothing closes a loop and nothing is analysed. The
run plays the file as it is, scaled by one level, records every enabled input and every drive as
played, and leaves a run folder that opens in the analysis side. Before anything is driven, the
control service checks the file: that it is whole, that it fits the channel table and the drive
ceiling, and that each drive starts and ends at rest.

This guide is written for the test engineer running the test. It follows the order of a run: the
station and channels, the drive file and its check, the level and the limits, the records, then
the run itself and what it saves. Playback shares its run bar, station panel and channel table with
the other test types.

## What the Test Does

1. **Convert.** The file you made is read on the computer that runs the test and written, once, to
   a drive folder: the samples in the one form the control service plays, beside a sidecar that
   says where they came from.
2. **Check.** The control service reads the drive folder and checks it against the setup and the
   channel table. A drive that fails is refused, with the channel and the numbers; when only its
   ends fail, Fix Ends can write a corrected copy for you to look at.
3. **Start.** The file is checked again, the station opens at the file's own rate, and the run
   records a pre-roll at zero drive.
4. **Play.** The file plays at the level, every drive at once, sample for sample, then a post-roll
   at zero drive is recorded. The run ends completed.
5. **End early.** You stop or abort the run, or a protection ends it (see Stop, Abort and
   Protections).

## Opening the Test

In Acquire & Control, choose **Playback** from the test types, under Specialized. The screen has
two steps, **Setup** and **Run**. The run bar (where the data comes from, the run's state, the
station time, the file's rate and the time played, who holds control, and Stop and Abort once a
run is live) stays in view on both. The summary beside every setup section says what the setup
holds, what the control service made of it, whether the drive passes its check, and what is still
missing.

**Proceed to Run** applies the setup (with any channel table edits first) and shows the run; the
control service checks the drive file again as it takes the setup. **Start** checks the file once
more, opens the station and starts the pre-roll, in one step.

**Save Setup**, **Open Setup** and **Open Setup From Run** keep and reopen the setup in the open
project, as on every test screen: the drive folder with its SHA-256, the map, the level, the
ceiling, the rolls and the records. A setup opened names the folder it was made with; the control
service refuses it if that folder's samples are no longer the ones the setup names.

## Station and Channels

The station panel and the channel table are shared with the other test types. Every enabled input
row (control, monitor or reference) is recorded, in table order. Every enabled drive row is a
place a file channel can play; a drive row that no channel plays on is driven at 0 V for the whole
run. The run needs at least one enabled input and one enabled drive.

Each input row's abort limits (acceleration, velocity, displacement) are the run's response
aborts. Set them in the channel table; Playback adds no column and no limit of its own.

## The Drive File

**Drive File** reads a file by its full path on the computer that runs the test (the file is
never uploaded), or the open project's time histories. **Read** lists the signals the file holds,
each with where it is in the file, its length, its rate and its unit when the file states them.
Tick the signals to play and put them in the order they play: the first is file channel 0, the
next file channel 1, and so on. **Convert** writes them to a new drive folder under `drives/` in
the program's local data folder and sets the setup to play it. A folder is never written over: a
second conversion makes another folder.

The forms read:

| Form | Files | Signals |
|---|---|---|
| Delimited text | `.csv`, `.tsv`, `.txt` and the like | Every column but a time column; a unit rides in the header, as `Drive (mV)` |
| MATLAB | `.mat`, version 5 and 7.3 | Every real numeric variable |
| NumPy | `.npy`, `.npz` | The array, or every array of the archive (never unpickled) |
| HDF5 | `.h5`, `.hdf5` | Every 1-D or 2-D numeric dataset |
| Hertzforge | `.hforge` | Each record of each time history |
| Universal file | `.unv`, `.uff` | Each record of each dataset 58 time function |
| Excel | `.xlsx` | Every column of every sheet |
| WAV | `.wav` | Each channel; integer samples at full scale are 1 V |
| The open project | | Each record of the time histories chosen |

**Units.** A drive is volts at the drive output. A stated voltage (V, mV) is converted to volts;
any other stated unit (g, m/s², N, counts) is refused by name. A signal whose unit the file does
not state is taken as volts, and the conversion says so.

**Rate.** The drive plays at the rate it was made at, and is never resampled. A rate the file
states (a time column, a rate or step variable, a WAV header) is used; a file that states none
asks for it. Every chosen signal must have the same length and the same rate. The station is asked
for the file's rate when the run starts; a file outside 1,000 to 204,800 samples a second is
refused at the check, and a station that cannot run the file's rate is refused at Start (see
Running the Test).

**Drive to Play** names the folder the setup plays and the SHA-256 of its samples, and maps each
file channel to a drive row: each channel on its own enabled drive row, no row twice. A new file's
channels are put on the free drive rows in table order; change any of them.

The drive folder's layout is in `docs/drive-file.md`.

## The Check

**Check** shows the control service's check of the drive as the setup stands, made again after
every edit, and the check the run holds once the setup is applied. Per channel:

- its peak, and its peak as played at the level;
- its first and last samples;
- its **velocity** and **displacement** at the end against their largest values over the record,
  with the drive taken as an acceleration and integrated as it stands, with no high-pass (velocity
  in V s, displacement in V s²);
- **Pass**, or each fault in words.

Under the table each drive has a small preview: the drive, its velocity and its displacement over
the whole record, each drawn as the band between its smallest and largest values.

The ends pass when the first and last samples are within 0.5 % of the channel's peak, and the
velocity and the displacement end within 1 % of their own peaks. A channel that is zero throughout
passes, and says so. The tolerances are fixed.

What each refusal means:

| The check says | What it means |
|---|---|
| The folder cannot be read; not a drive folder; a version it does not read | The folder is not there, or is not a drive folder this version plays. Convert the file again. |
| The samples are not the ones the sidecar states | `drive.bin` changed after it was written. Convert the file again. |
| The folder is not the one the setup names | The setup names samples by their SHA-256, and the folder now holds others. Convert again, or open the setup that goes with the folder. |
| A channel is not in volts; a value is not a number | The drive folder breaks its own layout. Convert the file again. |
| Ends not at rest | A channel starts or ends away from zero, or its velocity or displacement does not come back to rest. Fix the file, or use Fix Ends. |
| A channel for every drive row; a row missing or used twice; more channels than drives | The map does not fit the channel table. |
| Peak over the ceiling | A channel's peak at the level is over the drive ceiling. Lower the level, or raise the ceiling if the shaker and amplifier allow it. |
| Ceiling over the row's range | The ceiling is over a mapped drive row's range in the channel table. |
| A rate a station does not run | The file's rate is outside the rates a station is asked for. |
| No input, or no drive row | The channel table has nothing to record, or nowhere to play. |

Nothing is driven on a file that has not passed. The screen shows the check; the control service
refuses, whatever the screen shows.

## Fix Ends

When the only refusal is the ends, **Fix Ends** has the analysis side write a copy of the drive
whose ends are at rest, in a new folder beside it (the original is not changed):

1. a half-cosine taper at each end, by default 1 % of the record, at least 10 ms and at most 1 s,
   or the taper you give;
2. then a small correction, one half and one whole sine across the record (zero at both ends),
   chosen so the corrected record's velocity and displacement end at zero.

The table shows each channel before and after: its first and last samples, its peak, its velocity
and displacement at the end against their peaks, and its rms. **Play the Fixed Drive** sets the
setup to the new folder, which the control service then checks like any other; **Keep the
Original** leaves the setup as it was. A fix changes the drive the test plays: a long taper or a
large correction changes what the test asks of the shaker, so look at both before you choose. The
fix is kept in the new folder's sidecar.

## Level, Ceiling and Rolls

- **Level**, -40 to 0 dB (default -12): every sample of the file is multiplied by 10^(level/20).
  Each run plays at one level.
- **Drive ceiling**, volts: above 0, at most 10 V and at most each mapped drive row's range. Each
  channel's peak at the level must be within it, and every sample played is judged against it.
- **Pre-roll** and **post-roll**, 0 to 60 s each (defaults 0.5 s and 2 s): recorded at zero drive
  before and after the file. The pre-roll includes the zeros the station plays before anything
  else (a few frames on the simulator, a few blocks on hardware); a pre-roll shorter than those
  is as long as they are.

**Response Aborts** lists each enabled input row's abort limits as the channel table holds them.

## Run Records

**Records** holds the Run Record panel (who runs the test, on what article, to which document and
clause, notes, and whether the time histories are kept), the run's title, the run folder's name
(`playback` unless named) and the fields kept with the run.

## Running the Test

**Start** passes the channel table's arm gate, checks that the table still has the drive rows the
setup was checked with, computes the file's SHA-256 again, opens the run's record, reads the file
ahead into a buffer, opens the station at the file's rate and starts the clock at zero drive. Start
is refused, and nothing is driven, when:

- the file changed since its check;
- the channel table's drive rows changed since the setup was applied;
- the station programs another rate than the file's (the refusal names both, and the rate the
  station offers);
- the run's data folder cannot be written;
- the station refuses to open.

The run then plays its phases: **Pre-roll** (recording at zero drive), **Playing** (the file at
the level), **Post-roll** (recording at zero drive), and ends **Completed**. The Run page shows the
inputs and the drives as played, on the station's clock, about the last half second of each; the
Play card shows the phase, the time played of the file's length with the share played, how much of
the file is read ahead, the level, the ceiling, the rate and the drive rows. The length of a file
is limited only by the disk: the file is read from disk while it plays.

When the run has finished, the Run card shows how it ended, with **Open in Analysis**, which
converts the run folder and opens its inputs and drives in the analysis side's Signals workspace,
and **Copy Folder Path**. **New Run** plays the same setup again, after the same checks.

## Stop, Abort and Protections

- **Stop** is the controlled stop: the drive ramps down 40 dB over 3 s, multiplying the file as it
  plays, then goes to zero; the run ends "Stopped by the Operator" once a block of zeros after the
  ramp has played and the inputs have come back past it. During a roll the drive is already zero
  and the run ends as soon as the inputs have come back.
- **Abort** takes the drive to zero at once.
- Any connected screen may stop or abort; everything else needs control.
- These end the run with the drive at zero at once: an input row past one of its abort limits; an
  input overload on a row with abort limits; a sample over the drive ceiling; the file not read in
  time to play (the run ends "The Drive File Was Not Read in Time"); a failure reading the file; a
  fault of the control loop.

## What a Run Saves

Each run writes a run folder under the program's local data folder (`docs/run-folder.md`, "What a
Playback Run Saves"); when the name is taken, the run gets the next free one, `playback-2`, and so
on. The folder is begun before the station opens: a data folder that cannot be written refuses the
start. It holds:

| File | What | In the analysis side |
|---|---|---|
| `run.json` | The run's identity, title, operator, article, notes and specification, the fields kept with the run, the source (simulator or hardware), the station with its devices, the rate, the build and the start | Part of the **Run Record** |
| `table.json` | Every row of the channel table the run started with | **Channel Table** |
| `setup.json` | The setup as accepted; what it resolved to: the drive folder, its SHA-256, its rate, samples and channels with the row each plays on, its sidecar whole (the file it came from and any fix), the level and its gain, the ceiling, the rolls, the stop and every abort in force; the check the run was set up with; and the derived numbers | Part of the **Run Record**, and the setup Open Setup From Run opens |
| `log.jsonl` | The check, the arm, each phase as it began, a stop or abort and who asked, a trip, an underrun, an overload and the outcome | Part of the **Run Record** |
| `streams/inputs` | Every enabled input as read, in table order and in each row's unit, from the station's first frame until past the drive's last recorded frame by the station's input delay | **inputs**, a time history |
| `streams/drives` | Every drive as the device accepted it, in volts, frame for frame with the inputs: the pre-roll's zeros, the file at the level, then the post-roll, or a stop ramp and a block of zeros after it. The zeros the station plays after that, while the inputs come back, are not recorded | **drives**, a time history |
| `results/playback_check` | The check report, and per channel its numbers and its drive, velocity and displacement envelopes | Part of the **Run Record**, as named arrays |
| `results/playback_outcome` | How the run ended, the samples of the file played and the frames generated | Part of the **Run Record** |
| `end.json` | The end, the samples played of the file's, and the run's last state, with the control loop's health: its ticks, its worst tick's compute time against the tick's budget, its scheduling faults and ring overruns | Part of the **Run Record** |

With **Record time histories** off there is no `streams/` folder; the rest is always kept.

## What It Does Not Check

- The check takes the drive signal as an acceleration. It cannot know the motion of the table
  without a model of the shaker and its amplifier, so a drive that passes can still ask for
  displacement at low frequency.
- Stroke is not checked.
- Each run plays once, at one level: there are no repeats and no level schedule.
- The check's tolerances are fixed.
