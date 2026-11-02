"""The imported-units pane, and the table conventions it shares.

Every table in hertzforge behaves the same way — see "How tables behave" in
PLAN.md — so most of what is checked here is checked through this pane but
is really a statement about all of them.
"""

from __future__ import annotations

import re

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge


@pytest.fixture
def time_data():
    return hertzforge.import_file(fixture_path('plate', 'time.npz'))


@pytest.fixture
def hinted_frf(tmp_path):
    """An FRF whose file names the quantity but has no 164 to size it."""
    source = fixture_path('plate', 'frfs.unv')
    with open(source) as f:
        text = f.read()
    path = tmp_path / 'no_164.unv'
    path.write_text(re.sub(r'    -1\n   164\n.*?    -1\n', '', text,
                           flags=re.DOTALL))
    return hertzforge.import_file(str(path))


@pytest.fixture
def psd():
    return hertzforge.import_file(fixture_path('plate', 'psd.npz'))


@pytest.fixture
def cpsd():
    """A 2×2 CPSD matrix: two channels, four records."""
    from hertzforge.core.data import Psd

    freq = np.linspace(0.0, 100.0, 11)
    ordinate = np.outer(np.arange(1.0, 5.0), np.ones(11)).astype(complex)
    return Psd(freq, ordinate,
               response_dof=['101X+', '101X+', '102X+', '102X+'],
               reference_dof=['101X+', '102X+', '101X+', '102X+'])


def test_lbm_is_a_pound_mass():
    """pint refuses 'lbm' outright, which silently killed the shortlist
    entry; normalized, it is the pound."""
    from hertzforge.units import si_transform

    scale, offset = si_transform('lbm', 'mass')
    assert scale == pytest.approx(0.45359237)
    assert offset == 0.0
