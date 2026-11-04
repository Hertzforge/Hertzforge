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
    assert ids == {op.id for op in OPERATIONS}
    # the tools on the demonstration datasets, by name; the project's own are
    # many and each is tested where it is defined
    assert {'random.psd_vs_spec', 'random.error_db', 'random.waterfall',
            'modal.modes', 'modal_run.waterfall', 'modal_run.modes',
            'modal_run.ods', 'modal_run.mac', 'modal_run.motion',
            'modal_run.resynthesis'} <= ids
    assert all(op.accepts == 'project' for op in OPERATIONS if op.id.startswith('project.'))
    kinds = {d['kind'] for d in catalog['datasets']}
    assert kinds == {'random_run', 'modal_model', 'modal_run'}
    assert 'plate-random' in {d['id'] for d in catalog['datasets']}


def test_channel_param_comes_from_the_dataset(catalog):
    taking = [op['id'] for op in catalog['operations']
              if any(p['name'] == 'channel' for p in op['params'])]
    assert taking == ['random.psd_vs_spec', 'random.error_db']
    for op in catalog['operations']:
        channel = next((p for p in op['params'] if p['name'] == 'channel'), None)
        if channel is None:
            continue
        assert channel['type'] == 'enum'
        assert channel['options_from'] == 'channels'
        assert 'options' not in channel


def test_every_module_has_its_tools_and_the_shared_ones_say_so(catalog):
    """The analysis is split into modules by what is studied, one rail entry
    each, and an act every study needs is offered in every module."""
    from hertzforge.service.descriptors import Workspace

    modules = set(Workspace.__args__)
    homes = {op['id']: op['workspace'] for op in catalog['operations']}
    assert set(homes.values()) <= modules
    for module in ('signals', 'random', 'shock', 'sine', 'modal', 'geometry', 'model',
                   'specify', 'report'):
        assert module in homes.values(), module
    assert homes['project.shocks'] == 'shock' and homes['project.dwell'] == 'sine'
    assert homes['project.add_solid_block'] == 'geometry' and homes['project.mesh_solids'] == 'model'
    shared = {op['id'] for op in catalog['operations'] if op.get('everywhere')}
    assert {'project.plot', 'project.table', 'project.save_as', 'project.set_averaging',
            'project.set_channel_cell'} <= shared
    assert 'project.shocks' not in shared


def test_workspaces_and_outputs(catalog):
    by_id = {op['id']: op for op in catalog['operations']}
    assert by_id['random.psd_vs_spec']['workspace'] == 'random'
    assert by_id['random.psd_vs_spec']['outputs'] == ['spectrum', 'reading']
    assert 'everywhere' not in by_id['random.psd_vs_spec']
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
