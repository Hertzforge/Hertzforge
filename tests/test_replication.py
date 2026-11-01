"""How closely a transient replicated the waveform it was controlled to.

A transient specification is one waveform played over and over, so the
comparison begins by cutting the record back into those repeats. Where
they are is derived, not read: the file describes the repeats nowhere,
and the parameters it *does* carry describe a different measurement.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge.core.averaging import Averaging
from hertzforge.core.data import (
    ShockSpecification,
    TimeHistory,
    TransientSpecification,
)
from hertzforge.core.replication import (
    SRS_FLOOR_DB,
    ZERO_TARGET_DB,
    averaging_for,
    compare,
    lag_of,
)
from hertzforge.core.replication import leftover as rep_leftover
from hertzforge.core.replication import playings as rep_playings

RATE = 2048.0

FRAME = 1024


def _pulse(samples=FRAME, onset=None, peak=50.0, decay=90.0, ring=180.0):
    """A decaying impulse response — the shape a transient target is."""
    onset = samples // 8 if onset is None else onset
    t = np.arange(samples) / RATE
    out = np.zeros(samples)
    after = t[onset:] - t[onset]
    out[onset:] = peak * np.exp(-decay * after) * np.sin(2 * np.pi * ring
                                                         * after)
    return out


def _target(dofs=('1Z+', '2Z+'), scales=(1.0, 0.4)):
    rows = np.array([_pulse() * s for s in scales])
    spec = TransientSpecification(np.arange(FRAME) / RATE, rows,
                                  response_dof=list(dofs))
    spec.define_units('m/s^2')
    return spec


def _measured(spec, repeats=4, gain=1.0, lag=0, extra=0, noise=None):
    """The target played `repeats` times, optionally shifted and scaled."""
    rows = []
    for row in spec.ordinate:
        record = np.tile(row * gain, repeats)
        if extra:
            record = np.concatenate([record, row[:extra] * gain])
        rows.append(record)
    rows = np.asarray(rows)
    if lag:
        rows = np.concatenate([np.zeros((rows.shape[0], lag)), rows], axis=1)
    if noise is not None:
        rows = rows + noise
    history = TimeHistory(np.arange(rows.shape[1]) / RATE, rows,
                          response_dof=list(spec.response_dof))
    history.define_units('m/s^2')
    return history


def test_the_frame_is_the_specification_and_nothing_else_is_guessed():
    """Four of the five averaging numbers are settled by construction.

    A repeat plays the control signal, so the frame is its length; the
    repeats run back to back, so there is no overlap; a taper would
    distort the waveform being replicated, so the window is
    rectangular; and the record starts where it starts. Only the count
    is measured, and measuring it is division.
    """
    spec = _target()
    averaging = averaging_for(_measured(spec, repeats=4), spec)
    assert averaging == Averaging(frame_length=FRAME, overlap=0.0,
                                  window='rectangle', frames=4, start=0.0)


def test_a_part_played_repeat_is_not_an_average():
    """A profile whose stop lands near a frame boundary leaves a
    fraction of a repeat on the end. A fraction of a frame is not an
    average, and `Averaging` will not hold one either."""
    spec = _target()
    history = _measured(spec, repeats=3, extra=FRAME // 2)
    averaging = averaging_for(history, spec)
    assert averaging.frames == 3, 'the half repeat is not a fourth average'
    assert averaging.span == 3 * FRAME
    assert history.ordinate.shape[1] - averaging.span == FRAME // 2


def test_a_record_shorter_than_one_repeat_has_no_averaging():
    spec = _target()
    short = TimeHistory(np.arange(FRAME // 2) / RATE,
                        spec.ordinate[:, :FRAME // 2],
                        response_dof=list(spec.response_dof))
    assert averaging_for(short, spec) is None
    assert compare(short, spec) == []


@pytest.mark.parametrize('shift', [0, 1, 3, 7])
def test_the_shift_between_target_and_record_is_recovered(shift):
    spec = _target()
    history = _measured(spec, repeats=3, lag=shift)
    assert lag_of(history, spec) == shift


def test_one_shift_is_found_for_every_channel_together():
    """The delay belongs to the acquisition, not to a channel.

    Given its own shift each channel would hunt for the alignment that
    flattered it most and report the error it found there, which is a
    measurement of the search rather than of the test.
    """
    spec = _target()
    history = _measured(spec, repeats=3, lag=4)
    # wreck the second channel; the shared shift must not move
    history.ordinate[1] = np.roll(history.ordinate[1], 9)
    assert lag_of(history, spec) == 4


def test_alignment_is_applied_before_anything_is_compared():
    spec = _target()
    shifted = _measured(spec, repeats=3, lag=5)
    aligned = compare(shifted, spec)
    assert max(r['waveform'] for r in aligned) < 1e-6
    ignored = compare(shifted, spec, lag=0)
    assert min(r['waveform'] for r in ignored) > 10.0, (
        'unaligned, a perfect replication reads as a bad one')


def test_a_channel_whose_target_is_silent_cannot_drag_the_shift():
    spec = _target(scales=(1.0, 0.0))
    history = _measured(spec, repeats=3, lag=2)
    assert lag_of(history, spec) == 2


def test_a_perfect_replication_scores_zero_on_all_three():
    spec = _target()
    rows = compare(_measured(spec, repeats=4), spec)
    assert [r['label'] for r in rows] == ['1Z+'] * 4 + ['2Z+'] * 4, (
        'every repeat is reported and none is singled out')
    assert sorted({r['frame'] for r in rows}) == [0, 1, 2, 3]
    for row in rows:
        assert row['waveform'] == pytest.approx(0.0, abs=1e-9)
        assert row['srs'] == pytest.approx(0.0, abs=1e-9)
        assert row['level'] == pytest.approx(0.0, abs=1e-9)


def test_level_reads_a_scaling_and_waveform_reads_it_too():
    """Doubling every sample is 6.02 dB, and the two readings part
    company: the level is exactly the scaling, while the waveform error
    is the whole target's worth of difference."""
    spec = _target()
    rows = compare(_measured(spec, repeats=3, gain=2.0), spec)
    for row in rows:
        assert row['level'] == pytest.approx(20 * np.log10(2.0), abs=1e-6)
        assert row['srs'] == pytest.approx(20 * np.log10(2.0), abs=1e-6)
        assert row['waveform'] == pytest.approx(100.0, abs=1e-6)


