"""The hardware and Random control messages against the schema: every
golden validates as a message and as its own payload, and the schema
refuses what the contract says is wrong (PROTOCOL.md, "Hardware and
Random Control")."""

from __future__ import annotations

import copy
import json

import jsonschema
import pytest

from sds_analysis import frames

from .conftest import GOLDEN

#: golden -> (path to its payload, the schema definition the payload follows)
PAYLOADS = {
    'request_random_configure': ('params', 'randomConfigureParams'),
    'response_random_configure': ('result', 'randomConfigureResult'),
    'response_station_list': ('result', 'stationList'),
    'response_channels_get': ('result', 'channelTable'),
    'event_random_state': ('data', 'randomState'),
    'event_session_changed': ('data', 'sessionChanged'),
    'response_error_controller_held': ('error', None),
    'response_error_table_invalid': ('error.detail', 'report'),
}


def _text(name):
    return json.loads((GOLDEN / f'{name}.json').read_text(encoding='utf-8'))


def _at(message, path):
    for key in path.split('.'):
        message = message[key]
    return message


def _live_data():
    wire, _ = frames.to_wire(frames.decode((GOLDEN / 'event_random_live.bin').read_bytes()))
    return wire


def _refused(validate, instance, name):
    with pytest.raises(jsonschema.ValidationError):
        validate(instance, name)


@pytest.mark.parametrize('name', sorted(PAYLOADS))
def test_control_golden_validates(name, validate):
    message = _text(name)
    validate(message)
    path, definition = PAYLOADS[name]
    if definition:
        validate(_at(message, path), definition)


def test_random_live_validates(validate):
    wire = _live_data()
    validate(wire)
    validate(wire['data'], 'randomLive')
    validate(wire['data']['views'], 'views')


def test_every_refusal_carries_a_listed_reason(validate):
    error = _text('response_error_controller_held')
    bad = copy.deepcopy(error)
    bad['error']['reason'] = 'controller_busy'
    _refused(validate, bad, None)
    assert error['error']['detail']['holder'] in error['error']['message']


def test_hardware_source_needs_physical_devices(validate):
    # a driver-simulated station, or the simulator, is never HARDWARE
    selected = _text('response_station_list')['result']['selected']
    for cls in ('ni_simulated', 'sim'):
        _refused(validate, {**selected, 'class': cls}, 'stationSelected')
    validate({**selected, 'class': 'ni_simulated', 'source': 'SIM'}, 'stationSelected')

    live = _live_data()['data']
    _refused(validate, {**live, 'class': 'ni_simulated'}, 'randomLive')

    state = _text('event_random_state')['data']
    station = {**state['station'], 'class': 'ni_simulated'}
    _refused(validate, {**state, 'station': station}, 'randomState')
    validate({**state, 'station': station, 'source': 'SIM'}, 'randomState')


def test_a_breakpoint_is_an_anchor_or_a_slope(validate):
    validate({'freq_hz': 20, 'psd_g2_hz': 0.01}, 'breakpoint')
    validate({'freq_hz': 80, 'slope_db_oct': 3}, 'breakpoint')
    _refused(validate, {'freq_hz': 80, 'psd_g2_hz': 0.04, 'slope_db_oct': 3}, 'breakpoint')
    _refused(validate, {'freq_hz': 80}, 'breakpoint')
    _refused(validate, {'freq_hz': 80, 'psd_g2_hz': 0.04, 'alarm_db': [3]}, 'breakpoint')


def test_channel_rows_carry_every_field(validate):
    row = _text('response_channels_get')['result']['channels'][0]
    assert row['interface'] == 'single_ended'
    validate({**row, 'interface': 'iepe', 'iepe_bias_v': 12}, 'channelRow')
    for field in row:
        _refused(validate, {k: v for k, v in row.items() if k != field}, 'channelRow')
    _refused(validate, {**row, 'interface': 'icp'}, 'channelRow')


def test_finished_runs_say_how_they_ended(validate):
    state = _text('event_random_state')['data']
    _refused(validate, {**state, 'end': None}, 'randomState')
    running = {**state, 'state': 'running', 'phase': 'full_level', 'end': None}
    validate(running, 'randomState')
    _refused(validate, {**running, 'end': state['end']}, 'randomState')
    # only a protection abort carries a trip, and a trip carries its kind's facts
    end = state['end']
    _refused(validate, {**end, 'outcome': 'operator_abort'}, 'runEnd')
    validate({**end, 'outcome': 'operator_abort', 'trip': None}, 'runEnd')
    trip = {k: v for k, v in end['trip'].items() if k != 'worst_db'}
    _refused(validate, {**end, 'trip': trip}, 'runEnd')


def test_an_unconfigured_run_has_no_run_id(validate):
    state = _text('response_random_configure')['result']['state']
    blank = {**state, 'run_id': None, 'state': 'unconfigured', 'title': None, 'source': None,
             'station': None, 'grid': None, 'mode': None, 'levels_db': None,
             'full_level_s': None}
    validate(blank, 'randomState')
    _refused(validate, {**blank, 'run_id': 'r-0008'}, 'randomState')
    _refused(validate, {**state, 'source': None}, 'randomState')


def test_level_names_a_step_or_a_level_not_both(validate):
    validate({'delta_db': 1}, 'randomLevelParams')
    validate({'to_db': -6}, 'randomLevelParams')
    _refused(validate, {'delta_db': 1, 'to_db': -6}, 'randomLevelParams')
    _refused(validate, {}, 'randomLevelParams')


def test_selecting_ni_names_its_devices(validate):
    validate({'kind': 'sim'}, 'stationSelectParams')
    validate({'kind': 'ni', 'devices': ['cDAQ1Mod1']}, 'stationSelectParams')
    _refused(validate, {'kind': 'ni'}, 'stationSelectParams')
    _refused(validate, {'kind': 'ni', 'devices': []}, 'stationSelectParams')


def test_an_unavailable_ni_station_says_why(validate):
    ni = _text('response_station_list')['result']['stations'][1]
    _refused(validate, {**ni, 'available': False}, 'niStation')
    validate({**ni, 'available': False, 'devices': [],
              'unavailable': {'reason': 'ni_driver_unavailable',
                              'message': 'The NI-DAQmx runtime is not installed.'}},
             'niStation')


def test_live_views_come_in_their_stated_order(validate):
    live = _live_data()['data']
    control, drive, reading = live['views']
    _refused(validate, {**live, 'views': [reading, control, drive]}, 'randomLive')
    _refused(validate, {**live, 'views': [control, drive]}, 'randomLive')
