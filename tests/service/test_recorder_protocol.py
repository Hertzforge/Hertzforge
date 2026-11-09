"""The Recorder messages against the schema: every Recorder golden validates as a message
and as its own payload, the binary ones decode to their expected JSON, the schema refuses
what the contract says is wrong, and the golden run folder reads as its records say
(PROTOCOL.md, "Recorder")."""

from __future__ import annotations

import copy
import json
import struct

import jsonschema
import pytest

from hertzforge.service import frames

from .conftest import GOLDEN

#: text golden -> (path to its payload, the schema definition the payload follows)
TEXT = {
    'request_recorder_configure': ('params', 'recorderConfigureParams'),
    'response_recorder_configure': ('result', 'recorderConfigureResult'),
    'request_recorder_start': ('params', 'recorderEmptyParams'),
    'response_recorder_start': ('result', 'recorderState'),
    'request_recorder_arm': ('params', 'recorderEmptyParams'),
    'response_recorder_arm': ('result', 'recorderArmResult'),
    'request_recorder_record': ('params', 'recorderEmptyParams'),
    'response_recorder_record': ('result', 'recorderRecordResult'),
    'request_recorder_cut': ('params', 'recorderEmptyParams'),
    'response_recorder_cut': ('result', 'recorderCutResult'),
    'event_recorder_state': ('data', 'recorderState'),
    'request_recorder_subscribe': ('params', 'recorderSubscribeParams'),
    'response_recorder_subscribe': ('result', 'recorderSubscribeResult'),
    'response_recorder_recordings': ('result', 'recorderRecordingsResult'),
    'response_recorder_marks': ('result', 'recorderMarksResult'),
    'response_error_recorder_config_invalid': ('error.detail', 'report'),
}

#: binary golden -> (path to its payload, the schema definition the payload follows)
BINARY = {
    'event_recorder_live': ('data', 'recorderLive'),
    'event_recorder_data': ('data', 'recorderData'),
}

RUN = GOLDEN / 'recorder_run'


def test_every_recorder_golden_is_checked_here():
    # The Rust, TypeScript and Python suites read the same files in schema/golden/; the
    # run folder is read by the tests below.
    names = {p.name.split('.')[0] for p in GOLDEN.iterdir()
             if '_recorder_' in p.name and not p.name.endswith('.expected.json')}
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
def test_recorder_text_golden_validates(name, validate):
    message = _text(name)
    validate(message)
    path, definition = TEXT[name]
    validate(_at(message, path), definition)


@pytest.mark.parametrize('name', sorted(BINARY))
def test_recorder_binary_golden_validates_and_decodes_to_its_expected_json(name, validate):
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


def test_recorder_configuration_takes_no_other_key_and_a_level_trigger_its_three(validate):
    config = _text('request_recorder_configure')['params']['config']
    assert set(config) == {'title', 'acquisition', 'trigger', 'analysis', 'capture',
                           'metadata'}
    validate({}, 'recorderConfig')
    # The wire offers no data root and no drive.
    for key, value in (('data_root', '/tmp'), ('drive', {}), ('outputs', [])):
        bad = copy.deepcopy(config)
        bad[key] = value
        _refused(validate, bad, 'recorderConfig')
    for key, inner in (('capture', {'data_root': '/tmp'}), ('trigger', {'mode2': 'x'}),
                       ('analysis', {'nfft': 4096}), ('acquisition', {'block': 256})):
        bad = copy.deepcopy(config)
        bad[key] = {**bad[key], **inner}
        _refused(validate, bad, 'recorderConfig')
    for missing in ('row', 'level', 'record_s'):
        bad = copy.deepcopy(config)
        del bad['trigger'][missing]
        _refused(validate, bad, 'recorderConfig')
    bad = copy.deepcopy(config)
    bad['trigger']['mode'] = 'free_run'
    _refused(validate, bad, 'recorderConfig')


