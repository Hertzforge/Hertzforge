"""The overload check: each channel of a record read against its range.

Pinned against numbers that do not depend on the implementation: the
range a chain reads to (10 V over 100 mV/g is 100 g), the decibel
(20 log10 of the range over the peak), and a worked example: 12 seconds
of the flat 0.04 g^2/Hz profile from 20 to 2,000 Hz at 5,120 samples a
second, 8.88 grms with its largest peak at 37 g. At minus 6 dB its
largest peak is 18.49 g, 2.62 dB under a 25 g range, and scaled up 6 dB
that is 37 g, past it. Through the 25 g range at full level it reads
287 samples on the rail, 0.47 %, about 24 a second, a crest factor of
2.83 and a kurtosis of 2.86, which the kurtosis bars' band of 2 to 4
passes and this check does not. The range is read from a channel table:
10 V over 400 mV/g. The level a channel clips at is the table's
`clip_ranges()`, the smaller of its DAQ Range and its Sensor Range: a
0.5 V input over 200 mV/g is a 2.5 g range, and a 10 V input feeding a
sensor whose own output stops at 5 V is a 5 V one.
"""

from __future__ import annotations

import math

import numpy as np
import pytest

from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.data import TimeHistory
from hertzforge.core.kurtosis import HIGH, LOW, channel_kurtosis
from hertzforge.core.overload import (
    OVERLOADS,
    STEP_DB,
    channel_overload,
    headroom_rows,
    notes_for,
    range_in_unit,
    table_full_scales,
)

G = 9.80665

RATE = 5120.0


def flat_random(seconds, rate, seed, low=20.0, high=2000.0, level=0.04):
    """A Gaussian record in g whose one-sided ASD is `level` g^2/Hz from
    `low` to `high`: white noise shaped line by line in frequency."""
    n = round(seconds * rate)
    spectrum = np.fft.rfft(np.random.default_rng(seed).standard_normal(n))
    f = np.fft.rfftfreq(n, 1.0 / rate)
    # white noise of unit variance has a one-sided ASD of 2/rate
    gain = np.where((f >= low) & (f <= high),
                    np.sqrt(level / (2.0 / rate)), 0.0)
    return np.fft.irfft(spectrum * gain, n)


@pytest.fixture(scope='module')
def clean():
    """The worked example's record at full level: its largest peak 37 g."""
    return flat_random(12.0, RATE, 1403)


def history(*rows, dofs=None):
    rows = np.vstack(rows) * G
    count = len(rows)
    return TimeHistory(np.arange(rows.shape[1]) / RATE, rows,
                       response_dof=dofs or [f'{k + 1}Z+'
                                             for k in range(count)],
                       ordinate_dim=['acceleration'] * count,
                       ordinate_unit=['g'] * count)


def table(nodes, ranges, sensitivities, units=None, types=None,
          sensors=None):
    """A channel table with `ranges` its DAQ Range and `sensors`, when
    given, its Sensor Range, both in volts."""
    count = len(nodes)
    return ChannelTable({
        'channel': list(range(1, count + 1)), 'node': list(nodes),
        'direction': ['Z+'] * count,
        'unit': units or ['g'] * count,
        'channel_type': types or ['acceleration'] * count,
        'sensitivity': list(sensitivities),
        'daq_range': [str(value) for value in ranges],
        'sensor_range': [str(value) for value in sensors or [''] * count]})


@pytest.fixture(scope='module')
def run(clean):
    """Three channels: the record at minus 6 dB, the record through a
    25 g range at full level, and the record at minus 12 dB."""
    return history(clean / 2.0, np.clip(clean, -25.0, 25.0), clean / 4.0)


@pytest.fixture(scope='module')
def channels():
    """Each channel 10 V over 400 mV/g: a 25 g range, on no default."""
    return table([1, 2, 3], [10, 10, 10], [400, 400, 400])


def test_a_10_volt_input_on_100_millivolts_a_g_reads_to_100_g():
    assert range_in_unit(10, 100.0) == 100.0
    assert range_in_unit(5, 200.0) == 25.0


def test_a_sensitivity_that_is_not_a_number_gives_no_range():
    assert math.isnan(range_in_unit(5, float('nan')))
    assert math.isnan(range_in_unit(5, 0.0))


