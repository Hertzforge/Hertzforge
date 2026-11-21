"""The Modal Test messages against the schema: every Modal Test golden validates as a
message and as its own payload, the binary live frame decodes to its expected JSON, the
schema refuses what the contract says is wrong, and the golden run folders' setup, check,
end, frames and log read as their records say (PROTOCOL.md, "Modal Test";
docs/run-folder.md, "What a Modal Test Run Saves")."""

from __future__ import annotations

import copy
import json

import jsonschema
import pytest

from hertzforge.service import frames

from .conftest import GOLDEN

#: text golden -> (path to its payload, the schema definition the payload follows)
TEXT = {
    'request_modal_test_configure': ('params', 'modalTestConfigureParams'),
    'request_modal_test_start': ('params', 'modalTestEmptyParams'),
    'request_modal_test_arm': ('params', 'modalTestEmptyParams'),
    'request_modal_test_select': ('params', 'modalTestSelectParams'),
    'request_modal_test_sim_hit': ('params', 'modalTestSimHitParams'),
    'event_modal_test_state': ('data', 'modalTestState'),
    'response_error_modal_test_config_invalid': ('error.detail', 'modalTestInvalidDetail'),
}

#: binary golden -> (path to its payload, the schema definition the payload follows)
BINARY = {
    'event_modal_test_live': ('data', 'modalTestLive'),
}

#: the golden Modal Test run folders, written by the control service's own test
RUNS = GOLDEN / 'runs' / 'modal_test'
FOLDERS = ('impact_roving', 'shaker_burst_random')


def test_modal_test_every_golden_is_checked_here():
    names = {p.name.split('.')[0] for p in GOLDEN.iterdir()
             if '_modal_test_' in p.name and not p.name.endswith('.expected.json')}
    assert names == set(TEXT) | set(BINARY), sorted(names ^ (set(TEXT) | set(BINARY)))
    assert sorted(p.name for p in RUNS.iterdir() if p.is_dir()) == list(FOLDERS)


def _text(name):
    return json.loads((GOLDEN / f'{name}.json').read_text(encoding='utf-8'))


def _json(folder, *parts):
    return json.loads(RUNS.joinpath(folder, *parts).read_text(encoding='utf-8'))


def _jsonl(folder, *parts):
    text = RUNS.joinpath(folder, *parts).read_text(encoding='utf-8')
    return [json.loads(line) for line in text.splitlines() if line.strip()]


def _at(message, path):
    for key in path.split('.'):
        message = message[key]
    return message


def _refused(validate, instance, name):
    with pytest.raises(jsonschema.ValidationError):
        validate(instance, name)


@pytest.mark.parametrize('name', sorted(TEXT))
def test_modal_test_text_golden_validates(name, validate):
    message = _text(name)
    validate(message)
    path, definition = TEXT[name]
    validate(_at(message, path), definition)


def test_modal_test_live_golden_validates_and_decodes_to_its_expected_json(validate):
    raw = (GOLDEN / 'event_modal_test_live.bin').read_bytes()
    wire, _arrays = frames.to_wire(frames.decode(raw))
    validate(wire)
    validate(wire['data'], 'modalTestLive')
    got = frames.decode(raw)['data']
    want = json.loads((GOLDEN / 'event_modal_test_live.expected.json').read_text(
        encoding='utf-8'))['data']
    assert list(got) == list(want)
    for a, b in zip(got['time']['channels'], want['time']['channels'], strict=True):
        assert (a['index'], a['role'], a['unit']) == (b['index'], b['role'], b['unit'])
        assert a['values'].dtype.name == 'float32'
        assert a['values'].tolist() == b['values']['values']
    # every live channel names its row, its part and its unit: the shell draws, computes nothing
    hit = got['hit']
    assert [c['role'] for c in hit['channels']] == sorted(
        (c['role'] for c in hit['channels']), key=lambda r: r != 'reference')
    assert all(len(c['values']) == hit['samples'] for c in hit['channels'])
    w = hit['windows']['force']
    assert w['pre_frames'] + w['width_frames'] + w['fall_frames'] <= hit['samples']
    s = hit['force_spectrum']
    assert len(s['values']) == len(s['frequency_hz']) and s['unit'].endswith('^2/Hz')


