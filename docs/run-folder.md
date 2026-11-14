# The Run Folder

A control run is kept on disk as a run folder: one folder per run, written by the
control service while the run goes, that needs no other file and no running
service to be read. The analysis side converts it into an ordinary project file
(`hertzforge convert-run FOLDER`, or open the folder or its `run.json`), so a
finished run reaches every analysis tool with its channel table, its setup, its
log and its measurements. Today the Random module writes one for every run; the
other control modules do not yet.

The exact form is the reader's, `hertzforge.io.run_folder`; the conversion is
`hertzforge.io.run_convert`, described in `docs/hforge-format.md` ("Run Folders
and Their Conversion"). This guide says what is in a folder and the rules it is
written by.

## Where It Goes

Under the service's data folder: the folder `HERTZFORGE_DATA_ROOT` names, or
`Hertzforge_Data` in the home folder. A Random run's folder is named
`<YYYYMMDD-HHMMSS>_random_<run id>`, the time it armed in UTC. The run's state
(`random.state`) names the folder from arm on (`record.folder`).

## Layout

```
<run name>/
  run.json            the manifest
  table.json          the channel table the run armed with
  setup.json          the configuration as accepted, and what it resolved to
  law/law.json        the control law in use
  log.jsonl           the run's log, one entry per line
  streams/<name>/     telemetry.f32 and stream.json
  results/<name>.json, <name>.bin, <name>.jsonl
  end.json            written once, at the end
```

### `run.json`

The manifest: `schema` (`hertzforge-run/1`), `test`, `run_uid` (unique across
restarts and machines: the time it armed and eight hexadecimal digits),
`run_id` (the service's own id, which repeats after a restart), `title`,
`operator`, `article`, `notes`, `specification` (`document`, `clause`, when
given), `source` (`SIM` or `HARDWARE`, from the station's class, never absent),
`sim` for a simulated run (the plant's `model`, its `parameters` and its
`seed`), `station` (`kind`, `class`, `devices` each with `id`, `model` and
`serial`, and `clock`), `grid` (the rate asked for and the rate the station
programmed, the FFT length, averages, line spacing and lines), `build`
(`version`, `commit`, `dirty`), `armed` and `start` (`unix` seconds,
`utc_offset_s`, and `start.station_frame`, the station frame at that instant),
`streams` and `results` (what the folder holds, each `name` and `kind`, a
stream also its channels), `recording` (`streams` `on` or `off`, and `why` when
off) and `complete`, false until the run has ended and `end.json` is written.

The station frame counts the input frames the station has delivered since it
started; every stream and log entry is placed by it.

### `table.json`

Every row of the channel table the run armed with, enabled or not, in the
protocol's own spelling for a channel row (`channels.get`), with the table's
`revision` and the SHA-256 of its rows (`sha256`), so two runs on one table can
be told to be so.

### `setup.json`

`config`, the configuration exactly as the service accepted it; `resolved`,
what the service made of it (the profile as anchors in g²/Hz, each with the
alarm and abort bands that hold from it to the next; the level schedule; the
tolerances; every limit and every abort in force with its channel; the control
settings; the control channels; the rate asked for and programmed); `derived`,
the derived values with the grid the run used; and `feasibility`, its inputs
and its verdict.

### `law/law.json`

The control law in use: its `id` and `name`, `builtin`, its parameters and the
guards it runs under. A law that is a file has its bytes beside it.

### `log.jsonl`

One JSON object per line, in order, each appended as the entry is made: `seq`,
`at_unix`, `frame` (the station frame, or null before the station has
delivered one), `kind`, `text` (the words the operator saw) and, where it has
any, `data`: the level of a level change, an alarm's lines and worst excursion,
a trip's facts, an overload's channel, an acknowledgement's words, the action
an operator took and who took it, what a `record` entry could not write. Every
state change, level change, alarm, trip, overload, override and operator action
is an entry.

### `streams/<name>/`

`telemetry.f32`: little-endian float32, frame-interleaved, in engineering
units (each channel in its table unit, the drive in volts), no header.
`stream.json`: `kind` (`inputs`, `drives` or `recording`), `fs_hz`, `channels`
in slot order (each with its table `index` and `unit`), `first_station_frame`,
`frames`, `gaps` (each `at_frame`, its place in the file, and
`dropped_frames`), `complete` and `fault`. A drive stream also says its
`output_latency_frames`, how many frames after being written a drive sample is
played and whether that is `declared` by the station or `measured`; a Random
run adds `pipeline_lag_frames`, the drive-to-response lag its system
identification measured. A gap is never padded: the samples after it follow on.

### `results/`

A result is a set of named arrays: `<name>.json` describes it (`kind`,
`abscissa`, `channels` by table index, `convention` for a cross term, and each
array's `name`, `dtype`, `shape`, `unit` and byte `offset`), `<name>.bin` holds
the values (little-endian float64; complex as real and imaginary pairs). A
result that repeats appends a record's arrays to `<name>.bin` and a line to
`<name>.jsonl` each time. Cross terms are in the file's convention, conj(X)·Y,
converted once by the control service. Values are in the channels' own units;
the analysis side converts them to SI.

### `end.json`

`run_uid`, `end` exactly as the run's state reports it (`outcome`, `message`,
`at_s`, `level_db`, `trip`), `at_unix`, `station_frames`, each stream's
`frames`, `drops`, `gaps` and `fault`, and a closing summary. Written once;
`run.json` is rewritten with `complete: true` after it.

## The Rules

- **One folder per run, never reused.** The folder is claimed at arm by
  creating it with a call that fails when it exists; a name that is taken gets
  the next free `<name>-<n>`. A run never writes into another's folder.
- **Valid at every moment.** `run.json`, `table.json`, `setup.json` and the law
  are on disk before the first sample; small files are written whole to a
  temporary name and renamed into place; the log and the streams are appended
  and synced to the disk every five seconds. A run that stops any way, the
  service killed included, leaves a folder the reader takes whole, with
  `complete: false` and no `end.json` when it did not end cleanly.
- **All writing on one thread.** The record has its own writer thread, and
  every file operation happens there. The run hands it entries and results with
  a send that never waits. The streams are taken where the station's blocks
  pass, into buffers made before the run; when the writer has fallen behind and
  no buffer is free, the block is dropped, counted, and kept as a gap where it
  happened. Nothing on the control loop's thread touches a file, takes a lock a
  slow disk can hold, or waits.
- **Self-describing.** Every array has a unit, every stream names its channels
  by table index, and SIM or HARDWARE is stated, never guessed.
- **It grows by optional members.** A reader ignores members it does not know,
  and refuses a schema number newer than its own.

## When the Disk Fails

- **At arm.** When the folder, `run.json`, `table.json`, `setup.json` or the
  law cannot be written (a missing or read-only data folder), the arm is
  refused `record_unwritable`, naming the path and the reason. Nothing has been
  driven.
- **Below the free-space floor at arm** (512 MB kept free), the run goes on
  without its streams: `run.json` says `recording.streams` `off` and why, the
  log has a `record` entry, and the state's `record.streams` is `off`.
- **While the run goes.** A stream that cannot be written, or the free space
  falling under the floor (512 MB plus what the streams already hold, which the
  converted file needs again), stops that stream: its `stream.json` names the
  fault, the log has a `record` entry, and the state's `record.problem` says it
  in words. A log line or a result that cannot be written is reported the same
  way. The run goes on: recording never stops, stalls or aborts a control run.
- **The end.** When `end.json` cannot be written, the folder keeps
  `complete: false`, and the reader says the run did not end cleanly.

## What a Random Run Saves

From arm: `run.json`, `table.json`, `setup.json`, `law/law.json` (the built-in
control law and its guards) and `log.jsonl`; `streams/inputs`, every enabled
input as read from the station in table order, and `streams/drives`, every
enabled drive as the device accepted it, in volts, unless `record.streams` is
false; and at the end `end.json`. It writes no results yet.
