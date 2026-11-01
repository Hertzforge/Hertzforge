"""How many lines of a comparison fall outside its bands by chance.

An averaged autospectrum scatters about its true level by an amount
its degrees of freedom set, so a response sitting exactly on its
specification still puts lines outside a narrow band. The count of
lines outside is read beside the number chance alone puts there, and
lines may be combined, as some test documents allow, to shrink the
scatter. Pinned against the chi-square's closed forms at two and four
degrees of freedom, the printed shares at 120 and 100, hand-built
spectra whose every line is placed, and averaged white noise.
"""

from __future__ import annotations

import math

import numpy as np
import pytest

from hertzforge.core import compliance
from hertzforge.core.chance import (
    chance_outside,
    compare_chance,
    describe_chance,
    expected_text,
)
from hertzforge.core.data import Psd, Specification

#: the spacing of the hand-built spectra's lines, Hz
DF = 5.0

#: one line's degrees of freedom in the hand-built spectra
DOF = 120.0


def flat_spec(low=30.0, high=1970.0, warning=1.5, abort=3.0, channels=1):
    """A flat specification of unit density over `low` to `high`, its
    bands a fixed number of dB either side (None for no such pair)."""
    f = np.array([low, high])
    target = np.ones((channels, 2))
    limits = {}
    for pair, db in (('warning', warning), ('abort', abort)):
        if db is not None:
            limits[f'{pair}_lower'] = target * 10.0 ** (-db / 10.0)
            limits[f'{pair}_upper'] = target * 10.0 ** (db / 10.0)
    return Specification(f, target,
                         response_dof=[f'{i + 1}Z+' for i in range(channels)],
                         ordinate_dim='acceleration**2/frequency', **limits)


def lines(db=None, count=401, dof=DOF):
    """A PSD on lines `DF` apart from zero, each `db` over unit density
    (all on it by default), carrying `dof` degrees of freedom."""
    values = np.ones(count) if db is None else 10.0 ** (np.asarray(db) / 10.0)
    psd = Psd(DF * np.arange(count), values[None, :], response_dof=['1Z+'],
              ordinate_dim='acceleration**2/frequency')
    if dof is not None:
        psd.statistical_dof = np.array([float(dof)])
    return psd


def at_cells(changes, count=401):
    """`db` for `lines` with the judged cells numbered from zero: the
    30 Hz line (half judged) is cell 0, PSD line 6."""
    db = np.zeros(count)
    for cell, value in changes.items():
        db[6 + cell] = value
    return db


def test_two_degrees_of_freedom_are_an_exponential():
    """One frame: the estimate over the truth is an exponential variable
    over two, so a line reads under r times the truth with probability
    1 - exp(-r) and over it with exp(-r)."""
    share = chance_outside(2.0, -3.0, 3.0)
    assert share.below == pytest.approx(1.0 - math.exp(-10.0 ** -0.3), rel=1e-12)
    assert share.above == pytest.approx(math.exp(-10.0 ** 0.3), rel=1e-12)
    assert share.below == pytest.approx(0.394189, abs=1e-6)
    assert share.above == pytest.approx(0.135978, abs=1e-6)
    assert share.total == pytest.approx(share.below + share.above)


def test_four_degrees_of_freedom_follow_their_closed_form():
    """P(chi2_4 > x) = exp(-x/2) (1 + x/2), at x = 4 r."""
    def over(x):
        return math.exp(-x / 2.0) * (1.0 + x / 2.0)

    share = chance_outside(4.0, -3.0, 3.0)
    assert share.above == pytest.approx(over(4.0 * 10.0 ** 0.3), rel=1e-12)
    assert share.below == pytest.approx(1.0 - over(4.0 * 10.0 ** -0.3),
                                        rel=1e-12)


@pytest.mark.parametrize('dof, band, total', [(120.0, 1.5, 0.00845),
                                              (100.0, 1.5, 0.01606)])
def test_a_tight_band_meets_the_scatter(dof, band, total):
    """About 0.85 percent of lines outside plus or minus 1.5 dB at 120
    degrees of freedom and 1.6 percent at 100, most of them low: the
    scatter of an average reaches further below the truth than above."""
    share = chance_outside(dof, -band, band)
    assert share.total == pytest.approx(total, abs=5e-5)
    assert share.below > 2.0 * share.above


