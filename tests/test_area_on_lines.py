"""A breakpoint specification read onto lines keeps its area, at any
spacing, and the comparison reads those lines as what they are."""

import dataclasses

import numpy as np
import pytest

from hertzforge.core import compliance
from hertzforge.core.author import SpecificationDraft
from hertzforge.core.compliance import area_on_lines, log_log_area, record_on_lines
from hertzforge.core.data import Psd, Specification

DIM = 'acceleration**2/frequency'
FLAT = ([21.9, 2000.0], [0.01, 0.01])
SLOPED = ([20.0, 80.0, 350.0, 2000.0], [0.01, 0.04, 0.04, 0.007])


def _sheet(frequencies, levels):
    draft = SpecificationDraft.at_dofs(['1Z+'], 'acceleration')
    n = len(frequencies) - 1
    return dataclasses.replace(
        draft, frequencies=list(frequencies), levels=[list(levels)],
        bands=[{**draft.bands[0], 'warning': draft.bands[0]['warning'][:1] * n,
                'abort': draft.bands[0]['abort'][:1] * n}])


def _spec(frequencies=FLAT[0], levels=FLAT[1], **limits):
    return Specification(np.array(frequencies), np.array([levels], dtype=float),
                         response_dof=['1Z+'], ordinate_dim=[DIM], **limits)


def _psd(f, values):
    return Psd(f, np.array([values], dtype=float), response_dof=['1Z+'],
               ordinate_dim=[DIM])


@pytest.mark.parametrize('spacing', [0.25, 0.5, 1.0, 2.0, 4.0, 3.0])
@pytest.mark.parametrize('written', [FLAT, SLOPED])
def test_the_lines_hold_the_area_of_the_curve_at_any_spacing(written, spacing):
    lines = _sheet(*written).to_lines(spacing)
    area = np.sum(lines.levels[0]) * spacing
    assert area == pytest.approx(log_log_area(*written), rel=1e-12)


def test_an_end_line_holds_the_part_of_its_bin_the_curve_covers():
    """21.9 Hz is 0.6 of the way through the 22 Hz line's bin, and 2000 Hz
    is half way through its own."""
    lines = _sheet(*FLAT).to_lines(1.0)
    assert lines.frequencies[0] == 22.0 and lines.frequencies[-1] == 2000.0
    assert lines.levels[0][0] == pytest.approx(0.006)
    assert lines.levels[0][-1] == pytest.approx(0.005)
    assert np.allclose(lines.levels[0][1:-1], 0.01)
    assert any('an end line holds only' in note for note in lines.notes)


def test_a_curve_reaching_barely_into_a_bin_makes_that_line():
    lines = _sheet([22.4, 2000.0], [0.01, 0.01]).to_lines(1.0)
    assert lines.frequencies[0] == 22.0, 'its center is below the curve'
    assert lines.levels[0][0] == pytest.approx(0.01 * 0.1)
    lines = _sheet([22.5, 2000.0], [0.01, 0.01]).to_lines(1.0)
    assert lines.frequencies[0] == 23.0, 'the next bin starts on the curve'
    assert lines.levels[0][0] == pytest.approx(0.01)
    lines = _sheet([21.9, 2000.0], [0.01, 0.01]).to_lines(4.0)
    assert lines.frequencies[0] == 20.0
    assert lines.levels[0][0] == pytest.approx(0.01 * 0.1 / 4.0)


def test_a_curve_written_to_the_edges_of_its_bins_is_flat_on_its_lines():
    """The writer's way to a flat target at 1 Hz: every line whole."""
    lines = _sheet([19.5, 2000.5], [0.01, 0.01]).to_lines(1.0)
    assert lines.frequencies[0] == 20.0 and lines.frequencies[-1] == 2000.0
    assert np.allclose(lines.levels[0], 0.01)
    assert not any('an end line holds only' in note for note in lines.notes)


