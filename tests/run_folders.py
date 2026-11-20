"""Synthetic run folders, written to the run folder layout
(`hertzforge.io.run_folder`), for the reader and converter tests.

`write_run` is the one place that knows the layout: every builder here
goes through it, so folders written by the control service can replace
or join these by swapping what calls it. The builders give a Random run
(inputs, a drive, a resolved profile, PSD, FRF and coherence results, a
log, a law and an end), a Sine run (a sweep or a dwell, its resolved
profile, its levels and its control trace), a Recorder run (a recording
with a pre-trigger and a gap), a pulse run (one stream of shots), and an
unfinished run.
Values are fixed, so a folder written twice is the same bytes.
"""

from __future__ import annotations

import json
import os
from collections.abc import Mapping, Sequence
from typing import Any

import numpy as np

FS = 1024.0
G = 9.80665

#: a start time and offset every folder shares, so conversions repeat
START = {'unix': 1794240000.25, 'utc_offset_s': -25200, 'station_frame': 1000}


def _json(path: str, value: Any) -> None:
    with open(path, 'w', encoding='utf-8') as f:
        json.dump(value, f, indent=1)


def write_run(folder: str, manifest: Mapping[str, Any], table: Mapping[str, Any],
              setup: Mapping[str, Any], *,
              streams: Mapping[str, tuple[dict[str, Any], np.ndarray]] = {},
              results: Mapping[str, tuple[dict[str, Any], dict[str, np.ndarray],
                                          Sequence[dict[str, Any]] | None]] = {},
              log: Sequence[Mapping[str, Any]] = (), end: Mapping[str, Any] | None = None,
              law: tuple[dict[str, Any], str, bytes] | None = None,
              torn_log: bytes = b'', torn_stream: Mapping[str, bytes] = {},
              ) -> str:
    """Write a run folder: the manifest, table, setup, log, law, streams
    (each a description and its frames by channels, written as float32
    frame by frame), results (each a description without `arrays`, its
    arrays, and its records when it repeats, the arrays then holding the
    records first) and the end, when given. `torn_log` is appended to the
    log with no line end, and `torn_stream` to a stream's samples, as a
    run cut off mid-write leaves them. Returns the folder."""
    os.makedirs(folder)
    manifest = dict(manifest)
    manifest.setdefault('streams', [{'name': name, 'kind': d['kind'],
                                     'channels': [c['index'] for c in d['channels']]}
                                    for name, (d, _v) in streams.items()])
    manifest.setdefault('results', [{'name': name, 'kind': d['kind']}
                                    for name, (d, _a, _r) in results.items()])
    _json(os.path.join(folder, 'run.json'), manifest)
    _json(os.path.join(folder, 'table.json'), table)
    _json(os.path.join(folder, 'setup.json'), setup)
    with open(os.path.join(folder, 'log.jsonl'), 'wb') as f:
        f.writelines(json.dumps(entry).encode() + b'\n' for entry in log)
        f.write(torn_log)
    if law is not None:
        os.makedirs(os.path.join(folder, 'law'))
        _json(os.path.join(folder, 'law', 'law.json'), law[0])
        with open(os.path.join(folder, 'law', law[1]), 'wb') as f:
            f.write(law[2])
    for name, (description, values) in streams.items():
        where = os.path.join(folder, 'streams', name)
        os.makedirs(where)
        with open(os.path.join(where, 'telemetry.f32'), 'wb') as f:
            f.write(np.ascontiguousarray(values, dtype='<f4').tobytes())
            f.write(torn_stream.get(name, b''))
        _json(os.path.join(where, 'stream.json'), {'name': name, **description})
    if results:
        os.makedirs(os.path.join(folder, 'results'))
    for name, (description, arrays, records) in results.items():
        layout, offset = [], 0
        for array_name, values in arrays.items():
            one = values[0] if records is not None else values
            dtype = 'c16' if np.iscomplexobj(one) else 'f8'
            layout.append({'name': array_name, 'dtype': dtype, 'shape': list(one.shape),
                           'unit': description.get('units', {}).get(array_name, ''),
                           'offset': offset})
            offset += one.size * (16 if dtype == 'c16' else 8)
        side = {key: value for key, value in description.items() if key != 'units'}
        side = {'name': name, **side, 'arrays': layout}
        _json(os.path.join(folder, 'results', f'{name}.json'), side)
        dtypes = {a['name']: '<c16' if a['dtype'] == 'c16' else '<f8' for a in layout}
        with open(os.path.join(folder, 'results', f'{name}.bin'), 'wb') as f:
            for k in range(len(records) if records is not None else 1):
                for array_name, values in arrays.items():
                    one = values[k] if records is not None else values
                    f.write(np.asarray(one, dtype=dtypes[array_name]).tobytes())
        if records is not None:
            with open(os.path.join(folder, 'results', f'{name}.jsonl'), 'w') as f:
                f.writelines(json.dumps(record) + '\n' for record in records)
    if end is not None:
        _json(os.path.join(folder, 'end.json'), end)
    return folder


def row(index: int, role: str, units: str = 'g', *, enabled: bool = True,
        description: str = '', extra: Mapping[str, str] | None = None,
        **fields: Any) -> dict[str, Any]:
    """A channel row in the protocol's spelling, every field present."""
    out = {'index': index, 'enabled': enabled, 'description': description or f'ch{index}',
           'role': role, 'physical': {'device': 'Dev1', 'channel': f'ai{index - 1}'},
           'interface': 'single_ended', 'iepe_bias_v': None, 'units': units,
           'sensitivity_mv_per_eu': 1000.0 if role == 'drive' else 100.0,
           'range_max': 10.0 if role == 'drive' else 50.0, 'polarity': 'positive',
           'control_group': None,
           'abort': {'accel_g': None, 'velocity_ips': None, 'displacement_mil': None}}
    out.update(fields)
    if extra is not None:
        out['extra'] = dict(extra)
    return out


