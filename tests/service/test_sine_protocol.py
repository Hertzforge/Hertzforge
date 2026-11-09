"""The Sine messages against the schema: every Sine golden validates as a message
and as its own payload, the binary ones decode to their expected JSON, and the
schema refuses what the contract says is wrong (PROTOCOL.md, "Sine" and "Control
Laws")."""

from __future__ import annotations

import copy
import json

import jsonschema
import pytest

from hertzforge.service import frames

from .conftest import GOLDEN

#: text golden -> (path to its payload, the schema definition the payload follows)
TEXT = {
    'request_sine_configure': ('params', 'sineConfigureParams'),
    'response_sine_configure': ('result', 'sineConfigureResult'),
    'event_sine_state': ('data', 'sineState'),
    'request_sine_subscribe': ('params', 'sineSubscribeParams'),
    'response_sine_subscribe': ('result', 'sineSubscribeResult'),
    'request_sine_level': ('params', 'sineLevelParams'),
    'request_sine_adjust': ('params', 'sineAdjustParams'),
    'response_sine_profile_import': ('result', 'sineProfileImportResult'),
    'response_laws_list_sine': ('result', 'lawsListResult'),
    'response_laws_check_sine': ('result', 'lawsCheckResult'),
    'response_error_sine_config_invalid': ('error.detail', 'report'),
}

#: binary golden -> (path to its payload, the schema definition the payload follows)
BINARY = {
    'event_sine_live': ('data', 'sineLive'),
    'event_sine_data': ('data', 'sineData'),
    'response_sine_profile_validate': ('result', 'sineProfileValidateResult'),
}


