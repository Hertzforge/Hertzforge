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