def test_the_tables_range_is_read_in_si(run, channels):
    scales, reasons, notes = table_full_scales(channels, run)
    assert scales == pytest.approx([25.0 * G] * 3)
    assert reasons == ['', '', '']
    assert notes == [''] * 3


def test_a_range_in_si_units_is_its_volts_over_its_sensitivity(run):
    # 10 V over 51 mV per m/s^2 is 196.08 m/s^2
    scales = table_full_scales(
        table([1, 2, 3], [10, 10, 10], [51.0] * 3, units=['m/s^2'] * 3),
        run).full_scales
    assert scales == pytest.approx([10000.0 / 51.0] * 3)


def test_each_channel_without_a_range_says_why(run):
    described = table([1, 2, 7], [5, 5, 5], [200.0, float('nan'), 200.0],
                      units=['g', 'g', 'g'])
    scales, reasons, _notes = table_full_scales(described, run)
    assert scales[0] == pytest.approx(25.0 * G)
    assert np.isnan(scales[1:]).all()
    assert reasons == ['', 'no sensitivity in the channel table',
                       'not in the channel table']


def test_a_unit_of_another_quantity_gives_no_range(run):
    described = table([1, 2, 3], [5, 5, 5], [200.0] * 3,
                      units=['g', 'lbf', 'g'])
    scales, reasons, _notes = table_full_scales(described, run)
    assert np.isnan(scales[1])
    assert reasons[1] == "the table's unit 'lbf' is not acceleration"


def test_a_row_with_no_unit_says_so(run):
    """A blank unit is named as blank, not as a unit of another quantity
    (the guard before the unit is converted)."""
    described = table([1, 2, 3], [10, 10, 10], [400.0] * 3,
                      units=['g', '', 'g'])
    scales, reasons, _notes = table_full_scales(described, run)
    assert np.isnan(scales[1])
    assert reasons == ['', 'no unit in the channel table', '']


def test_a_row_with_no_type_names_the_one_channel_at_its_dof(run):
    # a blank unit too, or the table reads the type off the unit
    described = table([1, 2, 3], [5, 5, 5], [200.0] * 3,
                      units=['', '', ''], types=['', '', ''])
    assert described.types() == ['', '', '']
    assert described.identities(run) == [
        ('1Z+', 'acceleration'), ('2Z+', 'acceleration'),
        ('3Z+', 'acceleration')]
    assert table_full_scales(described, run).reasons == [
        'no unit in the channel table'] * 3


def test_a_row_with_no_type_names_no_channel_where_two_share_its_dof(clean):
    """With acceleration and force both at 1Z+, a row naming 1Z+ with no
    type names neither: there is no one channel there to take."""
    record = TimeHistory(np.arange(clean.size) / RATE,
                         np.vstack([clean, clean]) * G,
                         response_dof=['1Z+', '1Z+'],
                         ordinate_dim=['acceleration', 'force'],
                         ordinate_unit=['g', 'N'])
    # a blank unit too, or the table reads the type off the unit
    described = table([1], [10], [400.0], units=[''], types=[''])
    assert described.types() == ['']
    assert described.identities(record) == [None]
    found = table_full_scales(described, record)
    assert np.isnan(found.full_scales).all()
    assert found.reasons == ['not in the channel table'] * 2


def test_two_rows_naming_one_channel_must_agree(run):
    """Rows 2 and 4 both name 2Z+. Alike, the range is read and a note
    says which rows; different, there is no range, and the reason says
    why, rather than the first row winning unsaid."""
    alike = table([1, 2, 3, 2], [10, 10, 10, 5], [400.0, 400.0, 400.0, 200.0])
    found = table_full_scales(alike, run)
    assert found.full_scales == pytest.approx([25.0 * G] * 3)
    assert found.notes == ['', ('rows 2 and 4 of the channel table name '
                                'it, with the same range'), '']
    check = run.overload_check(alike)
    assert check.channels[1].note.startswith('rows 2 and 4')
    assert check.channels[1].describe('g', G).endswith(
        'Rows 2 and 4 of the channel table name it, with the same range.')
    differ = table([1, 2, 3, 2], [10, 10, 10, 10], [400.0] * 3 + [200.0])
    found = table_full_scales(differ, run)
    assert np.isnan(found.full_scales[1])
    assert found.reasons[1] == ('rows 2 and 4 of the channel table name it '
                                'with different ranges')
    assert not run.overload_check(differ).channels[1].has_range


