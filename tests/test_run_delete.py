"""Deleting a run: the data folder as the control service resolves it, and
`delete_run`, the one place a run is deleted, with every way it refuses.
Each refusal leaves every file where it was."""

from __future__ import annotations

import json
import os

import pytest
import run_folders as rf

from hertzforge import Project
from hertzforge.io import data_folder as df
from hertzforge.io import run_convert


def _run(root, name='20261114-120000_random_7', **kw):
    os.makedirs(root, exist_ok=True)
    return rf.random_run(os.path.join(str(root), name), **kw)


def _converted(root, name='20261114-120000_random_7'):
    folder = _run(root, name)
    return folder, run_convert.converted(folder)


def _files(path):
    return sorted(os.path.relpath(os.path.join(d, f), path)
                  for d, _s, fs in os.walk(path) for f in fs)


def _refused(code, folder, root, open_files=()):
    before = _files(root)
    with pytest.raises(df.RunDeleteRefused) as caught:
        df.delete_run(folder, root, open_files)
    assert caught.value.code == code, caught.value.message
    assert _files(root) == before, 'a refusal deletes nothing'
    return caught.value.message


def test_the_data_folder_is_resolved_as_the_control_service_resolves_it(monkeypatch, tmp_path):
    monkeypatch.setenv('HERTZFORGE_DATA_ROOT', str(tmp_path / 'runs'))
    assert df.data_root() == str(tmp_path / 'runs')
    assert df.data_root_source() == 'environment'
    monkeypatch.setenv('HERTZFORGE_DATA_ROOT', '')
    monkeypatch.setenv('HOME', str(tmp_path))
    monkeypatch.setenv('USERPROFILE', str(tmp_path))
    assert df.data_root() == os.path.join(str(tmp_path), 'Hertzforge_Data')
    assert df.data_root_source() == 'default'


def test_a_finished_run_is_deleted_with_its_converted_file(tmp_path):
    root = tmp_path / 'data'
    folder, project_file = _converted(root)
    keep = _run(root, '20261114-130000_random_8')
    plan = df.plan_delete(folder, root)
    assert (plan.folder, plan.project_file, plan.project_file_exists) == (
        os.path.realpath(folder), os.path.realpath(project_file), True)
    assert os.path.isdir(folder), 'a plan deletes nothing'
    done = df.delete_run(os.path.join(folder, 'run.json'), root)
    assert done == plan
    assert not os.path.exists(folder) and not os.path.exists(project_file)
    assert os.path.isdir(keep) and os.path.isdir(root)


def test_a_run_not_converted_yet_is_deleted_and_says_it_had_no_file(tmp_path):
    folder = _run(tmp_path / 'data')
    assert df.plan_delete(folder, tmp_path / 'data').project_file_exists is False
    df.delete_run(folder, tmp_path / 'data')
    assert not os.path.exists(folder)


def test_refused_when_the_run_is_live_or_did_not_end(tmp_path):
    root = tmp_path / 'data'
    live = rf.unfinished_run(str(root / 'live'))
    assert 'not complete' in _refused('incomplete', live, root)
    folder = _run(root)
    with open(os.path.join(folder, 'run.json')) as f:
        manifest = json.load(f)
    manifest.pop('complete')
    with open(os.path.join(folder, 'run.json'), 'w') as f:
        json.dump(manifest, f)
    _refused('incomplete', folder, root)


def test_refused_for_a_folder_that_is_not_a_run_folder(tmp_path):
    root = tmp_path / 'data'
    plain = root / 'notes'
    plain.mkdir(parents=True)
    (plain / 'a.txt').write_text('kept')
    assert 'not a run folder' in _refused('not_a_run_folder', plain, root)
    (plain / 'run.json').write_text(json.dumps({'schema': 'something-else/1',
                                                 'complete': True}))
    _refused('not_a_run_folder', plain, root)
    (plain / 'run.json').write_text('{not json')
    _refused('not_a_run_folder', plain, root)


def test_refused_outside_the_data_folder_and_for_the_data_folder_itself(tmp_path):
    root = tmp_path / 'data'
    root.mkdir()
    elsewhere = rf.random_run(str(tmp_path / 'elsewhere'))
    assert 'not inside the data folder' in _refused('outside_data_folder', elsewhere, root)
    # the data folder holding a run.json of its own is still not deleted
    (root / 'run.json').write_text(json.dumps({'schema': 'hertzforge-run/1',
                                                'complete': True}))
    _refused('outside_data_folder', root, root)
    _refused('outside_data_folder', root / '..' / 'elsewhere', root)
    assert os.path.isdir(elsewhere)


@pytest.mark.skipif(not hasattr(os, 'symlink') or os.name == 'nt',
                    reason='links made by a test need a POSIX file system here')
