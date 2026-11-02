"""A shock synthesized to a shock response spectrum, checked against the
shaker it is to be played on.

The record is judged by the package's own readings, not by the
synthesis's word for itself: its spectrum computed again from the
record by `TimeHistory.compute_srs` and judged by
`compliance.srs_tolerance_all`, its travel, velocity and force by
`core.stroke.read_stroke`. The wavelet is checked against its integrals
worked by hand.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core import shock_synthesis, signals
from hertzforge.core.compliance import srs_tolerance_all
from hertzforge.core.data import Psd, ShockSpecification
from hertzforge.core.progress import Cancelled
from hertzforge.core.shaker import ShakerRating
from hertzforge.core.shock_synthesis import (
    MAX_PASSES,
    MOST_HALF_CYCLES,
    synthesize,
    wavelet,
)
from hertzforge.core.stroke import read_stroke

G = 9.80665

#: a quick requirement: a sixth of an octave from 50 Hz, a knee at
#: 400 Hz, 32 points, so a synthesis takes a few hundredths of a second
RATE = 10240.0


def _spec(levels=(20.0, 100.0, 100.0), dof='1Z+', **kwargs):
    kwargs.setdefault('per_octave', 6)
    kwargs.setdefault('warning', (-1.5, 3.0))
    kwargs.setdefault('abort', (-3.0, 6.0))
    return ShockSpecification.from_breakpoints(
        [50.0, 400.0, 2000.0], list(levels), dof, ordinate_unit='g', **kwargs)


def _integrated(row, rate):
    """Velocity and displacement by the trapezoid rule from rest."""
    v = np.concatenate([[0.0], np.cumsum(row[1:] + row[:-1]) / (2 * rate)])
    d = np.concatenate([[0.0], np.cumsum(v[1:] + v[:-1]) / (2 * rate)])
    return v, d


def test_a_wavelet_of_three_half_cycles_peaks_in_travel_at_its_middle():
    """By hand: a(t) = A sin(πt/T) sin(2πft) with T = 3/(2f) is half the
    difference of cos(2πt/T) and cos(4πt/T), so its displacement from
    rest is (A/2)[(1 - cos(2πt/T))/(2π/T)² - (1 - cos(4πt/T))/(4π/T)²],
    never below zero, largest at t = T/2, where it is A T²/(4π²) =
    9A/(16π²f²), and zero again at T, as is its velocity."""
    rate, f = 102400.0, 10.0
    m = round(3 * rate / (2 * f))
    row = wavelet(m, 3)
    v, d = _integrated(row, rate)
    peak = 9.0 / (16.0 * np.pi ** 2 * f ** 2)
    assert d[m // 2] == pytest.approx(peak, rel=1e-6)
    assert int(np.argmax(d)) == m // 2
    assert d.min() > -1e-15, 'one-sided: it never crosses rest'
    assert abs(v[-1]) < 1e-15 and abs(d[-1]) < 1e-15


def test_five_half_cycles_swing_both_ways_and_still_end_at_rest():
    rate = 20480.0
    for half_cycles, samples in ((5, 1024), (7, 999), (15, 77)):
        row = wavelet(samples, half_cycles)
        v, d = _integrated(row, rate)
        assert row[0] == 0.0 and abs(row[-1]) < 1e-12
        assert abs(row.sum()) < 1e-12, 'the trapezoid velocity ends at zero'
        assert abs(v[-1]) < 1e-15 * samples
        assert abs(d[-1]) < 1e-15 * samples, \
            'symmetric about its middle: the displacement ends at zero'
        assert d.min() < -0.1 * d.max(), 'it swings below rest'


def test_five_half_cycles_need_under_half_the_travel_of_three():
    """For the same spectrum at its own frequency, a wavelet of five half
    cycles travels under half as far from rest as one of three."""
    from hertzforge.core.srs import maximax

    rate, f = 20480.0, 10.0
    travel = {}
    for half_cycles in (3, 5):
        m = round(half_cycles * rate / (2 * f))
        row = np.concatenate([wavelet(m, half_cycles), np.zeros(4096)])
        spectrum = maximax(row, [f], rate)[0]
        _v, d = _integrated(row, rate)
        travel[half_cycles] = np.abs(d).max() / spectrum
    assert travel[5] < 0.5 * travel[3]
    assert travel[5] / travel[3] == pytest.approx(0.418, abs=0.005)


def test_an_even_or_too_short_wavelet_is_refused():
    with pytest.raises(ValueError, match='odd whole number, three or more'):
        wavelet(100, 4)
    with pytest.raises(ValueError, match='odd whole number'):
        wavelet(100, 1)
    with pytest.raises(ValueError, match='odd whole number'):
        wavelet(100, 3.5)
    with pytest.raises(ValueError, match='under two samples a half cycle'):
        wavelet(9, 5)
    assert wavelet(10, 5).size == 11


def test_the_record_meets_the_requirement_by_the_package_own_spectrum():
    spec = _spec()
    made = synthesize(spec, sample_rate=RATE)
    points = spec.abscissa
    srs = made.history.compute_srs(low=points[0], high=points[-1],
                                   per_octave=6)
    assert np.allclose(srs.abscissa, points, rtol=1e-12)
    assert np.allclose(srs.ordinate, made.srs.ordinate, rtol=1e-12), \
        "the synthesis's spectrum is the record's"
    rows = srs_tolerance_all(spec, srs)
    assert len(rows) == 1
    result = rows[0][2]
    assert result['passed'] is True
    assert result['inside_count'] == result['points'] == points.size, \
        'every point inside the inner band, the first and the last too'
    db = 20 * np.log10(srs.ordinate[0] / spec.ordinate[0])
    assert np.all(np.abs(db) < 1.0), 'the passes went on past the band edge'
    assert made.channels[0].worst_db == pytest.approx(np.abs(db).max())
    assert made.tolerance()[0][2]['inside_count'] == points.size


def test_the_record_starts_and_ends_at_rest_as_the_stroke_reading_reads_it():
    made = synthesize(_spec(), sample_rate=RATE)
    reading = read_stroke(made.history)
    assert reading.acceleration[0] == 0.0
    assert abs(reading.velocity[-1]) < 1e-12 * reading.peak_velocity
    assert abs(reading.displacement[-1]) < 1e-12 * reading.peak_to_peak
    channel = made.channels[0]
    assert np.allclose(channel.reading.displacement, reading.displacement)
    # the quiet tail: a period of the lowest point after the last wavelet
    tail = made.history.ordinate.shape[1] - 1 - channel.samples.max()
    assert tail == int(np.ceil(RATE / 50.0))


def test_wavelets_sit_on_whole_samples_near_their_points():
    made = synthesize(_spec(), sample_rate=RATE)
    c = made.channels[0]
    assert np.array_equal(c.frequency, c.half_cycles * RATE / (2 * c.samples))
    assert np.all(np.abs(c.frequency / c.points - 1.0) < 0.04)
    assert c.start.min() == 0, 'the longest opens the record'
    middles = c.start + c.samples / 2.0
    assert np.ptp(middles) <= 0.5, 'centered on one instant'


def test_twice_the_requirement_is_twice_the_record():
    one = synthesize(_spec(), sample_rate=RATE)
    two = synthesize(_spec(levels=(40.0, 200.0, 200.0)), sample_rate=RATE)
    assert np.allclose(two.history.ordinate, 2.0 * one.history.ordinate,
                       rtol=1e-9, atol=1e-9)
    assert two.channels[0].passes == one.channels[0].passes


def test_the_default_settings_meet_a_twelfth_octave_requirement():
    """The suggested settings on a requirement as written: 10 Hz to
    2 kHz, a twelfth of an octave, a knee at 100 Hz."""
    spec = ShockSpecification.from_breakpoints(
        [10.0, 100.0, 2000.0], [10.0, 100.0, 100.0], '1Z+',
        ordinate_unit='g', warning=(-1.5, 3.0), abort=(-3.0, 6.0))
    settings = signals.suggested(spec)
    # ten times the highest point, 1918 Hz, rounded up to 1024s
    assert settings.kind == 'srs' and settings.sample_rate == 19456.0
    assert settings.half_cycles == shock_synthesis.HALF_CYCLES == 5
    history = signals.generate(spec, settings)
    srs = history.compute_srs(low=10.0, high=spec.abscissa[-1])
    result = srs_tolerance_all(spec, srs)[0][2]
    assert result['inside_count'] == result['points'] == 92


def test_the_suggested_rate_reads_the_highest_written_point():
    from hertzforge.core.shock_synthesis import suggested_rate

    f = np.array([100.0, 1000.0, 4000.0])
    spec = ShockSpecification(f, [[10.0, 100.0, 0.0]], response_dof=['1Z+'],
                              ordinate_dim=['acceleration'])
    assert suggested_rate(spec) == 10240.0, '10 x 1000 Hz, not 4000 Hz'


def test_the_iteration_converges_and_says_how_fast():
    told = []
    made = synthesize(_spec(), sample_rate=RATE,
                      progress=lambda done, total: told.append((done, total)))
    passes = made.channels[0].passes
    assert 2 <= passes <= 12 < MAX_PASSES
    assert told[-1][0] == told[-1][1], 'the bar reaches its end'
    assert all(done <= total for done, total in told)


def test_the_passes_go_on_past_the_band_edge(monkeypatch):
    """Stopped on the first pass inside the band, the spectrum sits
    nearer the band's edge than when the passes go on while they gain."""
    went_on = synthesize(_spec(), sample_rate=RATE).channels[0]
    monkeypatch.setattr(shock_synthesis, 'ACCURACY_DB', 10.0)
    first = synthesize(_spec(), sample_rate=RATE).channels[0]
    assert first.passes < went_on.passes
    assert went_on.worst_db < first.worst_db
    monkeypatch.setattr(shock_synthesis, 'ACCURACY_DB', 0.5)
    monkeypatch.setattr(shock_synthesis, 'STALL_DB', 0.0)
    longer = synthesize(_spec(), sample_rate=RATE).channels[0]
    assert longer.passes >= went_on.passes
    assert longer.worst_db <= went_on.worst_db + 1e-12


