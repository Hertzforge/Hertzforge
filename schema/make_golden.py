"""Write the golden frames every codec in the suite is tested against.

Standard library only, so the files can be regenerated anywhere:

    python3 schema/make_golden.py

Each `*.bin` is one binary frame exactly as it crosses the socket. Each
`*.expected.json` is the same message decoded, with every array written as
{"dtype", "shape", "values"} so a test in any language can compare against it.
"""

from __future__ import annotations

import json
import pathlib
import struct

HERE = pathlib.Path(__file__).resolve().parent / 'golden'
CODES = {'f32': 'f', 'f64': 'd', 'i32': 'i', 'u32': 'I', 'u8': 'B'}
SIZES = {'f32': 4, 'f64': 8, 'i32': 4, 'u32': 4, 'u8': 1}


def encode(message: dict, arrays: dict[str, tuple[str, list[int], list]]) -> bytes:
    """Frame `message`, whose array values are {"$array": id} references."""
    payload = bytearray()
    entries = []
    for ident, (dtype, shape, values) in arrays.items():
        while len(payload) % 8:
            payload.append(0)
        data = struct.pack(f'<{len(values)}{CODES[dtype]}', *values)
        entries.append({'id': ident, 'dtype': dtype, 'shape': shape,
                        'offset': len(payload), 'nbytes': len(data)})
        payload += data
    header = json.dumps({**message, 'arrays': entries}, ensure_ascii=False,
                        separators=(',', ':')).encode('utf-8')
    header += b' ' * ((-(8 + len(header))) % 8)
    return b'SDS1' + struct.pack('<I', len(header)) + header + bytes(payload)


def expected(message: dict, arrays: dict) -> dict:
    def walk(node):
        if isinstance(node, dict):
            if set(node) == {'$array'}:
                dtype, shape, values = arrays[node['$array']]
                return {'dtype': dtype, 'shape': shape, 'values': values}
            return {key: walk(value) for key, value in node.items()}
        if isinstance(node, list):
            return [walk(item) for item in node]
        return node
    return walk(message)


def write(name: str, message: dict, arrays: dict) -> None:
    (HERE / f'{name}.bin').write_bytes(encode(message, arrays))
    (HERE / f'{name}.expected.json').write_text(
        json.dumps(expected(message, arrays), ensure_ascii=False, indent=1) + '\n',
        encoding='utf-8')


# --- hardware and Random control (PROTOCOL.md, "Hardware and Random Control") ---
#
# Whole numbers in a binary header are written as integers: a JSON writer
# in each language must give the same bytes, and only integers are spelled
# the same way by all three.

#: Four lines of a 6.25 Hz grid, every value exact in binary, and their bins.
LIVE_LINES = [100.0, 106.25, 112.5, 118.75]
LIVE_EDGES = [96.875, 103.125, 109.375, 115.625, 121.875]
LIVE_SERIES = [
    ('Control', 'measured', [0.0398, 0.0402, 0.0411, 0.0389]),
    ('Reference', 'reference', [0.04, 0.04, 0.04, 0.04]),
    ('Alarm high', 'tolerance', [0.0798, 0.0798, 0.0798, 0.0798]),
    ('Alarm low', 'tolerance', [0.02005, 0.02005, 0.02005, 0.02005]),
    ('Abort high', 'abort', [0.1592, 0.1592, 0.1592, 0.1592]),
    ('Abort low', 'abort', [0.01005, 0.01005, 0.01005, 0.01005]),
]
LIVE_DRIVE = [0.00021, 0.00025, 0.00031, 0.00028]


def _stepped_series(arrays: dict, name: str, role: str, values: list) -> dict:
    """A series drawn flat across its bins, its arrays named in traversal order."""
    refs = []
    for data in (LIVE_LINES, values, LIVE_EDGES):
        ident = f'a{len(arrays)}'
        arrays[ident] = ('f64', [len(data)], data)
        refs.append({'$array': ident})
    return {'name': name, 'role': role, 'x': refs[0], 'y': refs[1],
            'shape': 'steps', 'edges': refs[2]}


def _random_live_build() -> tuple[dict, dict]:
    arrays: dict = {}
    psd = {'label': 'PSD', 'unit': 'g²/Hz', 'scale': 'log'}
    freq = {'label': 'Frequency', 'unit': 'Hz', 'scale': 'log'}
    control = {'view': 'spectrum', 'title': 'Control', 'x': freq, 'y': psd,
               'series': [_stepped_series(arrays, *s) for s in LIVE_SERIES],
               'link': 'frequency'}
    drive = {'view': 'spectrum', 'title': 'Drive', 'x': freq,
             'y': {'label': 'PSD', 'unit': 'V²/Hz', 'scale': 'log'},
             'series': [_stepped_series(arrays, 'Drive', 'drive', LIVE_DRIVE)],
             'link': 'frequency'}
    reading = {'view': 'reading', 'title': 'Run', 'rows': [
        {'label': 'Control rms', 'value': 6.02, 'unit': 'g', 'status': 'pass'},
        {'label': 'Reference rms', 'value': 6.06, 'unit': 'g', 'status': 'info'},
        {'label': 'Drive rms', 'value': 0.412, 'unit': 'V', 'status': 'info'},
        {'label': 'Level', 'value': 'full level', 'unit': None, 'status': 'info'},
        {'label': 'Lines in alarm', 'value': '2 of 317', 'unit': None, 'status': 'warn'},
        {'label': 'Elapsed', 'value': 63.5, 'unit': 's', 'status': 'info'},
    ]}
    message = {
        'type': 'event', 'topic': 'random.live', 'seq': 318,
        'data': {
            'run_id': 'r-0007', 'source': 'HARDWARE', 'class': 'ni_real',
            'state': 'running', 'phase': 'full_level', 'mode': 'auto',
            'elapsed_s': 63.5, 'level_db': 0, 'commanded_db': 0, 'at_level_s': 12.5,
            'remaining_at_level_s': 287.5, 'level_s': 300, 'loop': 118,
            'control_rms_g': 6.02, 'reference_rms_g': 6.06, 'control_error_db': -0.06,
            'drive_rms_v': 0.412,
            'lines': {'in_band': 317, 'alarm': 2, 'abort': 0},
            'channels': [
                {'index': 1, 'description': 'Control', 'role': 'control', 'rms': 6.02,
                 'unit': 'g', 'overload': False},
                {'index': 2, 'description': 'Fixture corner', 'role': 'monitor', 'rms': 7.35,
                 'unit': 'g', 'overload': False},
            ],
            'alarms': [{'kind': 'envelope', 'lines': 2, 'worst_db': 3.4}],
            'views': [control, drive, reading],
        },
    }
    return message, arrays


def random_live() -> dict:
    return _random_live_build()[0]


#: Two channels' PSDs on the four lines; channel 2 is limited on the middle two
DATA_CH1 = [0.0398, 0.0402, 0.0411, 0.0389]
DATA_CH2 = [0.051, 0.062, 0.0705, 0.048]
DATA_LIMIT_LINES = [106.25, 112.5]
DATA_LIMIT_EDGES = [103.125, 109.375, 115.625]
DATA_LIMIT = [0.06, 0.06]
#: The last loop's samples of channel 1 at 8192 Hz (every value exact in binary)
DATA_TIME_X = [0.0, 0.0001220703125, 0.000244140625, 0.0003662109375]
DATA_TIME_Y = [0.5, -1.25, 2.0, -0.75]


def _series(arrays: dict, name: str, role: str, x: list, y: list, edges=None) -> dict:
    """A series, its arrays named in traversal order; stepped when given edges."""
    refs = []
    for data in (x, y) + ((edges,) if edges is not None else ()):
        ident = f'a{len(arrays)}'
        arrays[ident] = ('f64', [len(data)], data)
        refs.append({'$array': ident})
    series = {'name': name, 'role': role, 'x': refs[0], 'y': refs[1]}
    if edges is not None:
        series.update({'shape': 'steps', 'edges': refs[2]})
    return series


def _random_data_build() -> tuple[dict, dict]:
    """`random.data` for a subscription of a control plot, a response plot over
    channels 1 and 2 (channel 2 limited) and a time plot of channel 1: what the
    service's own builder gives for that run (the Rust golden test builds it so)."""
    arrays: dict = {}
    freq = {'label': 'Frequency', 'unit': 'Hz', 'scale': 'log'}
    psd = {'label': 'PSD', 'unit': 'g²/Hz', 'scale': 'log'}
    control = {'view': 'spectrum', 'title': 'Control', 'x': freq, 'y': psd,
               'series': [_stepped_series(arrays, *s) for s in LIVE_SERIES],
               'link': 'frequency'}
    ch1 = {'view': 'spectrum', 'title': 'Channel 1: Control', 'x': freq, 'y': psd,
           'series': [_series(arrays, 'Channel 1', 'measured', LIVE_LINES, DATA_CH1,
                              LIVE_EDGES)],
           'link': 'frequency'}
    ch2 = {'view': 'spectrum', 'title': 'Channel 2: Fixture corner', 'x': freq, 'y': psd,
           'series': [_series(arrays, 'Channel 2', 'measured', LIVE_LINES, DATA_CH2,
                              LIVE_EDGES),
                      _series(arrays, 'Limit', 'limit', DATA_LIMIT_LINES, DATA_LIMIT,
                              DATA_LIMIT_EDGES)],
           'link': 'frequency'}
    time = {'view': 'spectrum', 'title': 'Channel 1: Control',
            'x': {'label': 'Time', 'unit': 's', 'scale': 'linear'},
            'y': {'label': 'Amplitude', 'unit': 'g', 'scale': 'linear'},
            'series': [_series(arrays, 'Channel 1', 'measured', DATA_TIME_X, DATA_TIME_Y)]}
    message = {
        'type': 'event', 'topic': 'random.data', 'seq': 319,
        'data': {
            'run_id': 'r-0007', 'source': 'HARDWARE', 'class': 'ni_real',
            'state': 'running', 'phase': 'full_level', 'mode': 'auto',
            'elapsed_s': 63.5, 'level_db': 0, 'commanded_db': 0, 'at_level_s': 12.5,
            'remaining_at_level_s': 287.5, 'level_s': 300, 'loop': 118,
            'control_rms_g': 6.02, 'reference_rms_g': 6.06, 'control_error_db': -0.06,
            'drive_rms_v': 0.412,
            'lines': {'in_band': 4, 'alarm': 1, 'abort': 0},
            'channels': [
                {'index': 1, 'description': 'Control', 'role': 'control', 'rms': 6.02,
                 'unit': 'g', 'overload': False},
                {'index': 2, 'description': 'Fixture corner', 'role': 'monitor', 'rms': 7.35,
                 'unit': 'g', 'overload': False},
            ],
            'alarms': [{'kind': 'envelope', 'lines': 1, 'worst_db': 3.4}],
            'plots': [
                {'id': 'ctl', 'quantity': 'control', 'channels': None, 'views': [control]},
                {'id': 'lim', 'quantity': 'response', 'channels': [1, 2], 'views': [ch1, ch2]},
                {'id': 't1', 'quantity': 'time', 'channels': [1], 'views': [time]},
            ],
        },
    }
    return message, arrays


def random_data() -> dict:
    return _random_data_build()[0]


def random_data_arrays() -> dict:
    return _random_data_build()[1]


def random_live_arrays() -> dict:
    return _random_live_build()[1]


def _row(index, description, role, physical, units, range_max, accel_abort=None):
    return {'index': index, 'enabled': True, 'description': description, 'role': role,
            'physical': physical, 'interface': 'single_ended', 'iepe_bias_v': None,
            'units': units, 'sensitivity_mv_per_eu': 100 if units == 'g' else 1000,
            'range_max': range_max, 'polarity': 'positive', 'control_group': None,
            'abort': {'accel_g': accel_abort, 'velocity_ips': None, 'displacement_mil': None}}


RANDOM_CONFIG = {
    'title': 'Qualification Random',
    'profile': {
        'breakpoints': [
            {'freq_hz': 20, 'psd_g2_hz': 0.01},
            {'freq_hz': 80, 'slope_db_oct': 3},
            {'freq_hz': 350, 'psd_g2_hz': 0.04, 'alarm_db': [2, 2]},
            {'freq_hz': 2000, 'slope_db_oct': -3},
        ],
        'alarm_db': [3, 3], 'abort_db': [6, 6],
    },
    'acquisition': {'fs_hz': 8192, 'n_pts': 2048, 'n_avg': 4},
    'schedule': {'mode': 'auto',
                 'levels': [{'db': -12, 'dwell_s': 30}, {'db': -6, 'dwell_s': 30}, {'db': 0}],
                 'full_level_s': 300},
    'control': {'composite': 'weighted_avg', 'control_dof': 120, 'damping': 4,
                'max_lines_out': 10, 'max_loops_out': 5, 'max_sigma': 5,
                'drive_ceiling_vrms': 2.5, 'drive_abort_vrms': None,
                'review_after_sysid': True,
                'pretest': {'averages': 4, 'level_db': -20, 'max_vrms': 0.5, 'start_vrms': 0.01,
                            'shaped': False},
                'level_ramp_s': 3, 'shutdown_ramp_s': 3},
    'limits': [{'channel': 2, 'kind': 'response',
                'rows': [{'freq_hz': 20, 'ceiling': 0.2}, {'freq_hz': 2000, 'ceiling': 0.2}]}],
    'feasibility': None,
}

CONFIGURED_STATE = {
    'run_id': 'r-0007', 'state': 'configured', 'phase': 'idle',
    'title': 'Qualification Random', 'source': 'SIM',
    'station': {'kind': 'sim', 'class': 'sim', 'devices': []},
    'grid': {'fs_hz': 8192, 'n_pts': 2048, 'df_hz': 4, 'lines': 496, 'loop_s': 1,
             'actual': False},
    'clock': None, 'mode': 'auto', 'levels_db': [-12, -6, 0], 'level_db': None,
    'full_level_s': 300, 'elapsed_s': None, 'at_level_s': None,
    'remaining_at_level_s': None, 'alarm': False, 'feasibility': None, 'end': None,
}

ABORTED_STATE = {
    'run_id': 'r-0007', 'state': 'finished', 'phase': 'aborted',
    'title': 'Qualification Random', 'source': 'HARDWARE',
    'station': {'kind': 'ni', 'class': 'ni_real',
                'devices': [{'id': 'cDAQ1Mod1', 'serial': '01A2B3C4'},
                            {'id': 'cDAQ1Mod2', 'serial': '01A2B3C5'}]},
    'grid': {'fs_hz': 51200, 'n_pts': 8192, 'df_hz': 6.25, 'lines': 317, 'loop_s': 0.64,
             'actual': True},
    'clock': 'shared', 'mode': 'auto', 'levels_db': [-12, -6, 0], 'level_db': None,
    'full_level_s': 300, 'elapsed_s': 131.2, 'at_level_s': 42.5,
    'remaining_at_level_s': None, 'alarm': False,
    'feasibility': {'verdict': 'not_evaluated',
                    'message': 'No shaker was given, so the profile was not checked against '
                               'shaker ratings.'},
    'end': {'outcome': 'aborted',
            'message': 'Envelope abort: 14 lines outside the abort band, worst +7.2 dB, '
                       'at 0.0 dB.',
            'at_s': 131.2, 'level_db': 0,
            'trip': {'kind': 'envelope', 'abort_lines': 14, 'worst_db': 7.2,
                     'at_level_db': 0, 'at_loop': 212}},
}

SUBSCRIBE_PLOTS = [
    {'id': 'ctl', 'quantity': 'control'},
    {'id': 'lim', 'quantity': 'response', 'channels': 'limited'},
    {'id': 't2', 'quantity': 'time', 'channels': [2, 3], 'points': 1024},
]

