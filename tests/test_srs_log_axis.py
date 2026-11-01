"""An SRS reads in decades of natural frequency, on every surface.

Reviewer feedback (Brandon, 2026-08-25): SRS plots are conventionally
viewed on log frequency axes. One flag on the class —
`Srs.log_abscissa` — read by the 2-D plot, the 3-D stage and the
report, so the three renderers cannot disagree about the layout: the
same discipline as `log_scaled()` on the vertical axis.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core import srs as srs_module
from hertzforge.core.data import Psd, ShockSpecification, Srs, TimeHistory

_ALIVE: list = []


def _srs(channels=2):
    frequencies = srs_module.octave_frequencies(20.0, 2000.0)
    values = 10.0 * (frequencies / 20.0) ** 0.5
    return Srs(frequencies, np.tile(values, (channels, 1)),
               response_dof=[f'{101 + i}Z+' for i in range(channels)],
               ordinate_dim='acceleration', ordinate_unit='m/s**2')


def _specification():
    frequencies = srs_module.octave_frequencies(20.0, 2000.0)
    required = 10.0 * (frequencies / 20.0) ** 0.5
    return ShockSpecification(
        abscissa=frequencies, ordinate=np.atleast_2d(required),
        response_dof=['101Z+'], ordinate_dim=['acceleration'],
        abort_upper=np.atleast_2d(required * 2.0),
        abort_lower=np.atleast_2d(required * 0.5))


def test_the_srs_reads_in_decades_and_nothing_else_does():
    assert Srs.log_abscissa
    assert ShockSpecification.log_abscissa, \
        'a shock specification is an SRS in every axis respect'
    assert not Psd.log_abscissa
    assert not TimeHistory.log_abscissa


def test_the_stage_stands_in_log10_of_natural_frequency():
    from hertzforge.drawing.viz.waterfall import waterfall_arrays

    data = _srs()
    arrays = waterfall_arrays(data, unit_system=hertzforge.SI)
    assert arrays['log_abscissa']
    cx = arrays['curves'][0][0]
    assert cx[0] == pytest.approx(np.log10(20.0), abs=1e-6)
    assert cx[-1] == pytest.approx(np.log10(float(data.abscissa[-1])),
                                   abs=1e-6)
    assert arrays['xlabel'].startswith('log10 '), \
        "VTK prints the coordinates as they are, so the title says so"
    flat = waterfall_arrays(
        TimeHistory(np.arange(256) / 256.0, np.ones((1, 256)),
                    response_dof='101Z+', ordinate_dim='acceleration'),
        unit_system=hertzforge.SI)
    assert not flat['log_abscissa']


def test_every_report_figure_of_an_srs_reads_in_decades():
    from hertzforge.report import _build_block

    objects = {'SRS': _srs(1), 'Spec': _specification()}
    us = hertzforge.SI

    flat = _build_block({'kind': 'plot', 'source': 'SRS',
                         'mode': 'curves', 'caption': 'c'}, objects, us, [])
    assert flat['logx']
    assert flat['x'][0] == pytest.approx(np.log10(20.0), abs=1e-6)

    comparison = _build_block(
        {'kind': 'plot', 'source': 'SRS', 'mode': 'curves',
         'specification': 'Spec', 'caption': 'c'}, objects, us, [])
    assert comparison['logx']
    assert comparison['x'][0] == pytest.approx(np.log10(20.0), abs=1e-6)

    banded = _build_block({'kind': 'plot', 'source': 'Spec',
                           'mode': 'curves', 'caption': 'c'},
                          objects, us, [])
    assert banded['logx'], 'the requirement alone reads the same way'

    stage = _build_block({'kind': 'plot', 'source': 'SRS',
                          'mode': 'stage', 'caption': 'c'}, objects, us, [])
    stage = stage[0] if isinstance(stage, list) else stage
    assert stage['logx']
    assert not stage['xlabel'].startswith('log10'), \
        'the canvas prints real values at decade ticks, so no prefix'
    x0, _x1, _z0, _z1 = stage['extents']
    assert x0 == pytest.approx(np.log10(20.0), abs=1e-3)

    # and the page draws both x-tick sites through the flag — the same
    # counted-occurrence pin the z axis carries
    from hertzforge.report.page import _JS

    assert _JS.count('ticks(view.x0, view.x1, block.logx)') == 1
    assert _JS.count('ticks(x0, x1, block.logx)') == 1
    assert 'ticks(view.x0, view.x1, false)' not in _JS
    assert 'ticks(x0, x1, false)' not in _JS


def test_a_time_figure_stays_linear_in_x():
    from hertzforge.report import _build_block

    history = TimeHistory(np.arange(256) / 256.0, np.ones((1, 256)),
                          response_dof='101Z+',
                          ordinate_dim='acceleration')
    built = _build_block({'kind': 'plot', 'source': 'T',
                          'mode': 'curves', 'caption': 'c'},
                         {'T': history}, hertzforge.SI, [])
    assert not built['logx']
    assert built['x'][0] == pytest.approx(0.0)