def test_modal_test_configuration_takes_its_polarity_and_no_other_key(validate):
    config = _text('request_modal_test_configure')['params']['config']
    validate(config, 'modalTestConfig')
    for polarity in ('positive', 'negative'):
        good = copy.deepcopy(config)
        good['impact']['polarity'] = polarity
        validate(good, 'modalTestConfig')
    bad = copy.deepcopy(config)
    bad['impact']['polarity'] = 'up'
    _refused(validate, bad, 'modalTestConfig')
    # the doc's old name for the trigger is not a key
    bad = copy.deepcopy(config)
    bad['impact']['trigger_pct'] = 10
    _refused(validate, bad, 'modalTestConfig')
    for missing in ('mode', 'fs_hz'):
        bad = copy.deepcopy(config)
        del bad[missing]
        _refused(validate, bad, 'modalTestConfig')


def test_modal_test_states_outcomes_and_refusals_after_review(validate):
    for state in ('arming', 'armed', 'captured'):
        validate(state, 'modalTestRunState')
    _refused(validate, 'waiting', 'modalTestRunState')
    for outcome in ('run_panicked', 'run_not_started', 'source_failed'):
        validate(outcome, 'modalTestOutcome')
    validate('ending', 'reason')
    validate({}, 'modalTestEmptyParams')  # modal_test.clear_point
    _refused(validate, {'point': 1}, 'modalTestEmptyParams')


def test_modal_test_double_hit_names_its_frame_peak_and_ratio(validate):
    report = _text('event_modal_test_state')['data']['latest']['report']
    validate(report, 'modalTestHitReport')
    second = copy.deepcopy(report)
    second['double_hit'] = {'frame': 61, 'peak': 7.5, 'ratio': 0.41}
    validate(second, 'modalTestHitReport')
    second['double_hit'] = {'frame': 61, 'ratio': 0.41}
    _refused(validate, second, 'modalTestHitReport')


def test_modal_test_shaker_live_frame_carries_its_reference_autospectra(validate):
    live = _text('event_modal_test_state')['data']
    arr = {'$array': 'a'}
    frame = {'run_id': live['run_id'], 'state': 'measuring', 'point': 0,
             'time': {'rate_hz': 1024, 'channels': [
                 {'index': 1, 'role': 'reference', 'unit': 'lbf', 'values': arr},
                 {'index': 4, 'role': 'drive', 'unit': 'V', 'values': arr}]},
             'frf': {'frequency_hz': arr, 'averages': 3, 'pairs': [], 'coherence': []},
             'references': {'frequency_hz': arr, 'averages': 3, 'autospectra': [
                 {'index': 1, 'unit': 'lbf^2/Hz', 'values': arr}]}}
    validate(frame, 'modalTestLive')
    frame['time']['channels'][0]['role'] = 'force'
    _refused(validate, frame, 'modalTestLive')


@pytest.mark.parametrize('folder', FOLDERS)
def test_modal_test_golden_run_folder_reads_as_its_records_say(folder, validate):
    run = _json(folder, 'run.json')
    assert run['test'] == 'modal_test' and run['schema'] == 'hertzforge-run/1'
    validate(run['source'], 'source')
    setup = _json(folder, 'setup.json')
    validate(setup['config'], 'modalTestConfig')
    validate(setup['check'], 'modalTestCheck')
    assert setup['check']['mode'] == setup['config']['mode']
    if setup['check']['mode'] == 'impact':
        trigger = setup['check']['impact']['trigger']
        assert trigger['polarity'] in ('positive', 'negative') and trigger['watches']
        assert (trigger['level'] < 0) == (trigger['polarity'] == 'negative')
    end = _json(folder, 'end.json')
    validate(end['end'], 'modalTestEnd')
    validate(end['state'], 'modalTestState')
    assert end['end']['outcome'] == end['state']['end']['outcome']
    outcome = _json(folder, 'results', 'modal_outcome.json')
    assert outcome['outcome'] == end['end']['outcome']
    assert outcome['totals'] == end['totals']
    for line in _jsonl(folder, 'results', 'modal_frames.jsonl'):
        if line.get('report') is not None:
            validate(line['report'], 'modalTestHitReport')
    for entry in _jsonl(folder, 'log.jsonl'):
        validate({k: entry[k] for k in ('seq', 'at_unix', 'kind', 'text')}, 'modalTestLogEntry')