TEXT_GOLDENS = {
    'request_random_subscribe': {
        'type': 'request', 'id': 21, 'method': 'random.subscribe',
        'params': {'rate_hz': 5, 'plots': [
            {'id': 'ctl', 'quantity': 'control'},
            {'id': 'lim', 'quantity': 'response', 'channels': 'limited'},
            {'id': 't2', 'quantity': 'time', 'channels': [2, 3]},
        ]},
    },
    'response_random_subscribe': {
        'type': 'response', 'id': 21,
        'result': {'default': False, 'rate_hz': 5, 'plots': SUBSCRIBE_PLOTS,
                   'quantities': ['control', 'drive', 'response', 'time', 'coherence',
                                  'readings']},
    },
    'request_random_configure': {
        'type': 'request', 'id': 21, 'method': 'random.configure',
        'params': {'config': RANDOM_CONFIG},
    },
    'response_random_configure': {
        'type': 'response', 'id': 21,
        'result': {
            'state': CONFIGURED_STATE,
            'derived': {'grms_g': 6.06, 'velocity_rms_ips': 2.31, 'displacement_rms_mil': 21.4,
                        'band_hz': [20, 2000],
                        'grid': {'fs_hz': 8192, 'n_pts': 2048, 'df_hz': 4, 'lines': 496,
                                 'loop_s': 1, 'actual': False},
                        'control_channels': [1]},
            'warnings': [],
        },
    },
    'response_station_list': {
        'type': 'response', 'id': 3,
        'result': {
            'stations': [
                {'kind': 'sim', 'title': 'Simulator', 'available': True, 'class': 'sim',
                 'inputs': 8, 'outputs': 1, 'clock': 'shared'},
                {'kind': 'ni', 'title': 'NI-DAQmx', 'available': True, 'unavailable': None,
                 'scanned_unix': 1793577600,
                 'devices': [
                     {'id': 'cDAQ1', 'product': 'cDAQ-9178', 'serial': '01C2D3E4',
                      'simulated': False, 'parent': None, 'slot': None,
                      'timing_family': 'cDAQ1', 'inputs': [], 'outputs': []},
                     {'id': 'cDAQ1Mod1', 'product': 'NI 9234', 'serial': '01A2B3C4',
                      'simulated': False, 'parent': 'cDAQ1', 'slot': 1,
                      'timing_family': 'cDAQ1',
                      'inputs': [{'id': f'ai{k}', 'label': f'ai{k}'} for k in range(4)],
                      'outputs': []},
                     {'id': 'cDAQ1Mod2', 'product': 'NI 9263', 'serial': '01A2B3C5',
                      'simulated': False, 'parent': 'cDAQ1', 'slot': 2,
                      'timing_family': 'cDAQ1', 'inputs': [],
                      'outputs': [{'id': f'ao{k}', 'label': f'ao{k}'} for k in range(4)]},
                 ]},
            ],
            'selected': {'kind': 'ni', 'devices': ['cDAQ1Mod1', 'cDAQ1Mod2'],
                         'class': 'ni_real', 'source': 'HARDWARE', 'inputs': 4, 'outputs': 4,
                         'timing_family': 'cDAQ1'},
        },
    },
    'response_channels_get': {
        'type': 'response', 'id': 4,
        'result': {
            'revision': 3,
            'channels': [
                _row(1, 'Control', 'control', {'device': 'cDAQ1Mod1', 'channel': 'ai0'}, 'g',
                     50, accel_abort=40),
                _row(2, 'Fixture corner', 'monitor', {'device': 'cDAQ1Mod1', 'channel': 'ai1'},
                     'g', 50),
                _row(3, 'Drive', 'drive', {'device': 'cDAQ1Mod2', 'channel': 'ao0'}, 'V', 1),
            ],
            'report': {'errors': [], 'warnings': [
                {'reason': 'drive_channel_present', 'channel': 3,
                 'message': 'channel 3 is a drive output (sensitivity/range are advisory)'},
            ]},
        },
    },
    'event_random_state': {
        'type': 'event', 'topic': 'random.state', 'seq': 412, 'data': ABORTED_STATE,
    },
    'event_session_changed': {
        'type': 'event', 'topic': 'session.changed', 'seq': 7,
        'data': {'role': 'observer',
                 'controller': {'name': 'Shell on bench-pc', 'present': False},
                 'orphan_s': 21.5},
    },
    'event_random_log': {
        'type': 'event', 'topic': 'random.log', 'seq': 413,
        'data': {'run_id': 'r-0007', 'entries': [
            {'seq': 41, 'at_unix': 1793577731.2, 'kind': 'trip',
             'text': 'Envelope abort: 14 lines outside the abort band, worst +7.2 dB, at 0.0 dB.'},
        ]},
    },
    'response_error_controller_held': {
        'type': 'response', 'id': 2,
        'error': {'code': 'refused', 'reason': 'controller_held',
                  'message': 'Another client controls this service (Shell on bench-pc). Close '
                             'that client, or wait for it to release control.',
                  'detail': {'holder': 'Shell on bench-pc'}},
    },
    'response_error_table_invalid': {
        'type': 'response', 'id': 9,
        'error': {'code': 'bad_request', 'reason': 'table_invalid',
                  'message': 'The channel table has 2 errors, so it was not written.',
                  'detail': {
                      'errors': [
                          {'reason': 'zero_sensitivity', 'channel': 2,
                           'message': 'channel 2 has zero sensitivity'},
                          {'reason': 'physical_channel_shared', 'channel': 3,
                           'message': 'channel 3 is bound to cDAQ1Mod1/ai0, which channel 1 '
                                      'already uses; channels are never shared'},
                      ],
                      'warnings': []}},
    },
    # a connection's own project: an import, its progress, and the summary back
    'request_project_import': {
        'type': 'request', 'id': 31, 'method': 'project.import',
        'params': {'path': '/Users/someone/tests/bracket/geometry.unv',
                   'units': {'length_unit': 'mm'}},
    },
    'event_project_progress': {
        'type': 'event', 'topic': 'project.progress', 'seq': 4,
        'data': {'done': 3, 'total': 4, 'path': '/Users/someone/tests/bracket/geometry.unv'},
    },
    'response_project_import': {
        'type': 'response', 'id': 31,
        'result': {'added': ['Geometry'], 'project': {
            'title': 'Bracket Survey', 'file': '/Users/someone/tests/bracket/survey.hforge',
            'modified': True, 'objects': [
                {'name': 'FRF', 'kind': 'frf', 'records': 22, 'source': 'SIM',
                 'units_defined': True, 'groups': ['Group 1']},
                {'name': 'Geometry', 'kind': 'geometry', 'units_defined': True,
                 'groups': []}]}},
    },
}


# --- MIMO Random and the law catalog (PROTOCOL.md, "MIMO Random", "Control Laws") ---

#: Eight lines of a 6.25 Hz grid and a two-channel reference on them: each PSD
#: 0.04, the cross spectrum 0.012 + 0.016j (coherence 0.25, phase 53.13 degrees),
#: packed as the upper triangle of each line, row by row: G00, G01, G11.
MIMO_LINES = [100.0 + 6.25 * k for k in range(8)]
MIMO_RE = [0.04, 0.012, 0.04] * 8
MIMO_IM = [0.0, 0.016, 0.0] * 8

MIMO_SETUP = {
    'title': 'Two-Axis Random',
    'channels': {
        'controls': [{'channel': 1, 'alarm_db': [3, 3], 'abort_db': [6, 6]},
                     {'channel': 2, 'alarm_db': [3, 3], 'abort_db': [6, 6]}],
        'drives': [{'channel': 3, 'ceiling_vrms': 2, 'abort_vrms': None},
                   {'channel': 4, 'ceiling_vrms': 2, 'abort_vrms': 3}],
    },
    'space': {'kind': 'physical'},
    'reference': {'kind': 'inline', 'freqs_hz': MIMO_LINES, 'n': 2, 're': MIMO_RE, 'im': MIMO_IM},
    'law': {'id': 'pinv_closed', 'params': {'convergence': 0.5}},
    'estimation': {'fs_hz': 8192, 'n_pts': 4096, 'frames_per_loop': 8, 'window': 'hann',
                   'overlap': 0.5, 'memory_loops': 4},
    'identification': {'level_db': -20, 'frames': 32, 'noise_frames': 16, 'coherence_floor': 0.5,
                       'condition_limit': 1000, 'review': True},
    'control': {'frf': 'hold', 'level_ramp_s': 3, 'shutdown_ramp_s': 3, 'max_loops_out': 5},
    'schedule': {'mode': 'auto', 'levels': [{'db': -6, 'dwell_s': 10}, {'db': 0}],
                 'full_level_s': 60},
    'backend': {'kind': 'sim'},
}

MIMO_LAW_REF = {'id': 'pinv_closed', 'name': 'pseudo-inverse, closed loop', 'version': '1.0.0',
                'sha256': None}

MIMO_SHAPE = {'controls': 2, 'drives': 2, 'space': 'physical', 'dofs': None, 'shape': 'square',
              'labels': ['Control X', 'Control Y'], 'drive_labels': ['Shaker X', 'Shaker Y'],
              'bands': [{'alarm_db': [3, 3], 'abort_db': [6, 6]}] * 2}

MIMO_GRID = {'fs_hz': 8192, 'n_pts': 4096, 'df_hz': 2, 'lines': 8, 'loop_s': 2.25, 'actual': False}


def _mimo_state(**over) -> dict:
    state = {
        'run_id': 'm-0003', 'state': 'configured', 'phase': 'idle', 'title': 'Two-Axis Random',
        'source': 'SIM', 'station': {'kind': 'sim', 'class': 'sim', 'devices': []},
        'grid': MIMO_GRID, 'clock': None, 'shape': MIMO_SHAPE, 'mode': 'auto',
        'levels_db': [-6, 0], 'level_db': None, 'elapsed_s': None, 'at_level_s': None,
        'remaining_at_level_s': None, 'loop': 0, 'alarm': False, 'control_error_db': [],
        'gates': None, 'caps': [], 'drive_rms_v': [], 'law': MIMO_LAW_REF, 'faults': [],
        'override': None, 'overrides': [], 'prediction': None, 'excluded': [], 'end': None,
    }
    state.update(over)
    return state


def _mimo_validation(errors=(), law_admitted=True) -> dict:
    return {
        'ok': not errors, 'errors': list(errors), 'warnings': [],
        'shape': {'controls': 2, 'drives': 2, 'space': 'physical', 'dofs': None,
                  'shape': 'square'},
        'transforms': {'response': None, 'drive': None},
        'reference': {'source': 'inline', 'lines': 8, 'n': 2, 'band_hz': [100, 143.75],
                      'min_eigenvalue_ratio': 0, 'rank_min': 2, 'rank_max': 2,
                      'repaired_lines': 0},
        'feasibility': [
            {'channel': ch, 'ceiling_vrms': 2, 'verdict': 'not_evaluated', 'predicted_vrms': None,
             'message': 'The drive this test needs is predicted after system identification, '
                        'before the loop engages.'} for ch in (3, 4)],
        'law': {**MIMO_LAW_REF, 'admitted': law_admitted},
    }


MIMO_GATES = {'coherence_min': 0.962, 'coherence_floor': 0.5, 'condition_max': 12.5,
              'condition_limit': 1000, 'passed': True, 'coherence': [0.962, 0.981]}

MIMO_RUNNING = _mimo_state(
    state='running', phase='engaged', level_db=-6, elapsed_s=31.5, at_level_s=4.5,
    remaining_at_level_s=5.5, loop=14, control_error_db=[0.42, -0.31], gates=MIMO_GATES,
    caps=[1, 0.875], drive_rms_v=[0.412, 0.388],
    prediction={'iteration': 1, 'law': MIMO_LAW_REF, 'drive_rms_v': [0.82, 0.77],
                'ceiling_vrms': [2, 2], 'error_db': [0.5, -0.5], 'drives_pass': True,
                'has_achievable': False, 'unreachable_db': [0, 0], 'drive_accel': [1.25, 1.1],
                'accel_control': [0, 1],
                'feasibility': [{'verdict': 'pass', 'message': ''},
                                {'verdict': 'not_evaluated', 'message': 'no shaker record'}]})

LAW_PINV_CLOSED = {'id': 'pinv_closed',
     'world': 'mimo',
     'builtin': True,
     'name': 'pseudo-inverse, closed loop',
     'version': '1.0.0',
     'author': 'Hertzforge',
     'summary': 'Starts from the regularized pseudo-inverse drive, then each loop corrects a '
                "lower-triangular factor of the reference from the measured error (Smallwood's "
                'coupling-matrix correction), autospectra first in channel order, each factor '
                'diagonal and each drive PSD held to a per-loop limit, the sources held to the '
                "plant's rank. Every drive is positive semi-definite by construction.",
     'sha256': None,
     'check': {'status': 'built_in',
               'date': None,
               'tool_version': None,
               'cases': [],
               'hardware': True,
               'message': 'Built in: cleared for hardware.'},
     'params': [{'key': 'regularization',
                 'label': 'Regularization',
                 'help': 'How the inverse of the plant treats weak directions. Truncated drops '
                         'singular values below the amount times the largest; Tikhonov damps every '
                         'one with the amount times the largest as the damping; none inverts every '
                         'singular value above the numerical floor.',
                 'kind': 'choice',
                 'options': ['truncated', 'tikhonov', 'none'],
                 'default': 'truncated'},
                {'key': 'regularization_amount',
                 'label': 'Regularization amount',
                 'help': 'Relative to the largest singular value on each line. For truncation, '
                         "1e-3 keeps the inverse's condition number under 1,000. Ignored with no "
                         'regularization.',
                 'kind': 'number',
                 'unit': '',
                 'min': 0,
                 'max': 1,
                 'step': 0.0001,
                 'default': 0.001},
                {'key': 'convergence',
                 'label': 'Convergence factor',
                 'help': "The fraction of each loop's correction applied. 1 applies it whole; "
                         'smaller values converge more slowly and average estimation noise.',
                 'kind': 'number',
                 'unit': '',
                 'min': 0.1,
                 'max': 1,
                 'step': 0.05,
                 'default': 0.5},
                {'key': 'cap_up_db',
                 'label': 'Largest rise per loop',
                 'help': "The most any drive's PSD, or any factor diagonal's square, may rise in "
                         "one loop. Never looser than the controller's own cap.",
                 'kind': 'number',
                 'unit': 'dB',
                 'min': 0.1,
                 'max': 3.5,
                 'step': 0.1,
                 'default': 3.5},
                {'key': 'cap_down_db',
                 'label': 'Largest fall per loop',
                 'help': "The most any drive's PSD, or any factor diagonal's square, may fall in "
                         "one loop. Never looser than the controller's own cap.",
                 'kind': 'number',
                 'unit': 'dB',
                 'min': 0.1,
                 'max': 6,
                 'step': 0.1,
                 'default': 6},
                {'key': 'rank_tolerance',
                 'label': 'Rank tolerance',
                 'help': 'Singular values of the plant above this fraction of the largest count '
                         'toward its rank. The reference factor keeps no more independent sources '
                         'than the rank.',
                 'kind': 'number',
                 'unit': '',
                 'min': 0,
                 'max': 0.1,
                 'step': 1e-06,
                 'default': 1e-06},
                {'key': 'weighting',
                 'label': 'Error weighting',
                 'help': "How the least-squares inverse weighs each control's error when there are "
                         'more controls than drives: equally, or relative to its reference PSD so '
                         'a quiet control counts as much as a loud one. No effect on a square '
                         'plant.',
                 'kind': 'choice',
                 'options': ['none', 'reference'],
                 'default': 'none'}],
     'diagnostics': [{'key': 'condition',
                      'label': 'Condition number',
                      'unit': '',
                      'help': 'Largest over smallest singular value of the plant on each line '
                              '(weighted when the law weights). 0 out of band; 1e300 on a singular '
                              'line.',
                      'layout': 'per_line'},
                     {'key': 'kept',
                      'label': 'Singular values kept',
                      'unit': '',
                      'help': 'How many singular values of the plant the inverse used on each '
                              'line.',
                      'layout': 'per_line'},
                     {'key': 'correction_db',
                      'label': 'Largest drive change',
                      'unit': 'dB',
                      'help': "The largest change of any drive's PSD on any line from the law's "
                              'previous drive. 0 at the seed.',
                      'layout': 'scalar'},
                     {'key': 'clamped',
                      'label': 'Clamped factor diagonals',
                      'unit': '',
                      'help': 'Diagonal elements of the reference factor whose correction this '
                              'loop was held to the per-loop limit, over all lines.',
                      'layout': 'scalar'}],
     'shapes': ['square', 'more_controls_than_drives', 'fewer_controls_than_drives'],
     'needs': []}

#: A law file imported from the operator's folder, not yet checked.
LAW_IMPORTED = {
    'id': '9f2c4e6a8b0d1f3e5a7c9b1d3f5e7a9c0b2d4f6e8a0c2e4f6a8b0d2f4e6a8c0b',
    'world': 'mimo', 'builtin': False, 'name': 'notch-aware pseudo-inverse', 'version': '0.2.0',
    'author': 'A. Contributor', 'summary': 'A pseudo-inverse law that holds notched lines.',
    'sha256': '9f2c4e6a8b0d1f3e5a7c9b1d3f5e7a9c0b2d4f6e8a0c2e4f6a8b0d2f4e6a8c0b',
    'check': {'status': 'unchecked', 'date': None, 'tool_version': None, 'cases': [],
              'hardware': False,
              'message': 'This control law has not been checked on the simulator; check it '
                         'before it drives hardware.'},
    'params': [{'key': 'gain', 'label': 'Loop gain', 'help': 'The fraction of each correction '
                'applied.', 'kind': 'number', 'unit': '', 'min': 0.1, 'max': 1, 'step': 0.05,
                'default': 0.5}],
    'diagnostics': [{'key': 'held', 'label': 'Held lines', 'unit': '',
                     'help': 'Lines held this loop.', 'layout': 'scalar'}],
    'shapes': ['square', 'more_controls_than_drives'],
    'needs': ['last_drive'],
}

