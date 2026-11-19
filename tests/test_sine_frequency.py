"""The sine frequency check: the tone's own frequency on the record
against the specification's.

The numbers are closed forms. A record written as cos(s * phi), phi the
specification's sweep argument, carries the tone at s times the
specified frequency at every instant, so its error reads s - 1 of the
frequency everywhere: 1.5 % for s = 1.015. A linear sweep specified at
100 Hz/s and run at 101 Hz/s from the same start is t Hz ahead t seconds
in. The records are sampled at 4096 Hz.
"""

from __future__ import annotations

from dataclasses import replace

import numpy as np
import pytest

from hertzforge.core.data import TimeHistory
from hertzforge.core.sine import (
    LOG,
    SineLevel,
    SineLevelSet,
    SineSweepSpecification,
    SineTone,
    extract_sine,
)
from hertzforge.core.sine_frequency import (
    FREQUENCY_TOLERANCE,
    allowed,
    check_frequency,
)

FS = 4096.0

AMPLITUDE = 2.0

CONTROL = '101Z+'

ONSET = 0.5


def _record(x, dofs=(CONTROL,), dims=('acceleration',)):
    x = np.atleast_2d(x)
    return TimeHistory(np.arange(x.shape[1]) / FS, x,
                       response_dof=list(dofs), ordinate_dim=list(dims),
                       ordinate_unit=['m/s**2'] * len(dofs))


def _tone(dwell=False, frequency=100.0):
    if dwell:
        return SineTone.dwell('Hold', frequency, [AMPLITUDE], 4.0,
                          start_time=ONSET)
    return SineTone('Up', ONSET, [100.0, 500.0], [[AMPLITUDE]] * 2, [0],
                    [100.0])


def _sweep(scale=1.0, dwell=False, frequency=100.0, noise=0.0, run=None,
           seconds=6.0, second_tone=False):
    """The tone recorded `scale` times its specified frequency (or along
    the tone `run`, when given), under white `noise`; with
    `second_tone`, a 1 m/s**2 tone at 1000 Hz alongside from 1 to 3 s."""
    tone = _tone(dwell, frequency)
    tones = [tone]
    if second_tone:
        tones.append(SineTone('High', 1.0, [1000.0, 1002.0], [[1.0]] * 2,
                              [0], [1.0]))
    spec = SineSweepSpecification(tones, [CONTROL], ordinate_unit='m/s**2')
    n = int(seconds * FS)
    x = np.random.default_rng(5).standard_normal(n) * noise
    played = run or tone
    argument = played.argument(1.0 / FS)
    start = int(ONSET * FS)
    m = min(len(argument), n - start)
    x[start:start + m] += AMPLITUDE * np.cos(scale * argument[:m])
    if second_tone:
        high = tones[1].argument(1.0 / FS)
        at = int(1.0 * FS)
        x[at:at + len(high)] += np.cos(high)
    return _record(x), spec


def _levels(history, spec, cycles=10.0, refine=True):
    onsets = {tone.name: tone.start_time for tone in spec.tones}
    return extract_sine(history, spec, cycles=cycles, workers=1,
                        onsets=onsets, refine=refine)


def _checked(tolerance=FREQUENCY_TOLERANCE, floor=0.0, **kwargs):
    history, spec = _sweep(**kwargs)
    levels = _levels(history, spec)
    return (check_frequency(history, spec, levels, tolerance=tolerance,
                            floor=floor), history, spec, levels)


def test_the_band_is_a_share_of_the_frequency_or_a_floor():
    assert FREQUENCY_TOLERANCE == 0.02
    assert allowed(100.0) == pytest.approx(2.0)
    assert allowed([20.0, 100.0], 0.02, 1.0).tolist() == \
        pytest.approx([1.0, 2.0]), '2 % or 1 Hz, whichever is greater'
    assert allowed(500.0, 0.0125) == pytest.approx(6.25)
    for tolerance, floor, match in ((0.0, 0.0, 'more than zero'),
                                    (np.nan, 0.0, 'more than zero'),
                                    (0.02, -1.0, 'zero or more'),
                                    (0.02, np.inf, 'zero or more')):
        with pytest.raises(ValueError, match=match):
            allowed(100.0, tolerance, floor)


