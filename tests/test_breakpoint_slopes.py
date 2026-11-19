"""A random specification written by slope, and its table read by segment.

Many written tables give a level, a slope in dB per octave and the next
frequency, and leave the level there to the reader. The slope is an
exponent n = N / 3.0103, the level it reaches is W1 (f2 / f1)**n, and
each segment's area has a closed form with a logarithmic case at
n = -1 (minus 3.0103 dB per octave). The hand values below are worked
on a workmanship-style profile: 0.01 g²/Hz at 20 Hz, plus 3 dB per
octave to 80 Hz, flat at 0.04 to 500 Hz, minus 3 dB per octave to
2000 Hz. Its areas are 1.50, 16.80 and 27.73 g² and its running rms
1.22, 4.28 and 6.78 g, every one checked by hand:

    rise   0.04 x 80 / 2 x (1 - (20/80)**2)  = 1.6 x 0.9375 = 1.5
    flat   0.04 x (500 - 80)                 = 16.8
    fall   0.04 x 500 x ln 4                 = 27.7259
    rms    sqrt(46.0259)                     = 6.7842
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core.author import SpecificationDraft
from hertzforge.core.breakpoints import (
    OCTAVE_DB,
    level_at_slope,
    levels_from_slopes,
    levels_holding_slopes,
    levels_with_slope,
    read_breakpoints,
    running_rms,
    segment_areas,
    segment_slopes,
    slope_db,
    slope_exponent,
)
from hertzforge.core.compliance import specification_rms

G = 9.80665

#: the profile's breakpoints and levels, in Hz and g²/Hz, with its
#: slopes exactly an exponent of one (3.0103 dB per octave)
F = [20.0, 80.0, 500.0, 2000.0]

W = [0.01, 0.04, 0.04, 0.01]

AREAS = [1.5, 16.8, 0.04 * 500.0 * np.log(4.0)]

RUNNING = [np.sqrt(1.5), np.sqrt(18.3), np.sqrt(18.3 + AREAS[2])]


def _sheet(frequencies=None, levels=None, channels=('1Z+',)):
    """A one-or-more-channel draft in SI, levels given in g²/Hz."""
    frequencies = F if frequencies is None else frequencies
    levels = W if levels is None else levels
    return SpecificationDraft(
        list(channels), ['acceleration'] * len(channels), list(frequencies),
        [[v * G * G for v in levels] for _ in channels],
        {(i, j): (0.0, 0.0) for i in range(len(channels))
         for j in range(i + 1, len(channels))})


def test_a_slope_is_an_exponent_and_the_documents_three_is_not_quite_one():
    assert OCTAVE_DB == pytest.approx(3.0103, abs=1e-4)
    assert float(slope_exponent(3.0)) == pytest.approx(0.99658, abs=1e-5)
    assert float(slope_exponent(-6.0)) == pytest.approx(-1.99316, abs=1e-5)
    assert float(slope_exponent(OCTAVE_DB)) == 1.0
    assert float(slope_db(slope_exponent(4.5))) == pytest.approx(4.5)
    # the unprinted levels: two octaves at the documents' 3 dB reach
    # 0.0398 and 0.01005, the table's 0.04 and 0.01 to half a percent
    assert level_at_slope(20, 0.01, 80, 3) == pytest.approx(0.039811, abs=1e-6)
    assert level_at_slope(500, 0.04, 2000, -3) == pytest.approx(0.0100475, abs=1e-7)
    assert level_at_slope(20, 0.01, 80, OCTAVE_DB) == pytest.approx(0.04, rel=1e-14)
    # followed downward too, and flat is flat
    assert level_at_slope(80, 0.04, 20, OCTAVE_DB) == pytest.approx(0.01, rel=1e-14)
    assert level_at_slope(80, 0.04, 500, 0.0) == 0.04
    with pytest.raises(ValueError, match='positive'):
        level_at_slope(0, 0.01, 80, 3)
    with pytest.raises(ValueError, match='level'):
        level_at_slope(20, -0.01, 80, 3)


def test_the_segment_areas_are_the_closed_form_by_hand():
    areas = segment_areas(F, W)
    assert areas == pytest.approx(AREAS, rel=1e-12)
    assert areas[2] == pytest.approx(27.7259, abs=1e-4)
    reading = read_breakpoints(F, W)
    assert reading.running == pytest.approx(RUNNING, rel=1e-12)
    assert reading.rms == pytest.approx(6.7842, abs=1e-4)
    assert reading.total_area == pytest.approx(46.0259, abs=1e-4)
    assert reading.slopes == pytest.approx([OCTAVE_DB, 0.0, -OCTAVE_DB])
    # the falling segment, short on log axes, is 60 percent of the area
    assert reading.shares == pytest.approx([0.0326, 0.3650, 0.6024], abs=1e-4)
    assert reading.shares.sum() == pytest.approx(1.0)
    # the floor's shorthand, one rectangle: 79.2 g², 8.90 grms
    flat = read_breakpoints([20.0, 2000.0], [0.04, 0.04])
    assert flat.areas == pytest.approx([79.2])
    assert flat.rms == pytest.approx(8.8994, abs=1e-4)


def test_minus_one_takes_the_logarithm_and_minus_three_db_the_general_form():
    """An exponent of exactly minus one divides the general form by zero;
    typed as minus 3.0103 dB per octave the levels make that exponent to
    the last bit and the area is the logarithm. Typed as the documents'
    minus 3 the exponent is -0.9966 and the general form is right."""
    exact = levels_from_slopes([500.0, 2000.0], 0.04, [-OCTAVE_DB])
    assert exact[1] == pytest.approx(0.01, rel=1e-14)
    assert segment_areas([500.0, 2000.0], exact)[0] == pytest.approx(
        0.04 * 500.0 * np.log(4.0), rel=1e-12)
    typed = levels_from_slopes([500.0, 2000.0], 0.04, [-3.0])
    n = -3.0 / OCTAVE_DB
    by_hand = 0.04 * 500.0 * ((2000.0 / 500.0) ** (n + 1.0) - 1.0) / (n + 1.0)
    assert segment_areas([500.0, 2000.0], typed)[0] == pytest.approx(by_hand, rel=1e-9)
    assert by_hand == pytest.approx(27.7917, abs=1e-4)
    # and the trapezoid is wrong there: 37.5 against 27.7
    assert 0.5 * (0.04 + 0.01) * 1500.0 == pytest.approx(37.5)


def test_a_minus_one_segment_followed_by_another_takes_the_logarithm_whole():
    """The logarithmic case has two homes in `log_log_areas`: the whole
    segment, accumulated into the running total at the breakpoints, and
    the partial segment a band ends inside. A minus-one segment that is
    the last of its table only ever reaches the second, so the table
    here carries one on to a flat segment after it."""
    f = [*F, 4000.0]
    w = [*W, 0.01]
    assert segment_slopes(f, w)[2] == pytest.approx(-OCTAVE_DB, rel=1e-13)
    areas = segment_areas(f, w)
    assert areas == pytest.approx([*AREAS, 0.01 * 2000.0], rel=1e-12)
    # the running rms at 2000 Hz is the cumulative area through the
    # minus-one segment whole, and past it the flat segment adds 20 g²
    tail = running_rms(f, w, [2000.0, 3000.0, 4000.0])
    assert tail ** 2 == pytest.approx(
        [sum(AREAS), sum(AREAS) + 10.0, sum(AREAS) + 20.0], rel=1e-12)
    reading = read_breakpoints(f, w)
    assert reading.running[2] == pytest.approx(RUNNING[2], rel=1e-12)
    assert reading.rms ** 2 == pytest.approx(sum(AREAS) + 20.0, rel=1e-12)
    # and from slopes: minus 3.0103 into 2000 Hz, then flat to 4000 Hz
    typed = levels_from_slopes(f, 0.01, [OCTAVE_DB, 0.0, -OCTAVE_DB, 0.0])
    assert segment_areas(f, typed) == pytest.approx(areas, rel=1e-12)


def test_the_cited_worked_example_reads_the_same():
    """The worked example of the reference in `core.breakpoints`
    (Irvine, Integration of the Power Spectral Density Function, Rev B):
    10 Hz at 0.002, 100 and 1000 Hz at 0.04, 2000 Hz at 0.02 g²/Hz. It
    prints areas of 1.726, 36.000 and 27.726 g² and 8.09 grms. Its first
    exponent is rounded there to 1.3 from log(20)/log(10) = 1.30103,
    which is the whole of the difference from the 1.7296 here."""
    f = [10.0, 100.0, 1000.0, 2000.0]
    w = [0.002, 0.04, 0.04, 0.02]
    areas = segment_areas(f, w)
    assert areas == pytest.approx([1.7296, 36.0, 27.7259], abs=1e-4)
    rounded = 0.002 / 10.0 ** 1.3 / 2.3 * (100.0 ** 2.3 - 10.0 ** 2.3)
    assert rounded == pytest.approx(1.726, abs=5e-4), 'its own figure, at n = 1.3'
    assert read_breakpoints(f, w).rms == pytest.approx(8.09, abs=5e-3)
    # its Appendix A: 0.04 at 350 Hz, minus 3 dB per octave to 2000 Hz
    # is 0.007, and plus 3 dB per octave down from 80 to 20 Hz is 0.01
    assert level_at_slope(350.0, 0.04, 2000.0, -3.0) == pytest.approx(0.007, abs=5e-4)
    assert level_at_slope(80.0, 0.04, 20.0, 3.0) == pytest.approx(0.01, abs=5e-4)


def test_the_running_rms_climbs_from_zero_to_the_total():
    at = [10.0, 20.0, 80.0, 500.0, 2000.0, 3000.0]
    assert running_rms(F, W, at) == pytest.approx([0.0, 0.0, *RUNNING,
                                                   RUNNING[-1]], rel=1e-12)
    assert running_rms(F, W) == pytest.approx([0.0, *RUNNING], rel=1e-12)
    fine = np.geomspace(20.0, 2000.0, 500)
    climb = running_rms(F, W, fine)
    assert np.all(np.diff(climb) >= 0.0), 'an area only accumulates'
    # inside a segment it is the partial area by the same power law
    mid = running_rms(F, W, [40.0])[0]
    assert mid ** 2 == pytest.approx(0.01 / 20.0 * (40.0 ** 2 - 20.0 ** 2) / 2.0,
                                     rel=1e-12)


def test_slopes_and_levels_convert_both_ways():
    slopes = segment_slopes(F, W)
    assert levels_from_slopes(F, W[0], slopes) == pytest.approx(W, rel=1e-13)
    assert levels_from_slopes(F, 0.01, [3.0, 0.0, -3.0]) == pytest.approx(
        [0.01, 0.039811, 0.039811, 0.0100002], abs=1e-6)
    with pytest.raises(ValueError, match='2 slopes for 3 segments'):
        levels_from_slopes(F, 0.01, [3.0, 0.0])
    with pytest.raises(ValueError, match='increase'):
        segment_slopes([20.0, 20.0, 80.0], [1.0, 1.0, 1.0])
    with pytest.raises(ValueError, match='positive and finite'):
        segment_slopes(F, [0.01, 0.0, 0.04, 0.01])
    with pytest.raises(ValueError, match='at least two'):
        read_breakpoints([20.0], [0.01])


def test_a_typed_slope_holds_every_slope_after_it():
    after = levels_with_slope(F, W, 0, 2 * OCTAVE_DB)
    assert after[0] == W[0], 'the levels before it, exactly'
    assert after[1] == pytest.approx(0.16, rel=1e-13), 'two octaves at +6.02'
    assert segment_slopes(F, after)[1:] == pytest.approx(segment_slopes(F, W)[1:])
    assert after == pytest.approx([0.01, 0.16, 0.16, 0.04], rel=1e-13)
    middle = levels_with_slope(F, W, 2, -2 * OCTAVE_DB)
    assert list(middle[:3]) == W[:3]
    assert middle[3] == pytest.approx(0.0025, rel=1e-13)
    with pytest.raises(ValueError, match='no segment 4'):
        levels_with_slope(F, W, 3, 3.0)


def test_a_typed_level_holds_every_slope_after_it():
    after = levels_holding_slopes(F, W, 1, 0.08)
    assert after == pytest.approx([0.01, 0.08, 0.08, 0.02], rel=1e-13)
    assert after[1] == 0.08, 'exactly as typed'
    assert segment_slopes(F, after)[1:] == pytest.approx(segment_slopes(F, W)[1:])
    whole = levels_holding_slopes(F, W, 0, 0.02)
    assert whole == pytest.approx([0.02, 0.08, 0.08, 0.02], rel=1e-13)
    with pytest.raises(ValueError, match='no breakpoint 5'):
        levels_holding_slopes(F, W, 4, 0.1)
    with pytest.raises(ValueError, match='positive'):
        levels_holding_slopes(F, W, 1, -1.0)


def test_a_table_entered_by_slope_is_the_table():
    draft = _sheet([20.0, 80.0], [0.01, 0.04])
    draft = draft.with_slope_breakpoint(500.0, 0.0)
    draft = draft.with_slope_breakpoint(2000.0, -OCTAVE_DB)
    assert draft.frequencies == F
    assert [v / G ** 2 for v in draft.levels[0]] == pytest.approx(W, rel=1e-13)
    assert draft.slopes_db()[0] == pytest.approx([OCTAVE_DB, 0.0, -OCTAVE_DB])
    reading = draft.reading('1Z+')
    assert reading.areas / G ** 2 == pytest.approx(AREAS, rel=1e-12)
    assert reading.rms / G == pytest.approx(6.7842, abs=1e-4)
    # the specification made from it carries the same rms, by the
    # same closed form
    assert specification_rms(draft.make()) / G == pytest.approx(reading.rms / G,
                                                                rel=1e-12)
    # three segments, three bands each
    assert len(draft.bands[0]['warning']) == 3
    # the documents' 3 dB rather than 3.0103: 0.01005 at the top
    typed = (_sheet([20.0, 80.0], [0.01, 0.04])
             .with_slope_breakpoint(500.0, 0.0)
             .with_slope_breakpoint(2000.0, -3.0))
    assert typed.levels[0][-1] / G ** 2 == pytest.approx(0.0100475, abs=1e-7)


def test_a_slope_breakpoint_inside_a_segment_splits_it_and_holds_the_rest():
    draft = _sheet()
    split = draft.with_slope_breakpoint(1000.0, -2 * OCTAVE_DB)
    assert split.frequencies == [20.0, 80.0, 500.0, 1000.0, 2000.0]
    # the lower half at the typed slope: 0.04 falls 6.02 dB in an octave
    assert split.levels[0][3] / G ** 2 == pytest.approx(0.01, rel=1e-13)
    # the upper half keeps the slope the segment had, -3.0103
    assert split.slopes_db()[0][3] == pytest.approx(-OCTAVE_DB)
    assert split.levels[0][4] / G ** 2 == pytest.approx(0.005, rel=1e-13)
    assert len(split.bands[0]['abort']) == 4, 'both halves wear its band'
    # at a frequency already a breakpoint, the slope into it is typed
    again = draft.with_slope_breakpoint(80.0, 2 * OCTAVE_DB)
    assert again.frequencies == F
    assert again.levels[0][1] / G ** 2 == pytest.approx(0.16, rel=1e-13)
    with pytest.raises(ValueError, match='not above the first breakpoint'):
        draft.with_slope_breakpoint(20.0, 3.0)
    with pytest.raises(ValueError, match='not above the first breakpoint'):
        draft.with_slope_breakpoint(10.0, 3.0)


def test_the_channels_named_take_the_slope_and_the_others_the_power_law():
    draft = _sheet(channels=('1X+', '1Y+'))
    out = draft.with_slope_breakpoint(4000.0, -2 * OCTAVE_DB, channels=['1X+'])
    assert out.levels[0][-1] / G ** 2 == pytest.approx(0.0025, rel=1e-13)
    assert out.levels[1][-1] == out.levels[1][-2], 'past the end: flat'
    sloped = draft.with_slope(0, 0.0, channels=['1Y+'])
    assert sloped.levels[0] == draft.levels[0]
    assert sloped.levels[1][1] == pytest.approx(draft.levels[1][0])
    with pytest.raises(ValueError, match="no channel '9Z\\+'"):
        draft.with_slope(0, 3.0, channels=['9Z+'])


def test_the_entry_decides_what_a_typed_level_holds():
    draft = _sheet()
    assert draft.entry == 'levels'
    alone = draft.with_level(1, 0.08 * G * G)
    assert [v / G ** 2 for v in alone.levels[0]] == pytest.approx(
        [0.01, 0.08, 0.04, 0.01])
    by_slope = draft.with_entry('slopes')
    assert by_slope.entry == 'slopes' and draft.entry == 'levels'
    assert by_slope.levels == draft.levels, 'the requirement is the same'
    held = by_slope.with_level(1, 0.08 * G * G)
    assert [v / G ** 2 for v in held.levels[0]] == pytest.approx(
        [0.01, 0.08, 0.08, 0.02])
    assert held.entry == 'slopes', 'an operation keeps the entry'
    assert np.allclose(np.real(by_slope.make().ordinate),
                       np.real(draft.make().ordinate)), \
        'the entry changes nothing in the specification made'
    with pytest.raises(ValueError, match="'levels' or 'slopes'"):
        draft.with_entry('decibels')
    with pytest.raises(ValueError, match='no breakpoint 9'):
        draft.with_level(8, 1.0)
    with pytest.raises(ValueError, match='no channel 3'):
        draft.reading(2)


def test_the_entry_rides_the_dict_form():
    draft = _sheet().with_entry('slopes')
    data = draft.as_dict()
    assert data['entry'] == 'slopes'
    assert SpecificationDraft.from_dict(data) == draft
    del data['entry']
    assert SpecificationDraft.from_dict(data).entry == 'levels', \
        'a record written before the entry existed reads as levels'


CONTROLS = ['101X+', '101Y+', '101Z+', '102X+', '102Y+', '102Z+']
