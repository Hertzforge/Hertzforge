"""The classical shock messages against the schema: every shock golden validates as
a message and as its own payload, the binary ones decode to their expected JSON,
and the schema refuses what the contract says is wrong (PROTOCOL.md, "Classical
Shock" and "Control Laws")."""

from __future__ import annotations

import copy
import json

import jsonschema
import pytest

from hertzforge.service import frames

from .conftest import GOLDEN

#: text golden -> (path to its payload, the schema definition the payload follows)
TEXT = {
    'request_shock_configure': ('params', 'shockConfigureParams'),
    'response_shock_configure': ('result', 'shockConfigureResult'),
    'event_shock_state': ('data', 'shockState'),
    'request_shock_subscribe': ('params', 'shockSubscribeParams'),
    'response_shock_subscribe': ('result', 'shockSubscribeResult'),
    'request_shock_fire': ('params', None),
    'request_shock_level': ('params', 'shockLevelParams'),
    'response_laws_list_shock_classical': ('result', 'lawsListResult'),
    'response_laws_check_shock_classical': ('result', 'lawsCheckResult'),
    'response_error_shock_config_invalid': ('error.detail', 'report'),
}

#: binary golden -> (path to its payload, the schema definition the payload follows)
BINARY = {
    'event_shock_live': ('data', 'shockLive'),
    'event_shock_pulse': ('data', 'shockPulseEvent'),
    'event_shock_data': ('data', 'shockData'),
    'response_shock_preview': ('result', 'shockPreviewResult'),
}


def test_every_shock_golden_is_checked_here():
    # The Rust, TypeScript and Python suites read the same files in schema/golden/.
    names = {p.name.split('.')[0] for p in GOLDEN.iterdir()
             if 'shock' in p.name and 'shock_srs' not in p.name
             and 'mimo_shock' not in p.name and 'shock_mimo' not in p.name
             and not p.name.endswith('.expected.json')}
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


def _refused(validate, instance, name):
    with pytest.raises(jsonschema.ValidationError):
        validate(instance, name)


@pytest.mark.parametrize('name', sorted(TEXT))
def test_shock_text_golden_validates(name, validate):
    message = _text(name)
    validate(message)
    path, definition = TEXT[name]
    if definition:
        validate(_at(message, path), definition)


@pytest.mark.parametrize('name', sorted(BINARY))
def test_shock_binary_golden_validates_and_decodes_to_its_expected_json(name, validate):
    wire = _wire(name)
    validate(wire)
    path, definition = BINARY[name]
    validate(_at(wire, path), definition)
    decoded = frames.decode((GOLDEN / f'{name}.bin').read_bytes())
    expected = json.loads((GOLDEN / f'{name}.expected.json').read_text(encoding='utf-8'))

    def same(a, b, where):
        if isinstance(b, dict) and set(b) == {'dtype', 'shape', 'values'}:
            assert a.dtype.name == {'f64': 'float64', 'f32': 'float32', 'u32': 'uint32',
                                    'i32': 'int32', 'u8': 'uint8'}[b['dtype']], where
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


def test_the_live_frame_is_binary_with_no_arrays():
    raw = (GOLDEN / 'event_shock_live.bin').read_bytes()
    assert raw[:4] == b'SDS1'
    header = json.loads(raw[8:8 + int.from_bytes(raw[4:8], 'little')].decode('utf-8'))
    assert header['arrays'] == []
    assert header['data']['fire'] == {'done': 2048, 'of': 8192}


def test_a_configuration_carries_every_field_and_the_drive_limits_are_required(validate):
    config = _text('request_shock_configure')['params']['config']
    assert set(config) == {'title', 'pulse', 'standard', 'acquisition', 'schedule', 'spacing',
                           'identification', 'control', 'limits', 'feasibility', 'train',
                           'gunfire', 'capture', 'metadata'}
    for missing in ('drive_vpk_ceiling', 'drive_vpk_abort'):
        bad = copy.deepcopy(config)
        del bad['limits'][missing]
        _refused(validate, bad, 'shockConfig')
    bad = copy.deepcopy(config)
    bad['pulse']['shape'] = 'square'
    _refused(validate, bad, 'shockConfig')
    trapezoid = copy.deepcopy(config)
    trapezoid['pulse'].update(shape='trapezoid', rise_frac=0.1, fall_frac=0.1)
    validate(trapezoid, 'shockConfig')
    trapezoid['pulse']['fall_frac'] = None
    _refused(validate, trapezoid, 'shockConfig')
    # the minimal configuration: the pulse, the schedule and the drive limits
    validate({'pulse': {'shape': 'half_sine', 'amplitude_g': 1, 'duration_ms': 11},
              'schedule': {'mode': 'manual', 'levels_db': [0], 'pulses': [1]},
              'limits': {'drive_vpk_ceiling': 0.5, 'drive_vpk_abort': 0.8}}, 'shockConfig')


