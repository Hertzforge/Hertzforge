"""Visual Dynamics' spectral answers, checked against sdynpy's — as data.

sdynpy is the tool this package's audience already trusts, and this
package will be scrutinized precisely because it was built with AI
assistance, so agreement is worth demonstrating (Brandon, 2026-08-28).
What must never happen is an import: sdynpy is GPL-3.0, and one import
from anything distributed with Visual Dynamics would settle its
licensing permanently. So the comparison arrives the way the ADF
corpus's truth files do — sdynpy runs privately in the generators
repository, and its *numbers* are frozen into
`testdata/sdynpy_oracle/oracle.npz` beside the exact input signals.
Running a GPL program privately carries no conditions, and a program's
numerical output is not a covered work; nothing here imports sdynpy,
and `test_license_boundary` now checks the tests as well as the
package.

**An agreement oracle, not a truth oracle.** sdynpy has around 13% test
coverage, so when the two disagree the discrepancy is investigated from
first principles — Parseval, closed forms, the scipy cross-checks
already in this suite — and the verdict is recorded, whichever way it
falls. Nothing here may be "fixed" toward sdynpy on disagreement alone.

What building the oracle actually found, for the record:

* **Agreement at machine precision** everywhere the conventions
  coincide: PSD 4e-16, FRF H1 2e-15, Hv 1e-14, H2 5e-16, coherence
  1e-15, SRS 9e-15, RMS 6e-16 (worst relative errors, 2026-08-28). The
  framings provably match — same frames, same periodic windows, same
  Welch scaling — so these are two implementations of the same
  definitions, not merely similar answers.
* **CPSD is conjugated between the tools.** Visual Dynamics computes
  Bendat & Piersol's `Gxy = conj(X)·Y`; sdynpy computes `X·conj(Y)`.
  Both are self-consistent conventions with opposite phase sign; the
  comparison conjugates and says so, and neither side is wrong.
* **H2 was the one deliberate divergence, and Brandon closed it.**
  sdynpy computes the classical coupled square form, Gxx * Gfx^-1
  (Rocklin, Crowley and Vold, 1985); this package refused it for a
  day, because the coupling makes a channel's H2 depend on which
  sibling channels are in the set — measured at 0.2% on these very
  signals, where H1 is bit-identical under the same swap. Brandon
  chose compatibility with the tool his audience trusts (2026-08-28),
  so the square form is computed, matched below at 4e-15, and the
  coupling is documented in compute_frfs instead of avoided.
  Single-reference H2 keeps the uncoupled per-response reading both
  tools share, and non-square multi-reference stays refused by both.
* **Multiple coherence is checked against sdynpy's own computation**,
  which lives inline in its SignalProcessingGUI rather than in a
  callable function; the generator drives that GUI off-screen rather
  than copying the block out. Agreement at 1e-15, both references at
  once.
* And the first "discrepancy" was the harness: sdynpy's `srs` returns
  a `(values, frequencies)` tuple, the first oracle draft stored the
  tuple whole, and the comparison read the frequency grid as an SRS
  100% wrong. Worth remembering before blaming either tool.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path

#: observed worst-case relative error is 1e-14; a hundred times that
#: still fails on any real convention difference, whose scale is
#: percent, not parts per trillion
TOLERANCE = 1e-12


@pytest.fixture(scope='module')
def oracle():
    return np.load(fixture_path('sdynpy_oracle', 'oracle.npz'))


def names_of(oracle):
    return [str(name) for name in oracle['signal_names']]


def test_the_srs_agrees_with_sdynpy(oracle):
    """Both are Smallwood's ramp-invariant recursion at Q of 10,
    maximax absolute acceleration; sdynpy runs it through lfilter where
    this package wrote the recursion out, and the two ways of applying
    the same coefficients agree to 9e-15."""
    from hertzforge.core.srs import maximax

    ours = maximax(oracle['transient'], oracle['srs_frequencies'],
                   float(oracle['sample_rate']))
    theirs = np.asarray(oracle['srs'])
    assert theirs.ndim == 1, 'the oracle stored values, not the tuple'
    assert np.max(np.abs(ours - theirs) / theirs) <= TOLERANCE


def test_the_oracle_says_where_it_came_from(oracle):
    """The provenance rides in the file, so the numbers can be traced
    to the sdynpy version and script that made them without leaving
    this repository."""
    import json

    record = json.loads(str(oracle['provenance']))
    assert record['sdynpy'], 'the version is named'
    assert 'never imported' in record['note']


def test_mac_agrees_with_sdynpy(oracle):
    """One formula, but both sides conjugate somewhere, and agreeing on
    random *complex* shapes proves the conjugates sit in the right
    places — real shapes would pass with them wrong. sdynpy lays
    shapes (dof, mode); this package (mode, dof); the transpose is the
    whole translation."""
    from hertzforge.core.shapes import mac_matrix

    ours = mac_matrix(oracle['mac_shapes'].T)
    assert np.max(np.abs(ours - oracle['mac'])) < TOLERANCE


def test_the_cmif_agrees_with_sdynpy(oracle):
    """The singular values of the measured FRF matrix, per line — the
    curve Find Mode hunts — against sdynpy's compute_cmif on the same
    matrix.

    Compared in the coherent SI system, deliberately: cmif_curves
    serves the display and converts to the display units, and the
    first comparison came back off by exactly 0.45359237 — a
    kilogram's worth of pounds — because the default display is
    in-slinch-lbf-s (g) and the oracle's matrix is unitless SI. The
    conversion is correct behavior, not error, and naming the system
    is what makes the comparison about the SVD."""
    from hertzforge.drawing.plot import cmif_curves
    from hertzforge.units import SI

    H = oracle['frf_H1']
    rows, response, reference = [], [], []
    for i, r in enumerate(names_of(oracle)[:3]):
        for j, f in enumerate(names_of(oracle)[3:]):
            rows.append(H[:, i, j])
            response.append(r)
            reference.append(f)
    from hertzforge.core.data import Frf

    built = Frf(oracle['frf_frequencies'], np.asarray(rows),
                response_dof=response, reference_dof=reference,
                ordinate_dim=['acceleration/force'] * len(rows))
    values, _x = cmif_curves(built, unit_system=SI)
    theirs = oracle['cmif']
    assert values.shape == theirs.shape
    assert np.max(np.abs(values - theirs)) <= TOLERANCE * theirs.max()


def test_the_srs_family_agrees(oracle):
    """Beyond one type at one Q: the positive and negative peaks
    against sdynpy's spectrum types 7 and 8 (the signed maxima), and
    the maximax again at Q of 50, where the wavelets are ten times
    narrower and a coefficient error would scale differently."""
    from hertzforge.core.srs import maximax, peaks

    fs = float(oracle['sample_rate'])
    frequencies = oracle['srs_frequencies']
    highest, lowest = peaks(oracle['transient'], frequencies, fs)
    type7 = oracle['srs_types'][6]
    type8 = np.abs(oracle['srs_types'][7])
    assert np.max(np.abs(highest - type7) / type7) <= TOLERANCE
    assert np.max(np.abs(-lowest - type8) / type8) <= TOLERANCE

    at_q50 = maximax(oracle['transient'], frequencies, fs, q=50.0)
    assert np.max(np.abs(at_q50 - oracle['srs_q50'])
                  / oracle['srs_q50']) <= TOLERANCE
