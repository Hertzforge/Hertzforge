"""What a selection in the tree shows (`project.show`), by Visual
Dynamics' table of readings: one object by its kind, and the pairs that
mean something together."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

from hertzforge.core.data import Frf, Psd, Specification, TimeHistory
from hertzforge.core.shapes import ShapeSet
from hertzforge.service import frames, projects, show
from hertzforge.service.protocol import ProtocolError

from .test_ops import wire_views
from .test_server import _session

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'


@pytest.fixture(scope='module')
def plate():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    projects.import_file(state, {'path': str(PLATE / 'modal_spectra.nc4')})
    projects.import_file(state, {'path': str(PLATE / 'geometry.unv'),
                                 'units': {'length_unit': 'mm'}})
    return state


def named(state, kind):
    return next(name for name, obj in state.project.items() if projects.kind_of(obj) == kind)


def shown(state, *objects, records=None, part=None, validate=None, prefs=None):
    out = projects.show(state, {'objects': list(objects), 'records': records or {}, 'part': part,
                                **({'prefs': prefs} if prefs else {})})
    if validate:
        # the views carry arrays until they go on the wire; check them as the shell gets them
        wire_views(out['views'], validate)
        validate({**out, 'views': []}, 'shown')
    return out


def test_one_object_shows_by_its_kind(plate, validate):
    geometry, frf, table = named(plate, 'geometry'), named(plate, 'frf'), named(plate, 'channel_table')
    assert [v['view'] for v in shown(plate, geometry, validate=validate)['views']] == ['shape3d']
    whole = shown(plate, frf, validate=validate)
    assert [v['view'] for v in whole['views']] == ['waterfall'] and whole['note'] == ''
    picked = shown(plate, frf, records={frf: [0, 1, 2, 5]}, validate=validate)
    assert [v['view'] for v in picked['views']] == ['spectrum']
    assert len(picked['views'][0]['series']) == 4, 'the records picked, as curves'
    assert shown(plate, table, validate=validate)['views'][0]['view'] == 'table'


def test_a_time_history_on_a_geometry_plays_back(plate, validate):
    geometry, history = named(plate, 'geometry'), named(plate, 'time_history')
    out = shown(plate, geometry, history, validate=validate)
    stage, plot = out['views']
    assert stage['view'] == 'shape3d' and stage['modes'] == [] and stage['link'] == 'time'
    assert stage['motion']['values'].shape[0] == len(plot['series'])
    assert stage['motion']['quantity'] == 'Acceleration' and stage['motion']['peak'] > 0
    assert 'another quantity' in out['note'], 'the force channels are not played'
    # the same pair the other way round, and with records picked
    assert shown(plate, history, geometry)['views'][0]['view'] == 'shape3d'
    one = shown(plate, geometry, history, records={history: [0]})
    assert len(one['views'][1]['series']) == 1


def test_an_frf_on_a_geometry_is_its_operating_shape_at_the_loudest_line(plate, validate):
    geometry, frf = named(plate, 'geometry'), named(plate, 'frf')
    out = shown(plate, frf, geometry, validate=validate)
    stage, curves = out['views']
    assert stage['view'] == 'shape3d' and len(stage['modes']) == 1
    mode = stage['modes'][0]
    assert mode['label'].startswith('ODS at') and 'shape_imag' in mode
    assert mode['frequency_hz'] > 0 and np.abs(mode['shape']).max() > 0
    assert curves['view'] == 'spectrum'


def test_a_shape_set_shows_on_its_geometry_or_as_its_table_and_mac(plate, validate):
    project = plate.project
    geometry = project[named(plate, 'geometry')]
    dofs = [f'{int(n)}Z+' for n in geometry.node_id[:6]]
    shapes = ShapeSet(frequency=[10.0, 20.0], damping=[0.01, 0.01],
                      shape_matrix=np.ones((2, 6)), coordinate=dofs)
    project.add('Modes', shapes)
    try:
        out = shown(plate, 'Modes', validate=validate)
        assert [v['view'] for v in out['views']] == ['shape3d', 'table'], 'on the active geometry'
        both = shown(plate, 'Modes', named(plate, 'geometry'), records={'Modes': [1]})
        assert [v['view'] for v in both['views']] == ['shape3d', 'table']
        project.add('Modes 2', shapes)
        mac = shown(plate, 'Modes', 'Modes 2', validate=validate)
        assert mac['views'][0]['view'] == 'matrix'
    finally:
        project.remove('Modes 2')
        project.remove('Modes')


def test_a_shape_set_with_no_geometry_shows_its_table_and_its_own_mac(validate):
    from hertzforge.project import Project

    project = Project()
    project.add('Modes', ShapeSet(frequency=[10.0, 20.0], damping=[0.01, 0.01],
                                  shape_matrix=np.eye(2), coordinate=['1Z+', '2Z+']))
    state = projects.OpenProject(project=project)
    out = shown(state, 'Modes', validate=validate)
    assert [v['view'] for v in out['views']] == ['table', 'matrix']


def test_a_measurement_beside_its_requirement_is_the_comparison(validate):
    from hertzforge.project import Project

    project = Project()
    f = np.linspace(20, 2000, 100)
    psd = Psd(abscissa=f, ordinate=np.full((2, 100), 0.01), response_dof=['1Z+', '2Z+'],
              ordinate_unit=['g**2/Hz'] * 2)
    spec = Specification(abscissa=f, ordinate=np.full((2, 100), 0.01),
                         response_dof=['1Z+', '2Z+'], ordinate_unit=['g**2/Hz'] * 2)
    project.add('Measured', psd)
    project.add('Spec', spec)
    project.add('Other', Psd(abscissa=f, ordinate=np.full((2, 100), 0.02),
                             response_dof=['1Z+', '2Z+'], ordinate_unit=['g**2/Hz'] * 2))
    state = projects.OpenProject(project=project)
    out = shown(state, 'Spec', 'Measured', validate=validate)
    assert out['views'][0]['view'] == 'spectrum' and 'Spec' in out['title']
    two = shown(state, 'Measured', 'Other', validate=validate)
    assert [v['view'] for v in two['views']] == ['spectrum'], 'two PSDs overlaid'
    alone = shown(state, 'Spec', validate=validate)
    assert alone['views'][0]['view'] == 'waterfall'


def test_a_pair_with_no_joint_reading_shows_each_on_its_own(plate, validate):
    frf, table = named(plate, 'frf'), named(plate, 'channel_table')
    out = shown(plate, frf, table, validate=validate)
    assert [v['view'] for v in out['views']] == ['waterfall', 'table']
    assert 'each object on its own' in out['note']
    three = shown(plate, frf, table, named(plate, 'geometry'), validate=validate)
    assert len(three['views']) == 3


def test_a_part_of_a_geometry_is_noted(plate, validate):
    out = shown(plate, named(plate, 'geometry'), part={'kind': 'family', 'id': 'quads'},
                validate=validate)
    assert out['views'][0]['view'] == 'shape3d' and 'family quads' in out['note']


def test_what_cannot_be_shown_is_refused_or_said(plate):
    with pytest.raises(ProtocolError, match='no object named'):
        shown(plate, 'nope')
    with pytest.raises(ProtocolError, match='needs "objects"'):
        projects.show(plate, {'objects': []})
    with pytest.raises(ProtocolError, match='"records"'):
        projects.show(plate, {'objects': [named(plate, 'frf')], 'records': {'x': [-1]}})
    with pytest.raises(ProtocolError, match='none is open'):
        projects.show(None, {'objects': ['x']})


def test_record_runs_read_as_the_shell_numbers_them():
    assert show._runs([0, 1, 2, 6]) == '1-3, 7'
    assert show._runs([4]) == '5'
    assert show._runs([2, 0, 1]) == '1-3'
    assert show._runs([0, 2, 3, 4, 7, 8]) == '1, 3-5, 8-9'


def test_time_data_on_a_geometry_that_shares_no_node_says_so(validate):

    state, (geometry,) = projects.import_file(None, {'path': str(PLATE / 'geometry.unv'),
                                                    'units': {'length_unit': 'mm'}})
    project = state.project
    project.add('Stray', TimeHistory(abscissa=np.linspace(0, 1, 16), ordinate=np.zeros((1, 16)),
                                     response_dof=['999999Z+'], ordinate_unit=['m/s**2']))
    out = shown(state, geometry, 'Stray', validate=validate)
    assert out['views'] == [] or out['views'][0]['view'] == 'shape3d'
    assert 'lands on' in out['note'] or 'each object on its own' in out['note']
    frf = Frf(abscissa=np.linspace(1, 100, 8), ordinate=np.ones((1, 8), dtype=complex) * 1j,
              response_dof=['999999Z+'], reference_dof=['1Z+'], ordinate_unit=['m/s**2/N'])
    project.add('Stray FRF', frf)
    out = shown(state, geometry, 'Stray FRF')
    assert 'lands on' in out['note'] or 'each object on its own' in out['note']


def test_the_server_sends_the_selections_views_with_their_arrays():
    import asyncio

    async def check(client, socket):
        await socket.recv()        # hello
        await socket.send(frames.encode_text({'type': 'request', 'id': 7, 'method': 'project.import',
                                              'params': {'path': str(PLATE / 'geometry.unv'),
                                                         'units': {'length_unit': 'mm'}}}))
        while True:
            imported = frames.decode(await socket.recv())
            if imported.get('id') == 7:
                break
        (geometry,) = [o['name'] for o in imported['result']['project']['objects']]
        answer = await client.call('project.show', {'objects': [geometry]})
        (view,) = answer['result']['views']
        assert view['view'] == 'shape3d' and len(view['nodes']) > 0, 'the mesh came through'
        assert answer['result']['title'] == geometry

    asyncio.run(_session(check))


def test_two_geometries_are_one_scene_and_a_report_says_where_it_opens(plate, validate):
    project = plate.project
    first = named(plate, 'geometry')
    projects.import_file(plate, {'path': str(PLATE / 'geometry.unv'),
                                 'units': {'length_unit': 'mm'}})
    second = next(n for n, o in project.items() if projects.kind_of(o) == 'geometry' and n != first)
    try:
        out = shown(plate, first, second, validate=validate)
        (scene,) = out['views']
        assert scene['view'] == 'shape3d' and scene['title'] == f'{second} over {first}'
        assert len(scene['nodes']) == len(project[first].node_id) + len(project[second].node_id)
        assert out['note'] == ''
    finally:
        project.remove(second)
    from hertzforge.core.report import Report

    project.add('Report', Report(title='Plate survey'))
    try:
        out = shown(plate, 'Report', validate=validate)
        assert out['views'][0]['view'] == 'reading'
        assert any(r['value'] == 'the Report workspace' for r in out['views'][0]['rows'])
    finally:
        project.remove('Report')


def test_the_bar_reads_data_flat_in_decades_and_by_component(plate, validate):
    frf, history = named(plate, 'frf'), named(plate, 'time_history')
    stage = shown(plate, frf, validate=validate)
    assert stage['views'][0]['view'] == 'waterfall'
    assert stage['offers'] == {'flat': True, 'complex': True, 'frequency': True}
    flat = shown(plate, frf, prefs={'flat': True}, validate=validate)
    (plot,) = flat['views']
    assert plot['view'] == 'spectrum' and len(plot['series']) == plate.project[frf].num_records
    assert plot['x']['scale'] != 'log'
    decades = shown(plate, frf, prefs={'flat': True, 'log_frequency': True})
    assert decades['views'][0]['x']['scale'] == 'log'
    phase = shown(plate, frf, prefs={'flat': True, 'component': 'phase'})
    assert 'phase' in phase['views'][0]['y']['label'].lower() or phase['views'][0]['y']['unit'] in ('°', 'deg', 'rad')
    time = shown(plate, history, validate=validate)
    assert time['offers'] == {'flat': True, 'complex': False, 'frequency': False}
    assert shown(plate, named(plate, 'geometry'))['offers'] == {'flat': False, 'complex': False,
                                                                  'frequency': False}
    with pytest.raises(ProtocolError, match='"prefs"'):
        projects.show(plate, {'objects': [frf], 'prefs': {'stage': True}})
    with pytest.raises(ProtocolError, match='component'):
        projects.show(plate, {'objects': [frf], 'prefs': {'component': 'abs'}})


def test_a_geometrys_views_carry_the_drawn_nodes_ids(plate, validate):
    geometry, frf, history = named(plate, 'geometry'), named(plate, 'frf'), named(plate, 'time_history')
    project = plate.project
    for objects in ((geometry,), (geometry, frf), (geometry, history)):
        (stage, *_rest) = shown(plate, *objects, validate=validate)['views']
        ids = stage['node_ids']
        assert ids.dtype == np.uint32 and len(ids) == len(stage['nodes'])
        assert set(ids.tolist()) <= {int(n) for n in project[geometry].node_id}
