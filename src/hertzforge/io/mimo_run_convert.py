"""Convert a MIMO Random run folder into a `.hforge` run file.

The control service records a multiple-output random run in its own folder
while the run goes (decision C, option 2 of the file-format plan): binary
payloads of little-endian float64 with a JSON sidecar beside each, appended
as the loops come, so a run cut short leaves everything it wrote readable.
This module turns that folder into an ordinary `.hforge` file with the run
under `run/` (the layout is in `docs/hforge-format.md`, "`run/`, a MIMO
Random Run"; a proposal for the analysis maintainer). It also reads such a
file back (`read`), which is how its tests check a round trip.

What the folder holds, and where each goes:

==========================  =============================================
`reference.bin/.json`       `run/reference/`
`identification.bin/.json`  `run/identification/`
`loops_diagonal.bin/.json`  `run/loops/diagonal/`
`loops_summary.jsonl`       `run/loops/summary/`
`sdm_<loop>.bin/.json`      `run/loops/sdm/<loop>/`
`law.json`, `law_file.wasm` `run/law/`
`law_calls.bin/.jsonl`      `run/law/calls/`
`report.json`               `run/report` (JSON text)
`capture.json`              `run/` attributes and `run/settings`
==========================  =============================================

The raw input stream (`telemetry.f32`) is the run writer's to convert, with
every other module's; it is not read here.

**Convention.** The controller holds every spectral matrix as
`G = E[X X^H]` per line, Hermitian, packed as its upper triangle row by row
(`G[0,0], G[0,1], ..., G[0,n-1], G[1,1], ...`). The file keeps that packing
(half the size of the full matrix) and says so in attributes, so the phase
of every cross term arrives unchanged. `unpack` gives the full matrices.

A folder from a run that did not finish converts too: whatever was written
is kept, the parts never written are left out, and `run` carries
`complete = False`.
"""

from __future__ import annotations

import json
import os
from typing import Any

import h5py
import numpy as np

#: the schema stamp a `.hforge` file carries
HFORGE_SCHEMA = 1
#: the convention and packing every spectral matrix here is in
CONVENTION = 'G = E[X X^H] per line'
PACKING = 'upper triangle, row-major, line-major'
#: the record format tags this module reads
IDENT_FORMAT = 'hertzforge-mimo-identification/1'
LAW_CALLS_FORMAT = 'hertzforge-mimo-law-calls/1'


class RunFolderError(ValueError):
    """The folder is not a MIMO Random run, or a record in it cannot be read."""


# ---- reading the controller's records ----------------------------------------------------

def _json(path: str) -> Any:
    with open(path, encoding='utf-8') as f:
        return json.load(f)