def random_rows() -> list[dict[str, Any]]:
    """A Random run's table: a control input with a node, a monitor with
    none, the drive, and a monitor switched off."""
    return [
        row(1, 'control', description='Control', interface='iepe', iepe_bias_v=4.0,
            control_group=0, abort={'accel_g': 40.0, 'velocity_ips': None,
                                    'displacement_mil': 250.0},
            extra={'node': '101', 'direction': 'Z+', 'serial_number': '00417',
                   'model': 'A100', 'cal_date': '2027-01-31', 'mount': 'stud'}),
        row(2, 'monitor', description='Monitor', polarity='negative'),
        row(3, 'drive', 'V', description='Drive', physical={'device': 'Dev1',
                                                            'channel': 'ao0'}),
        row(4, 'monitor', enabled=False, description='Spare', physical=None),
    ]


def _random_profile() -> dict[str, Any]:
    return {'breakpoints': [
        {'freq_hz': 20.0, 'psd_g2_hz': 0.01, 'alarm_db': [3, 3], 'abort_db': [6, 6]},
        {'freq_hz': 80.0, 'psd_g2_hz': 0.04, 'alarm_db': [3, 3], 'abort_db': [6, 6]},
        {'freq_hz': 350.0, 'psd_g2_hz': 0.04, 'alarm_db': [3, 3], 'abort_db': [6, 6]},
        {'freq_hz': 500.0, 'psd_g2_hz': 0.02, 'alarm_db': [3, 3], 'abort_db': [6, 6]}]}


def random_setup() -> dict[str, Any]:
    """A Random setup as accepted and as resolved."""
    config = {'title': 'Qualification Random',
              'profile': {'breakpoints': [{'freq_hz': 20, 'psd_g2_hz': 0.01},
                                          {'freq_hz': 80, 'slope_db_oct': 6},
                                          {'freq_hz': 350, 'psd_g2_hz': 0.04},
                                          {'freq_hz': 500, 'psd_g2_hz': 0.02}],
                          'alarm_db': [3, 3], 'abort_db': [6, 6]},
              'acquisition': {'fs_hz': FS, 'n_pts': 256, 'n_avg': 4},
              'schedule': {'mode': 'auto', 'levels': [{'db': -6, 'dwell_s': 1}, {'db': 0}],
                           'full_level_s': 2},
              'control': {'shutdown_ramp_s': 1}, 'limits': [], 'feasibility': None}
    return {'config': config,
            'resolved': {'profile': _random_profile(),
                         'schedule': [{'db': -6, 'dwell_s': 1}, {'db': 0, 'dwell_s': 2}],
                         'control': {'composite': 'weighted_avg', 'control_dof': 120}},
            'derived': {'lines': 128, 'df_hz': 4.0},
            'feasibility': None}


def _manifest(test: str, **fields: Any) -> dict[str, Any]:
    out = {'schema': 'hertzforge-run/1', 'test': test, 'run_uid': f'uid-{test}-0001',
           'run_id': 'r-0007', 'title': f'Golden {test}', 'operator': 'Operator',
           'article': 'Bracket', 'notes': '', 'source': 'SIM',
           'sim': {'model': 'two_mass', 'seed': 7},
           'station': {'kind': 'sim', 'class': 'sim', 'devices': [], 'clock': 'internal'},
           'grid': {'fs_hz': FS, 'block_frames': 256},
           'build': {'version': '0.0.0', 'commit': '0' * 40, 'dirty': False},
           'start': dict(START), 'recording': {'streams': 'on'}, 'complete': True}
    out.update(fields)
    return out


def _wave(frames: int, channels: int, seed: float = 0.0) -> np.ndarray:
    t = np.arange(frames) / FS
    return np.stack([(0.5 + 0.25 * c) * np.sin(2 * np.pi * (40 + 30 * c) * t + seed)
                     for c in range(channels)], axis=1).astype(np.float32)


def random_log() -> list[dict[str, Any]]:
    """A Random run's log, in order."""
    kinds = [('state', 'armed'), ('station', 'the station programmed 1024 Hz'),
             ('level', 'level -6 dB'), ('alarm', 'channel 1 alarm at 120 Hz'),
             ('level', 'level 0 dB'), ('state', 'completed')]
    return [{'seq': k, 'at_unix': START['unix'] + k, 'frame': 1000 + 512 * k, 'kind': kind,
             'text': text, **({'data': {'level_db': -6 if k == 2 else 0}}
                              if kind == 'level' else {})}
            for k, (kind, text) in enumerate(kinds)]


