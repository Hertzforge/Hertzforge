"""The MIMO Sine messages against the protocol: every MIMO Sine golden
validates as a message, the binary ones decode to their expected JSON, and the
goldens say what the contract says (PROTOCOL.md, "MIMO Sine")."""

from __future__ import annotations

import json

import pytest

from hertzforge.service import frames

from .conftest import GOLDEN

TEXT = [
    'request_mimo_sine_configure', 'response_mimo_sine_configure', 'request_mimo_sine_start',
    'request_mimo_sine_proceed', 'response_mimo_sine_state', 'event_mimo_sine_state',
    'request_mimo_sine_level', 'request_mimo_sine_goto', 'request_mimo_sine_jog',
    'request_mimo_sine_subscribe', 'response_mimo_sine_subscribe',
    'response_error_mimo_sine_config_invalid', 'response_error_mimo_sine_reverse',
    'response_error_mimo_sine_not_a_sweep', 'event_mimo_sine_log', 'event_mimo_sine_report',
]
BINARY = ['event_mimo_sine_live', 'event_mimo_sine_sweep', 'event_mimo_sine_data']

#: every member a MIMO Sine state carries, in the order the service writes them
STATE_KEYS = ['run_id', 'state', 'phase', 'title', 'source', 'station', 'grid', 'clock', 'test',
              'shape', 'phase_specified', 'mode', 'levels_db', 'level_db', 'rung',
              'frequency_hz', 'direction', 'sweeps', 'steps', 'held', 'request_held',
              'elapsed_s', 'at_level_s', 'remaining_s', 'alarm', 'notch', 'notch_scale',
              'identification', 'review', 'law', 'feasibility', 'end', 'record']


def test_every_mimo_sine_golden_is_checked_here():
    # The Rust, TypeScript and Python suites read the same files in schema/golden/.
    names = {p.name.split('.')[0] for p in GOLDEN.iterdir()
             if '_mimo_sine_' in p.name and not p.name.endswith('.expected.json')}
    assert names == set(TEXT) | set(BINARY), sorted(names ^ (set(TEXT) | set(BINARY)))


def _text(name):
    return json.loads((GOLDEN / f'{name}.json').read_text(encoding='utf-8'))


@pytest.mark.parametrize('name', TEXT)
def test_mimo_sine_text_golden_validates(name, validate):
    validate(_text(name))


@pytest.mark.parametrize('name', BINARY)
def test_mimo_sine_binary_golden_validates_and_decodes_to_its_expected_json(name, validate):
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


def test_the_mimo_sine_states_carry_every_member_in_order():
    for name, path in (('response_mimo_sine_state', 'result'), ('event_mimo_sine_state', 'data'),
                       ('response_mimo_sine_configure', 'result')):
        state = _text(name)[path]
        state = state.get('state', state) if path == 'result' and 'derived' in state else state
        assert list(state) == STATE_KEYS, name


def test_the_mimo_sine_configuration_carries_every_field_and_the_record():
    config = _text('request_mimo_sine_configure')['params']['config']
    assert set(config) == {'title', 'channels', 'phase_specified', 'test', 'acquisition',
                           'schedule', 'identification', 'control', 'requests', 'limits',
                           'feasibility', 'simulator', 'operator', 'article', 'notes',
                           'specification', 'record', 'metadata'}
    assert 'capture' not in config and 'cola' not in config
    derived = _text('response_mimo_sine_configure')['result']['derived']
    assert derived['law']['test_types'] == ['sweep', 'dwell', 'stepped']
    assert derived['law']['amplitude_only'] is True


def test_the_mimo_sine_live_frame_names_every_control_limit_and_drive():
    live = json.loads((GOLDEN / 'event_mimo_sine_live.expected.json').read_text())['data']
    assert [c['channel'] for c in live['controls']] == [1, 2, 3, 4]
    assert [c['channel'] for c in live['limits']] == [5, 6]
    assert [d['channel'] for d in live['drives']] == [7, 8, 9, 10]
    assert [v['title'] for v in live['views']] == ['Controls', 'Drives', 'Run']