def _array(payload: bytes, info: dict) -> np.ndarray:
    """One array of a payload, as its sidecar entry describes it."""
    start, length = int(info['offset_bytes']), int(info['len_bytes'])
    if start + length > len(payload):
        raise RunFolderError(f"array {info['name']} runs past its payload")
    raw = np.frombuffer(payload, dtype='<f8', count=length // 8, offset=start).copy()
    shape = [int(x) for x in info['shape']]
    if info['kind'] == 'c64':
        values = raw[0::2] + 1j * raw[1::2]
        if info.get('packed'):
            lines, n = shape[0], shape[1]
            return values.reshape(lines, n * (n + 1) // 2)
        return values.reshape(shape)
    return raw.reshape(shape)


def read_record(folder: str, name: str) -> tuple[dict, dict[str, np.ndarray]]:
    """A `<name>.bin` / `<name>.json` record: its sidecar and its arrays by name."""
    side = _json(os.path.join(folder, f'{name}.json'))
    with open(os.path.join(folder, f'{name}.bin'), 'rb') as f:
        payload = f.read()
    return side, {a['name']: _array(payload, a) for a in side.get('arrays', [])}


def unpack(packed: np.ndarray) -> np.ndarray:
    """Full Hermitian matrices `[lines, n, n]` from packed upper triangles."""
    lines, p = packed.shape
    n = round((np.sqrt(8 * p + 1) - 1) / 2)
    if n * (n + 1) // 2 != p:
        raise RunFolderError(f'{p} values per line is not a packed triangle')
    full = np.zeros((lines, n, n), dtype=complex)
    iu = np.triu_indices(n)
    full[:, iu[0], iu[1]] = packed
    full[:, iu[1], iu[0]] = np.conj(packed)
    return full


def _diagonal(folder: str) -> dict[str, np.ndarray] | None:
    side_path = os.path.join(folder, 'loops_diagonal.json')
    if not os.path.exists(side_path):
        return None
    side = _json(side_path)
    lines, nc, nd = int(side['lines']), int(side['controls']), int(side['drives'])
    per = lines * (2 * nc + nd)
    raw = np.fromfile(os.path.join(folder, 'loops_diagonal.bin'), dtype='<f8')
    loops = min(int(side['loops']), raw.size // per, len(side['loop_index']))
    raw = raw[:loops * per].reshape(loops, per)
    a, b = lines * nc, 2 * lines * nc
    return {
        'loop_index': np.asarray(side['loop_index'][:loops], dtype=np.int64),
        'control_psd': raw[:, :a].reshape(loops, lines, nc),
        'control_error_db': raw[:, a:b].reshape(loops, lines, nc),
        'drive_psd': raw[:, b:].reshape(loops, lines, nd),
    }


def _summary(folder: str) -> list[dict]:
    path = os.path.join(folder, 'loops_summary.jsonl')
    if not os.path.exists(path):
        return []
    out = []
    with open(path, encoding='utf-8') as f:
        for line in f:
            if not line.strip():
                continue
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError:
                break  # a line cut short by a crash ends the record
    return out


def _law_calls(folder: str) -> tuple[list[str], bytes] | None:
    index_path = os.path.join(folder, 'law_calls.jsonl')
    if not os.path.exists(index_path):
        return None
    with open(os.path.join(folder, 'law_calls.bin'), 'rb') as f:
        payload = f.read()
    lines = []
    with open(index_path, encoding='utf-8') as f:
        for line in f:
            if not line.strip():
                continue
            try:
                entry = json.loads(line)
            except json.JSONDecodeError:
                break
            if entry.get('format') != LAW_CALLS_FORMAT:
                raise RunFolderError(
                    f'the law-input record is not format {LAW_CALLS_FORMAT!r}; convert it '
                    'with the version that wrote it')
            if entry['offset_bytes'] + entry['len_bytes'] > len(payload):
                break
            lines.append(json.dumps(entry, sort_keys=True))
    return lines, payload


# ---- writing the run file ---------------------------------------------------------------

def _strings(group: h5py.Group, name: str, values) -> None:
    group.create_dataset(name, data=list(values), dtype=h5py.string_dtype('utf-8'))


def _packed(group: h5py.Group, name: str, values: np.ndarray) -> None:
    ds = group.create_dataset(name, data=values)
    ds.attrs['packing'] = PACKING
    ds.attrs['convention'] = CONVENTION
    n = round((np.sqrt(8 * values.shape[1] + 1) - 1) / 2)
    ds.attrs['n'] = n


def convert(folder: str, path: str) -> str:
    """Write the run in `folder` to the `.hforge` file `path`; returns `path`.

    Refuses a folder with no MIMO Random record in it. Writes to a temporary
    file beside `path` and renames it, so `path` is never half written.
    """
    if not os.path.isdir(folder):
        raise RunFolderError(f'{folder} is not a folder')
    names = set(os.listdir(folder))
    if not ({'reference.json', 'identification.json', 'loops_diagonal.json',
             'law.json'} & names):
        raise RunFolderError(
            f'{folder} holds no MIMO Random records; choose the run folder the control '
            'service wrote')
    tmp = path + '.partial'
    with h5py.File(tmp, 'w') as f:
        f.attrs['hertzforge_schema'] = HFORGE_SCHEMA
        run = f.create_group('run')
        run.attrs['test'] = 'mimo_random'
        run.attrs['run_folder'] = os.path.basename(os.path.normpath(folder))
        complete = 'report.json' in names
        settings: dict = {}
        if 'capture.json' in names:
            cap = _json(os.path.join(folder, 'capture.json'))
            text = cap.get('run_config_json', '')
            _strings(run, 'settings', [text])
            try:
                settings = json.loads(text) if text else {}
            except json.JSONDecodeError:
                settings = {}
            run.attrs['start_unix_secs'] = int(cap.get('start_unix_secs', 0))
            complete = complete and bool(cap.get('complete', False))
        run.attrs['complete'] = complete
        run.attrs['source'] = settings.get('source', 'SIM')
        if 'setup_hash' in settings:
            run.attrs['setup_hash'] = settings['setup_hash']

        if 'reference.json' in names:
            side, arr = read_record(folder, 'reference')
            g = run.create_group('reference')
            _packed(g, 'sdm', arr['sdm'])
            for k in ('alarm_upper', 'alarm_lower', 'abort_upper', 'abort_lower'):
                g.create_dataset(k, data=arr[k])
            g.attrs['df_hz'] = float(side['df_hz'])
            g.attrs['band'] = np.asarray(side['band'], dtype=np.int64)
            g.attrs['space'] = side['space']
            g.create_dataset('controls', data=np.asarray(side['controls'], dtype=np.int64))

        if 'identification.json' in names:
            side, arr = read_record(folder, 'identification')
            if side.get('format') != IDENT_FORMAT:
                raise RunFolderError(
                    f"the identification record is format {side.get('format')!r}, this "
                    f'converter reads {IDENT_FORMAT!r}')
            g = run.create_group('identification')
            for k in ('frf', 'g_rd'):
                g.create_dataset(k, data=arr[k])
            for k in ('g_cc', 'g_dd', 'noise'):
                _packed(g, k, arr[k])
            g.create_dataset('coherence', data=arr['coherence'])
            g.create_dataset('condition', data=arr['condition'])
            g.create_dataset('drive_vrms', data=np.asarray(side['drive_vrms'], dtype=float))
            for k in ('setup_hash', 'level_db', 'frames', 'steps', 'lag_frames'):
                g.attrs[k] = side[k]

        loops = run.create_group('loops')
        diag = _diagonal(folder)
        if diag is not None:
            g = loops.create_group('diagonal')
            for k, v in diag.items():
                g.create_dataset(k, data=v)
            g.attrs['levels'] = 'full level'
        summary = _summary(folder)
        if summary:
            g = loops.create_group('summary')
            g.create_dataset('loop_index', data=np.asarray(
                [s['loop_index'] for s in summary], dtype=np.int64))
            g.create_dataset('level_db', data=np.asarray(
                [s['level_db'] if s['level_db'] is not None else np.nan for s in summary],
                dtype=float))
            _strings(g, 'state', [s['state'] for s in summary])
            g.create_dataset('law_misses', data=np.asarray(
                [s['law_misses'] for s in summary], dtype=np.int64))
            _strings(g, 'json', [json.dumps(s, sort_keys=True) for s in summary])
        sdm_names = sorted(n[:-5] for n in names
                           if n.startswith('sdm_') and n.endswith('.json'))
        if sdm_names:
            g = loops.create_group('sdm')
            for name in sdm_names:
                side, arr = read_record(folder, name)
                s = g.create_group(f"{int(side['loop_index']):06d}")
                s.attrs['loop_index'] = int(side['loop_index'])
                s.attrs['why'] = side['why']
                _packed(s, 'response_full_level', arr['response_full_level'])
                _packed(s, 'drive_full_level', arr['drive_full_level'])

        if 'law.json' in names:
            law = _json(os.path.join(folder, 'law.json'))
            g = run.create_group('law')
            for k in ('id', 'name', 'version', 'sha256'):
                g.attrs[k] = law.get(k, '')
            g.attrs['builtin'] = bool(law.get('builtin', law.get('sha256', '') == ''))
            _strings(g, 'params', [json.dumps(p, sort_keys=True)
                                   for p in law.get('params', [])])
            _strings(g, 'parameters', law.get('parameters', []))
            if law.get('catalog_path'):
                g.attrs['catalog_path'] = law['catalog_path']
            if law.get('file') and law['file'] in names:
                with open(os.path.join(folder, law['file']), 'rb') as lf:
                    g.create_dataset('file', data=np.frombuffer(lf.read(), dtype=np.uint8))
            calls = _law_calls(folder)
            if calls is not None:
                c = g.create_group('calls')
                index, payload = calls
                _strings(c, 'index', index)
                c.create_dataset('payload', data=np.frombuffer(payload, dtype=np.uint8))
                c.attrs['format'] = LAW_CALLS_FORMAT

        if 'report.json' in names:
            with open(os.path.join(folder, 'report.json'), encoding='utf-8') as rf:
                _strings(run, 'report', [rf.read()])
    os.replace(tmp, path)
    return path


# ---- reading the run file back ---------------------------------------------------------

def _read_strings(ds) -> list[str]:
    return [x.decode('utf-8') if isinstance(x, bytes) else str(x) for x in ds[()]]


def read(path: str) -> dict:
    """The `run/` group of a `.hforge` run file, as nested dictionaries of numpy
    arrays and attributes (`'attrs'`)."""
    def walk(group) -> dict:
        out: dict = {'attrs': {k: group.attrs[k] for k in group.attrs}}
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
        if 'run' not in f:
            raise RunFolderError(f'{path} holds no run')
        return walk(f['run'])


def law_call_arrays(run: dict, k: int) -> dict[str, np.ndarray]:
    """The arrays written in law call `k` of a run read by `read` (inputs that did
    not change since the call before are listed in its `carried`, not repeated)."""
    calls = run['law']['calls']
    entry = json.loads(calls['index'][k])
    payload = calls['payload'].tobytes()
    base = int(entry['offset_bytes'])
    chunk = payload[base:base + int(entry['len_bytes'])]
    return {a['name']: _array(chunk, a) for a in entry['arrays']}