def random_run(folder: str, frames: int = 4096, *, complete: bool = True,
               **manifest: Any) -> str:
    """A finished Random run: two enabled inputs, one drive, a profile,
    PSD, FRF and coherence results, a law file, a log and an end."""
    lines = 128
    f0, df = 0.0, 4.0
    freq = f0 + df * np.arange(lines)
    psd = np.stack([0.02 + 0.001 * np.arange(lines), 0.01 + 0.0005 * np.arange(lines)])
    frf = (np.arange(1, 3)[:, None, None] * (1.0 + 0.5j)
           * np.ones((2, 1, lines))) * (1 + freq / 1000.0)
    coherence = np.clip(0.9 + 0.0005 * np.arange(lines), 0, 1) * np.ones((2, 1, lines))
    loops = np.stack([psd[0] * (1 + 0.1 * k) for k in range(3)])[:, np.newaxis, :]
    inputs = _wave(frames, 2)
    drives = _wave(frames, 1, seed=1.0)
    results = {
        'response_psd': ({'kind': 'psd', 'abscissa': {'kind': 'frequency', 'unit': 'Hz',
                                                      'start': f0, 'step': df},
                          'channels': {'responses': [1, 2]}, 'convention': 'file',
                          'units': {'values': 'g^2/Hz'}}, {'values': psd}, None),
        'identification_frf': ({'kind': 'frf', 'abscissa': {'kind': 'frequency',
                                                            'unit': 'Hz', 'array': 'frequency'},
                                'channels': {'responses': [1, 2], 'references': [3]},
                                'convention': 'file', 'units': {'values': 'g/V',
                                                                'frequency': 'Hz'}},
                               {'frequency': freq, 'values': frf}, None),
        'identification_coherence': ({'kind': 'coherence', 'abscissa': {
                                          'kind': 'frequency', 'unit': 'Hz', 'start': f0,
                                          'step': df},
                                      'channels': {'responses': [1, 2], 'references': [3]}},
                                     {'values': coherence}, None),
        'control_loops': ({'kind': 'psd', 'abscissa': {'kind': 'frequency', 'unit': 'Hz',
                                                       'start': f0, 'step': df},
                           'channels': {'responses': [1]},
                           'records': {'index': 'loop', 'level_db': 'level'},
                           'units': {'values': 'g^2/Hz'}}, {'values': loops},
                          [{'index': k, 'frame': 2000 + 512 * k, 'level_db': 0}
                           for k in range(3)]),
        'verdicts': ({'kind': 'verdicts', 'abscissa': {'kind': 'index'},
                      'channels': {'responses': [1]}},
                     {'lines_out': np.array([0.0, 1.0, 0.0])}, None),
    }
    return write_run(
        folder, _manifest('random', complete=complete, **manifest),
        {'revision': 12, 'sha256': 'a' * 64, 'rows': random_rows()},
        random_setup(),
        streams={'inputs': ({'kind': 'inputs', 'fs_hz': FS,
                             'channels': [{'index': 1, 'unit': 'g'},
                                          {'index': 2, 'unit': 'g'}],
                             'first_station_frame': START['station_frame'],
                             'frames': frames, 'gaps': [], 'complete': complete,
                             'fault': None}, inputs),
                 'drives': ({'kind': 'drives', 'fs_hz': FS,
                             'channels': [{'index': 3, 'unit': 'V'}],
                             'first_station_frame': START['station_frame'],
                             'output_latency_frames': {'frames': 38, 'how': 'declared'},
                             'frames': frames, 'gaps': [], 'complete': complete,
                             'fault': None}, drives)},
        results=results, log=random_log(),
        end=({'run_uid': 'uid-random-0001',
              'end': {'outcome': 'completed', 'message': 'Completed.', 'at_s': 4.0},
              'at_unix': START['unix'] + 4, 'station_frames': frames,
              'streams': {'inputs': {'drops': 0, 'gaps': 0}}, 'summary': {'loops': 3}}
             if complete else None),
        law=({'id': 'law-1', 'name': 'Diagonal', 'version': '1.0', 'sha256': 'b' * 64,
              'builtin': False, 'params': {'gain': 0.5}, 'guards': []}, 'diagonal.py',
             b'def iterate(state):\n    return state\n'))