def test_a_tone_on_its_frequency_reads_no_error_and_passes():
    check, *_rest = _checked()
    assert check.judged.all() and check.passed
    assert np.max(np.abs(check.relative)) < 1e-6
    assert check.notes == ()
    assert check.tone == 'Up' and check.dofs == [CONTROL]
    assert check.cycles == 10.0, "read over the extraction's own smoothing"
    assert check.specified == pytest.approx(check.frequency, rel=2e-3), \
        'the span is centered on the line, so its mean is near the line'


def test_a_tone_off_its_frequency_reads_its_own_share():
    check, *_rest = _checked(scale=1.015, tolerance=0.0125)
    assert check.relative[0] == pytest.approx(0.015, abs=2e-5)
    assert check.error[0] == pytest.approx(0.015 * check.specified, rel=2e-3)
    assert check.over.all() and not check.passed
    assert any("moved this sweep's clock by +7.5 Hz" in note
               for note in check.notes), \
        'the extraction corrected the clock, and the check says it counted it'
    assert check.describe().endswith(
        f'{check.judged.sum()} of {check.judged.sum()} lines outside ±1.25 %')
    looser = replace(check, tolerance=0.02)
    assert looser.passed and not looser.over.any()
    assert looser.describe().endswith('within ±2 %')
    below, *_rest = _checked(scale=0.985)
    assert below.relative[0] == pytest.approx(-0.015, abs=2e-5), \
        'a tone below its frequency reads below'


def test_a_dwell_held_off_its_frequency_is_read_off_it():
    check, *_rest = _checked(dwell=True, scale=1.015, tolerance=0.0125)
    assert check.error[0] == pytest.approx(1.5, abs=1e-3)
    assert not check.passed
    worst = check.worst()
    assert worst[1] == pytest.approx(1.5, abs=1e-3)
    assert worst[2] == pytest.approx(0.015, abs=1e-5)


def test_a_tone_that_steps_off_its_frequency_is_read_on_each_side():
    # held at 100 Hz for two seconds, then at 101.5 Hz: no smooth clock
    # correction follows a step, so it is the envelope that reads it
    history, spec = _sweep(dwell=True)
    n = history.ordinate.shape[1]
    t = np.arange(n) / FS - ONSET
    step = 2.0
    phase = 2.0 * np.pi * (100.0 * t + 1.5 * np.clip(t - step, 0.0, None))
    x = np.where((t >= 0.0) & (t < 4.0), AMPLITUDE * np.cos(phase), 0.0)
    history = _record(x)
    check = check_frequency(history, spec, _levels(history, spec),
                            tolerance=0.0125)
    when = check.seconds - ONSET
    before = (when > 0.3) & (when < step - 0.3)
    after = (when > step + 0.3) & (when < 3.7)
    assert check.error[0][before] == pytest.approx(0.0, abs=2e-3)
    assert check.error[0][after] == pytest.approx(1.5, abs=2e-3)
    assert not check.over[0][before].any() and check.over[0][after].all()


def test_a_tone_well_off_is_held_in_the_band_and_read_in_full():
    # 5 % off, well into the skirt of a ten-cycle band: read without the
    # turning taken into the argument, it reads short
    check, *_rest = _checked(dwell=True, scale=1.05)
    assert check.relative[0] == pytest.approx(0.05, abs=1e-4)


def test_a_floor_widens_the_band_at_low_frequency():
    # 1.5 % of 20 Hz is 0.3 Hz: outside 1 %, inside a 0.5 Hz floor
    check, *_rest = _checked(dwell=True, frequency=20.0, scale=1.015,
                             tolerance=0.01, floor=0.5)
    assert check.error[0] == pytest.approx(0.3, abs=1e-3)
    assert check.allowed == pytest.approx(0.5)
    assert check.passed
    assert check.band_words() == '±1 % or 0.5 Hz, whichever is greater'
    assert not replace(check, floor=0.0).passed


