"""A run's transformations in its report.

A Rattlesnake environment may control a virtual response through a
response transformation over its control channels, and drive through a
reference transformation over its drive channels. The report of such a
run shows each matrix it used as a table, after the instrumentation
table: a row per transformed degree of freedom, numbered from 1 as the
run's specification and spectra name them, and a column per physical
channel headed by its DOF. A run that used neither shows no such table.
The run is synthetic, written here in the layout the controller saves.
"""

import json
import re

import numpy as np
import pytest

import hertzforge
from hertzforge.core.report import random_template
from hertzforge.report import render_html

RATE = 256.0

SAMPLES = 4096

CONTROLS, DRIVES = 4, 2

#: two virtual rows over the four control channels
RESPONSE = np.array([[0.25, 0.25, 0.25, 0.25],
                     [0.5, 0.5, -0.5, -0.5]])

#: three virtual drives over the two drive channels
REFERENCE = np.array([[1.0, 0.0],
                      [0.0, 1.0],
                      [0.5, -0.5]])

LINES = 5


def _write(path, response=None, reference=None):
    import netCDF4

    channels = CONTROLS + DRIVES
    rows = CONTROLS if response is None else response.shape[0]
    rng = np.random.default_rng(3)
    with netCDF4.Dataset(path, 'w', format='NETCDF4') as ds:
        ds.sample_rate = RATE
        ds.createDimension('response_channels', channels)
        ds.createDimension('time_samples', SAMPLES)
        ds.createVariable('time_data', 'f8',
                          ('response_channels', 'time_samples'))[...] = \
            rng.standard_normal((channels, SAMPLES))
        group = ds.createGroup('channels')
        for name, values in (
                ('node_number', [str(101 + i) for i in range(channels)]),
                ('node_direction', ['Z+'] * CONTROLS + ['X-'] * DRIVES),
                ('unit', ['m/s^2'] * CONTROLS + ['N'] * DRIVES),
                ('feedback_device', [''] * CONTROLS + ['Input'] * DRIVES)):
            group.createVariable(name, str, ('response_channels',))[:] = \
                np.array(values, dtype=object)
        ds.createDimension('num_environments', 1)
        ds.createVariable('environment_names', str,
                          ('num_environments',))[0] = 'Random'
        ds.createVariable('environment_types', int,
                          ('num_environments',))[0] = 0
        env = ds.createGroup('Random')
        env.samples_per_frame = 256
        env.frames_in_cpsd = 4
        env.createDimension('control_channels', CONTROLS)
        env.createVariable('control_channel_indices', 'i4',
                           ('control_channels',))[...] = range(CONTROLS)
        for form, matrix in (('response', response), ('reference', reference)):
            if matrix is None:
                continue
            env.createDimension(f'{form}_transformation_rows', matrix.shape[0])
            env.createDimension(f'{form}_transformation_cols', matrix.shape[1])
            env.createVariable(f'{form}_transformation_matrix', 'f8',
                               (f'{form}_transformation_rows',
                                f'{form}_transformation_cols'))[...] = matrix
        env.createDimension('fft_lines', LINES)
        env.createDimension('two', 2)
        env.createDimension('specification_channels', rows)
        env.createVariable('specification_frequency_lines', 'f8',
                           ('fft_lines',))[...] = 10.0 + np.arange(LINES) * 20
        cpsd = np.zeros((LINES, rows, rows))
        for k in range(rows):
            cpsd[:, k, k] = 1.0
        for name, values in (('specification_cpsd_matrix_real', cpsd),
                             ('specification_cpsd_matrix_imag', 0 * cpsd)):
            env.createVariable(name, 'f8',
                               ('fft_lines', 'specification_channels',
                                'specification_channels'))[...] = values
        diagonal = np.ones((LINES, rows))
        for name, (low, high) in (('specification_warning_matrix', (0.5, 2.0)),
                                  ('specification_abort_matrix', (0.25, 4.0))):
            var = env.createVariable(name, 'f8', ('two', 'fft_lines',
                                                  'specification_channels'))
            var[0] = diagonal * low
            var[1] = diagonal * high
    return str(path)


