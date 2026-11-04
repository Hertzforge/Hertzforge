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
              'labels': ['Control X', 'Control Y'], 'drive_labels': ['Shaker X', 'Shaker Y']}

MIMO_GRID = {'fs_hz': 8192, 'n_pts': 4096, 'df_hz': 2, 'lines': 8, 'loop_s': 2.25, 'actual': False}


def _mimo_state(**over) -> dict:
    state = {
        'run_id': 'm-0003', 'state': 'configured', 'phase': 'idle', 'title': 'Two-Axis Random',
        'source': 'SIM', 'station': {'kind': 'sim', 'class': 'sim', 'devices': []},
        'grid': MIMO_GRID, 'clock': None, 'shape': MIMO_SHAPE, 'mode': 'auto',
        'levels_db': [-6, 0], 'level_db': None, 'elapsed_s': None, 'at_level_s': None,
        'remaining_at_level_s': None, 'loop': 0, 'alarm': False, 'control_error_db': [],
        'gates': None, 'caps': [], 'drive_rms_v': [], 'law': MIMO_LAW_REF, 'faults': [],
        'override': None, 'prediction': None, 'end': None,
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
              'condition_limit': 1000, 'passed': True}

MIMO_RUNNING = _mimo_state(
    state='running', phase='engaged', level_db=-6, elapsed_s=31.5, at_level_s=4.5,
    remaining_at_level_s=5.5, loop=14, control_error_db=[0.42, -0.31], gates=MIMO_GATES,
    caps=[1, 0.875], drive_rms_v=[0.412, 0.388],
    prediction={'iteration': 1, 'law': MIMO_LAW_REF, 'drive_rms_v': [0.82, 0.77],
                'ceiling_vrms': [2, 2], 'error_db': [0.5, -0.5], 'drives_pass': True})

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
            'gates': MIMO_GATES, 'override': None,
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
         'error': {'code': 'not_found', 'message': 'No dataset named "plate-x"; catalog.list names the ones that exist.'}}) + '\n')

    for name, message in {**TEXT_GOLDENS, **MIMO_TEXT_GOLDENS}.items():
        (HERE / f'{name}.json').write_text(
            json.dumps(message, ensure_ascii=False, indent=1) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