def test_level_is_blind_to_a_shape_the_waveform_error_catches():
    """The two readings earn their places by disagreeing.

    Reversing a frame in time keeps every sample and so keeps the
    level exactly; the waveform is a different waveform, and says so.
    """
    spec = _target()
    history = _measured(spec, repeats=2)
    history.ordinate[:, FRAME:2 * FRAME] = history.ordinate[:, :FRAME][:, ::-1]
    for row in compare(history, spec, frame=1):
        assert row['level'] == pytest.approx(0.0, abs=1e-9), (
            'the same samples in another order hold the same power')
        assert row['waveform'] > 50.0, 'and are not the same waveform'


def test_level_counts_a_steady_offset_once():
    """The level is the ratio of the two frames' mean squares, a steady
    offset included. Until 2026-09-23 it was worked out from a private
    PSD that doubled the DC and Nyquist lines along with the rest, so an
    offset counted twice; here that read 18.5 dB for a true 15.5."""
    spec = _target()
    history = _measured(spec, repeats=2)
    offset = 0.5 * np.abs(spec.ordinate).max()
    history.ordinate[:, FRAME:2 * FRAME] += offset
    target = np.asarray(spec.ordinate)
    measured = history.ordinate[:, FRAME:2 * FRAME]
    want = 10 * np.log10(np.mean(measured ** 2, axis=1)
                         / np.mean(target ** 2, axis=1))
    rows = [r for r in compare(history, spec, frame=1) if r['frame'] == 1]
    assert [r['level'] for r in rows] == pytest.approx(want, abs=1e-9)
    assert want.min() > 1.0, 'the offset is a real part of the level'


def test_every_repeat_is_reported_and_none_is_singled_out():
    """Which playing was the bad one is a judgment about what the
    article is for, not a measurement. An earlier version reduced these
    to the worst repeat and labeled it so."""
    spec = _target()
    history = _measured(spec, repeats=4)
    history.ordinate[:, 2 * FRAME:3 * FRAME] *= 2.0
    rows = compare(history, spec)
    for row in rows:
        expected = 100.0 if row['frame'] == 2 else 0.0
        assert row['waveform'] == pytest.approx(expected, abs=1e-6)
    assert not any(key.endswith('_frame') for row in rows for key in row), (
        'no reading carries a repeat it decided was the worst')


def test_a_target_that_asks_for_nothing_is_not_scored_against():
    """Every metric here is a ratio against the target. Against a
    target of nothing they do not become large so much as stop meaning
    anything: the survey's lateral control channel is 230 dB below
    the loudest and read 1.3e12 percent before this existed."""
    spec = _target(scales=(1.0, 0.0))
    rows = compare(_measured(spec, repeats=3), spec)
    quiet = next(r for r in rows if r['label'] == '2Z+')
    assert quiet['zero_target'] is True
    for key in ('waveform', 'srs', 'level'):
        assert np.isnan(quiet[key])
    loud = next(r for r in rows if r['label'] == '1Z+')
    assert loud['zero_target'] is False
    assert loud['waveform'] == pytest.approx(0.0, abs=1e-9), (
        'and the silent channel does not spoil the one beside it')


def test_the_silence_threshold_is_relative_to_the_loudest_target():
    """Quiet is not silent. A control channel genuinely asked for at a
    low level is still asked for, and is scored."""
    quiet = 10.0 ** (-(ZERO_TARGET_DB - 20.0) / 20.0)
    spec = _target(scales=(1.0, quiet))
    rows = compare(_measured(spec, repeats=3), spec)
    assert not any(r['zero_target'] for r in rows)


