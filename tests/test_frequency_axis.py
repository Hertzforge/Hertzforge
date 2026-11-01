"""The frequency axis is the viewer's: decades or hertz, one switch.

By convention an SRS reads in decades and everything else in hertz;
some readers of a specification want the other (Brandon, 2026-09-05).
*Log f* on the plot bar switches every frequency plot, the stage and
the report's figures at once, through `core.data.frequency_axis`.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.data import Psd, Srs, TimeHistory, frequency_axis


@pytest.fixture(autouse=True)
def default_axis():
    frequency_axis('default')
    yield
    frequency_axis('default')


def _psd():
    f = np.arange(0.0, 2050.0, 2.0)
    return Psd(f, np.ones((2, len(f))), response_dof=['101Z+', '104Z+'],
               reference_dof=['101Z+', '104Z+'], ordinate_dim='acceleration**2/frequency')


def test_the_choice_overrides_the_convention_for_frequency_data_only():
    assert frequency_axis() == 'default'
    assert Srs.log_abscissa and not Psd.log_abscissa
    assert frequency_axis('log') == 'log'
    assert Psd.log_abscissa and Srs.log_abscissa
    assert not TimeHistory.log_abscissa, 'time is never in decades'
    assert _psd().log_abscissa, 'on the instance as on the class'
    assert frequency_axis('linear') == 'linear'
    assert not Srs.log_abscissa, "the viewer's choice beats the convention"
    assert frequency_axis(True) == 'log' and frequency_axis(False) == 'linear'
    assert frequency_axis(None) == 'default'
    assert Srs.log_abscissa and not Psd.log_abscissa
    with pytest.raises(ValueError, match="'log', 'linear' or 'default'"):
        frequency_axis('decades')
    assert hertzforge.frequency_axis is frequency_axis


def test_the_report_figure_follows_the_switch(tmp_path):
    from hertzforge.core.report import Report
    from hertzforge.report import render_html

    project = hertzforge.Project('t')
    project.add('PSD', _psd())
    report = Report()
    report.blocks.append({'kind': 'plot', 'source': 'PSD'})
    linear = render_html(report, project, hertzforge.SI)
    frequency_axis('log')
    decades = render_html(report, project, hertzforge.SI)
    assert '"logx": false' in linear and '"logx": true' in decades


def test_steps_and_power_laws_are_built_in_hertz_then_put_in_decades():
    """The bug the toggle exposed: the stage and the report figure took
    log10 of the lines first and then worked bin edges and power laws
    out of the exponents, so an octave PSD's steps and a specification's
    law landed in the wrong places under a log axis."""
    from hertzforge.core.author import SpecificationDraft
    from hertzforge.drawing.plot import as_power_law, step_outline
    from hertzforge.drawing.viz.waterfall import waterfall_arrays
    from hertzforge.report import _build_block

    banded = _psd().to_octave(3)
    spec = SpecificationDraft.at_dofs(['1Z+']).with_all_pairs(0.0).make()
    frequency_axis('log')
    us = hertzforge.SI
    # the stage: a banded PSD's steps
    # the object's own drawn edges: an end band is drawn over the part
    # its source covered (2026-09-19)
    real_x, _values = step_outline(banded.abscissa, banded.ordinate,
                                   banded.bin_edges())
    cx, _cz = waterfall_arrays(banded, unit_system=us)['curves'][0]
    assert np.allclose(cx, np.log10(real_x)), \
        'edges from hertz, then decades — not edges from exponents'
    assert cx[0] == pytest.approx(np.log10(banded.bin_edges()[0]))
    # the stage: a specification's power law between two breakpoints
    law_x, _law = as_power_law(spec.abscissa, np.real(spec.ordinate[0]))
    assert len(law_x) > 2, 'filled in between 20 and 2000 Hz'
    cx, _cz = waterfall_arrays(spec, unit_system=us)['curves'][0]
    assert np.allclose(cx, np.log10(law_x))
    # the report's figure, both shapes
    built = _build_block({'kind': 'plot', 'source': 'Banded',
                          'mode': 'curves', 'caption': 'c'},
                         {'Banded': banded}, us, [])
    assert built['logx'] and built['steps']
    assert np.allclose(built['x'], np.log10(banded.abscissa))
    assert np.allclose(built['edges'], np.log10(banded.bin_edges()))
    built = _build_block({'kind': 'plot', 'source': 'Spec',
                          'mode': 'curves', 'caption': 'c'},
                         {'Spec': spec}, us, [])
    assert built['logx']
    # drawn on the block's grid, which its zones share (2026-10-24)
    drawn = built['curves'][0]['x'] or built['x']
    assert np.allclose(drawn, np.log10(law_x))
    # and in hertz everything is where it was
    frequency_axis('linear')
    cx, _cz = waterfall_arrays(banded, unit_system=us)['curves'][0]
    assert np.allclose(cx, real_x)