def test_a_sweep_run_at_the_wrong_rate_runs_ahead_of_its_schedule():
    fast = SineTone('Up', ONSET, [100.0, 500.0], [[AMPLITUDE]] * 2, [0],
                    [101.0])
    check, *_rest = _checked(run=fast)
    # t seconds into the sweep the tone is t Hz ahead of the schedule
    t = check.seconds - ONSET
    inside = (t > 0.2) & (t < 3.8)
    assert check.error[0][inside] == pytest.approx(t[inside], abs=0.02)


def test_noise_on_the_channel_scatters_the_reading_a_little():
    check, *_rest = _checked(noise=0.5)
    assert check.passed
    assert np.max(np.abs(check.relative)) < 0.003


def test_a_tone_alongside_is_rebuilt_with_it_and_reads_clean():
    check, *_rest = _checked(second_tone=True)
    assert check.judged.all()
    assert np.max(np.abs(check.relative)) < 1e-4


def test_a_tone_the_levels_do_not_hold_leaves_its_lines_unjudged():
    history, spec = _sweep(second_tone=True)
    levels = _levels(history, spec)
    only = SineLevelSet([levels.tone('Up')], cycles=levels.cycles)
    check = check_frequency(history, spec, only)
    shared = ~check.judged[0]
    assert shared.any() and not shared.all()
    assert any('shared with a tone the levels do not hold' in note
               for note in check.notes)
    assert check.passed, 'the lines judged are on frequency'


def test_a_line_under_the_noise_floor_is_not_judged():
    check, history, spec, levels = _checked()
    level = levels.levels[0]
    below = np.zeros_like(level.below_floor)
    below[0, :10] = True
    marked = SineLevelSet([SineLevel(
        abscissa=level.abscissa, ordinate=level.ordinate,
        response_dof=level.response_dof, ordinate_dim=level.ordinate_dim,
        ordinate_unit=level.ordinate_unit, tone=level.tone,
        onset=level.onset, seconds=level.seconds, floor=level.floor,
        below_floor=below)], cycles=levels.cycles)
    check = check_frequency(history, spec, marked)
    assert check.judged[0].sum() == len(check.frequency) - 10
    assert '10 lines under the noise floor, not judged' in check.notes


def test_without_levels_the_extraction_is_run_with_the_records_setting():
    from hertzforge.core.sine import SineExtraction

    history, spec = _sweep()
    history.sine_extraction = SineExtraction(cycles=10.0)
    check = check_frequency(history, spec)
    assert check.cycles == 10.0 and check.passed


def test_a_dead_channel_is_not_judged_and_does_not_pass():
    history, spec = _sweep()
    levels = _levels(history, spec)
    dead = _record(np.zeros(history.ordinate.shape[1]))
    check = check_frequency(dead, spec, levels)
    assert not check.judged.any() and not check.passed
    assert any('no tone on the channel to read' in note
               for note in check.notes)
    assert check.describe() == 'Up: no line could be judged'
    assert check.worst() is None


def test_a_sample_that_is_not_a_number_leaves_the_channel_unjudged():
    history, spec = _sweep()
    levels = _levels(history, spec)
    x = np.array(history.ordinate[0])
    x[int(2.0 * FS)] = np.nan
    check = check_frequency(_record(x), spec, levels)
    assert not check.judged.any() and not check.passed
    assert any('1 sample over the tone\'s stretch is not a finite number'
               in note for note in check.notes)


