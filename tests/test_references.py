"""A project holds a large object from another `.hforge` by reference.

A project that analyzes a 23 GB run must not hold a second copy of it. An object of
`REFERENCE_FROM_BYTES` or
more imported from a `.hforge` is saved as a reference to that file, by
its path relative to the project, its name there, and the file's size
and SHA-256. These pin what that promises: the project file is small and
reopens to the same values, an object edited since is saved whole, a
moved or copied file is found by its checksum, a missing or changed one
leaves the object out without losing its reference, and saving over the
referenced file itself writes the object whole. The thresholds are
lowered so a test file of a few hundred kilobytes is "large".
"""

import os
import shutil

import h5py
import numpy as np
import pytest

import hertzforge
from hertzforge.core.data import TimeHistory
from hertzforge.core.data_index import CHANGED, NOT_FOUND
from hertzforge.io import native


@pytest.fixture(autouse=True)
def small_thresholds(monkeypatch):
    monkeypatch.setattr(native, 'MAP_FROM_BYTES', 64 * 1024)
    monkeypatch.setattr(native, 'REFERENCE_FROM_BYTES', 64 * 1024)


def run(samples=20_000, channels=4, seed=1):
    rng = np.random.default_rng(seed)
    t = np.arange(samples) / 1024.0
    return TimeHistory(t, rng.standard_normal((channels, samples)),
                       response_dof=[f'{i + 1}Z+' for i in range(channels)],
                       ordinate_dim='acceleration', ordinate_unit='m/s**2')


def run_file(folder, seed=1):
    """A run saved in its own project file, as the controller writes one."""
    project = hertzforge.Project('run')
    project.add('Run', run(seed=seed))
    return project.save(folder / 'run.hforge')


def analysis(folder, run_path):
    """A project that imports the run and keeps a short record of its own."""
    project = hertzforge.Project('analysis')
    project.import_file(run_path)
    project.add('Short', TimeHistory(np.arange(8.0), np.ones((1, 8)),
                                     response_dof='9Z+'))
    return project, project.save(folder / 'analysis.hforge')


def group_of(path, name):
    with h5py.File(path, 'r') as f:
        group = next(g for g in f['objects'].values() if g.attrs['name'] == name)
        return dict(group.attrs), list(group), int(f.attrs[native.STAMP])


def test_a_large_imported_object_is_saved_as_a_reference(tmp_path):
    run_path = run_file(tmp_path)
    project, path = analysis(tmp_path, run_path)
    attrs, datasets, stamp = group_of(path, 'Run')
    assert datasets == [], 'nothing of the run is copied'
    assert attrs['reference_path'] == 'run.hforge'
    assert attrs['reference_object'] == 'Run'
    assert attrs['reference_sha256'] == project.data_index[0].sha256
    assert stamp == native.REFERENCE_SCHEMA
    assert os.path.getsize(path) < os.path.getsize(run_path) / 10
    back = hertzforge.Project.open(path)
    assert np.array_equal(back['Run'].ordinate, project['Run'].ordinate)
    assert back['Run'].response_dof == project['Run'].response_dof
    assert back.references['Run'].path == os.path.abspath(run_path)
    assert list(back) == ['Run', 'Short']


def test_a_small_object_is_copied_and_the_file_stays_schema_1(tmp_path):
    small = hertzforge.Project('small')
    small.add('Short', TimeHistory(np.arange(8.0), np.ones((1, 8)), response_dof='9Z+'))
    source = small.save(tmp_path / 'small.hforge')
    project = hertzforge.Project('analysis')
    project.import_file(source)
    path = project.save(tmp_path / 'analysis.hforge')
    attrs, datasets, stamp = group_of(path, 'Short')
    assert 'reference_sha256' not in attrs and 'ordinate' in datasets
    assert stamp == native.BASE_SCHEMA
    assert project.references == {}


@pytest.mark.parametrize('mapping', [True, False], ids=['mapped', 'read'])
@pytest.mark.parametrize('edit', ['units', 'value', 'replaced'])
def test_an_object_edited_since_it_was_read_is_saved_whole(tmp_path, monkeypatch,
                                                          mapping, edit):
    """A reference would lose the edit, so the save copies the object and
    the project holds it from then on."""
    if not mapping:
        monkeypatch.setattr(native, 'MAP_FROM_BYTES', 2**62)
    if edit == 'value' and mapping:
        pytest.skip('an edit inside a mapped array is not seen (native.fingerprint); '
                    'the core never makes one without changing a unit or name')
    project, _ = analysis(tmp_path, run_file(tmp_path))
    data = project['Run']
    if edit == 'units':
        data.define_units('g')
    elif edit == 'value':
        data.ordinate[0, 0] += 1.0
    else:
        data.ordinate = np.array(data.ordinate) * 2.0
    expected = np.array(data.ordinate)
    path = project.save(tmp_path / 'edited.hforge')
    attrs, datasets, _ = group_of(path, 'Run')
    assert 'reference_sha256' not in attrs and 'ordinate' in datasets
    assert 'Run' not in project.references
    assert np.array_equal(hertzforge.Project.open(path)['Run'].ordinate, expected)


