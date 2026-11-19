"""A unit typed for the imported units: what it reads as."""

from __future__ import annotations

import pytest


def test_lbm_is_a_pound_mass():
    """pint refuses 'lbm' outright, which silently killed the shortlist
    entry; normalized, it is the pound."""
    from hertzforge.units import si_transform

    scale, offset = si_transform('lbm', 'mass')
    assert scale == pytest.approx(0.45359237)
    assert offset == 0.0