def test_a_cancel_from_the_progress_callback_stops_the_synthesis():
    def cancel(done, total):
        if done:
            raise Cancelled('stop')

    with pytest.raises(Cancelled):
        synthesize(_spec(), sample_rate=RATE, progress=cancel)


def test_a_requirement_no_pass_meets_is_refused_naming_its_worst_point():
    """A requirement notched to a hundredth of its neighbors at one
    point: the oscillator there answers its neighbors' wavelets, and no
    amplitude of its own can bring it down."""
    f = 100.0 * 2.0 ** (np.arange(19) / 6.0)
    level = np.full(f.size, 100.0 * G)
    level[9] = G
    spec = ShockSpecification(f, level[None, :], response_dof=['1Z+'],
                              ordinate_dim=['acceleration'])
    told = []
    with pytest.raises(ValueError) as refused:
        synthesize(spec, sample_rate=RATE,
                   progress=lambda done, total: told.append((done, total)))
    words = str(refused.value)
    assert words.startswith('1Z+: no synthesis of 5 to 15 half cycles meets '
                            'the requirement in 40 passes')
    assert MAX_PASSES == 40
    assert told[-1] == (240, 240), 'forty passes at each of six counts'
    assert f'at {f[9]:.4g} Hz' in words, words
    with pytest.raises(ValueError, match='no synthesis of 15 half cycles '
                                         'meets the requirement'):
        synthesize(spec, sample_rate=RATE, half_cycles=15)