_TRIP_CAP = ('The change cap held drive 4 back on 721 lines for 5 loops in a row: the control '
             'law asked for more change than the core allows.')


def _check_case(name, plant, requirement, outcome, conv, at_full, final, line, settle, *,
                trip=None, capped=0, calls=3, ms=7.4, gate=True):
    """One case of a `laws.check` report, its `detail` in the controller's words."""
    gate_words = '' if gate else '; identification gate overridden for the check'
    detail = (f'{outcome}; {calls} calls, slowest {ms} ms of 2125 ms; 0 drive-lines repaired, '
              f'{capped} capped{gate_words}')
    return {
        'name': name, 'plant': plant, 'passed': True, 'detail': detail,
        'loops_to_converge': conv, 'loops_at_full': at_full, 'final_error_db': final,
        'worst_line_db': line, 'settle_db': settle, 'requirement': requirement, 'trip': trip,
        'misses': 0, 'rejected': 0, 'repaired_lines': 0, 'capped_lines': capped, 'calls': calls,
        'worst_call_ms': ms, 'budget_ms': 2125, 'gate_passed': gate,
    }


def _within(final, line):
    return (f'within +/- 1 dB 1 loop after full level (worst control {final} dB, worst line '
            f'{line} dB)')


#: `laws.check` on the imported law: the report (the open-loop law file's check, its
#: figures rounded; on the ill-conditioned plant the core's own judgement ended the run,
#: which that case allows) and one progress event. The Rust golden test rebuilds both
#: from the same figures through the controller's own serializers.
LAW_CHECK_REPORT = {'passed': True, 'recorded': True, 'tool_version': 'mimo-check 1', 'cases': [
    _check_case('modal 4 x 4', 'modal-4x4', 'converge', _within('0.46', '4.26'), 1, 1, 0.46,
                4.26, None),
    _check_case('modal 12 x 6', 'modal-12x6', 'settle',
                'settled after 20 loops at full level (worst control 1.98 dB, last changes under '
                '0.01 dB)', None, 20, 1.98, 11.06, 0.01, calls=22, ms=32.9),
    _check_case('ill-conditioned 12 x 6', 'modal-12x6-ill', 'survive',
                f'every call answered; the core ended the run by its own judgement ({_TRIP_CAP})',
                None, 2, 6.32, 10.55, None, trip=_TRIP_CAP, capped=7210, calls=5, ms=32.3,
                gate=False),
    _check_case('rigid plate 12 x 6', 'rigid-plate-12x6', 'settle', _within('0.62', '2.79'), 1, 1,
                0.62, 2.79, None, ms=5.7),
    _check_case('measurement noise 4 x 4', 'modal-4x4', 'converge', _within('0.77', '5.29'), 1, 1,
                0.77, 5.29, None),
    _check_case('level ladder 12 x 6', 'modal-12x6', 'settle', _within('0.36', '4.02'), 1, 1,
                0.36, 4.02, None, calls=6, ms=31.7),
]}
LAW_CHECK_EVENT = {'law_id': '9f2c4e6a8b0d1f3e5a7c9b1d3f5e7a9c0b2d4f6e8a0c2e4f6a8b0d2f4e6a8c0b',
                   'index': 2, 'of': 6, 'case': 'ill-conditioned 12 x 6', 'state': 'done',
                   'result': LAW_CHECK_REPORT['cases'][2]}
LAW_CHECKED = {**LAW_IMPORTED, 'check': {
    'status': 'passed', 'date': '2026-11-03T15:42:07Z', 'tool_version': 'mimo-check 1',
    'cases': [{'name': c['name'], 'passed': c['passed'], 'detail': c['detail']}
              for c in LAW_CHECK_REPORT['cases']],
    'hardware': True,
    'message': 'Checked against the simulated test articles and passed: cleared for hardware.',
}}

MIMO_TEXT_GOLDENS = {
    'request_mimo_random_setup': {
        'type': 'request', 'id': 31, 'method': 'mimo_random.setup',
        'params': {'setup': MIMO_SETUP},
    },
    'response_mimo_random_setup': {
        'type': 'response', 'id': 31,
        'result': {'state': _mimo_state(), 'validation': _mimo_validation()},
    },
    'response_error_setup_invalid': {
        'type': 'response', 'id': 32,
        'error': {'code': 'bad_request', 'reason': 'setup_invalid',
                  'message': 'The MIMO setup has 1 error; correct it and send it again.',
                  'detail': _mimo_validation(errors=[
                      {'reason': 'control_unknown_channel', 'channel': 9,
                       'message': 'Channel 9 is not an enabled control row of the channel '
                                  'table; set its role to control, or remove it from the '
                                  'setup.'}])},
    },
    'event_mimo_random_status': {
        'type': 'event', 'topic': 'mimo_random.status', 'seq': 77, 'data': MIMO_RUNNING,
    },
    'request_mimo_random_override': {
        'type': 'request', 'id': 33, 'method': 'mimo_random.override',
        'params': {'ack': 'Coherence is low below 30 Hz where the fixture is stiff; accepted.'},
    },
    # A prediction over a drive ceiling holds the run until it too is acknowledged.
    'request_mimo_random_override_prediction': {
        'type': 'request', 'id': 39, 'method': 'mimo_random.override',
        'params': {'ack': 'Drive 2 is predicted 0.4 dB over its ceiling; the ceiling holds it '
                          'and the error is accepted for this run.',
                   'reason': 'prediction_exceeded'},
    },
    # Virtual control with a tolerance band per degree of freedom, and every
    # optional setup field the run service takes.
    'request_mimo_random_setup_virtual': {
        'type': 'request', 'id': 40, 'method': 'mimo_random.setup',
        'params': {'setup': {
            **MIMO_SETUP,
            'channels': {
                'controls': [{'channel': 1}, {'channel': 2}],
                'drives': [{'channel': 3, 'ceiling_vrms': 2, 'abort_vrms': None, 'peak_v': 6,
                            'shaker': None, 'moving_mass_lb': 12.5},
                           {'channel': 4, 'ceiling_vrms': 2, 'abort_vrms': 3, 'peak_v': 6,
                            'shaker': None, 'moving_mass_lb': 12.5}],
                'limits': [{'channel': 5, 'ceiling': 0.04, 'abort_margin_db': 3}],
                'article_mass_lb': 40,
            },
            'space': {'kind': 'virtual', 'dofs': ['Sum', 'Difference'],
                      'response_transform': {'rows': 2, 'cols': 2, 'values': [0.5, 0.5, 1, -1]},
                      'drive_transform': None,
                      'dof_bands': [{'alarm_db': [2, 2], 'abort_db': [4, 4]},
                                    {'alarm_db': [3, 3], 'abort_db': [6, 6]}]},
            'control': {'frf': 'update', 'frf_blend': 0.25, 'level_ramp_s': 3,
                        'shutdown_ramp_s': 3, 'max_loops_out': 5, 'cap_up_db': 2,
                        'cap_down_db': 4, 'law_budget': 0.2, 'law_misses': 2},
        }},
    },
    # A control with low coherence taken out of control at the gate, kept as a monitor.
    'request_mimo_random_exclude': {
        'type': 'request', 'id': 42, 'method': 'mimo_random.exclude',
        'params': {'controls': [1],
                   'ack': 'Control Y sits on a fixture node; kept as a monitor for this run.'},
    },
    # The achievable reference on the Predict page, as a block of the grid.
    'request_mimo_random_subscribe_pred_sdm': {
        'type': 'request', 'id': 41, 'method': 'mimo_random.subscribe',
        'params': {'rate_hz': 2, 'plots': [
            {'id': 'ach', 'quantity': 'pred_sdm', 'block': [0, 1], 'points': 256},
            {'id': 'acc', 'quantity': 'pred_drive_accel', 'diagonal': True},
        ]},
    },
    'response_mimo_random_report': {
        'type': 'response', 'id': 34,
        'result': {'report': {
            'run_id': 'm-0003', 'title': 'Two-Axis Random', 'source': 'SIM', 'grid': MIMO_GRID,
            'shape': MIMO_SHAPE, 'law': MIMO_LAW_REF, 'loops': 52,
            'controls': [
                {'label': 'Control X', 'rms': 0.5821, 'reference_rms': 0.5657, 'error_db': 0.25},
                {'label': 'Control Y', 'rms': 0.5528, 'reference_rms': 0.5657, 'error_db': -0.2},
            ],
            'drives': [
                {'channel': 3, 'label': 'Shaker X', 'rms_v': 0.8231, 'ceiling_vrms': 2,
                 'min_cap': 0.875},
                {'channel': 4, 'label': 'Shaker Y', 'rms_v': 0.7764, 'ceiling_vrms': 2,
                 'min_cap': 1},
            ],
            'gates': MIMO_GATES, 'override': None, 'overrides': [],
            'end': {'outcome': 'completed', 'message': 'The schedule finished.', 'at_s': 102.5,
                    'level_db': 0, 'trip': None},
        }},
    },
    'request_laws_list': {
        'type': 'request', 'id': 35, 'method': 'laws.list', 'params': {'world': 'mimo'},
    },
    'response_laws_list': {
        'type': 'response', 'id': 35,
        'result': {'laws': [LAW_PINV_CLOSED], 'problems': [
            {'file': '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.wasm',
             'reason': 'its contents have the SHA-256 '
                       'fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210, not '
                       'the one it is named by'}]},
    },
    'request_laws_import': {
        'type': 'request', 'id': 36, 'method': 'laws.import',
        'params': {'path': '/home/operator/laws/notch_pinv.wasm'},
    },
    'response_laws_import': {
        'type': 'response', 'id': 36, 'result': {'law': LAW_IMPORTED},
    },
    'request_laws_check': {
        'type': 'request', 'id': 39, 'method': 'laws.check',
        'params': {'path': '/home/operator/laws/notch_pinv.wasm', 'params': {'gain': 0.5}},
    },
    'response_laws_check': {
        'type': 'response', 'id': 39,
        'result': {'law': LAW_CHECKED, 'report': LAW_CHECK_REPORT},
    },
    'event_laws_check': {
        'type': 'event', 'topic': 'laws.check', 'seq': 412, 'data': LAW_CHECK_EVENT,
    },
    'request_mimo_random_identification_load': {
        'type': 'request', 'id': 38, 'method': 'mimo_random.identification.load',
        'params': {'path': '/home/operator/hertzforge/identification/two_axis'},
    },
    'response_mimo_random_identification_load': {
        'type': 'response', 'id': 38,
        'result': {'identification': {
            'path': '/home/operator/hertzforge/identification/two_axis',
            'setup_hash': '9f3c2a7e5b1d0c84', 'frames': 64, 'level_db': -20.3,
            'drive_vrms': [0.0512, 0.0498]}},
    },
    # The error heatmap's two modes: the dB error and the coherence error.
    'request_mimo_random_subscribe_coh_error': {
        'type': 'request', 'id': 37, 'method': 'mimo_random.subscribe',
        'params': {'rate_hz': 5, 'plots': [
            {'id': 'heat', 'quantity': 'error_db', 'rows': 'all', 'points': 512},
            {'id': 'coh_heat', 'quantity': 'coh_error', 'rows': 'all', 'points': 512},
        ]},
    },
}


def _mimo_data_build() -> tuple[dict, dict]:
    """`mimo_random.data` for a subscription of a 2 by 2 block of the spectral matrix
    (packed, three lines), one coherence pair (four lines) and the error heatmap (two
    rows by four columns, f32): what the service's builders give for that run (the
    Rust golden test builds it so)."""
    arrays: dict = {}

    def push(dtype, shape, values):
        ident = f'a{len(arrays)}'
        arrays[ident] = (dtype, shape, values)
        return {'$array': ident}

    freq = {'label': 'Frequency', 'unit': 'Hz', 'scale': 'log'}
    sdm_lines = MIMO_LINES[:3]
    sdm = {'view': 'sdm', 'title': 'Spectral Matrix', 'channels': ['Control X', 'Control Y'],
           'packing': 'upper', 'unit': 'g²/Hz', 'x': freq,
           'x_values': push('f64', [3], sdm_lines)}
    sdm['measured'] = {'re': push('f64', [3, 3], MIMO_DATA_RE), 'im': push('f64', [3, 3], MIMO_DATA_IM)}
    sdm['reference'] = {'re': push('f64', [3, 3], MIMO_DATA_REF_RE),
                        'im': push('f64', [3, 3], MIMO_DATA_IM)}
    coh_lines = MIMO_LINES[:4]
    series = []
    for name, role, y in (('Measured', 'measured', MIMO_DATA_COH), ('Reference', 'reference', [0.25] * 4)):
        series.append({'name': name, 'role': role, 'x': push('f64', [4], coh_lines),
                       'y': push('f64', [4], y)})
    coh = {'view': 'spectrum', 'title': 'Coherence: Control X, Control Y', 'x': freq,
           'y': {'label': 'Coherence', 'unit': None, 'scale': 'linear'}, 'series': series,
           'link': 'frequency'}
    heat = {'view': 'heatmap', 'title': 'Control Error',
            'rows': {'label': 'Control', 'ticks': ['Control X', 'Control Y']},
            'x': freq, 'x_values': push('f64', [4], coh_lines),
            'values': push('f32', [2, 4], MIMO_DATA_HEAT),
            'value': {'label': 'Error', 'unit': 'dB'},
            'row_bands': [{'alarm_db': [3, 3], 'abort_db': [6, 6]}] * 2}
    labels = ['Control X', 'Control Y']
    message = {
        'type': 'event', 'topic': 'mimo_random.data', 'seq': 120,
        'data': {
            'run_id': 'm-0003', 'source': 'SIM', 'state': 'running', 'phase': 'full_level',
            'loop': 30, 'elapsed_s': 70.5, 'level_db': 0,
            'plots': [
                {'id': 'grid', 'quantity': 'sdm', 'selector': {'block': [0, 1]}, 'labels': labels,
                 'views': [sdm]},
                {'id': 'coh01', 'quantity': 'coh', 'selector': {'pair': [0, 1]}, 'labels': labels,
                 'views': [coh]},
                {'id': 'heat', 'quantity': 'error_db', 'selector': {'rows': 'all'},
                 'labels': labels, 'views': [heat]},
            ],
        },
    }
    return message, arrays


#: The block's packed lines (G00, G01, G11 per line), every value exact in binary.
MIMO_DATA_RE = [0.0390625, 0.01171875, 0.041015625] * 3
MIMO_DATA_IM = [0.0, 0.015625, 0.0] * 3
MIMO_DATA_REF_RE = [0.0390625, 0.01171875, 0.0390625] * 3
MIMO_DATA_COH = [0.25, 0.2421875, 0.25, 0.234375]
MIMO_DATA_HEAT = [0.5, -0.25, 0.125, 0.0, -0.5, 0.25, 0.375, -0.125]


# --- classical shock (PROTOCOL.md, "Classical Shock") ---
#
# The configuration golden is the contract's example with every field given; its
# `derived` is what the control service computes for it (the Rust golden test
# recomputes it). The binary goldens carry short records whose values are exact in
# binary, laid out as the service's view builders lay them out (each series' x then
# y, every array numbered in traversal order).

SHOCK_SHAKER = {
    'name': 'Bench shaker', 'manufacturer': '', 'model': '', 'notes': '',
    'force_random': None, 'force_sine': {'value': 2000, 'basis': 'Peak'},
    'force_shock': {'value': 4000, 'basis': 'Peak'},
    'max_velocity_ips': {'value': 70, 'basis': 'Peak'},
    'max_displacement_in_pp': {'value': 1, 'basis': 'Peak'},
    'max_accel_bare_g': {'value': 100, 'basis': 'Peak'},
    'armature_mass_lb': 45, 'overturning_moment_limit_lbf_in': None, 'custom': [],
}