def test_a_wide_band_is_out_by_chance_one_line_in_a_million():
    share = chance_outside(120.0, -3.0, 3.0)
    assert share.total == pytest.approx(9.96e-7, rel=1e-3)
    # and one decibel low, the narrow under-test side of an asymmetric
    # band, is about one line in twenty
    assert chance_outside(120.0, -1.0).below == pytest.approx(0.0471, abs=1e-4)


@pytest.mark.parametrize('dof', [2.0, 13.7, 120.0, 960.0])
@pytest.mark.parametrize('level', [0.90, 0.95])
def test_the_chance_is_the_confidence_bands_tail(dof, level):
    """The same law as the confidence band on an averaged PSD, read the
    other way: where `chi_square_factors` puts the truth about an
    estimate at a level, an estimate reads that far from the truth with
    half the rest of the probability on each side."""
    from hertzforge.core.averaging import chi_square_factors

    lower, upper = chi_square_factors(dof, level)
    share = chance_outside(dof, -10.0 * np.log10(upper),
                           -10.0 * np.log10(lower))
    tail = (1.0 - level) / 2.0
    assert share.below == pytest.approx(tail, rel=1e-9)
    assert share.above == pytest.approx(tail, rel=1e-9)


def test_an_open_edge_puts_nothing_past_it():
    assert chance_outside(120.0, None, 1.5).below == 0.0
    assert chance_outside(120.0, -1.5, None).above == 0.0
    assert chance_outside(120.0, -np.inf, np.inf).total == 0.0


def test_edges_that_vary_give_a_share_per_line():
    share = chance_outside(120.0, [-1.5, -3.0], [1.5, 3.0])
    assert isinstance(share.below, np.ndarray)
    assert share.total[0] == pytest.approx(chance_outside(120.0, -1.5, 1.5).total)
    assert share.total[1] == pytest.approx(chance_outside(120.0, -3.0, 3.0).total)


@pytest.mark.parametrize('dof', [0.0, -2.0, float('nan')])
def test_no_degrees_of_freedom_is_refused(dof):
    with pytest.raises(ValueError, match='more than zero'):
        chance_outside(dof, -1.5, 1.5)


def test_a_spectrum_on_its_target_is_expected_to_miss_by_chance():
    """389 lines judged from 30 to 1970 Hz on 5 Hz lines, none outside,
    and 389 times each line's chance expected: about 3.3 outside the
    1.5 dB band, 0.00039 outside the 3 dB band."""
    result = compare_chance(flat_spec(), lines(), scale_db=0)
    assert result['lines'] == 389
    assert result['dof'] == DOF and result['judged_dof'] == DOF
    assert result['warning_lines'] == result['abort_lines'] == 0
    each = chance_outside(DOF, -1.5, 1.5)
    assert result['warning_expected'] == pytest.approx(389 * each.total)
    assert result['warning_expected'] == pytest.approx(3.288, abs=1e-3)
    assert result['warning_expected_below'] == pytest.approx(389 * each.below)
    assert result['warning_expected_above'] == pytest.approx(389 * each.above)
    assert result['abort_expected'] == pytest.approx(
        389 * chance_outside(DOF, -3.0, 3.0).total)
    # each line's edges in dB from the target, read from the areas
    groups = result['groups']
    assert np.allclose(groups['warning_lower_db'], -1.5)
    assert np.allclose(groups['abort_upper_db'], 3.0)
    assert np.allclose(groups['miss_db'], 0.0, atol=1e-12)
    assert groups['lines'].tolist() == [1] * 389
    # the expectation is the lines' own chances added
    assert groups['warning_chance'].sum() == pytest.approx(
        result['warning_expected'])
    assert groups['abort_chance'].sum() == pytest.approx(
        result['abort_expected'])


