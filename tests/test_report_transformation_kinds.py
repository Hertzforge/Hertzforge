"""Each report shows the transformations of its own environment.

A Rattlesnake run may hold several environments, each with its own
response and reference transformations: a mixed run's random may
control its channels as measured while its sine controls through a
matrix. The channel table keeps each loop's kind with its matrices, and
the random report shows only the random loop's tables, the sine report
only the sine loop's. The run is synthetic, written here in the layout
the controller saves.
"""

import json
import re

import h5py
import numpy as np
import pytest

import hertzforge
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.report import random_template, sine_template
from hertzforge.report import render_html

CHANNELS, DRIVES = 6, 2

#: the sine's response transformation over control channels 1 to 4
SINE_RESPONSE = np.array([[0.25, 0.25, 0.25, 0.25],
                          [0.5, 0.5, -0.5, -0.5]])

#: the sine's reference transformation over the two drives
SINE_REFERENCE = np.array([[1.0, 1.0]])

#: a random response transformation, for the run where both have one
RANDOM_RESPONSE = np.array([[1.0, -1.0, 0.0, 0.0]])


def _write(path, random_matrix=None):
    """Random (type 1) and sine (type 3) over the same channels; the
    sine controls through matrices, the random through `random_matrix`
    or none."""
    import netCDF4

    total = CHANNELS + DRIVES
    with netCDF4.Dataset(str(path), 'w', format='NETCDF4') as ds:
        ds.sample_rate = 256.0
        ds.createDimension('response_channels', total)
        ds.createDimension('time_samples', 64)
        ds.createVariable('time_data', 'f8', ('response_channels',
                                              'time_samples'))[...] = 0.0
        group = ds.createGroup('channels')
        for name, values in (
                ('node_number', [str(101 + i) for i in range(total)]),
                ('node_direction', ['Z+'] * CHANNELS + ['Y-'] * DRIVES),
                ('unit', ['m/s^2'] * CHANNELS + ['N'] * DRIVES),
                ('feedback_device', [''] * CHANNELS + ['Input'] * DRIVES)):
            group.createVariable(name, str, ('response_channels',))[:] = \
                np.array(values, dtype=object)
        ds.createDimension('num_environments', 2)
        ds.createVariable('environment_names', str,
                          ('num_environments',))[:] = np.array(
                              ['Shaker A', 'Shaker B'], dtype=object)
        ds.createVariable('environment_types', int,
                          ('num_environments',))[:] = [1, 3]
        for name, matrices in (
                ('Shaker A', {'response': random_matrix}),
                ('Shaker B', {'response': SINE_RESPONSE,
                              'reference': SINE_REFERENCE})):
            env = ds.createGroup(name)
            env.createDimension('control_channels', 4)
            env.createVariable('control_channel_indices', 'i4',
                               ('control_channels',))[...] = [0, 1, 2, 3]
            for form, matrix in matrices.items():
                if matrix is None:
                    continue
                env.createDimension(f'{form}_transformation_rows',
                                    matrix.shape[0])
                env.createDimension(f'{form}_transformation_cols',
                                    matrix.shape[1])
                env.createVariable(f'{form}_transformation_matrix', 'f8',
                                   (f'{form}_transformation_rows',
                                    f'{form}_transformation_cols'))[...] = \
                    matrix
    return str(path)


def _table(objects):
    return next(o for o in objects.values() if isinstance(o, ChannelTable))


def _tables(template, objects):
    """{(environment, form): table block} the report draws."""
    groups = [{'members': list(objects), 'role': 'Basis'}]
    page = render_html(template(objects, object_groups=groups), objects,
                       object_groups=groups)
    payload = json.loads(re.search(
        r'<script id="data"[^>]*>(.*?)</script>', page, re.DOTALL).group(1))
    found = {}
    for block in payload['blocks']:
        for form in ('Response', 'Reference'):
            prefix = f'{form} transformation, '
            if block['kind'] == 'table' and block['caption'].startswith(prefix):
                environment = block['caption'][len(prefix):].split(':')[0]
                found[environment, form.lower()] = block
    return found


def test_each_loop_keeps_the_kind_the_file_gives_it(tmp_path):
    objects = hertzforge.import_file(_write(tmp_path / 'mixed.nc4'))
    loops = _table(objects).environments
    assert loops['Shaker A'].kind == 'random'
    assert loops['Shaker B'].kind == 'sine'
    assert loops['Shaker A'].response_transformation is None
    assert np.allclose(loops['Shaker B'].response_transformation,
                       SINE_RESPONSE)


