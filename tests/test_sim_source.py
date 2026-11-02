"""Simulated data stays labeled (FILE_FORMAT_PLAN.md, section 3).

A Rattlesnake run says what it drove in its `hardware` attribute. What
it measured is labeled `SIM` or `HARDWARE` from that on import, what a
project derives from it carries the same label, and the label is saved
with the object, so a result from a simulated plant can never pass for a
measurement.
"""

import shutil

import h5py
import netCDF4
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge.core.data import HARDWARE, SIM
from hertzforge.io import native, rattlesnake

RUN = fixture_path('plate', 'random.nc4')


def imported(path):
    project = hertzforge.Project('labeled')
    project.import_file(str(path))
    return project


def with_hardware(tmp_path, value):
    path = tmp_path / f'hardware_{value}.nc4'
    shutil.copy(RUN, path)
    with netCDF4.Dataset(path, 'a') as ds:
        ds.hardware = value
    return path


def test_a_run_on_a_simulated_plant_is_labeled_sim_and_its_target_is_not():
    project = imported(RUN)
    assert project['Time History'].source == SIM
    assert project['Specification'].source is None


def test_what_a_project_derives_carries_the_label(tmp_path):
    project = imported(RUN)
    psds = project.compute_psds('Time History')
    assert project[psds].source == SIM
    velocity = project.integrate('Time History')
    assert project[velocity].source == SIM
    assert project[project.compute_psds(velocity)].source == SIM


def test_the_label_is_saved_and_an_unlabeled_object_stays_unlabeled(tmp_path):
    project = imported(RUN)
    path = project.save(tmp_path / 'labeled')
    back = hertzforge.Project.open(path)
    assert back['Time History'].source == SIM
    assert back['Specification'].source is None
    with h5py.File(path) as f:
        kinds = {f['objects'][k].attrs['name']: dict(f['objects'][k].attrs)
                 for k in f['objects']}
    assert kinds['Time History']['source'] == SIM
    assert 'source' not in kinds['Specification']


@pytest.mark.parametrize('value,label', [(0, HARDWARE), (1, HARDWARE), (3, HARDWARE),
                                         (4, SIM), (5, SIM), (6, SIM),
                                         (-1, None), (8, None), (42, None)])
def test_the_hardware_number_decides(tmp_path, value, label):
    assert rattlesnake.HARDWARE_TYPES.get(value) == label
    project = imported(with_hardware(tmp_path, value))
    assert project['Time History'].source == label


def test_a_file_from_before_the_label_reads_unknown():
    project = native.load(str(fixture_path('hforge_corpus', 'schema1.hforge')))
    assert project['Time History'].source is None
