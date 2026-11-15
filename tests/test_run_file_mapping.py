"""Large run files where a mapped file cannot be replaced (Windows): a run's
converted file is mapped there too, since nothing saves over it in place
while it is mapped; a save goes to a new name and leaves it byte for byte,
and an ordinary project is read, not mapped, and still saves in place.
`REPLACE_WHILE_MAPPED` stands in for the operating system, so this runs
on every one."""

from __future__ import annotations

import gc
import hashlib
import os

import pytest
import run_folders as rf

from hertzforge import Project
from hertzforge.io import native, run_convert


@pytest.fixture
def like_windows(monkeypatch):
    monkeypatch.setattr(native, 'REPLACE_WHILE_MAPPED', False)
    monkeypatch.setattr(native, 'MAP_FROM_BYTES', 1)
    yield
    gc.collect()


def _digest(path):
    with open(path, 'rb') as f:
        return hashlib.sha256(f.read()).hexdigest()


def _mapped_arrays(project):
    import numpy as np

    return [(name, key) for name, obj in project.items()
            for key, value in vars(obj).items()
            if isinstance(value, np.ndarray) and native._mapping_of(value) is not None]


def _run_file(tmp_path):
    folder = rf.random_run(str(tmp_path / '20261114-120000_random_3'))
    return folder, run_convert.converted(folder)


def test_a_run_file_is_known_by_its_place_in_its_run_folder(tmp_path):
    folder, path = _run_file(tmp_path)
    assert native.is_run_file(path)
    other = tmp_path / 'plain.hforge'
    Project.open(path).save(str(other))
    assert not native.is_run_file(other)
    assert not native.is_run_file(os.path.join(folder, 'renamed.hforge'))


def test_a_run_file_is_mapped_and_saves_to_a_new_name_leaving_it_whole(tmp_path, like_windows):
    _folder, path = _run_file(tmp_path)
    before = _digest(path)
    project = Project.open(path)
    assert _mapped_arrays(project), 'a run file is mapped where saves cannot replace it'
    assert native.mapped_files() == [native._file_key(path)]
    copy = str(tmp_path / 'my analysis.hforge')
    project.name = 'edited'
    project.save(copy)
    assert Project.open(copy).name == 'edited'
    with pytest.raises(native.MappedFileError, match='save the project under a new name'):
        project.save(path)
    assert _digest(path) == before
    assert not any(n.endswith('.partial') for n in os.listdir(os.path.dirname(path)))


def test_the_service_refuses_saving_over_a_mapped_run_file(tmp_path, like_windows):
    from hertzforge.service.projects import OpenProject, save
    from hertzforge.service.protocol import ProtocolError

    _folder, path = _run_file(tmp_path)
    before = _digest(path)
    state = OpenProject(Project.open(path))
    with pytest.raises(ProtocolError) as caught:
        save(state, {})
    assert caught.value.code == 'refused' and 'new name' in caught.value.message
    assert save(state, {'path': str(tmp_path / 'kept.hforge')}).endswith('kept.hforge')
    assert _digest(path) == before


def test_an_ordinary_project_is_read_and_still_saves_in_place(tmp_path, like_windows):
    _folder, path = _run_file(tmp_path)
    plain = str(tmp_path / 'plain.hforge')
    Project.open(path).save(plain)
    gc.collect()
    project = Project.open(plain)
    assert _mapped_arrays(project) == []
    project.name = 'saved in place'
    project.save(plain)
    assert Project.open(plain).name == 'saved in place'


def test_a_mapping_ends_with_its_project_and_a_reopen_maps_again(tmp_path, like_windows):
    _folder, path = _run_file(tmp_path)
    project = Project.open(path)
    project.name = 'later'
    del project
    gc.collect()
    assert native.mapped_files() == []
    reopened = Project.open(path)
    reopened.name = 'later'
    # mapped again by the open: the save still goes elsewhere
    with pytest.raises(native.MappedFileError):
        reopened.save(path)


def test_elsewhere_a_mapped_run_file_is_saved_over_as_before(tmp_path, monkeypatch):
    monkeypatch.setattr(native, 'MAP_FROM_BYTES', 1)
    _folder, path = _run_file(tmp_path)
    project = Project.open(path)
    assert _mapped_arrays(project)
    if not native.REPLACE_WHILE_MAPPED:
        pytest.skip('Windows cannot replace a mapped file')
    project.name = 'in place'
    project.save(path)
    assert Project.open(path).name == 'in place'