def test_a_channel_is_found_by_its_quantity_as_well_as_its_dof():
    history, spec = _sweep(scale=1.015)
    levels = _levels(history, spec)
    x = np.array(history.ordinate[0])
    both = _record(np.vstack([np.zeros_like(x), x]), dofs=(CONTROL, CONTROL),
                   dims=('force', 'acceleration'))
    check = check_frequency(both, spec, levels)
    assert check.relative[0] == pytest.approx(0.015, abs=2e-5), \
        'the acceleration record, not the force gauge at the same DOF'


def test_the_verdict_says_the_share_in_percent():
    check, *_rest = _checked(scale=1.015, tolerance=0.0125)
    assert check.describe().startswith('Up: worst +1.5 % (+'), \
        check.describe()


def test_each_channel_is_read_and_judged_on_its_own():
    # 101Z+ on its frequency, 102Z+ 1.5 % above it
    tone = SineTone('Up', ONSET, [100.0, 500.0],
                    [[AMPLITUDE, AMPLITUDE]] * 2, [0], [100.0])
    spec = SineSweepSpecification([tone], [CONTROL, '102Z+'],
                                  ordinate_unit='m/s**2')
    n = int(6.0 * FS)
    x = np.zeros((2, n))
    argument = tone.argument(1.0 / FS)
    start = int(ONSET * FS)
    m = min(len(argument), n - start)
    x[0, start:start + m] = AMPLITUDE * np.cos(argument[:m])
    x[1, start:start + m] = AMPLITUDE * np.cos(1.015 * argument[:m])
    history = _record(x, dofs=(CONTROL, '102Z+'),
                      dims=('acceleration', 'acceleration'))
    check = check_frequency(history, spec, _levels(history, spec),
                            tolerance=0.0125)
    assert check.dofs == [CONTROL, '102Z+']
    assert (check.row(), check.row(CONTROL), check.row('102Z+')) == (0, 0, 1)
    with pytest.raises(ValueError, match="103Z\\+ was not read; the check "
                                         "holds \\['101Z\\+', '102Z\\+'\\]"):
        check.row('103Z+')
    assert check.relative[0] == pytest.approx(0.0, abs=2e-5)
    assert check.relative[1] == pytest.approx(0.015, abs=2e-5)
    assert check.worst(CONTROL)[2] == pytest.approx(0.0, abs=2e-5)
    assert check.worst('102Z+')[2] == pytest.approx(0.015, abs=2e-5)
    assert check.worst()[2] == pytest.approx(0.015, abs=2e-5), \
        'with no channel named, the worst of any'
    assert not check.over[0].any() and check.over[1].all()
    assert check.describe(CONTROL).endswith('within ±1.25 %')
    assert check.describe('102Z+').startswith('Up at 102Z+: worst +1.5 %')


def test_a_record_off_zero_reads_the_same_frequency():
    history, spec = _sweep(dwell=True, frequency=20.0, scale=1.015)
    levels = _levels(history, spec)
    clean = check_frequency(history, spec, levels)
    x = np.array(history.ordinate[0]) + 50.0
    offset = check_frequency(_record(x), spec, levels)
    assert offset.judged.all()
    assert offset.error[0] == pytest.approx(clean.error[0], abs=1e-4)


def _log_sweep(low, high, rate, scale):
    """A log sweep from `low` to `high` Hz at `rate` octaves a minute,
    recorded `scale` times its specified frequency, and its levels."""
    tone = SineTone('Log', ONSET, [low, high], [[AMPLITUDE]] * 2, [LOG],
                    [rate])
    spec = SineSweepSpecification([tone], [CONTROL], ordinate_unit='m/s**2')
    n = int((ONSET + tone.duration() + 0.5) * FS)
    x = np.zeros(n)
    argument = tone.argument(1.0 / FS)
    start = int(ONSET * FS)
    m = min(len(argument), n - start)
    x[start:start + m] = AMPLITUDE * np.cos(scale * argument[:m])
    history = _record(x)
    return history, spec, _levels(history, spec)


@pytest.mark.parametrize('low,high,rate', [(20.0, 200.0, 8.0),
                                           (10.0, 1000.0, 4.0)])