def test_the_srs_is_compared_only_where_the_target_asks_for_something():
    """The bottom of an SRS band answers what a very low oscillator did
    during a short record, which for a shock is residual drift rather
    than the shock. On the survey the unmasked band reported 21 dB of
    deviation from a target 80 dB below its own peak; masked, the same
    channel reads 6.9 dB, which is the real answer.
    """
    spec = _target()
    history = _measured(spec, repeats=2)
    # a slow drift, well below the ringing the target is made of
    t = np.arange(history.ordinate.shape[1]) / RATE
    history.ordinate += 0.2 * np.sin(2 * np.pi * 2.2 * t)
    masked = max(abs(r['srs']) for r in compare(history, spec))

    from hertzforge.core import replication
    original = replication.SRS_FLOOR_DB
    try:
        replication.SRS_FLOOR_DB = 1e6          # compare everywhere
        everywhere = max(abs(r['srs']) for r in compare(history, spec))
    finally:
        replication.SRS_FLOOR_DB = original
    assert everywhere > 10.0, (
        'unmasked, the drift is read as a replication failure')
    assert masked < everywhere - 5.0, (
        'the mask drops the bands where the target asks for nothing')
    assert original == SRS_FLOOR_DB


def _transient_file(path, repeats=3, frame=128, rate=256.0, extra=0):
    """A streamed transient run, carrying the `sysid_*` trap.

    Built here rather than taken from a fixture so the numbers going in
    are known — and so the sysid attributes are certainly present, this
    being the thing the importer must not read.
    """
    import netCDF4

    signal = np.stack([_pulse(frame, peak=50.0),
                       _pulse(frame, peak=20.0)])
    record = np.tile(signal, repeats)
    if extra:
        record = np.concatenate([record, signal[:, :extra]], axis=1)
    with netCDF4.Dataset(path, 'w') as ds:
        ds.sample_rate = rate
        ds.createDimension('response_channels', 2)
        ds.createDimension('time_samples', record.shape[1])
        ds.createDimension('num_environments', 1)
        channels = ds.createGroup('channels')
        for name, values in (('node_number', ['101', '111']),
                             ('node_direction', ['Z+', 'Z+']),
                             ('channel_type', ['acceleration'] * 2),
                             ('unit', ['m/s^2'] * 2)):
            variable = channels.createVariable(name, str,
                                               ('response_channels',))
            for i, value in enumerate(values):
                variable[i] = value
        names = ds.createVariable('environment_names', str,
                                  ('num_environments',))
        names[0] = 'Drop'
        kinds = ds.createVariable('environment_types', 'i8',
                                  ('num_environments',))
        kinds[0] = 2                       # ENVIRONMENT_KINDS: transient
        data = ds.createVariable('time_data', 'f8',
                                 ('response_channels', 'time_samples'))
        data[:] = record
        group = ds.createGroup('Drop')
        # exactly the shelf Rattlesnake writes, describing the random
        # excitation it ran beforehand and not this test at all
        group.sysid_frame_size = frame // 4
        group.sysid_averages = 10
        group.sysid_overlap = 0.5
        group.sysid_window = 'Hann'
        group.createDimension('specification_channels', 2)
        group.createDimension('signal_samples', frame)
        control = group.createVariable(
            'control_signal', 'f8',
            ('specification_channels', 'signal_samples'))
        control[:] = signal
        indices = group.createVariable('control_channel_indices', 'i4',
                                       ('specification_channels',))
        indices[:] = [0, 1]
    return signal


def test_a_rattlesnake_transient_run_reads_its_own_frames(tmp_path):
    """The imported record carries the averaging its repeats imply,
    not the one the file's `sysid_*` attributes describe.

    Those attributes are the trap: Hann, half overlap and a frame a
    quarter of the true one. They belong to the random excitation
    Rattlesnake runs beforehand to measure the FRF it inverts, and the
    importer would have taken them — they are the third entry in
    `AVERAGING_KEYS` and nothing else in a transient file answers.
    """
    path = tmp_path / 'run.nc4'
    _transient_file(path, repeats=3, frame=128)
    project = hertzforge.Project()
    project.import_file(str(path))
    history = next(obj for obj in project.values()
                   if type(obj) is TimeHistory)
    spec = next(obj for obj in project.values()
                if isinstance(obj, TransientSpecification))
    assert history.averaging == Averaging(
        frame_length=128, overlap=0.0, window='rectangle', frames=3)
    assert history.averaging == averaging_for(history, spec)
    assert history.averaging.frame_length != 32, 'that is the sysid frame'
    assert history.averaging.window != 'hann', 'that is the sysid window'


def test_the_imported_run_compares_against_its_own_specification(tmp_path):
    """End to end: the file goes in, the three readings come out."""
    path = tmp_path / 'run.nc4'
    _transient_file(path, repeats=3, frame=128)
    project = hertzforge.Project()
    project.import_file(str(path))
    history = next(obj for obj in project.values()
                   if type(obj) is TimeHistory)
    spec = next(obj for obj in project.values()
                if isinstance(obj, TransientSpecification))
    rows = compare(history, spec)
    assert [r['label'] for r in rows] == ['101Z+'] * 3 + ['111Z+'] * 3
    for row in rows:
        assert row['waveform'] == pytest.approx(0.0, abs=1e-9)
        assert row['srs'] == pytest.approx(0.0, abs=1e-9)
        assert row['level'] == pytest.approx(0.0, abs=1e-9)


def test_a_random_run_still_reads_its_own_averaging():
    """The transient route must not capture a file that has no
    transient in it."""
    project = hertzforge.Project()
    project.import_file(fixture_path('plate', 'random.nc4'))
    history = next(obj for obj in project.values()
                   if type(obj) is TimeHistory)
    assert history.averaging is not None
    assert history.averaging.window != 'rectangle' or (
        history.averaging.overlap != 0.0)