SHOCK_CONFIG = {
    'title': 'Half-Sine 20 g 11 ms',
    'pulse': {'shape': 'half_sine', 'amplitude_g': 20, 'duration_ms': 11, 'polarity': 'positive',
              'rise_frac': None, 'fall_frac': None,
              'compensation': {'shape': 'rectangular', 'pre_frac': 4, 'post_frac': 4,
                               'symmetric': True}},
    'standard': {'name': 'mil_std_810_516', 'tolerance_abort': 'full_level',
                 'overshoot_abort': None},
    'acquisition': {'fs_hz': 51200,
                    'srs': {'f_lo_hz': 20, 'f_hi_hz': 2000, 'bands_per_octave': 3,
                            'band_tol_db': 3},
                    'pvss': False},
    'schedule': {'mode': 'auto', 'levels_db': [-12, -9, -6, -3, 0], 'pulses': [1, 1, 1, 1, 3],
                 'start_db': -12, 'advance': 'on_pass', 'max_pulses_per_level': 5},
    'spacing': {'cooling_s': 1, 'settling_s': 0.5, 'compute_s': 0.25},
    'identification': {'drive_vrms': 0.05, 'duration_s': 4, 'nfft': 2048, 'band_hz': [5, 5000],
                       'min_mean_coherence': 0.8, 'review': True},
    'control': {'law': {'id': 'fd_correct', 'params': {'beta': 0.5}},
                'band_hz': [5, 5000], 'coherence_floor': 0.9, 'tikhonov_floor': 0.05,
                'ref_floor_db': 50, 'min_band_energy': 0.9, 'law_budget': 0.25, 'law_misses': 3,
                'max_pulses': 100, 'max_peak_rise_db': 10, 'max_bin_rise_db': 10,
                'first_drive_ratio': 2, 'divergence_pulses': 3},
    'limits': {'drive_vpk_ceiling': 0.5, 'drive_vpk_abort': 0.8, 'peak_accel_g': None,
               'monitors': [{'channel': 2, 'unit': 'A', 'peak_ceiling': 40,
                             'kind': 'between_pulse'}]},
    'feasibility': {'shaker': SHOCK_SHAKER,
                    'moving_mass_lb': {'armature': 45, 'slip_table': 0, 'drive_adapter': 3,
                                       'fixturing': 10, 'test_article': 12},
                    'residual_velocity_ips': 0.1, 'residual_disp_in': 0.001},
    'train': {'count': 5, 'spacing_s': 6},
    'gunfire': {'rounds_per_min': 30, 'rounds': 3},
    'capture': {'auto_start': False, 'run_name': 'classical_shock'},
    'metadata': [{'key': 'run_number', 'label': 'Run Number', 'value': '7'},
                 {'key': 'test_article', 'label': 'Test Article', 'value': 'Bracket'},
                 {'key': 'serial_number', 'label': 'SN', 'value': '0042'},
                 {'key': 'operator', 'label': 'Operator', 'value': ''}],
}

SHOCK_DERIVED = {
    'grid': {'fs_hz': 51200, 'record_len': 8192, 'actual': False},
    'reference': {'peak_g': 20, 'velocity_change_ips': 53.9588,
                  'displacement_pk_pk_in': 0.687732, 'main_start': 2534, 'main_len': 563},
    'quiet_window': {'worst_frac': 0.0795, 'limit_frac': 0.15, 'inside': True},
    'spacing_s': 1, 'call_budget_s': 0.25,
    'feasibility': {'verdict': 'pass', 'message': "The pulse is within the shaker's ratings."},
}

SHOCK_LAW_REF = {'id': 'fd_correct', 'name': 'frequency-domain correction', 'version': '0.1.0',
                 'sha256': None, 'misses': 0}
SHOCK_LEVELS = [-12, -9, -6, -3, 0]
SHOCK_STATION = {'kind': 'sim', 'class': 'sim', 'devices': []}


def _shock_state(**over) -> dict:
    s = {
        'run_id': 's-0001', 'state': 'configured', 'phase': 'idle',
        'title': SHOCK_CONFIG['title'], 'source': 'SIM', 'station': SHOCK_STATION,
        'grid': SHOCK_DERIVED['grid'], 'clock': None, 'mode': 'auto', 'levels_db': SHOCK_LEVELS,
        'level_db': None, 'rung': None,
        'pulses': {'fired': 0, 'passed': 0, 'at_level': 0, 'of_level': 0},
        'next_pulse_s': None, 'elapsed_s': None, 'alarm': False, 'identification': None,
        'prediction': None, 'last_pulse': None, 'law': SHOCK_LAW_REF, 'feasibility': None,
        'end': None,
    }
    s.update(over)
    return s


#: The third pulse of the golden run: -6 dB, passed, the law's call used.
SHOCK_ROW = {
    'index': 2, 'origin': 'ladder', 'scheduled_db': -6, 'commanded_db': -6, 'limit_scale': 1,
    'drive_vpk': 0.2, 'peak_g': 9.98, 'peak_ratio': 0.998, 'error_pct': 1.9,
    'velocity_change_ips': 27.02, 'velocity_change_ratio': 1.0014,
    'verdicts': {'envelope': True, 'velocity': True, 'substitution': True}, 'passed': True,
    'srs': {'bands': 21, 'passed_bands': 21}, 'at_s': 9.75,
    'law': {'used': True, 'miss': None, 'capped_bins': 0, 'peak_capped': False, 'call_ms': 1.4},
}

SHOCK_RUNNING = _shock_state(
    run_id='s-0003', state='running', phase='waiting',
    grid={'fs_hz': 51200, 'record_len': 8192, 'actual': True}, clock='shared',
    level_db=-6, rung=2, pulses={'fired': 3, 'passed': 3, 'at_level': 1, 'of_level': 1},
    next_pulse_s=0.6, elapsed_s=12.5,
    identification={'drive_vrms': 0.05, 'drive_vpk': 0.21, 'coherence_mean': 0.998,
                    'min_mean_coherence': 0.8, 'passed': True},
    prediction={'drive_vpk': 0.4, 'drive_vpk_ceiling': 0.5, 'drive_vpk_abort': 0.8,
                'response_peak_g': 20.1, 'velocity_pk_ips': 27.3, 'displacement_pk_pk_in': 0.69},
    last_pulse=SHOCK_ROW,
    feasibility={'verdict': 'pass', 'message': "The pulse is within the shaker's ratings."})

#: The live frame mid-fire: a pulse playing at -6 dB.
SHOCK_LIVE = {
    'run_id': 's-0003', 'source': 'SIM', 'state': 'running', 'phase': 'firing', 'mode': 'auto',
    'level_db': -6, 'rung': 2, 'pulses': {'fired': 3, 'passed': 3, 'at_level': 1, 'of_level': 1},
    'next_pulse_s': None, 'fire': {'done': 2048, 'of': 8192},
}

#: Eight samples at 1 kHz, the main pulse from the third (t = 0), and the band over six.
SHOCK_FS = 1000.0
SHOCK_START = -2
SHOCK_REF = [0.0, -0.5, 0.0, 7.5, 10.0, 7.5, 0.0, -0.5]
SHOCK_BAND_START = 1
SHOCK_BAND_HI = [1.5, 1.5, 9.0, 11.5, 9.0, 1.5]
SHOCK_BAND_LO = [-1.5, -1.5, 6.0, 8.5, 6.0, -1.5]
SHOCK_MEASURED = [0.0, -0.25, 0.25, 7.25, 9.75, 7.75, 0.25, -0.5]
SHOCK_DRIVE = [0.0, -0.0125, 0.0, 0.1875, 0.25, 0.1875, 0.0, -0.0125]
SHOCK_SRS_F = [100.0, 200.0, 400.0]
SHOCK_SRS_REF = [12.5, 15.0, 11.0]
SHOCK_SRS_HI = [17.5, 21.0, 15.5]
SHOCK_SRS_LO = [9.0, 10.5, 7.75]
SHOCK_SRS_MEAS = [12.25, 15.5, 10.5]
SHOCK_ID_F = [100.0, 200.0, 300.0]
SHOCK_ID_GAIN = [0.5, 1.0, 0.75]
SHOCK_ID_PHASE = [-10.0, -20.0, -30.0]
SHOCK_ID_COH = [0.99, 0.5, 0.98]
SHOCK_ID_TRUSTED = [True, False, True]
SHOCK_ID_FLOOR = 0.9


def _times(start: int, n: int) -> list:
    return [(start + i) / SHOCK_FS for i in range(n)]


class _Arr:
    """Arrays numbered as the service numbers them: `a0`, `a1`, ... in push order."""

    def __init__(self):
        self.arrays: dict = {}

    def push(self, values, dtype='f64'):
        ident = f'a{len(self.arrays)}'
        self.arrays[ident] = (dtype, [len(values)], list(values))
        return {'$array': ident}

    def series(self, name, role, x, y):
        return {'name': name, 'role': role, 'x': self.push(x), 'y': self.push(y)}


def _axis(label, unit, scale):
    return {'label': label, 'unit': unit, 'scale': scale}


_TIME = _axis('Time', 's', 'linear')
_FREQ = _axis('Frequency', 'Hz', 'log')


def _spectrum(title, x, y, series, link):
    return {'view': 'spectrum', 'title': title, 'x': x, 'y': y, 'series': series, 'link': link}


def _shock_pulse_view(a: _Arr, measured=True) -> dict:
    n = len(SHOCK_REF)
    series = [a.series('Reference', 'reference', _times(SHOCK_START, n), SHOCK_REF)]
    bt = _times(SHOCK_START + SHOCK_BAND_START, len(SHOCK_BAND_HI))
    series.append(a.series('Tolerance high', 'tolerance', bt, SHOCK_BAND_HI))
    series.append(a.series('Tolerance low', 'tolerance', bt, SHOCK_BAND_LO))
    if measured:
        series.append(a.series('Measured', 'measured', _times(SHOCK_START, n), SHOCK_MEASURED))
    return _spectrum('Pulse', _TIME, _axis('Acceleration', 'g', 'linear'), series, 'time')


def _shock_drive_view(a: _Arr) -> dict:
    return _spectrum('Drive', _TIME, _axis('Drive', 'V', 'linear'),
                     [a.series('Drive', 'drive', _times(SHOCK_START, len(SHOCK_DRIVE)),
                               SHOCK_DRIVE)], 'time')


def _shock_srs_view(a: _Arr) -> dict:
    return _spectrum('SRS', _axis('Natural frequency', 'Hz', 'log'),
                     _axis('Peak acceleration', 'g', 'log'),
                     [a.series('Reference', 'reference', SHOCK_SRS_F, SHOCK_SRS_REF),
                      a.series('Band high', 'tolerance', SHOCK_SRS_F, SHOCK_SRS_HI),
                      a.series('Band low', 'tolerance', SHOCK_SRS_F, SHOCK_SRS_LO),
                      a.series('Measured', 'measured', SHOCK_SRS_F, SHOCK_SRS_MEAS)],
                     'frequency')


def _shock_kinematic_view(a: _Arr, title: str, unit: str, values: list) -> dict:
    return _spectrum(title, _TIME, _axis(title, unit, 'linear'),
                     [a.series('Reference', 'reference', _times(SHOCK_START, len(values)),
                               values)], 'time')


def _shock_identification_views(a: _Arr) -> list:
    gain = _spectrum('Identification Gain', _FREQ, _axis('Gain', 'g/V', 'log'),
                     [a.series('Gain', 'measured', SHOCK_ID_F, SHOCK_ID_GAIN)], 'frequency')
    phase = _spectrum('Identification Phase', _FREQ, _axis('Phase', 'deg', 'linear'),
                      [a.series('Phase', 'measured', SHOCK_ID_F, SHOCK_ID_PHASE)], 'frequency')
    tx = [f for f, t in zip(SHOCK_ID_F, SHOCK_ID_TRUSTED) if t]
    ty = [c for c, t in zip(SHOCK_ID_COH, SHOCK_ID_TRUSTED) if t]
    ends = [SHOCK_ID_F[0], SHOCK_ID_F[-1]]
    coh = _spectrum('Coherence', _FREQ, _axis('Coherence', None, 'linear'),
                    [a.series('Coherence', 'measured', SHOCK_ID_F, SHOCK_ID_COH),
                     a.series('Trusted bins', 'other', tx, ty),
                     a.series('Floor', 'tolerance', ends, [SHOCK_ID_FLOOR] * 2)], 'frequency')
    return [gain, phase, coh]


SHOCK_LAW_READING = {'view': 'reading', 'title': 'Law', 'rows': [
    {'label': 'Corrected bins', 'value': 412.0, 'unit': 'bins', 'status': 'info'},
    {'label': 'Bins at the rise limit', 'value': 3.0, 'unit': 'bins', 'status': 'info'},
    {'label': 'Drive change', 'value': 0.42, 'unit': 'dB', 'status': 'info'},
]}

#: The run's reading as the service writes it for the golden state, between pulses.
SHOCK_RUN_READING = {'view': 'reading', 'title': 'Run', 'rows': [
    {'label': 'Level', 'value': -6.0, 'unit': 'dB', 'status': 'info'},
    {'label': 'Pulses fired', 'value': 3.0, 'unit': None, 'status': 'info'},
    {'label': 'Pulses passed', 'value': 3.0, 'unit': None, 'status': 'info'},
    {'label': 'Last peak', 'value': 9.98, 'unit': 'g', 'status': 'pass'},
    {'label': 'Last error', 'value': 1.9, 'unit': '%', 'status': 'info'},
    {'label': 'Last drive peak', 'value': 0.2, 'unit': 'V', 'status': 'info'},
    {'label': 'Next pulse', 'value': 0.6, 'unit': 's', 'status': 'info'},
]}

SHOCK_PULSES_TABLE = {
    'view': 'table', 'title': 'Pulses',
    'columns': ['Pulse', 'Origin', 'Level (dB)', 'Peak (g)', 'Error (%)',
                'Velocity change (in/s)', 'Envelope', 'Velocity', 'Substitution', 'Passed'],
    'rows': [['3', 'ladder', '-6', '9.98', '1.9', '27.02', 'pass', 'pass', 'pass', 'pass']],
}

SHOCK_SUBSCRIBE_PLOTS = [
    {'id': 'pulse', 'quantity': 'pulse', 'points': 1024},
    {'id': 'id', 'quantity': 'identification'},
    {'id': 'law', 'quantity': 'law'},
    {'id': 'run', 'quantity': 'readings'},
    {'id': 'table', 'quantity': 'pulses'},
]
SHOCK_QUANTITIES = ['pulse', 'velocity', 'displacement', 'srs', 'spectrum', 'identification',
                    'law', 'readings', 'pulses']


def _shock_pulse_build() -> tuple[dict, dict]:
    a = _Arr()
    views = [_shock_pulse_view(a), _shock_drive_view(a), _shock_srs_view(a)]
    message = {'type': 'event', 'topic': 'shock.pulse', 'seq': 141,
               'data': {'run_id': 's-0003', 'pulse': SHOCK_ROW, 'views': views}}
    return message, a.arrays


def _shock_data_build() -> tuple[dict, dict]:
    a = _Arr()
    plots = [
        {'id': 'pulse', 'quantity': 'pulse', 'views': [_shock_pulse_view(a), _shock_drive_view(a)]},
        {'id': 'id', 'quantity': 'identification', 'views': _shock_identification_views(a)},
        {'id': 'law', 'quantity': 'law', 'views': [SHOCK_LAW_READING]},
        {'id': 'run', 'quantity': 'readings', 'views': [SHOCK_RUN_READING]},
        {'id': 'table', 'quantity': 'pulses', 'views': [SHOCK_PULSES_TABLE]},
    ]
    data = {k: v for k, v in SHOCK_LIVE.items()}
    data.update({'phase': 'waiting', 'next_pulse_s': 0.6, 'fire': None, 'plots': plots})
    message = {'type': 'event', 'topic': 'shock.data', 'seq': 142, 'data': data}
    return message, a.arrays


#: The preview of the short record: its facts and its three views.
SHOCK_PREVIEW_FACTS = {'peak_g': 10, 'velocity_change_ips': 9.6522,
                       'displacement_pk_pk_in': 0.0123, 'main_start': 2, 'main_len': 3}
SHOCK_PREVIEW_QUIET = {'worst_frac': 0.05, 'limit_frac': 0.15, 'inside': True}
SHOCK_PREVIEW_VEL = [0.0, -0.096522, -0.193044, 1.25, 4.6, 7.9, 9.65, 9.6522]
SHOCK_PREVIEW_DISP = [0.0, 0.0, -0.0001, -0.0002, 0.0026, 0.0089, 0.0123, 0.0122]


def _shock_preview_build() -> tuple[dict, dict]:
    a = _Arr()
    views = [_shock_pulse_view(a, measured=False),
             _shock_kinematic_view(a, 'Velocity', 'in/s', SHOCK_PREVIEW_VEL),
             _shock_kinematic_view(a, 'Displacement', 'in', SHOCK_PREVIEW_DISP)]
    message = {'type': 'response', 'id': 53,
               'result': {'derived': {'reference': SHOCK_PREVIEW_FACTS,
                                      'quiet_window': SHOCK_PREVIEW_QUIET},
                          'views': views}}
    return message, a.arrays