def test_the_range_is_where_the_channel_clips(clean):
    """The range is `ChannelTable.clip_ranges()` over the sensitivity: a
    0.5 V input over 200 mV/g reads to 2.5 g, so a channel clipped at
    2.5 g is read against 2.5 g and is on its rail, not against a range
    it never had. A 10 V input feeding a sensor whose own output stops at
    5 V clips at 5 V, 25 g over 200 mV/g; the input alone would say 50."""
    record = history(np.clip(clean, -2.5, 2.5), clean / 4.0)
    described = table([1, 2], [0.5, 10.0], [200.0, 200.0],
                      sensors=['', 5.0])
    assert described.clip_ranges().tolist() == [0.5, 5.0]
    found = table_full_scales(described, record)
    assert found.full_scales == pytest.approx([2.5 * G, 25.0 * G])
    clipped, quiet = record.overload_check(described).channels
    assert clipped.headroom_db == pytest.approx(0.0, abs=0.01)
    assert 'rail' in clipped.flagged
    assert quiet.full_scale == pytest.approx(25.0 * G)


def test_a_row_stating_neither_range_gives_none_and_says_so(clean):
    record = history(clean / 4.0)
    check = record.overload_check(table([1], [''], [200.0]))
    channel = check.channels[0]
    assert not channel.has_range
    assert channel.reason == ('no range in the channel table: neither its '
                              'DAQ Range nor its Sensor Range is stated')
    assert channel.describe('g', G) == (
        '1Z+: no range (no range in the channel table: neither its DAQ '
        'Range nor its Sensor Range is stated).')


def test_the_tables_rows_name_the_historys_channels(run, channels):
    assert channels.identities(run) == [('1Z+', 'acceleration'),
                                        ('2Z+', 'acceleration'),
                                        ('3Z+', 'acceleration')]
    other = table([9], [5], [200.0])
    assert other.identities(run) == [None]
    assert other.identities() == [('9Z+', 'acceleration')]


def test_the_clipped_channel_reads_the_worked_examples_numbers(run, channels):
    check = run.overload_check(channels)
    clipped = check.channels[1]
    assert clipped.label == '2Z+'
    assert clipped.rail == 287
    assert 100 * clipped.rail_share == pytest.approx(0.47, abs=0.005)
    assert clipped.rail_rate == pytest.approx(287 / 12.0)
    assert round(clipped.rail_rate) == 24
    assert clipped.crest == pytest.approx(2.83, abs=0.005)
    assert clipped.headroom_db == pytest.approx(0.0, abs=1e-9)
    assert clipped.flagged == ['rail']


def test_the_kurtosis_band_passes_the_clip_this_check_catches(run, channels):
    check = run.overload_check(channels)
    clipped = check.channels[1]
    assert clipped.statistics.kurtosis == pytest.approx(2.86, abs=0.005)
    assert LOW <= clipped.statistics.kurtosis <= HIGH
    assert 'kurtosis' not in clipped.statistics.flagged
    assert 'rail' in clipped.flagged


def test_the_kurtosis_is_the_kurtosis_bars(run, channels):
    check = run.overload_check(channels)
    bars = dict(channel_kurtosis(run))
    for channel in check.channels:
        assert channel.statistics.kurtosis == bars[channel.label]


def test_minus_6_db_reaches_its_range_at_the_next_level(run, channels):
    low = run.overload_check(channels).channels[0]
    assert low.peak / G == pytest.approx(18.49, abs=0.005)
    assert low.headroom_db == pytest.approx(20 * math.log10(25.0 / 18.4949),
                                            abs=1e-3)
    assert low.headroom_db == pytest.approx(2.62, abs=0.005)
    # 6 dB up is 1.995 times, about double, against 25 g
    assert low.next_peak / G == pytest.approx(18.4949 * 10 ** 0.3, abs=1e-3)
    assert low.next_peak / G == pytest.approx(36.90, abs=0.005)
    assert low.next_headroom_db == pytest.approx(low.headroom_db - 6.0)
    assert low.rail == 0
    assert low.flagged == ['next']


