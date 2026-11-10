"""Convert a Recorder run folder into a `.hforge` run file.

The control service writes a Recorder run into its own folder while the
run goes: the run's records, and one folder per recording with the raw
input stream and two sidecars. This module reads that folder straight
from its bytes and writes an ordinary `.hforge` file with every
recording's time data under `run/`. It also reads such a file back
(`read`), which is how its tests check a round trip.

What the folder holds, and where each goes:

=================================  ==========================================
`run.json`                         `run/settings` (JSON text)
`log.jsonl`                        `run/log` (one JSON string per entry)
`marks.jsonl`                      `run/marks` (one JSON string per mark)
`end.json`                         `run/end` (JSON text, finished runs only)
`recordings/rec_NNNN/`             `run/recordings/rec_NNNN/`
  `telemetry.f32`                    `data`, float32, frames by rows
  `recorder.json`                    attributes, `gaps`, `recorder_json`
  `capture.json`                     `capture_json`
=================================  ==========================================

The layout of `run/`:

    run/                  attrs: test ('recorder'), run_folder,
                          complete (false when the run or any recording
                          did not finish: what was written is kept)
      settings, end       JSON text
      log, marks          one JSON string per line
      recordings/
        rec_NNNN/         attrs: name, trigger, requested_fs_hz, fs_hz,
                          group_delay_frames, rows, units,
                          first_station_frame, trigger_station_frame,
                          pre_frames, pre_frames_asked, frames (the
                          file's), frames_stated (the sidecar's),
                          gap_free, complete, fault ('' when none),
                          partial_frame_bytes (a frame cut short at the
                          end of the file, left out)
          data            float32, frames by rows, in the row's unit
          gaps            int64, n by 2: at_frame, dropped_frames
          recorder_json   the sidecar as written, JSON text
          capture_json    the writer's sidecar as written, JSON text

`telemetry.f32` has no header: little-endian 32-bit floats, one frame
after another, one value per recorded row. Its size is the authority on
its length, so `data` holds every whole frame the file holds whatever a
sidecar says; a frame cut short by a crash is left out and counted.
Values are kept as float32, the precision they were recorded at.

A frame's station frame is the recording's first station frame plus its
file frame plus the frames missing before it (`station_frames`); its
time from the trigger is its station frame less the trigger's, over
`fs_hz` (`seconds_from_trigger`). On a gap-free recording that is
`(file frame - pre_frames) / fs_hz`.

A folder holding a single recording (`telemetry.f32` and
`recorder.json`) converts too, as a run of one recording.
"""

from __future__ import annotations

import json
import os
from typing import Any

import h5py
import numpy as np

#: the schema stamp a `.hforge` file carries
HFORGE_SCHEMA = 1
#: the `test` attribute of a converted Recorder run
TEST = 'recorder'

_TELEMETRY = 'telemetry.f32'
_SIDECAR = 'recorder.json'
_CAPTURE = 'capture.json'


class RunFolderError(ValueError):
    """The folder is not a Recorder run, or a record in it cannot be read."""


# ---- reading the run folder --------------------------------------------------------------

def _json(path: str) -> Any:
    with open(path, encoding='utf-8') as f:
        return json.load(f)


def _text(path: str) -> str:
    with open(path, encoding='utf-8') as f:
        return f.read()


def _lines(path: str) -> list[str]:
    """The whole JSON lines of a `.jsonl` record; a line cut short by a crash ends it."""
    out: list[str] = []
    if not os.path.exists(path):
        return out
    with open(path, encoding='utf-8') as f:
        for line in f:
            if not line.strip():
                continue
            try:
                json.loads(line)
            except json.JSONDecodeError:
                break
            out.append(line.strip())
    return out


def _is_recording(folder: str) -> bool:
    return os.path.isfile(os.path.join(folder, _TELEMETRY)) and os.path.isfile(
        os.path.join(folder, _SIDECAR))


def recording_folders(folder: str) -> list[str]:
    """The recording folders of a run folder, in the order they began (their names)."""
    if _is_recording(folder):
        return [folder]
    base = os.path.join(folder, 'recordings')
    if not os.path.isdir(base):
        return []
    names = sorted(n for n in os.listdir(base) if os.path.isdir(os.path.join(base, n)))
    return [os.path.join(base, n) for n in names]


