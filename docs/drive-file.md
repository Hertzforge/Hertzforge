# The Drive File, Laid Out

A Playback test plays a drive the operator made, open loop, on as many drives as the station has.
The control service reads that drive from one form only, the **drive folder** this page lays out.
Every other form (a CSV, a MATLAB or NumPy file, HDF5, a `.hforge` time history, a universal file,
an Excel sheet, a WAV) is read on the analysis side and written to a drive folder by the converter,
`hertzforge.io.drive_convert`, which the analysis service offers as `drive.inspect`,
`drive.convert` and `drive.fix_ends` (`schema/PROTOCOL.md`, "Drive files"). The control service
checks the folder again before it arms. This page is the layout, so the folder can be read and
written without either.

## The Folder

```
<name>/
  drive.bin     the samples
  drive.json    the sidecar, written last
```

New folders go under `drives/` in the data folder, the folder `HERTZFORGE_DATA_ROOT` names or
`Hertzforge_Data` in the home folder, which is local to the computer that runs the test. A folder
is never written over: the converter makes it with a call that fails when it exists, and a name
that is taken gets the next free `<name>-<n>`. A folder named outright is refused when it exists.
`drive.bin` and then `drive.json` are each written to a temporary name and renamed into place, so a
folder with no `drive.json` is one whose writing did not finish, and is not a drive.

## `drive.bin`

| Field | Type |
|---|---|
| Magic: the bytes `HFDRV01` and a zero byte | 8 bytes |
| The samples, interleaved by sample: sample 0 of every channel in channel order, then sample 1, and so on | `samples x channels` little-endian IEEE 754 `f64` |

Nothing else: no header beyond the magic, no padding, no trailer. The file is exactly
`8 + 8 x channels x samples` bytes. Every value is a finite number, in volts at the drive output.

The samples are `f64` so the control service checks the values the converter wrote. Rounded to
`f32`, then integrated twice over a long record, the rounding alone can end a high tone's
displacement as far from rest as the tone's whole displacement. The drive output plays `f32`: each
sample is rounded to `f32` as it is read to play, and nowhere before, and the drive ceiling is
judged on the peak so rounded when that is the larger.

## `drive.json`

