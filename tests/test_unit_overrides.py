import pytest

import hertzforge


def test_with_units_override():
    system = hertzforge.IN_LBF_S.with_units(acceleration='g')
    assert system.unit('acceleration') == 'g'
    assert system.unit('length') == 'in'  # everything else unchanged
    assert system.from_si(9.80665, 'acceleration') == pytest.approx(1.0)


def test_with_units_validates_dimension():
    with pytest.raises(hertzforge.units.UnitError, match='is not an acceleration unit'):
        hertzforge.SI.with_units(acceleration='in')  # not an acceleration unit


def test_with_units_does_not_mutate_original():
    hertzforge.SI.with_units(length='km')
    assert hertzforge.SI.unit('length') == 'm'


def test_with_units_name():
    assert hertzforge.SI.with_units('SI-g', acceleration='g').name == 'SI-g'
