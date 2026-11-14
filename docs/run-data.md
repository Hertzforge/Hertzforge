# Run Data

A test on a shaker is worth what it leaves behind. When a run ends, the channel table it ran with,
its setup, its limits, what it measured and what it decided should arrive on the analysis side as
ordinary objects, ready for a report or the next analysis, without anyone retyping them. This guide
says what run data is, the rules a change that touches it follows, the standard a run's record is
judged against, and the Run data section a pull request carries. It is written for anyone changing
the code, a person or a coding agent.

The standard below is a target. Where the code falls short of it, a pull request says so in plain
words; it does not describe the code as doing what it does not do.

## What Run Data Is

Four things, each with a control-side form and an analysis-side form:

- **The setup.** Everything a test is configured with: the profile, target or reference, the
  schedule, the limits and aborts, the control settings and the law, the shaker and the article.
  The shell's test screens build it (`ui/src/lib/acquire`), and the control service accepts it as
  the configuration of a run (`crates/sds-control`, the module crates, and the control half of
  `schema/PROTOCOL.md`).
- **The channel table.** Every channel's role, sensitivity, range, units and limits. The control
  service holds the table a run uses (`crates/hertzforge-channel-table`); the shell reads and
  writes table files (`ui/src/lib/tables`); the analysis side keeps the same table as an object
  (`src/hertzforge/core/channel_table.py`, `src/hertzforge/io/tables.py`, and `channel_table/` in
  `docs/hforge-format.md`).
- **The run.** What happened: the station it ran on, every state, level, alarm, abort and
  operator action, and the measured streams. Where a module records, the control service writes
  the run into a run folder (`crates/hertzforge-capture`, `crates/hertzforge-recorder`).
- **The results.** What the module computed from the run: spectra, pulses, verdicts,
  identifications. The analysis side has converters for two modules' run folders
  (`src/hertzforge/io/*_run_convert.py`); `run/` in `docs/hforge-format.md` is the layout they
  write.

`tools/run_data_paths.txt` lists the code that decides what these hold, on the control side and at
the seam. A change to the analysis side alone is not on it.

## Rules for a Change

These bind every change to a listed path.

1. **Say what a run saves, truthfully.** A pull request says what it adds to the setup, the
   table, the results and the timeline, and where a finished run keeps each one, or that it is not
   recorded yet. A sentence in a guide or the protocol about what a run saves is written only for
   behavior the code has, and a test covers it.
2. **Do not widen the gap.** No table column that lives only in the shell. No result that exists
   only on screen. No setup state the control service never sees: a new setup field is part of the
   configuration the control service accepts.
3. **Never write over a run.** Code that makes a run folder takes a name no earlier run has, and
   claims it by creating the folder with a call that fails when the folder exists. A default name
   is where a unique name starts, not where it ends.
4. **Simulated stays labeled, and a label is never guessed.** Every record and every object from a
   run says whether it came from the simulator or from hardware. A reader that finds no label
   refuses, or marks the source unknown; it never fills in a default.
5. **Recording stays off the loop thread.** All file work, results and events included, happens
   on a writer thread; the loop thread hands data over with a send that never waits. A stream that
   fails during a control run is an event and a notice, never a stall, a stop or an abort. The
   Recorder is the stated exception: its recording is what it is for, and it ends the run when a
   write fails, as `docs/recorder.md` says.
6. **One mapping, in one language.** A conversion between a control form and an analysis form
   (table rows, profiles, references, units) is written once, in Python. A copy in another
   language exists only as a test against shared golden data. The copies that exist today stand
   until they are replaced; no new one is added. The cross-spectral convention is converted once,
   at the control service's edge, and nowhere else.
7. **Acceptance includes the bridge.** Every control module's acceptance list has the row "the
   finished run opens in the analysis side with its channel table and setup". Until it does, the
   row is there and recorded as not met; it is never left out.
8. **Recording work near the drive is drive-path work.** A change in a crate on the drive-path list
   also follows `docs/drive-path.md`. Anything read from a file that feeds a test (a restored
   table, an opened setup, a seed from a project) is checked by the control service as a typed
   value is, and the operator sees it before arming.

## What a Run's Record Holds

This is the standard a change is measured against. For every run, on the simulator and on
hardware, the record holds:

1. **The channel table as used,** every column.
2. **The station:** its devices and their serial numbers, the rates, and for a simulated run the
   plant and its seed.
3. **The setup as accepted, whole,** with the profile, target or reference as resolved.
4. **The limits and aborts in force.**
5. **The control settings:** the law's identity, parameters and file, and the identification.
6. **The shaker and the feasibility inputs.**
7. **The timeline:** every state, level, alarm, abort, overload and operator action, and the
   outcome with its reason.
8. **The data:** every enabled input and every drive, and the module's results.
9. **The provenance:** the software build, the source label (simulator or hardware), the
   operator, the article, the specification, and the start and end times with their UTC offset.
10. **A unit on every array.**

A module meets the standard when a test on the simulator checks its run's record item by item, and
its finished run opens in the analysis side with its channel table and setup. A pull request that
adds to a module's record says which items it adds; one that adds to the setup, the table or the
results without recording them says "not recorded yet" and why.

## Seed, Do Not Retype

Where an analysis object can supply a setup input (a profile from a specification, a channel table
from a project, a reference from field data), the setup offers to take it. Where a result can arrive
in the analysis side as an object, it does. A pull request that adds either says what it seeds from
and what it flows to. A seed fills a draft; it never arms anything, and the control service checks
what it fills in as it checks a typed value.

## The Run Data Section

The pull request template has a "Run data" section, after "Contracts touched". A pull request that
changes none of the paths in `tools/run_data_paths.txt` keeps its first line,
`No run data touched.`, and deletes the fields. One that changes a listed path deletes that line and
fills in all eight fields:

- **Adds to the setup:** the setup fields it adds, or none.
- **Adds to the table:** the channel table columns it adds, or none.
- **Adds to the results:** the named results it adds, or none.
- **Adds to the timeline:** the event kinds it adds, or none.
- **Recorded in:** for each of those, the file and field a finished run keeps it in, or "not
  recorded yet" and why.
- **Seeds from:** the analysis objects it can take its inputs from, or none.
- **Flows to:** the analysis objects it becomes, or none.
- **Bridge test:** "row added or updated", "row not possible yet", or "no run data touched" for a
  change on a listed path that adds nothing a run holds (a refactor, a test fix).

CI fails until the fields are there and filled in when the diff touches a listed path. A Markdown
file can be listed by one section (`docs/hforge-format.md#channel_table`); it counts as touched
when the diff changes a line under that heading.

## Running the Check

The run-data section is checked by the same script as the drive path:

```bash
python tools/check_drive_path.py --base origin/main \
    --title "the pull request's title" --body-file description.md
```

It prints which listed paths the branch touches and what the description is missing. Its tests are
in `tools/tests/test_check_drive_path.py`.
