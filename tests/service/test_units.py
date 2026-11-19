"""Declaring units a file did not state: by channel, an FRF's response and
reference channels apart, a record converted once all its channels are
named, and nothing changed by a unit that does not read."""

from __future__ import annotations

import asyncio
from pathlib import Path

import numpy as np
import pytest

import hertzforge as hz
from hertzforge.core.data import Frf, TimeHistory
from hertzforge.service import projects, units
from hertzforge.service.protocol import ProtocolError

from .test_projects import _exchange

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'


def frf_without_units():
    """Two responses against two references, one a drive point whose
    accelerometer and load cell share a DOF: units undeclared, the file
    saying what each channel measured."""
    response = ['1Z+', '2Z+', '1Z+', '2Z+']
    reference = ['1Z+', '1Z+', '3Z+', '3Z+']
    values = np.ones((4, 5), dtype=complex)
    return Frf(np.arange(5.0), values, response_dof=response, reference_dof=reference,
               ordinate_dim='unknown', dimension_hint='acceleration/force')


def project_with(**objects):
    project = hz.Project('units')
    for name, obj in objects.items():
        project.add(name, obj)
    return project


def test_an_frf_declares_its_response_and_reference_channels_apart(validate):
    project = project_with(FRF=frf_without_units())
    found = units.tables(project, 'FRF')
    validate(found, 'projectUnitsResult')
    sides = {t['side']: [r['key'] for r in t['rows']] for t in found['tables']}
    assert sides == {'response': ['1Z+|acceleration', '2Z+|acceleration'],
                     'reference': ['1Z+|force', '3Z+|force']}
    row = found['tables'][0]['rows'][0]
    assert row['unit'] is None and row['quantity'] == 'acceleration' and 'g' in row['choices']


def test_a_record_converts_once_both_its_channels_are_named():
    frf = frf_without_units()
    project = project_with(FRF=frf)
    converted = units.define(project, 'FRF', [
        {'side': 'response', 'key': '1Z+|acceleration', 'unit': 'g'},
        {'side': 'response', 'key': '2Z+|acceleration', 'unit': 'g'},
        {'side': 'reference', 'key': '1Z+|force', 'unit': 'lbf'}])
    assert converted == [0, 1], 'the records against 3Z+ wait for it'
    assert frf.undefined_records == [2, 3]
    assert np.real(frf.ordinate[0, 0]) == pytest.approx(9.80665 / 4.4482216152605)
    assert units.define(project, 'FRF', [
        {'side': 'reference', 'key': '3Z+|force', 'unit': 'N'}]) == [2, 3]
    assert frf.units_defined
    assert np.real(frf.ordinate[2, 0]) == pytest.approx(9.80665)


def test_a_unit_that_does_not_read_changes_nothing():
    frf = frf_without_units()
    project = project_with(FRF=frf)
    with pytest.raises(ProtocolError, match='not a unit') as error:
        units.define(project, 'FRF', [
            {'side': 'response', 'key': '1Z+|acceleration', 'unit': 'g'},
            {'side': 'reference', 'key': '1Z+|force', 'unit': 'blorps'}])
    assert error.value.code == 'refused'
    assert frf.undefined_records == [0, 1, 2, 3]


def test_a_row_the_object_does_not_have_is_a_bad_request():
    project = project_with(FRF=frf_without_units())
    with pytest.raises(ProtocolError, match='no reference row') as error:
        units.define(project, 'FRF', [{'side': 'reference', 'key': '9Z+|force', 'unit': 'N'}])
    assert error.value.code == 'bad_request'


def test_a_time_history_names_each_channel_once():
    history = TimeHistory(np.arange(4.0), np.ones((3, 4)),
                          response_dof=['1Z+', '1Z+', '2Z+'], ordinate_dim='unknown',
                          dimension_hint=['acceleration', 'force', 'acceleration'])
    project = project_with(Run=history)
    (table,) = units.tables(project, 'Run')['tables']
    assert table['side'] == 'channels'
    assert [r['key'] for r in table['rows']] == ['1Z+|acceleration', '1Z+|force',
                                                 '2Z+|acceleration']
    assert units.define(project, 'Run', [
        {'side': 'channels', 'key': '1Z+|force', 'unit': 'lbf'}]) == [1]
    assert history.ordinate_dim[1] == 'force' and history.undefined_records == [0, 2]


