"""The catalog and the descriptors against the schema, and how a
descriptor checks the params a request sends."""

from __future__ import annotations

import pytest

from hertzforge.service.protocol import ProtocolError
from hertzforge.service.server import OPERATIONS, Service


@pytest.fixture(scope='module')
def catalog():
    service = Service()
    try:
        yield service.catalog_list()
    finally:
        service.close()


def test_catalog_validates(catalog, validate):
    validate(catalog, 'catalog')
    for op in catalog['operations']:
        validate(op, 'operation')


def test_catalog_names_every_operation_and_dataset(catalog):
    ids = {op['id'] for op in catalog['operations']}
    assert ids == {'random.psd_vs_spec', 'random.error_db', 'modal.modes'}
    kinds = {d['kind'] for d in catalog['datasets']}
    assert kinds == {'random_run', 'modal_model'}
    assert 'plate-random' in {d['id'] for d in catalog['datasets']}


def test_channel_param_comes_from_the_dataset(catalog):
    for op in catalog['operations']:
        if op['accepts'] != 'random_run':
            continue
        channel = next(p for p in op['params'] if p['name'] == 'channel')
        assert channel['type'] == 'enum'
        assert channel['options_from'] == 'channels'
        assert 'options' not in channel


def test_workspaces_and_outputs(catalog):
    by_id = {op['id']: op for op in catalog['operations']}
    assert by_id['random.psd_vs_spec']['workspace'] == 'analyze'
    assert by_id['random.psd_vs_spec']['outputs'] == ['spectrum', 'reading']
    assert by_id['random.error_db']['outputs'] == ['spectrum']
    assert by_id['modal.modes']['workspace'] == 'modal'
    assert by_id['modal.modes']['outputs'] == ['shape3d']


OPS = {op.id: op for op in OPERATIONS}


def test_resolve_fills_defaults_and_the_first_channel():
    values = OPS['random.psd_vs_spec'].resolve({}, ['a', 'b'])
    assert values == {'channel': 'a', 'tolerance_db': 3.0}
    assert OPS['modal.modes'].resolve({}, []) == {'count': 6, 'rigid': False}


@pytest.mark.parametrize('op, params, words', [
    ('random.psd_vs_spec', {'channel': 'z'}, "'z', which is not one of"),
    ('random.psd_vs_spec', {'tolerance_db': 13}, 'the most it takes is 12'),
    ('random.psd_vs_spec', {'tolerance_db': -1}, 'the least it takes is 0'),
    ('random.psd_vs_spec', {'tolerance_db': '3'}, "must be a number, not the string '3'"),
    ('random.psd_vs_spec', {'tolerance_db': True}, 'must be a number'),
    ('random.psd_vs_spec', {'level': 1}, "no param named 'level'"),
    ('modal.modes', {'count': 2.5}, 'whole number'),
    ('modal.modes', {'count': 0}, 'the least it takes is 1'),
    ('modal.modes', {'rigid': 1}, 'true or false'),
])
def test_resolve_refuses_with_the_param_named(op, params, words):
    with pytest.raises(ProtocolError) as caught:
        OPS[op].resolve(params, ['a', 'b'])
    assert caught.value.code == 'bad_request'
    assert words in caught.value.message


def test_resolve_refuses_params_that_are_not_an_object():
    with pytest.raises(ProtocolError, match='must be an object'):
        OPS['modal.modes'].resolve([1], [])
