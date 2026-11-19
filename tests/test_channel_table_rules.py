"""The channel table's refusals and edits, pinned by value where a
mutation pass found them unguarded by the schema tests (2026-10-11)."""

from __future__ import annotations

import pytest

from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.unit_choices import (
    ALL_ORDINATE_UNITS,
    ORDINATE_UNITS,
    base_dimension,
    engineering_unit,
    squared_per_hz,
    units_for,
)


def _table():
    return ChannelTable({'channel': [1, 2, 3], 'node': [101, 102, 103],
                         'direction': ['Z+', 'Z+', 'X+'], 'unit': ['g', 'g', 'lbf']})


def test_a_delete_names_rows_that_exist_and_leaves_one():
    table = _table()
    with pytest.raises(IndexError, match=r'no such channels: \[3\]'):
        table.delete_channels([3])
    with pytest.raises(IndexError, match=r'no such channels: \[-1\]'):
        table.delete_channels([-1])
    with pytest.raises(ValueError, match='cannot delete every channel'):
        table.delete_channels([0, 1, 2])
    table.delete_channels([0])
    assert table.dof_strings() == ['102Z+', '103X+']


def test_columns_of_different_lengths_name_the_odd_one():
    with pytest.raises(ValueError, match="column 'node' has 2 entries, expected 3"):
        ChannelTable({'channel': [1, 2, 3], 'node': [101, 102],
                      'direction': ['Z+', 'Z+', 'X+']})


def test_a_sensitivity_is_not_zero_on_an_enabled_input():
    # the one rule set (schema/PROTOCOL.md, "Channel Table"): a table with no
    # controller columns has every row on and none a drive
    table = _table()
    table.set_cell('sensitivity', 0, '0.001')
    with pytest.raises(ValueError, match='sensitivity cannot be 0 on an enabled input'):
        table.set_cell('sensitivity', 0, '0')
    table.add_controller_settings()
    table.set_cell('enabled', 0, 'False')
    table.set_cell('sensitivity', 0, '0')
    assert table['sensitivity'][0] == '0'


def test_a_sensitivity_is_never_negative_on_an_enabled_row():
    # decided by the project's owner: polarity is the row's own column, so a
    # negative sensitivity on an enabled row is refused, naming the row, the
    # column and the value; a disabled row holds any number, as it holds 0.
    # Falsified by letting a negative through (`< 0.0` read as `== 0.0`):
    # set_cell takes -100 and rule_errors finds nothing.
    table = _table()
    with pytest.raises(ValueError, match=r'channel 1, Sensitivity: -100(\.0)? is negative'):
        table.set_cell('sensitivity', 0, '-100')
    table.add_controller_settings()
    table.set_cell('drive', 1, 'True')
    with pytest.raises(ValueError, match='Polarity column'):
        table.set_cell('sensitivity', 1, '-1000')
    table.set_cell('enabled', 0, 'False')
    table.set_cell('sensitivity', 0, '-100')
    assert float(table['sensitivity'][0]) == -100.0
    assert table.rule_errors() == []
    table.set_cell('enabled', 0, 'True')
    assert table.rule_errors() == [(
        'channel 1, Sensitivity: -100 is negative; a sensitivity is never negative on an '
        'enabled row, and a channel read inverted says so in its Polarity column')]


def test_a_new_type_withdraws_a_unit_of_another_kind_and_keeps_its_own():
    table = _table()
    table.set_cell('channel_type', 2, 'acceleration')
    assert table.frame['unit'][2] == '', 'lbf is not an acceleration'
    table.set_cell('channel_type', 0, 'acceleration')
    assert table.frame['unit'][0] == 'g'


def test_the_quantity_a_unit_is_picked_for_is_the_leading_term():
    assert base_dimension('acceleration**2/frequency') == 'acceleration'
    assert base_dimension('velocity/force') == 'velocity'
    assert units_for('acceleration/force') == ORDINATE_UNITS['acceleration']
    assert units_for(None) == ALL_ORDINATE_UNITS
    assert units_for('unheard of', ['x']) == ['x']


@pytest.mark.parametrize('unit, label', [
    ('g', 'g²/Hz'), ('m/s**2', '(m/s²)²/Hz'), ('', ''), (None, '')])
def test_a_psd_unit_reads_as_its_square_per_hertz_and_back(unit, label):
    assert squared_per_hz(unit) == label
    if unit:
        assert engineering_unit(label) == unit
        assert engineering_unit(unit) == unit, 'the suffix is optional'