def _bars(mode, history, spec):
    from hertzforge.report import _build_block

    block = {'kind': 'bars', 'mode': mode, 'source': 'spec',
             'measured': 'time', 'caption': 'c'}
    return _build_block(block, {'spec': spec, 'time': history},
                        hertzforge.SI, [])


def test_the_waveform_error_draws_as_bars():
    spec = _target()
    built = _bars('waveform', _measured(spec, repeats=3, gain=2.0), spec)
    assert built['kind'] == 'bars'
    assert built['labels'] == ['1Z+ e1', '1Z+ e2', '1Z+ e3',
                               '2Z+ e1', '2Z+ e2', '2Z+ e3'], (
        'a bar per channel per playing, the repeat named because there '
        'is more than one to tell apart')
    assert all(v == pytest.approx(100.0, abs=1e-4) for v in built['values'])


def test_one_repeat_needs_no_repeat_in_its_label():
    spec = _target()
    built = _bars('waveform', _measured(spec, repeats=1), spec)
    assert built['labels'] == ['1Z+', '2Z+']


@pytest.mark.parametrize('mode', ['srs', 'level'])
def test_only_the_waveform_error_is_read_from_the_time_data(mode):
    """A level wants two PSDs and an SRS deviation two spectra. Both
    are reached by computing them and comparing the pair — where a
    specification's own PSD is a Specification — so offering them here
    too would be two ways to one number, agreeing only by luck."""
    spec = _target()
    assert _bars(mode, _measured(spec, repeats=3), spec) is None


def test_a_one_sided_reading_carries_its_threshold_where_the_page_looks():
    """The page tells one-sided from two-sided by `high` being null,
    and reads the only threshold out of `low`. A waveform error put in
    `high` would shade nothing at all."""
    from hertzforge.core.replication import WAVEFORM_PERCENT

    spec = _target()
    history = _measured(spec, repeats=3)
    waveform = _bars('waveform', history, spec)
    assert waveform['high'] is None
    assert waveform['low'] == WAVEFORM_PERCENT
    assert waveform['floor'] == 0.0, 'an error in percent cannot go below 0'


def test_an_unscorable_channel_is_named_rather_than_dropped_silently():
    """A bar of zero would read as a channel that replicated perfectly,
    which is the opposite of what is known about it."""
    spec = _target(scales=(1.0, 0.0))
    built = _bars('waveform', _measured(spec, repeats=1), spec)
    assert built['labels'] == ['1Z+'], 'no bar it cannot earn'
    assert '2Z+' in built['caption']
    assert 'asked for nothing' in built['caption']


def test_an_unknown_reading_binds_to_nothing():
    spec = _target()
    assert _bars('trac', _measured(spec, repeats=3), spec) is None


def test_the_transient_report_asks_for_the_waveform_error(tmp_path):
    """The template's bars must bind on a real imported run, not just
    build in isolation."""
    from hertzforge.core.report import transient_template
    from hertzforge.report import _build_block

    path = tmp_path / 'run.nc4'
    _transient_file(path, repeats=3, frame=128)
    project = hertzforge.Project()
    project.import_file(str(path))
    report = transient_template(project, object_groups=project.object_groups)
    bars = [b for b in report.blocks if b['kind'] == 'bars']
    # no 'srs': that judgment moved to the shock report, where the
    # SRS is the test's own measure rather than a borrowed one
    # error first: spectral data and its RMS reading precede the
    # waveform judgment in the standing order (Brandon, 2026-08-23)
    assert [b['mode'] for b in bars] == ['error', 'waveform',
                                         'kurtosis']
    built = _build_block(bars[1], dict(project), hertzforge.SI, project.object_groups)
    assert built is not None
    assert built['labels'] == ['101Z+ e1', '101Z+ e2', '101Z+ e3',
                               '111Z+ e1', '111Z+ e2', '111Z+ e3']
    # the spectral pair binds once the spectra exist, and not before
    assert _build_block(bars[0], dict(project), hertzforge.SI,
                        project.object_groups) is None

    # the overlay: the first playing over its target, channel by channel
    overlay = next(b for b in report.blocks
                   if b.get('mode') == 'overlay')
    built = _build_block(overlay, dict(project), hertzforge.SI,
                         project.object_groups)
    assert built is not None
    assert built['logy'] is False, 'waveforms draw on linear axes'
    assert [ch['label'] for ch in built['channels']] == ['101Z+', '111Z+']
    assert 'playing 1 of 3' in built['curves'][1]['label']
    assert all(ch['zones'] == [] for ch in built['channels']), (
        'a target waveform carries no limits to shade')

    # the SRS deviation bars live in the shock template now — bound
    # here against the same run once its spectra exist, because a
    # transient run is the shock template's data too once the engineer
    # declares it one
    from hertzforge.core.report import shock_template

    for source in list(project):
        if type(project[source]).__name__ in ('TimeHistory',
                                              'TransientSpecification'):
            project.compute_srs(source)
    shock = shock_template(project, object_groups=project.object_groups)
    srs_bars = next(b for b in shock.blocks
                    if b.get('kind') == 'bars' and b.get('mode') == 'srs')
    srs = _build_block(srs_bars, dict(project), hertzforge.SI,
                       project.object_groups)
    assert srs is not None
    # the SRS reading, not the PSD one falling through: a spectrum has
    # no area to integrate, so an RMS-error-over-a-band label here would
    # mean the wrong comparison had been made
    assert srs['ylabel'] == 'SRS deviation, RMS across the band [dB]'
    # both sides: the deviation is signed now — an under-hit shock used
    # to read as an over-test (Brandon, 2026-08-20)
    from hertzforge.core.compliance import SRS_ERROR_DB

    assert srs['low'] == -SRS_ERROR_DB and srs['high'] == SRS_ERROR_DB
    assert 'floor' not in srs, 'a signed deviation reaches below zero'
    assert len(srs['labels']) == 2 * 3, 'a channel per playing'


