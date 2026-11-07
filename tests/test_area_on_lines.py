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


def test_a_flat_response_through_the_end_of_the_curve_is_out_nowhere():
    spec = _spec(**_limits())
    f = np.arange(0.0, 4096.0, 1.0)
    over, under = compliance.exceedances(spec, _psd(f, np.full(f.size, 0.01)),
                                         scale_db=0)
    assert not over.any() and not under.any()
    got = compliance.compare(spec, _psd(f, np.full(f.size, 0.01)), scale_db=0)
    assert got['difference_db'] == pytest.approx(0.0, abs=1e-9)


def test_a_dead_end_line_and_a_hot_one_are_still_found():
    spec = _spec(**_limits())
    f = np.arange(0.0, 4096.0, 1.0)
    values = np.full(f.size, 0.01)
    values[22] = 0.0
    values[2000] = 0.05
    over, under = compliance.exceedances(spec, _psd(f, values), scale_db=0)
    assert under[22] and over[2000]
    assert under.sum() == 1 and over.sum() == 1
