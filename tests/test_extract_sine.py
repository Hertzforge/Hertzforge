"""Sine extraction: the tracking-filter reading, graded two ways.

The synthetic tests plant known tones in known noise and hold the
recovered numbers; the oracle tests (skipped where the stressdata is
absent) grade the same code against real controller runs and the
controller's own tracked-amplitude record.
"""

from __future__ import annotations

import os

import numpy as np
import pytest

import hertzforge
from hertzforge.core.data import TimeHistory
from hertzforge.core.sine import (
    SineLevel,
    SineSweepSpecification,
    SineTone,
    extract_sine,
    find_tone,
)

FS = 4096.0

STRESS = os.path.join(os.path.dirname(os.path.dirname(
    os.path.abspath(__file__))), 'stressdata', 'plate')


def _spec():
    up = SineTone('Up', 1.0, [100.0, 800.0],
                  [[2.0, 3.0], [2.0, 3.0]], [0], [100.0])
    down = SineTone('Down', 2.0, [600.0, 200.0],
                    [[1.0, 1.0], [1.0, 1.0]], [0], [-80.0])
    return SineSweepSpecification([up, down], ['101Z+', '104Z+'],
                                  ordinate_unit='m/s**2')


def _recording(spec, seconds=12.0, noise=0.0, seed=7):
    """The specification, played: each tone at its own start time with
    its own amplitude, plus gaussian noise at `noise` RMS."""
    rng = np.random.default_rng(seed)
    samples = int(seconds * FS)
    signal = rng.standard_normal((2, samples)) * noise
    for tone in spec.tones:
        arg = tone.argument(1.0 / FS)
        start = int(tone.start_time * FS)
        n = min(len(arg), samples - start)
        for channel in range(2):
            amplitude = tone.amplitude[0, channel]
            signal[channel, start:start + n] += (amplitude
                                                 * np.cos(arg[:n]))
    return TimeHistory(
        abscissa=np.arange(samples) / FS, ordinate=signal,
        response_dof=['101Z+', '104Z+'],
        ordinate_dim=['acceleration'] * 2,
        ordinate_unit=['m/s**2'] * 2)


