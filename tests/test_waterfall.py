"""The 3-D waterfall: the numbers, the mesh, and the size ceiling.

The scene claims live in `waterfall_arrays`' dict — log mapping, group
choice, labels — so nearly everything here holds numbers, not pixels.
The mesh tests pin the two decisions that carry large models: one
PolyData whatever the record count, and no record bringing more points
than the budget.
"""

import numpy as np
import pytest

from hertzforge.core.data import Coherence, Psd, TimeHistory
from hertzforge.decimate import peak_decimate
from hertzforge.drawing.viz.waterfall import (
    POINT_BUDGET,
    _finite_runs,
    waterfall_arrays,
    waterfall_group,
)
from hertzforge.units import DEFAULT_SYSTEM

PSD_DIM = 'acceleration**2/frequency'

PSD_UNIT = '(m/s**2)**2/Hz'


def psd(rows=4, samples=513):
    rng = np.random.default_rng(11)
    f = np.linspace(0.0, 2000.0, samples)
    values = 1e-4 * (1 + rng.random((rows, samples)))
    return Psd(f, values, response_dof=[f'{100 + i}Z+' for i in range(rows)],
               ordinate_dim=[PSD_DIM] * rows, ordinate_unit=[PSD_UNIT] * rows)


def test_peak_decimation_keeps_every_extreme():
    rng = np.random.default_rng(3)
    x = np.linspace(0.0, 10.0, 100_000)
    y = rng.standard_normal(100_000)
    y[31_337] = 40.0   # the one spike that is the reason to look
    y[77_777] = -40.0
    cx, cy = peak_decimate(x, y, 4096)
    assert len(cx) <= 4096
    assert cy.max() == 40.0 and cy.min() == -40.0
    assert np.all(np.diff(cx) >= 0), 'decimation must not reorder x'


def test_short_input_passes_through_untouched():
    x = np.arange(10.0)
    y = x ** 2
    cx, cy = peak_decimate(x, y, 4096)
    assert cx is x and cy is y


def test_a_slice_of_all_gap_keeps_the_gap():
    """nanargmin raises on an all-NaN slice; the gap must survive
    instead, so a specification's out-of-band NaN still reads as a
    hole after decimation."""
    x = np.arange(10_000.0)
    y = np.full(10_000, np.nan)
    y[:100] = 1.0
    cx, cy = peak_decimate(x, y, 100)
    assert len(cx) <= 100
    assert np.isnan(cy).any(), 'the gap thinned away entirely'


def test_the_object_answers_for_its_own_axis():
    assert psd().log_scaled() is True
    history = TimeHistory(np.linspace(0, 1, 64), np.ones((2, 64)),
                          response_dof=['1Z+', '2Z+'])
    assert history.log_scaled() is False
    coherence = Coherence(np.linspace(0, 100, 64), np.ones((1, 64)),
                          response_dof=['1Z+'], reference_dof=['2Z+'])
    assert coherence.log_scaled() is False, 'a 0..1 ratio never reads log'


def test_mixed_quantities_draw_the_largest_group():
    """One floor cannot hold accelerations and volts; the bigger group
    draws and the rest are named for the status bar."""
    f = np.linspace(0.0, 100.0, 65)
    data = Psd(f, np.ones((5, 65)),
               response_dof=[f'{i}Z+' for i in range(1, 6)],
               ordinate_dim=[PSD_DIM] * 3 + ['voltage**2/frequency'] * 2,
               ordinate_unit=[PSD_UNIT] * 3 + ['V**2/Hz'] * 2)
    drawn, left_out = waterfall_group(data)
    assert drawn == [0, 1, 2]
    assert left_out == [3, 4]


def test_a_record_pick_narrows_the_group():
    drawn, left_out = waterfall_group(psd(rows=6), records=[5, 1, 3])
    assert drawn == [5, 1, 3], 'the pick is taken as given, in its order'
    assert left_out == []


def test_log_data_maps_to_log10_exactly():
    data = psd(rows=2, samples=257)   # under budget: decimation is identity
    arrays = waterfall_arrays(data)
    assert arrays['log_scaled'] is True
    _cx, cz = arrays['curves'][0]
    # a PSD draws flat across each bin, so every value appears on both
    # of its bin's edges — the log mapping rides the step outline
    expected = np.repeat(
        np.log10(data.display_ordinate(DEFAULT_SYSTEM, [0])[0]), 2)
    assert np.allclose(cz, expected)