| Member | What |
|---|---|
| `format` | `"hertzforge-drive"` |
| `version` | `1` |
| `rate_hz` | Samples per second. The drive plays at this rate and is never resampled. |
| `samples` | Samples per channel. |
| `channels` | One per channel, in drive order: `name`, `unit` (always `"V"`) and `source`, the signal of the source it came from (its key, as `drive.inspect` gives it). |
| `sha256` | The SHA-256 of `drive.bin`, whole (magic included), as 64 lowercase hexadecimal digits. |
| `source` | Where it came from: `path` (the file on the computer that converted it, or for a project's objects the project's file, or null), `sha256` (of that file whole; for a project's objects, of the values chosen as little-endian `f64`, channel after channel in drive order), `format` (`delimited`, `mat5`, `mat73`, `npy`, `npz`, `hdf5`, `hforge`, `unv`, `xlsx`, `wav` or `project`) and `selections` (each `signal` and the `name` it was given). |
| `fixes` | Empty, or what was done to the drive after it was read (below), oldest first. |
| `tool` | What wrote it, `hertzforge` and its version. |
| `created_utc` | When, as `YYYY-MM-DDTHH:MM:SSZ`. |

A reader checks, before it uses a drive: the format and version; the magic; the size against
`samples` and the number of channels; the SHA-256 against `sha256`; and that every value is
finite. A reader ignores members it does not know.

## What the Converter Reads

| Form | Suffix | Signals |
|---|---|---|
| Delimited text | `.csv`, `.tsv`, `.txt`, `.tab`, `.asc`, `.prn` | Every column but the time column, read as the delimited reader reads a time history (`hertzforge.io.delimited`): the delimiter, header and units are found; a unit rides in the header as `Drive (mV)`. A time column is optional; with none, give the rate. |
| MATLAB | `.mat`, version 5 and 7.3 | Every real numeric variable. A version 7.3 file is HDF5 and is read in MATLAB's own orientation. |
| NumPy | `.npy`, `.npz` | The array, or every array of the archive. An array of Python objects is refused and never unpickled. |
| HDF5 | `.h5`, `.hdf5` | Every 1-D or 2-D real numeric dataset; a `unit` or `units` attribute states its unit, one per column. |
| Hertzforge | `.hforge` | Each record of each time history. |
| Universal file | `.unv`, `.uff` | Each record of each dataset 58 time function. |
| Excel | `.xlsx` | Every column of every sheet, under an optional header row; a column named for time is the time column. |
| WAV | `.wav` | Each channel. Integer samples are scaled to [-1, 1), so full scale is 1 V; float samples are taken as volts. |
| A project's time histories | | Each record, by object name. |

A 1-D array is one signal, as is a 2-D array with a single row; any other 2-D array offers its
columns, samples down the rows. A scalar named for a rate (`fs`, `sample_rate`, `rate_hz`,
`sampling_rate`, ...) or a step (`dt`, `time_step`, ...), an HDF5 attribute of the same name on
the file, or a 1-D array named for time (`t`, `time`), states the file's rate. A rate read from a
file is kept to ten significant digits; a rate given is written as given.

**Units.** A drive is in volts. A stated voltage unit (V, mV) is converted to volts. Any other
stated unit (g, m/s², N, counts) is refused by name. A signal whose unit the file does not state is
taken as volts, and the conversion says so.

**Refused,** each by name: a signal the file does not hold, or one chosen twice; drives of unequal
length; signals whose rates differ, or a stated rate that disagrees with the rate given by more
than one part in ten thousand; no rate at all; a time column that does not step evenly; a value
that is not finite, or too large for the drive output's `f32`; a folder that exists.

## Fixing the Ends

The control service refuses a drive whose ends are not at rest: taken as an acceleration and
integrated with a plain trapezoid (no high-pass), each channel's first and last samples must be
within 0.5 % of its peak, and its velocity and displacement must end within 1 % of their peaks. A
drive that fails can be fixed into a **new** folder (`drive.fix_ends`, or `fix_ends` on
`drive.convert`); the original is not changed, and the new folder is checked like any other.

For each channel of `samples` samples at `rate_hz`, with `T = (samples - 1) / rate_hz`:

1. A half-cosine taper at each end, `0.5 (1 - cos(pi n / L))` over the first `L + 1` samples and
   its mirror over the last, so the first and last samples are zero. `L` is the taper given, or 1 %
   of the record, at least 10 ms and at most 1 s; never more than half the record.
2. `c(t) = a sin(pi t / T) + b sin(2 pi t / T)`, zero at both ends, is subtracted, with `a` and `b`
   solved so the corrected record's trapezoid velocity and displacement end at zero. They are
   solved once more on the corrected record exactly as it is written, so the file ends at rest.

A channel that is zero throughout is left as it is. The fix is appended to `fixes`:

| Member | What |
|---|---|
| `kind` | `"fix_ends"` |
| `from` | The original folder (`folder`) and its `sha256` |
| `taper_s`, `taper_samples` | The taper at each end |
| `correction` | The correction's form, in words |
| `channels` | Each channel's `name`, `a` and `b` (volts), `rms_before` and `rms_after` (volts) |

A fix changes the drive the test plays. Look at the drive before and after before playing it: a
long taper or a large correction changes what the test asks of the shaker.

## The Golden Folders

`schema/golden/drive/basic/` is a drive folder of two channels, 401 samples at 1000 samples per
second, written by `python schema/make_golden_drive.py` with its `tool` and `created_utc` fixed.
Channel `k` is `A_k cos(2 pi m_k (t - T/2) / T) sin(pi t / T)^2`, `T = 0.4 s`, rounded to `f32` so
the bytes do not depend on the platform's sine, and stored as `f64`: `Drive 1` with `A = 0.5 V`,
`m = 8`, and `Drive 2` with `A = 0.25 V`, `m = 20`. Each starts and ends at zero, and its
trapezoid velocity and displacement end at zero to that rounding, so it passes the ends check. Its
`source` names a CSV of the same values at a fixed path; converting that CSV (`drive.convert` of
columns 2 and 3) writes the same `drive.bin`, byte for byte.

`schema/golden/drive/fixed/` is what `fix_ends` writes from a drive whose ends are not at rest:
two channels, 301 samples at 1000 samples per second, `Offset` (0.1 V plus a 0.4 V tone at 370 Hz)
and `Cosine` (a 0.5 V cosine at 370 Hz), each starting and ending away from zero. The same script
writes it, with the folder it was fixed from and the build fixed in its sidecar. The control
service's tests check that it passes the service's check with its velocity and displacement ending
within a millionth of their peaks.

`tests/test_drive_convert.py` reads both with and without the converter's reader, checks `basic`
against the formula and `fixed` against its ends, and checks the script writes them again.
