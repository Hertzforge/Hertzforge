"""A connection's own project: new, open, import and save by path, the
summary the shell lists, and the progress a long import reports."""

from __future__ import annotations

import asyncio
from pathlib import Path

import pytest
from websockets.asyncio.client import connect

from hertzforge.service import frames, projects
from hertzforge.service.protocol import ProtocolError
from hertzforge.service.server import Service, start

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
CORPUS = Path(__file__).resolve().parents[2] / 'testdata' / 'hforge_corpus'


def test_kinds_are_class_names_in_lower_case_with_underscores(modal_run):
    kinds = {name: projects.kind_of(obj) for name, obj in modal_run.project.items()}
    assert kinds == {'Channel Table': 'channel_table', 'Time History': 'time_history',
                     'FRF': 'frf', 'Multiple Coherence': 'multiple_coherence',
                     'Geometry': 'geometry'}


def test_a_new_project_is_empty_and_named(validate):
    state = projects.new({'title': ' Bracket Survey '})
    summary = projects.summary(state)
    validate(summary, 'project')
    assert summary == {'title': 'Bracket Survey', 'file': None,
                       'modified': False, 'objects': [], 'active_geometry': None, 'links': []}


def test_importing_a_run_lists_its_objects_and_reports_progress(validate):
    seen = []
    state, added = projects.import_file(
        None, {'path': str(PLATE / 'modal.nc4')},
        lambda done, total: seen.append((done, total)))
    summary = projects.summary(state)
    validate(summary, 'project')
    assert added == [o['name'] for o in summary['objects']]
    assert summary['modified'] and summary['file'] is None
    history = next(o for o in summary['objects'] if o['kind'] == 'time_history')
    assert history['records'] == state.project[history['name']].num_records
    assert history['source'] == 'SIM' and history['units_defined']
    # the reader's objects are linked into one group with its channel table
    assert all(o['groups'] == ['Group 1'] for o in summary['objects'])
    assert seen and seen[-1][0] == seen[-1][1]
    # journaled as the call a script makes, with no progress callable in it
    assert state.project.journal[-1] == f"project.import_file({str(PLATE / 'modal.nc4')!r})"


def test_a_second_import_adds_to_the_same_project():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    before = len(state.project)
    same, added = projects.import_file(state, {'path': str(PLATE / 'geometry.unv'),
                                               'units': {'length_unit': 'mm'}})
    assert same is state and len(state.project) == before + len(added)
    geometry = state.project[added[0]]
    assert projects.kind_of(geometry) == 'geometry' and geometry.units_defined


def test_undeclared_units_import_and_say_so():
    state, added = projects.import_file(None, {'path': str(PLATE / 'geometry.unv')})
    (entry,) = [o for o in projects.summary(state)['objects'] if o['name'] == added[0]]
    assert entry['units_defined'] is False


def test_save_then_open_gives_the_same_objects(tmp_path, validate):
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    written = projects.save(state, {'path': str(tmp_path / 'survey')})
    assert written == str(tmp_path / 'survey.hforge')
    assert not state.modified and state.project.file == written
    opened = projects.open_file({'path': written})
    summary = projects.summary(opened)
    validate(summary, 'project')
    assert summary['file'] == written and not summary['modified']
    assert summary['objects'] == projects.summary(state)['objects']
    # saved again to the file it came from
    assert projects.save(opened, {}) == written


def test_a_project_from_the_corpus_opens(validate):
    summary = projects.summary(projects.open_file({'path': str(CORPUS / 'schema1.hforge')}))
    validate(summary, 'project')
    assert summary['objects']


def test_a_data_file_is_refused_as_a_project_with_a_pointer_to_import():
    with pytest.raises(ProtocolError, match='modal.nc4 is not a project file .*import it into '
                       'a project') as error:
        projects.open_file({'path': str(PLATE / 'modal.nc4')})
    assert error.value.code == 'refused'