def test_zero_is_a_gap_not_a_value():
    """A log axis fed a zero once dragged a report 300 decades down;
    here it must become NaN and then a split line, never a point."""
    f = np.linspace(0.0, 100.0, 129)
    values = np.ones((1, 129))
    values[0, 40:60] = 0.0
    data = Psd(f, values, response_dof=['1Z+'],
               ordinate_dim=[PSD_DIM], ordinate_unit=[PSD_UNIT])
    arrays = waterfall_arrays(data)
    _cx, cz = arrays['curves'][0]
    assert np.isnan(cz[80:120]).all(), (
        'the gap rides the step outline, two points per bin')


def test_time_data_stays_linear():
    t = np.linspace(0.0, 1.0, 200)
    history = TimeHistory(t, np.vstack([np.sin(9 * t), np.cos(9 * t)]),
                          response_dof=['1Z+', '2Z+'],
                          ordinate_dim=['acceleration'] * 2,
                          ordinate_unit=['m/s**2'] * 2)
    arrays = waterfall_arrays(history)
    assert arrays['log_scaled'] is False
    assert arrays['xlabel'] == 'Time (s)'


def test_labels_are_the_record_labels():
    data = psd(rows=3)
    arrays = waterfall_arrays(data)
    assert arrays['labels'] == [data.record_label(i) for i in range(3)]


def test_axis_titles_are_ascii():
    """VTK's axis titles silently drop unicode superscripts —
    (in/s²)²/Hz read (in/s)/Hz, wrong by two squarings — so the titles
    must carry caret exponents that every font has."""
    from hertzforge.units import IN_LBF_S

    # the coherent system named explicitly: the default displays
    # acceleration in g, whose label has no superscript to convert,
    # and this test is about the conversion
    arrays = waterfall_arrays(psd(), unit_system=IN_LBF_S)
    assert arrays['zlabel'] == 'log10 (in/s^2)^2/Hz'
    assert arrays['xlabel'] == 'Frequency (Hz)'
    assert arrays['zlabel'].isascii() and arrays['xlabel'].isascii()


def test_finite_runs_split_on_gaps():
    z = np.array([1.0, 2.0, np.nan, np.nan, 3.0, 4.0, 5.0])
    assert _finite_runs(z) == [(0, 2), (4, 7)]


def test_a_lone_point_between_gaps_draws_nothing():
    z = np.array([np.nan, 1.0, np.nan])
    assert _finite_runs(z) == []


def test_no_finite_values_no_runs():
    assert _finite_runs(np.full(5, np.nan)) == []


def test_point_budget_is_the_module_default():
    """`POINT_BUDGET` is what the GUI will pass nothing to get; a
    change to it is a change to every scene and should trip a test."""
    assert POINT_BUDGET == 4096


def test_the_area_under_the_drawn_psd_is_its_own_integral():
    """The rule, stated as arithmetic: flat across each bin means the
    trapezoid under the trace on the stage equals sum(G * df) — the
    RMS-squared is literally the area drawn, in 3-D as in 2-D."""
    from hertzforge.drawing.plot import bin_edges

    data = psd(rows=1, samples=65)     # under budget: nothing decimated
    arrays = waterfall_arrays(data)
    cx, cz = arrays['curves'][0]
    drawn_area = np.trapezoid(10.0 ** cz, cx)
    values = data.display_ordinate(DEFAULT_SYSTEM, [0])[0]
    widths = np.diff(bin_edges(data.abscissa))
    assert np.isclose(drawn_area, float(np.sum(values * widths)),
                      rtol=1e-12), 'the picture and the integral disagree'