def test_a_smaller_step_holds(run, channels):
    low = run.overload_check(channels, step_db=2.0).channels[0]
    assert low.next_peak / G == pytest.approx(18.4949 * 10 ** 0.1, abs=1e-3)
    assert low.flagged == []


def test_minus_12_db_holds_the_next_level_too(run, channels):
    quiet = run.overload_check(channels).channels[2]
    assert quiet.headroom_db == pytest.approx(2.62 + 6.02, abs=0.01)
    assert quiet.flagged == []
    assert quiet.describe('g', G) == (
        '3Z+: 8.6 dB of headroom, 2.6 dB at +6 dB.')


def test_the_judgment_names_each_channel_for_what_it_is(run, channels):
    check = run.overload_check(channels)
    assert check.describe() == ('Of 3 channels: 1 on the rail (2Z+); '
                                '1 reaches its range at +6 dB (1Z+).')
    assert [channel.label for channel in check.flagged] == ['1Z+', '2Z+']
    assert check.worst.label == '2Z+'


def test_the_words_state_the_numbers_in_the_units_asked(run, channels):
    check = run.overload_check(channels)
    assert check.channels[1].describe('g', G) == (
        '2Z+: 287 samples on the rail (0.47 %, 23.9 a second), crest '
        'factor 2.83.')
    assert check.channels[0].describe('g', G) == (
        '1Z+: 2.6 dB of headroom; +6 dB puts its peak at 36.9 g against '
        'a range of 25 g.')


def test_without_the_worst_on_the_rail_the_least_headroom_is_worst(clean):
    record = history(clean / 4.0, clean / 2.0)
    check = record.overload_check(25.0 * G)
    assert check.worst.label == '2Z+'


def test_every_channel_inside_its_range_is_said_so(clean):
    check = history(clean / 4.0).overload_check(25.0 * G)
    assert check.describe() == ('All 1 channel inside their ranges, at '
                                '+6 dB as well.')


def test_samples_beyond_the_range_say_the_table_is_wrong(clean):
    # 2 V over 100 mV/g is 20 g, and the record holds samples to 25 g
    record = history(np.clip(clean, -25.0, 25.0))
    check = record.overload_check(table([1], [2], [100.0]))
    channel = check.channels[0]
    assert channel.full_scale == pytest.approx(20.0 * G)
    assert channel.beyond == int(np.count_nonzero(
        np.abs(np.clip(clean, -25.0, 25.0)) > 1.001 * 20.0))
    assert channel.beyond > 0
    assert channel.flagged == ['beyond', 'rail']
    assert 'not the one the record was taken with' in channel.describe()
    assert check.describe().startswith('Of 1 channel: 1 beyond its range')


def test_flat_tops_inside_the_range_are_left_to_the_statistics(clean):
    # the sensor clipped at 20 g on an input that reads to 50: the
    # range held, and the flat tops are the time statistics' to judge
    record = history(np.clip(clean, -20.0, 20.0))
    channel = record.overload_check(50.0 * G).channels[0]
    assert channel.rail == 0
    assert channel.flagged == []
    assert 'at_peak' in channel.statistics.flagged


def test_the_rail_is_judged_as_the_statistics_judge_it(run, channels):
    allowed = run.overload_check(channels, rail_samples=300).channels[1]
    assert allowed.rail == 287
    assert allowed.flagged == []
    assert 'rail' not in allowed.statistics.flagged


def test_a_channel_with_no_range_is_not_judged(run):
    check = run.overload_check([25.0 * G, None, float('nan')])
    assert [channel.has_range for channel in check.channels] == [
        True, False, False]
    assert check.channels[1].reason == 'no range given'
    assert check.channels[1].flags() == {}
    assert check.describe().endswith('2 channels with no range.')
    assert notes_for(check) == {1: 'no range: no range given',
                                2: 'no range: no range given'}
    rows = headroom_rows(check)
    assert rows[0][0] == '1Z+' and np.isnan(rows[1][1])


def test_one_range_serves_every_channel(run):
    check = run.overload_check(25.0 * G)
    assert [channel.full_scale for channel in check.channels] == [
        25.0 * G] * 3


def test_some_channels_can_be_read_alone(run):
    check = run.overload_check(25.0 * G, records=[2, 0])
    assert [channel.record for channel in check.channels] == [2, 0]


