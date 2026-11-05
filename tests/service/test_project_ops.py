"""Operations on a connection's own project: params that list the project's
objects by kind, and the core's verbs run on what the person opened."""

from __future__ import annotations

import asyncio
from pathlib import Path

import numpy as np
import pytest

from hertzforge.service import frames, projects
from hertzforge.service.descriptors import Param
from hertzforge.service.ops_project import OPERATIONS
from hertzforge.service.protocol import ProtocolError
from hertzforge.service.server import Service, start

from .conftest import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in OPERATIONS}


@pytest.fixture
def plate():
    """A project made from the plate's modal run and its geometry, as a
    person would import them."""
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    projects.import_file(state, {'path': str(PLATE / 'geometry.unv'),
                                 'units': {'length_unit': 'mm'}})
    return state.project


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name)
                               for name in project})
    return op.run(project, values)


def test_an_object_param_lists_the_projects_objects_of_its_kinds():
    param = Param('data', 'Data', 'enum', options_from='objects:psd|frf')
    assert param.kinds == ('psd', 'frf')
    assert param.to_json()['options_from'] == 'objects:psd|frf'
    op = OPS['project.waterfall']
    objects = {'Channel Table': 'channel_table', 'Run': 'time_history', 'FRF': 'frf'}
    assert op.resolve({}, objects=objects)['data'] == 'Run'
    assert op.resolve({'data': 'FRF'}, objects=objects)['data'] == 'FRF'
    with pytest.raises(ProtocolError, match='not one of the choices') as error:
        op.resolve({'data': 'Channel Table'}, objects=objects)
    assert error.value.code == 'bad_request'
    with pytest.raises(ProtocolError, match='Mode Shapes needs a shape set') as error:
        OPS['project.mode_shapes'].resolve({}, objects=objects)
    assert error.value.code == 'refused'


@pytest.mark.parametrize('source', ['objects:', 'objects:psd|', 'items:psd'])
def test_an_options_source_is_channels_or_objects_of_named_kinds(source):
    with pytest.raises(ValueError, match='options_from'):
        Param('x', 'X', 'enum', options_from=source)


