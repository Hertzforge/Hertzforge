"""The Playback messages against the schema: every Playback golden validates as a message
and as its own payload, the binary one decodes to its expected JSON, the schema refuses
what the contract says is wrong, and the golden run folder's setup, check, outcome, end
and log read as their records say (PROTOCOL.md, "Playback"; docs/run-folder.md)."""

from __future__ import annotations

import copy
import json

import jsonschema
import pytest

from hertzforge.service import frames

from .conftest import GOLDEN

#: text golden -> (path to its payload, the schema definition the payload follows)
TEXT = {
    'request_playback_configure': ('params', 'playbackConfigureParams'),
    'request_playback_start': ('params', 'playbackEmptyParams'),
    'request_playback_log': ('params', 'playbackLogParams'),
    'event_playback_state': ('data', 'playbackState'),
    'response_error_playback_config_invalid': ('error.detail', 'playbackInvalidDetail'),
}

#: binary golden -> (path to its payload, the schema definition the payload follows)
BINARY = {
    'event_playback_live': ('data', 'playbackLive'),
}

#: the golden Playback run folder, written by the control service's own test
RUN = GOLDEN / 'runs' / 'playback'


def test_every_playback_golden_is_checked_here():
    # The Rust, TypeScript and Python suites read the same files in schema/golden/; the
    # run folder is read by the tests below.
    names = {p.name.split('.')[0] for p in GOLDEN.iterdir()
             if '_playback_' in p.name and not p.name.endswith('.expected.json')}
    assert names == set(TEXT) | set(BINARY), sorted(names ^ (set(TEXT) | set(BINARY)))


def _text(name):
    return json.loads((GOLDEN / f'{name}.json').read_text(encoding='utf-8'))


def _json(*parts):
    return json.loads(RUN.joinpath(*parts).read_text(encoding='utf-8'))


def _at(message, path):
    for key in path.split('.'):
        message = message[key]
    return message


def _refused(validate, instance, name):
    with pytest.raises(jsonschema.ValidationError):
        validate(instance, name)


@pytest.mark.parametrize('name', sorted(TEXT))
def test_playback_text_golden_validates(name, validate):
    message = _text(name)
    validate(message)
    path, definition = TEXT[name]
    validate(_at(message, path), definition)


def test_playback_live_golden_validates_and_decodes_to_its_expected_json(validate):
    raw = (GOLDEN / 'event_playback_live.bin').read_bytes()
    wire, _arrays = frames.to_wire(frames.decode(raw))
    validate(wire)
    validate(wire['data'], 'playbackLive')
    decoded = frames.decode(raw)
    expected = json.loads((GOLDEN / 'event_playback_live.expected.json').read_text(
        encoding='utf-8'))
    assert decoded['topic'] == expected['topic'] == 'playback.live'
    got, want = decoded['data'], expected['data']
    assert list(got) == list(want)
    assert got['progress'] == want['progress']
    for a, b in zip(got['time']['channels'], want['time']['channels'], strict=True):
        assert (a['index'], a['role'], a['unit']) == (b['index'], b['role'], b['unit'])
        assert a['values'].dtype.name == 'float32'
        assert a['values'].tolist() == b['values']['values']
    # inputs first in table order, then drives
    roles = [c['role'] for c in got['time']['channels']]
    assert roles == sorted(roles, key=lambda r: r != 'input')


def test_playback_configuration_takes_no_other_key_and_needs_its_drive(validate):
    config = _text('request_playback_configure')['params']['config']
    validate(config, 'playbackConfig')
    for key, value in (('data_root', '/tmp'), ('repeats', 2), ('aborts', [])):
        bad = copy.deepcopy(config)
        bad[key] = value
        _refused(validate, bad, 'playbackConfig')
    for missing in ('drive', 'map', 'ceiling_v'):
        bad = copy.deepcopy(config)
        del bad[missing]
        _refused(validate, bad, 'playbackConfig')
    for bad_drive in ({'folder': '/d'}, {'folder': '/d', 'sha256': 'abc'},
                      {'folder': '/d', 'sha256': '0' * 64, 'rate_hz': 1000}):
        bad = copy.deepcopy(config)
        bad['drive'] = bad_drive
        _refused(validate, bad, 'playbackConfig')
    # only the configuration's own defaults are optional
    validate({'drive': config['drive'], 'map': [2], 'ceiling_v': 1}, 'playbackConfig')


def test_the_state_golden_counts_agree_with_each_other():
    s = _text('event_playback_state')['data']
    p, fs = s['progress'], s['fs_hz']
    assert p['samples'] == s['drive']['samples']
    assert p['played_s'] == p['played'] / fs and p['fraction'] == p['played'] / p['samples']
    assert s['drive']['duration_s'] == s['drive']['samples'] / fs
    assert [r['index'] for r in s['drives']] == s['drive']['map']
    assert [o['channel'] for o in s['overloads']] == [r['index'] for r in s['inputs']]


def test_an_unconfigured_state_and_a_file_that_cannot_be_read_have_their_short_forms(validate):
    validate({'run_id': None, 'state': 'unconfigured'}, 'playbackState')
    _refused(validate, {'run_id': None, 'state': 'configured'}, 'playbackState')
    unread = {'pass': False, 'refusals': [{'reason': 'drive_unreadable', 'message': 'gone',
                                           'detail': {'path': '/d', 'reason': 'no file'}}]}
    validate(unread, 'playbackCheck')
    _refused(validate, {'pass': True, 'refusals': unread['refusals']}, 'playbackCheck')
    validate({'errors': [{'reason': 'drive_unreadable', 'field': 'drive', 'message': 'gone'}],
              'warnings': [], 'check': unread, 'derived': None}, 'playbackDryRunResult')


def test_the_golden_run_folder_reads_as_its_records_say(validate):
    run = _json('run.json')
    assert run['test'] == 'playback' and run['schema'] == 'hertzforge-run/1'
    validate(run['source'], 'source')
    setup = _json('setup.json')
    validate(setup['config'], 'playbackConfig')
    validate(setup['check'], 'playbackCheck')
    check = setup['check']
    assert check['pass'] and check['refusals'] == []
    assert check['sha256'] == setup['config']['drive']['sha256']
    assert [c['drive_row'] for c in check['channels']] == setup['config']['map']
    for c in check['channels']:
        assert c['pass'] and c['faults'] == []
        assert c['played_peak_v'] == pytest.approx(c['peak_v'] * 10 ** (check['level_db'] / 20))
    result = _json('results', 'playback_check.json')
    assert result['name'] == 'playback_check'
    validate(result['report'], 'playbackCheckReport')
    end = _json('end.json')
    validate(end['end'], 'playbackEnd')
    validate(end['state'], 'playbackState')
    assert end['end']['outcome'] == end['state']['end']['outcome'] == 'completed'
    outcome = _json('results', 'playback_outcome.json')
    assert outcome['outcome'] == end['end']['outcome']
    assert outcome['samples'] == end['end']['played'] == check['samples']
    for line in (RUN / 'log.jsonl').read_text(encoding='utf-8').splitlines():
        entry = json.loads(line)
        validate({k: entry[k] for k in ('seq', 'at_unix', 'kind', 'text')}, 'playbackLogEntry')