def test_the_lines_out_are_counted_by_side():
    """Three lines 2 dB low are out of warning below; one 3.5 dB high is
    out of both pairs above, and counted in each, as `compare` counts."""
    db = at_cells({10: -2.0, 50: -2.0, 300: -2.0, 200: 3.5})
    spec, psd = flat_spec(), lines(db)
    result = compare_chance(spec, psd, scale_db=0)
    assert (result['warning_lines'], result['warning_below'],
            result['warning_above']) == (4, 3, 1)
    assert (result['abort_lines'], result['abort_below'],
            result['abort_above']) == (1, 0, 1)
    assert np.flatnonzero(result['groups']['warning_below']).tolist() == [10, 50, 300]
    plain = compliance.compare(spec, psd, scale_db=0)
    assert (plain['warning_lines'], plain['abort_lines']) == (4, 1)


def test_the_counts_are_the_comparisons_own_on_averaged_noise():
    """Uncombined, every count is `compare`'s, channel for channel; and
    over sixteen channels of white noise averaged to 120 degrees of
    freedom, unwindowed, the lines outside 1.5 dB come to within their
    binomial spread of what chance predicts."""
    from hertzforge.core.averaging import Averaging
    from hertzforge.core.data import TimeHistory

    rate, channels = 5120.0, 16
    averaging = Averaging(frame_length=1024, overlap=0.0, window='boxcar',
                          frames=60)
    rng = np.random.default_rng(1)
    history = TimeHistory(np.arange(averaging.span) / rate,
                          rng.standard_normal((channels, averaging.span)),
                          response_dof=[f'{i + 1}Z+' for i in range(channels)],
                          ordinate_dim='acceleration')
    history.averaging = averaging
    psd = history.compute_psds()
    # white noise of unit variance has the one-sided density 2/fs
    spec = flat_spec(channels=channels)
    spec.ordinate = spec.ordinate * 2.0 / rate
    for name in list(spec.limits):
        spec.limits[name] = spec.limits[name] * 2.0 / rate
    counted = expected = judged = 0
    for i in range(channels):
        result = compare_chance(spec, psd, i, i, scale_db=0)
        plain = compliance.compare(spec, psd, i, i, scale_db=0)
        assert result['dof'] == pytest.approx(120.0)
        assert result['lines'] == plain['lines'] == 389
        assert result['warning_lines'] == plain['warning_lines']
        assert result['abort_lines'] == plain['abort_lines']
        counted += result['warning_lines']
        expected += result['warning_expected']
        judged += result['lines']
    assert expected == pytest.approx(judged * 0.008452, rel=1e-3)
    assert abs(counted - expected) < 4.0 * math.sqrt(expected)


def test_correlated_lines_combined_and_in_a_coarse_cell_are_worth_less():
    """Lines correlated at 0.25 one apart: two combined are worth
    (1 + 1)² n / (2 + 2 x 0.25), 1.6 times a line. A cell four lines
    wide, correlated at 0.5, holds 16 n / (4 + 2 x 0.5 x 3), 16/7 of a
    line."""
    psd = lines()
    psd.line_correlation = np.array([1.0, 0.25])
    result = compare_chance(flat_spec(), psd, scale_db=0, combine=2)
    assert result['groups']['dof'][1:-1] == pytest.approx(1.6 * DOF)
    f = np.array([20.0, 40.0, 60.0, 80.0])
    values = np.ones((1, 4))
    spec = Specification(f, values, response_dof=['1Z+'],
                         ordinate_dim='acceleration**2/frequency',
                         warning_lower=values * 0.5, warning_upper=values * 2.0)
    spec.interpolation = 'bin'
    fine = Psd(2.5 + 5.0 * np.arange(21), np.ones((1, 21)),
               response_dof=['1Z+'], ordinate_dim='acceleration**2/frequency')
    fine.line_correlation = np.array([1.0, 0.5])
    coarse = compare_chance(spec, fine, dof=30.0, scale_db=0)
    assert coarse['groups']['lines'].tolist() == [4, 4, 4, 4]
    assert coarse['groups']['dof'] == pytest.approx(np.full(4, 30.0 * 16.0 / 7.0))
    assert 'note' not in coarse


def test_lines_summed_without_a_correlation_are_said_to_be_taken_independent():
    result = compare_chance(flat_spec(), lines(), scale_db=0, combine=8)
    assert 'taken as independent' in result['note']
    assert 'note' not in compare_chance(flat_spec(), lines(), scale_db=0)