def test_files_that_cannot_be_read_are_refused_in_the_cores_words(tmp_path):
    (tmp_path / 'notes.txt').write_text('hello there')
    with pytest.raises(ProtocolError, match='No importer recognizes') as error:
        projects.import_file(None, {'path': str(tmp_path / 'notes.txt')})
    assert error.value.code == 'refused'
    with pytest.raises(ProtocolError, match='no file at') as error:
        projects.import_file(None, {'path': str(tmp_path / 'gone.nc4')})
    assert error.value.code == 'not_found'
    with pytest.raises(ProtocolError, match='no file at') as error:
        projects.open_file({'path': str(tmp_path / 'gone.hforge')})
    assert error.value.code == 'not_found'
    (tmp_path / 'fake.hforge').write_text('not hdf5')
    with pytest.raises(ProtocolError) as error:
        projects.open_file({'path': str(tmp_path / 'fake.hforge')})
    assert error.value.code == 'refused'


def test_saving_needs_a_project_and_somewhere_to_write(tmp_path):
    with pytest.raises(ProtocolError, match='no project open'):
        projects.save(None, {})
    with pytest.raises(ProtocolError, match='needs "path"'):
        projects.save(projects.new({}), {})
    with pytest.raises(ProtocolError, match='vdyn'):
        projects.save(projects.new({}), {'path': str(tmp_path / 'old.vdyn')})


def test_bad_params_are_bad_requests():
    for call, params in [(projects.open_file, {}), (projects.open_file, {'path': ''}),
                         (projects.new, {'title': ''})]:
        with pytest.raises(ProtocolError) as error:
            call(params)
        assert error.value.code == 'bad_request'
    with pytest.raises(ProtocolError) as error:
        projects.import_file(None, {'path': 'x.unv', 'units': {'length_unit': 3}})
    assert error.value.code == 'bad_request'


# ---- over the socket ------------------------------------------------------------

async def _exchange(requests):
    """Send each request in turn and collect every frame up to its
    response; the frames for each request, decoded."""
    service = Service()
    server = await start(service, port=0)
    port = server.sockets[0].getsockname()[1]
    out = []
    try:
        async with connect(f'ws://127.0.0.1:{port}', max_size=None) as socket:
            frames.decode(await socket.recv())             # hello
            for ident, (method, params) in enumerate(requests, 1):
                await socket.send(frames.encode_text(
                    {'type': 'request', 'id': ident, 'method': method,
                     'params': params}))
                got = []
                while True:
                    message = frames.decode(await socket.recv())
                    got.append(message)
                    if message['type'] == 'response':
                        break
                out.append(got)
    finally:
        server.close()
        await server.wait_closed()
        service.close()
    return out


def test_a_session_imports_saves_and_opens_with_progress_events(tmp_path, validate):
    path = str(PLATE / 'modal.nc4')
    target = str(tmp_path / 'run.hforge')
    imported, saved, opened, created = asyncio.run(_exchange([
        ('project.import', {'path': path}),
        ('project.save', {'path': target}),
        ('project.open', {'path': target}),
        ('project.new', {'title': 'Empty'}),
    ]))
    for message in imported + saved + opened + created:
        validate(message)
    *events, response = imported
    assert events and all(e['topic'] == 'project.progress' for e in events)
    for event in events:
        validate(event['data'], 'projectProgress')
    assert events[-1]['data']['done'] == events[-1]['data']['total']
    assert events[-1]['data']['path'] == path
    assert [e['seq'] for e in events] == sorted(e['seq'] for e in events)
    validate(response['result'], 'projectImportResult')
    validate(saved[-1]['result'], 'projectSaveResult')
    assert saved[-1]['result']['path'] == target
    assert opened[-1]['result']['project']['file'] == target
    assert (opened[-1]['result']['project']['objects']
            == response['result']['project']['objects'])
    assert created[-1]['result']['project']['objects'] == []


def test_refusals_over_the_socket_keep_the_project(tmp_path):
    imported, refused, again = asyncio.run(_exchange([
        ('project.import', {'path': str(PLATE / 'geometry.unv')}),
        ('project.open', {'path': str(PLATE / 'modal.nc4')}),
        ('project.import', {'path': str(PLATE / 'geometry.unv')}),
    ]))
    assert refused[-1]['error']['code'] == 'refused'
    # the refused open left the imported project in place: the geometry again
    # comes in beside the first one under a new name
    first = imported[-1]['result']['project']['objects']
    second = again[-1]['result']['project']['objects']
    assert len(second) == 2 * len(first)


