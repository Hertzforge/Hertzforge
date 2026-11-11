"""The Signals messages against the schema: every Signals golden validates as a message
and as its own payload, the binary one decodes to its expected JSON, its numbers agree
with each other, and the schema refuses what the contract says is wrong (PROTOCOL.md,
"Signals")."""

from __future__ import annotations

import json

import jsonschema
import pytest

from hertzforge.service import frames

from .conftest import GOLDEN

#: text golden -> (path to its payload, the schema definition the payload follows)
TEXT = {
    'request_signals_subscribe': ('params', 'signalsSubscribeParams'),
    'response_signals_subscribe': ('result', 'signalsSubscribeResult'),
    'request_signals_unsubscribe': ('params', 'signalsEmpty'),
    'response_signals_unsubscribe': ('result', 'signalsEmpty'),
}

#: binary golden -> (path to its payload, the schema definition the payload follows)
BINARY = {
    'event_signals_data': ('data', 'signalsData'),
}


def test_every_signals_golden_is_checked_here():
    # The Rust, TypeScript and Python suites read the same files in schema/golden/.
    names = {p.name.split('.')[0] for p in GOLDEN.iterdir()
             if '_signals_' in p.name and not p.name.endswith('.expected.json')}
    assert names == set(TEXT) | set(BINARY), sorted(names ^ (set(TEXT) | set(BINARY)))


def _text(name):
    return json.loads((GOLDEN / f'{name}.json').read_text(encoding='utf-8'))


def _at(message, path):
    for key in path.split('.'):
        message = message[key]
    return message


def _refused(validate, instance, name):
    with pytest.raises(jsonschema.ValidationError):
        validate(instance, name)


@pytest.mark.parametrize('name', sorted(TEXT))
def test_signals_text_golden_validates(name, validate):
    message = _text(name)
    validate(message)
    path, definition = TEXT[name]
    validate(_at(message, path), definition)


@pytest.mark.parametrize('name', sorted(BINARY))
def test_signals_binary_golden_validates_and_decodes_to_its_expected_json(name, validate):
    data = (GOLDEN / f'{name}.bin').read_bytes()
    wire, _arrays = frames.to_wire(frames.decode(data))
    validate(wire)
    path, definition = BINARY[name]
    validate(_at(wire, path), definition)
    decoded = frames.decode(data)
    expected = json.loads((GOLDEN / f'{name}.expected.json').read_text(encoding='utf-8'))

    def same(a, b, where):
        if isinstance(b, dict) and set(b) == {'dtype', 'shape', 'values'}:
            assert frames.dtype_name(a) == b['dtype'], where
            assert list(a.shape) == b['shape'], where
            assert a.reshape(-1).tolist() == b['values'], where
        elif isinstance(b, dict):
            assert list(a) == list(b), where
            for k in b:
                same(a[k], b[k], f'{where}.{k}')
        elif isinstance(b, list):
            assert len(a) == len(b), where
            for i, (u, v) in enumerate(zip(a, b, strict=True)):
                same(u, v, f'{where}[{i}]')
        else:
            assert a == b, where

    same(decoded, expected, name)


def test_signals_data_is_on_the_station_clock_and_decimated_as_asked():
    d = frames.decode((GOLDEN / 'event_signals_data.bin').read_bytes())['data']
    fs = d['fs_hz']
    assert d['t_s'] == d['frames'] / fs
    raw, env = d['plots']
    # Every sample of a window that fits its points, one station frame apart, ending at
    # the newest frame.
    x = raw['views'][0]['series'][0]['x']
    assert len(x) == raw['window_s'] * fs == 64
    assert x[0] == raw['first_frame'] / fs
    assert x[-1] == (d['frames'] - 1) / fs
    assert 'y_min' not in raw['views'][0]['series'][0]
    # A longer window is the envelope at its points: each point the largest and the
    # smallest of four frames, never less than the smallest.
    assert env['channels'] == [1, 2, 3]
    assert [v['title'] for v in env['views']] == ['Channels 1, 3', 'Channel 2: Drive']
    assert [v['y']['unit'] for v in env['views']] == ['g', 'V']
    for view in env['views']:
        for s in view['series']:
            assert len(s['x']) == 64
            assert s['x'][1] - s['x'][0] == 4 / fs
            assert (s['y'] >= s['y_min']).all()
    assert env['views'][1]['series'][0]['role'] == 'drive'


def test_signals_subscriptions_refuse_what_the_contract_refuses(validate):
    validate({'plots': []}, 'signalsSubscribeParams')
    validate({'id': 'a', 'channels': 'drives', 'arrange': 'split'}, 'signalsSubscribePlot')
    _refused(validate, {}, 'signalsSubscribeParams')
    _refused(validate, {'plots': [], 'rate_hz': 20}, 'signalsSubscribeParams')
    _refused(validate, {'plots': [], 'channels': [1]}, 'signalsSubscribeParams')
    for plot in ({'id': ''},
                 {'id': 'a', 'quantity': 'time'},
                 {'id': 'a', 'channels': 'limited'},
                 {'id': 'a', 'channels': [-1]},
                 {'id': 'a', 'window_s': 0},
                 {'id': 'a', 'window_s': 10.5},
                 {'id': 'a', 'points': 63},
                 {'id': 'a', 'arrange': 'stacked'}):
        _refused(validate, plot, 'signalsSubscribePlot')
    result = _text('response_signals_subscribe')['result']
    _refused(validate, {**result, 'available': 'yes'}, 'signalsSubscribeResult')
    bad = json.loads(json.dumps(result))
    bad['channels'][0]['kind'] = 'output'
    _refused(validate, bad, 'signalsSubscribeResult')
    _refused(validate, {'plots': []}, 'signalsEmpty')