@pytest.mark.parametrize('spacing', [0.5, 1.0, 2.0, 4.0])
@pytest.mark.parametrize('written', [FLAT, SLOPED,
                                     ([10.0, 40.0, 200.0, 2000.0],
                                      [0.0002, 0.002, 0.002, 0.0002])])
def test_the_lines_fold_back_to_the_breakpoints(written, spacing):
    lines = _sheet(*written).to_lines(spacing)
    folded = lines.to_breakpoints()
    assert folded.frequencies == pytest.approx(written[0], abs=1e-6)
    assert folded.levels[0] == pytest.approx(written[1], rel=1e-6)
    assert np.allclose(folded.to_lines(spacing).levels, lines.levels)


def test_a_sheet_on_lines_read_again_is_not_read_twice():
    lines = _sheet(*FLAT).to_lines(1.0)
    assert lines.to_lines(1.0) is lines
    assert np.sum(lines.to_lines(2.0).levels[0]) * 2.0 == pytest.approx(
        log_log_area(*FLAT), rel=1e-9)


def test_a_cross_term_keeps_its_coherence_and_phase_on_a_part_bin():
    draft = SpecificationDraft.at_dofs(['1X+', '2X+'], 'acceleration')
    spec = dataclasses.replace(draft, levels=[[1.0, 0.01], [4.0, 0.4]],
                               pairs={(0, 1): (0.8, -40.0)}).make()
    lines = np.arange(0.0, 2100.0, 5.0)
    i = next(k for k in range(spec.num_records)
             if spec.record_pair(k) == ('1X+', '1X+'))
    j = next(k for k in range(spec.num_records)
             if spec.record_pair(k) == ('2X+', '2X+'))
    cross = next(k for k in range(spec.num_records)
                 if spec.response_dof[k] != spec.reference_dof[k])
    term = record_on_lines(spec, cross, lines)
    auto_a = record_on_lines(spec, i, lines)
    auto_b = record_on_lines(spec, j, lines)
    held = np.isfinite(auto_a) & np.isfinite(auto_b)
    assert held.sum() > 300 and np.isnan(term[~held]).all()
    np.testing.assert_allclose(
        np.abs(term[held]) ** 2 / (auto_a[held] * auto_b[held]), 0.8, rtol=1e-9)
    np.testing.assert_allclose(np.abs(np.angle(term[held], deg=True)), 40.0,
                               rtol=1e-9)


def test_the_area_read_is_nan_where_the_bin_holds_none_of_the_curve():
    lines = np.arange(0.0, 2010.0, 1.0)
    read = area_on_lines(*FLAT, lines)
    assert np.isnan(read[:22]).all() and np.isnan(read[2001:]).all()
    assert read[22] == pytest.approx(0.006)


def _limits(spec_f=FLAT[0], level=0.01):
    return {'abort_lower': np.array([[level / 4.0] * 2]),
            'abort_upper': np.array([[level * 4.0] * 2])}


@pytest.mark.parametrize('spacing', [1.0, 4.0])
def test_a_response_on_the_lines_read_from_the_curve_is_out_nowhere(spacing):
    """An end line holding a sliver of its bin is not low against a curve
    that only reached a sliver into it."""
    spec = _spec(**_limits())
    lines = _sheet(*FLAT).to_lines(spacing)
    f = np.arange(0.0, 4096.0, spacing)
    values = np.zeros_like(f)
    values[np.searchsorted(f, lines.frequencies)] = lines.levels[0]
    over, under = compliance.exceedances(spec, _psd(f, values), scale_db=0)
    assert not over.any() and not under.any()


def _warned(low=20.0, high=2000.0):
    level = np.array([[0.01, 0.01]])
    return Specification(np.array([low, high]), level, response_dof=['1Z+'],
                         ordinate_dim=[DIM], warning_lower=level / 10 ** 0.3,
                         warning_upper=level * 10 ** 0.3)


