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

from .test_ops import wire_views

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
                                        for name, obj in project.items()})
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
        assert op.accepts == 'project'


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