#: The built-in classical law's catalog entry, from its own declaration.
LAW_FD_CORRECT = {
    'id': 'fd_correct', 'world': 'shock_classical', 'builtin': True,
    'name': 'frequency-domain correction', 'version': '0.1.0',
    'author': 'Hertzforge contributors',
    'summary': 'Starts from the inverse of the identified plant applied to the reference pulse, '
               'then after each pulse corrects the drive spectrum by a fraction of the inverse '
               'times the error, on the bins the controller trusts. Each bin\'s growth is '
               'limited per pulse.',
    'sha256': None,
    'check': {'status': 'built_in', 'date': None, 'tool_version': None, 'cases': [],
              'hardware': True, 'message': 'Built in: cleared for hardware.'},
    'params': [
        {'key': 'beta', 'label': 'Correction gain',
         'help': "The fraction of each pulse's error corrected before the next pulse. 1 "
                 'corrects the whole error at once; smaller values settle more gently when the '
                 'plant estimate is uncertain.',
         'kind': 'number', 'unit': '', 'min': 0.05, 'max': 1, 'step': 0.05, 'default': 0.5},
        {'key': 'max_db_per_pulse', 'label': 'Largest rise per pulse',
         'help': 'The most any one frequency of the drive may grow from one pulse to the next. '
                 'A frequency may always fall.',
         'kind': 'number', 'unit': 'dB', 'min': 1, 'max': 9, 'step': 0.5, 'default': 6},
        {'key': 'initial_backoff_db', 'label': 'First pulse backoff',
         'help': 'How far under the predicted drive the first pulse starts. The correction '
                 'brings the level up over the next pulses.',
         'kind': 'number', 'unit': 'dB', 'min': 0, 'max': 12, 'step': 1, 'default': 0}],
    'diagnostics': [
        {'key': 'corrected_bins', 'label': 'Corrected bins', 'unit': 'bins',
         'help': 'Bins whose drive the law moved on this call.', 'layout': None},
        {'key': 'clamped_bins', 'label': 'Bins at the rise limit', 'unit': 'bins',
         'help': 'Bins whose rise the law held to its largest rise per pulse on this call.',
         'layout': None},
        {'key': 'drive_change_db', 'label': 'Drive change', 'unit': 'dB',
         'help': "The drive's peak against the last drive's, in dB (0 at the seed).",
         'layout': None}],
    'shapes': None, 'needs': None,
}


def _shock_case(name, plant, requirement, *, pulses, first, full, full_passed, calls, ms,
                vpk, end='completed', trip=None, capped=0, passed=True, detail=None):
    if detail is None:
        detail = (f'{end}; {pulses} pulses, the first pass at pulse {first}; '
                  f'{full_passed} of {full} full-level pulses passed; {calls} calls, '
                  f'slowest {ms} ms of 250 ms')
    return {'name': name, 'plant': plant, 'requirement': requirement, 'passed': passed,
            'detail': detail, 'end': end, 'trip': trip, 'pulses': pulses,
            'first_pass_pulse': first, 'full_level_pulses': full,
            'full_level_passed': full_passed, 'misses': 0, 'refused': 0, 'capped_bins': capped,
            'calls': calls, 'worst_call_ms': ms, 'budget_ms': 250, 'max_drive_vpk': vpk}


#: `laws.check` on the built-in law: a check is not recorded for a built-in law.
SHOCK_CHECK_REPORT = {'passed': True, 'recorded': False, 'tool_version': 'shock-classical-check 1',
                      'cases': [
    _shock_case('flat', 'a wire', 'every_pulse', pulses=7, first=1, full=3, full_passed=3,
                calls=8, ms=1.1, vpk=0.4),
    _shock_case('one mode', 'one mode, 400 Hz, Q 10', 'pass_by', pulses=8, first=2, full=3,
                full_passed=3, calls=9, ms=1.2, vpk=0.41),
    _shock_case('station plant', 'two modes, 200 and 550 Hz, Q 2.5', 'pass_by', pulses=8,
                first=2, full=3, full_passed=3, calls=9, ms=1.2, vpk=0.47),
    _shock_case('transport delay', 'one mode, 400 Hz, Q 10, behind 2 ms', 'pass_by', pulses=9,
                first=3, full=3, full_passed=3, calls=10, ms=1.3, vpk=0.42),
    _shock_case('identification error', 'a wire, identified gain +3 dB', 'pass_by', pulses=8,
                first=2, full=3, full_passed=3, calls=9, ms=1.1, vpk=0.4),
    _shock_case('measurement noise', 'one mode, 400 Hz, Q 10, noise 30 dB under', 'pass_by',
                pulses=9, first=3, full=3, full_passed=3, calls=10, ms=1.2, vpk=0.41),
    _shock_case('anti-resonance', 'a 30 dB notch at 300 Hz', 'survive', pulses=7, first=1,
                full=3, full_passed=3, calls=8, ms=1.2, vpk=0.44, capped=12),
]}

SHOCK_TEXT_GOLDENS = {
    'request_shock_configure': {
        'type': 'request', 'id': 51, 'method': 'shock.configure',
        'params': {'config': SHOCK_CONFIG},
    },
    'response_shock_configure': {
        'type': 'response', 'id': 51,
        'result': {'state': _shock_state(), 'derived': SHOCK_DERIVED, 'warnings': []},
    },
    'event_shock_state': {
        'type': 'event', 'topic': 'shock.state', 'seq': 140, 'data': SHOCK_RUNNING,
    },
    'request_shock_subscribe': {
        'type': 'request', 'id': 52, 'method': 'shock.subscribe',
        'params': {'rate_hz': 2, 'plots': SHOCK_SUBSCRIBE_PLOTS},
    },
    'response_shock_subscribe': {
        'type': 'response', 'id': 52,
        'result': {'default': False, 'rate_hz': 2, 'plots': SHOCK_SUBSCRIBE_PLOTS,
                   'quantities': SHOCK_QUANTITIES},
    },
    'request_shock_fire': {
        'type': 'request', 'id': 54, 'method': 'shock.fire', 'params': {},
    },
    'request_shock_level': {
        'type': 'request', 'id': 55, 'method': 'shock.level', 'params': {'rung': 4},
    },
    'response_laws_list_shock_classical': {
        'type': 'response', 'id': 56,
        'result': {'laws': [LAW_FD_CORRECT], 'problems': []},
    },
    'response_laws_check_shock_classical': {
        'type': 'response', 'id': 57,
        'result': {'law': LAW_FD_CORRECT, 'report': SHOCK_CHECK_REPORT},
    },
    'response_error_shock_config_invalid': {
        'type': 'response', 'id': 58,
        'error': {'code': 'bad_request', 'reason': 'config_invalid',
                  'message': 'The configuration has 2 errors.',
                  'detail': {'errors': [
                      {'reason': 'out_of_range', 'field': 'limits.drive_vpk_abort',
                       'message': 'The drive abort is 0.4; it is above the drive ceiling of 0.5 '
                                  'V and at most 10 V'},
                      {'reason': 'spacing_below_floor', 'field': 'train.spacing_s',
                       'message': '0.5 s between pulses is below the floor of 1 s (the largest '
                                  'of the cooling, settling and compute times).'}],
                      'warnings': [
                      {'reason': 'quiet_window_exceeded', 'field': 'pulse.compensation',
                       'message': "The compensation reaches 24.7% of the peak in the standard's "
                                  'quiet window, whose tolerance is 15%: the reference itself '
                                  'would fail the amplitude check there. Longer or rectangular '
                                  'shoulders sit lower.'}]}},
    },
}


# --- SRS shock (PROTOCOL.md, "SRS Shock") ---
#
# As classical shock's: the configuration golden is the contract's example with
# every field given, and its `derived` is what the control service computes for it
# (the Rust golden test recomputes it). The binary goldens carry short records
# whose values are exact in binary, laid out as the service's view builders lay
# them out.

SRS_CONFIG = {
    'title': 'SRS 100 g, 100 to 2000 Hz',
    'target': {'kind': 'breakpoints', 'points': [[100, 40], [1000, 100], [2500, 100]]},
    'bands': {'f_lo_hz': 100, 'f_hi_hz': 2000, 'bands_per_octave': 3, 'q': 10},
    'synthesis': {'engine': 'wavsyn', 'components': None, 'max_iters': 50, 'tol_db': 1.5,
                  'max_gain_db_per_iter': 3},
    'envelope': {'alarm_db': [3, 3], 'abort_db': [9, 6], 'per_band': None},
    'acquisition': {'fs_hz': 51200, 'ring_out_s': 0.05, 'pvss': True},
    'schedule': {'mode': 'auto', 'levels_db': [-12, -6, 0], 'pulses': [2, 2, 3], 'start_db': -12,
                 'advance': 'on_pass', 'max_pulses_per_level': 5},
    'spacing': {'cooling_s': 1, 'settling_s': 0.5, 'compute_s': 0.25},
    'identification': {'excitation': 'shaped', 'level_db': -20, 'max_vpk': None, 'nfft': 1024,
                       'h_floor_frac': 0.05, 'min_mean_coherence': 0.8, 'review': True},
    'control': {'law': {'id': 'srs_ratio', 'params': {'max_db_per_pulse': 3}},
                'law_budget': 0.25, 'law_misses': 3, 'max_pulses': 100, 'max_gain_rise_db': 6},
    'limits': {'drive_vpk_ceiling': 0.5, 'drive_vpk_abort': 0.8, 'peak_accel_g': None,
               'abort_factor': 2,
               'kinematic': {'accel_g': None, 'velocity_ips': None, 'disp_pp_in': None},
               'monitors': [{'channel': 2, 'unit': 'A', 'peak_ceiling': 40,
                             'kind': 'between_pulse'}]},
    'limit_channels': {'headroom_db': 1,
                       'channels': [{'channel': 3, 'bound': {'kind': 'flat', 'accel_g': 50},
                                     'frf': None}]},
    'feasibility': {'shaker': SHOCK_SHAKER,
                    'moving_mass_lb': {'armature': 45, 'slip_table': 0, 'drive_adapter': 3,
                                       'fixturing': 10, 'test_article': 12},
                    'residual_velocity_ips': 0.1, 'residual_disp_in': 0.001},
    'capture': {'auto_start': False, 'run_name': 'srs_shock'},
    'metadata': [{'key': 'run_number', 'label': 'Run Number', 'value': '7'},
                 {'key': 'test_article', 'label': 'Test Article', 'value': 'Bracket'},
                 {'key': 'serial_number', 'label': 'SN', 'value': '0042'},
                 {'key': 'operator', 'label': 'Operator', 'value': ''}],
}

SRS_DERIVED = {
    'grid': {'fs_hz': 51200, 'record_len': 10569, 'actual': False},
    'bands_hz': [100, 125.9921, 158.7401, 200, 251.9842, 317.4802, 400, 503.9684, 634.9604, 800,
                 1007.9368, 1269.9208, 1600, 2015.8737],
    'target_g': [40, 43.8521, 48.0752, 52.705, 57.7807, 63.3451, 69.4454, 76.1333, 83.4651,
                 91.5031, 100, 100, 100, 100],
    'synthesis': {'engine': 'wavsyn', 'components': 14, 'iters': 9, 'converged': True,
                  'worst_err_db': -0.893, 'duration_s': 0.15643},
    'reference': {'peak_g': 22.2066, 'velocity_pk_ips': 6.1726, 'displacement_pk_pk_in': 0.020336,
                  'residual_velocity_ips': 0, 'residual_disp_in': 0},
    'spacing_s': 1, 'call_budget_s': 0.25,
    'feasibility': {'verdict': 'pass', 'message': "The pulse is within the shaker's ratings."},
}

SRS_LAW_REF = {'id': 'srs_ratio', 'name': 'SRS ratio', 'version': '0.1.0', 'sha256': None,
               'misses': 0}
SRS_LEVELS = [-12, -6, 0]


def _srs_state(**over) -> dict:
    s = {
        'run_id': 'srs-0001', 'state': 'configured', 'phase': 'idle',
        'title': SRS_CONFIG['title'], 'source': 'SIM', 'station': SHOCK_STATION,
        'grid': SRS_DERIVED['grid'], 'clock': None, 'mode': 'auto', 'levels_db': SRS_LEVELS,
        'level_db': None, 'rung': None,
        'pulses': {'fired': 0, 'passed': 0, 'at_level': 0, 'of_level': 0},
        'next_pulse_s': None, 'elapsed_s': None, 'alarm': False, 'identification': None,
        'prediction': None, 'notch': None, 'last_pulse': None, 'law': SRS_LAW_REF,
        'feasibility': None, 'end': None,
    }
    s.update(over)
    return s


#: The third pulse of the golden run: -6 dB, every band in the alarm band.
SRS_ROW = {
    'index': 2, 'origin': 'ladder', 'scheduled_db': -6, 'commanded_db': -6, 'limit_scale': 1,
    'drive_vpk': 0.205, 'peak_g': 36.3, 'velocity_pk_ips': 7.1, 'displacement_pk_pk_in': 0.016,
    'worst_err_db': -1.2, 'worst_band_hz': 630, 'alarm_bands': 0, 'abort_bands': 0,
    'passed': True, 'limited_components': 2, 'worst_limit_db': -0.7, 'at_s': 7.5,
    'law': {'used': True, 'miss': None, 'call_ms': 0.2},
}

SRS_RUNNING = _srs_state(
    run_id='srs-0004', state='running', phase='waiting',
    grid={'fs_hz': 51200, 'record_len': 14336, 'actual': True}, clock='shared',
    level_db=-6, rung=1, pulses={'fired': 3, 'passed': 2, 'at_level': 1, 'of_level': 2},
    next_pulse_s=0.6, elapsed_s=9.5,
    identification={'excitation': 'shaped', 'drive_vpk': 0.05, 'max_vpk': 0.05,
                    'coherence_mean': 0.97, 'min_mean_coherence': 0.8, 'passed': True},
    prediction={'drive_vpk': 0.41, 'drive_vpk_ceiling': 0.5, 'drive_vpk_abort': 0.8,
                'response_peak_g': 72.1, 'velocity_pk_ips': 14.1, 'displacement_pk_pk_in': 0.031},
    notch={'notched_bands': 2, 'worst_before_db': 3.1, 'worst_after_db': -0.6, 'converged': True},
    last_pulse=SRS_ROW,
    feasibility={'verdict': 'pass', 'message': "The pulse is within the shaker's ratings."})

SRS_LIVE = {
    'run_id': 'srs-0004', 'source': 'SIM', 'state': 'running', 'phase': 'firing',
    'mode': 'auto', 'level_db': -6, 'rung': 1,
    'pulses': {'fired': 3, 'passed': 2, 'at_level': 1, 'of_level': 2},
    'next_pulse_s': None, 'fire': {'done': 4096, 'of': 14336},
}

#: Three bands, eight samples at 1 kHz from the record's first sample.
SRS_FS = 1000.0
SRS_BANDS = [100.0, 200.0, 400.0]
SRS_TARGET = [12.5, 15.0, 11.0]
SRS_ALARM_HI = [17.5, 21.0, 15.5]
SRS_ALARM_LO = [9.0, 10.5, 7.75]
SRS_ABORT_HI = [25.0, 30.0, 22.0]
SRS_ABORT_LO = [6.25, 7.5, 5.5]
SRS_MEASURED = [12.25, 15.5, 10.5]
SRS_ACHIEVED = [12.0, 15.25, 11.5]
SRS_PULSE = [0.0, 0.5, -1.25, 2.5, -3.0, 1.5, -0.5, 0.0]
SRS_DRIVE = [0.0, 0.0125, -0.03125, 0.0625, -0.075, 0.0375, -0.0125, 0.0]
SRS_VELOCITY = [0.0, 0.25, -0.125, 0.5, -0.5, 0.25, 0.0, 0.0]
SRS_DISPLACEMENT = [0.0, 0.0001, 0.0002, 0.0003, 0.0002, 0.0001, 0.0, 0.0]


def _srs_times(n: int) -> list:
    return [i / SRS_FS for i in range(n)]


_SRS_X = _axis('Natural frequency', 'Hz', 'log')
_SRS_Y = _axis('Peak acceleration', 'g', 'log')


def _srs_bands_view(a: _Arr, title: str, other=None) -> dict:
    x = SRS_BANDS
    series = [a.series('Target', 'reference', x, SRS_TARGET),
              a.series('Alarm high', 'tolerance', x, SRS_ALARM_HI),
              a.series('Alarm low', 'tolerance', x, SRS_ALARM_LO),
              a.series('Abort high', 'abort', x, SRS_ABORT_HI),
              a.series('Abort low', 'abort', x, SRS_ABORT_LO)]
    if other:
        name, role, y = other
        series.append(a.series(name, role, x, y))
    return _spectrum(title, _SRS_X, _SRS_Y, series, 'frequency')


def _srs_time_view(a: _Arr, title, label, unit, name, role, values) -> dict:
    return _spectrum(title, _TIME, _axis(label, unit, 'linear'),
                     [a.series(name, role, _srs_times(len(values)), values)], 'time')


def _srs_pulse_views(a: _Arr) -> list:
    return [_srs_bands_view(a, 'SRS', ('Measured', 'measured', SRS_MEASURED)),
            _srs_time_view(a, 'Pulse', 'Acceleration', 'g', 'Measured', 'measured', SRS_PULSE),
            _srs_time_view(a, 'Drive', 'Drive', 'V', 'Drive', 'drive', SRS_DRIVE)]