def test_combining_eight_lines_takes_the_scatter_out():
    """Lines alternately 2 dB high and 2 dB low are every one outside
    plus or minus 1.5 dB; eight at a time their powers add to 0.2 dB
    over, and no group is out. Each full group is worth 960 degrees of
    freedom, eight times a line's; the first holds the 30 Hz line judged
    over half its bin, so it is worth (7.5)^2 / (7.25/120); the last
    holds the five lines left."""
    db = np.where(np.arange(401) % 2, 2.0, -2.0)
    spec, psd = flat_spec(), lines(db)
    single = compare_chance(spec, psd, scale_db=0)
    assert single['warning_lines'] == 389
    result = compare_chance(spec, psd, scale_db=0, combine=8)
    assert result['combine'] == 8
    assert result['lines'] == 49
    assert result['warning_lines'] == 0
    groups = result['groups']
    assert groups['lines'].tolist() == [8] * 48 + [5]
    assert groups['dof'][1:48] == pytest.approx(np.full(47, 960.0))
    assert groups['dof'][0] == pytest.approx(7.5 ** 2 / (7.25 / 120.0))
    assert groups['dof'][-1] == pytest.approx(4.5 ** 2 / (4.25 / 120.0))
    assert result['judged_dof'] == pytest.approx(960.0)
    assert result['warning_expected'] < 1e-6
    # a group's level is its lines' powers added, over the target's
    assert abs(groups['miss_db'][5]) == pytest.approx(
        10.0 * np.log10((10.0 ** 0.2 + 10.0 ** -0.2) / 2.0))


def test_a_real_excursion_survives_the_combination():
    """Sixteen adjacent lines 2.5 dB high, cells 40 to 55, are two whole
    groups of eight, and both are out above; the single lines count
    sixteen."""
    db = at_cells({cell: 2.5 for cell in range(40, 56)})
    spec, psd = flat_spec(), lines(db)
    assert compare_chance(spec, psd, scale_db=0)['warning_above'] == 16
    result = compare_chance(spec, psd, scale_db=0, combine=8)
    assert result['warning_above'] == 2
    assert np.flatnonzero(result['groups']['warning_above']).tolist() == [5, 6]
    assert result['abort_lines'] == 0


def test_a_group_on_a_slope_is_worth_less_than_its_lines_added():
    """Two lines asked for 1 and 3 add to (1 + 3)^2 / ((1 + 9) / n),
    1.6 times a line's count, not two: the louder line carries more of
    the sum."""
    f = np.array([10.0, 20.0])
    spec = Specification(f, np.array([[1.0, 3.0]]), response_dof=['1Z+'],
                         ordinate_dim='acceleration**2/frequency',
                         warning_lower=np.array([[1.0, 3.0]]) * 0.5,
                         warning_upper=np.array([[1.0, 3.0]]) * 2.0)
    spec.interpolation = 'bin'
    psd = Psd(f, np.array([[1.0, 3.0]]), response_dof=['1Z+'],
              ordinate_dim='acceleration**2/frequency')
    result = compare_chance(spec, psd, dof=50.0, combine=2, scale_db=0)
    assert result['lines'] == 1
    assert result['groups']['dof'][0] == pytest.approx(1.6 * 50.0)
    assert result['warning_expected'] == pytest.approx(
        chance_outside(80.0, -10 * np.log10(2.0), 10 * np.log10(2.0)).total)


def test_a_cell_coarser_than_the_lines_holds_their_count():
    """A specification on 20 Hz bins against 5 Hz lines is judged on its
    own bins, and each holds four lines: four times a line's degrees of
    freedom, with nothing combined."""
    f = np.array([20.0, 40.0, 60.0, 80.0])
    values = np.ones((1, 4))
    spec = Specification(f, values, response_dof=['1Z+'],
                         ordinate_dim='acceleration**2/frequency',
                         warning_lower=values * 0.5, warning_upper=values * 2.0)
    spec.interpolation = 'bin'
    psd = Psd(2.5 + 5.0 * np.arange(21), np.ones((1, 21)),
              response_dof=['1Z+'], ordinate_dim='acceleration**2/frequency')
    result = compare_chance(spec, psd, dof=30.0, scale_db=0)
    assert result['groups']['lines'].tolist() == [4, 4, 4, 4]
    assert result['groups']['dof'] == pytest.approx(np.full(4, 120.0))
    assert result['judged_dof'] == pytest.approx(120.0)