def test_the_state_ends_exactly_when_finished_and_a_trip_names_its_facts(validate):
    state = _text('event_shock_state')['data']
    assert state['state'] == 'running' and state['end'] is None
    finished = {**state, 'state': 'finished', 'phase': 'aborted', 'next_pulse_s': None}
    _refused(validate, finished, 'shockState')
    trip = {'kind': 'drive_vpk', 'pulse': 0, 'at_level_db': 0, 'vpk': 0.4, 'abort_vpk': 0.38,
            'predicted': True}
    finished['end'] = {'outcome': 'aborted', 'message': 'The predicted drive passes the abort.',
                       'at_s': 4.2, 'level_db': 0, 'pulse': 0, 'trip': trip}
    validate(finished, 'shockState')
    for facts in ({'kind': 'tolerance', 'envelope_pass': False, 'velocity_change_pass': True,
                   'error_pct': 18.2, 'worst_deviation_frac': 0.19},
                  {'kind': 'overshoot', 'peak_ratio': 1.58, 'limit_ratio': 1.3},
                  {'kind': 'kinematic', 'axis': 'displacement', 'value': 0.62, 'limit': 0.55,
                   'predicted': True},
                  {'kind': 'accel', 'predicted_g': 31, 'limit_g': 25},
                  {'kind': 'monitor', 'channel': 2, 'value': 44, 'limit': 40},
                  {'kind': 'divergence', 'pulses': 3, 'error_pct': 41},
                  {'kind': 'not_converged', 'limit': 'per_level', 'pulses': 5},
                  {'kind': 'law_fault', 'reason': 'over_budget', 'consecutive': 3}):
        validate({'pulse': 4, 'at_level_db': -12, **facts}, 'shockTrip')
    _refused(validate, {'kind': 'drive_vpk', 'pulse': 0, 'at_level_db': 0, 'vpk': 0.4},
             'shockTrip')
    _refused(validate, {'kind': 'law_fault', 'pulse': 0, 'at_level_db': 0, 'reason': 'slow',
                        'consecutive': 3}, 'shockTrip')
    # a completed run carries no trip; an aborted one must
    done = {'outcome': 'completed', 'message': 'The schedule finished.', 'at_s': 30,
            'level_db': 0, 'pulse': 6, 'trip': None}
    validate(done, 'shockEnd')
    _refused(validate, {**done, 'outcome': 'aborted'}, 'shockEnd')


def test_a_row_is_judged_or_unmeasured_and_its_law_call_says_how_it_was_used(validate):
    row = _text('event_shock_state')['data']['last_pulse']
    validate(row, 'shockRow')
    unmeasured = {**row, 'peak_g': None, 'peak_ratio': None, 'error_pct': None,
                  'velocity_change_ips': None, 'velocity_change_ratio': None, 'verdicts': None,
                  'passed': None, 'srs': None, 'law': None, 'origin': 'train'}
    validate(unmeasured, 'shockRow')
    missed = {**row, 'law': {**row['law'], 'used': False, 'miss': 'over_budget'}}
    validate(missed, 'shockRow')
    _refused(validate, {**row, 'law': {**row['law'], 'miss': 'late'}}, 'shockRow')
    _refused(validate, {**row, 'origin': 'manual'}, 'shockRow')


def test_a_level_is_one_step_or_a_rung(validate):
    validate({'delta_db': 1}, 'shockLevelParams')
    validate({'delta_db': -1}, 'shockLevelParams')
    validate({'rung': 0}, 'shockLevelParams')
    for bad in ({'delta_db': 2}, {'rung': -1}, {'delta_db': 1, 'rung': 2}, {}):
        _refused(validate, bad, 'shockLevelParams')