def _srs_identification_views(a: _Arr) -> list:
    f = [100.0, 200.0, 300.0]
    ends = [f[0], f[-1]]
    gain = _spectrum('Identification Gain', _FREQ, _axis('Gain', 'g/V', 'log'),
                     [a.series('Gain', 'measured', f, [0.5, 1.0, 0.75]),
                      a.series('Floor', 'tolerance', ends, [0.05, 0.05])], 'frequency')
    coh = _spectrum('Coherence', _FREQ, _axis('Coherence', None, 'linear'),
                    [a.series('Coherence', 'measured', f, [0.99, 0.5, 0.98]),
                     a.series('Hold', 'tolerance', ends, [0.8, 0.8])], 'frequency')
    return [gain, coh]


def _srs_component_views(a: _Arr) -> list:
    comps = _spectrum('Components', _FREQ, _axis('Amplitude', 'V', 'log'),
                      [a.series('Drive amplitude', 'drive', SRS_BANDS, [0.125, 0.25, 0.0625])],
                      'frequency')
    gains = _spectrum('Gains', _FREQ, _axis('Gain', None, 'log'),
                      [a.series('Last gains', 'other', SRS_BANDS, [1.0, 1.125, 0.875])],
                      'frequency')
    return [comps, gains]


def _srs_limit_views(a: _Arr) -> list:
    bx = [100.0, 400.0]
    return [_spectrum('Limit Fixture', _SRS_X, _SRS_Y,
                      [a.series('Predicted', 'other', SRS_BANDS, [10.0, 45.0, 20.0]),
                       a.series('Bound', 'limit', bx, [50.0, 50.0]),
                       a.series('Bound less headroom', 'tolerance', bx, [50.0, 50.0])],
                      'frequency')]


def _srs_frf_views(a: _Arr) -> list:
    return [_spectrum('Gain Fixture', _FREQ, _axis('Gain', 'g/V', 'log'),
                      [a.series('Supplied', 'reference', [100.0, 500.0, 1000.0],
                                [1.0, 4.0, 1.0])], 'frequency')]


SRS_LAW_READING = {'view': 'reading', 'title': 'Law', 'rows': [
    {'label': 'Worst band error', 'value': -1.2, 'unit': 'dB', 'status': 'info'},
    {'label': 'Worst band', 'value': 630.0, 'unit': 'Hz', 'status': 'info'},
    {'label': 'Components at the change limit', 'value': 0.0, 'unit': 'components',
     'status': 'info'},
]}

SRS_RUN_READING = {'view': 'reading', 'title': 'Run', 'rows': [
    {'label': 'Level', 'value': -6.0, 'unit': 'dB', 'status': 'info'},
    {'label': 'Pulses fired', 'value': 3.0, 'unit': None, 'status': 'info'},
    {'label': 'Pulses passed', 'value': 2.0, 'unit': None, 'status': 'info'},
    {'label': 'Worst band', 'value': -1.2, 'unit': 'dB', 'status': 'pass'},
    {'label': 'At', 'value': 630.0, 'unit': 'Hz', 'status': 'info'},
    {'label': 'Last peak', 'value': 36.3, 'unit': 'g', 'status': 'info'},
    {'label': 'Last drive peak', 'value': 0.205, 'unit': 'V', 'status': 'info'},
    {'label': 'Next pulse', 'value': 0.6, 'unit': 's', 'status': 'info'},
]}

SRS_PULSES_TABLE = {
    'view': 'table', 'title': 'Pulses',
    'columns': ['Pulse', 'Level (dB)', 'Peak (g)', 'Worst band (dB)', 'At (Hz)', 'Alarm bands',
                'Abort bands', 'Drive (V)', 'Passed'],
    'rows': [['3', '-6', '36.3', '-1.2', '630', '0', '0', '0.205', 'pass']],
}

SRS_SUBSCRIBE_PLOTS = [
    {'id': 'srs', 'quantity': 'srs'},
    {'id': 'pulse', 'quantity': 'pulse', 'points': 1024},
    {'id': 'vel', 'quantity': 'velocity', 'points': 1024},
    {'id': 'id', 'quantity': 'identification'},
    {'id': 'comp', 'quantity': 'components'},
    {'id': 'lim', 'quantity': 'limit_channels'},
    {'id': 'frf', 'quantity': 'limit_frf'},
    {'id': 'law', 'quantity': 'law'},
    {'id': 'run', 'quantity': 'readings'},
    {'id': 'table', 'quantity': 'pulses'},
]
SRS_QUANTITIES = ['srs', 'pulse', 'velocity', 'displacement', 'identification', 'components',
                  'limit_channels', 'limit_frf', 'law', 'readings', 'pulses']


def _srs_pulse_build() -> tuple[dict, dict]:
    a = _Arr()
    views = _srs_pulse_views(a)
    message = {'type': 'event', 'topic': 'srs.pulse', 'seq': 151,
               'data': {'run_id': 'srs-0004', 'pulse': SRS_ROW, 'views': views}}
    return message, a.arrays


def _srs_data_build() -> tuple[dict, dict]:
    a = _Arr()
    plots = [
        {'id': 'srs', 'quantity': 'srs',
         'views': [_srs_bands_view(a, 'SRS', ('Measured', 'measured', SRS_MEASURED))]},
        {'id': 'pulse', 'quantity': 'pulse',
         'views': [_srs_time_view(a, 'Pulse', 'Acceleration', 'g', 'Measured', 'measured',
                                  SRS_PULSE),
                   _srs_time_view(a, 'Drive', 'Drive', 'V', 'Drive', 'drive', SRS_DRIVE)]},
        {'id': 'vel', 'quantity': 'velocity',
         'views': [_srs_time_view(a, 'Velocity', 'Velocity', 'in/s', 'Measured', 'measured',
                                  SRS_VELOCITY)]},
        {'id': 'id', 'quantity': 'identification', 'views': _srs_identification_views(a)},
        {'id': 'comp', 'quantity': 'components', 'views': _srs_component_views(a)},
        {'id': 'lim', 'quantity': 'limit_channels', 'views': _srs_limit_views(a)},
        {'id': 'frf', 'quantity': 'limit_frf', 'views': _srs_frf_views(a)},
        {'id': 'law', 'quantity': 'law', 'views': [SRS_LAW_READING]},
        {'id': 'run', 'quantity': 'readings', 'views': [SRS_RUN_READING]},
        {'id': 'table', 'quantity': 'pulses', 'views': [SRS_PULSES_TABLE]},
    ]
    data = dict(SRS_LIVE)
    data.update({'phase': 'waiting', 'next_pulse_s': 0.6, 'fire': None, 'plots': plots})
    message = {'type': 'event', 'topic': 'srs.data', 'seq': 152, 'data': data}
    return message, a.arrays


SRS_PREVIEW_DERIVED = {
    'bands_hz': [100, 200, 400], 'target_g': [12.5, 15, 11],
    'synthesis': {'engine': 'wavsyn', 'components': 3, 'iters': 6, 'converged': True,
                  'worst_err_db': 1.2, 'duration_s': 0.008},
    'reference': {'peak_g': 3, 'velocity_pk_ips': 0.5, 'displacement_pk_pk_in': 0.0003,
                  'residual_velocity_ips': 0, 'residual_disp_in': 0},
}


def _srs_preview_build() -> tuple[dict, dict]:
    a = _Arr()
    views = [
        _srs_bands_view(a, 'Target'),
        _spectrum('Synthesis', _SRS_X, _SRS_Y,
                  [a.series('Target', 'reference', SRS_BANDS, SRS_TARGET),
                   a.series('Synthesis', 'other', SRS_BANDS, SRS_ACHIEVED)], 'frequency'),
        _srs_time_view(a, 'Pulse', 'Acceleration', 'g', 'Synthesized', 'reference', SRS_PULSE),
        _srs_time_view(a, 'Velocity', 'Velocity', 'in/s', 'Synthesized', 'reference',
                       SRS_VELOCITY),
        _srs_time_view(a, 'Displacement', 'Displacement', 'in', 'Synthesized', 'reference',
                       SRS_DISPLACEMENT),
    ]
    message = {'type': 'response', 'id': 63,
               'result': {'derived': SRS_PREVIEW_DERIVED, 'views': views}}
    return message, a.arrays


#: The built-in SRS law's catalog entry, from its own declaration.
LAW_SRS_RATIO = {
    'id': 'srs_ratio', 'world': 'shock_srs', 'builtin': True, 'name': 'SRS ratio',
    'version': '0.1.0', 'author': 'Hertzforge contributors',
    'summary': 'Starts each component from the inverse of the identified gain at its frequency, '
               'then after each pulse scales it by the target over the measured response at the '
               'nearest band, limited per pulse. The controller caps every change by what the '
               'limit channels can take.',
    'sha256': None,
    'check': {'status': 'built_in', 'date': None, 'tool_version': None, 'cases': [],
              'hardware': True, 'message': 'Built in: cleared for hardware.'},
    'params': [
        {'key': 'max_db_per_pulse', 'label': 'Largest change per pulse',
         'help': 'The most any one component of the drive may rise or fall from one pulse to the '
                 'next. Smaller values settle more gently when the measured response is noisy.',
         'kind': 'number', 'unit': 'dB', 'min': 0.5, 'max': 5, 'step': 0.5, 'default': 3}],
    'diagnostics': [
        {'key': 'worst_band_error_db', 'label': 'Worst band error', 'unit': 'dB',
         'help': "The last pulse's measured response against the target at the band furthest "
                 'from it, in dB (0 at the seed).', 'layout': None},
        {'key': 'worst_band_hz', 'label': 'Worst band', 'unit': 'Hz',
         'help': 'The frequency of that band (0 at the seed).', 'layout': None},
        {'key': 'clamped_components', 'label': 'Components at the change limit',
         'unit': 'components',
         'help': 'Components whose change the law held to its largest change per pulse on this '
                 'call.', 'layout': None}],
    'shapes': None, 'needs': None,
}


def _srs_case(name, plant, requirement, *, pulses, first, worst, limit_over=None, limited=0,
              calls, ms, vpk):
    detail = (f'completed; {pulses} pulses; every band within the band from full-level pulse '
              f'{first}; {calls} calls, slowest {ms} ms of 250 ms')
    return {'name': name, 'plant': plant, 'requirement': requirement, 'passed': True,
            'detail': detail, 'end': 'completed', 'trip': None, 'pulses': pulses,
            'full_level_pulses': 3, 'first_within_pulse': first, 'final_worst_band_db': worst,
            'worst_limit_over_db': limit_over, 'misses': 0, 'refused': 0,
            'limited_components': limited, 'calls': calls, 'worst_call_ms': ms,
            'budget_ms': 250, 'max_drive_vpk': vpk}


SRS_CHECK_REPORT = {'passed': True, 'recorded': False, 'tool_version': 'shock-srs-check 1',
                    'cases': [
    _srs_case('one mode', 'one mode, 400 Hz, Q 10', 'within_by', pulses=9, first=1, worst=0.6,
              calls=10, ms=0.1, vpk=0.41),
    _srs_case('close modes', 'two modes, 240 and 260 Hz, Q 30', 'within_by', pulses=9, first=2,
              worst=1.1, calls=10, ms=0.1, vpk=0.43),
    _srs_case('measurement noise', 'one mode, 400 Hz, Q 10, noise 30 dB under', 'within_by',
              pulses=9, first=1, worst=0.9, calls=10, ms=0.1, vpk=0.41),
    _srs_case('level ladder', 'a wire', 'each_level_within', pulses=9, first=1,
              worst=0.7, calls=10, ms=0.1, vpk=0.41),
    _srs_case('identification error', 'one mode, 400 Hz, Q 10, identified gain +3 dB',
              'within_by', pulses=9, first=2, worst=0.8, calls=10, ms=0.1, vpk=0.42),
    _srs_case('limit channel', 'a wire, a limit channel resonating at 500 Hz', 'limit_held',
              pulses=9, first=1, worst=1.4, limit_over=0.3, limited=6, calls=10, ms=0.1,
              vpk=0.38),
]}

SRS_TEXT_GOLDENS = {
    'request_srs_configure': {
        'type': 'request', 'id': 61, 'method': 'srs.configure',
        'params': {'config': SRS_CONFIG},
    },
    'response_srs_configure': {
        'type': 'response', 'id': 61,
        'result': {'state': _srs_state(), 'derived': SRS_DERIVED, 'warnings': []},
    },
    'event_srs_state': {
        'type': 'event', 'topic': 'srs.state', 'seq': 150, 'data': SRS_RUNNING,
    },
    'request_srs_subscribe': {
        'type': 'request', 'id': 62, 'method': 'srs.subscribe',
        'params': {'rate_hz': 2, 'plots': SRS_SUBSCRIBE_PLOTS},
    },
    'response_srs_subscribe': {
        'type': 'response', 'id': 62,
        'result': {'default': False, 'rate_hz': 2, 'plots': SRS_SUBSCRIBE_PLOTS,
                   'quantities': SRS_QUANTITIES},
    },
    'request_srs_fire': {
        'type': 'request', 'id': 64, 'method': 'srs.fire', 'params': {},
    },
    'request_srs_level': {
        'type': 'request', 'id': 65, 'method': 'srs.level', 'params': {'delta_db': 1},
    },
    'response_srs_target_import': {
        'type': 'response', 'id': 69,
        'result': {'target': {'kind': 'breakpoints',
                              'points': [[100, 40], [1000, 100], [2500, 100]]},
                   'errors': [], 'warnings': []},
    },
    'response_srs_table_import': {
        'type': 'response', 'id': 70,
        'result': {'components': {'engine': 'wavsyn', 'rows': [
            {'freq_hz': 100, 'amplitude_g': 1.5, 'half_cycles': 5, 'delay_s': 0.04},
            {'freq_hz': 200, 'amplitude_g': 1.25, 'half_cycles': 7, 'delay_s': 0.02},
            {'freq_hz': 400, 'amplitude_g': 1, 'half_cycles': 9, 'delay_s': 0}]},
                   'errors': [], 'warnings': []},
    },
    'response_laws_list_shock_srs': {
        'type': 'response', 'id': 66,
        'result': {'laws': [LAW_SRS_RATIO], 'problems': []},
    },
    'response_laws_check_shock_srs': {
        'type': 'response', 'id': 67,
        'result': {'law': LAW_SRS_RATIO, 'report': SRS_CHECK_REPORT},
    },
    'response_error_srs_config_invalid': {
        'type': 'response', 'id': 68,
        'error': {'code': 'bad_request', 'reason': 'config_invalid',
                  'message': 'The configuration has 3 errors.',
                  'detail': {'errors': [
                      {'reason': 'alarm_wider_than_abort', 'field': 'envelope.abort_db',
                       'message': 'The abort band is narrower than the alarm band on one side; '
                                  'a band would abort without ever alarming. Widen the abort '
                                  'band or narrow the alarm band.'},
                      {'reason': 'out_of_range', 'field': 'limits.drive_vpk_abort',
                       'message': 'The drive abort is 0.4; it is above the drive ceiling of 0.5 '
                                  'V and at most 10 V'},
                      {'reason': 'limit_unknown_channel',
                       'field': 'limit_channels.channels[0].channel',
                       'message': 'Channel 7 is not a row of the channel table other than the '
                                  'control channel; name the row the bound is for.'}],
                      'warnings': []}},
    },
}

# --- Sine (PROTOCOL.md, "Sine") ---
#
# As the shock modules': the configuration golden is the contract's example with
# every field given, and its `derived` is what the control service computes for it
# (the Rust golden test recomputes it). The binary goldens carry short records
# whose values are exact in binary, laid out as the service's view builders lay
# them out (each series' x then y, every array numbered in traversal order).