def test_a_shaker_that_holds_it_keeps_the_fewest_half_cycles():
    rating = ShakerRating(stroke=0.0508, velocity=1.78, force=1e6,
                          moving_mass=45.0, body_mass=900.0)
    made = synthesize(_spec(), sample_rate=RATE, rating=rating)
    c = made.channels[0]
    assert c.half_cycles == 5 and c.tried == ()
    assert c.reading.fits is True
    assert c.reading.counts_body
    lines = made.describe()
    assert lines[0].startswith('1Z+: 32 wavelets of 5 half cycles')
    assert lines[1].startswith('32/32 points inside the band')
    assert any('to the stop: within' in line for line in lines)


def test_the_reading_is_said_in_the_units_asked_for():
    from hertzforge.units import SI

    made = synthesize(_spec(), sample_rate=RATE,
                      rating=ShakerRating(stroke=0.0508))
    assert made.describe(SI)[2] == ('Travel up 0.000695 m against 0.0254 m '
                                    'to the stop: within, 3 %')


def test_a_band_met_on_the_first_pass_says_one_pass(monkeypatch):
    monkeypatch.setattr(shock_synthesis, 'ACCURACY_DB', 50.0)
    made = synthesize(_spec(warning=(-40.0, 40.0), abort=None), sample_rate=RATE)
    assert made.channels[0].passes == 1
    assert made.describe()[0] == ('1Z+: 32 wavelets of 5 half cycles, 1 '
                                  'pass, event 50 ms')