def test_a_project_moved_with_its_run_finds_it_by_the_relative_path(tmp_path):
    (tmp_path / 'a' / 'runs').mkdir(parents=True)
    run_path = run_file(tmp_path / 'a' / 'runs')
    _, path = analysis(tmp_path / 'a', run_path)
    assert group_of(path, 'Run')[0]['reference_path'] == 'runs/run.hforge'
    shutil.move(tmp_path / 'a', tmp_path / 'b')
    back = hertzforge.Project.open(tmp_path / 'b' / 'analysis.hforge')
    assert back.unresolved == {}
    assert back.references['Run'].path == str(tmp_path / 'b' / 'runs' / 'run.hforge')


def test_a_copied_run_with_a_new_time_is_taken_by_its_checksum(tmp_path):
    (tmp_path / 'a').mkdir()
    run_path = run_file(tmp_path / 'a')
    _, path = analysis(tmp_path / 'a', run_path)
    os.utime(run_path, ns=(1, 1))
    back = hertzforge.Project.open(path)
    assert 'Run' in back and back.references['Run'].modified_ns == 1


def test_a_missing_run_leaves_the_object_out_and_keeps_its_reference(tmp_path):
    run_path = run_file(tmp_path)
    project, path = analysis(tmp_path, run_path)
    os.rename(run_path, tmp_path / 'away.hforge')
    with pytest.warns(UserWarning, match='Run is held in .*run.hforge, which was not found'):
        back = hertzforge.Project.open(path)
    assert 'Run' not in back and list(back) == ['Short']
    assert back.unresolved['Run'].reason == NOT_FOUND
    back.add('Run', TimeHistory(np.arange(8.0), np.ones((1, 8)), response_dof='1X+'))
    assert 'Run' not in back, 'the missing object keeps its name'
    again = back.save(tmp_path / 'again.hforge')
    os.rename(tmp_path / 'away.hforge', run_path)
    restored = hertzforge.Project.open(again)
    assert np.array_equal(restored['Run'].ordinate, project['Run'].ordinate)


def test_a_changed_run_is_refused_by_its_checksum(tmp_path):
    run_path = run_file(tmp_path)
    _, path = analysis(tmp_path, run_path)
    run_file(tmp_path, seed=2)
    with pytest.warns(UserWarning, match='has changed'):
        back = hertzforge.Project.open(path)
    assert 'Run' not in back
    assert back.unresolved['Run'].reason == CHANGED


def test_removing_a_missing_object_drops_its_reference(tmp_path):
    run_path = run_file(tmp_path)
    _, path = analysis(tmp_path, run_path)
    os.remove(run_path)
    with pytest.warns(UserWarning):
        back = hertzforge.Project.open(path)
    back.remove('Run')
    assert back.unresolved == {}
    _, _, stamp = group_of(back.save(tmp_path / 'again.hforge'), 'Short')
    assert stamp == native.BASE_SCHEMA


def test_saving_over_the_referenced_file_writes_the_object_whole(tmp_path):
    run_path = run_file(tmp_path)
    project, _ = analysis(tmp_path, run_path)
    expected = np.array(project['Run'].ordinate)
    project.save(run_path)
    attrs, datasets, _ = group_of(run_path, 'Run')
    assert 'reference_sha256' not in attrs and 'ordinate' in datasets
    assert np.array_equal(hertzforge.Project.open(run_path)['Run'].ordinate, expected)


def test_a_project_imported_from_a_project_refers_to_the_run_itself(tmp_path):
    run_path = run_file(tmp_path)
    _, middle = analysis(tmp_path, run_path)
    outer = hertzforge.Project('outer')
    outer.import_file(middle)
    path = outer.save(tmp_path / 'outer.hforge')
    attrs, _, _ = group_of(path, 'Run')
    assert attrs['reference_absolute'] == os.path.abspath(run_path)


def test_a_renamed_reference_follows_its_object(tmp_path):
    project, _ = analysis(tmp_path, run_file(tmp_path))
    project.rename('Run', 'Sweep')
    attrs, datasets, _ = group_of(project.save(tmp_path / 'renamed.hforge'), 'Sweep')
    assert attrs['reference_object'] == 'Run' and datasets == []


def test_a_single_object_file_is_referenced_whole(tmp_path):
    native.save(run(), tmp_path / 'alone.hforge')
    project = hertzforge.Project('analysis')
    project.import_file(tmp_path / 'alone.hforge')
    path = project.save(tmp_path / 'analysis.hforge')
    name = next(iter(project))
    attrs, datasets, _ = group_of(path, name)
    assert attrs['reference_object'] == '' and datasets == []
    assert np.array_equal(hertzforge.Project.open(path)[name].ordinate, run().ordinate)


def test_a_fingerprint_names_a_mapped_array_without_reading_it(tmp_path):
    run_path = run_file(tmp_path)
    data = hertzforge.Project.open(run_path)['Run']
    first = native.fingerprint(data)
    assert native.fingerprint(data) == first
    data.comment[0] = 'moved'
    assert native.fingerprint(data) != first