SINE_CONFIG = {
    'title': 'Sine 1 g, 20 to 2000 Hz',
    'profile': {'label': '1 g',
                'rows': [{'freq_hz': 20, 'quantity': 'disp_pp_in', 'value': 0.0097},
                         {'freq_hz': 45, 'quantity': 'accel_g', 'value': 1,
                          'alarm_db': [1, 1], 'abort_db': [2, 2]},
                         {'freq_hz': 2000, 'quantity': 'accel_g', 'value': 1}],
                'alarm_db': [1.5, 1.5], 'abort_db': [3, 3]},
    'test': {'kind': 'sweep', 'band_hz': [20, 2000], 'rate': {'law': 'log', 'oct_per_min': 2},
             'start_hz': 20, 'direction': 'up', 'end': {'sweeps': 4, 'duration_s': None}},
    'acquisition': {'fs_hz': None, 'tick_ms': 10},
    'schedule': {'mode': 'auto', 'levels_db': [-12, -6, 0], 'start_db': -12,
                 'per_level': {'sweeps': 1}, 'full_level': {'sweeps': 2}},
    'identification': {'level_db': -20, 'gate_coherence': 0.5, 'trust_coherence': 0.5,
                       'trust_h_floor_frac': 0.05, 'review': True},
    'control': {'composite': 'weighted_avg', 'weights': [{'channel': 1, 'weight': 1}],
                'tracking': {'kind': 'proportional', 'pct': 10, 'max_hz': 100},
                'law': {'id': 'servo_blend',
                        'params': {'blend_w': 0.5, 'compression': 'slow', 'kp': 0.3}},
                'law_budget': 0.25, 'law_misses': 3, 'max_step_db': 1, 'max_rise_db_per_s': 240,
                'divergence_s': 1, 'fault_debounce_ticks': 50},
    'limits': {'drive_vpk_ceiling': 0.5, 'drive_vpk_abort': 0.8, 'kinematic_monitor': True,
               'channels': [{'channel': 3, 'shared_name': None,
                             'profile': {'label': 'Fixture bound',
                                         'rows': [{'freq_hz': 20, 'quantity': 'accel_g',
                                                   'value': 3},
                                                  {'freq_hz': 2000, 'quantity': 'accel_g',
                                                   'value': 3}],
                                         'alarm_db': [1.5, 1.5], 'abort_db': [3, 3]}}],
               'notch': {'drive_floor': 0.01, 'smoothing_ticks': 3, 'release_ramp_db_per_s': 20}},
    'cola': {'enabled': False, 'volts_pk': 0.1, 'waveform': 'sine'},
    'feasibility': {'shaker': SHOCK_SHAKER,
                    'moving_mass_lb': {'armature': 45, 'slip_table': 0, 'drive_adapter': 3,
                                       'fixturing': 10, 'test_article': 12}},
    'capture': {'rule': 'off', 'first_s': None, 'level_settle_s': 1, 'periodic_save_s': None,
                'run_name': 'sine'},
    'metadata': [{'key': 'run_number', 'label': 'Run Number', 'value': '7'},
                 {'key': 'test_article', 'label': 'Test Article', 'value': 'Bracket'},
                 {'key': 'serial_number', 'label': 'SN', 'value': '0042'},
                 {'key': 'operator', 'label': 'Operator', 'value': ''}],
}

SINE_LAW_FACTS = {'id': 'servo_blend', 'name': 'Servo blend', 'version': '0.1.0',
                  'sha256': None, 'live': ['blend_w']}

SINE_FEASIBILITY = {'verdict': 'pass', 'message': "The profile is within the shaker's ratings."}

SINE_DERIVED = {
    'band_hz': [20, 2000],
    'grid': {'fs_hz': 16384, 'tick_frames': 164, 'tick_s': 0.0100098, 'actual': False},
    'profile': {'crossovers': [{'freq_hz': 44.9, 'from': 'disp_pp_in', 'to': 'accel_g'}],
                'peak_accel_g': 1.004, 'peak_velocity_ips': 1.371, 'peak_disp_pp_in': 0.0097},
    'test_s': 797.3, 'identification_s': 128, 'call_budget_s': 0.0025024,
    'stop_ramp_db_per_s': 120, 'safe_stop_s': 0.95,
    'tracking': {'verdict': 'ok', 'ratio': 0.00722, 'at_hz': None, 'message': None},
    'law': SINE_LAW_FACTS,
    'feasibility': SINE_FEASIBILITY,
}

SINE_GRID_ACTUAL = {'fs_hz': 16384, 'tick_frames': 164, 'tick_s': 0.0100098, 'actual': True}
SINE_LEVELS = [-12, -6, 0]


def _sine_state(**over) -> dict:
    s = {
        'run_id': 'sine-0001', 'state': 'configured', 'phase': 'idle',
        'title': SINE_CONFIG['title'], 'source': 'SIM', 'station': SHOCK_STATION,
        'grid': SINE_DERIVED['grid'], 'clock': None, 'test': 'sweep', 'mode': 'auto',
        'levels_db': SINE_LEVELS, 'level_db': None, 'rung': None, 'frequency_hz': None,
        'direction': None, 'sweeps': None, 'held': False, 'elapsed_s': None,
        'at_level_s': None, 'remaining_s': None, 'alarm': False, 'notch': False,
        'identification': None,
        'law': {'id': 'servo_blend', 'name': 'Servo blend', 'version': '0.1.0', 'sha256': None,
                'misses': 0, 'capped_ticks': 0, 'live': {'blend_w': 0.5}},
        'feasibility': None, 'end': None,
    }
    s.update(over)
    return s


SINE_RUNNING = _sine_state(
    run_id='sine-0003', state='running', phase='engaged', grid=SINE_GRID_ACTUAL, clock='shared',
    level_db=-6, rung=1, frequency_hz=970.25, direction='down',
    sweeps={'done': 0, 'of': 1}, elapsed_s=230.5, at_level_s=31.25, remaining_s=566.75,
    identification={'level_db': -20, 'band_coherence': 0.98, 'gate_coherence': 0.5,
                    'lag_frames': 0, 'trusted_frac': 0.97, 'passed': True},
    law={'id': 'servo_blend', 'name': 'Servo blend', 'version': '0.1.0', 'sha256': None,
         'misses': 0, 'capped_ticks': 2, 'live': {'blend_w': 0.5}},
    feasibility=SINE_FEASIBILITY)

#: The golden run's live numbers: a down sweep at -6 dB, three enabled inputs.
SINE_CHANNELS = [
    {'index': 1, 'description': 'Control', 'role': 'control', 'amplitude': 0.5,
     'phase_deg': -12.5, 'rms': 0.375, 'unit': 'g', 'overload': False},
    {'index': 2, 'description': 'Response', 'role': 'monitor', 'amplitude': 0.75,
     'phase_deg': -30.25, 'rms': 0.5625, 'unit': 'g', 'overload': False},
    {'index': 3, 'description': 'Fixture', 'role': 'monitor', 'amplitude': 1.25,
     'phase_deg': -95.5, 'rms': 0.875, 'unit': 'g', 'overload': False},
]

SINE_LAW_LIVE = {'values': {'servo_step_db': 0.015625, 'prediction_step_db': 0.0078125,
                            'total_step_db': 0.0234375, 'servo_level_db': -12.5},
                 'misses': 0, 'capped_ticks': 2}

SINE_HEAD = {
    'run_id': 'sine-0003', 'source': 'SIM', 'class': 'sim', 'state': 'running',
    'phase': 'engaged', 'test': 'sweep', 'mode': 'auto', 't_s': 236.75, 'elapsed_s': 230.5,
    'frequency_hz': 970.25, 'direction': 'down', 'sweep_rate_oct_per_min': -2,
    'sweeps': {'done': 0, 'of': 1}, 'held': False, 'level_db': -6, 'commanded_db': -6,
    'target': 0.5, 'measured': 0.5, 'error_db': 0, 'drive_vpk': 0.125, 'cola_vpk': None,
    'notch': False, 'alarm': False, 'channels': SINE_CHANNELS, 'alarms': [],
    'law': SINE_LAW_LIVE,
}

#: Four points of the sweep grid the down sweep has passed, and the run's values on
#: them, every value exact in binary.
SINE_F = [500.0, 750.0, 1000.0, 1250.0]
SINE_VISITED = [1000.0, 1250.0]
SINE_TARGET = [0.5, 0.5, 0.5, 0.5]
SINE_ALARM_HI = [0.625, 0.625, 0.625, 0.625]
SINE_ALARM_LO = [0.375, 0.375, 0.375, 0.375]
SINE_ABORT_HI = [0.75, 0.75, 0.75, 0.75]
SINE_ABORT_LO = [0.25, 0.25, 0.25, 0.25]
SINE_CONTROL = [0.5, 0.515625]
SINE_DRIVE = [0.125, 0.140625]
SINE_RESPONSE_2 = [0.75, 0.875]
SINE_RESPONSE_3 = [1.25, 1.5]
SINE_LIMIT_3 = [1.5, 1.5, 1.5, 1.5]
SINE_PHASE_1 = [-12.5, -14.0]
SINE_PHASE_3 = [-95.5, -101.25]
SINE_THD = [0.5, 0.625]
SINE_TIME_X = [0.0, 0.00006103515625, 0.0001220703125, 0.00018310546875]
SINE_TIME_1 = [0.0, 0.25, 0.5, 0.25]

_SINE_UNIT_Y = _axis('Amplitude', 'g', 'log')


def _sine_control_view(a: _Arr) -> dict:
    return _spectrum('Control', _FREQ, _SINE_UNIT_Y,
                     [a.series('Control', 'measured', SINE_VISITED, SINE_CONTROL),
                      a.series('Profile', 'reference', SINE_F, SINE_TARGET),
                      a.series('Alarm high', 'tolerance', SINE_F, SINE_ALARM_HI),
                      a.series('Alarm low', 'tolerance', SINE_F, SINE_ALARM_LO),
                      a.series('Abort high', 'abort', SINE_F, SINE_ABORT_HI),
                      a.series('Abort low', 'abort', SINE_F, SINE_ABORT_LO)], 'frequency')


def _sine_drive_view(a: _Arr) -> dict:
    return _spectrum('Drive', _FREQ, _axis('Drive', 'V', 'log'),
                     [a.series('Drive', 'drive', SINE_VISITED, SINE_DRIVE)], 'frequency')


SINE_RUN_READING = {'view': 'reading', 'title': 'Run', 'rows': [
    {'label': 'Frequency', 'value': 970.25, 'unit': 'Hz', 'status': 'info'},
    {'label': 'Direction', 'value': 'down', 'unit': None, 'status': 'info'},
    {'label': 'Level', 'value': -6, 'unit': 'dB', 'status': 'info'},
    {'label': 'Target', 'value': 0.5, 'unit': 'g', 'status': 'info'},
    {'label': 'Measured', 'value': 0.5, 'unit': 'g', 'status': 'pass'},
    {'label': 'Error', 'value': 0, 'unit': 'dB', 'status': 'pass'},
    {'label': 'Drive', 'value': 0.125, 'unit': 'V', 'status': 'info'},
    {'label': 'Sweeps', 'value': '0 of 1', 'unit': None, 'status': 'info'},
    {'label': 'Elapsed', 'value': 230.5, 'unit': 's', 'status': 'info'},
    {'label': 'Remaining', 'value': 566.75, 'unit': 's', 'status': 'info'},
]}

SINE_LAW_READING = {'view': 'reading', 'title': 'Law', 'rows': [
    {'label': 'Servo step', 'value': 0.015625, 'unit': 'dB', 'status': 'info'},
    {'label': 'Prediction step', 'value': 0.0078125, 'unit': 'dB', 'status': 'info'},
    {'label': 'Total step', 'value': 0.0234375, 'unit': 'dB', 'status': 'info'},
    {'label': 'Servo level', 'value': -12.5, 'unit': 'dB', 'status': 'info'},
    {'label': 'Unusable calls in a row', 'value': 0, 'unit': None, 'status': 'pass'},
    {'label': 'Ticks a cap bound', 'value': 2, 'unit': None, 'status': 'info'},
]}


def _sine_live_build() -> tuple[dict, dict]:
    a = _Arr()
    views = [_sine_control_view(a), _sine_drive_view(a), SINE_RUN_READING]
    data = dict(SINE_HEAD)
    data['views'] = views
    return {'type': 'event', 'topic': 'sine.live', 'seq': 171, 'data': data}, a.arrays


SINE_SUBSCRIBE_PLOTS = [
    {'id': 'ctl', 'quantity': 'control'},
    {'id': 'drv', 'quantity': 'drive'},
    {'id': 'resp', 'quantity': 'response', 'channels': [1, 3]},
    {'id': 'ph', 'quantity': 'phase', 'channels': [1, 3]},
    {'id': 'thd', 'quantity': 'thd'},
    {'id': 't1', 'quantity': 'time', 'channels': [1], 'points': 1024},
    {'id': 'id', 'quantity': 'identification'},
    {'id': 'law', 'quantity': 'law'},
    {'id': 'run', 'quantity': 'readings'},
]
SINE_QUANTITIES = ['control', 'drive', 'response', 'phase', 'thd', 'time', 'identification',
                   'law', 'readings']


def _sine_channel_view(a: _Arr, index: int, name: str, y_axis: dict, x: list, y: list,
                       limit=None) -> dict:
    series = [a.series(f'Channel {index}', 'measured', x, y)]
    if limit is not None:
        series.append(a.series('Limit', 'limit', SINE_F, limit))
    return _spectrum(f'Channel {index}: {name}', _FREQ, y_axis, series, 'frequency')


def _sine_identification_views(a: _Arr) -> list:
    f = [100.0, 200.0, 300.0]
    ends = [f[0], f[-1]]
    gain = _spectrum('Identification Gain', _FREQ, _axis('Gain', 'g/V', 'log'),
                     [a.series('Gain', 'measured', f, [0.5, 1.0, 0.75]),
                      a.series('Trust floor', 'tolerance', ends, [0.05, 0.05])], 'frequency')
    coh = _spectrum('Coherence', _FREQ, _axis('Coherence', None, 'linear'),
                    [a.series('Coherence', 'measured', f, [0.99, 0.25, 0.98]),
                     a.series('Trusted', 'reference', [100.0, 300.0], [0.99, 0.98]),
                     a.series('Trust coherence', 'tolerance', ends, [0.5, 0.5])], 'frequency')
    return [gain, coh]


def _sine_data_build() -> tuple[dict, dict]:
    a = _Arr()
    phase_y = _axis('Phase', 'deg', 'linear')
    plots = [
        {'id': 'ctl', 'quantity': 'control', 'channels': None, 'views': [_sine_control_view(a)]},
        {'id': 'drv', 'quantity': 'drive', 'channels': None, 'views': [_sine_drive_view(a)]},
        {'id': 'resp', 'quantity': 'response', 'channels': [1, 3], 'views': [
            _sine_channel_view(a, 1, 'Control', _SINE_UNIT_Y, SINE_VISITED, SINE_CONTROL),
            _sine_channel_view(a, 3, 'Fixture', _SINE_UNIT_Y, SINE_VISITED, SINE_RESPONSE_3,
                               SINE_LIMIT_3)]},
        {'id': 'ph', 'quantity': 'phase', 'channels': [1, 3], 'views': [
            _sine_channel_view(a, 1, 'Control', phase_y, SINE_VISITED, SINE_PHASE_1),
            _sine_channel_view(a, 3, 'Fixture', phase_y, SINE_VISITED, SINE_PHASE_3)]},
        {'id': 'thd', 'quantity': 'thd', 'channels': None, 'views': [
            _spectrum('Distortion', _FREQ, _axis('THD', '%', 'linear'),
                      [a.series('THD', 'measured', SINE_VISITED, SINE_THD)], 'frequency')]},
        {'id': 't1', 'quantity': 'time', 'channels': [1], 'views': [
            {'view': 'spectrum', 'title': 'Channel 1: Control', 'x': _TIME,
             'y': _axis('Amplitude', 'g', 'linear'),
             'series': [a.series('Channel 1', 'measured', SINE_TIME_X, SINE_TIME_1)]}]},
        {'id': 'id', 'quantity': 'identification', 'channels': None,
         'views': _sine_identification_views(a)},
        {'id': 'law', 'quantity': 'law', 'channels': None, 'views': [SINE_LAW_READING]},
        {'id': 'run', 'quantity': 'readings', 'channels': None, 'views': [SINE_RUN_READING]},
    ]
    data = dict(SINE_HEAD)
    data['plots'] = plots
    return {'type': 'event', 'topic': 'sine.data', 'seq': 172, 'data': data}, a.arrays


#: The validate golden's figures: three points of a flat 1 g profile, exact in binary.
SINE_VAL_F = [50.0, 100.0, 200.0]


def _sine_validate_build() -> tuple[dict, dict]:
    a = _Arr()
    f = SINE_VAL_F
    acc = _axis('Acceleration', 'g', 'log')
    views = [
        _spectrum('Profile', _FREQ, acc,
                  [a.series('Profile', 'reference', f, [1.0, 1.0, 1.0]),
                   a.series('Alarm high', 'tolerance', f, [1.1875, 1.1875, 1.1875]),
                   a.series('Alarm low', 'tolerance', f, [0.84375, 0.84375, 0.84375]),
                   a.series('Abort high', 'abort', f, [1.4140625, 1.4140625, 1.4140625]),
                   a.series('Abort low', 'abort', f, [0.70703125, 0.70703125, 0.70703125])],
                  'frequency'),
        _spectrum('Velocity', _FREQ, _axis('Velocity', 'in/s', 'log'),
                  [a.series('Profile', 'reference', f, [1.25, 0.625, 0.3125])], 'frequency'),
        _spectrum('Displacement', _FREQ, _axis('Displacement', 'in pk-pk', 'log'),
                  [a.series('Profile', 'reference', f, [0.0078125, 0.001953125, 0.00048828125])],
                  'frequency'),
    ]
    result = {'errors': [], 'warnings': [],
              'crossovers': [],
              'kinematics': {'peak_accel_g': 1, 'peak_velocity_ips': 1.25,
                             'peak_disp_pp_in': 0.0078, 'verdict': 'pass',
                             'message': "The profile is within the shaker's ratings."},
              'views': views}
    return {'type': 'response', 'id': 78, 'result': result}, a.arrays