def test_planted_tones_come_back_at_their_amplitudes():
    spec = _spec()
    levels = extract_sine(_recording(spec), spec)
    assert levels.tone_names == ['Up', 'Down'], \
        'one extraction, one object, the tones inside'
    for level, planted in zip(levels, ([2.0, 3.0], [1.0, 1.0])):
        assert level.onset == pytest.approx(
            spec.tone(level.tone).start_time, abs=2e-3), \
            'the matched filter lands on the planted start'
        mid = slice(len(level.abscissa) // 4, -len(level.abscissa) // 4)
        for row, amplitude in enumerate(planted):
            measured = np.abs(level.ordinate[row, mid])
            # the two tones cross in frequency; under the tracking
            # demodulation each read the other as noise around the
            # crossing (0.4 dB, and this tolerance was 5%) — the joint
            # Vold-Kalman solve separates them, so every line holds
            assert np.median(measured) == pytest.approx(
                amplitude, rel=0.002), \
                f'{level.tone} channel {row}: the planted amplitude'
            assert np.abs(measured - amplitude).max() < 0.03 * amplitude, \
                f'{level.tone} channel {row}: worst line off by more than 3%'
    assert np.all(np.diff(levels.tone('Down').abscissa) > 0), \
        'a descending sweep still reads ascending in frequency'
    for level in levels:
        assert level.seconds is not None and \
            np.all(np.diff(level.seconds[np.argsort(level.seconds)]) >= 0), \
            'every line is stamped with the second it was measured'


def test_two_tones_that_stay_close_are_separated():
    """The case the Vold-Kalman solve was built for (Brandon,
    2026-09-03: the last analysis before a report should use the best
    estimator there is). The real sine run's Sweep Up (100-800 Hz at
    50 Hz/s) and Log Octave (200-400 Hz at 10 oct/min, from 1 s) sit
    within 20 Hz of each other for two seconds and within 50 Hz for
    six — inside each other's tracking filter for a long stretch, not
    a crossing instant. Planted at known amplitudes, the tracking
    demodulation read both 2 to 2.5 dB high through that stretch with
    excursions to -23 dB (and so does the controller's own tracker,
    which is why the oracle test below compares only where tones are
    apart). The joint solve recovers both to 0.000 dB."""
    up = SineTone('Up', 0.0, [100.0, 800.0], [[2.5, 2.5], [2.5, 2.5]],
                  [0], [50.0])
    log = SineTone('Log', 1.0, [200.0, 400.0], [[2.3, 2.3], [2.3, 2.3]],
                   [1], [10.0])
    spec = SineSweepSpecification([up, log], ['101Z+', '104Z+'],
                                  ordinate_unit='m/s**2')
    levels = extract_sine(_recording(spec, seconds=14.0), spec,
                          onsets={'Up': 0.0, 'Log': 1.0})
    dt = 1.0 / FS
    clock = np.arange(0.0, 14.0, dt)
    paths = {t.name: np.interp(clock, t.trajectory(dt)[0] + t.start_time,
                               t.trajectory(dt)[1], left=np.nan,
                               right=np.nan) for t in spec.tones}
    for level, amplitude in zip(levels, (2.5, 2.3)):
        other = next(n for n in paths if n != level.tone)
        gap = np.abs(np.interp(level.seconds, clock, paths[other],
                               left=np.nan, right=np.nan) - level.abscissa)
        close = gap < 20.0
        assert close.sum() > 50, 'the planted pair really is close for a while'
        error_db = 20 * np.log10(np.abs(level.ordinate[0]) / amplitude)
        assert np.abs(error_db[close]).max() < 0.1, (
            f'{level.tone}: {np.abs(error_db[close]).max():.2f} dB off where '
            'the other tone sits within 20 Hz')
        assert np.abs(error_db).max() < 0.1, f'{level.tone}: off somewhere'


def test_extraction_holds_under_loud_noise():
    """Noise 4x the quieter tone's amplitude: the tracking filter's
    whole job. With the automatic smoothing the readings that stood
    above the floor hold around the planted level: a realization's
    median scatters 0.9 dB (one standard deviation, measured over
    eight, 2026-10-01: the quieter tone at its heaviest smoothing has
    only some twenty independent readings in its middle half), the
    mean over the eight sat at +0.2 dB, and a single realization set
    the earlier tolerances too tight. Each within 2.5 dB, the mean
    within 0.6."""
    spec = _spec()
    errors = []
    for seed in range(7, 15):
        levels = extract_sine(_recording(spec, noise=4.0, seed=seed), spec)
        down = levels.tone('Down')
        mid = slice(len(down.abscissa) // 4, -len(down.abscissa) // 4)
        resolved = down.resolved[0, mid]
        measured = np.median(np.abs(down.ordinate[0, mid][resolved]))
        errors.append(20 * np.log10(measured / 1.0))
        assert abs(errors[-1]) < 2.5, \
            f'planted 1.0 under 4.0-RMS noise read back {errors[-1]:+.2f} dB off'
        assert levels.cycles > 10.0, 'the automatic climbed the ladder'
    assert abs(np.mean(errors)) < 0.6, errors


def test_a_short_recording_reports_its_coverage():
    """The record ends mid-sweep: the level covers what was run, and
    says nothing further — the coverage rule, not an error."""
    spec = _spec()
    levels = extract_sine(_recording(spec, seconds=5.0), spec,
                          tones=['Up'])
    top = levels.tone('Up').abscissa.max()
    assert top < 500.0, 'the sweep was cut off well before 800 Hz'
    assert top > 250.0, 'but a real stretch of it was recorded'


def test_missing_control_channels_refuse_by_name():
    spec = _spec()
    history = _recording(spec)
    short = TimeHistory(
        abscissa=history.abscissa, ordinate=history.ordinate[:1],
        response_dof=['101Z+'], ordinate_dim=['acceleration'],
        ordinate_unit=['m/s**2'])
    with pytest.raises(ValueError, match='104Z'):
        extract_sine(short, spec)


def test_an_onset_override_skips_the_search():
    spec = _spec()
    history = _recording(spec)
    levels = extract_sine(history, spec, tones=['Up'],
                          onsets={'Up': 1.0})
    assert levels.tone('Up').onset == 1.0


def test_find_tone_bounds_its_search_when_told():
    spec = _spec()
    history = _recording(spec)
    onset = find_tone(np.asarray(history.ordinate), 1.0 / FS,
                      spec.tone('Up'), search=(0.5, 1.5))
    assert onset == pytest.approx(1.0, abs=2e-3)


def test_the_level_set_round_trips_through_native(tmp_path):
    from hertzforge.core.sine import SineLevelSet

    spec = _spec()
    levels = extract_sine(_recording(spec), spec)
    path = str(tmp_path / 'levels.vdyn')
    hertzforge.io.save(levels, path)
    back = hertzforge.io.load(path)
    assert isinstance(back, SineLevelSet)
    assert back == levels
    up = back.tone('Up')
    assert isinstance(up, SineLevel)
    assert up.onset == pytest.approx(levels.tone('Up').onset)
    assert np.allclose(up.seconds, levels.tone('Up').seconds)


def test_the_project_verb_extracts_and_links():
    spec = _spec()
    project = hertzforge.Project('sine run')
    project.add('Time History', _recording(spec))
    project.add('Sine Specification', spec)
    names = project.extract_sine('Time History')
    assert names == ['Sine Levels'], 'one extraction, one object'
    assert project['Sine Levels'].tone_names == ['Up', 'Down']
    assert 'Time History' in (project.object_group_of('Sine Levels') or [])


needs_stress = pytest.mark.skipif(
    not os.path.exists(os.path.join(STRESS, 'sine.nc4')),
    reason='stressdata not generated on this machine')


def _controller_levels(control_npz, tone_index):
    """The controller's own tracked amplitude, laid over frequency.

    Its per-block amplitude records and its frequency trajectory share
    a clock, so amplitude-versus-frequency needs no time alignment —
    both are functions of the sweep's own monotonic frequency."""
    truth = np.load(control_npz)
    keys = sorted((int(k.rsplit('_', 1)[1]), k) for k in truth.files
                  if k.startswith('control_response_amplitudes'))
    tracked = np.concatenate([truth[k][tone_index] for _, k in keys],
                             axis=-1)
    frequency = truth['control_response_frequencies'][tone_index]
    n = min(tracked.shape[-1], len(frequency))
    return frequency[:n], tracked[:, :n]


def _against_controller(level, frequency, tracked, keep=None):
    """Median dB between the extraction and the controller's tracker,
    per channel, compared where both actually swept — and, with
    `keep`, only on the lines it marks."""
    moving = np.flatnonzero(np.abs(np.diff(frequency)) > 0)
    # margins by span, not by absolute frequency: a 390-410 near-dwell
    # has no room for percentage margins
    span = frequency[moving].max() - frequency[moving].min()
    lo = frequency[moving].min() + 0.1 * span
    hi = frequency[moving].max() - 0.1 * span
    inside = (level.abscissa > lo) & (level.abscissa < hi)
    if keep is not None:
        inside &= keep
    out = []
    for row in range(level.ordinate.shape[0]):
        order = np.argsort(frequency[moving])
        reference = np.interp(level.abscissa[inside],
                              frequency[moving][order],
                              tracked[row][moving][order])
        ratio = np.abs(level.ordinate[row, inside]) / np.maximum(
            reference, 1e-12)
        out.append(20 * np.log10(np.median(ratio)))
    return out


@needs_stress
def test_the_real_sine_run_sits_at_or_under_the_controller():
    """Four real tones over eight control channels and four shakers.

    The controller's live tracker reads *high* — its quadrature filter
    counts the other tones' leakage as amplitude — so the structural
    claim is one-sided: the extraction sits at or below the tracker,
    never meaningfully above. An *agreement* oracle, not a truth
    oracle (AGENTS.md): where the nearest other tone is more than
    50 Hz away the two read the same thing and agree within the
    envelope measured here (-0.9 to -4.8 dB when the tracking
    demodulation was the reader; the joint solve sits a little closer);
    where another tone is within 20 Hz the tracker double-counts and
    the extraction reads the truth, as the planted close-tones test
    proves — there the only claim is at-or-under. Point-for-point
    agreement is never the claim: on a resonant plate under live
    control the physical amplitude swings several dB within small
    frequency spans."""
    project = hertzforge.Project('real')
    project.import_file(os.path.join(STRESS, 'sine.nc4'))
    project.extract_sine(project.time_history)
    levels = project['Sine Levels']
    assert len(levels) == 4
    spec = project.sine_sweep_specification
    control_npz = os.path.join(STRESS, 'sine_control.npz')
    order = [str(n) for n in np.load(control_npz)['names']]
    dt = 1.0 / FS
    clock = np.arange(0.0, 16.0, dt)
    paths = {t.name: np.interp(clock, t.trajectory(dt)[0] + t.start_time,
                               t.trajectory(dt)[1], left=np.nan,
                               right=np.nan) for t in spec.tones}
    for level in levels:
        frequency, tracked = _controller_levels(
            control_npz, order.index(level.tone))
        tone = spec.tone(level.tone)
        seconds = level.seconds - (level.onset - tone.start_time)
        gap = np.nanmin(np.stack([
            np.abs(np.interp(seconds, clock, paths[o], left=np.nan,
                             right=np.nan) - level.abscissa)
            for o in paths if o != level.tone]), axis=0)
        apart, close = gap > 50.0, gap < 20.0
        if apart.sum() >= 50:
            for row, error in enumerate(
                    _against_controller(level, frequency, tracked, apart)):
                assert -6.0 < error < 0.5, (
                    f'{level.tone} channel {row}: {error:+.2f} dB from '
                    "the controller's own tracker, tones apart")
        for row, error in enumerate(
                _against_controller(level, frequency, tracked, close)):
            assert error < 0.5, (
                f'{level.tone} channel {row}: {error:+.2f} dB *above* '
                'the tracker where it double-counts a close tone')


@needs_stress
def test_the_real_mixed_run_reads_the_sweep_under_the_random():
    """The 0.5-amplitude sweep under a random 13.5 dB louder — the
    problem the arc exists for, on real data.

    Under that much noise the controller's tracker bias grows to
    several dB (it reads the random as sine and backs its drive off
    accordingly), so the one-sided envelope is wider. The physical
    check is the second assertion: the extracted levels bracket the
    0.5 target the way an eight-channel four-shaker compromise must —
    some channels above, some below, the median within a few dB."""
    project = hertzforge.Project('mixed')
    project.import_file(os.path.join(STRESS, 'mixed.nc4'))
    project.extract_sine(project.time_history)
    level = project['Sine Levels'].levels[0]
    control_npz = os.path.join(STRESS, 'mixed_sine_control.npz')
    frequency, tracked = _controller_levels(control_npz, 0)
    errors = _against_controller(level, frequency, tracked)
    for row, error in enumerate(errors):
        # -12.5: the joint solve's narrower noise bandwidth carries
        # less positive noise bias than the tracking average, so it
        # sits half a dB further under a tracker that reads the random
        # as sine (measured 2026-09-03: -4.2 to -8.9 dB by channel);
        # with the floor measured beside the tone (2026-10-01) seven
        # channels moved toward the tracker (-2.3 to -8.2 dB) and the
        # deepest-buried one from -11.7 to -12.1, the median level
        # unmoved (-3.6 dB from the 0.5 target either way)
        assert -12.5 < error < 0.5, (
            f'channel {row}: {error:+.2f} dB from the controller '
            'under the random')
    mid = (level.abscissa > 170) & (level.abscissa < 730)
    medians = [np.median(np.abs(level.ordinate[row, mid]))
               for row in range(level.ordinate.shape[0])]
    spread_db = 20 * np.log10(np.median(medians) / 0.5)
    assert abs(spread_db) < 5.0, (
        f'physically implausible: the channel-median level sits '
        f'{spread_db:+.2f} dB from the 0.5 target')


@needs_stress
def test_a_sweep_over_a_virtual_point_is_read_at_its_rows():
    """The same mixed run, with the sweep controlling a virtual point —
    three rows of a response transformation over the eight control
    channels — while the random controls the raw channels (Brandon,
    2026-09-24: a run like this did not load). The levels are read from
    the rows' own time histories, which ride the recording beside the
    raw channels, and graded against the controller's tracker on the
    same rows: the untransformed run's envelope holds (measured -3.5 to
    -3.8 dB by row).

    Regenerate with `generate_plate_sine.py mixed_transformed` in the
    generators repository."""
    path = os.path.join(STRESS, 'mixed_transformed.nc4')
    if not os.path.exists(path):
        pytest.skip('regenerate with generate_plate_sine.py mixed_transformed')
    project = hertzforge.Project('mixed_transformed')
    project.import_file(path)
    spec = project.sine_sweep_specification
    assert spec.response_dof == ['1', '2', '3']
    project.extract_sine(project.time_history)
    level = project['Sine Levels'].levels[0]
    assert level.ordinate.shape[0] == 3, 'one level per row'
    control_npz = os.path.join(STRESS, 'mixed_transformed_sine_control.npz')
    frequency, tracked = _controller_levels(control_npz, 0)
    for row, error in enumerate(_against_controller(level, frequency,
                                                    tracked)):
        assert -12.0 < error < 0.5, (
            f'row {row + 1}: {error:+.2f} dB from the controller '
            'under the random')


def test_the_sine_report_renders_its_comparison_and_bars():
    import json

    from hertzforge.report import render_html

    spec = _spec()
    for tone in spec.tones:
        tone.limits['warning_lower'] = tone.amplitude * 10 ** (-3 / 20)
        tone.limits['warning_upper'] = tone.amplitude * 10 ** (3 / 20)
        tone.limits['abort_lower'] = tone.amplitude * 10 ** (-6 / 20)
        tone.limits['abort_upper'] = tone.amplitude * 10 ** (6 / 20)
    project = hertzforge.Project('sine run')
    project.add('Time History', _recording(spec))
    project.add('Sine Specification', spec)
    project.extract_sine('Time History')
    project.set_basis(*list(project))
    name = project.generate_report('sine')
    html = render_html(project[name], project)
    payload = json.loads(html.split('type="application/json">')[1]
                         .split('</script>')[0])
    figures = [block for block in payload['blocks']
               if block.get('kind') == 'plot'
               and block.get('channels')]
    assert len(figures) >= 2, 'one comparison figure per tone'
    first = figures[-1]
    assert first['channels'][0]['zones'], \
        "the tone's warning and abort bands shade the figure"
    assert first['channels'][0]['responses'], \
        'the extracted level rides over the requirement'
    bars = [block for block in payload['blocks']
            if block.get('kind') == 'bars']
    assert bars and len(bars[0]['values']) == 4, \
        'two tones x two control channels, one signed bar each'
    assert bars[0]['low'] == -3.0 and bars[0]['high'] == 3.0


def _tones_for(spec, signal):
    """(arguments, frequencies, starts) clipped to the record, as
    `extract_sine` hands them to the solver."""
    dt = 1.0 / FS
    args, freqs, starts = [], [], []
    for tone in spec.tones:
        start = round(tone.start_time / dt)
        argument = tone.argument(dt)
        _t, f = tone.trajectory(dt)
        n = min(len(argument), len(signal) - start)
        args.append(argument[:n]); freqs.append(f[:n]); starts.append(start)
    return args, freqs, starts


def test_the_chunked_solve_is_the_whole_solve():
    """A 19-minute run solved whole wanted tens of gigabytes; solved in
    chunks with a margin of smoothing windows on each side it is the
    same envelope to a part in ten million, at the smallest chunk."""
    from hertzforge.core.sine import _vold_kalman_whole, vold_kalman

    spec = _spec()
    signal = np.asarray(_recording(spec, noise=0.5).ordinate)[0]
    args, freqs, starts = _tones_for(spec, signal)
    whole = _vold_kalman_whole(signal, args, freqs, starts, 1.0 / FS)
    chunked = vold_kalman(signal, args, freqs, starts, 1.0 / FS, chunk=4096)
    for ours, theirs in zip(chunked, whole):
        assert len(ours) == len(theirs)
        assert np.abs(ours - theirs).max() <= 1e-5 * np.abs(theirs).max()


def test_the_chunked_solve_holds_a_chunks_worth_of_memory():
    """Measured, since the point is the memory: the chunked solve peaks
    well under the whole solve's on the same record."""
    import tracemalloc

    from hertzforge.core.sine import _vold_kalman_whole, vold_kalman

    spec = _spec()
    signal = np.asarray(_recording(spec, noise=0.5).ordinate)[0]
    args, freqs, starts = _tones_for(spec, signal)
    tracemalloc.start()
    _vold_kalman_whole(signal, args, freqs, starts, 1.0 / FS)
    whole = tracemalloc.get_traced_memory()[1]
    tracemalloc.stop()
    tracemalloc.start()
    vold_kalman(signal, args, freqs, starts, 1.0 / FS, chunk=4096)
    chunked = tracemalloc.get_traced_memory()[1]
    tracemalloc.stop()
    assert chunked < 0.5 * whole, (chunked, whole)


def test_extraction_reads_the_same_levels_through_chunks(monkeypatch):
    """The extraction's own path, with the chunk forced small: every
    tone's levels as the whole-record extraction gives them."""
    from hertzforge.core import sine

    spec = _spec()
    history = _recording(spec, noise=0.5)
    whole = extract_sine(history, spec)
    monkeypatch.setattr(sine, 'CHUNK', 4096)
    chunked = extract_sine(history, spec)
    for ours, theirs in zip(chunked, whole):
        assert np.allclose(ours.abscissa, theirs.abscissa)
        assert np.allclose(ours.ordinate, theirs.ordinate, rtol=1e-5, atol=0)


def _flat_sweep(amplitude=2.0, noise=1.0, seed=3, rate_error=0.0,
                seconds=9.0):
    """One planted sweep of constant amplitude under white noise; the
    recording's sweep may run `rate_error` faster than the
    specification says."""
    spec_tone = SineTone('Up', 0.5, [100.0, 800.0],
                         [[amplitude] * 2] * 2, [0], [100.0])
    spec = SineSweepSpecification([spec_tone], ['101Z+', '104Z+'],
                                  ordinate_unit='m/s**2')
    actual = SineTone('Up', 0.5, [100.0, 800.0], [[amplitude] * 2] * 2,
                      [0], [100.0 * (1.0 + rate_error)])
    n = int(seconds * FS)
    rng = np.random.default_rng(seed)
    signal = rng.standard_normal((2, n)) * noise
    argument = actual.argument(1.0 / FS)
    start = int(0.5 * FS)
    m = min(len(argument), n - start)
    signal[:, start:start + m] += amplitude * np.cos(argument[:m])
    history = TimeHistory(np.arange(n) / FS, signal,
                          response_dof=['101Z+', '104Z+'],
                          ordinate_dim=['acceleration'] * 2,
                          ordinate_unit=['m/s**2'] * 2)
    return history, spec


def _inside(level, low=150.0, high=750.0):
    return (level.abscissa > low) & (level.abscissa < high)


def test_the_automatic_smoothing_climbs_with_the_noise():
    """Clean, the base ten cycles; under loud noise, enough smoothing
    to hold the readings' scatter near the target — and the readings
    then do: a 2.0 sweep under 6.0-RMS noise read with 10 cycles had
    a tenth of its points at zero and a 190 dB spread (2026-09-30)."""
    from hertzforge.core.sine import BASE_CYCLES, suggest_cycles

    quiet, spec = _flat_sweep(noise=0.0)
    assert suggest_cycles(quiet, spec) == BASE_CYCLES
    loud, spec = _flat_sweep(noise=6.0)
    chosen = suggest_cycles(loud, spec)
    assert chosen >= 60.0
    levels = extract_sine(loud, spec)
    assert levels.cycles == chosen
    level = next(iter(levels))
    inside = _inside(level)
    assert level.resolved[0, inside].all(), 'nothing left under the floor'
    readings = 20 * np.log10(np.abs(level.ordinate[0, inside]) / 2.0)
    # the readings' own scatter at the chosen smoothing reads 0.7 to
    # 1.4 dB over four realizations, their median -0.6 to -0.1 dB — a
    # small low bias at the heaviest smoothing and this signal-to-noise
    # that the debiasing's noise-bandwidth calibration leaves
    # (2026-10-01; STATUS.md names it)
    assert np.std(readings) < 1.6
    assert abs(np.median(readings)) < 0.75


def test_a_fixed_smoothing_is_used_as_given():
    loud, spec = _flat_sweep(noise=6.0)
    levels = extract_sine(loud, spec, cycles=20.0)
    assert levels.cycles == 20.0


def test_the_sampled_levels_read_the_scatter_quickly():
    """The sine view's preview and the automatic's measurement: a
    handful of short solves, the noise floor and the predicted scatter
    beside each reading."""
    import time

    from hertzforge.core.sine import sample_levels

    loud, spec = _flat_sweep(noise=6.0)
    started = time.perf_counter()
    sampled = sample_levels(loud, spec, cycles=10.0)
    assert time.perf_counter() - started < 2.0
    (reading,) = sampled
    assert reading['tone'] == 'Up' and reading['cycles'] == 10.0
    assert reading['amplitude'].shape == reading['floor'].shape == (2, 8)
    assert np.all(np.diff(reading['frequency']) > 0)
    assert np.median(reading['scatter_db']) > 2.0, 'loud noise reads loud'
    quiet, spec = _flat_sweep(noise=0.5)
    (reading,) = sample_levels(quiet, spec, cycles=10.0)
    assert np.median(reading['scatter_db']) < 0.5


def test_a_tone_under_the_floor_is_reported_at_the_floor_and_flagged():
    """A reading of zero was a hole in the curve; a reading at the
    floor says how much was not seen, and is marked so the comparison
    does not judge it (Brandon, 2026-09-30)."""
    loud, spec = _flat_sweep(noise=12.0)
    level = next(iter(extract_sine(loud, spec, cycles=10.0)))
    flagged = level.below_floor[0]
    assert flagged.any() and not flagged.all()
    magnitude = np.abs(level.ordinate[0])
    assert np.allclose(magnitude[flagged], level.floor[0][flagged])
    assert np.all(magnitude[~flagged] > 0.0)
    assert (level.resolved == ~level.below_floor).all()
    assert level.floor.shape == level.ordinate.shape


def test_a_flagged_line_is_not_scored():
    """The sine score (`sine_errors`) leaves a reading under its floor
    out: it says how much was not seen, not how much was there."""
    from hertzforge.core.compliance import sine_errors

    loud, spec = _flat_sweep(noise=12.0)
    levels = extract_sine(loud, spec, cycles=10.0)
    level = next(iter(levels))
    flagged = level.below_floor[0]
    assert flagged.any()
    scored = sine_errors(spec, levels)
    assert len(scored) == 2
    # the same score from the resolved readings alone
    got = np.abs(level.ordinate[0])[~flagged]
    wanted = spec.tone('Up').target(level.abscissa)[:, 0][~flagged]
    from hertzforge.core.compliance import signed_rms_db

    assert scored[0][2] == pytest.approx(signed_rms_db(got, wanted))
    # and a flagged reading moved to the floor leaves the score alone
    level.ordinate[0, flagged] *= 10.0
    assert sine_errors(spec, levels)[0][2] == pytest.approx(scored[0][2])


def test_the_clock_is_refined_against_the_specification():
    """A recording whose sweep ran 1 % faster than commanded: at forty
    cycles the filter's bandwidth is narrower than the drift by the
    end of the sweep and the unrefined reading falls; refined, the
    sweep argument follows the recording and the level holds, and the
    offset applied is reported."""
    fast, spec = _flat_sweep(noise=1.0, rate_error=0.01, seed=5)
    unrefined = next(iter(extract_sine(fast, spec, cycles=40.0,
                                       refine=False)))
    refined = next(iter(extract_sine(fast, spec, cycles=40.0)))
    late = refined.abscissa > 600.0
    assert np.median(np.abs(unrefined.ordinate[0, late])) < 1.7
    assert abs(np.median(np.abs(refined.ordinate[0, late])) - 2.0) < 0.1
    assert 5.0 < refined.drift_hz < 9.0
    assert unrefined.drift_hz == 0.0
    # a sweep on its clock is left alone
    true, spec = _flat_sweep(noise=1.0, seed=5)
    assert next(iter(extract_sine(true, spec, cycles=40.0))).drift_hz == 0.0


def test_the_floor_and_the_smoothing_travel_through_native(tmp_path):
    from hertzforge.core.sine import SineLevelSet

    loud, spec = _flat_sweep(noise=12.0)
    levels = extract_sine(loud, spec, cycles=10.0)
    path = str(tmp_path / 'levels.vdyn')
    hertzforge.io.save(levels, path)
    back = hertzforge.io.load(path)
    assert isinstance(back, SineLevelSet) and back == levels
    assert back.cycles == 10.0
    ours, theirs = next(iter(levels)), next(iter(back))
    assert np.allclose(theirs.floor, ours.floor)
    assert (theirs.below_floor == ours.below_floor).all()
    assert theirs.drift_hz == ours.drift_hz


def test_the_setting_rides_the_history_through_the_project(tmp_path):
    """The verb adopts the automatic setting, records what it chose,
    journals as one line that replays, and a changed smoothing badges
    the levels stale — the filter view's contract, for the sine view."""
    from hertzforge.core.sine import SineExtraction

    loud, spec = _flat_sweep(noise=6.0)
    project = hertzforge.Project('sweep')
    project.add('Time History', loud)
    project.add('Sine Specification', spec)
    assert project['Time History'].sine_extraction is None
    (name,) = project.extract_sine('Time History')
    setting = project['Time History'].sine_extraction
    assert setting.automatic and setting.chosen == project[name].cycles
    assert setting.chosen >= 60.0
    assert project.journal[-1] == "project.extract_sine('Time History')"
    assert not project.stale(), 'fresh'
    project['Time History'].sine_extraction = SineExtraction(cycles=20.0)
    assert name in project.stale(), 'a moved smoothing badges the levels'
    project.refresh(name)
    assert project[name].cycles == 20.0 and not project.stale()
    path = str(tmp_path / 'sweep.vdyn')
    project.save(path)
    back = hertzforge.Project.open(path)
    assert back['Time History'].sine_extraction == SineExtraction(cycles=20.0)
    assert back[name].cycles == 20.0
    # the journal line a drag or an edit writes rebuilds the setting
    line = repr(SineExtraction(cycles=20.0))
    room: dict = {}
    exec('from hertzforge.core.sine import SineExtraction\n'  # noqa: S102
         f'setting = {line}', room)
    assert room['setting'] == SineExtraction(cycles=20.0)


def test_the_pooled_extraction_is_the_serial_one():
    """Each piece and channel is one task and a task is the serial
    code, so the worker processes give the same numbers to the byte —
    forced onto two workers for a recording the automatic choice
    would keep in one process."""
    spec = _spec()
    history = _recording(spec, noise=0.5)
    serial = extract_sine(history, spec, cycles=20.0, workers=1)
    pooled = extract_sine(history, spec, cycles=20.0, workers=2)
    assert serial.cycles == pooled.cycles
    for ours, theirs in zip(serial, pooled):
        assert np.array_equal(ours.ordinate, theirs.ordinate)
        assert np.array_equal(ours.floor, theirs.floor)
        assert np.array_equal(ours.below_floor, theirs.below_floor)
        assert ours.drift_hz == theirs.drift_hz


def test_small_work_stays_in_one_process():
    """The workers take a second each to start; a recording under the
    floor is not worth them, and the automatic choice says so."""
    from hertzforge.core import sine

    spec = _spec()
    history = _recording(spec, noise=0.5)
    assert len(history.abscissa) * 2 < sine.PARALLEL_FLOOR
    assert sine._workers(None) >= 1
    assert sine._workers(3) == 3


def test_the_pieces_really_run_in_the_workers(monkeypatch):
    """A worker imports the real module, so breaking the task in this
    process breaks the serial path and not the pooled one — which is
    how the test knows the work left the process."""
    from hertzforge.core import sine

    def boom(*_args, **_kwargs):
        raise RuntimeError('ran in the parent')

    monkeypatch.setattr(sine, '_piece_task', boom)
    spec = _spec()
    history = _recording(spec, noise=0.5)
    with pytest.raises(RuntimeError, match='ran in the parent'):
        extract_sine(history, spec, cycles=20.0, workers=1)
    levels = extract_sine(history, spec, cycles=20.0, workers=2)
    assert len(levels) == 2


def _slow_sweep(amplitude=1.0, noise=2.0, seed=2, rate_hz=8192.0, seconds=60.0):
    """A log sweep from 5 Hz at a controller's rate: the case where a
    piece solved at the record's rate ran to tens of millions of
    samples — the margin at the low end reached across the sweep and
    the banded solve wanted a hundred gigabytes."""
    octaves_per_minute = np.log2(100.0) / ((seconds - 5.0) / 60.0)
    tone = SineTone('Up', 1.0, [5.0, 500.0], [[amplitude] * 2] * 2, [1],
                    [octaves_per_minute])
    spec = SineSweepSpecification([tone], ['101Z+', '104Z+'],
                                  ordinate_unit='m/s**2')
    n = int(seconds * rate_hz)
    rng = np.random.default_rng(seed)
    signal = rng.standard_normal((2, n)) * noise
    argument = tone.argument(1.0 / rate_hz)
    start = int(rate_hz)
    m = min(len(argument), n - start)
    signal[:, start:start + m] += amplitude * np.cos(argument[:m])
    history = TimeHistory(np.arange(n) / rate_hz, signal,
                          response_dof=['101Z+', '104Z+'],
                          ordinate_dim=['acceleration'] * 2,
                          ordinate_unit=['m/s**2'] * 2)
    return history, spec


def test_the_margin_is_cycles_of_the_tone():
    """`reach` walks the sweep by phase: so many cycles before or after
    a sample, from the closed-form phase, and never past the span."""
    from hertzforge.core.sine import _Laid

    history, spec = _slow_sweep()
    tone = spec.tones[0]
    dt = 1.0 / history.sample_rate
    placed = _Laid(tone, 1.0, int(history.sample_rate), 1 + sum(
        max(round(s / dt), 1) for s in tone.segment_seconds()), dt, 40.0)
    k = placed.n // 2
    ahead = placed.reach(k, 100.0, +1)
    behind = placed.reach(k, 100.0, -1)
    t = tone.grid_at(dt, [behind, k, ahead])
    phase = tone.phase_at(t) / (2.0 * np.pi)
    assert phase[2] - phase[1] == pytest.approx(100.0, abs=0.01)
    assert phase[1] - phase[0] == pytest.approx(100.0, abs=0.01)
    assert placed.reach(0, 1e9, -1) == 0 and placed.reach(0, 1e9, +1) == placed.n
    # at the low end a hundred cycles are many samples, at the high end few
    assert placed.reach(0, 100.0, +1) > 10 * (placed.n - placed.reach(placed.n - 1, 100.0, -1))


def test_a_slow_sweep_at_a_high_rate_is_solved_small_and_reads_true(monkeypatch):
    """The 23 GB case in miniature: a log sweep from 5 Hz at 8 kHz,
    chunked finely enough that the sweep is many pieces. Each piece is
    decimated to the rate its highest tone needs and its margin is
    cycles of the tone, so the largest piece the solver sees is a few
    thousand cycles at a few samples each — where at the record's rate
    the margin at the low end reached most of the sweep (the first
    piece alone was gigabytes on the real file) — and the planted level
    still reads back.
    """
    import tracemalloc

    from hertzforge.core import sine

    history, spec = _slow_sweep(seconds=240.0)
    sizes = []
    real = sine._piece_task
    monkeypatch.setattr(sine, 'CHUNK', 4096)
    monkeypatch.setattr(sine, '_piece_task',
                        lambda y, *rest: sizes.append(len(y)) or real(y, *rest))
    tracemalloc.start()
    level = next(iter(extract_sine(history, spec, cycles=40.0, workers=1)))
    peak = tracemalloc.get_traced_memory()[1]
    tracemalloc.stop()
    # measured 2026-10-01: decimated, 122 pieces of at most 46k samples
    # and a 168 MB peak; at the record's rate 942 pieces of up to 897k
    # samples, a 625 MB peak, and twenty-five times the run time
    assert len(sizes) > 50 and max(sizes) < 60_000, (len(sizes), max(sizes))
    assert peak < 300e6, peak
    inside = (level.abscissa > 8.0) & (level.abscissa < 400.0)
    magnitude = np.abs(level.ordinate[0, inside])
    assert level.resolved[0, inside].all()
    assert abs(20 * np.log10(np.median(magnitude))) < 0.3


def test_decimation_reads_what_the_full_rate_reads():
    """A piece solved at a decimated rate gives the reading the
    full-rate solve gives, to a tenth of a dB: the tone is still the
    tone below the decimated Nyquist, and the floor is the noise the
    smoothing lets through at either rate."""
    from hertzforge.core import sine

    history, spec = _slow_sweep(seconds=30.0)
    decimated = next(iter(extract_sine(history, spec, cycles=40.0, workers=1)))
    monkeypatch_value = sine.SAMPLES_PER_CYCLE
    try:
        sine.SAMPLES_PER_CYCLE = 10 ** 9           # never decimate
        full = next(iter(extract_sine(history, spec, cycles=40.0, workers=1)))
    finally:
        sine.SAMPLES_PER_CYCLE = monkeypatch_value
    inside = (decimated.abscissa > 10.0) & (decimated.abscissa < 400.0)
    assert np.allclose(decimated.abscissa, full.abscissa)
    ratio = np.abs(decimated.ordinate[0, inside]) / np.abs(full.ordinate[0, inside])
    assert np.abs(20 * np.log10(np.median(ratio))) < 0.1
    assert np.percentile(np.abs(20 * np.log10(ratio)), 95) < 0.5


def _nine_bands(rate_hz=8192.0, seconds=60.0, noise=1.0, seed=3):
    """Nine tones sweeping nine bands at once on two channels — the
    shape of a multi-tone run, where every tone solved with every
    other at the rate the highest needed was the 90 GB worker."""
    bands = [(5, 20), (12, 48), (30, 120), (80, 320), (150, 600),
             (300, 1200), (500, 1800), (1000, 2600), (1800, 3200)]
    opm = [np.log2(hi / lo) / ((seconds - 5.0) / 60.0) for lo, hi in bands]
    tones = [SineTone(f'Tone {i + 1}', 1.0, [lo, hi], [[1.0] * 2] * 2, [1], [r])
             for i, ((lo, hi), r) in enumerate(zip(bands, opm))]
    spec = SineSweepSpecification(tones, ['101Z+', '104Z+'],
                                  ordinate_unit='m/s**2')
    n = int(seconds * rate_hz)
    dt = 1.0 / rate_hz
    rng = np.random.default_rng(seed)
    signal = rng.standard_normal((2, n)) * noise
    start = int(rate_hz)
    for tone in tones:
        argument = tone.argument(dt)
        m = min(len(argument), n - start)
        signal[:, start:start + m] += np.cos(argument[:m])
    history = TimeHistory(np.arange(n) / rate_hz, signal,
                          response_dof=['101Z+', '104Z+'],
                          ordinate_dim=['acceleration'] * 2,
                          ordinate_unit=['m/s**2'] * 2)
    return history, spec, bands


def test_tones_apart_in_frequency_are_solved_apart_and_read_true(monkeypatch):
    """Nine bands, nine solves of one tone each, every one at its own
    decimated rate — and every planted level reads back."""
    from hertzforge.core import sine

    history, spec, bands = _nine_bands()
    lives, sizes = [], []
    real = sine._piece_task

    def spy(y, args, *rest):
        lives.append(len(args))
        sizes.append(len(y))
        return real(y, args, *rest)

    monkeypatch.setattr(sine, '_piece_task', spy)
    levels = extract_sine(history, spec, cycles=40.0, workers=1, refine=False)
    assert max(lives) == 1, 'a tone in its own band shares no solve'
    # the lowest band at eight samples a cycle of 20 Hz is a few
    # thousand samples for the minute; the top band is the record
    assert max(sizes) < 1.2 * sine.CHUNK, max(sizes)
    for level, (lo, hi) in zip(levels, bands):
        inside = (level.abscissa > lo * 1.3) & (level.abscissa < hi / 1.3)
        magnitude = np.abs(level.ordinate[:, inside])
        assert level.resolved[:, inside].all(), level.tone
        assert abs(20 * np.log10(np.median(magnitude))) < 0.3, level.tone


def test_tones_that_meet_share_a_solve_and_tones_apart_do_not():
    """`_tone_groups`: an up sweep and a down sweep that cross are one
    group; two sweeps in bands an octave apart are two."""
    from hertzforge.core.sine import _Laid, _tone_groups

    dt = 1.0 / 8192.0
    opm = np.log2(100.0) / 1.0

    def placed(name, lo, hi):
        tone = SineTone(name, 1.0, [lo, hi], [[1.0]] * 2, [1],
                        [opm if hi > lo else -opm])
        n = 1 + sum(max(round(s / dt), 1) for s in tone.segment_seconds())
        return _Laid(tone, 1.0, 8192, n, dt, 40.0)

    up, down = placed('Up', 5.0, 500.0), placed('Down', 500.0, 5.0)
    assert _tone_groups([up, down], 40.0) == [[0, 1]]
    low, high = placed('Low', 5.0, 20.0), placed('High', 500.0, 2000.0)
    assert _tone_groups([low, high], 40.0) == [[0], [1]]
    # close, not crossing: a tone a bandwidth above another all the way
    near = placed('Near', 5.0 * 1.02, 500.0 * 1.02)
    assert _tone_groups([up, near, high], 40.0) == [[0, 1], [2]]


def test_the_extraction_tells_its_progress():
    """`progress` hears (done, total): the total grows as each stage is
    sized, done never falls, and the last word is done == total."""
    history, spec = _flat_sweep(noise=1.0)
    heard = []
    extract_sine(history, spec, workers=1, progress=lambda d, t: heard.append((d, t)))
    assert heard, 'nothing heard'
    done = [d for d, _t in heard]
    assert done == sorted(done)
    assert all(d <= t for d, t in heard)
    assert heard[-1][0] == heard[-1][1] > 0
    # the matched filter per tone, the ladder's samples, then the solve:
    # the first word is well short of the last total
    assert heard[0][1] < heard[-1][1]


def test_what_the_progress_raises_stops_the_extraction():
    """A cancel is the callback raising; nothing swallows it."""
    history, spec = _flat_sweep(noise=1.0)

    class Stop(Exception):
        pass

    def cancel(done, total):
        if done > 0:
            raise Stop

    with pytest.raises(Stop):
        extract_sine(history, spec, workers=1, progress=cancel)


def _levels_against(spec_dim='acceleration', spec_unit='m/s**2',
                    history_declared=True):
    """A clean flat sweep's levels against its specification, with the
    specification's quantity and unit, and the record's units, set."""
    history, spec = _flat_sweep(noise=0.1)
    spec.ordinate_dim, spec.ordinate_unit = spec_dim, spec_unit
    if not history_declared:
        history = TimeHistory(history.abscissa, history.ordinate,
                              response_dof=list(history.response_dof),
                              ordinate_dim=['unknown'] * 2)
    return spec, extract_sine(history, spec, cycles=10.0)


@pytest.mark.parametrize('case,reason', [
    ('undeclared tone', ('the units of the tone are not declared, so its '
     'levels cannot be read against the other; set its units first')),
    ('velocity tone', ('the tone is of velocity and the level of '
     'acceleration, which cannot be compared')),
])
def test_a_sine_level_that_cannot_be_compared_is_refused(case, reason):
    """A specification whose unit is None holds its amplitudes as
    written, so a level in SI cannot be scored against it; nor can a
    level of another quantity. Each keeps its row and says why."""
    from hertzforge.core.compliance import sine_errors

    spec, levels = _levels_against(
        **({'spec_unit': None} if case == 'undeclared tone'
           else {'spec_dim': 'velocity'}))
    rows = sine_errors(spec, levels)
    assert [(row.dof, row.tone) for row in rows] == [('101Z+', 'Up'),
                                                     ('104Z+', 'Up')]
    assert all(np.isnan(row.db) and row.refused == reason for row in rows)


def test_two_undeclared_sides_are_scored_and_say_so():
    from hertzforge.core.compliance import RAW_NOTE, sine_errors

    spec, levels = _levels_against(spec_unit=None, history_declared=False)
    rows = sine_errors(spec, levels)
    assert rows and all(row.refused is None and row.note == RAW_NOTE
                        for row in rows)
    assert all(abs(row.db) < 0.5 for row in rows), 'a clean sweep on target'
    spec, levels = _levels_against()
    assert all(row.note is None for row in sine_errors(spec, levels))


def test_an_undeclared_specification_gives_an_undeclared_curve():
    """A specification whose unit is None holds its amplitudes as
    written. Its tone curve claimed m/s**2 for them, because a DataArray
    reads a named quantity with no unit as SI; it is undeclared now,
    the quantity kept as its hint, and the numbers are unchanged."""
    _history, spec = _flat_sweep()
    spec.ordinate_unit = None
    curve = spec.tone_curve('Up')
    assert curve.ordinate_dim == ['unknown', 'unknown']
    assert curve.ordinate_unit == [None, None]
    assert curve.dimension_hint == ['acceleration', 'acceleration']
    assert np.allclose(curve.ordinate, 2.0)
    spec.ordinate_unit = 'm/s**2'
    declared = spec.tone_curve('Up')
    assert declared.ordinate_dim == ['acceleration', 'acceleration']
    assert declared.ordinate_unit == ['m/s**2', 'm/s**2']