def test_the_shortest_event_the_shaker_holds_is_kept():
    """At this requirement five half cycles travel 0.70 mm up, seven
    1.05 mm, nine 0.61 mm: with 0.65 mm to each stop the synthesis tries
    five and seven and keeps nine."""
    rating = ShakerRating(stroke=0.0013)
    made = synthesize(_spec(), sample_rate=RATE, rating=rating)
    c = made.channels[0]
    assert c.half_cycles == 9
    assert [n for n, _limit, _share in c.tried] == [5, 7]
    assert all(limit == 'stroke' and share > 1.0
               for _n, limit, share in c.tried)
    assert c.reading.fits_stroke is True
    alone = synthesize(_spec(), sample_rate=RATE, half_cycles=9)
    assert np.array_equal(alone.history.ordinate, made.history.ordinate)


@pytest.mark.parametrize('rating,words', [
    (ShakerRating(stroke=0.0002),
     ('at 9 half cycles, the stroke runs out (Travel down 0.0239 in against '
      '0.00394 in to the stop: OVER, 608 %), and the requirement at 50 Hz '
      'alone asks for it')),
    (ShakerRating(velocity=0.05),
     ('at 15 half cycles, the shock velocity runs out (Peak velocity 10.3 '
      'in/s against 1.97 in/s: OVER, 521 %), and the requirement from 50 to '
      '79.37 Hz alone asks for it')),
    (ShakerRating(force=500.0, moving_mass=10.0),
     ('at 15 half cycles, the shock force runs out (Force 1,718 lbf against '
      '112 lbf: OVER, 1529 %), and the requirement from 1425 to 1796 Hz '
      'alone asks for it')),
])
def test_a_shaker_that_cannot_hold_it_is_named_with_its_band(rating, words):
    """The stroke and the velocity are spent from the low end up, the
    force from the top down."""
    told = []
    with pytest.raises(ValueError) as refused:
        synthesize(_spec(), sample_rate=RATE, rating=rating,
                   progress=lambda done, total: told.append((done, total)))
    assert str(refused.value) == ('1Z+: no synthesis of 5 to 15 half cycles '
                                  'fits the shaker. Closest, ' + words)
    assert told[-1][1] == 6 * 40, 'five to fifteen half cycles, no more'


def test_the_band_counts_from_the_end_that_spends_the_limit():
    points = np.array([10.0, 20.0, 40.0, 80.0])
    share = np.array([1.0, 1.0, 1.0, 1.0])
    assert shock_synthesis._band(share, points, 2.5, upward=True) == (10.0,
                                                                      40.0)
    assert shock_synthesis._band(share, points, 2.0, upward=True) == (10.0,
                                                                      20.0)
    assert shock_synthesis._band(share, points, 1.5, upward=False) == (40.0,
                                                                       80.0)
    assert shock_synthesis._band(share, points, 9.0, upward=True) == (10.0,
                                                                      80.0)
    assert shock_synthesis._band(np.array([3.0, -1.0, 1.0, 1.0]), points,
                                 3.0, upward=True) == (10.0, 10.0)