def test_a_group_does_not_reach_across_an_unwritten_stretch():
    """A specification on lines with one line unwritten in the middle:
    the lines either side of it are not neighbors, so four at a time
    gives groups of four and one on each side."""
    f = 10.0 * np.arange(1, 12)
    values = np.ones((1, 11))
    values[0, 5] = np.nan
    spec = Specification(f, values, response_dof=['1Z+'],
                         ordinate_dim='acceleration**2/frequency',
                         warning_lower=values * 0.5, warning_upper=values * 2.0)
    spec.interpolation = 'bin'
    psd = Psd(f, np.ones((1, 11)), response_dof=['1Z+'],
              ordinate_dim='acceleration**2/frequency')
    result = compare_chance(spec, psd, dof=20.0, combine=4, scale_db=0)
    assert result['groups']['lines'].tolist() == [4, 1, 4, 1]
    assert result['groups']['low'].tolist() == [5.0, 45.0, 65.0, 105.0]


def test_a_psd_that_does_not_say_its_count_is_refused_until_given():
    spec, psd = flat_spec(), lines(dof=None)
    refused = compare_chance(spec, psd, scale_db=0)
    assert refused == {'lines': 0, 'refused': (
        'the PSD does not say how many degrees of freedom its average '
        'has; give them')}
    given = compare_chance(spec, psd, dof=100.0, scale_db=0)
    assert given['dof'] == 100.0
    assert given['warning_expected'] == pytest.approx(
        389 * chance_outside(100.0, -1.5, 1.5).total)


def test_a_line_on_its_limit_is_inside_as_compare_reads_it():
    """Lines on a specification of their own lines, two of them exactly
    on the warning edge above and two a part in a million past it: the
    two on it are inside, alone and as a pair, as `compare` counts, and
    the two past it are out, alone and as a pair. A pair whose first
    line is inside and whose sum is out is out as a pair: the group is
    judged on its sum, not its first line."""
    f = DF * np.arange(1, 9)
    edge = 10.0 ** 0.15
    values = np.ones((1, 8))
    spec = Specification(f, values, response_dof=['1Z+'],
                         ordinate_dim='acceleration**2/frequency',
                         warning_lower=values / edge, warning_upper=values * edge)
    spec.interpolation = 'bin'
    level = np.ones(8)
    level[[2, 3]] = edge
    level[[4, 5]] = [edge * 0.999, edge * 1.002]
    level[[6, 7]] = edge * (1.0 + 1e-6)
    psd = Psd(f, level[None, :], response_dof=['1Z+'],
              ordinate_dim='acceleration**2/frequency')
    psd.statistical_dof = np.array([DOF])
    single = compare_chance(spec, psd, scale_db=0)
    assert np.flatnonzero(single['groups']['warning_above']).tolist() == \
        [5, 6, 7]
    assert single['warning_above'] == compliance.compare(
        spec, psd, scale_db=0)['warning_lines'] == 3
    paired = compare_chance(spec, psd, scale_db=0, combine=2)
    assert paired['groups']['lines'].tolist() == [2, 2, 2, 2]
    assert np.flatnonzero(paired['groups']['warning_above']).tolist() == [2, 3]


def test_the_count_is_the_measured_records_own():
    """Two records averaged to different counts: the second is read at
    its own, not the first's."""
    psd = Psd(DF * np.arange(401), np.ones((2, 401)),
              response_dof=['1Z+', '2Z+'],
              ordinate_dim='acceleration**2/frequency')
    psd.statistical_dof = np.array([120.0, 30.0])
    spec = flat_spec(channels=2)
    assert compare_chance(spec, psd, 0, 0, scale_db=0)['dof'] == 120.0
    second = compare_chance(spec, psd, 1, 1, scale_db=0)
    assert second['dof'] == 30.0
    assert second['warning_expected'] == pytest.approx(
        389 * chance_outside(30.0, -1.5, 1.5).total)


