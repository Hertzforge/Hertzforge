"""A channel is one (DOF, quantity) however it is reached: strain and
dimensionless name one channel, a typed row answers to its type, and a
coordinate correction carries the channel's role."""

import numpy as np
import pytest

import hertzforge as hz
from hertzforge.core.channel_table import ChannelTable


def strain_table():
    return ChannelTable({'channel': [1, 2, 3], 'node': ['1', '2', '1'],
                         'direction': ['X+', 'X+', 'X+'],
                         'unit': ['microstrain', 'g', 'lbf'],
                         'channel_type': ['strain', 'acceleration', 'force']})


def strain_history():
    history = hz.TimeHistory(np.arange(16) / 16.0, np.ones((3, 16)),
                             ['1X+', '2X+', '1X+'])
    history.define_units(['microstrain', 'g', 'lbf'])
    return history


def test_a_strain_row_names_the_history_channel():
    assert strain_table().identities(strain_history()) == [
        ('1X+', 'dimensionless'), ('2X+', 'acceleration'), ('1X+', 'force')]


@pytest.mark.parametrize('quantity', ['strain', 'dimensionless'])
def test_a_strain_row_renames_by_either_word(quantity):
    table = strain_table()
    assert table.rename_dof('1X+', '9X+', quantity=quantity) == 1
    assert table.dof_strings() == ['9X+', '2X+', '1X+']


def test_a_typed_row_with_no_unit_renames_by_its_type():
    table = ChannelTable({'channel': [1], 'node': ['1'], 'direction': ['X+'],
                          'unit': [''], 'channel_type': ['acceleration']})
    assert table.rename_dof('1X+', '9X+', quantity='acceleration') == 1


def test_a_row_with_no_type_still_renames_by_its_unit():
    table = ChannelTable({'channel': [1], 'node': ['1'], 'direction': ['X+'],
                          'unit': ['lbf']})
    assert table.rename_dof('1X+', '9X+', quantity='force') == 1
    with pytest.raises(ValueError, match='no acceleration channel'):
        table.rename_dof('9X+', '8X+', quantity='acceleration')


# ---- the role moves with the channel -----------------------------------------

def voltage_run():
    rng = np.random.default_rng(0)
    history = hz.TimeHistory(np.arange(8192) / 1024.0, rng.normal(size=(2, 8192)),
                             ['1Z+', '2Z+'])
    history.define_units(['V', 'g'])
    project = hz.Project('p')
    project.add('h', history)
    project.set_channel_role('h', '1Z+', 'voltage', 'reference')
    return project


def test_a_correction_keeps_the_stated_role():
    project = voltage_run()
    project['h'].rename_dof('1Z+', '5Z+', 'voltage')
    assert project['h'].channel_roles()[('5Z+', 'voltage')] == 'reference'
    assert ('1Z+', 'voltage') not in project['h'].roles


def test_an_frf_refreshes_after_its_reference_was_corrected():
    project = voltage_run()
    frf = project.compute_frfs('h')
    project.rename_dof('h', '1Z+', '5Z+', 'voltage')
    assert project.stale() == {}, 'a corrected coordinate changes no number'
    project.refresh(frf)
    assert set(project[frf].reference_dof) == {'5Z+'}


def test_correcting_another_quantity_leaves_the_role_alone():
    project = voltage_run()
    project['h'].rename_dof('2Z+', '7Z+')
    assert project['h'].channel_roles()[('1Z+', 'voltage')] == 'reference'