def test_the_random_report_shows_none_of_the_sines_matrices(tmp_path):
    objects = dict(hertzforge.import_file(_write(tmp_path / 'mixed.nc4')))
    assert _tables(random_template, objects) == {}, (
        'the random loop used no transformation; the sine loop did')


def test_the_sine_report_shows_the_sines_matrices(tmp_path):
    objects = dict(hertzforge.import_file(_write(tmp_path / 'mixed.nc4')))
    tables = _tables(sine_template, objects)
    assert set(tables) == {('Shaker B', 'response'), ('Shaker B', 'reference')}
    response = tables['Shaker B', 'response']
    assert response['headers'] == ['Row', '101Z+', '102Z+', '103Z+', '104Z+']
    assert np.allclose([[float(v) for v in row[1:]]
                        for row in response['rows']], SINE_RESPONSE)
    assert tables['Shaker B', 'reference']['headers'] == ['Row', '107Y-',
                                                          '108Y-']


def test_when_both_have_one_each_report_shows_its_own(tmp_path):
    objects = dict(hertzforge.import_file(
        _write(tmp_path / 'both.nc4', random_matrix=RANDOM_RESPONSE)))
    random = _tables(random_template, objects)
    assert set(random) == {('Shaker A', 'response')}
    assert np.allclose([[float(v) for v in row[1:]]
                        for row in random['Shaker A', 'response']['rows']],
                       RANDOM_RESPONSE)
    assert set(_tables(sine_template, objects)) == {
        ('Shaker B', 'response'), ('Shaker B', 'reference')}


def test_the_kind_rides_the_file(tmp_path):
    objects = hertzforge.import_file(_write(tmp_path / 'mixed.nc4'))
    table = _table(objects)
    table.save(tmp_path / 'table.hforge')
    back = hertzforge.load(tmp_path / 'table.hforge')
    assert back == table
    assert back.environments['Shaker B'].kind == 'sine'


def _saved_without_kinds(table, path):
    """`table` saved as a file written before the kind was kept: the
    same layout with no kind on any loop."""
    table.save(path)
    with h5py.File(path, 'r+') as f:
        def strip(_name, item):
            if 'kind' in item.attrs and 'controls' in item:
                del item.attrs['kind']
        f.visititems(strip)
    return hertzforge.load(path)


def test_an_older_file_still_opens_with_its_matrices(tmp_path):
    objects = hertzforge.import_file(_write(tmp_path / 'mixed.nc4'))
    back = _saved_without_kinds(_table(objects), tmp_path / 'old.hforge')
    assert [loop.kind for loop in back.environments.values()] == ['', '']
    assert np.allclose(back.environments['Shaker B'].response_transformation,
                       SINE_RESPONSE)
    # two loops of unknown kind: neither report can tell which is its own
    assert _tables(random_template, {'table': back}) == {}
    assert _tables(sine_template, {'table': back}) == {}


@pytest.mark.parametrize('template', [random_template, sine_template])
def test_an_older_file_with_one_loop_shows_it(tmp_path, template):
    objects = hertzforge.import_file(_write(tmp_path / 'mixed.nc4'))
    table = _table(objects)
    table.remove_environment('Shaker A')
    back = _saved_without_kinds(table, tmp_path / 'old.hforge')
    assert back.environments['Shaker B'].kind == ''
    assert set(_tables(template, {'table': back})) == {
        ('Shaker B', 'response'), ('Shaker B', 'reference')}


def test_the_kind_rides_the_tables_json_form():
    """The shell's table files carry a loop as JSON and send it back;
    the kind comes back with it, and a form written before it was kept
    reads as unknown."""
    from hertzforge.core.channel_table import Environment
    from hertzforge.io.run_convert import (
        environment_from_json,
        environment_to_json,
    )

    loop = Environment(controls=[1, 2], drives=[3],
                       response_transformation=[[0.5, 0.5]], kind='sine')
    sent = environment_to_json(loop)
    assert sent['kind'] == 'sine'
    assert environment_from_json(json.loads(json.dumps(sent))) == loop
    del sent['kind']
    assert environment_from_json(sent).kind == ''
    assert 'kind' not in environment_to_json(environment_from_json(sent))