def test_the_descriptors_validate(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')


def test_a_waterfall_draws_any_data_object(plate, validate):
    (view,) = run(plate, 'project.waterfall')
    wire_views([view], validate)
    assert view['view'] == 'waterfall' and view['title'] == 'Time History'
    assert len(view['records']) == 11


def test_computed_frfs_and_psds_join_the_project(plate, validate):
    (frfs,) = run(plate, 'project.compute_frfs', method='H1')
    assert frfs['title'] == 'Time History H1 FRFs' and 'Time History H1 FRFs' in plate
    assert projects.kind_of(plate['Time History H1 FRFs']) == 'frf'
    (psds,) = run(plate, 'project.compute_psds')
    assert projects.kind_of(plate[psds['title']]) == 'psd'
    wire_views([frfs, psds], validate)


def test_fitted_modes_are_drawn_on_the_projects_geometry(plate, validate):
    run(plate, 'project.compute_frfs')
    (view,) = run(plate, 'project.fit_modes', low_hz=300.0, high_hz=1300.0, count=3)
    wire_views([view], validate)
    fitted = plate['Time History Hv FRFs Modes']
    assert projects.kind_of(fitted) == 'shape_set' and fitted.num_shapes == 3
    assert view['view'] == 'shape3d'
    assert [m['frequency_hz'] for m in view['modes']] == pytest.approx(list(fitted.frequency))
    # the measured DOFs land on the geometry: each mode moves some node
    for mode in view['modes']:
        assert np.abs(mode['shape']).max() > 0


def test_fitted_modes_are_listed_without_a_geometry(validate):
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    run(state.project, 'project.compute_frfs')
    (view,) = run(state.project, 'project.fit_modes', low_hz=300.0, high_hz=1300.0,
                  count=2)
    wire_views([view], validate)
    assert view['view'] == 'reading' and len(view['rows']) == 2


def test_an_empty_band_is_refused(plate):
    run(plate, 'project.compute_frfs')
    with pytest.raises(ProtocolError, match='band 500 to 400 Hz is empty'):
        run(plate, 'project.fit_modes', low_hz=500.0, high_hz=400.0)


def test_mode_shapes_draw_a_shape_set_on_a_geometry(plate, validate):
    run(plate, 'project.compute_frfs')
    run(plate, 'project.fit_modes', low_hz=300.0, high_hz=1300.0, count=2)
    (view,) = run(plate, 'project.mode_shapes')
    wire_views([view], validate)
    assert view['title'] == 'Time History Hv FRFs Modes on Geometry'
    assert len(view['modes']) == 2


# ---- over the socket ------------------------------------------------------------

async def _session(requests):
    service = Service()
    server = await start(service, port=0)
    port = server.sockets[0].getsockname()[1]
    responses = []
    try:
        from websockets.asyncio.client import connect

        async with connect(f'ws://127.0.0.1:{port}', max_size=None) as socket:
            frames.decode(await socket.recv())
            for ident, (method, params) in enumerate(requests, 1):
                await socket.send(frames.encode_text(
                    {'type': 'request', 'id': ident, 'method': method, 'params': params}))
                while True:
                    message = frames.decode(await socket.recv())
                    if message['type'] == 'response':
                        responses.append(message)
                        break
    finally:
        server.close()
        await server.wait_closed()
        service.close()
    return responses


def test_an_operation_on_the_project_returns_its_summary(tmp_path, validate):
    before, imported, saved, computed, drawn, with_handle = asyncio.run(_session([
        ('op.run', {'op': 'project.waterfall', 'params': {}}),
        ('project.import', {'path': str(PLATE / 'modal.nc4')}),
        ('project.save', {'path': str(tmp_path / 'run.hforge')}),
        ('op.run', {'op': 'project.compute_psds', 'params': {}}),
        ('op.run', {'op': 'project.waterfall', 'params': {'data': 'Time History PSDs'}}),
        ('op.run', {'op': 'project.waterfall', 'handle': 'h1', 'params': {}}),
    ]))
    assert before['error']['code'] == 'refused'
    assert 'none is open' in before['error']['message']
    assert imported['result']['project']['modified']
    result = computed['result']
    # saved, then added to: modified again; drawing adds nothing
    assert not saved['result']['project']['modified']
    assert result['project']['modified']
    names = [o['name'] for o in result['project']['objects']]
    assert names[-1] == 'Time History PSDs' and result['views'][0]['view'] == 'waterfall'
    assert drawn['result']['views'][0]['title'] == 'Time History PSDs'
    assert with_handle['error']['code'] == 'bad_request'
    for message in (computed, drawn):
        validate(wire_message := frames.to_wire(message)[0])
        validate(wire_message['result']['project'], 'project')


def test_a_computed_object_offers_no_quantity_to_rerun_with():
    """Choosing a quantity in the view reruns the operation; for a verb that
    adds an object that would add a second one, so the view offers none."""
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    history = state.project['Time History']
    assert len({history.known_dim(i) for i in range(history.num_records)}) > 1
    (view,) = run(state.project, 'project.compute_psds')
    assert 'quantities' not in view and 'quantity' not in view


def test_a_verb_param_lists_the_objects_the_core_offers_the_verb_on():
    param = Param('source', 'Source', 'enum', options_from='verb:integrate')
    assert param.verb == 'integrate' and param.kinds == ()
    op = OPS['project.integrate']
    verbs = {'Forces': ['filter_data'], 'Accels': ['integrate', 'filter_data']}
    assert op.resolve({}, verbs=verbs)['source'] == 'Accels'
    with pytest.raises(ProtocolError, match='not one of the choices'):
        op.resolve({'source': 'Forces'}, verbs=verbs)
    with pytest.raises(ProtocolError, match='Integrate applies to none') as error:
        op.resolve({}, verbs={'Forces': ['filter_data']})
    assert error.value.code == 'refused'


def test_the_summary_lists_each_objects_verbs_from_the_cores_table(plate):
    state = projects.OpenProject(plate)
    listed = {o['name']: o['verbs'] for o in projects.summary(state)['objects']}
    # the plate's channel table is linked with its record, so it can be read
    # against the table's ranges as well
    assert listed['Time History'] == [verb for verb, _ in plate.verbs('Time History')] + [
        'overload_check']
    assert 'compute_frfs' in listed['Time History']
    assert 'compute_frfs' not in listed['Channel Table']


@pytest.mark.parametrize(('op_id', 'given', 'kind'), [
    ('project.integrate', {}, 'time_history'),
    ('project.filter_data', {}, 'time_history'),
    ('project.resample', {'rate': 1024.0}, 'time_history'),
    ('project.compute_spectra', {}, 'spectrum'),
    ('project.compute_cpsds', {}, 'psd'),
    ('project.compute_multiple_coherence', {}, 'multiple_coherence'),
    ('project.compute_srs', {'per_octave': 6, 'q': 20.0}, 'srs'),
])
def test_a_verb_on_the_time_history_adds_its_result(plate, validate, op_id, given, kind):
    before = set(plate)
    (view,) = run(plate, op_id, **given)
    wire_views([view], validate)
    (added,) = set(plate) - before
    assert view['title'] == added and projects.kind_of(plate[added]) == kind


def test_the_verbs_chain_as_the_cores_table_allows(plate):
    run(plate, 'project.integrate', drift_corner=0.0)
    velocity = next(name for name in plate if name.startswith('Time History') and name != 'Time History')
    assert plate.provenance[velocity]['params'].get('drift_corner') is None
    assert 'differentiate' in projects.verbs_of(plate, velocity)
    run(plate, 'project.compute_psds')
    psd = next(name for name in plate if projects.kind_of(plate[name]) == 'psd')
    run(plate, 'project.compute_octave', source=psd, per_octave=3)
    run(plate, 'project.convert_to_specification', source=psd)
    spec = next(name for name in plate if projects.kind_of(plate[name]) == 'specification')
    run(plate, 'project.convert_to_psd', source=spec)


def test_a_verb_the_core_refuses_is_refused_in_its_words(plate):
    with pytest.raises(ProtocolError, match='no span set') as error:
        run(plate, 'project.truncate_data')
    assert error.value.code == 'refused'


def test_detecting_shocks_marks_the_history_and_counts_as_an_edit(plate, validate):
    (reading,) = run(plate, 'project.detect_shocks')
    wire_views([reading], validate)
    assert reading['rows'][0]['value'] == len(plate['Time History'].shocks) > 0
    assert OPS['project.detect_shocks'].edits


def test_rigid_body_modes_are_drawn_on_their_geometry(plate, validate):
    (view,) = run(plate, 'project.generate_rigid_body_modes')
    wire_views([view], validate)
    assert view['view'] == 'shape3d' and len(view['modes']) == 6
    assert 'Geometry Rigid Body Modes' in plate


def test_a_transform_the_shapes_cannot_resolve_is_refused_in_the_cores_words(plate):
    run(plate, 'project.generate_rigid_body_modes')
    with pytest.raises(ProtocolError, match='resolve only') as error:
        run(plate, 'project.transform', shapes='Geometry Rigid Body Modes')
    assert error.value.code == 'refused'


def test_solve_modes_draws_a_models_modes_on_it(validate):
    from test_fem import square_plate

    import hertzforge as hz
    from hertzforge.core.fem import GroupProperties, Material

    geometry = square_plate(4).geometry()
    geometry.group_properties = {int(geometry.group_id[0]): GroupProperties(
        Material('aluminum', youngs_modulus=70e9, density=2700, poissons_ratio=0.3), 0.01)}
    project = hz.Project('model')
    project.add('Skin', geometry)
    project.add('Bare', square_plate(2).geometry())
    assert projects.verbs_of(project, 'Bare').count('solve_modes') == 0
    (view,) = run(project, 'project.solve_modes', num_modes=10)
    wire_views([view], validate)
    assert view['view'] == 'shape3d' and len(view['modes']) == 10
    assert view['modes'][6]['frequency_hz'] > 0, 'six rigid-body modes, then elastic'


def test_a_set_is_projected_onto_another_sets_dofs(plate):
    run(plate, 'project.compute_frfs', method='H1')
    plate.fit_modes('Time History H1 FRFs', bounds=(100.0, 1300.0), limit=3)
    run(plate, 'project.generate_rigid_body_modes')
    fitted = next(n for n in plate if projects.kind_of(plate[n]) == 'shape_set'
                  and 'Rigid' not in n)
    (view,) = run(plate, 'project.project_onto_basis', source='Geometry Rigid Body Modes',
                  onto=fitted)
    added = [n for n in plate if n.startswith('Geometry Rigid Body Modes') and
             n != 'Geometry Rigid Body Modes']
    assert added and plate[added[0]].num_shapes == 6
    assert view['view'] in ('shape3d', 'reading')


def test_mac_of_a_set_against_itself_and_another_on_the_geometry(plate, validate):
    run(plate, 'project.compute_frfs', method='H1')
    plate.fit_modes('Time History H1 FRFs', bounds=(100.0, 1300.0), limit=4)
    fitted = next(name for name in plate if projects.kind_of(plate[name]) == 'shape_set')
    matrix, shape = run(plate, 'project.mac', first=fitted, second=fitted)
    wire_views([matrix, shape], validate)
    assert matrix['title'] == f'Auto-MAC of {fitted}' and matrix['selects'] == 'shape3d'
    assert np.allclose(np.diag(np.asarray(matrix['values'])), 1.0)
    assert len(shape['overlays']) == len(shape['modes'])
    plate.fit_modes('Time History H1 FRFs', bounds=(100.0, 1300.0), limit=3)
    other = [name for name in plate if projects.kind_of(plate[name]) == 'shape_set'][-1]
    (matrix, _) = run(plate, 'project.mac', first=fitted, second=other)
    assert np.asarray(matrix['values']).shape == (plate[fitted].num_shapes,
                                                  plate[other].num_shapes)


def test_mac_without_a_geometry_is_the_matrix_alone(plate):
    run(plate, 'project.compute_frfs', method='H1')
    plate.fit_modes('Time History H1 FRFs', bounds=(100.0, 1300.0), limit=3)
    plate.remove(*[name for name in plate if projects.kind_of(plate[name]) == 'geometry'])
    fitted = next(name for name in plate if projects.kind_of(plate[name]) == 'shape_set')
    assert [v['view'] for v in run(plate, 'project.mac', first=fitted, second=fitted)] == ['matrix']
def test_plot_draws_the_records_named_on_one_plot(plate, validate):
    (view,) = run(plate, 'project.plot', data='Time History', records='1-3')
    wire_views([view], validate)
    assert view['view'] == 'spectrum' and view['title'] == 'Time History'
    assert [s['name'] for s in view['series']] == [
        plate['Time History'].record_label(i) for i in range(3)]
    assert view['x']['label'] == 'Time' and view['y']['label'] == 'Acceleration'


def test_plot_leaves_out_records_of_another_quantity_and_says_so(plate):
    (view,) = run(plate, 'project.plot', data='Time History', records='1-13')
    assert len(view['series']) == 11
    assert view['title'] == 'Time History (2 of another quantity left out)'


@pytest.mark.parametrize(('text', 'expected'), [('', list(range(8))), ('2', [1]),
                                                ('1-3, 3, 7', [0, 1, 2, 6])])
def test_records_are_named_from_one(text, expected):
    from hertzforge.service.ops_project import chosen_records
    assert chosen_records(text, 13) == expected


@pytest.mark.parametrize('text', ['0', '14', '3-x', '5-2'])
def test_records_outside_the_object_are_refused(text):
    from hertzforge.service.ops_project import chosen_records
    with pytest.raises(ProtocolError) as error:
        chosen_records(text, 13)
    assert error.value.code == 'refused'
