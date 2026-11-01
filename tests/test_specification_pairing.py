"""A specification bounds autospectra; a CPSD carries cross terms too.

Drawn together, the 30 cross terms of a six-channel CPSD are 30 curves with no
limit anywhere near them. They are not wrong — they are just not the
comparison being asked for, and they bury the six that are.
"""

from __future__ import annotations

import pytest
from conftest import fixture_path

import hertzforge


@pytest.fixture(scope='module')
def spectra():
    return hertzforge.import_file(fixture_path('plate', 'random_spectra.nc4'))


def counts(series):
    return [len(records) if records is not None else data.num_records
            for _name, data, records in series]
