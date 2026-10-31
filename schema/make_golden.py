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

    # text frames: a hello and an error response, no arrays
    (HERE / 'hello.json').write_text(json.dumps(
        {'type': 'hello', 'protocol': 'sds/1', 'service': 'analysis', 'version': '0.1.0'}) + '\n')
    (HERE / 'response_error.json').write_text(json.dumps(
        {'type': 'response', 'id': 8,
         'error': {'code': 'not_found', 'message': 'No dataset named "plate-x"; catalog.list names the ones that exist.'}}) + '\n')


if __name__ == '__main__':
    main()
