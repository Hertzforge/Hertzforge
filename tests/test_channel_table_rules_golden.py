"""The channel table's one rule set, as the golden table file holds it
(`schema/golden/channel_table_rules.json`; schema/PROTOCOL.md, "Channel
Table"): what a table file holds and refuses, case by case, read here as the
control service and the shell's mock service read it in their own tests."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from hertzforge.io import tables

GOLDEN = Path(__file__).resolve().parents[1] / 'schema' / 'golden' / 'channel_table_rules.json'
RULES = json.loads(GOLDEN.read_text(encoding='utf-8'))


@pytest.mark.parametrize('case', RULES['cases'], ids=lambda case: case['name'])
def test_a_table_file_holds_or_refuses_each_case(case):
    if case['table_file'] == 'held':
        table = tables.channel_table_of_rows(case['rows'])
        assert table.num_channels == len(case['rows'])
    else:
        with pytest.raises(tables.TableContentError):
            tables.channel_table_of_rows(case['rows'])


def test_every_unit_of_the_vocabulary_is_held():
    base = RULES['cases'][0]['rows']
    for unit in RULES['unit_vocabulary']:
        rows = [*base, {**base[0], 'index': 9, 'role': 'monitor', 'units': unit}]
        table = tables.channel_table_of_rows(rows)
        assert list(table['unit'])[-1] == unit, unit


def test_the_rules_are_the_control_services_too():
    """What the control service answers is in the same file: a case it
    refuses is one a table file refuses, so a table that can be applied can
    be saved. (A table file refuses a unit no side reads, which the control
    service runs as written with a warning; the file says so.)"""
    for case in RULES['cases']:
        if case['control'] != 'ok':
            assert case['table_file'] == 'refused', case['name']