def _tables(project):
    """Every table the random report of `project` draws, by caption."""
    page = render_html(random_template(project,
                                       object_groups=project.object_groups),
                       project, object_groups=project.object_groups)
    payload = json.loads(re.search(
        r'<script id="data"[^>]*>(.*?)</script>', page, re.DOTALL).group(1))
    return {b['caption']: b for b in payload['blocks']
            if b['kind'] == 'table'}


def _transformations(tables, form):
    return [b for caption, b in tables.items()
            if caption.startswith(f'{form} transformation'.capitalize())]


def test_a_response_transformation_is_a_labeled_table(tmp_path):
    project = hertzforge.random_vibration_run(
        _write(tmp_path / 'response.nc4', response=RESPONSE))
    tables = _tables(project)
    assert 'Instrumentation' in tables
    [table] = _transformations(tables, 'response')
    assert 'Random' in table['caption']
    assert table['headers'] == ['Row', '101Z+', '102Z+', '103Z+', '104Z+']
    assert [row[0] for row in table['rows']] == ['1', '2']
    assert np.allclose([[float(v) for v in row[1:]] for row in table['rows']],
                       RESPONSE)
    assert not _transformations(tables, 'reference')


def test_a_reference_transformation_is_a_labeled_table(tmp_path):
    project = hertzforge.random_vibration_run(
        _write(tmp_path / 'reference.nc4', reference=REFERENCE))
    tables = _tables(project)
    [table] = _transformations(tables, 'reference')
    assert table['headers'] == ['Row', '105X-', '106X-'], (
        'the columns are the drive channels, not the controls')
    assert [row[0] for row in table['rows']] == ['1', '2', '3']
    assert np.allclose([[float(v) for v in row[1:]] for row in table['rows']],
                       REFERENCE)
    assert not _transformations(tables, 'response')


def test_a_run_with_neither_shows_no_transformation(tmp_path):
    project = hertzforge.random_vibration_run(_write(tmp_path / 'plain.nc4'))
    report = random_template(project, object_groups=project.object_groups)
    assert not [b for b in report.blocks if 'form' in b], (
        'no block is written for a matrix the run did not use')
    tables = _tables(project)
    assert 'Instrumentation' in tables
    assert not _transformations(tables, 'response')
    assert not _transformations(tables, 'reference')


def test_both_survive_a_project_saved_and_opened(tmp_path):
    project = hertzforge.random_vibration_run(_write(
        tmp_path / 'both.nc4', response=RESPONSE, reference=REFERENCE))
    project.save(tmp_path / 'both.hforge')
    back = hertzforge.load(tmp_path / 'both.hforge')
    tables = _tables(back)
    [response] = _transformations(tables, 'response')
    [reference] = _transformations(tables, 'reference')
    assert response['headers'][1:] == ['101Z+', '102Z+', '103Z+', '104Z+']
    assert reference['headers'][1:] == ['105X-', '106X-']


@pytest.mark.parametrize('form', ['response', 'reference'])
def test_a_block_whose_loop_lost_its_matrix_is_not_drawn(tmp_path, form):
    """A report saved with the tables, over a table re-imported
    without the matrix, draws nothing for it rather than an empty
    grid."""
    project = hertzforge.random_vibration_run(_write(
        tmp_path / 'both.nc4', response=RESPONSE, reference=REFERENCE))
    report = random_template(project, object_groups=project.object_groups)
    table = next(o for o in project.values()
                 if type(o).__name__ == 'ChannelTable')
    setattr(table.environments['Random'], f'{form}_transformation', None)
    page = render_html(report, project, object_groups=project.object_groups)
    payload = json.loads(re.search(
        r'<script id="data"[^>]*>(.*?)</script>', page, re.DOTALL).group(1))
    captions = [b['caption'] for b in payload['blocks']
                if b['kind'] == 'table']
    assert not any(c.startswith(f'{form.capitalize()} transformation')
                   for c in captions)