def read_recording(folder: str) -> dict[str, Any]:
    """One recording read from its bytes: `data` (frames by rows, float32), the
    sidecars as dictionaries and as text, and the facts the run file keeps."""
    side_path = os.path.join(folder, _SIDECAR)
    try:
        side_text = _text(side_path)
        side = json.loads(side_text)
    except (OSError, json.JSONDecodeError) as e:
        raise RunFolderError(f'{side_path} cannot be read: {e}') from e
    rows = [int(r) for r in side.get('rows') or []]
    if not rows:
        raise RunFolderError(f'{side_path} names no recorded rows')
    cap_path = os.path.join(folder, _CAPTURE)
    cap_text = _text(cap_path) if os.path.exists(cap_path) else ''
    try:
        cap = json.loads(cap_text) if cap_text else {}
    except json.JSONDecodeError:
        cap = {}
    tele = os.path.join(folder, _TELEMETRY)
    width = len(rows)
    size = os.path.getsize(tele)
    whole = size // (4 * width)
    partial = size - whole * width * 4
    raw = np.fromfile(tele, dtype='<f4', count=whole * width)
    data = raw.reshape(whole, width).astype(np.float32, copy=False)
    gaps = np.asarray([[int(g['at_frame']), int(g['dropped_frames'])]
                       for g in side.get('gaps') or []], dtype=np.int64).reshape(-1, 2)
    fault = side.get('fault')
    complete = bool(side.get('complete', False)) and (not cap or bool(cap.get('complete', True)))
    return {
        'name': str(side.get('name') or os.path.basename(os.path.normpath(folder))),
        'trigger': str(side.get('trigger') or ''),
        'requested_fs_hz': float(side.get('requested_fs_hz', side.get('fs_hz', 0.0)) or 0.0),
        'fs_hz': float(side.get('fs_hz', 0.0) or 0.0),
        'group_delay_frames': float(side.get('group_delay_frames', 0.0) or 0.0),
        'rows': rows,
        'units': [str(u) for u in side.get('units') or [''] * width],
        'first_station_frame': int(side.get('first_station_frame', 0) or 0),
        'trigger_station_frame': int(side.get('trigger_station_frame',
                                              side.get('first_station_frame', 0)) or 0),
        'pre_frames': int(side.get('pre_frames', 0) or 0),
        'pre_frames_asked': int(side.get('pre_frames_asked', side.get('pre_frames', 0)) or 0),
        'frames': int(whole),
        'frames_stated': int(side.get('frames', whole) or 0),
        'gap_free': gaps.shape[0] == 0 and int(cap.get('chunks_dropped', 0) or 0) == 0,
        'complete': complete,
        'fault': '' if fault is None else str(fault),
        'partial_frame_bytes': int(partial),
        'gaps': gaps,
        'data': data,
        'recorder_json': side_text,
        'capture_json': cap_text,
    }


def station_frames(rec: dict[str, Any]) -> np.ndarray:
    """Each file frame's station frame: the first station frame, plus the file
    frame, plus every frame missing before it."""
    n = int(rec['data'].shape[0]) if 'data' in rec else int(rec['frames'])
    out = np.arange(n, dtype=np.int64) + int(rec['first_station_frame'])
    for at, dropped in np.asarray(rec['gaps'], dtype=np.int64).reshape(-1, 2):
        out[int(at):] += int(dropped)
    return out


def seconds_from_trigger(rec: dict[str, Any]) -> np.ndarray:
    """Each file frame's time from the trigger, seconds of station time."""
    return (station_frames(rec) - int(rec['trigger_station_frame'])) / float(rec['fs_hz'])


# ---- writing the run file ---------------------------------------------------------------

def _strings(group: h5py.Group, name: str, values) -> None:
    group.create_dataset(name, data=list(values), dtype=h5py.string_dtype('utf-8'))


_ATTRS = ('name', 'trigger', 'requested_fs_hz', 'fs_hz', 'group_delay_frames',
          'first_station_frame', 'trigger_station_frame', 'pre_frames', 'pre_frames_asked',
          'frames', 'frames_stated', 'gap_free', 'complete', 'fault', 'partial_frame_bytes')