def test_the_guide_example():
    """The numbers the guide quotes for its example."""
    from hertzforge.core.shock_synthesis import suggested_rate
    from hertzforge.units import to_si

    spec = ShockSpecification.from_breakpoints(
        [10.0, 100.0, 2000.0], [10.0, 100.0, 100.0], '1Z+',
        ordinate_unit='g', warning=(-1.5, 3.0), abort=(-3.0, 6.0))
    rating = ShakerRating(stroke=float(to_si(2.0, 'in')),
                          velocity=float(to_si(70.0, 'in/s')),
                          force=float(to_si(10000.0, 'lbf')),
                          moving_mass=float(to_si(100.0, 'lb')),
                          body_mass=float(to_si(2000.0, 'lb')))
    assert suggested_rate(spec) == 19456.0
    made = synthesize(spec, sample_rate=19456.0, rating=rating)
    assert made.describe() == [
        '1Z+: 92 wavelets of 5 half cycles, 9 passes, event 250 ms',
        '92/92 points inside the band, within 0.61 dB of the requirement',
        ('Travel up with the body 0.37 in against 1 in to the stop: '
         'within, 37 %'),
        ('Peak velocity with the body 37.3 in/s against 70 in/s: within, '
         '53 %'),
        'Force 9,002 lbf against 10,000 lbf: within, 90 %']
    assert made.history.abscissa[-1] == pytest.approx(0.35, abs=1e-3), \
        'the record runs 100 ms past the 250 ms wavelet'
    small = ShakerRating(stroke=float(to_si(0.5, 'in')),
                         moving_mass=rating.moving_mass,
                         body_mass=rating.body_mass)
    with pytest.raises(ValueError) as refused:
        synthesize(spec, sample_rate=19456.0, rating=small)
    assert str(refused.value) == (
        '1Z+: no synthesis of 5 to 15 half cycles fits the shaker. Closest, '
        'at 9 half cycles, the stroke runs out (Travel down with the body '
        '0.363 in against 0.25 in to the stop: OVER, 145 %), and the '
        'requirement from 10 to 22.45 Hz alone asks for it')


def test_what_cannot_be_synthesized_is_refused_by_name():
    spec = _spec()
    with pytest.raises(TypeError, match='to a shock specification'):
        synthesize(Psd([1.0, 2.0], [[1.0, 1.0]], response_dof=['1Z+']),
                   sample_rate=RATE)
    with pytest.raises(ValueError, match='above zero'):
        synthesize(spec, sample_rate=0.0)
    with pytest.raises(ValueError, match='odd whole number'):
        synthesize(spec, sample_rate=RATE, half_cycles=6)
    with pytest.raises(ValueError, match='past the most tried, 15'):
        synthesize(spec, sample_rate=RATE, half_cycles=MOST_HALF_CYCLES + 2)
    with pytest.raises(ValueError, match='at least one DOF'):
        synthesize(spec, sample_rate=RATE, dofs=[])
    with pytest.raises(ValueError, match='named more than once'):
        synthesize(spec, sample_rate=RATE, dofs=['1Z+', '1Z+'])
    with pytest.raises(ValueError, match='names no 2Z\\+; it names 1Z\\+'):
        synthesize(spec, sample_rate=RATE, dofs=['2Z+'])
    with pytest.raises(ValueError,
                       match=r'runs to 1795.94 Hz.*more than 8979.7 Hz'):
        synthesize(spec, sample_rate=8960.0)
    assert synthesize(spec, sample_rate=8990.0).channels
    edge = ShockSpecification.from_breakpoints(
        [50.0, 400.0, 1600.0], [20.0, 100.0, 100.0], '1Z+', per_octave=6,
        ordinate_unit='g')
    assert edge.abscissa[-1] == 1600.0
    with pytest.raises(ValueError, match='sample at more than 8000 Hz'):
        synthesize(edge, sample_rate=8000.0)
    assert synthesize(edge, sample_rate=8000.5).channels