@pytest.mark.parametrize('ranges, options, match', [
    ([1.0, 2.0], {}, '2 ranges for 3 channels'),
    (-1.0, {}, 'has to be above zero'),
    (25.0, {'margin': 1.5}, 'margin is a share'),
    (25.0, {'step_db': float('nan')}, 'number of dB'),
    (25.0, {'records': [3]}, 'no channel 3'),
])
def test_a_setting_that_says_nothing_is_refused(run, ranges, options, match):
    with pytest.raises(ValueError, match=match):
        run.overload_check(ranges, **options)


def test_a_dead_channel_says_why_and_is_not_judged(clean):
    dead = np.full(clean.size, np.nan)
    check = history(clean / 4.0, dead).overload_check(25.0 * G)
    assert check.channels[1].statistics is None
    assert 'two finite samples' in check.channels[1].reason
    assert check.channels[1].flagged == []


def test_a_plain_array_is_read_the_same(clean):
    clipped = np.clip(clean, -25.0, 25.0)
    channel = channel_overload(clipped, RATE, 25.0, label='clipped')
    assert channel.rail == 287
    assert channel.flagged == ['rail']
    assert channel.headroom_db == pytest.approx(0.0, abs=1e-9)
    unranged = channel_overload(clipped, RATE, None)
    assert unranged.flags() == {} and unranged.reason == 'no range given'
    with pytest.raises(ValueError, match='above zero'):
        channel_overload(clipped, RATE, 0.0)


def test_every_flag_has_a_name():
    assert list(OVERLOADS) == ['beyond', 'rail', 'next']
    assert STEP_DB == 6.0


def test_the_project_finds_the_table_that_describes_a_record(run, channels):
    from hertzforge.project import Project

    project = Project()
    project.add('Run', run)
    project.add('Channels', channels)
    project.add('Other', table([9], [5], [200.0]))
    assert project.channel_table_for('Run') is None
    project.link('Run', 'Other')
    assert project.channel_table_for('Run') is None, (
        'a table naming none of its channels describes another recording')
    project.link('Run', 'Channels')
    name, found = project.channel_table_for('Run')
    assert name == 'Channels' and found is channels
    assert project.channel_table_for('Channels') is None


def test_a_channel_of_zeros_is_not_read_and_never_passes(clean):
    """Zero throughout has infinite headroom, which would read as a
    channel well inside its range: it is not read, and says why."""
    check = history(clean / 4.0, np.zeros(clean.size)).overload_check(
        25.0 * G)
    dead = check.channels[1]
    assert dead.statistics is None and not dead.has_range
    assert dead.flagged == [] and dead.reason.startswith('zero throughout')
    assert dead.describe() == ('2Z+: not read (zero throughout: a dead '
                               'channel has no peak to read against its '
                               'range).')
    assert check.describe() == (
        'All 1 channel inside their ranges, at +6 dB as well. Not read: '
        '2Z+, zero throughout: a dead channel has no peak to read against '
        'its range.')
    assert notes_for(check) == {1: 'not read: zero throughout: a dead '
                                   'channel has no peak to read against its '
                                   'range'}
    alone = channel_overload(np.zeros(64), RATE, 25.0)
    assert alone.statistics is None and alone.flags() == {}


def test_one_sample_not_finite_is_said_and_left_out(clean):
    holed = clean / 4.0
    holed[1000] = np.nan
    check = history(holed).overload_check(25.0 * G)
    channel, = check.channels
    assert channel.statistics.nonfinite == 1 and channel.has_range
    assert '1 sample is not finite and left out' in channel.cautions()
    assert channel.describe().endswith(' 1 sample is not finite and left '
                                       'out.')
    assert check.describe().endswith(
        'Samples not finite, left out: 1Z+ 1.')


def test_an_empty_selection_is_refused(run):
    with pytest.raises(ValueError, match='no channel asked for'):
        run.overload_check(25.0 * G, records=[])


def test_a_record_whose_clock_starts_late_reads_the_same(run):
    late = TimeHistory(np.asarray(run.abscissa) + 3.25, run.ordinate,
                       response_dof=list(run.response_dof),
                       ordinate_dim=list(run.ordinate_dim),
                       ordinate_unit=['g'] * 3)
    early, later = run.overload_check(25.0 * G), late.overload_check(25.0 * G)
    assert later.describe() == early.describe()
    assert [c.headroom_db for c in later.channels] == pytest.approx(
        [c.headroom_db for c in early.channels])
    assert later.channels[1].rail == early.channels[1].rail == 287