def sine_run(folder: str, test: str = 'sweep', frames: int = 2048) -> str:
    """A finished Sine run, as the control service writes one: a control
    input, a monitor and the drive; a profile of 0.5 g resolved on 200 to
    400 Hz; a `sweep` up and back down at 2 oct/min at -6 dB then 0 dB, or
    a `dwell` of 30 s at 250 Hz; its levels in two records of 64 points
    (the last filled out with NaN) and its control trace."""
    rows = [row(1, 'control', description='Control'), row(2, 'monitor', description='Monitor'),
            row(3, 'drive', 'V', description='Drive')]
    if test == 'dwell':
        band, resolved_test = (250.0, 250.0), {'kind': 'dwell', 'freq_hz': 250.0,
                                               'dwell': {'seconds': 30.0}}
        freq = np.array([250.0])
    else:
        band = (200.0, 400.0)
        resolved_test = {'kind': 'sweep', 'band_hz': list(band),
                         'rate': {'law': 'log', 'oct_per_min': 2.0}, 'start_hz': 200.0,
                         'direction': 'up', 'sweeps': 2, 'duration_s': None}
        freq = 200.0 * 2.0 ** (np.arange(9) / 8)
    target = np.full(len(freq), 0.5)
    reference = {'frequency': freq, 'reference': target, 'alarm_low': target * 10 ** -0.075,
                 'alarm_high': target * 10 ** 0.075, 'abort_low': target * 10 ** -0.15,
                 'abort_high': target * 10 ** 0.15}
    n, half = 100, 50
    t = np.arange(n) * 0.1
    if test == 'dwell':
        f = np.full(n, 250.0)
        direction = np.zeros(n)
        passes = np.zeros(n)
    else:
        up = 200.0 * 2.0 ** (np.arange(half) / (half - 1))
        f = np.concatenate([up, up[::-1]])
        direction = np.concatenate([[0.0], np.ones(half - 1), -np.ones(half)])
        passes = np.concatenate([np.zeros(half), np.ones(half)])
    level = np.concatenate([np.full(half, -6.0), np.zeros(n - half)])
    tracked = np.stack([0.5 * 10 ** (level / 20), 1.5 * 10 ** (level / 20)])
    phase = np.stack([np.full(n, -10.0), np.full(n, -95.0)])

    def blocks(values: np.ndarray) -> np.ndarray:
        padded = np.full((*values.shape[:-1], 128), np.nan)
        padded[..., :n] = values
        return np.stack([padded[..., :64], padded[..., 64:]])

    levels = {'time': blocks(t), 'frequency': blocks(f), 'level_db': blocks(level),
              'direction': blocks(direction), 'pass': blocks(passes),
              'target': blocks(0.5 * 10 ** (level / 20)), 'measured': blocks(tracked[0]),
              'drive': blocks(0.1 * 10 ** (level / 20)), 'values': blocks(tracked),
              'phase': blocks(phase)}
    trace = {'time': np.arange(64.0) * 0.1, 'frequency': np.full(64, 250.0),
             'target': np.full(64, 0.5), 'measured': np.full(64, 0.49),
             'applied': np.full(64, 0.1)}
    results = {
        'reference': ({'kind': 'envelope', 'abscissa': {'kind': 'frequency', 'unit': 'Hz',
                                                        'array': 'frequency'},
                       'channels': {'controls': [1]},
                       'units': {'frequency': 'Hz', **{k: 'g' for k in reference
                                                       if k != 'frequency'}}},
                      reference, None),
        'channel_levels': ({'kind': 'sine_levels', 'abscissa': {'kind': 'time', 'unit': 's',
                                                                'array': 'time'},
                            'channels': {'responses': [1, 2]},
                            'records': {'index': 'record'},
                            'units': {'time': 's', 'frequency': 'Hz', 'level_db': 'dB',
                                      'target': 'g', 'measured': 'g', 'drive': 'V',
                                      'values': 'g', 'phase': 'deg'}},
                           levels, [{'index': 0, 'points': 64}, {'index': 1, 'points': 36}]),
        'control_trace': ({'kind': 'sine_control', 'abscissa': {'kind': 'time', 'unit': 's',
                                                                'array': 'time'},
                           'channels': {}, 'records': {'index': 'record'},
                           'units': {'time': 's', 'frequency': 'Hz', 'target': 'g',
                                     'measured': 'g', 'applied': 'V'}},
                          {k: v[np.newaxis] for k, v in trace.items()},
                          [{'index': 0, 'calls': 64}]),
    }
    setup = {'config': {'title': 'Golden sine', 'test': {'kind': test}},
             'resolved': {'test': resolved_test,
                          'profile': {'breakpoints': [
                              {'freq_hz': 100.0, 'quantity': 'accel_g_pk', 'value': 0.5,
                               'accel_g_pk': 0.5, 'alarm_db': [1.5, 1.5],
                               'abort_db': [3.0, 3.0]},
                              {'freq_hz': 1000.0, 'quantity': 'accel_g_pk', 'value': 0.5,
                               'accel_g_pk': 0.5, 'alarm_db': [1.5, 1.5],
                               'abort_db': [3.0, 3.0]}], 'unit': 'g'}},
             'derived': {}, 'feasibility': None}
    del band
    return write_run(
        folder, _manifest('sine', grid={'fs_hz': FS, 'tick_frames': 102}),
        {'revision': 3, 'sha256': 'c' * 64, 'rows': rows}, setup,
        streams={'inputs': ({'kind': 'inputs', 'fs_hz': FS,
                             'channels': [{'index': 1, 'unit': 'g'}, {'index': 2, 'unit': 'g'}],
                             'first_station_frame': 0, 'frames': frames, 'gaps': [],
                             'complete': True, 'fault': None}, _wave(frames, 2)),
                 'drives': ({'kind': 'drives', 'fs_hz': FS,
                             'channels': [{'index': 3, 'unit': 'V'}],
                             'first_station_frame': 0, 'frames': frames, 'gaps': [],
                             'complete': True, 'fault': None}, _wave(frames, 1, seed=1.0))},
        results=results,
        log=[{'seq': 0, 'at_unix': START['unix'], 'frame': None, 'kind': 'state',
              'text': 'configured', 'data': {'to': 'idle'}}],
        end={'run_uid': 'uid-sine-0001', 'end': {'outcome': 'completed', 'message': 'Done.'},
             'at_unix': START['unix'] + 10, 'station_frames': frames, 'streams': {}})


def recorder_run(folder: str, frames: int = 2048, pre: int = 256,
                 gap: tuple[int, int] = (1024, 64)) -> str:
    """A Recorder run: one recording with a pre-trigger and a gap."""
    rows = [row(1, 'control', description='Triangle'),
            row(2, 'monitor', description='Ramp', extra={'node': '7', 'direction': 'X+'})]
    first = 5000
    setup = {'config': {'title': 'Golden recording', 'acquisition': {'fs_hz': FS},
                        'trigger': {'mode': 'level', 'pre_s': pre / FS},
                        'analysis': {'block': 256, 'window': 'hann', 'overlap_pct': 50}},
             'resolved': {}, 'derived': {'analysis': {'averages_s': 1.125,
                                                      'overlap_pct': 50}}}
    return write_run(
        folder, _manifest('recorder'), {'revision': 3, 'sha256': 'c' * 64, 'rows': rows},
        setup,
        streams={'rec_0001': ({'kind': 'recording', 'fs_hz': FS,
                               'channels': [{'index': 1, 'unit': 'g'},
                                            {'index': 2, 'unit': 'g'}],
                               'first_station_frame': first, 'trigger': 'level',
                               'trigger_station_frame': first + pre, 'pre_frames': pre,
                               'frames': frames,
                               'gaps': [{'at_frame': gap[0], 'dropped_frames': gap[1]}],
                               'marks': [], 'complete': True, 'fault': None},
                              _wave(frames, 2))},
        log=[{'seq': 0, 'at_unix': START['unix'], 'frame': None, 'kind': 'state',
              'text': 'armed'},
             {'seq': 1, 'at_unix': START['unix'] + 1, 'frame': first + 1024 + 32,
              'kind': 'record', 'text': 'the disk fell behind: 64 frames lost',
              'data': {'stream': 'rec_0001', 'failed': 'queue full'}}],
        end={'run_uid': 'uid-recorder-0001',
             'end': {'outcome': 'operator_stop', 'message': 'Stopped.', 'at_s': 3.0},
             'at_unix': START['unix'] + 3, 'station_frames': 8000})


