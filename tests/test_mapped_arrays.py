"""Large arrays are mapped from a .hforge, not read (`io.native._read_array`).

Opening a long run must not read its samples: the ordinate comes back as
an ordinary array mapped onto the file, copy-on-write, and the operating
system pages in only what is touched. These pin what that promises and
where it stops: an edit stays in memory, saving over the open file keeps
the open project's values, and a compressed array, a file held in memory
and a small array are read as before. The threshold is lowered here so a
test file of a few hundred kilobytes is "large".
"""

import os
import pathlib

import h5py
import numpy as np
import pytest

import hertzforge
from hertzforge.core.data import TimeHistory
from hertzforge.io import export_file, import_file, native

pytestmark = pytest.mark.skipif(os.name == 'nt', reason='Windows reads every array')


@pytest.fixture(autouse=True)
def small_threshold(monkeypatch):
    monkeypatch.setattr(native, 'MAP_FROM_BYTES', 64 * 1024)


def mapped(array) -> bool:
    """Whether `array`'s bytes are a mapping of a file."""
    while array is not None:
        if isinstance(array, np.memmap):
            return True
        array = getattr(array, 'base', None)
    return False


def run(samples=20_000, channels=4):
    rng = np.random.default_rng(1)
    t = np.arange(samples) / 1024.0
    return TimeHistory(t, rng.standard_normal((channels, samples)),
                       response_dof=[f'{i + 1}Z+' for i in range(channels)],
                       ordinate_dim='acceleration', ordinate_unit='m/s**2')


def saved(tmp_path, compress=False):
    project = hertzforge.Project('mapped')
    project.add('Run', run())
    project.add('Short', TimeHistory(np.arange(8.0), np.ones((1, 8)), response_dof='9Z+'))
    return project, project.save(tmp_path / 'run', compress=compress)


def test_a_large_ordinate_is_mapped_and_reads_the_same(tmp_path):
    project, path = saved(tmp_path)
    back = hertzforge.Project.open(path)
    assert mapped(back['Run'].ordinate)
    assert not mapped(back['Short'].ordinate), 'a small array is read'
    assert np.array_equal(back['Run'].ordinate, project['Run'].ordinate)
    assert back['Run'] == project['Run']


def test_an_edit_stays_in_memory(tmp_path):
    _project, path = saved(tmp_path)
    back = hertzforge.Project.open(path)
    first = float(back['Run'].ordinate[0, 0])
    back['Run'].ordinate[0, 0] = 1e6
    again = hertzforge.Project.open(path)
    assert float(again['Run'].ordinate[0, 0]) == first


def test_saving_over_the_open_file_keeps_the_open_values(tmp_path):
    """The save writes a new file and renames it over the old one, so the
    open project's mapping stays on the bytes it had, and the file holds
    what was saved."""
    _project, path = saved(tmp_path)
    back = hertzforge.Project.open(path)
    before = np.array(back['Run'].ordinate)
    back['Run'].ordinate[1] *= 2.0
    back.save(path)
    assert np.array_equal(back['Run'].ordinate[0], before[0])
    reopened = hertzforge.Project.open(path)
    assert np.array_equal(reopened['Run'].ordinate[1], 2.0 * before[1])
    assert not [name for name in os.listdir(tmp_path) if name.endswith('.partial')]


def test_a_failed_save_leaves_the_last_file(tmp_path, monkeypatch):
    project, path = saved(tmp_path)
    good = pathlib.Path(path).read_bytes()

    def broken(obj, group):
        group.create_dataset('half', data=np.zeros(4))
        raise RuntimeError('the disk filled')

    monkeypatch.setattr(native, 'save_data', broken)
    monkeypatch.setattr(native, '_savers', lambda: [(TimeHistory, 'data', broken)])
    with pytest.raises(RuntimeError, match='the disk filled'):
        project.save(path)
    assert pathlib.Path(path).read_bytes() == good
    assert sorted(os.listdir(tmp_path)) == ['run.hforge']


def test_a_compressed_array_is_read(tmp_path):
    _project, path = saved(tmp_path, compress=True)
    back = hertzforge.Project.open(path)
    assert not mapped(back['Run'].ordinate)


def test_a_file_held_in_memory_is_read(tmp_path):
    """The .mat reader passes the native reader a file in memory, which
    has no bytes on disk to map."""
    data = run()
    mat = str(tmp_path / 'run.mat')
    export_file(data, mat)
    back = import_file(mat)
    assert not mapped(back.ordinate)
    assert np.array_equal(back.ordinate, data.ordinate)


def test_the_mapping_is_the_files_own_bytes(tmp_path):
    """Mapped at the dataset's offset, with its dtype: the first sample
    written is the first sample read, byte for byte."""
    _project, path = saved(tmp_path)
    with h5py.File(path) as f:
        group = next(f['objects'][k] for k in f['objects']
                     if f['objects'][k].attrs['name'] == 'Run')
        stored = group['ordinate'][0, :5]
    back = hertzforge.Project.open(path)
    assert np.array_equal(back['Run'].ordinate[0, :5], stored)