def test_an_octave_psd_steps_on_its_own_band_edges():
    """A banded spectrum carries its own bin widths, and the trace must
    land on those exact edges — midpoints between centers are wrong for
    bands whose centers are geometric means."""
    from hertzforge.core.octave import bin_bounds

    centers = np.array([10.0, 20.0, 40.0])
    widths = np.array([7.0, 14.0, 28.0])
    data = Psd(centers, np.ones((1, 3)), response_dof=['1Z+'],
               ordinate_dim=[PSD_DIM], ordinate_unit=[PSD_UNIT],
               bandwidth=widths)
    arrays = waterfall_arrays(data)
    cx, _cz = arrays['curves'][0]
    lower, upper = bin_bounds(centers, widths)
    expected = np.repeat(np.concatenate([lower, [upper[-1]]]), 2)[1:-1]
    assert np.allclose(cx, expected), 'the bands, not the midpoints'


def test_a_specification_follows_its_power_law():
    """log_log breakpoints mean the power law between them: halfway in
    log frequency the drawn value is the log-interpolated one, not the
    straight-line midpoint a polyline would take."""
    from hertzforge.core.data import Specification

    spec = Specification(np.array([10.0, 100.0]),
                         np.array([[1e-4, 1e-2]]),
                         response_dof=['1Z+'],
                         ordinate_dim=[PSD_DIM], ordinate_unit=[PSD_UNIT])
    assert spec.interpolation == 'log_log'
    arrays = waterfall_arrays(spec)
    cx, cz = arrays['curves'][0]
    assert len(cx) > 2, 'filled in, not two breakpoints joined straight'
    at = np.argmin(np.abs(cx - np.sqrt(10.0 * 100.0)))
    display = spec.display_ordinate(DEFAULT_SYSTEM, [0])[0]
    law_mid = np.sqrt(display[0] * display[1])
    assert np.isclose(10.0 ** cz[at], law_mid, rtol=1e-2), \
        'halfway in log frequency, the geometric mean of the breakpoints'


def test_a_signed_component_is_never_a_density():
    """The real part of a CPSD is signed; drawing it as steps would
    claim an area for a quantity that has none."""
    from hertzforge.drawing.plot import drawing_shape

    data = psd(rows=2, samples=65)
    assert drawing_shape(data, tagged=False) == 'steps'
    assert drawing_shape(data, tagged=True) == 'line'


def test_a_stepped_spectrum_keeps_every_tread_past_the_budget():
    """The bug Brandon found in transient.vdyn: the step outline is two
    points per bin, so it crosses the point budget at half the line
    count a plain curve would — and peak decimation keeps each slice's
    *extremes*, not its bin edges, so the treads were exactly what got
    thinned away. The staircase came back a polyline, and the area
    under it stopped being sum(G*df).

    A spectrum is thousands of lines where a record is hundreds of
    thousands, so a step curve is not decimated at all — the same call
    the 2-D plot makes, for the same stated reason.
    """
    from hertzforge.drawing.plot import bin_edges

    # comfortably past POINT_BUDGET once doubled into an outline
    lines = POINT_BUDGET // 2 + 500
    data = psd(rows=2, samples=lines)
    arrays = waterfall_arrays(data)
    cx, cz = arrays['curves'][0]
    assert len(cx) == 2 * lines, 'every bin keeps both of its edges'

    # and the rule the thinning had broken: the area drawn is the
    # area summed, exactly
    drawn = np.trapezoid(10.0 ** cz, cx)
    values = data.display_ordinate(DEFAULT_SYSTEM, [0])[0]
    widths = np.diff(bin_edges(data.display_abscissa(DEFAULT_SYSTEM)))
    assert np.isclose(drawn, float(np.sum(values * widths)), rtol=1e-12)


def test_a_long_record_is_still_decimated():
    """The budget is for time histories, which is what it was always
    for — dropping it for steps must not drop it for everything."""
    t = np.linspace(0.0, 10.0, 200_000)
    rng = np.random.default_rng(4)
    history = TimeHistory(t, rng.standard_normal((2, 200_000)),
                          response_dof=['1Z+', '2Z+'],
                          ordinate_dim=['acceleration'] * 2,
                          ordinate_unit=['m/s**2'] * 2)
    arrays = waterfall_arrays(history)
    cx, _cz = arrays['curves'][0]
    assert len(cx) <= POINT_BUDGET, 'a record still thins to the budget'