def convert(folder: str, path: str) -> str:
    """Write the Recorder run in `folder` to the `.hforge` file `path`; returns `path`.

    Refuses a folder with no recording and no run record in it. Writes to a
    temporary file beside `path` and renames it, so `path` is never half written.
    """
    if not os.path.isdir(folder):
        raise RunFolderError(f'{folder} is not a folder')
    single = _is_recording(folder)
    recs = recording_folders(folder)
    if not single and not recs and not os.path.exists(os.path.join(folder, 'run.json')):
        raise RunFolderError(
            f'{folder} holds no Recorder run; choose the run folder the control service '
            'wrote, or one of its recording folders')
    tmp = path + '.partial'
    try:
        with h5py.File(tmp, 'w') as f:
            f.attrs['hertzforge_schema'] = HFORGE_SCHEMA
            run = f.create_group('run')
            run.attrs['test'] = TEST
            run.attrs['run_folder'] = os.path.basename(os.path.normpath(folder))
            complete = single or os.path.exists(os.path.join(folder, 'end.json'))
            if not single:
                for name, key in (('run.json', 'settings'), ('end.json', 'end')):
                    p = os.path.join(folder, name)
                    if os.path.exists(p):
                        _strings(run, key, [_text(p)])
                for name, key in (('log.jsonl', 'log'), ('marks.jsonl', 'marks')):
                    lines = _lines(os.path.join(folder, name))
                    if lines or os.path.exists(os.path.join(folder, name)):
                        _strings(run, key, lines)
            group = run.create_group('recordings')
            for rf in recs:
                rec = read_recording(rf)
                g = group.create_group(rec['name'])
                for k in _ATTRS:
                    g.attrs[k] = rec[k]
                g.attrs['rows'] = np.asarray(rec['rows'], dtype=np.int64)
                g.attrs['units'] = [str(u) for u in rec['units']]
                g.create_dataset('data', data=rec['data'])
                g.create_dataset('gaps', data=rec['gaps'])
                _strings(g, 'recorder_json', [rec['recorder_json']])
                _strings(g, 'capture_json', [rec['capture_json']])
                complete = complete and rec['complete']
            run.attrs['complete'] = bool(complete)
        os.replace(tmp, path)
    finally:
        if os.path.exists(tmp):
            os.remove(tmp)
    return path


# ---- reading the run file back ---------------------------------------------------------

def _read_strings(ds) -> list[str]:
    return [x.decode('utf-8') if isinstance(x, bytes) else str(x) for x in ds[()]]


def _attr(v):
    if isinstance(v, bytes):
        return v.decode('utf-8')
    if isinstance(v, np.ndarray) and v.dtype.kind in 'OSU':
        return [x.decode('utf-8') if isinstance(x, bytes) else str(x) for x in v]
    return v


def read(path: str) -> dict:
    """The `run/` group of a Recorder `.hforge` file, as nested dictionaries of numpy
    arrays and attributes (`'attrs'`)."""
    def walk(group) -> dict:
        out: dict = {'attrs': {k: _attr(group.attrs[k]) for k in group.attrs}}
        for k, v in group.items():
            if isinstance(v, h5py.Group):
                out[k] = walk(v)
            elif h5py.check_string_dtype(v.dtype) is not None:
                out[k] = _read_strings(v)
            else:
                out[k] = v[()]
        return out

    with h5py.File(path, 'r') as f:
        if 'hertzforge_schema' not in f.attrs:
            raise RunFolderError(f'{path} is not a Hertzforge file')
        if int(f.attrs['hertzforge_schema']) > HFORGE_SCHEMA:
            raise RunFolderError(
                f'{path} was written by a newer Hertzforge; update Hertzforge to open it')
        if 'run' not in f or _attr(f['run'].attrs.get('test', '')) != TEST:
            raise RunFolderError(f'{path} holds no Recorder run')
        return walk(f['run'])


def recording_names(run: dict) -> list[str]:
    """The recordings of a run read by `read`, in the order they began."""
    return sorted(k for k in run['recordings'] if k != 'attrs')


def recording_of(run: dict, name: str) -> dict[str, Any]:
    """One recording of a run read by `read`, in the shape `read_recording` gives
    (its attributes, `data` and `gaps`), for `station_frames` and
    `seconds_from_trigger`."""
    g = run['recordings'][name]
    return {**g['attrs'], 'data': g['data'], 'gaps': g['gaps']}