def test_a_requirement_with_nothing_or_a_gap_to_meet_is_refused():
    f = np.array([50.0, 100.0, 200.0, 400.0])
    zero = ShockSpecification(f, np.zeros((1, 4)), response_dof=['1Z+'],
                              ordinate_dim=['acceleration'])
    with pytest.raises(ValueError, match='not above zero at any point'):
        synthesize(zero, sample_rate=RATE)
    gap = ShockSpecification(f, [[100.0, np.nan, 300.0, 300.0]],
                             response_dof=['1Z+'],
                             ordinate_dim=['acceleration'])
    with pytest.raises(ValueError, match='not a level above zero at 100 Hz'):
        synthesize(gap, sample_rate=RATE)
    late = ShockSpecification(f, [[100.0, 200.0, 0.0, 300.0]],
                              response_dof=['1Z+'],
                              ordinate_dim=['acceleration'])
    with pytest.raises(ValueError, match='not a level above zero at 200 Hz'):
        synthesize(late, sample_rate=RATE)
    # a point at 0 Hz is no oscillator: the band starts above it
    rest = ShockSpecification([0.0, 100.0, 200.0], [[50.0, 300.0, 600.0]],
                              response_dof=['1Z+'],
                              ordinate_dim=['acceleration'])
    made = synthesize(rest, sample_rate=RATE)
    assert np.array_equal(made.channels[0].points, [100.0, 200.0])
    # outside the written band is not a gap: the band starts where the
    # requirement does
    ends = ShockSpecification(f, [[np.nan, 300.0, 600.0, 0.0]],
                              response_dof=['1Z+'],
                              ordinate_dim=['acceleration'])
    made = synthesize(ends, sample_rate=RATE)
    assert np.array_equal(made.channels[0].points, [100.0, 200.0])
    spectrum = made.srs.ordinate[0]
    assert np.isnan(spectrum[[0, 3]]).all() and np.isfinite(spectrum[1:3]).all()
    assert made.tolerance()[0][2]['points'] == 2


def test_a_requirement_of_another_quantity_or_no_units_is_refused():
    f = np.array([50.0, 100.0, 200.0])
    raw = ShockSpecification(f, [[1.0, 2.0, 2.0]], response_dof=['1Z+'])
    with pytest.raises(ValueError, match='declare its units first'):
        synthesize(raw, sample_rate=RATE)
    velocity = ShockSpecification(f, [[1.0, 2.0, 2.0]], response_dof=['1Z+'],
                                  ordinate_dim=['velocity'])
    with pytest.raises(ValueError, match='spectrum of velocity'):
        synthesize(velocity, sample_rate=RATE)
    # paired by quantity as well as DOF: the acceleration beside a velocity
    both = ShockSpecification(f, [[1.0, 2.0, 2.0], [100.0, 200.0, 200.0]],
                              response_dof=['1Z+', '1Z+'],
                              ordinate_dim=['velocity', 'acceleration'])
    made = synthesize(both, sample_rate=RATE)
    assert np.array_equal(made.channels[0].target, [100.0, 200.0, 200.0])
    assert made.history.ordinate_unit == ['m/s**2']
    twice = ShockSpecification(f, [[1.0, 2.0, 2.0], [100.0, 200.0, 200.0]],
                               response_dof=['1Z+', '1Z+'],
                               ordinate_dim=['acceleration'] * 2)
    with pytest.raises(ValueError, match=r'2 acceleration records at 1Z\+'):
        synthesize(twice, sample_rate=RATE)