def test_one_sample_beyond_and_one_on_the_rail_are_each_said_in_the_singular():
    values = np.zeros(4096)
    values[::7] = 1.0
    values[100] = 30.0                          # beyond a 25 range
    beyond = channel_overload(values * G, RATE, 25.0 * G, label='a')
    assert beyond.beyond == 1 and 'beyond' in beyond.flagged
    assert beyond.describe('g', G).startswith(
        'a: 1 sample beyond its range of 25 g, the largest 30 g')
    values[100] = 25.0                          # on the rail, allowed 5
    railed = channel_overload(values * G, RATE, 25.0 * G, label='b',
                              rail_samples=5)
    assert railed.rail == 1 and railed.flagged == []
    assert railed.describe('g', G).startswith('b: 1 sample on the rail')


def test_the_next_level_is_judged_against_the_range_less_its_margin():
    values = np.zeros(4096)
    values[::7] = 1.0
    values[3] = 4.5                             # at +6 dB, 8.98 of 10
    held = channel_overload(values, RATE, 10.0, margin=0.05)
    assert held.flags()['next'] is False
    near = channel_overload(values, RATE, 10.0, margin=0.2)
    assert near.flags()['next'] is True, '8.98 is past 10 less 20 %'


def test_a_sample_beyond_at_the_record_s_first_sample_is_counted():
    values = np.zeros(4096)
    values[::7] = 1.0
    values[0] = 30.0
    assert channel_overload(values, RATE, 25.0,
                            spans=[(0, 4096)]).beyond == 1
    assert channel_overload(values, RATE, None).beyond == 0


def test_a_channel_not_read_has_no_rms_and_no_caution(clean):
    check = history(clean / 4.0, np.zeros(clean.size)).overload_check(
        25.0 * G)
    live, dead = check.channels
    assert live.rms == pytest.approx(live.statistics.rms)
    assert np.isnan(dead.rms) and dead.cautions() == []
    assert channel_overload(clean, RATE, 25.0).record == 0


def test_the_worst_is_the_one_on_the_rail_before_the_one_nearest_its_range():
    railed = np.zeros(4096)
    railed[::7] = 1.0
    railed[50] = 25.0                           # one sample on the rail
    near = np.zeros(4096)
    near[::7] = 1.0
    near[60] = 24.0                             # nearer at the next level
    check = history(railed, near).overload_check(25.0 * G)
    assert check.channels[0].rail == 1 and check.channels[1].rail == 0
    assert check.worst.label == '1Z+'


def test_the_settings_hold_at_their_edges(run):
    assert run.overload_check(25.0 * G, margin=0.0).margin == 0.0
    with pytest.raises(ValueError, match='margin is a share'):
        run.overload_check(25.0 * G, margin=1.0)
    with pytest.raises(ValueError, match='has to be above zero'):
        run.overload_check([25.0 * G, 0.0, 25.0 * G])
    one = history(np.ones(64))
    with pytest.raises(ValueError, match='2 ranges for 1 channel:'):
        one.overload_check([1.0, 2.0])
    with pytest.raises(ValueError, match='the record has 1 channel; there '
                                         'is no channel 1'):
        one.overload_check(1.0, records=[1])


def test_the_step_moved_judges_the_readings_again_without_reading(run, channels):
    """Nothing read from the record depends on the step to the next
    level, so `with_step` keeps every reading and rejudges: the same
    answer as a check made with that step."""
    check = run.overload_check(channels, step_db=6.0)
    moved = check.with_step(9.0)
    fresh = run.overload_check(channels, step_db=9.0)
    assert moved.step_db == 9.0 and moved.margin == check.margin
    for a, b in zip(moved.channels, fresh.channels):
        assert a.statistics is b.statistics or (
            a.statistics.peak == b.statistics.peak)
        assert a.step_db == 9.0 and a.flags() == b.flags()
        assert a.next_headroom_db == pytest.approx(b.next_headroom_db)
    assert [c.record for c in moved.flagged] == [c.record for c in fresh.flagged]
