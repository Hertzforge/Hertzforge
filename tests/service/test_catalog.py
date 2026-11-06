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


def test_catalog_names_every_operation(catalog):
    ids = {op['id'] for op in catalog['operations']}
    assert ids == {op.id for op in OPERATIONS}
    assert set(catalog) == {'operations'}
    # a few by name; each is tested where it is defined
    assert {'project.plot', 'project.waterfall', 'project.fit_modes', 'project.mac',
            'project.mode_shapes', 'project.compute_psds'} <= ids
    assert all(op['id'].startswith('project.') for op in catalog['operations'])


def test_an_object_param_lists_no_options_of_its_own(catalog):
    for op in catalog['operations']:
        for param in op['params']:
            if 'options_from' in param:
                assert param['type'] == 'enum'
                assert 'options' not in param


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
    offered = {op['id']: op['modules'] for op in catalog['operations']}
    assert all(module in offered['project.save_as'] for module in ('signals', 'shock', 'model'))
    assert offered['project.set_averaging'] == ['signals', 'random', 'modal']
    assert offered['project.shocks'] == ['shock']


def test_workspaces_and_outputs(catalog):
    by_id = {op['id']: op for op in catalog['operations']}
    assert by_id['project.fit_modes']['workspace'] == 'modal'
    assert by_id['project.fit_modes']['outputs'] == ['shape3d', 'reading']
    assert by_id['project.fit_modes']['modules'] == ['modal']
    assert by_id['project.waterfall']['workspace'] == 'signals'
    assert by_id['project.waterfall']['outputs'] == ['waterfall']
    assert 'shock' in by_id['project.waterfall']['modules']
    assert by_id['project.mode_shapes']['outputs'] == ['shape3d']


OPS = {op.id: op for op in OPERATIONS}


OBJECTS = {'Noise': 'time_history', 'PSDs': 'psd', 'Shock': 'srs',
           'Required': 'shock_specification'}
VERBS = {'Noise': ['compute_srs', 'compute_psds'], 'PSDs': []}


def test_resolve_fills_defaults_and_the_first_object():
    values = OPS['project.waterfall'].resolve({}, objects=OBJECTS)
    assert values == {'data': 'Noise', 'component': 'magnitude', 'page': 1, 'quantity': ''}
    values = OPS['project.compute_srs'].resolve({}, objects=OBJECTS, verbs=VERBS)
    assert values['source'] == 'Noise'
    assert set(values) == {'source', 'per_octave', 'q', 'kind'}


@pytest.mark.parametrize('op, params, words', [
    ('project.compute_srs', {'kind': 'z'}, "'z', which is not one of"),
    ('project.compute_srs', {'per_octave': 49}, 'the most it takes is 48'),
    ('project.compute_srs', {'per_octave': 0}, 'the least it takes is 1'),
    ('project.compute_srs', {'per_octave': 2.5}, 'whole number'),
    ('project.compute_srs', {'q': '3'}, "must be a number, not the string '3'"),
    ('project.compute_srs', {'q': True}, 'must be a number'),
    ('project.compute_srs', {'q': 0.1}, 'the least it takes is 0.5'),
    ('project.compute_srs', {'level': 1}, "no param named 'level'"),
    ('project.waterfall', {'data': 'Nope'}, "'Nope', which is not one of"),
    ('project.srs_points', {'limits': 1}, 'true or false'),
])
def test_resolve_refuses_with_the_param_named(op, params, words):
    with pytest.raises(ProtocolError) as caught:
        OPS[op].resolve(params, objects=OBJECTS, verbs=VERBS)
    assert caught.value.code == 'bad_request'
    assert words in caught.value.message


def test_resolve_refuses_an_object_the_project_lacks():
    with pytest.raises(ProtocolError) as caught:
        OPS['project.mac'].resolve({}, objects=OBJECTS, verbs=VERBS)
    assert caught.value.code == 'refused'
    assert 'needs a shape set' in caught.value.message


def test_resolve_refuses_params_that_are_not_an_object():
    with pytest.raises(ProtocolError, match='must be an object'):
        OPS['project.waterfall'].resolve([1], objects=OBJECTS)