def test_save_with_nothing_open_is_refused():
    (response,) = asyncio.run(_exchange([('project.save', {'path': '/tmp/x.hforge'})]))[0:1]
    assert response[-1]['error']['code'] == 'refused'


@pytest.mark.parametrize('name', ['request_project_import', 'event_project_progress',
                                  'response_project_import'])
def test_the_project_goldens_validate_and_round_trip(name, validate):
    import json

    from .conftest import GOLDEN

    text = (GOLDEN / f'{name}.json').read_text(encoding='utf-8')
    message = frames.decode(text)
    validate(message)
    part = {'request_project_import': ('params', 'projectImportParams'),
            'event_project_progress': ('data', 'projectProgress'),
            'response_project_import': ('result', 'projectImportResult')}[name]
    validate(message[part[0]], part[1])
    assert frames.pack(message) == json.dumps(message, separators=(',', ':'),
                                              ensure_ascii=False)


def test_objects_are_renamed_removed_and_a_geometry_made_active_over_the_socket(validate):
    responses = asyncio.run(_exchange([
        ('project.import', {'path': str(PLATE / 'modal.nc4')}),
        ('project.import', {'path': str(PLATE / 'geometry.unv')}),
        ('project.import', {'path': str(PLATE / 'geometry.unv')}),
        ('project.rename', {'name': 'Time History', 'to': 'Hammer Run'}),
        ('project.activate', {'geometry': 'Geometry (2)'}),
        ('project.remove', {'names': ['Geometry']}),
        ('project.rename', {'name': 'Hammer Run', 'to': 'Channel Table'}),
        ('project.remove', {'names': ['Hammer Run', 'Nothing']}),
        ('project.activate', {'geometry': 'Hammer Run'}),
    ]))
    results = [got[-1] for got in responses]
    for response in results[3:6]:
        validate(response['result'], 'projectResult')
    renamed = results[3]['result']['project']
    names = [o['name'] for o in renamed['objects']]
    assert 'Hammer Run' in names and 'Time History' not in names
    assert renamed['modified']
    assert results[4]['result']['project']['active_geometry'] == 'Geometry (2)'
    removed = results[5]['result']['project']
    assert 'Geometry' not in [o['name'] for o in removed['objects']]
    assert removed['active_geometry'] == 'Geometry (2)'
    assert results[6]['error']['code'] == 'refused', 'a name in use'
    assert results[7]['error']['code'] == 'not_found'
    assert results[8]['error']['code'] == 'refused', 'not a geometry'


def test_a_refused_remove_removes_nothing():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    before = list(state.project)
    with pytest.raises(ProtocolError):
        projects.remove(state, {'names': [before[0], 'Nothing']})
    assert list(state.project) == before


def test_edits_need_an_open_project_and_well_formed_params():
    with pytest.raises(ProtocolError, match='none is open'):
        projects.rename(None, {'name': 'a', 'to': 'b'})
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    with pytest.raises(ProtocolError, match='"names"') as error:
        projects.remove(state, {'names': 'Time History'})
    assert error.value.code == 'bad_request'


def test_a_reference_that_did_not_resolve_is_listed_and_can_be_removed(tmp_path, monkeypatch, validate):
    import os

    import numpy as np

    import hertzforge as hz
    from hertzforge.core.data import TimeHistory
    from hertzforge.io import native

    monkeypatch.setattr(native, 'REFERENCE_FROM_BYTES', 1024)
    run = hz.Project('run')
    run.add('Run', TimeHistory(np.arange(4096) / 1024.0, np.ones((2, 4096)),
                               response_dof=['1Z+', '2Z+']))
    run_path = run.save(tmp_path / 'run.hforge')
    analysis = hz.Project('analysis')
    analysis.import_file(run_path)
    path = analysis.save(tmp_path / 'analysis.hforge')
    os.remove(run_path)
    with pytest.warns(UserWarning):
        state = projects.open_file({'path': path})
    summary = projects.summary(state)
    validate(summary, 'project')
    assert summary['objects'] == []
    (missing,) = summary['unresolved']
    assert missing == {'name': 'Run', 'kind': 'data', 'path': str(run_path),
                       'reason': 'not found'}
    projects.remove(state, {'names': ['Run']})
    assert 'unresolved' not in projects.summary(state)