@pytest.mark.parametrize('df', [0.5, 1.0, 2.0, 4.0])
def test_a_line_is_judged_by_the_energy_in_its_whole_bin(df):
    """The curve's energy does not depend on the spacing (Brandon,
    2026-11-07). A response on the area-equivalent lines holds exactly
    that at any spacing and is out nowhere. One running flat through the
    end lines holds more, by an amount that grows with the bin, and those
    two lines are over."""
    spec = _warned()
    f = np.arange(0.0, 4096.0 + df, df)
    on = area_on_lines(spec.abscissa, spec.ordinate[0], f)
    held = _psd(f, np.nan_to_num(on))
    over, under = compliance.exceedances(spec, held, 0, 0, 'warning', 0.0)
    assert not over.any() and not under.any()
    got = compliance.compare(spec, held, scale_db=0)
    assert got['measured_rms'] ** 2 == pytest.approx(19.8)
    assert got['specification_rms'] ** 2 == pytest.approx(19.8)
    assert 'partly_written' not in got

    flat = _psd(f, np.where(np.isfinite(on), 0.01, 0.0))
    over, under = compliance.exceedances(spec, flat, 0, 0, 'warning', 0.0)
    assert not under.any()
    assert f[over].tolist() == [20.0, 2000.0]
    got = compliance.compare(spec, flat, scale_db=0)
    assert got['measured_rms'] ** 2 == pytest.approx(19.8 + 0.01 * df)
    assert [line for line, _share in got['partly_written']] == pytest.approx([20.0, 2000.0])
    assert [share for _line, share in got['partly_written']] == pytest.approx([0.5, 0.5])
    assert compliance.partly_written_text(got['partly_written']).endswith(
        'whole bin: 20 Hz (50 %), 2000 Hz (50 %)')


def test_a_spec_written_to_the_bins_has_one_reading():
    """Written from 19.5 to 2000.5 Hz, every 1 Hz line is whole, so a flat
    response is on it, and half a line is under the limits."""
    spec = _warned(19.5, 2000.5)
    f = np.arange(0.0, 4096.0)
    flat = np.where((f >= 20.0) & (f <= 2000.0), 0.01, 0.0)
    over, under = compliance.exceedances(spec, _psd(f, flat), 0, 0, 'warning', 0.0)
    assert not over.any() and not under.any()
    flat[2000] /= 2.0
    over, under = compliance.exceedances(spec, _psd(f, flat), 0, 0, 'warning', 0.0)
    assert not over.any() and f[under].tolist() == [2000.0]


def test_a_range_cut_from_a_longer_curve_is_not_an_end():
    """A flat response is flat across a step in the tolerance: the line
    on the step is read in each range over its own part, and only the
    curve's real ends read whole."""
    from hertzforge.core.requirement import within

    spec = _warned()
    f = np.arange(0.0, 4096.0)
    held = _psd(f, np.full(f.size, 0.01))
    below = compliance.cells(within(spec, 0, None, 500.0), held)
    above = compliance.cells(within(spec, 0, 500.0, None), held)
    assert below[0]['pieces'] == [(19.5, 20.5)]
    assert below[-1]['pieces'] == [(499.5, 500.0)]
    assert above[0]['pieces'] == [(500.0, 500.5)]
    assert above[-1]['pieces'] == [(1999.5, 2000.5)]
    assert below[0]['low'] == 20.0 and above[-1]['high'] == 2000.0


def test_a_dead_end_line_and_a_hot_one_are_still_found():
    spec = _spec(**_limits())
    f = np.arange(0.0, 4096.0, 1.0)
    values = np.full(f.size, 0.01)
    values[22] = 0.0
    values[2000] = 0.05
    over, under = compliance.exceedances(spec, _psd(f, values), scale_db=0)
    assert under[22] and over[2000]
    assert under.sum() == 1 and over.sum() == 1
