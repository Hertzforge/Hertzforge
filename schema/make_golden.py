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
            'elapsed_s': 63.5, 'level_db': 0, 'at_level_s': 12.5,
            'remaining_at_level_s': 287.5, 'loop': 118,
            'control_rms_g': 6.02, 'reference_rms_g': 6.06, 'drive_rms_v': 0.412,
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
                'pretest': {'averages': 4, 'max_vrms': 0.5, 'min_vrms': 0.01, 'shaped': False},
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

TEXT_GOLDENS = {
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
        'data': {'controller': {'name': 'Shell on bench-pc', 'present': False},
                 'orphan_s': 21.5},
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
             'modes': [{'index': 1, 'frequency_hz': 12.5, 'damping': None, 'shape': {'$array': 's'}}]},
        ]},
    }, {
        'n': ('f32', [4, 3], [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]),
        't': ('u32', [2, 3], [0, 1, 2, 0, 2, 3]),
        's': ('f32', [4, 3], [0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 1.0, 0.0, 0.0, 0.25]),
    })

    # the Random live display: three views, every series stepped on the run's lines
    write('event_random_live', random_live(), random_live_arrays())

    # text frames: a hello and an error response, no arrays
    (HERE / 'hello.json').write_text(json.dumps(
        {'type': 'hello', 'protocol': 'sds/1', 'service': 'analysis', 'version': '0.1.0'}) + '\n')
    (HERE / 'response_error.json').write_text(json.dumps(
        {'type': 'response', 'id': 8,
         'error': {'code': 'not_found', 'message': 'No dataset named "plate-x"; catalog.list names the ones that exist.'}}) + '\n')

    for name, message in TEXT_GOLDENS.items():
        (HERE / f'{name}.json').write_text(
            json.dumps(message, ensure_ascii=False, indent=1) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
