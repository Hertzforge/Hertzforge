"""The MIMO Shock messages against the schema: every MIMO Shock golden validates as a
message and as its own payload, the binary ones decode to their expected JSON, and the
schema refuses what the contract says is wrong (PROTOCOL.md, "MIMO Shock")."""

from __future__ import annotations

import copy
import json

import jsonschema
import pytest

from hertzforge.service import frames

from .conftest import GOLDEN

#: text golden -> (path to its payload, the schema definition the payload follows)
TEXT = {
    'request_mimo_shock_configure': ('params', 'mimoShockConfigureParams'),
    'request_mimo_shock_configure_spectrum': ('params', 'mimoShockConfigureParams'),
    'response_mimo_shock_configure': ('result', 'mimoShockConfigureResult'),
    'event_mimo_shock_state': ('data', 'mimoShockState'),
    'event_mimo_shock_state_review': ('data', 'mimoShockState'),
    'event_mimo_shock_state_unscaled': ('data', 'mimoShockState'),
    'request_mimo_shock_proceed': ('params', 'mimoShockProceedParams'),
    'request_mimo_shock_subscribe': ('params', 'mimoShockSubscribeParams'),
    'response_mimo_shock_subscribe': ('result', 'mimoShockSubscribeResult'),
    'request_mimo_shock_fire': ('params', None),
    'response_error_mimo_shock_unreachable': ('error', None),
    'response_error_mimo_shock_config_invalid': ('error.detail', 'report'),
}

#: binary golden -> (path to its payload, the schema definition the payload follows)
BINARY = {
    'event_mimo_shock_live': ('data', 'mimoShockLive'),
    'event_mimo_shock_pulse': ('data', 'mimoShockPulseEvent'),
    'event_mimo_shock_pulse_spectrum': ('data', 'mimoShockPulseEvent'),
    'event_mimo_shock_data': ('data', 'mimoShockData'),
}


def test_every_mimo_shock_golden_is_checked_here():
    # The Rust, TypeScript and Python suites read the same files in schema/golden/.
    names = {p.name.split('.')[0] for p in GOLDEN.iterdir()
             if '_mimo_shock_' in p.name and not p.name.endswith('.expected.json')}
    assert names == set(TEXT) | set(BINARY), sorted(names ^ (set(TEXT) | set(BINARY)))


def _text(name):
    return json.loads((GOLDEN / f'{name}.json').read_text(encoding='utf-8'))


def _wire(name):
    wire, _arrays = frames.to_wire(frames.decode((GOLDEN / f'{name}.bin').read_bytes()))
    return wire


def _at(message, path):
    for key in path.split('.'):
        message = message[key]
    return message


@pytest.mark.parametrize('name', sorted(TEXT))
def test_mimo_shock_text_golden_validates(name, validate):
    message = _text(name)
    validate(message)
    path, definition = TEXT[name]
    if definition:
        validate(_at(message, path), definition)


@pytest.mark.parametrize('name', sorted(BINARY))
def test_mimo_shock_binary_golden_validates_and_decodes_to_its_expected_json(name, validate):
    wire = _wire(name)
    validate(wire)
    path, definition = BINARY[name]
    validate(_at(wire, path), definition)
    decoded = frames.decode((GOLDEN / f'{name}.bin').read_bytes())
    expected = json.loads((GOLDEN / f'{name}.expected.json').read_text(encoding='utf-8'))

    def same(a, b, where):
        if isinstance(b, dict) and set(b) == {'dtype', 'shape', 'values'}:
            assert list(a.shape) == b['shape'], where
            assert a.reshape(-1).tolist() == b['values'], where
        elif isinstance(b, dict):
            assert list(a) == list(b), where
            for k in b:
                same(a[k], b[k], f'{where}.{k}')
        elif isinstance(b, list):
            assert len(a) == len(b), where
            for i, (u, v) in enumerate(zip(a, b)):
                same(u, v, f'{where}[{i}]')
        else:
            assert a == b, where

    same(decoded, expected, name)


def test_the_state_says_when_scaled_aborts_are_off(validate):
    on = _text('event_mimo_shock_state')['data']
    off = _text('event_mimo_shock_state_unscaled')['data']
    assert on['scaled_aborts'] is True and on['notice'] is None
    assert off['scaled_aborts'] is False and off['notice']
    bad = copy.deepcopy(on)
    del bad['scaled_aborts']
    with pytest.raises(jsonschema.ValidationError):
        validate(bad, 'mimoShockState')


def test_a_pulse_row_names_why_a_control_failed_and_its_margins(validate):
    row = _text('event_mimo_shock_state')['data']['last_pulse']
    failing = [c for c in row['controls'] if not c['passed']]
    assert failing and all(c['reasons'] for c in failing)
    for c in row['controls']:
        assert {'scaled_margin_db', 'absolute_margin_db'} <= set(c)
    bad = copy.deepcopy(row)
    bad['controls'][0]['reasons'] = ['sideways']
    with pytest.raises(jsonschema.ValidationError):
        validate(bad, 'mimoShockRow')
