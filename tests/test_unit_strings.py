"""Unit strings a controller's table or a person types, read as what they
mean: `Gs` is no gigasecond, a bare `G` is gravity inside a compound too, an
rpm is a rotation, a decibel is not a scale, and one temperature channel does
not refuse the run around it."""

import shutil

import netCDF4
import numpy as np
import pytest
from conftest import fixture_path

import hertzforge as hz
from hertzforge.service import projects
from hertzforge.service.protocol import ProtocolError
from hertzforge.service.units import define
from hertzforge.units import (
    UnitError,
    dimension_of,
    fold_unit_case,
    si_factor,
    si_transform,
)

GN = 9.80665
LBF = 4.4482216152605


@pytest.mark.parametrize('unit, factor', [
    ('G/lbf', GN / LBF), ('G/N', GN), ('G^2/Hz', GN ** 2), ('G**2/Hz', GN ** 2),
    ('G^2', GN ** 2), ('G', GN), ('G/LBF', GN / LBF)])
def test_a_bare_capital_g_is_gravity_inside_a_compound(unit, factor):
    assert si_factor(unit) == pytest.approx(factor)


@pytest.mark.parametrize('unit', ['mG', 'GHz', 'GPa', 'Gs', 'mV', 'MV'])
def test_a_prefixed_unit_stays_as_written(unit):
    assert fold_unit_case(unit) == unit


def test_a_string_nothing_makes_readable_is_returned_as_written():
    assert fold_unit_case('G/widgets') == 'G/widgets'


@pytest.mark.parametrize('unit', ['rpm', 'RPM', 'rad/s', 'deg/s'])
def test_a_rotation_is_an_angular_velocity(unit):
    assert dimension_of(unit) == 'angular_velocity'
    with pytest.raises(UnitError):
        si_transform(unit, 'frequency')


def test_sixty_rpm_is_one_turn_a_second():
    assert si_factor('rpm') * 60 == pytest.approx(2 * np.pi)


@pytest.mark.parametrize('unit', ['deg', 'rad'])
def test_an_angle_is_not_a_plain_number(unit):
    with pytest.raises(UnitError):
        si_transform(unit, 'dimensionless')


def test_a_frequency_is_not_an_angular_velocity():
    with pytest.raises(UnitError):
        si_transform('Hz', 'angular_velocity')


@pytest.mark.parametrize('unit', ['dB', 'dBm', 'Np'])
def test_a_logarithmic_unit_is_not_read(unit):
    assert dimension_of(unit) is None
    with pytest.raises(UnitError):
        si_transform(unit)


def test_the_units_that_worked_still_read():
    for unit, tag in [('g', 'acceleration'), ('Hz', 'frequency'), ('deg', 'angle'),
                      ('microstrain', 'dimensionless'), ('%', 'dimensionless'),
                      ('degC', 'temperature'), ('rad/s**2', 'angular_acceleration'),
                      ('lbf*in', 'moment')]:
        assert dimension_of(unit) == tag


# ---- a Rattlesnake channel table ---------------------------------------------

def variant(tmp_path, unit, row=0):
    path = tmp_path / 'run.nc4'
    shutil.copy(fixture_path('plate', 'shock.nc4'), path)
    with netCDF4.Dataset(path, 'a') as ds:
        ds['channels']['unit'][row] = unit
    return str(path)


def imported(path):
    return next(obj for obj in hz.import_file(path).values()
                if isinstance(obj, hz.TimeHistory))


@pytest.fixture
def base():
    return imported(fixture_path('plate', 'shock.nc4'))


def raw(history, row=0):
    return history.ordinate[row, 5] / si_factor(history.ordinate_unit[row])


@pytest.mark.parametrize('unit', ['Gs', 'dB'])
def test_a_channel_in_a_unit_no_channel_measures_imports_unit_less(tmp_path, base, unit):
    history = imported(variant(tmp_path, unit))
    assert history.ordinate_dim[0] == 'unknown'
    assert history.ordinate[0, 5] == pytest.approx(raw(base))
    assert history.ordinate_dim[1] == base.ordinate_dim[1]


def test_a_capital_g_squared_in_a_table_is_not_a_pressure(tmp_path):
    history = imported(variant(tmp_path, 'G^2'))
    assert history.ordinate_dim[0] != 'pressure'


def test_an_rpm_channel_imports_as_a_rotation(tmp_path, base):
    history = imported(variant(tmp_path, 'RPM'))
    assert history.ordinate_dim[0] == 'angular_velocity'
    assert history.ordinate[0, 5] == pytest.approx(raw(base) * 2 * np.pi / 60)


@pytest.mark.parametrize('unit, offset, scale', [
    ('degC', 273.15, 1.0), ('degF', 255.3722222, 5 / 9)])
def test_a_temperature_channel_converts_with_its_offset(tmp_path, base, unit, offset, scale):
    history = imported(variant(tmp_path, unit))
    assert history.ordinate_dim[0] == 'temperature'
    assert history.ordinate_unit[0] == unit
    assert history.ordinate[0, 5] == pytest.approx(raw(base) * scale + offset)
    assert history.ordinate[1, 5] == pytest.approx(base.ordinate[1, 5])


# ---- declared in the service --------------------------------------------------

def session_with(history):
    state = projects.new({'title': 'u'})
    state.project.add('th', history)
    return state.project


@pytest.mark.parametrize('unit', ['Gs', 'dB'])
def test_the_service_refuses_what_no_channel_measures(unit):
    project = session_with(hz.TimeHistory(np.arange(4) / 4.0, np.ones((1, 4)), ['1X+']))
    with pytest.raises(ProtocolError, match=unit):
        define(project, 'th', [{'side': 'channels', 'key': '1X+|', 'unit': unit}])
    assert project['th'].ordinate_dim == ['unknown']


def test_the_service_declares_a_rotation():
    project = session_with(hz.TimeHistory(np.arange(4) / 4.0, np.full((1, 4), 60.0), ['1X+']))
    define(project, 'th', [{'side': 'channels', 'key': '1X+|', 'unit': 'rpm'}])
    assert project['th'].ordinate_dim == ['angular_velocity']
    assert project['th'].ordinate[0, 0] == pytest.approx(2 * np.pi)