def test_a_geometry_declares_its_length_over_the_socket(validate):
    responses = asyncio.run(_exchange([
        ('project.import', {'path': str(PLATE / 'geometry.unv')}),
        ('project.units', {'name': 'Geometry'}),
        ('project.define_units', {'name': 'Geometry',
                                  'units': [{'side': 'object', 'key': 'length', 'unit': 'mm'}]}),
        ('project.units', {'name': 'Geometry'}),
        ('project.define_units', {'name': 'Geometry', 'units': []}),
    ]))
    asked, defined, again, empty = (got[-1] for got in responses[1:])
    validate(asked['result'], 'projectUnitsResult')
    assert asked['result']['tables'][0]['rows'][0]['unit'] is None
    validate(defined['result'], 'projectDefineUnitsResult')
    (geometry,) = defined['result']['project']['objects']
    assert geometry['units_defined'] and defined['result']['project']['modified']
    assert again['result']['tables'][0]['rows'][0]['unit'] == 'mm'
    assert empty['error']['code'] == 'bad_request'


def test_units_need_an_open_project():
    with pytest.raises(ProtocolError, match='none is open'):
        projects.units(None, {'name': 'FRF'})


# ---- declared one table at a time ------------------------------------------------

def plain_frf():
    return Frf(np.arange(1.0, 9.0), np.full((1, 8), 2.0 + 0j), ['1X+'], ['9X+'])


def plain_cpsd():
    return hz.Psd(np.arange(1.0, 9.0), np.ones((4, 8)) + 0j, ['1X+', '2X+', '1X+', '2X+'],
                  ['1X+', '2X+', '2X+', '1X+'])


def row_of(project, name, side, label):
    table = next(t for t in units.tables(project, name)['tables'] if t['side'] == side)
    return next(row for row in table['rows'] if row['label'] == label)


def test_a_half_named_frf_converts_when_the_other_table_is_named():
    project = project_with(f=plain_frf())
    assert units.define(project, 'f', [{'side': 'response', 'key': '1X+|', 'unit': 'g'}]) == []
    assert project['f'].ordinate_dim == ['unknown']
    assert row_of(project, 'f', 'response', '1X+')['unit'] == 'g', 'the row keeps what was named'
    assert units.define(project, 'f', [{'side': 'reference', 'key': '9X+|', 'unit': 'lbf'}]) == [0]
    assert project['f'].ordinate_dim == ['acceleration/force']
    assert project['f'].ordinate[0, 0] == pytest.approx(2.0 * 9.80665 / 4.4482216152605)
    assert project['f'].pending_units is None


def test_a_cross_term_converts_when_its_second_channel_is_named():
    project = project_with(c=plain_cpsd())
    assert units.define(project, 'c', [{'side': 'channels', 'key': '1X+|', 'unit': 'g'}]) == [0]
    second = row_of(project, 'c', 'channels', '2X+')
    converted = units.define(project, 'c', [{'side': 'channels', 'key': second['key'],
                                             'unit': 'lbf'}])
    assert converted == [1, 2, 3]
    assert 'unknown' not in project['c'].ordinate_dim


def test_a_channel_is_one_row_however_many_of_its_records_converted():
    project = project_with(c=plain_cpsd())
    units.define(project, 'c', [{'side': 'channels', 'key': '1X+|', 'unit': 'g'}])
    rows = units.tables(project, 'c')['tables'][0]['rows']
    assert [row['label'] for row in rows] == ['1X+', '2X+']
    assert rows[0]['unit'] == 'g'


def test_a_load_cell_and_an_accelerometer_at_one_dof_stay_two_rows():
    project = project_with(f=frf_without_units())
    rows = {row['key'] for table in units.tables(project, 'f')['tables']
            for row in table['rows']}
    assert '1Z+|acceleration' in rows and '1Z+|force' in rows


def test_a_unit_that_waits_is_not_saved(tmp_path):
    project = project_with(f=plain_frf())
    units.define(project, 'f', [{'side': 'response', 'key': '1X+|', 'unit': 'g'}])
    project.save(tmp_path / 'p.hforge')
    assert hz.Project.open(tmp_path / 'p.hforge')['f'].pending_units is None
