"""Drawing several selections at once.

Each distinct (abscissa, quantity) gets its own plot, and there is one
budget of curves across all of them so a large selection stays interactive.
How that budget is divided is the whole of this file: it used to be first
come, first served, which left later plots empty.
"""

from __future__ import annotations

import pytest
from conftest import fixture_path

import hertzforge


@pytest.fixture
def both():
    return (hertzforge.import_file(fixture_path('plate', 'time.npz')),
            hertzforge.import_file(fixture_path('plate', 'spectrum.npz')))


@pytest.fixture
def spec():
    return hertzforge.import_file(
        fixture_path('plate', 'random.nc4'))['Random_specification']