def pulse_run(folder: str, shots: int = 3, frames: int = 512) -> str:
    """A pulse run: one inputs stream of `shots` shots one after another."""
    rows = [row(1, 'control', description='Control'),
            row(2, 'drive', 'V', description='Drive')]
    values = np.concatenate([_wave(frames, 1, seed=float(k)) * (k + 1)
                             for k in range(shots)])
    shot_list = [{'first_frame': k * frames, 'frames': frames, 'pre_frames': 64,
                  'station_frame': 10000 * (k + 1) + 64, 'level_db': -6.0 * (shots - 1 - k)}
                 for k in range(shots)]
    return write_run(
        folder, _manifest('classical_shock'),
        {'revision': 1, 'sha256': 'd' * 64, 'rows': rows}, {'config': {}, 'resolved': {}},
        streams={'inputs': ({'kind': 'inputs', 'fs_hz': FS,
                             'channels': [{'index': 1, 'unit': 'g'}],
                             'first_station_frame': 10000, 'frames': shots * frames,
                             'gaps': [], 'shots': shot_list, 'complete': True,
                             'fault': None}, values)},
        log=[], end={'run_uid': 'uid-classical_shock-0001',
                     'end': {'outcome': 'completed', 'message': 'Done.', 'at_s': 9.0}})


def unfinished_run(folder: str, frames: int = 2048) -> str:
    """A Random run cut off: not complete, no end, a frame of a stream and
    the last line of the log cut short."""
    random_run(folder, frames, complete=False)
    with open(os.path.join(folder, 'streams', 'inputs', 'telemetry.f32'), 'ab') as f:
        f.write(b'\x00' * 6)
    with open(os.path.join(folder, 'log.jsonl'), 'ab') as f:
        f.write(b'{"seq": 6, "at_unix": 17942')
    return folder


#: a Playback run's drive folder, as its setup names it
PLAYBACK_DRIVE = {'folder': '/home/someone/Hertzforge_Data/drives/sweep',
                  'sha256': 'd' * 64}


def playback_run(folder: str, pre: int = 512, play: int = 2048, post: int = 1024) -> str:
    """A Playback run, as `docs/run-folder.md` lays a run out: two inputs
    and two drives as continuous streams over the pre-roll, the play and
    the post-roll; the drive file named in the setup with its pinned
    SHA-256 and sidecar; the check report (`playback_check`, per drive
    row, with envelopes) and the outcome (`playback_outcome`, words only)
    as results; and a log of the run's events."""
    rows = [row(1, 'monitor', description='Table top', extra={'node': '1', 'direction': 'Z+'}),
            row(2, 'monitor', description='Fixture', extra={'node': '2', 'direction': 'X+'}),
            row(3, 'drive', 'V', description='Shaker 1',
                physical={'device': 'Dev1', 'channel': 'ao0'}),
            row(4, 'drive', 'V', description='Shaker 2',
                physical={'device': 'Dev1', 'channel': 'ao1'}),
            row(5, 'monitor', enabled=False, description='Spare', physical=None)]
    sidecar = {'format': 'hertzforge-drive', 'version': 1, 'rate_hz': FS, 'samples': play,
               'channels': [{'name': 'Drive 1', 'unit': 'V', 'source': 'column:2'},
                            {'name': 'Drive 2', 'unit': 'V', 'source': 'column:3'}],
               'sha256': PLAYBACK_DRIVE['sha256'],
               'source': {'path': '/home/someone/tests/sweep.csv', 'sha256': 'e' * 64,
                          'format': 'delimited',
                          'selections': [{'signal': 'column:2', 'name': 'Drive 1'},
                                         {'signal': 'column:3', 'name': 'Drive 2'}]},
               'fixes': [], 'tool': 'hertzforge 0.0.0', 'created_utc': '2026-11-15T00:00:00Z'}
    config = {'title': 'Golden playback',
              'drive': dict(PLAYBACK_DRIVE),
              'map': [3, 4], 'level_db': -12.0, 'ceiling_v': 5.0, 'pre_roll_s': pre / FS,
              'post_roll_s': post / FS, 'capture': {'run_name': 'playback'}}
    setup = {'config': config,
             'resolved': {'drive': sidecar, 'rate_hz': FS, 'level': 10 ** (-12 / 20)},
             'derived': {'frames': pre + play + post}}
    frames = pre + play + post
    drive = np.zeros((frames, 2), dtype=np.float32)
    drive[pre:pre + play] = _wave(play, 2) * np.float32(10 ** (-12 / 20))
    first = 1000
    common = {'fs_hz': FS, 'first_station_frame': first, 'frames': frames, 'gaps': [],
              'complete': True, 'fault': None}
    peaks = np.array([0.5, 0.75])
    check = {'kind': 'playback_check', 'channels': [3, 4], 'passed': True,
             'drive': dict(PLAYBACK_DRIVE),
             'checks': [{'channel': 3, 'passed': True, 'reason': ''},
                        {'channel': 4, 'passed': True, 'reason': ''}],
             'tolerances': {'ends_of_peak': 0.005, 'velocity_of_peak': 0.01,
                            'displacement_of_peak': 0.01},
             'units': {'peak': 'V', 'start': 'V', 'end': 'V', 'velocity_end': 'V s',
                       'velocity_peak': 'V s', 'displacement_end': 'V s^2',
                       'displacement_peak': 'V s^2', 'envelope': 'V'}}
    check_arrays = {'peak': peaks, 'start': np.zeros(2), 'end': np.zeros(2),
                    'velocity_end': np.array([1e-9, -2e-9]),
                    'velocity_peak': np.array([2e-3, 1.5e-3]),
                    'displacement_end': np.array([3e-12, 1e-12]),
                    'displacement_peak': np.array([1e-5, 4e-6]),
                    'envelope': np.stack([np.stack([-peaks[c] * np.ones(16),
                                                    peaks[c] * np.ones(16)]) for c in range(2)])}
    outcome = {'kind': 'playback_outcome', 'samples_played': play, 'outcome': 'completed',
               'reason': ''}
    kinds = [('check', 'the drive file passed its check'), ('state', 'armed'),
             ('state', 'pre-roll'), ('state', 'playing'), ('state', 'post-roll'),
             ('state', 'completed')]
    return write_run(
        folder, _manifest('playback'), {'revision': 5, 'sha256': 'f' * 64, 'rows': rows},
        setup,
        streams={'inputs': ({'kind': 'inputs', 'channels': [{'index': 1, 'unit': 'g'},
                                                            {'index': 2, 'unit': 'g'}],
                             **common}, _wave(frames, 2, seed=0.3)),
                 'drives': ({'kind': 'drives', 'channels': [{'index': 3, 'unit': 'V'},
                                                            {'index': 4, 'unit': 'V'}],
                             'output_latency_frames': 0, 'output_latency': 'declared',
                             **common}, drive)},
        results={'playback_check': (check, check_arrays, None),
                 'playback_outcome': (outcome, {}, None)},
        log=[{'seq': k, 'at_unix': START['unix'] + k, 'frame': first + 256 * k, 'kind': kind,
              'text': text} for k, (kind, text) in enumerate(kinds)],
        end={'run_uid': 'uid-playback-0001',
             'end': {'outcome': 'completed', 'message': 'The drive played to its end.',
                     'at_s': frames / FS},
             'at_unix': START['unix'] + 6, 'station_frames': frames})