def test_recorder_derived_values_are_the_ni_stations_at_12800_hz():
    derived = _text('response_recorder_configure')['result']['derived']
    grid = derived['grid']
    assert (grid['fs_hz'], grid['block_frames'], grid['batch_frames']) == (12800, 256, 512)
    assert grid['group_delay_frames'] == round(38.4 + 3.2e-6 * 12800, 2) == 38.44
    assert derived['outputs'] == 'none' and derived['rows'] == [1, 2, 3, 4]
    a = derived['analysis']
    assert (a['df_hz'], a['lines'], a['span_hz'], a['block_s']) == (6.25, 800, 5000, 0.16)
    # The block plus seven hops of half a block, at 12 800 Hz.
    assert a['averages_s'] == (2048 + 7 * 1024) / 12800
    assert derived['trigger'] == {'pre_frames': 6400, 'record_frames': 12800,
                                  'holdoff_frames': 0}
    disk = derived['disk']
    assert disk['bytes_per_s'] == 4 * 4 * 12800
    assert disk['floor_bytes'] == max(1 << 30, 2 * (6400 + 12800) * 16)
    assert disk['seconds_above_floor'] == (disk['free_bytes'] - disk['floor_bytes']) // 204800


def test_recorder_state_counts_agree_with_each_other():
    s = _text('event_recorder_state')['data']
    assert s['t_s'] == s['station_frames'] / 12800
    r = s['recording']
    assert r['trigger_station_frame'] - r['first_station_frame'] == r['pre_frames']
    assert r['frames_fed'] == s['station_frames'] - r['first_station_frame']
    assert r['backlog_s'] == round((r['frames_fed'] - r['frames_on_disk']) / 12800, 3)
    assert r['seconds'] == round((s['station_frames'] - r['trigger_station_frame']) / 12800, 3)
    assert s['health']['batches'] == s['station_frames'] // 512
    t = s['trigger']
    assert t['left'] == 5 - t['saved'] - 1, 'one saved, one in progress, of five'


def test_recorder_marks_and_subscriptions_refuse_what_the_contract_refuses(validate):
    marks = _text('response_recorder_marks')['result']['marks']
    limit = next(m for m in marks if m['kind'] == 'limit')
    bad = dict(limit, axis='jerk')
    _refused(validate, bad, 'recorderMark')
    overload = next(m for m in marks if m['kind'] == 'overload')
    bad = dict(overload, axis='accel')
    _refused(validate, bad, 'recorderMark')
    for plot in ({'id': 'f', 'quantity': 'frf', 'channels': 'all'},
                 {'id': 's', 'quantity': 'spectrum', 'points': 64},
                 {'id': 't', 'quantity': 'time', 'seconds': 3},
                 {'id': 't', 'quantity': 'time', 'points': 32},
                 {'id': 'x', 'quantity': 'survey'}):
        _refused(validate, plot, 'recorderSubscribePlot')


def test_recorder_golden_run_folder_reads_as_its_records_say(validate):
    run = json.loads((RUN / 'run.json').read_text(encoding='utf-8'))
    validate(run, 'recorderRunRecord')
    end = json.loads((RUN / 'end.json').read_text(encoding='utf-8'))
    validate(end, 'recorderEndRecord')
    for line in (RUN / 'log.jsonl').read_text(encoding='utf-8').splitlines():
        validate(json.loads(line), 'logEntry')
    assert (RUN / 'marks.jsonl').read_text(encoding='utf-8') == ''

    rec = RUN / 'recordings' / 'rec_0001'
    side = json.loads((rec / 'recorder.json').read_text(encoding='utf-8'))
    validate(side, 'recorderSidecar')
    cap = json.loads((rec / 'capture.json').read_text(encoding='utf-8'))
    raw = (rec / 'telemetry.f32').read_bytes()
    rows = len(side['rows'])
    # The file's size is the authority on its length.
    assert len(raw) == 8192 == side['frames'] * rows * 4 == cap['frames_written'] * rows * 4
    assert side['fs_hz'] == cap['config']['sample_rate_hz'] == run['fs_hz']
    assert side['complete'] and cap['complete'] and side['fault'] is None
    values = struct.unpack(f'<{len(raw) // 4}f', raw)
    # Every file frame sits at its stated station frame.
    for k in range(side['frames']):
        n = side['first_station_frame'] + k
        assert values[k * rows] == 1.0 - abs((n % 64) - 32) / 16.0, k
        assert values[k * rows + 1] == (n % 1024) / 1024.0 - 0.5, k
    assert side['trigger_station_frame'] - side['first_station_frame'] == side['pre_frames']