def test_a_log_sweep_off_its_schedule_reads_its_own_share(low, high, rate):
    # A log sweep's departure from its schedule is no quadratic in time.
    # Fitted as one, the turning the fit missed was read through the
    # filter's band near the start: 1.35 to 1.49 % over the first second
    # of the 20 to 200 Hz sweep, and -20.7 to +3.2 % over the first lines
    # of the 10 to 1000 Hz one (2026-10-24). Every line is pinned here,
    # judged or not: the wide sweep's extraction leaves its first 174
    # lines under its noise floor, and they read 1.5 % all the same.
    history, spec, levels = _log_sweep(low, high, rate, 1.015)
    check = check_frequency(history, spec, levels)
    assert np.isfinite(check.relative[0]).all()
    assert check.relative[0] == pytest.approx(0.015, abs=1e-4)


def test_a_sweep_of_two_laws_recorded_late_reads_how_far_behind_it_is():
    # a log segment and then a linear one, recorded 0.2 s behind its
    # schedule: the tone reads the frequency the schedule had 0.2 s
    # earlier, up to 4 Hz below it. Late on any law, the envelope turns
    # by the delay times the angular frequency, which is a column of the
    # fit; without it the worst line here read 0.0043 Hz off rather than
    # 0.0011 (2026-10-24).
    tone = SineTone('Two', ONSET, [20.0, 100.0, 500.0], [[AMPLITUDE]] * 3,
                    [LOG, 0], [8.0, 20.0])
    spec = SineSweepSpecification([tone], [CONTROL], ordinate_unit='m/s**2')
    late = 0.2
    t = np.arange(int((ONSET + tone.duration() + 0.5) * FS)) / FS
    since = t - ONSET - late
    x = np.where((since >= 0.0) & (since < tone.duration()),
                 AMPLITUDE * np.cos(tone.phase_at(np.clip(since, 0.0, None))),
                 0.0)
    history = _record(x)
    levels = _levels(history, spec)
    check = check_frequency(history, spec, levels)
    when = check.seconds - ONSET
    expected = tone.frequency_at(when - late) - tone.frequency_at(when)
    # away from the ends, and from the breakpoint, where the frequency
    # turns a corner inside the span
    corner = float(np.interp(100.0, tone.frequency_at(when), when))
    away = ((when > 0.5) & (when < tone.duration() - 0.5)
            & (np.abs(when - corner) > 0.5))
    assert away.sum() > 0.8 * len(away)
    assert check.error[0][away] == pytest.approx(expected[away], abs=0.0025)


def test_what_cannot_be_checked_is_refused():
    history, spec = _sweep()
    levels = _levels(history, spec)
    with pytest.raises(ValueError, match='more than zero'):
        check_frequency(history, spec, levels, tolerance=0.0)
    with pytest.raises(ValueError, match='zero or more'):
        check_frequency(history, spec, levels, floor=-1.0)
    with pytest.raises(ValueError, match='the frequency is read over more '
                                         'than zero cycles'):
        check_frequency(history, spec, levels, cycles=0.0)
    with pytest.raises(ValueError, match="no tone 'Down'"):
        check_frequency(history, spec, levels, tone='Down')
    with pytest.raises(ValueError, match='no reading for'):
        check_frequency(history, spec, levels, channels=['9X+'])
    with pytest.raises(ValueError, match='less than the 5000 the frequency '
                                         'is read over'):
        check_frequency(history, spec, levels, cycles=5000.0)
    uneven = TimeHistory(np.cumsum(np.r_[0.0, np.full(99, 1e-3)] *
                                   np.r_[1.0, np.ones(98), 2.0]),
                         np.zeros((1, 100)), response_dof=[CONTROL],
                         ordinate_dim=['acceleration'],
                         ordinate_unit=['m/s**2'])
    with pytest.raises(ValueError, match='Checking sine frequency'):
        check_frequency(uneven, spec, levels)