def test_a_part_played_repeat_is_not_analyzed():
    """Rattlesnake repeats until told to stop, so a record rarely ends
    on a boundary. What is left is not a playing: a waveform error over
    part of a window is taken against a different stretch of the
    target, and a column of those invites comparing things that do not
    compare.
    """
    spec = _target()
    history = _measured(spec, repeats=3, extra=FRAME // 2)
    assert [p['frame'] for p in rep_playings(history, spec)] == [0, 1, 2]
    assert sorted({r['frame'] for r in
                   compare(history, spec, metrics=('waveform',))}) == [0, 1, 2]


def test_every_view_of_the_record_counts_the_same_playings():
    """The bug this replaced: the grid counted a part-played one as a
    seventh event while the shading, which comes from
    `Averaging.frame_bounds`, knew only about whole frames. Two views
    of one record giving two answers."""
    spec = _target()
    history = _measured(spec, repeats=3, extra=FRAME // 2)
    averaging = averaging_for(history, spec)
    assert (len(rep_playings(history, spec))
            == averaging.frames
            == len(averaging.frame_bounds(RATE))
            == 3)


def test_what_was_left_over_is_still_reported():
    """Not analyzed is not the same as not mentioned, which is where
    this started: a run that recorded two seconds of a further event
    should not have it vanish between a frame count and a file size."""
    spec = _target()
    history = _measured(spec, repeats=3, extra=FRAME // 2)
    samples, share = rep_leftover(history, spec)
    assert samples == FRAME // 2
    assert share == pytest.approx(0.5)


def test_a_record_ending_on_a_boundary_has_nothing_left_over():
    spec = _target()
    assert rep_leftover(_measured(spec, repeats=3), spec) is None


def test_a_psd_of_a_target_is_averaged_over_every_playing(tmp_path):
    """Which is why the PSD comparison needs no event chooser: the
    repeats are already in it. `compute_psds` averages over exactly the
    frames the averaging defines, and for a transient run those frames
    are the playings."""
    from dataclasses import replace

    path = tmp_path / 'run.nc4'
    _transient_file(path, repeats=3, frame=128)
    project = hertzforge.Project()
    project.import_file(str(path))
    history = next(o for o in project.values() if type(o) is TimeHistory)
    assert history.averaging.frames == 3
    history.ordinate[:, 128:2 * 128] *= 4.0      # spoil the middle one
    averaged = history.compute_psds()
    alone = history.compute_psds(replace(history.averaging, frames=1))
    assert not np.allclose(averaged.ordinate, alone.ordinate), (
        'the average of three playings is not the first of them'
    )


def test_narrowing_the_averaging_narrows_the_comparison():
    """The panel is where a user says which events to analyze. Left
    unread, the shading on the time history said three and every number
    beside it was still worked out from six."""
    from dataclasses import replace

    spec = _target()
    history = _measured(spec, repeats=5)
    assert len(rep_playings(history, spec)) == 5
    history.averaging = replace(averaging_for(history, spec), frames=3)
    assert [p['frame'] for p in rep_playings(history, spec)] == [0, 1, 2]
    assert sorted({r['frame'] for r in
                   compare(history, spec, metrics=('waveform',))}) == [0, 1, 2]


def test_the_start_of_the_analysis_moves_the_playings():
    from dataclasses import replace

    spec = _target()
    history = _measured(spec, repeats=4)
    history.averaging = replace(averaging_for(history, spec), frames=2,
                                start=FRAME / RATE)
    starts = [p['start'] for p in rep_playings(history, spec)]
    assert starts == [FRAME, 2 * FRAME]


def test_an_averaging_about_something_else_is_not_honored():
    """A frame that is not the specification's length does not line up
    with a playing of it, and honoring it would compare each event
    against a slice of the target chosen by accident."""
    from dataclasses import replace

    spec = _target()
    history = _measured(spec, repeats=4)
    history.averaging = replace(averaging_for(history, spec),
                                frame_length=FRAME // 4, frames=9)
    derived = averaging_for(history, spec)
    assert derived.frame_length == FRAME
    assert derived.frames == 4, 'it fell back to what the playings are'


def test_what_was_left_over_does_not_move_with_the_choice():
    """Choosing to look at three of six is a decision and needs no
    announcing; a recording that stopped part way through a playing is
    a fact about the file."""
    from dataclasses import replace

    spec = _target()
    history = _measured(spec, repeats=4, extra=FRAME // 2)
    before = rep_leftover(history, spec)
    history.averaging = replace(averaging_for(history, spec), frames=2)
    assert rep_leftover(history, spec) == before == (FRAME // 2, 0.5)


def test_the_srs_of_a_transient_run_is_windowed_on_its_frames():
    """One curve per channel per playing, from the frames the averaging
    already defines — not from whatever a detector makes of the same
    record."""
    spec = _target()
    history = _measured(spec, repeats=4)
    history.averaging = averaging_for(history, spec)
    srs = history.compute_srs()
    assert srs.num_records == 4 * len(history.response_dof)
    assert sorted(set(srs.block)) == ['shock 1', 'shock 2', 'shock 3',
                                      'shock 4']


def test_the_srs_of_a_specification_is_one_playing():
    """It is a single playing of a waveform, whole."""
    spec = _target()
    assert spec.averaging is None and not spec.shocks
    srs = spec.compute_srs()
    assert srs.num_records == len(spec.response_dof)
    assert isinstance(srs, ShockSpecification)


def test_detected_shocks_still_win_where_there_are_any():
    """A shock record's events really do have to be found."""
    from hertzforge.core.shocks import Shock

    spec = _target()
    history = _measured(spec, repeats=4)
    history.averaging = averaging_for(history, spec)
    history.shocks = (Shock(0.0, FRAME / RATE),)
    assert history.compute_srs().num_records == len(history.response_dof)


def test_computing_an_srs_does_not_go_hunting_for_events():
    """Detection is the last resort, not the first. A record read as
    frames already says where its events are, and set loose on the
    survey run the detector answered with thirty-one where there were
    six — and cut the target, which is one playing, into three."""
    spec = _target()
    history = _measured(spec, repeats=3)
    history.averaging = averaging_for(history, spec)
    project = hertzforge.Project()
    project.add('Response', history)
    project.add('Target', spec)
    project.compute_srs('Response')
    project.compute_srs('Target')
    assert not history.shocks, 'the frames already said where they are'
    assert not spec.shocks, 'a target is one playing by definition'
    assert project['Response SRS'].num_records == 3 * len(history.response_dof)
    assert project['Target SRS'].num_records == len(spec.response_dof)


def test_a_perfect_replication_has_no_srs_deviation():
    from hertzforge.core.compliance import srs_errors

    spec = _target()
    history = _measured(spec, repeats=2)
    history.averaging = averaging_for(history, spec)
    for row in srs_errors(spec.compute_srs(), history.compute_srs()):
        assert row.db == pytest.approx(0.0, abs=1e-9)


def test_every_computed_spectrum_is_drawn_flat_in_band(tmp_path):
    """A PSD is a density sampled once per bin, so it is drawn flat
    across each — the area drawn is the area counted when it is
    integrated.

    The shape used to be chosen by class, and `Specification` means
    breakpoints of a power law. True of one written by hand; false of
    one computed from a record. The PSD of a transient target is four
    thousand lines of density and was being drawn as a power law
    through themselves.
    """
    from hertzforge.core.data import Psd, Specification

    path = tmp_path / 'run.nc4'
    _transient_file(path, repeats=3, frame=128)
    project = hertzforge.Project()
    project.import_file(str(path))
    for name, obj in list(project.items()):
        if isinstance(obj, TimeHistory):
            project.compute_psds(name)
    project.compute_cpsds(
        next(n for n, o in project.items() if type(o) is TimeHistory))
    computed = [o for o in project.values() if isinstance(o, Psd)]
    assert len(computed) >= 3
    for spectrum in computed:
        assert spectrum.interpolation == 'bin', (
            f'{type(spectrum).__name__} is a density')
    # and the target's own PSD is still a Specification, so the app
    # still knows which of the two is the requirement
    assert any(isinstance(o, Specification) for o in computed)


def test_a_written_specification_stays_a_power_law():
    """Its points are breakpoints, and drawing them as steps would make
    a staircase of a slope. A controller's target on its lines is the
    other thing — a density per line, stepped — and the spacing tells
    them apart at import (Brandon, 2026-09-19); this test pinned the
    lines one as breakpoints until then."""
    from hertzforge.core.data import Specification

    project = hertzforge.Project()
    project.import_file(fixture_path('plate', 'random.nc4'))
    lines = next(o for o in project.values() if type(o) is Specification)
    assert lines.interpolation == 'bin', "the controller's lines"
    written = Specification(np.array([20.0, 80.0, 800.0, 2000.0]),
                            np.array([[1e-3, 4e-3, 4e-3, 1e-3]]),
                            response_dof=['101Z+'],
                            ordinate_dim=['acceleration**2/frequency'])
    assert written.interpolation == 'log_log'


def test_whether_it_is_a_density_survives_the_project_file(tmp_path):
    """It cannot be worked out again from the numbers: a specification
    computed from a record and one written by hand can both arrive as
    several hundred evenly spaced lines, and only the first is a
    density. Left behind, a reopened project drew one as a power law.
    """
    path = tmp_path / 'run.nc4'
    _transient_file(path, repeats=3, frame=128)
    project = hertzforge.Project()
    project.import_file(str(path))
    target = next(n for n, o in project.items()
                  if isinstance(o, TransientSpecification))
    added = project.compute_psds(target)
    assert project[added].interpolation == 'bin'
    saved = project.save(tmp_path / 'p.vdyn')
    assert hertzforge.Project.open(saved)[added].interpolation == 'bin'


def test_an_under_hit_shock_reads_negative_not_over():
    """Brandon, on the drone stress set: the bar chart said the SRS was
    outside 3 dB *high* when the spectrum was actually low. The RMS was
    unsigned — a spectrum 6 dB low everywhere reported +6, shaded past
    the ceiling, and read as an over-test."""
    import copy

    from hertzforge.core.compliance import srs_errors

    spec = _target().compute_srs()
    quiet = copy.deepcopy(spec)
    quiet.ordinate = spec.ordinate * 10 ** (-6.0 / 20.0)   # 6 dB low
    loud = copy.deepcopy(spec)
    loud.ordinate = spec.ordinate * 10 ** (6.0 / 20.0)     # 6 dB high

    for _dof, _block, value, *_ in srs_errors(spec, quiet):
        assert value == pytest.approx(-6.0, abs=1e-6), \
            'an under-hit reads negative'
    for _dof, _block, value, *_ in srs_errors(spec, loud):
        assert value == pytest.approx(6.0, abs=1e-6), \
            'an over-test reads positive'


def test_the_search_for_the_shift_reaches_two_percent_of_a_frame():
    """Bounded on purpose: 2 % of the frame either side, 20 samples of
    1024. A shift at the edge is found; one past it is not — searching
    further was never tested, and neither was searching less."""
    spec = _target()
    assert lag_of(_measured(spec, repeats=3, lag=20), spec) == 20
    assert lag_of(_measured(spec, repeats=3, lag=21), spec) != 21


def test_a_shift_that_fits_with_nothing_to_spare_is_found():
    """A record exactly one playing and its delay long: the delayed
    playing ends on the last sample, and still counts."""
    spec = _target()
    once = _measured(spec, repeats=1, lag=3)
    assert once.ordinate.shape[1] == FRAME + 3
    assert lag_of(once, spec) == 3


def test_the_shift_is_searched_about_a_held_start_and_both_ways():
    """An analysis held three samples past the third playing's first
    sample is aligned by moving back three. The search used to start at
    sample 0 whatever the start, and to go forward only."""
    from dataclasses import replace

    spec = _target()
    history = _measured(spec, repeats=4)
    held = replace(Averaging(frame_length=FRAME, overlap=0.0,
                             window='rectangle', frames=1),
                   start=(2 * FRAME + 3) / RATE)
    assert lag_of(history, spec, held) == -3
    assert rep_playings(history, spec, held)[0]['start'] == 2 * FRAME


def test_nothing_to_align_against_is_no_shift():
    spec = _target()
    short = TimeHistory(np.arange(FRAME // 2) / RATE, spec.ordinate[:, :FRAME // 2],
                        response_dof=list(spec.response_dof))
    assert lag_of(short, spec) == 0
    silent = _target(scales=(0.0, 0.0))
    assert lag_of(_measured(_target(), repeats=2, lag=5), silent) == 0


def test_a_delayed_record_still_reports_what_is_left_over():
    """The leftover is counted from where the playings start, after the
    delay — not from the first sample."""
    spec = _target()
    history = _measured(spec, repeats=3, extra=FRAME // 2, lag=100)
    samples, share = rep_leftover(history, spec, lag=10)
    assert samples == (history.ordinate.shape[1] - 10) % FRAME
    assert share == pytest.approx(samples / FRAME)


def test_a_playing_cut_off_by_the_end_of_the_record_is_not_a_playing():
    spec = _target()
    history = _measured(spec, repeats=3, lag=5)
    cut = TimeHistory(history.abscissa[:-50], history.ordinate[:, :-50],
                      response_dof=list(history.response_dof))
    # three frames asked for — the record's own averaging, from before
    # the cut — and only the two the cut record still holds are given
    asked = averaging_for(history, spec)
    assert asked.frames == 3
    assert [p['frame'] for p in rep_playings(cut, spec, asked, lag=5)] == [0, 1]
    # and one started before the record began is not one either
    assert [p['frame'] for p in rep_playings(history, spec, lag=-1)] == [1, 2]


def test_a_waveform_of_two_samples_is_the_shortest_that_frames():
    two = TimeHistory(np.arange(2) / RATE, np.array([[0.0, 1.0]]),
                      response_dof=['1Z+'])
    record = TimeHistory(np.arange(4) / RATE, np.array([[0.0, 1.0, 0.0, 1.0]]),
                         response_dof=['1Z+'])
    assert averaging_for(record, two).frames == 2
    one = TimeHistory(np.arange(1) / RATE, np.array([[1.0]]), response_dof=['1Z+'])
    assert averaging_for(record, one) is None


def test_the_srs_band_is_one_cycle_per_frame_to_a_fifth_of_the_rate():
    """Low enough that the frame holds a cycle, high enough that the
    filter still resolves it: rate/frame to rate/5, in the SRS's own
    octave steps — and no band at all for a frame too short to have
    one between."""
    from hertzforge.core.replication import _band_for

    band = _band_for(FRAME, RATE)
    assert band[0] == pytest.approx(RATE / FRAME)
    assert band[-1] <= RATE / 5.0 * (1.0 + 1e-9)
    assert band[-1] > RATE / 5.0 / 2.0 ** (1.0 / 12.0)
    assert _band_for(5, RATE) is None, 'rate/5 is not above rate/5'


def test_a_target_below_the_silence_floor_is_not_scored():
    """The other side of the threshold: a control channel asked for
    more than ZERO_TARGET_DB under the loudest is asking for nothing,
    and is marked rather than scored."""
    silent = 10.0 ** (-(ZERO_TARGET_DB + 20.0) / 20.0)
    spec = _target(scales=(1.0, silent))
    rows = compare(_measured(spec, repeats=2), spec)
    assert {r['label']: r['zero_target'] for r in rows} == {'1Z+': False, '2Z+': True}


def test_one_event_is_the_playing_asked_for_on_the_targets_clock():
    from hertzforge.core.replication import event_slice

    spec = _target()
    history = _measured(spec, repeats=3, lag=4)
    history.ordinate[:, 4 + FRAME:4 + 2 * FRAME] *= 2.0     # the second, louder
    second = event_slice(history, spec, 1)
    assert second.comment == ['event 2', 'event 2']
    assert np.allclose(second.ordinate, 2.0 * spec.ordinate)
    assert second.abscissa[0] == 0.0
    assert event_slice(history, spec, 7) is None
    rows = compare(history, spec, frame=1)
    assert {r['frame'] for r in rows} == {1}


def test_an_unknown_reading_is_refused_by_name():
    spec = _target()
    with pytest.raises(ValueError, match="not a reading of a replication: .*'bogus'"):
        compare(_measured(spec, repeats=2), spec, metrics=('waveform', 'bogus'))


def test_a_channel_recorded_as_another_quantity_is_refused_not_dropped():
    """The control channel was matched by DOF and quantity, so a record
    of force at an acceleration target's DOF simply was not there. It
    gets a row per playing that says why."""
    spec = _target()
    history = _measured(spec, repeats=2)
    history.averaging = averaging_for(history, spec)
    history.ordinate_dim[0] = 'force'
    rows = compare(history, spec, metrics=('waveform',))
    first = str(spec.response_dof[0])
    refused = [row for row in rows if row['label'] == first]
    assert [row['frame'] for row in refused] == [0, 1]
    assert all(np.isnan(row['waveform']) for row in refused)
    assert refused[0]['refused'] == ('the target is of acceleration and the '
                                     'record of force, which cannot be '
                                     'compared')
    others = [row for row in rows if row['label'] != first]
    assert others and all(row.get('refused') is None for row in others)


def test_a_record_of_undeclared_units_against_a_declared_target_is_refused():
    spec = _target()
    history = _measured(spec, repeats=1)
    history.averaging = averaging_for(history, spec)
    history.ordinate_dim[:] = ['unknown'] * history.num_records
    rows = compare(history, spec, metrics=('waveform',))
    assert rows and all('units of the record are not declared'
                        in row['refused'] for row in rows)


def test_two_undeclared_sides_are_compared_and_say_so():
    from hertzforge.core.compliance import RAW_NOTE

    spec = _target()
    history = _measured(spec, repeats=1)
    history.averaging = averaging_for(history, spec)
    history.ordinate_dim[:] = ['unknown'] * history.num_records
    spec.ordinate_dim[:] = ['unknown'] * spec.num_records
    rows = compare(history, spec, metrics=('waveform',))
    assert rows and all(row['note'] == RAW_NOTE and not row.get('refused')
                        for row in rows)
    assert all(row['waveform'] == pytest.approx(0.0, abs=1e-6)
               for row in rows if not row['zero_target'])


def test_a_target_of_mixed_quantities_pairs_each_channel_by_its_own():
    """A drive point's force target beside an acceleration target: the
    first channel's quantity was read for every channel, so the
    acceleration channel was refused as 'force against acceleration'
    though its record matched it exactly. Each channel pairs by its own
    quantity, in the comparison, the alignment and the overlay."""
    t = np.arange(FRAME) / RATE
    spec = TransientSpecification(
        t, np.vstack([_pulse() * 100.0, _pulse() * 9.8]),
        response_dof=['1Z+', '2Z+'], ordinate_dim=['force', 'acceleration'],
        ordinate_unit=['N', 'm/s**2'])
    rows = np.tile(spec.ordinate, 2)
    history = TimeHistory(np.arange(rows.shape[1]) / RATE, rows,
                          response_dof=['1Z+', '2Z+'],
                          ordinate_dim=['force', 'acceleration'],
                          ordinate_unit=['N', 'm/s**2'])
    history.averaging = averaging_for(history, spec)
    scored = compare(history, spec, metrics=('waveform',))
    assert [(row['label'], row['frame']) for row in scored] == [
        ('1Z+', 0), ('1Z+', 1), ('2Z+', 0), ('2Z+', 1)]
    assert all(row.get('refused') is None
               and row['waveform'] == pytest.approx(0.0, abs=1e-9)
               for row in scored)
    from hertzforge.core.replication import event_slice

    assert event_slice(history, spec, 0).response_dof == ['1Z+', '2Z+']