def test_a_scene_holds_what_it_can_draw_exactly_and_pages_the_rest():
    """The budget is spent as a record cap, never as more thinning:
    what is drawn is drawn exactly, and the overflow is a page away.
    A full CPSD from a 30-channel survey is 900 records — an ordinary
    modal survey, and 29.5M vertices at 16k lines."""
    from hertzforge.drawing.viz.waterfall import SCENE_BUDGET, records_per_page

    lines = 4001
    data = psd(rows=40, samples=lines)
    budget = 20 * 2 * lines            # room for exactly twenty records
    arrays = waterfall_arrays(data, scene_budget=budget)
    assert arrays['per_page'] == 20
    assert arrays['pages'] == 2
    assert arrays['page'] == 0
    assert len(arrays['drawn']) == 20
    assert arrays['of_quantity'] == 40
    # and every record that *is* drawn keeps every tread
    cx, _cz = arrays['curves'][0]
    assert len(cx) == 2 * lines

    later = waterfall_arrays(data, scene_budget=budget, page=1)
    assert later['page'] == 1
    assert later['drawn'] == arrays['drawn'][:0] + list(range(20, 40))
    assert not set(later['drawn']) & set(arrays['drawn']), 'no overlap'

    # the real default holds a 900-record CPSD in three pages
    assert records_per_page(data, 900, 16385, 'steps', SCENE_BUDGET) == 305


def test_a_page_past_the_end_lands_on_the_last_one():
    data = psd(rows=10, samples=1001)
    budget = 3 * 2 * 1001
    arrays = waterfall_arrays(data, scene_budget=budget, page=99)
    assert arrays['page'] == arrays['pages'] - 1
    assert arrays['drawn'], 'a clamped page still draws'


def test_one_record_draws_even_when_it_alone_is_over_budget():
    """A page showing nothing is not a page."""
    from hertzforge.drawing.viz.waterfall import records_per_page

    assert records_per_page(None, 5, 1_000_000, 'steps', budget=10) == 1


def test_a_small_object_has_one_page_and_says_so():
    arrays = waterfall_arrays(psd(rows=4))
    assert arrays['pages'] == 1 and arrays['page'] == 0


def test_nice_axis_widens_only_when_nearly_free():
    """A 0.4998 s record's five even divisions put ticks at 0.125 and
    0.375, and the one-decimal default *printed* them as 0.1 and 0.4 —
    numbers that do not sit where they claim. Brandon read a 0.5 s
    record as 0.6 s against the truncation boxes' honest limits
    (2026-08-28). Widened to 0..0.5 with six labels, every tick is
    round and true; a range that cannot be widened cheaply keeps its
    exact ends instead."""
    from hertzforge.drawing.viz.waterfall import nice_axis

    assert nice_axis(0.0, 0.499755859375) == (0.0, 0.5, 6)
    assert nice_axis(-0.1, 0.4) == pytest.approx((-0.1, 0.4, 6))
    # a 1024 Hz Nyquist would widen to 1200 — a fifth of the stage
    # holding no data — so it keeps its exact range
    assert nice_axis(0.0, 1024.0) == (0.0, 1024.0, 5)
    # degenerate spans change nothing
    assert nice_axis(0.0, 0.0) == (0.0, 0.0, 5)


def test_the_stage_spans_what_is_drawn_not_every_line():
    """A PSD written as a controller writes a specification — every
    line to Nyquist, zero outside its band — draws only over the band,
    and the stage spanned all its lines: 20 to 400 Hz stood as a sliver
    of a 0 to 2048 Hz floor (Brandon, 2026-10-09)."""
    from hertzforge.core.data import Psd
    from hertzforge.drawing.viz.waterfall import stage_curves, waterfall_arrays

    lines = np.arange(0.0, 2048.0 + 1, 2.0)
    level = np.where((lines >= 20.0) & (lines <= 400.0), 0.04, 0.0)
    psd = Psd(lines, np.vstack([level, 2 * level]),
              response_dof=['101Z+', '102Z+'],
              ordinate_dim='acceleration**2/frequency',
              ordinate_unit='(m/s**2)**2/Hz')
    x0, x1, _z0, _z1 = stage_curves(waterfall_arrays(psd))['extents']
    assert 15.0 <= x0 <= 20.0 and 400.0 <= x1 <= 410.0, (x0, x1)
