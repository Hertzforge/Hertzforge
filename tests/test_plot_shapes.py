"""Each curve drawn the way its own points mean their area.

A PSD line is a density over its bin, so it is flat across the bin and
the area drawn is the area that gets summed. A specification's points
are breakpoints of a power law, so the curve between two of them is
that law and not the straight line a polyline would take. Anything that
is a value at a frequency rather than an area — an FRF, a coherence, a
phase — is drawn as the line it is.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.data import Psd, Specification

_ALIVE = []


def a_psd(lines=None, level=1e-3):
    lines = np.linspace(10.0, 100.0, 10) if lines is None else lines
    return Psd(abscissa=np.asarray(lines, dtype=float),
               ordinate=np.full((1, len(lines)), float(level)),
               response_dof=['101Z+'],
               ordinate_dim=['acceleration**2/frequency'])


def _report_block(source, **extra):
    """The plot block the report would build for this object."""
    from hertzforge.report import _build_block
    from hertzforge.units import DEFAULT_SYSTEM

    block = {'kind': 'plot', 'source': 'X', 'caption': '', **extra}
    return _build_block(block, {'X': source}, DEFAULT_SYSTEM)


def test_the_report_asks_the_same_shape_question_as_the_plot():
    """One rule, three readings: the flat plot, the 3-D waterfall and
    the exported figure all draw by `drawing_shape`. The report used to
    decide for itself — `interpolation == 'bin'` and nothing else — so
    a specification exported as straight segments where the app drew
    the power law."""
    from hertzforge.drawing.plot import drawing_shape

    psd = Psd(np.linspace(0.0, 100.0, 33), np.ones((1, 33)),
              response_dof=['1Z+'], ordinate_dim=['acceleration**2/frequency'],
              ordinate_unit=['(m/s**2)**2/Hz'])
    assert drawing_shape(psd) == 'steps'
    assert _report_block(psd).get('steps') is True

    spec = Specification(np.array([10.0, 100.0]), np.array([[1e-4, 1e-2]]),
                         response_dof=['1Z+'],
                         ordinate_dim=['acceleration**2/frequency'],
                         ordinate_unit=['(m/s**2)**2/Hz'])
    assert drawing_shape(spec) == 'law'
    assert not _report_block(spec).get('steps'), \
        'breakpoints are a power law, never a staircase'


def test_the_reports_specification_follows_the_power_law():
    """The frequency axis is linear in the report as in the app, so a
    straight segment between breakpoints runs ~5 dB off mid-decade."""
    spec = Specification(np.array([10.0, 100.0]), np.array([[1e-4, 1e-2]]),
                         response_dof=['1Z+'],
                         ordinate_dim=['acceleration**2/frequency'],
                         ordinate_unit=['(m/s**2)**2/Hz'])
    built = _report_block(spec)
    assert built['logy'] is True
    curve = built['curves'][0]
    assert curve['x'] is not None and len(curve['x']) > 2, \
        'filled in, not two breakpoints joined straight'
    # Halfway along the segment in *frequency*, the drawn value must
    # sit on the law rather than on the chord. Both readings are
    # derived from the drawn endpoints, because the report converts to
    # the display system — asserting SI numbers here was a bad test,
    # and it failed for that rather than for the code.
    xs = np.asarray(curve['x'], dtype=float)
    ys = np.asarray([np.nan if v is None else v for v in curve['y']],
                    dtype=float)
    f0, f1 = xs[0], xs[-1]
    l0, l1 = ys[0], ys[-1]          # already log10 — logy is on
    at = int(np.argmin(np.abs(xs - 0.5 * (f0 + f1))))
    f = xs[at]
    law = l0 + (l1 - l0) * np.log(f / f0) / np.log(f1 / f0)
    chord = l0 + (l1 - l0) * (f - f0) / (f1 - f0)
    assert abs(law - chord) > 0.2, 'the two readings must differ here'
    assert abs(ys[at] - law) < abs(ys[at] - chord), \
        'the drawn point sits on the law, not on the chord'


def test_the_report_steps_on_the_bands_own_edges():
    """An octave band's center is the geometric mean of its edges, so
    midpoints between centers — which the JS used to guess — miss them.
    The payload carries the real edges now."""
    from hertzforge.core.octave import bin_bounds

    centers = np.array([100.0, 125.0, 160.0])
    widths = np.array([23.0, 29.0, 36.0])
    banded = Psd(centers, np.ones((1, 3)), response_dof=['1Z+'],
                 ordinate_dim=['acceleration**2/frequency'],
                 ordinate_unit=['(m/s**2)**2/Hz'], bandwidth=widths)
    built = _report_block(banded)
    assert built.get('steps') is True
    lower, upper = bin_bounds(centers, widths)
    expected = np.concatenate([lower, [upper[-1]]])
    # in decades: an octave-band figure reads on a log axis (2026-09-20)
    assert built.get('logx') is True
    assert np.allclose(built['edges'], np.log10(expected), atol=1e-6), \
        'the bands, not the midpoints between their centers'