#: the structure a Modal Test run measures: three modes over six points, mostly on Z, a
#: little on X and Y
MODAL_FREQUENCIES = (20.0, 45.0, 80.0)
MODAL_DAMPING = (0.03, 0.02, 0.015)
MODAL_SHAPES = np.array([[1.0, 0.9, 0.6, 0.2, -0.3, -0.8],
                         [0.8, 0.2, -0.7, -1.0, -0.2, 0.6],
                         [0.5, -0.6, -0.4, 0.7, 0.3, -0.9]])
#: each axis's shapes, by the axis's letter
MODAL_AXES = {'Z': MODAL_SHAPES, 'X': 0.4 * np.roll(MODAL_SHAPES, 1, axis=1),
              'Y': 0.25 * np.roll(MODAL_SHAPES, 2, axis=1)}
#: its spectral lines (Hz) and its frames' samples
MODAL_STEP, MODAL_LINES, MODAL_BLOCK = 0.25, 601, 4096


def _modes(modes):
    return modes if modes is not None else (MODAL_FREQUENCIES, MODAL_DAMPING)


def modal_poles(shift: float = 0.0, modes=None) -> list[tuple[float, float]]:
    """Each mode's (natural frequency in Hz, damping ratio), its pole moved
    left by `shift` 1/s, as an exponential window of time constant 1/shift
    moves it (the damped frequency kept); of `modes` (frequencies, damping)
    in place of the structure's when given."""
    out = []
    for fn, zeta in zip(*_modes(modes), strict=True):
        wn = 2 * np.pi * fn
        decay, damped = zeta * wn + shift, wn * np.sqrt(1 - zeta ** 2)
        out.append((float(np.hypot(decay, damped) / (2 * np.pi)),
                    float(decay / np.hypot(decay, damped))))
    return out


def _dof(at) -> tuple[int, str, float]:
    """(point, axis, sign) of a point number (on Z+) or a (point, 'Z-') pair."""
    if isinstance(at, (int, np.integer)):
        return int(at), 'Z', 1.0
    point, direction = at
    return int(point), direction[0], -1.0 if direction.endswith('-') else 1.0


def modal_receptance(f: np.ndarray, j, k, shift: float = 0.0, modes=None) -> np.ndarray:
    """The structure's displacement at `j` per force at `k`, m/N, at `f` Hz,
    each a point from 0 (on Z+) or a (point, direction) pair ('X+', 'Z-'):
    symmetric in j and k; with its poles moved left by `shift` 1/s when
    given (`modal_poles`)."""
    w = 2 * np.pi * np.asarray(f, dtype=np.float64)
    (pj, aj, sj), (pk, ak, sk) = _dof(j), _dof(k)
    out = np.zeros(w.shape, dtype=np.complex128)
    for r, (fn, zeta) in enumerate(modal_poles(shift, modes)):
        wn = 2 * np.pi * fn
        out += (sj * MODAL_AXES[aj][r, pj] * sk * MODAL_AXES[ak][r, pk]
                / (wn ** 2 - w ** 2 + 2j * zeta * wn * w))
    return out


def modal_accelerance(f: np.ndarray, j, k, shift: float = 0.0, modes=None) -> np.ndarray:
    """The structure's acceleration at `j` per force at `k`, (m/s^2)/N
    (`modal_receptance`)."""
    w = 2 * np.pi * np.asarray(f, dtype=np.float64)
    return -(w ** 2) * modal_receptance(f, j, k, shift, modes)


