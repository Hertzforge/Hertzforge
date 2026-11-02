"""The codec against the golden frames every language's codec shares."""

from __future__ import annotations

import json
import struct

import numpy as np
import pytest

from hertzforge.service import frames

from .conftest import GOLDEN

BINARY = ('response_spectrum', 'event_shape', 'event_random_live', 'event_random_data',
          'response_waterfall')
#: Binary goldens whose arrays are named a0, a1, ... in traversal order
TRAVERSAL_NAMED = ('response_spectrum', 'event_random_live', 'event_random_data')
#: Text goldens of the hardware and Random control messages
CONTROL_TEXT = ('request_random_configure', 'response_random_configure',
                'request_random_subscribe', 'response_random_subscribe',
                'response_station_list', 'response_channels_get', 'event_random_state',
                'event_session_changed', 'response_error_controller_held',
                'response_error_table_invalid')


def _as_expected(node):
    """A decoded message in the expected.json form: each array as
    {"dtype", "shape", "values"}."""
    if isinstance(node, np.ndarray):
        return {'dtype': frames.dtype_name(node), 'shape': list(node.shape),
                'values': node.ravel().tolist()}
    if isinstance(node, dict):
        return {key: _as_expected(value) for key, value in node.items()}
    if isinstance(node, list):
        return [_as_expected(item) for item in node]
    return node


def _golden_input(name):
    """(message with references, {id: array}) for a golden frame: the
    references and their order from the frame's own header, the values from
    expected.json, so encoding is tested against exactly what made it."""
    data = (GOLDEN / f'{name}.bin').read_bytes()
    (length,) = struct.unpack_from('<I', data, 4)
    header = json.loads(data[8:8 + length])
    entries = header.pop('arrays')
    expected = json.loads((GOLDEN / f'{name}.expected.json').read_text(encoding='utf-8'))
    found = {}

    def walk(ref, value):
        if isinstance(ref, dict) and set(ref) == {'$array'}:
            found[ref['$array']] = value
        elif isinstance(ref, dict):
            for key in ref:
                walk(ref[key], value[key])
        elif isinstance(ref, list):
            for a, b in zip(ref, value, strict=True):
                walk(a, b)

    walk(header, expected)
    arrays = {}
    for entry in entries:
        spec = found[entry['id']]
        arrays[entry['id']] = np.asarray(
            spec['values'], dtype=frames.DTYPES[spec['dtype']]
        ).reshape(spec['shape'])
    return header, arrays


@pytest.mark.parametrize('name', BINARY)
def test_encode_reproduces_golden_bytes(name):
    message, arrays = _golden_input(name)
    assert frames.encode(message, arrays) == (GOLDEN / f'{name}.bin').read_bytes()


@pytest.mark.parametrize('name', BINARY)
def test_decode_matches_expected(name):
    decoded = frames.decode((GOLDEN / f'{name}.bin').read_bytes())
    expected = json.loads((GOLDEN / f'{name}.expected.json').read_text(encoding='utf-8'))
    assert _as_expected(decoded) == expected
    assert 'arrays' not in decoded


@pytest.mark.parametrize('name', TRAVERSAL_NAMED)
def test_pack_names_arrays_in_traversal_order(name):
    # these goldens' ids are a0, a1, ... in traversal order, so packing the
    # decoded message must give the same frame back
    raw = (GOLDEN / f'{name}.bin').read_bytes()
    assert frames.pack(frames.decode(raw)) == raw


@pytest.mark.parametrize('name', ('hello', 'response_error'))
def test_text_golden_round_trip(name):
    text = (GOLDEN / f'{name}.json').read_text(encoding='utf-8')
    message = frames.decode(text)
    assert message == json.loads(text)
    again = frames.pack(message)
    assert isinstance(again, str)
    assert json.loads(again) == message
    assert ': ' not in again and ', ' not in again.replace('", "', '')


@pytest.mark.parametrize('name', CONTROL_TEXT)
def test_control_text_goldens_round_trip_compactly(name):
    text = (GOLDEN / f'{name}.json').read_text(encoding='utf-8')
    message = frames.decode(text)
    assert message == json.loads(text)
    again = frames.pack(message)
    assert isinstance(again, str), 'a message without arrays is a text frame'
    assert again == json.dumps(message, separators=(',', ':'), ensure_ascii=False)


def test_random_live_decodes_to_stepped_series_on_shared_lines():
    message = frames.decode((GOLDEN / 'event_random_live.bin').read_bytes())
    control, drive, reading = message['data']['views']
    assert [s['role'] for s in control['series']] == [
        'measured', 'reference', 'tolerance', 'tolerance', 'abort', 'abort']
    assert drive['y']['unit'] == 'V²/Hz' and reading['view'] == 'reading'
    lines = control['series'][0]['x']
    for series in control['series'] + drive['series']:
        assert series['shape'] == 'steps'
        assert np.array_equal(series['x'], lines)
        assert series['edges'].shape == (len(lines) + 1,)
        assert series['x'].dtype == np.float64