def test_a_subscription_takes_points_on_the_time_histories_only(validate):
    validate({'plots': [{'id': 'p', 'quantity': 'pulse', 'points': 512}]}, 'shockSubscribeParams')
    validate({'default': True}, 'shockSubscribeParams')
    for bad in ({'id': 'p', 'quantity': 'srs', 'points': 512},
                {'id': 'p', 'quantity': 'pulse', 'points': 10},
                {'id': 'p', 'quantity': 'control'}):
        _refused(validate, {'plots': [bad]}, 'shockSubscribeParams')
    result = _text('response_shock_subscribe')['result']
    assert result['quantities'] == ['pulse', 'velocity', 'displacement', 'srs', 'spectrum',
                                    'identification', 'law', 'readings', 'pulses']


def test_the_data_frame_carries_each_plot_with_its_own_views():
    data = _wire('event_shock_data')['data']
    by_id = {p['id']: p for p in data['plots']}
    assert [v['title'] for v in by_id['pulse']['views']] == ['Pulse', 'Drive']
    assert [v['title'] for v in by_id['id']['views']] == [
        'Identification Gain', 'Identification Phase', 'Coherence']
    assert [v['view'] for v in by_id['table']['views']] == ['table']
    coherence = by_id['id']['views'][2]['series']
    assert [s['name'] for s in coherence] == ['Coherence', 'Trusted bins', 'Floor']


def test_the_pulse_event_carries_the_row_and_the_default_views():
    data = _wire('event_shock_pulse')['data']
    assert data['pulse']['index'] == 2
    assert [v['title'] for v in data['views']] == ['Pulse', 'Drive', 'SRS']
    pulse = data['views'][0]
    assert [s['role'] for s in pulse['series']] == ['reference', 'tolerance', 'tolerance',
                                                    'measured']
    assert pulse['x']['unit'] == 's' and pulse['y']['unit'] == 'g'
    assert data['views'][1]['y']['unit'] == 'V'


def test_the_classical_law_entry_and_its_check_say_which_world_they_are_for(validate):
    entry = _text('response_laws_list_shock_classical')['result']['laws'][0]
    assert entry['world'] == 'shock_classical' and entry['builtin'] and entry['check']['hardware']
    assert entry['shapes'] is None and entry['needs'] is None
    assert all(d['layout'] is None for d in entry['diagnostics'])
    _refused(validate, {**entry, 'shapes': ['square']}, 'lawEntry')
    validate({'world': 'shock_classical'}, 'lawsListParams')
    report = _text('response_laws_check_shock_classical')['result']['report']
    assert report['tool_version'] == 'shock-classical-check 1' and not report['recorded']
    assert len(report['cases']) == 7
    for case in report['cases']:
        validate(case, 'shockLawCheckCase')
    event = {'law_id': 'fd_correct', 'index': 6, 'of': 7, 'case': 'anti-resonance',
             'state': 'done', 'result': report['cases'][6]}
    validate(event, 'lawsCheckEvent')


def test_every_new_refusal_and_item_is_a_listed_reason(validate):
    error = _text('response_error_shock_config_invalid')
    validate(error)
    for reason in ('not_yet', 'wrong_mode', 'drive_ceiling', 'config_invalid', 'not_a_rung',
                   'law_not_checked', 'shutting_down', 'run_active', 'feasibility_exceeded'):
        e = copy.deepcopy(error)
        e['error']['reason'] = reason
        validate(e)
    e = copy.deepcopy(error)
    e['error']['reason'] = 'too_soon'
    _refused(validate, e, None)
    for item in ('pulses_length_mismatch', 'record_too_long', 'spacing_below_floor',
                 'quiet_window_exceeded', 'levels_empty', 'band_above_grid',
                 'limit_unknown_channel', 'law_wrong_world'):
        validate({'reason': item, 'field': 'pulse', 'message': 'x'}, 'reportItem')
    for kind in ('pulse', 'law'):
        validate({'seq': 0, 'at_unix': 1.5, 'kind': kind, 'text': 'x'}, 'logEntry')