def _modal_channel(index: int, at, unit: str) -> dict[str, Any]:
    point, axis, sign = _dof(at)
    return {'index': index, 'node': point + 1, 'direction': f'{axis}{"-" if sign < 0 else "+"}',
            'unit': unit}


def modal_test_run(folder: str, layout: str = 'roving_hammer', *, hits: int = 3,
                   measured: Sequence[int] | None = None, again: Sequence[int] = (),
                   frames: bool = True, complete: bool = True,
                   tau_s: float | None = None, torn: Sequence[int] = (),
                   unfinished: Sequence[int] = (), directions: Mapping[int, str] = {},
                   modes=None, noise_db: float | None = None) -> str:
    """A Modal Test run of the structure above, as the wire notes are
    assumed to lay one out (`hertzforge.io.modal_run.WIRE`).

    `layout` is 'roving_hammer' (an accelerometer fixed at point 1, the
    hammer at every point), 'triaxial_roving_hammer' (triaxial
    accelerometers fixed at points 1 to 3, nine responses, the hammer at
    every point), 'fixed_hammer' (six accelerometers, the hammer at point
    1, one measurement) or 'two_shakers' (six accelerometers, shakers at
    points 1 and 4, one measurement, multiple coherence).
    `measured` picks the roving hammer's points (from 0; every point when
    omitted); `again` names points measured a second time, after the rest
    (with nothing roving, the layout measured again once per entry), with
    their values doubled so the later set is told apart. `unfinished`
    names points selected again after that, with only a trial and a
    rejected hit in the frame lines and no records. `torn` names
    measurements (from 0) whose coherence and reference spectra lines were
    never written, as a run killed between them leaves it. `directions`
    gives a roving hammer's direction at a point ('Z-' to hit it from
    below). Each impact measurement has `hits` accepted frames, a rejected
    double hit and a trial hit in its frame lines. `complete` False leaves
    the run killed: no end, the outcome not written. `tau_s` names an
    exponential window on every channel in each record's window record,
    and the FRFs are written with the structure's poles moved left by
    1 / tau_s, as the window moves them (`modal_poles`). `modes` puts
    (frequencies, damping) in place of the structure's; `noise_db` adds
    complex noise that many dB below each FRF's own RMS, seeded."""
    lines = np.arange(MODAL_LINES) * MODAL_STEP
    pre = 64
    rng = np.random.default_rng(11)
    if layout in ('roving_hammer', 'triaxial_roving_hammer'):
        if layout == 'roving_hammer':
            fixed = [(1, (0, 'Z+'))]
        else:
            fixed = [(1 + 3 * p + a, (p, axis)) for p in range(3)
                     for a, axis in enumerate(('X+', 'Y+', 'Z+'))]
        hammer = len(fixed) + 1
        rows = [row(i, 'monitor', description=f'Fixed accelerometer {i}', node=str(at[0] + 1),
                    direction=at[1], analysis_role='response') for i, at in fixed]
        rows.append(row(hammer, 'reference', 'N', description='Hammer', node='1',
                        direction='Z+'))
        points = list(range(6)) if measured is None else list(measured)
        plan = [(p, fixed, [(hammer, (p, directions.get(p, 'Z+')))])
                for p in [*points, *again, *unfinished]]
        roving = {'channels': [hammer], 'points': [
            {'node': p + 1, 'direction': directions.get(p, 'Z+')} for p in range(6)]}
        mode = 'impact'
    else:
        refs = [0] if layout == 'fixed_hammer' else [0, 3]
        rows = [row(k + 1, 'monitor', description=f'Accelerometer {k + 1}', node=str(k + 1),
                    direction='Z+', analysis_role='response') for k in range(6)]
        rows += [row(7 + n, 'reference', 'N', description=f'Force {n + 1}', node=str(p + 1),
                     direction='Z+') for n, p in enumerate(refs)]
        plan = [(None, [(k + 1, k) for k in range(6)],
                 [(7 + n, p) for n, p in enumerate(refs)])]
        plan += [(None, plan[0][1], plan[0][2]) for _ in again]
        roving = {'channels': [], 'points': []}
        mode = 'impact' if layout == 'fixed_hammer' else 'shaker'
    selected_again = set(range(len(plan) - len(unfinished), len(plan))) if unfinished else set()
    window: dict[str, Any] = {
        'reference': ({'kind': 'force', 'width': 0.05, 'fall': 0.1} if mode == 'impact'
                      else {'kind': 'hann'}),
        'response': {'kind': 'boxcar' if mode == 'impact' else 'hann'}}
    if tau_s is not None:
        window['exponential'] = {'tau_s': tau_s, 'end': float(
            np.exp(-(MODAL_BLOCK - 1) / (tau_s * FS))), 'decay_rate_per_s': 1.0 / tau_s}
    averages = hits if mode == 'impact' else 40
    frf_records, coherence_records, spectra_records = [], [], []
    frf_values, coherence_values, spectra_values, condition = [], [], [], []
    frame_lines, stream_frames = [], []
    seen: set = set()
    for m, (point, responses, references) in enumerate(plan):
        later = 2.0 if (point if point is not None else 'all') in seen else 1.0
        if m in selected_again:
            later = 0.0
        seen.add(point if point is not None else 'all')
        unit = {**{i: 'g' for i, _p in responses}, **{i: 'N' for i, _p in references}}
        common = {'measurement': m, 'point': point,
                  'responses': [_modal_channel(i, p, unit[i]) for i, p in responses],
                  'references': [_modal_channel(i, p, unit[i]) for i, p in references]}
        shift = 0.0 if tau_s is None else 1.0 / tau_s
        values = np.array([[modal_accelerance(lines, j, k, shift, modes) / G * later
                            for _rk, k in references] for _ri, j in responses])
        if noise_db is not None:
            level = np.sqrt(np.mean(np.abs(values) ** 2, axis=-1, keepdims=True))
            values = values + level * 10 ** (noise_db / 20) / np.sqrt(2) * (
                rng.standard_normal(values.shape) + 1j * rng.standard_normal(values.shape))
        if m not in selected_again:
            frf_records.append({**common, 'estimator': 'H1', 'averages': averages,
                                'window': window})
            frf_values.append(values)
        coherence = np.ones((len(responses), len(references), MODAL_LINES)) * 0.99
        coherence[..., 0] = 0.0
        if m not in torn and m not in selected_again:
            if len(references) > 1:
                coherence_records.append({**common, 'coherence': 'multiple'})
                coherence_values.append(coherence[:, 0])
            else:
                coherence_records.append({**common, 'coherence': 'ordinary'})
                coherence_values.append(coherence)
            spectra_records.append(common)
            spectra_values.append(np.ones((len(references), MODAL_LINES)) * 1e-3)
            condition.append(np.full(MODAL_LINES, 3.0))
        if mode != 'impact' or not frames:
            continue
        # the frames: a half-sine hit at the trigger, the response by the receptance
        n = np.arange(MODAL_BLOCK)
        f = np.fft.rfftfreq(MODAL_BLOCK, 1 / FS)
        _ri, k = references[0]
        kinds = (('trial', 'double') if m in selected_again
                 else ('trial', 'accepted', 'double', *(['accepted'] * (hits - 1))))
        for a, kind in enumerate(kinds):
            pulse = np.where((n >= pre) & (n < pre + 5), np.sin(np.pi * (n - pre) / 5), 0.0)
            pulse = pulse * (100.0 + 10 * a)
            if kind == 'double':
                pulse = pulse + np.roll(pulse, 40)
            spectrum = np.fft.rfft(pulse)
            columns = [np.fft.irfft(spectrum * modal_accelerance(f, j, k, 0.0, modes) / G
                                    * (later or 1.0), MODAL_BLOCK) for _ri, j in responses]
            columns.append(pulse)
            offset = sum(len(s) for s in stream_frames)
            stream_frames.append(np.stack(columns, axis=1))
            frame_lines.append({
                'measurement': m, 'point': point, 'average': None if kind != 'accepted' else
                sum(1 for line in frame_lines if line['measurement'] == m
                    and line['accepted']) + 1,
                'stream': 'inputs', 'offset': offset, 'length': MODAL_BLOCK,
                'trigger_sample': pre, 'accepted': kind == 'accepted',
                'trial': kind == 'trial',
                'reason': {'trial': 'trial hit', 'double': 'double hit'}.get(kind, ''),
                'checks': {'double_hit': kind == 'double', 'overload': False},
                'at_unix': START['unix'] + 10 * m + a})
    results: dict[str, Any] = {}
    axis = {'start': 0.0, 'step': MODAL_STEP, 'unit': 'Hz'}
    if frf_records:
        results['modal_frf'] = ({'kind': 'modal_frf', 'abscissa': axis},
                                {'values': np.array(frf_values)}, frf_records)
    if coherence_records:
        results['modal_coherence'] = ({'kind': 'modal_coherence', 'abscissa': axis},
                                      {'values': np.array(coherence_values)},
                                      coherence_records)
        spectra = {'values': np.array(spectra_values)}
        if len(plan[0][2]) > 1:
            spectra['condition'] = np.array(condition)
        results['modal_reference_spectra'] = (
            {'kind': 'modal_reference_spectra', 'abscissa': axis}, spectra, spectra_records)
    if frame_lines:
        results['modal_frames'] = ({'kind': 'modal_frames'}, {}, frame_lines)
    if complete:
        results['modal_outcome'] = ({'kind': 'modal_outcome', 'points_planned':
                                     len(roving['points']),
                                     'points_measured': len({p for p, _r, _f in plan
                                                             if p is not None}),
                                     'frames': len(frame_lines),
                                     'frames_accepted': sum(line['accepted']
                                                            for line in frame_lines),
                                     'ended': 'completed', 'reason': ''}, {}, None)
    config = {'mode': mode, 'block': MODAL_BLOCK,
              'band': {'low_hz': 0.0, 'high_hz': lines[-1]}, 'estimator': 'H1',
              'averages': averages, 'roving': roving,
              'impact': {'trigger_level': 0.1, 'pre_trigger': pre, 'window': window},
              'shaker': {'window': 'hann', 'overlap': 0.5} if mode == 'shaker' else None}
    channels = [r['index'] for r in rows]
    streams: dict[str, Any] = {}
    if stream_frames:
        values = np.concatenate(stream_frames)
        order = [i for i, _p in plan[0][1]] + [i for i, _p in plan[0][2]]
        streams['inputs'] = ({'kind': 'inputs', 'channels': [
            {'index': i, 'unit': 'g' if i in [r for r, _p in plan[0][1]] else 'N'}
            for i in order], 'fs_hz': FS, 'first_station_frame': 1000, 'frames': len(values),
            'gaps': [], 'complete': complete, 'fault': None}, values)
    assert sorted(channels) == channels
    return write_run(
        folder, _manifest('modal_test', complete=complete),
        {'revision': 3, 'sha256': 'a' * 64, 'rows': rows}, {'config': config},
        streams=streams, results=results,
        log=[{'seq': 0, 'at_unix': START['unix'], 'frame': 1000, 'kind': 'state',
              'text': 'measuring'}],
        end=None if not complete else {
            'run_uid': 'uid-modal_test-0001',
            'end': {'outcome': 'completed', 'message': 'Every point was measured.',
                    'at_s': 60.0}, 'at_unix': START['unix'] + 60, 'station_frames': 61440})
