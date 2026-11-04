"""The MIMO Random and control-law messages against the schema: every golden
validates as a message and as its own payload, and the schema refuses what
the contract says is wrong (PROTOCOL.md, "MIMO Random" and "Control Laws")."""

from __future__ import annotations

import copy
import json

import jsonschema
import pytest

from .conftest import GOLDEN

#: golden -> (path to its payload, the schema definition the payload follows)
PAYLOADS = {
    'request_mimo_random_setup': ('params', 'mimoSetupParams'),
    'response_mimo_random_setup': ('result', 'mimoSetupResult'),
    'response_error_setup_invalid': ('error.detail', 'mimoValidation'),
    'event_mimo_random_status': ('data', 'mimoStatus'),
    'request_mimo_random_override': ('params', 'mimoOverrideParams'),
    'response_mimo_random_report': ('result', 'mimoReportResult'),
    'request_laws_list': ('params', 'lawsListParams'),
    'response_laws_list': ('result', 'lawsListResult'),
    'request_laws_import': ('params', 'lawsImportParams'),
    'response_laws_import': ('result', 'lawsImportResult'),
}


def _text(name):
    return json.loads((GOLDEN / f'{name}.json').read_text(encoding='utf-8'))


def _at(message, path):
    for key in path.split('.'):
        message = message[key]
    return message


def _refused(validate, instance, name):
    with pytest.raises(jsonschema.ValidationError):
        validate(instance, name)


@pytest.mark.parametrize('name', sorted(PAYLOADS))
def test_mimo_golden_validates(name, validate):
    message = _text(name)
    validate(message)
    path, definition = PAYLOADS[name]
    validate(_at(message, path), definition)


def test_the_packed_reference_holds_the_upper_triangle_of_each_line():
    ref = _text('request_mimo_random_setup')['params']['setup']['reference']
    n, lines = ref['n'], len(ref['freqs_hz'])
    assert len(ref['re']) == len(ref['im']) == lines * n * (n + 1) // 2
    # the diagonal of every line is real; G01 carries the cross spectrum's phase
    assert all(ref['im'][k * 3] == 0 and ref['im'][k * 3 + 2] == 0 for k in range(lines))
    g01 = complex(ref['re'][1], ref['im'][1])
    coherence = abs(g01) ** 2 / (ref['re'][0] * ref['re'][2])
    assert coherence == pytest.approx(0.25)


def test_a_setup_carries_its_channels_reference_and_schedule(validate):
    setup = _text('request_mimo_random_setup')['params']['setup']
    for field in ('channels', 'reference', 'schedule'):
        _refused(validate, {k: v for k, v in setup.items() if k != field}, 'mimoSetup')
    drives = copy.deepcopy(setup)
    del drives['channels']['drives'][0]['ceiling_vrms']
    _refused(validate, drives, 'mimoSetup')
    file_ref = {**setup, 'reference': {'kind': 'file', 'path': '/data/ref.hforge',
                                       'sha256': 'ab' * 32}}
    validate(file_ref, 'mimoSetup')
    _refused(validate, {**file_ref, 'reference': {**file_ref['reference'], 'sha256': 'AB' * 32}},
             'mimoSetup')
    virtual = {**setup, 'space': {'kind': 'virtual', 'dofs': ['X', 'Y'],
                                  'response_transform': {'rows': 2, 'cols': 2,
                                                         'values': [1, 0, 0, 1]}}}
    validate(virtual, 'mimoSetup')
    _refused(validate, {**setup, 'space': {'kind': 'virtual'}}, 'mimoSetup')


def test_a_validation_is_ok_exactly_when_it_has_no_errors(validate):
    good = _text('response_mimo_random_setup')['result']['validation']
    bad = _text('response_error_setup_invalid')['error']['detail']
    assert good['ok'] and not bad['ok']
    _refused(validate, {**good, 'ok': False}, 'mimoValidation')
    _refused(validate, {**bad, 'ok': True}, 'mimoValidation')


def test_the_status_ends_exactly_when_finished(validate):
    status = _text('event_mimo_random_status')['data']
    _refused(validate, {**status, 'state': 'finished'}, 'mimoStatus')
    _refused(validate, {**status, 'phase': 'coherence_gate'}, 'mimoStatus')
    report = _text('response_mimo_random_report')['result']['report']
    validate({**status, 'state': 'finished', 'phase': 'stopped', 'end': report['end']},
             'mimoStatus')


def test_an_override_names_the_operators_words(validate):
    _refused(validate, {'ack': ''}, 'mimoOverrideParams')
    _refused(validate, {'ack': '   '}, 'mimoOverrideParams')
    _refused(validate, {}, 'mimoOverrideParams')


def test_law_entries_say_which_world_they_are_for(validate):
    entry = _text('response_laws_list')['result']['laws'][0]
    assert entry['world'] == 'mimo' and entry['builtin'] and entry['check']['hardware']
    _refused(validate, {**entry, 'shapes': None}, 'lawEntry')
    single = {**entry, 'world': 'single', 'shapes': None, 'needs': None}
    validate(single, 'lawEntry')
    imported = _text('response_laws_import')['result']['law']
    assert imported['sha256'] == imported['id'] and not imported['check']['hardware']
    _refused(validate, {**imported['params'][0], 'kind': 'slider'}, 'lawParam')


def test_every_new_refusal_is_a_listed_reason(validate):
    error = _text('response_error_setup_invalid')
    validate(error)
    for reason in ('gate_failed', 'law_import_failed', 'catalog_unavailable'):
        e = copy.deepcopy(error)
        e['error']['reason'] = reason
        validate(e)
    e = copy.deepcopy(error)
    e['error']['reason'] = 'setup_rejected'
    _refused(validate, e, None)