def test_refused_through_a_link_and_for_a_link_inside(tmp_path):
    root = tmp_path / 'data'
    root.mkdir()
    outside = rf.random_run(str(tmp_path / 'outside'))
    os.symlink(outside, root / 'linked')
    assert 'is a link' in _refused('link', root / 'linked', root)
    hop = tmp_path / 'hop'
    hop.mkdir()
    rf.random_run(str(hop / 'run'))
    os.symlink(hop, root / 'hop')
    _refused('link', root / 'hop' / 'run', root)
    folder = _run(root)
    target = tmp_path / 'precious.txt'
    target.write_text('never deleted')
    os.symlink(target, os.path.join(folder, 'streams', 'escape'))
    _refused('link_inside', folder, root)
    assert target.read_text() == 'never deleted'
    assert os.path.isdir(outside)


@pytest.mark.skipif(os.name == 'nt', reason='a data folder reached through a link')
def test_a_data_folder_reached_through_a_link_still_deletes_its_own_runs(tmp_path):
    real = tmp_path / 'real'
    real.mkdir()
    os.symlink(real, tmp_path / 'named')
    folder = _run(tmp_path / 'named')
    df.delete_run(folder, tmp_path / 'named')
    assert os.listdir(real) == []


def test_refused_while_its_project_file_is_open(tmp_path):
    root = tmp_path / 'data'
    folder, project_file = _converted(root)
    assert 'open another project first' in _refused('open', folder, root, [project_file])
    _refused('open', folder, root, [os.path.join(folder, 'streams')])
    df.delete_run(folder, root, [str(tmp_path / 'other.hforge')])
    assert not os.path.exists(folder)


def test_refused_for_a_folder_that_is_not_there(tmp_path):
    root = tmp_path / 'data'
    root.mkdir()
    _refused('not_found', root / 'gone', root)


def test_the_service_names_what_it_would_delete_then_deletes_it(tmp_path, monkeypatch):
    from hertzforge.service import run_files
    from hertzforge.service.protocol import ProtocolError

    root = tmp_path / 'data'
    monkeypatch.setenv('HERTZFORGE_DATA_ROOT', str(root))
    folder, project_file = _converted(root)
    said = run_files.request('runs.data_folder', {}, [])
    assert said == {'path': str(root), 'source': 'environment', 'exists': True,
                    'writable': True}
    plan = run_files.request('runs.delete', {'folder': folder, 'dry_run': True}, [])
    assert plan == {'folder': os.path.realpath(folder),
                    'project_file': os.path.realpath(project_file),
                    'project_file_exists': True, 'deleted': False}
    with pytest.raises(ProtocolError) as caught:
        run_files.request('runs.delete', {'folder': folder}, [project_file])
    assert caught.value.code == 'refused'
    with pytest.raises(ProtocolError) as caught:
        run_files.request('runs.delete', {'folder': str(root / 'gone')}, [])
    assert caught.value.code == 'not_found'
    with pytest.raises(ProtocolError) as caught:
        run_files.request('runs.delete', {'folder': folder, 'force': True}, [])
    assert caught.value.code == 'bad_request'
    done = run_files.request('runs.delete', {'folder': folder}, [])
    assert done['deleted'] is True and not os.path.exists(folder)


def test_a_project_names_the_run_folder_of_each_run_record(tmp_path):
    from hertzforge.service.projects import OpenProject, summary

    root = tmp_path / 'data'
    first, _ = _converted(root, 'a')
    second = _run(root, 'b')
    project = Project.open(first)
    project.import_file(second)
    listed = {o['name']: o for o in summary(OpenProject(project))['objects']}
    records = {n: o.get('run_folder') for n, o in listed.items() if o['kind'] == 'run_record'}
    assert records == {'Run Record': first, 'Run Record (2)': second}
    assert all('run_folder' not in o for o in listed.values() if o['kind'] != 'run_record')


def test_a_run_whose_file_is_mapped_is_not_deleted(tmp_path, monkeypatch):
    import gc

    from hertzforge.io import native
    from hertzforge.service import run_files
    from hertzforge.service.protocol import ProtocolError

    # as on Windows, where a run's converted file is mapped and nothing else is
    monkeypatch.setattr(native, 'REPLACE_WHILE_MAPPED', False)
    monkeypatch.setattr(native, 'MAP_FROM_BYTES', 1)
    monkeypatch.setenv('HERTZFORGE_DATA_ROOT', str(tmp_path))
    folder, path = _converted(tmp_path)
    project = Project.open(path)
    assert native.mapped_files()
    with pytest.raises(ProtocolError, match='open another project first'):
        run_files.request('runs.delete', {'folder': folder}, [])
    assert os.path.isfile(path)
    del project
    gc.collect()
    assert run_files.request('runs.delete', {'folder': folder}, [])['deleted'] is True
