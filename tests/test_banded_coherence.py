"""Coherence with bands, and the coherence of a CPSD.

A coherence computed from an octave-banded CPSD — |<S_pq>|² over
<S_pp><S_qq>, each band's averages — is a band's value, and needs the
bands the CPSD has. The band-average paper found `Coherence` had none:
`build_plots` stopped at `bin_bounds` (2026-09-26). The bins are now one
implementation shared with `Psd` (`data._Bands`), and `Psd.coherence`
computes the ordinary coherence of every cross term, banded or not.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core.data import Coherence, Psd

F = np.linspace(0.0, 2000.0, 2001)


def cpsd(pp=1.0, qq=4.0, pq=1.0 + 0.0j):
    """Two channels' CPSD with a flat coherence of |pq|² / (pp qq)."""
    ones = np.ones_like(F)
    return Psd(F, np.array([pp * ones, pq * ones, np.conj(pq) * ones,
                            qq * ones]),
               response_dof=['1X+', '1X+', '2X+', '2X+'],
               reference_dof=['1X+', '2X+', '1X+', '2X+'],
               ordinate_dim=['acceleration**2/frequency'] * 4)


def test_a_coherence_takes_bands_and_refuses_the_wrong_shape():
    ok = Coherence(F[:3], np.ones((1, 3)), response_dof=['1X+'],
                   reference_dof=['2X+'], bandwidth=[1.0, 2.0, 3.0])
    left, right = ok.bin_bounds()
    assert np.allclose(right - left, [1.0, 2.0, 3.0])
    with pytest.raises(ValueError, match='bandwidth has shape'):
        Coherence(F[:3], np.ones((1, 3)), response_dof=['1X+'],
                  reference_dof=['2X+'], bandwidth=[1.0, 2.0])