def test_a_flat_requirement_and_one_with_no_band_of_its_own():
    """A constant level everywhere, and a requirement with no limits,
    whose band is the default inner band."""
    f = 100.0 * 2.0 ** (np.arange(13) / 6.0)
    flat = ShockSpecification(f, np.full((1, f.size), 50.0 * G),
                              response_dof=['1Z+'],
                              ordinate_dim=['acceleration'])
    made = synthesize(flat, sample_rate=RATE)
    result = made.tolerance()[0][2]
    assert result['limits'] is False
    assert result['inside_count'] == f.size


def test_the_requirement_q_and_type_are_kept():
    spec = _spec(q=20.0, kind='positive')
    made = synthesize(spec, sample_rate=RATE)
    assert made.srs.q == 20.0 and made.srs.kind == 'positive'
    srs = made.history.compute_srs(low=50.0, high=spec.abscissa[-1],
                                   per_octave=6, q=20.0, kind='positive')
    assert np.allclose(srs.ordinate, made.srs.ordinate, rtol=1e-12)
    assert srs_tolerance_all(spec, srs)[0][2]['passed'] is True


def test_each_channel_is_synthesized_to_its_own_requirement():
    spec = ShockSpecification.from_breakpoints(
        [50.0, 400.0, 2000.0], [[20.0, 100.0, 100.0], [10.0, 50.0, 50.0]],
        ['1X+', '1Z+'], per_octave=6, ordinate_unit='g', warning=(-1.5, 3.0))
    made = synthesize(spec, sample_rate=RATE)
    assert made.history.response_dof == ['1X+', '1Z+']
    assert made.history.ordinate_unit == ['g', 'g']
    for _dof, _event, result in made.tolerance():
        assert result['inside_count'] == result['points']
    only = synthesize(spec, sample_rate=RATE, dofs=['1Z+'])
    assert only.history.response_dof == ['1Z+']
    assert np.allclose(only.history.ordinate[0], made.history.ordinate[1])


def test_a_shock_specification_makes_an_srs_synthesis():
    spec = _spec()
    assert signals.kinds_for(spec) == ('srs',)
    assert signals.KINDS['srs'] == 'SRS synthesis'
    settings = signals.SignalSettings(kind='srs', sample_rate=RATE,
                                      stroke=0.0508, velocity=0.0)
    assert settings.describe() == 'SRS synthesis at 10240 Hz'
    rating = settings.rating()
    assert rating.stroke == 0.0508 and rating.velocity is None, \
        'zero is not given, as the stroke reading takes it'
    again = signals.SignalSettings.from_dict(settings.to_dict())
    assert again == settings
    made = signals.generate(spec, settings)
    assert made.ordinate_dim == ['acceleration']
    assert made.averaging.frame_length == made.ordinate.shape[1]
    assert np.array_equal(signals.preview(spec, settings).ordinate,
                          made.ordinate), 'iterated whole for its preview'
    assert signals.target_of(spec, settings) is None
    assert signals.frame_used(spec, settings) is None
    with pytest.raises(ValueError, match='SRS synthesis is not made from'):
        signals.generate(signals.flat_spectrum('1X+', quantity='force',
                                               band=(20.0, 200.0), level=1.0),
                         settings)


def test_the_verb_records_the_synthesis_and_a_changed_band_is_stale():
    project = hertzforge.Project()
    project.add('Requirement', _spec())
    name = project.generate_signal('Requirement', 'srs', sample_rate=RATE,
                                   stroke=0.0508)
    record = project.provenance[name]
    assert record['params']['kind'] == 'srs'
    assert record['params']['stroke'] == 0.0508
    made = project[name]
    again = synthesize(project['Requirement'], sample_rate=RATE,
                       rating=ShakerRating(stroke=0.0508))
    assert np.array_equal(made.ordinate, again.history.ordinate)
    line = next(e for e in project.journal if 'generate_signal' in e)
    assert "'srs'" in line and 'stroke=0.0508' in line
    assert 'progress' not in line
    assert name not in project.stale()
    project['Requirement'].limits['warning_lower'] *= 10 ** (0.5 / 20)
    assert name in project.stale(), 'the band every point is put inside moved'