def test_every_sine_golden_is_checked_here():
    # The Rust, TypeScript and Python suites read the same files in schema/golden/.
    names = {p.name.split('.')[0] for p in GOLDEN.iterdir()
             if ('_sine_' in p.name or p.name.split('.')[0].endswith('_sine'))
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
def test_sine_text_golden_validates(name, validate):
    message = _text(name)
    validate(message)
    path, definition = TEXT[name]
    validate(_at(message, path), definition)


@pytest.mark.parametrize('name', sorted(BINARY))
def test_sine_binary_golden_validates_and_decodes_to_its_expected_json(name, validate):
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


def test_a_configuration_carries_every_field_and_the_drive_limits_are_required(validate):
    config = _text('request_sine_configure')['params']['config']
    assert set(config) == {'title', 'profile', 'test', 'acquisition', 'schedule',
                           'identification', 'control', 'limits', 'cola', 'feasibility',
                           'capture', 'metadata'}
    for missing in ('drive_vpk_ceiling', 'drive_vpk_abort'):
        bad = copy.deepcopy(config)
        del bad['limits'][missing]
        _refused(validate, bad, 'sineConfig')
    for key, value in (('test', {'kind': 'random'}),
                       ('test', {'kind': 'sweep', 'band_hz': [20, 2000],
                                 'rate': {'law': 'log', 'oct_per_min': 2},
                                 'end': {'sweeps': None, 'duration_s': None}}),
                       ('test', {'kind': 'dwell', 'freq_hz': 100,
                                 'end': {'seconds': 10, 'cycles': 100}}),
                       ('profile', {'rows': [{'freq_hz': 20, 'quantity': 'accel_g',
                                              'value': 1}]}),
                       ('profile', {'rows': [{'freq_hz': 20, 'quantity': 'g', 'value': 1},
                                             {'freq_hz': 40, 'quantity': 'g', 'value': 1}]})):
        bad = copy.deepcopy(config)
        bad[key] = value
        _refused(validate, bad, 'sineConfig')
    # the other three tests
    for test in ({'kind': 'dwell', 'freq_hz': 250, 'end': {'cycles': 5000}},
                 {'kind': 'step', 'band_hz': [20, 2000], 'steps_per_octave': 3,
                  'spacing': 'log', 'dwell': None},
                 {'kind': 'rstd', 'band_hz': [20, 2000], 'level_db': -12,
                  'rate': {'law': 'linear', 'hz_per_s': 1}, 'tracking_bw_hz': 2,
                  'dwell_mode': 'phase_tracked', 'dwell': {'seconds': 60}}):
        good = copy.deepcopy(config)
        good['test'] = test
        good['schedule']['per_level'] = {'seconds': 10}
        good['schedule']['full_level'] = {'seconds': 10}
        validate(good, 'sineConfig')
    # the minimal configuration: the profile, the test and the drive limits
    validate({'profile': {'rows': [{'freq_hz': 20, 'quantity': 'accel_g', 'value': 1},
                                   {'freq_hz': 2000, 'quantity': 'accel_g', 'value': 1}]},
              'test': {'kind': 'sweep', 'band_hz': [20, 2000],
                       'rate': {'law': 'log', 'oct_per_min': 2}, 'end': {'sweeps': 1}},
              'limits': {'drive_vpk_ceiling': 0.5, 'drive_vpk_abort': 0.8}}, 'sineConfig')


def test_the_derived_values_say_the_grid_the_stop_and_the_law():
    derived = _text('response_sine_configure')['result']['derived']
    assert derived['grid'] == {'fs_hz': 16384, 'tick_frames': 164, 'tick_s': 0.0100098,
                               'actual': False}
    assert derived['stop_ramp_db_per_s'] == 120 and derived['safe_stop_s'] == 0.95
    assert derived['law']['live'] == ['blend_w']
    assert derived['identification_s'] == 128


def test_the_state_ends_exactly_when_finished_and_a_trip_names_its_facts(validate):
    state = _text('event_sine_state')['data']
    assert state['state'] == 'running' and state['end'] is None
    trip = {'kind': 'drive_vpk', 'at_hz': 100, 'at_level_db': 0, 'at_tick': 120,
            'vpk': 0.27, 'abort_vpk': 0.25}
    finished = {**state, 'state': 'finished', 'phase': 'aborted',
                'end': {'outcome': 'aborted', 'message': 'The drive reached its abort.',
                        'at_s': 4.2, 'level_db': 0, 'at_hz': 100, 'ramp_s': None,
                        'trip': trip}}
    validate(finished, 'sineState')
    for facts in ({'kind': 'abort_band', 'side': 'high', 'measured_g': 1.5, 'limit_g': 1.41},
                  {'kind': 'kinematic', 'axis': 'displacement', 'value': 0.0098,
                   'limit': 0.008},
                  {'kind': 'limit_channel', 'channel': 3, 'measured': 3.5, 'limit': 3},
                  {'kind': 'servo_fault', 'consecutive_ticks': 50},
                  {'kind': 'watchdog', 'channel': 1, 'axis': 'accel', 'value': 9,
                   'limit': 8, 'measured': 0.5, 'drive_vpk': 0.4},
                  {'kind': 'oscillator_range', 'requested_hz': 9000, 'fs_hz': 16384},
                  {'kind': 'divergence', 'capped_s': 1.0},
                  {'kind': 'law_fault', 'reason': 'over_budget', 'consecutive': 3}):
        validate({'at_hz': 100, 'at_level_db': 0, 'at_tick': 12, **facts}, 'sineTrip')
    for bad in ({'kind': 'law_fault', 'reason': 'gain_rise', 'consecutive': 3},
                {'kind': 'envelope', 'abort_bands': 2},
                {'kind': 'drive_vpk', 'vpk': 0.4}):
        _refused(validate, {'at_hz': 100, 'at_level_db': 0, 'at_tick': 12, **bad}, 'sineTrip')
    done = {'outcome': 'completed', 'message': 'The schedule finished.', 'at_s': 797.3,
            'level_db': 0, 'at_hz': 2000, 'ramp_s': 0.95, 'trip': None}
    validate(done, 'sineEnd')
    _refused(validate, {**done, 'outcome': 'law_fault'}, 'sineEnd')
    _refused(validate, {**done, 'outcome': 'not_converged'}, 'sineEnd')


def test_the_live_frame_has_its_three_views_and_one_rate():
    data = _wire('event_sine_live')['data']
    assert [v['title'] for v in data['views']] == ['Control', 'Drive', 'Run']
    assert [s['name'] for s in data['views'][0]['series']] == [
        'Control', 'Profile', 'Alarm high', 'Alarm low', 'Abort high', 'Abort low']
    assert data['sweep_rate_oct_per_min'] == -2 and 'sweep_rate_hz_per_s' not in data
    assert data['t_s'] >= data['elapsed_s']


def test_a_live_frame_carries_exactly_one_sweep_rate(validate):
    data = _wire('event_sine_live')['data']
    validate(data, 'sineLive')
    both = {**data, 'sweep_rate_hz_per_s': 1}
    _refused(validate, both, 'sineLive')
    linear = {k: v for k, v in data.items() if k != 'sweep_rate_oct_per_min'}
    linear['sweep_rate_hz_per_s'] = -10.5
    validate(linear, 'sineLive')
    neither = {k: v for k, v in data.items() if k != 'sweep_rate_oct_per_min'}
    _refused(validate, neither, 'sineLive')


def test_a_subscription_takes_channels_and_points_where_they_apply(validate):
    validate({'plots': [{'id': 'p', 'quantity': 'phase', 'channels': 'control'}]},
             'sineSubscribeParams')
    validate({'plots': [{'id': 't', 'quantity': 'time', 'channels': [1], 'points': 512}]},
             'sineSubscribeParams')
    validate({'default': True}, 'sineSubscribeParams')
    for bad in ({'id': 'p', 'quantity': 'control', 'channels': [1]},
                {'id': 'p', 'quantity': 'thd', 'points': 512},
                {'id': 'p', 'quantity': 'phase', 'points': 512},
                {'id': 'p', 'quantity': 'coherence'}):
        _refused(validate, {'plots': [bad]}, 'sineSubscribeParams')
    result = _text('response_sine_subscribe')['result']
    assert result['quantities'] == ['control', 'drive', 'response', 'phase', 'thd', 'time',
                                    'identification', 'law', 'readings']


def test_the_data_frame_carries_each_plot_with_its_own_views():
    data = _wire('event_sine_data')['data']
    by_id = {p['id']: p for p in data['plots']}
    assert [v['title'] for v in by_id['resp']['views']] == ['Channel 1: Control',
                                                           'Channel 3: Fixture']
    assert [s['role'] for s in by_id['resp']['views'][1]['series']] == ['measured', 'limit']
    assert by_id['ph']['views'][0]['y'] == {'label': 'Phase', 'unit': 'deg',
                                            'scale': 'linear'}
    assert [v['title'] for v in by_id['id']['views']] == ['Identification Gain', 'Coherence']
    assert [v['view'] for v in by_id['law']['views']] == ['reading']
    assert by_id['ctl']['channels'] is None and by_id['t1']['channels'] == [1]


def test_a_validated_profile_is_a_computation_with_three_views():
    result = _wire('response_sine_profile_validate')['result']
    assert [v['title'] for v in result['views']] == ['Profile', 'Velocity', 'Displacement']
    assert all(s['role'] != 'measured' for v in result['views'] for s in v['series'])
    assert result['kinematics']['verdict'] == 'pass'


def test_an_import_returns_what_a_configuration_takes_or_names_the_row(validate):
    result = _text('response_sine_profile_import')['result']
    validate(result['profile'], 'sineProfile')
    refused = {'profile': None, 'errors': [{'reason': 'profile_file', 'row': 2,
                                            'message': 'Data row 2: cannot parse frequency.'}],
               'warnings': []}
    validate(refused, 'sineProfileImportResult')
    validate({'text': 'freq_hz,accel_g\n20,1\n2000,1\n'}, 'sineProfileImportParams')
    validate({'path': '/data/profile.csv'}, 'sineProfileImportParams')
    _refused(validate, {'text': 'x', 'path': '/data/profile.csv'}, 'sineProfileImportParams')
    _refused(validate, {}, 'sineProfileImportParams')


def test_the_run_verbs_take_their_own_params(validate):
    for ok, name in (({'delta_db': -1}, 'sineLevelParams'), ({'to_db': -6}, 'sineLevelParams'),
                     ({'mode': 'manual'}, 'sineModeParams'), ({'delta_hz': 10}, 'sineJogParams'),
                     ({'hz': 300}, 'sineGotoParams'),
                     ({'key': 'blend_w', 'value': 0.2}, 'sineAdjustParams')):
        validate(ok, name)
    for bad, name in (({}, 'sineLevelParams'), ({'delta_db': -1, 'to_db': -6}, 'sineLevelParams'),
                      ({'mode': 'step'}, 'sineModeParams'), ({'hz': '300'}, 'sineGotoParams'),
                      ({'key': 'blend_w'}, 'sineAdjustParams')):
        _refused(validate, bad, name)
    validate({'key': 'kp', 'why': 'not_live'}, 'sineAdjustRefusal')
    _refused(validate, {'key': 'kp', 'why': 'nope'}, 'sineAdjustRefusal')


def test_a_sine_law_entry_alone_carries_live_and_its_check_says_which_world(validate):
    entry = _text('response_laws_list_sine')['result']['laws'][0]
    assert entry['world'] == 'sine' and entry['builtin'] and entry['live'] == ['blend_w']
    validate(entry, 'lawEntry')
    without = {k: v for k, v in entry.items() if k != 'live'}
    _refused(validate, without, 'lawEntry')
    other = _text('response_laws_list_shock_srs')['result']['laws'][0]
    _refused(validate, {**other, 'live': []}, 'lawEntry')
    validate({'world': 'sine'}, 'lawsListParams')
    report = _text('response_laws_check_sine')['result']['report']
    assert report['tool_version'] == 'sine-check 2' and not report['recorded']
    assert [c['name'] for c in report['cases']] == [
        'slow sweep', 'fast sweep', 'close modes', 'anti-resonance', 'measurement noise',
        'level ladder', 'limit channel', 'transport delay']
    for case in report['cases']:
        validate(case, 'sineLawCheckCase')
        _refused(validate, case, 'srsLawCheckCase')
    assert report['cases'][7]['requirement'] == 'settles'
    event = {'law_id': 'servo_blend', 'index': 7, 'of': 8, 'case': 'transport delay',
             'state': 'done', 'result': report['cases'][7]}
    validate(event, 'lawsCheckEvent')


def test_every_new_reason_is_listed(validate):
    validate(_text('response_error_sine_config_invalid'))
    for item in ('profile_too_short', 'value_not_positive', 'quantity_ambiguous',
                 'profile_not_covering', 'start_not_a_level', 'end_before_full_level',
                 'tick_out_of_range', 'abort_below_ceiling', 'limit_on_control_channel',
                 'cola_needs_two_outputs', 'safe_stop_budget', 'tracking_lag', 'profile_file',
                 'join_discontinuity', 'units_column_ignored'):
        validate({'reason': item, 'field': 'profile', 'message': 'x'}, 'reportItem')
    for reason in ('not_a_sweep', 'out_of_band', 'adjust_refused'):
        validate({'type': 'response', 'id': 1,
                  'error': {'code': 'refused', 'reason': reason, 'message': 'x'}})