#: The built-in Sine law's catalog entry, from its own declaration.
LAW_SERVO_BLEND = {
    'id': 'servo_blend', 'world': 'sine', 'builtin': True, 'name': 'Servo blend',
    'version': '0.1.0', 'author': 'Hertzforge contributors',
    'summary': "Moves the tone's amplitude each tick by a proportional-integral servo on the "
               "error in dB, blended with the identification's prediction of the drive at the "
               'present frequency. The controller caps every change and holds the drive under '
               'the limit channels and the drive ceiling.',
    'sha256': None,
    'check': {'status': 'built_in', 'date': None, 'tool_version': None, 'cases': [],
              'hardware': True, 'message': 'Built in: cleared for hardware.'},
    'params': [
        {'key': 'blend_w', 'label': 'Prediction weight',
         'help': "How much of each tick's change follows the identification's prediction of the "
                 'drive (1) rather than the servo on the measured error (0). It can be changed '
                 'while the test runs.',
         'kind': 'number', 'unit': '', 'min': 0, 'max': 1, 'step': 0.05, 'default': 0.5},
        {'key': 'compression', 'label': 'Compression',
         'help': 'How fast the servo corrects: slow (integral gain 2, at most 20 dB/s), fast (6, '
                 'at most 60 dB/s), or custom with the two values below.',
         'kind': 'choice', 'options': ['slow', 'fast', 'custom'], 'default': 'slow'},
        {'key': 'ki', 'label': 'Integral gain',
         'help': "The servo's integral gain, with custom compression.",
         'kind': 'number', 'unit': '', 'min': 0.1, 'max': 20, 'step': 0.1, 'default': 2},
        {'key': 'max_rate_db_per_s', 'label': 'Largest servo rate',
         'help': 'The fastest the servo moves the drive, with custom compression.',
         'kind': 'number', 'unit': 'dB/s', 'min': 1, 'max': 240, 'step': 1, 'default': 20},
        {'key': 'kp', 'label': 'Proportional gain',
         'help': "The servo's proportional gain on the error in dB.",
         'kind': 'number', 'unit': '', 'min': 0, 'max': 2, 'step': 0.05, 'default': 0.3}],
    'diagnostics': [
        {'key': 'servo_step_db', 'label': 'Servo step', 'unit': 'dB',
         'help': "The servo's part of the last tick's change.", 'layout': None},
        {'key': 'prediction_step_db', 'label': 'Prediction step', 'unit': 'dB',
         'help': "The prediction's part of the last tick's change.", 'layout': None},
        {'key': 'total_step_db', 'label': 'Total step', 'unit': 'dB',
         'help': "The change the law asked for on the last tick, before the controller's caps.",
         'layout': None},
        {'key': 'servo_level_db', 'label': 'Servo level', 'unit': 'dB',
         'help': "The servo's own drive level after the last tick, in dB relative to one drive "
                 'unit.',
         'layout': None}],
    'shapes': None, 'needs': None, 'live': ['blend_w'],
}


def _sine_case(name, plant, requirement, *, ticks, judged, worst, worst_hz, away=None,
               rungs=None, limit_over=None, overshoot=None, capped=0, ms=0.01, vpk):
    detail = (f'completed; {judged} of {ticks} ticks judged, worst {worst} dB; '
              f'{ticks} calls, slowest {ms} ms of 2.5 ms')
    return {'name': name, 'plant': plant, 'requirement': requirement, 'passed': True,
            'detail': detail, 'end': 'completed', 'trip': None, 'ticks': ticks,
            'judged_ticks': judged, 'worst_error_db': worst, 'worst_error_hz': worst_hz,
            'worst_away_error_db': away, 'rungs': rungs, 'worst_limit_over_db': limit_over,
            'worst_release_overshoot_db': overshoot, 'calls': ticks, 'misses': 0, 'refused': 0,
            'capped_ticks': capped, 'worst_call_ms': ms, 'budget_ms': 2.5, 'max_drive_vpk': vpk}


SINE_CHECK_REPORT = {'passed': True, 'recorded': False, 'tool_version': 'sine-check 1', 'cases': [
    _sine_case('slow sweep', 'one mode, 250 Hz, Q 50', 'through', ticks=18000, judged=17400,
               worst=0.42, worst_hz=251.25, vpk=0.41),
    _sine_case('fast sweep', 'one mode, 250 Hz, Q 50', 'within_away', ticks=3000, judged=2900,
               worst=3.5, worst_hz=249.5, away=0.75, vpk=0.43),
    _sine_case('close modes', 'two modes, 240 and 260 Hz, Q 30', 'within', ticks=9000,
               judged=8700, worst=1.25, worst_hz=260.5, vpk=0.39),
    _sine_case('anti-resonance', 'a 30 dB anti-resonance at 300 Hz', 'survive', ticks=9000,
               judged=8700, worst=2.5, worst_hz=300.25, capped=12, vpk=4.5),
    _sine_case('measurement noise', 'one mode, 250 Hz, Q 50', 'within', ticks=3000,
               judged=2900, worst=0.5, worst_hz=250, vpk=0.02),
    _sine_case('level ladder', 'one mode, 250 Hz, Q 50', 'each_rung_within', ticks=4000,
               judged=3600, worst=0.25, worst_hz=250,
               rungs=[[-20, 0.25], [-12, 0.125], [-6, 0.125], [0, 0.25]], vpk=0.02),
    _sine_case('limit channel', 'one mode, 250 Hz, Q 50', 'limit_held', ticks=9000,
               judged=8700, worst=0.75, worst_hz=300.5, limit_over=0.25, overshoot=0.5,
               capped=4, vpk=0.39),
]}

SINE_TEXT_GOLDENS = {
    'request_sine_configure': {
        'type': 'request', 'id': 71, 'method': 'sine.configure',
        'params': {'config': SINE_CONFIG},
    },
    'response_sine_configure': {
        'type': 'response', 'id': 71,
        'result': {'state': _sine_state(), 'derived': SINE_DERIVED, 'warnings': []},
    },
    'event_sine_state': {
        'type': 'event', 'topic': 'sine.state', 'seq': 170, 'data': SINE_RUNNING,
    },
    'request_sine_subscribe': {
        'type': 'request', 'id': 72, 'method': 'sine.subscribe',
        'params': {'rate_hz': 2, 'plots': SINE_SUBSCRIBE_PLOTS},
    },
    'response_sine_subscribe': {
        'type': 'response', 'id': 72,
        'result': {'default': False, 'rate_hz': 2, 'plots': SINE_SUBSCRIBE_PLOTS,
                   'quantities': SINE_QUANTITIES},
    },
    'request_sine_level': {
        'type': 'request', 'id': 73, 'method': 'sine.level', 'params': {'delta_db': -1},
    },
    'request_sine_adjust': {
        'type': 'request', 'id': 74, 'method': 'sine.adjust',
        'params': {'key': 'blend_w', 'value': 0.2},
    },
    'response_sine_profile_import': {
        'type': 'response', 'id': 77,
        'result': {'profile': {'label': '',
                               'rows': [{'freq_hz': 20, 'quantity': 'disp_pp_in',
                                         'value': 0.0097},
                                        {'freq_hz': 45, 'quantity': 'accel_g', 'value': 1,
                                         'alarm_db': [1, 1], 'abort_db': [2, 2]},
                                        {'freq_hz': 2000, 'quantity': 'accel_g', 'value': 1}],
                               'alarm_db': [1.5, 1.5], 'abort_db': [3, 3]},
                   'errors': [], 'warnings': []},
    },
    'response_laws_list_sine': {
        'type': 'response', 'id': 75,
        'result': {'laws': [LAW_SERVO_BLEND], 'problems': []},
    },
    'response_laws_check_sine': {
        'type': 'response', 'id': 76,
        'result': {'law': LAW_SERVO_BLEND, 'report': SINE_CHECK_REPORT},
    },
    'response_error_sine_config_invalid': {
        'type': 'response', 'id': 79,
        'error': {'code': 'bad_request', 'reason': 'config_invalid',
                  'message': 'The configuration has 3 errors.',
                  'detail': {'errors': [
                      {'reason': 'tick_out_of_range', 'field': 'acquisition.tick_ms',
                       'message': 'The control tick is 1 ms; it runs from 2 to 1000 ms for a '
                                  'built-in law.'},
                      {'reason': 'abort_below_ceiling', 'field': 'limits.drive_vpk_abort',
                       'message': 'The drive abort of 0.4 V is under the drive ceiling of 0.5 V; '
                                  'the abort is at least the ceiling.'},
                      {'reason': 'limit_on_control_channel', 'field': 'limits.channels[0].channel',
                       'message': 'Channel 1 is a control row; a limit channel is another enabled '
                                  'input.'}],
                      'warnings': []}},
    },
}


def main() -> None:
    HERE.mkdir(exist_ok=True)

    # a response carrying a spectrum and a reading; UTF-8 in the unit,
    # f64 data, and a u32 array whose 12 bytes force padding before the next
    write('response_spectrum', {
        'type': 'response', 'id': 7,
        'result': {'views': [
            {'view': 'spectrum', 'title': 'PSD Against Specification',
             'x': {'label': 'Frequency', 'unit': 'Hz', 'scale': 'log'},
             'y': {'label': 'PSD', 'unit': 'g²/Hz', 'scale': 'log'},
             'series': [
                 {'name': 'Control', 'role': 'measured', 'x': {'$array': 'a0'}, 'y': {'$array': 'a1'}},
                 {'name': 'Specification', 'role': 'reference', 'x': {'$array': 'a2'}, 'y': {'$array': 'a3'}},
             ],
             'link': 'frequency'},
            {'view': 'reading', 'title': 'Overall',
             'rows': [{'label': 'Control rms', 'value': 6.06, 'unit': 'g', 'status': 'pass'},
                      {'label': 'Lines in tolerance', 'value': 'all', 'unit': None, 'status': 'info'}]},
        ], 'counts': {'$array': 'a4'}},
    }, {
        'a0': ('f64', [5], [20.0, 80.0, 350.0, 1000.0, 2000.0]),
        'a1': ('f64', [5], [0.0101, 0.0398, 0.0403, 0.0396, 0.0099]),
        'a2': ('f64', [4], [20.0, 80.0, 1000.0, 2000.0]),
        'a3': ('f64', [4], [0.01, 0.04, 0.04, 0.01]),
        'a4': ('u32', [3], [3, 1, 4]),
    })

    # an event carrying f32 data with a 2-D shape and u32 connectivity
    write('event_shape', {
        'type': 'event', 'topic': 'demo.shape', 'seq': 42,
        'data': {'views': [
            {'view': 'shape3d', 'title': 'Two Triangles', 'unit': 'm',
             'nodes': {'$array': 'n'}, 'triangles': {'$array': 't'},
             'modes': [{'index': 1, 'frequency_hz': 12.5, 'damping': None, 'shape': {'$array': 's'}},
                       {'index': 2, 'frequency_hz': 31.0, 'damping': 0.02, 'shape': {'$array': 'r'},
                        'shape_imag': {'$array': 'i'}}]},
        ]},
    }, {
        'n': ('f32', [4, 3], [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]),
        't': ('u32', [2, 3], [0, 1, 2, 0, 2, 3]),
        's': ('f32', [4, 3], [0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 1.0, 0.0, 0.0, 0.25]),
        # a complex mode: the second has an imaginary part, so it travels
        'r': ('f32', [4, 3], [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0]),
        'i': ('f32', [4, 3], [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0]),
    })

    # the Random live display: three views, every series stepped on the run's lines
    write('event_random_live', random_live(), random_live_arrays())

    # one connection's subscribed data: a control, a response and a time plot
    write('event_random_data', random_data(), random_data_arrays())

    # one connection's MIMO data: a packed matrix block, a coherence pair, a heatmap
    write('event_mimo_random_data', *_mimo_data_build())

    # a waterfall of two records; f32 values with u32 offsets. No NaN gap:
    # the expected JSON must parse in JavaScript, which has no NaN literal
    write('response_waterfall', {
        'type': 'response', 'id': 9,
        'result': {'views': [
            {'view': 'waterfall', 'title': 'Two PSDs',
             'x': {'label': 'Frequency', 'unit': 'Hz', 'scale': 'linear'},
             'z': {'label': 'PSD', 'unit': '(m/s²)²/Hz', 'scale': 'log'},
             'records': ['101Z+', '102Z+'],
             'offsets': {'$array': 'o'}, 'x_values': {'$array': 'x'},
             'z_values': {'$array': 'z'}, 'x_range': [10.0, 40.0],
             'page': {'number': 1, 'count': 2, 'records': 4}},
        ]},
    }, {
        'o': ('u32', [3], [0, 4, 8]),
        'x': ('f32', [8], [10.0, 20.0, 30.0, 40.0, 10.0, 20.0, 30.0, 40.0]),
        'z': ('f32', [8], [0.5, 1.0, 0.25, 0.125, 2.0, 0.75, 1.0, 0.5]),
    })

    # measured motion on a mesh: three DOFs of four samples, two on one node
    write('response_motion', {
        'type': 'response', 'id': 11,
        'result': {'views': [
            {'view': 'shape3d', 'title': 'Capture 1 of 2, Acceleration', 'unit': 'm',
             'nodes': {'$array': 'n'}, 'lines': {'$array': 'l'}, 'modes': [],
             'link': 'time',
             'motion': {'label': 'Time', 'unit': 's', 'start': 0.0, 'step': 0.25,
                        'quantity': 'Acceleration', 'value_unit': 'm/s²',
                        'node': {'$array': 'd'}, 'direction': {'$array': 'u'},
                        'values': {'$array': 'v'}, 'peak': 2.5}},
        ]},
    }, {
        'n': ('f32', [3, 3], [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.0, 0.0]),
        'l': ('u32', [2, 2], [0, 1, 1, 2]),
        'd': ('u32', [3], [1, 1, 2]),
        'u': ('f32', [3, 3], [0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
        'v': ('f32', [3, 4], [0.0, 1.5, 0.0, -1.5, 0.0, 2.0, 0.0, -2.0, 0.5, 0.0, -0.5, 0.0]),
    })

    # a matrix of three rows by two columns, f32
    write('response_matrix', {
        'type': 'response', 'id': 10,
        'result': {'views': [
            {'view': 'matrix', 'title': 'Two Sets',
             'rows': {'label': 'Identified', 'ticks': ['10.0 Hz', '20.0 Hz', '31.0 Hz']},
             'columns': {'label': 'FE model', 'ticks': ['10.1 Hz', '30.5 Hz']},
             'values': {'$array': 'm'}, 'value_label': 'MAC', 'value_range': [0.0, 1.0]},
        ]},
    }, {
        'm': ('f32', [3, 2], [0.75, 0.125, 0.0, 0.25, 0.0625, 0.5]),
    })

    # text frames: a hello and an error response, no arrays
    (HERE / 'hello.json').write_text(json.dumps(
        {'type': 'hello', 'protocol': 'sds/1', 'service': 'analysis', 'version': '0.1.0'}) + '\n')
    (HERE / 'response_error.json').write_text(json.dumps(
        {'type': 'response', 'id': 8,
         'error': {'code': 'not_found', 'message': "No operation named 'random.nope'; catalog.list names the ones that exist."}}) + '\n')

    # classical shock: the live frame (no arrays), a pulse, a subscription's data, a preview
    write('event_shock_live', {'type': 'event', 'topic': 'shock.live', 'seq': 139,
                               'data': SHOCK_LIVE}, {})
    write('event_shock_pulse', *_shock_pulse_build())
    write('event_shock_data', *_shock_data_build())
    write('response_shock_preview', *_shock_preview_build())

    # SRS shock: the live frame (no arrays), a pulse, a subscription's data, a preview
    write('event_srs_live', {'type': 'event', 'topic': 'srs.live', 'seq': 149,
                             'data': SRS_LIVE}, {})
    write('event_srs_pulse', *_srs_pulse_build())
    write('event_srs_data', *_srs_data_build())
    write('response_srs_preview', *_srs_preview_build())

    # Sine: the live frame, a subscription's data, a profile's validation
    write('event_sine_live', *_sine_live_build())
    write('event_sine_data', *_sine_data_build())
    write('response_sine_profile_validate', *_sine_validate_build())

    for name, message in {**TEXT_GOLDENS, **MIMO_TEXT_GOLDENS, **SHOCK_TEXT_GOLDENS,
                          **SRS_TEXT_GOLDENS, **SINE_TEXT_GOLDENS}.items():
        (HERE / f'{name}.json').write_text(
            json.dumps(message, ensure_ascii=False, indent=1) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
