"""The MIMO Mixed Mode messages against the protocol: every MIMO Mixed Mode
golden validates as a message, and the binary ones decode to their expected
JSON (PROTOCOL.md, "MIMO Mixed Mode")."""

from __future__ import annotations

import json

import pytest

from hertzforge.service import frames

from .conftest import GOLDEN

TEXT = [
    'request_mimo_mixed_configure', 'response_mimo_mixed_configure', 'event_mimo_mixed_state',
    'request_mimo_mixed_level', 'request_mimo_mixed_mode', 'request_mimo_mixed_override',
    'request_mimo_mixed_part', 'request_mimo_mixed_subscribe', 'response_mimo_mixed_subscribe',
    'response_error_mimo_mixed_config_invalid', 'response_error_mimo_mixed_not_built',
    'response_error_mimo_mixed_part_refused',
]
BINARY = ['event_mimo_mixed_live', 'event_mimo_mixed_data']


def test_every_mimo_mixed_golden_is_checked_here():
    # The Rust, TypeScript and Python suites read the same files in schema/golden/.
    names = {p.name.split('.')[0] for p in GOLDEN.iterdir()
             if '_mimo_mixed_' in p.name and not p.name.endswith('.expected.json')}
    assert names == set(TEXT) | set(BINARY), sorted(names ^ (set(TEXT) | set(BINARY)))


def _text(name):
    return json.loads((GOLDEN / f'{name}.json').read_text(encoding='utf-8'))


@pytest.mark.parametrize('name', TEXT)
def test_mimo_mixed_text_golden_validates(name, validate):
    validate(_text(name))


@pytest.mark.parametrize('name', BINARY)
def test_mimo_mixed_binary_golden_validates_and_decodes_to_its_expected_json(name, validate):
    decoded = frames.decode((GOLDEN / f'{name}.bin').read_bytes())
    wire, _arrays = frames.to_wire(decoded)
    validate(wire)
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


def test_the_mimo_mixed_configuration_names_a_law_for_each_role():
    config = _text('request_mimo_mixed_configure')['params']['config']
    assert {'floor', 'tones'} <= set(config['law'])