def test_random_data_carries_each_plot_with_its_own_views():
    message = frames.decode((GOLDEN / 'event_random_data.bin').read_bytes())
    data = message['data']
    assert 'views' not in data
    assert [p['id'] for p in data['plots']] == ['ctl', 'lim', 't1']
    ctl, lim, t1 = data['plots']
    assert ctl['channels'] is None and len(ctl['views']) == 1
    assert lim['channels'] == [1, 2] and len(lim['views']) == 2
    measured, limit = lim['views'][1]['series']
    assert limit['role'] == 'limit' and limit['shape'] == 'steps'
    assert limit['edges'].shape == (len(limit['x']) + 1,)
    # the limit spans its own lines' bins, the same edges the measured line has there
    assert np.array_equal(limit['edges'], measured['edges'][1:4])
    series = t1['views'][0]['series'][0]
    assert t1['views'][0]['x']['scale'] == 'linear' and 'shape' not in series


def test_header_padding_and_alignment():
    frame = frames.pack({'type': 'event', 'topic': 't', 'seq': 0,
                         'data': {'a': np.arange(3, dtype=np.uint8),
                                  'b': np.ones(2)}})
    (length,) = struct.unpack_from('<I', frame, 4)
    assert (8 + length) % 8 == 0
    header = json.loads(frame[8:8 + length])
    assert [e['offset'] for e in header['arrays']] == [0, 8]
    assert frame[8 + length - 1:8 + length] in (b' ', b'}')


def _frame(header: dict, payload: bytes = b'') -> bytes:
    text = json.dumps(header).encode()
    text += b' ' * (-(8 + len(text)) % 8)
    return b'SDS1' + struct.pack('<I', len(text)) + text + payload


@pytest.mark.parametrize('frame, words', [
    (b'XXXX\0\0\0\0', 'magic'),
    (b'SDS1' + struct.pack('<I', 99), 'runs past'),
    (b'SDS1' + struct.pack('<I', 3) + b'{} ', 'multiple of 8'),
    (_frame({'type': 'event'}), '"arrays"'),
    (_frame({'x': {'$array': 'q'}, 'arrays': []}), "'q'"),
    (_frame({'arrays': [{'id': 'a', 'dtype': 'f64', 'shape': [1],
                         'offset': 0, 'nbytes': 8}]}, b'\0' * 8),
     'never referenced'),
    (_frame({'x': {'$array': 'a'}, 'y': {'$array': 'a'},
             'arrays': [{'id': 'a', 'dtype': 'f64', 'shape': [1],
                         'offset': 0, 'nbytes': 8}]}, b'\0' * 8),
     'referenced twice'),
    (_frame({'x': {'$array': 'a'},
             'arrays': [{'id': 'a', 'dtype': 'f64', 'shape': [2],
                         'offset': 0, 'nbytes': 8}]}, b'\0' * 16),
     'needs 16'),
    (_frame({'x': {'$array': 'a'},
             'arrays': [{'id': 'a', 'dtype': 'f64', 'shape': [2],
                         'offset': 0, 'nbytes': 16}]}, b'\0' * 8),
     'runs to byte'),
    (_frame({'x': {'$array': 'a'},
             'arrays': [{'id': 'a', 'dtype': 'f16', 'shape': [1],
                         'offset': 0, 'nbytes': 2}]}, b'\0' * 8),
     "'f16'"),
    (_frame({'x': {'$array': 'a'},
             'arrays': [{'id': 'a', 'dtype': 'u8', 'shape': [1],
                         'offset': 4, 'nbytes': 1}]}, b'\0' * 8),
     'multiple of 8'),
    ('{"x": {"$array": "a0"}}', 'binary frames'),
    ('[1, 2]', 'one JSON object'),
    ('{nope', 'not JSON'),
])
def test_decode_refuses_with_the_cause(frame, words):
    with pytest.raises(frames.FrameError, match=None) as caught:
        frames.decode(frame)
    assert words in str(caught.value)


def test_encode_refuses_what_sds1_cannot_carry():
    with pytest.raises(frames.FrameError, match='int64'):
        frames.pack({'a': np.arange(3)})
    with pytest.raises(frames.FrameError, match='JSON cannot carry'):
        frames.pack({'a': float('nan')})
    with pytest.raises(frames.FrameError, match='"arrays"'):
        frames.encode({'arrays': []}, {})


def test_decoded_arrays_are_read_only_and_typed():
    message = frames.decode(frames.pack(
        {'v': np.arange(6, dtype=np.float32).reshape(2, 3)}))
    array = message['v']
    assert array.dtype == np.float32 and array.shape == (2, 3)
    assert not array.flags.writeable