def test_a_cross_term_is_refused_as_the_confidence_band_refuses_it():
    """The chi-square law is an autospectrum's: a cross spectrum between
    two channels, and force against acceleration at one DOF, are both
    refused."""
    f = DF * np.arange(401)
    between = Psd(f, np.ones((1, 401)) + 0.5j, response_dof=['1Z+'],
                  reference_dof=['2Z+'],
                  ordinate_dim='acceleration**2/frequency')
    between.statistical_dof = np.array([DOF])
    refused = compare_chance(flat_spec(), between, scale_db=0)
    assert refused['lines'] == 0
    assert 'cross term (1Z+/2Z+)' in refused['refused']
    assert not between.is_autospectrum(0) and lines().is_autospectrum()
    drive = Psd(f, np.ones((1, 401)), response_dof=['1Z+'],
                reference_dof=['1Z+'],
                ordinate_dim='acceleration*force/frequency')
    assert not drive.is_autospectrum(0)
    with pytest.raises(ValueError, match='cross terms'):
        between.confidence_band(dof=DOF)


def test_a_count_given_outranks_the_psds():
    result = compare_chance(flat_spec(), lines(dof=120.0), dof=2.0, scale_db=0)
    assert result['dof'] == 2.0


def test_octave_bands_are_refused():
    spec = flat_spec()
    banded = lines().to_octave(3)
    banded_spec = Specification(banded.abscissa, np.ones((1, banded.abscissa.size)),
                                response_dof=['1Z+'],
                                ordinate_dim='acceleration**2/frequency',
                                bandwidth=banded.bandwidth,
                                warning_upper=np.full((1, banded.abscissa.size), 2.0))
    banded.statistical_dof = np.array([120.0])
    result = compare_chance(banded_spec, banded, scale_db=0)
    assert result['lines'] == 0
    assert 'narrowband lines' in result['refused']
    # a band against a breakpoint curve is refused as `compare` refuses it
    assert compare_chance(spec, banded, scale_db=0)['refused'] == \
        compliance.comparable(spec, banded)


def test_a_pair_of_different_quantities_is_refused():
    psd = lines()
    psd.ordinate_dim = ['velocity**2/frequency']
    result = compare_chance(flat_spec(), psd, scale_db=0)
    assert result['lines'] == 0 and 'cannot be compared' in result['refused']


@pytest.mark.parametrize('combine', [0, -1, 1.5, True])
def test_lines_are_combined_a_whole_number_at_a_time(combine):
    with pytest.raises(ValueError, match='whole number'):
        compare_chance(flat_spec(), lines(), combine=combine, scale_db=0)


def test_a_count_of_nothing_is_refused():
    with pytest.raises(ValueError, match='more than zero'):
        compare_chance(flat_spec(), lines(), dof=0.0, scale_db=0)


def test_a_specification_without_bands_has_nothing_to_count():
    result = compare_chance(flat_spec(warning=None, abort=None), lines(),
                            scale_db=0)
    assert result['lines'] == 389
    assert not any(key.startswith(('warning', 'abort')) for key in result)
    assert describe_chance(result, 'warning') == 'no warning band'


def test_one_pair_alone_is_read_alone():
    result = compare_chance(flat_spec(abort=None), lines(), scale_db=0)
    assert 'warning_expected' in result and 'abort_expected' not in result


def test_the_reading_in_words():
    db = at_cells({10: -2.0, 50: -2.0, 300: -2.0, 200: 3.5})
    result = compare_chance(flat_spec(), lines(db), scale_db=0)
    assert describe_chance(result, 'warning') == (
        '4 of 389 lines outside the warning band (3 below, 1 above), '
        '3.3 expected by chance (2.5 below, 0.8 above)')
    # the line 3.5 dB high adds to 0.6 dB over in its group of eight
    combined = compare_chance(flat_spec(), lines(db), scale_db=0, combine=8)
    assert describe_chance(combined, 'abort').startswith(
        '0 of 49 groups outside the abort band')


@pytest.mark.parametrize('value, text', [(3.2878, '3.3'), (0.095, '0.1'),
                                         (0.0773, '0.077'),
                                         (0.000387, '0.00039'),
                                         (4e-10, '0'), (0.0, '0')])
def test_an_expectation_keeps_the_digits_that_say_something(value, text):
    assert expected_text(value) == text
