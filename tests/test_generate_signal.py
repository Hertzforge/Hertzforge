"""Generate Signal: the project's verb, and its pane in the window.

Brandon, 2026-10-24: signals at the reference DOFs, made from a
specification or a channel table, before responses through a system.
The verb records its settings and the seed it drew, so Recompute makes
the same record and the journal replays it; the pane offers only the
kinds the object makes, previews as a setting changes, and its button
runs the verb (principle 13).
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core import signals
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.sine import LOG, SineSweepSpecification, SineTone


def _target():
    return signals.flat_spectrum(['1X+', '2X+'], quantity='force',
                                 band=(20.0, 2000.0), level=1.0)


def _table():
    return ChannelTable({
        'channel': [1, 2, 3],
        'node': ['1', '2', '101'],
        'direction': ['X+', 'Y+', 'Z+'],
        'unit': ['N', 'N', 'g'],
        'role': ['reference', 'reference', 'response'],
        'channel_type': ['force', 'force', 'acceleration']})


def _sweep():
    g = 9.80665
    tone = SineTone('Up', 0.0, [20.0, 200.0], [[g], [2 * g]], [LOG], [8.0])
    return SineSweepSpecification([tone], ['1X+'], ordinate_unit='m/s**2')


def test_the_verb_records_its_settings_and_the_seed_it_drew():
    project = hertzforge.Project()
    project.add('Target', _target())
    name = project.generate_signal('Target', 'random', duration=2.0,
                                   sample_rate=8192.0)
    made = project[name]
    assert made.ordinate.shape == (2, 16384)
    assert made.ordinate_dim == ['force', 'force']
    record = project.provenance[name]
    assert record['verb'] == 'generate_signal' and record['source'] == 'Target'
    seed = record['params']['seed']
    assert isinstance(seed, int), 'a seed drawn is kept'
    again = signals.generate(project['Target'],
                             signals.SignalSettings.from_dict(record['params']))
    assert np.array_equal(again.ordinate, made.ordinate)
    assert name in project.object_group_of('Target')


def test_recompute_makes_the_same_record_and_a_changed_target_is_stale():
    project = hertzforge.Project()
    project.add('Target', _target())
    name = project.generate_signal('Target', 'burst', bursts=3,
                                   frame_length=1024, sample_rate=8192.0,
                                   seed=4)
    first = project[name].ordinate.copy()
    assert name not in project.stale()
    project['Target'].ordinate *= 2.0
    assert name in project.stale(), 'the levels it was drawn to moved'
    project.refresh(name)
    assert np.allclose(project[name].ordinate, np.sqrt(2.0) * first)
    assert name not in project.stale()


def test_a_channel_table_makes_an_impulse_at_its_references():
    project = hertzforge.Project()
    project.add('Channels', _table())
    name = project.generate_signal('Channels', 'impulse', duration=1.0,
                                   sample_rate=10000.0, at=0.1, peak=5.0,
                                   width=0.004)
    made = project[name]
    assert made.response_dof == ['1X+', '2Y+'], 'the reference channels only'
    assert made.ordinate_dim == ['force', 'force']
    # the sampled half-sine of N = 40 intervals sums to cot(π/2N)
    assert made.ordinate[0].sum() == pytest.approx(
        5.0 / np.tan(np.pi / 80), rel=1e-9)
    pulse = project.generate_signal('Channels', 'pulse', duration=0.1,
                                    sample_rate=20000.0, peak=100.0,
                                    pulse_duration=0.005)
    assert project[pulse].ordinate.max() == pytest.approx(100.0, rel=1e-3)
    typed = project.generate_signal('Channels', 'random', duration=4.0,
                                    sample_rate=8192.0, band=(20.0, 500.0),
                                    level=2.0, seed=1)
    assert project[typed].ordinate[0].std() == pytest.approx(
        np.sqrt(2.0 * 480.0), rel=0.03)


def test_a_sine_specification_makes_its_tone():
    project = hertzforge.Project()
    project.add('Sweep', _sweep())
    name = project.generate_signal('Sweep', 'sine', sample_rate=4096.0)
    assert project[name].response_dof == ['1X+']
    assert project.provenance[name]['params']['kind'] == 'sine'


def test_what_the_verb_refuses():
    project = hertzforge.Project()
    project.add('Target', _target())
    project.add('Channels', ChannelTable({'channel': [1], 'node': ['1'],
                                          'direction': ['X+'], 'unit': ['N']}))
    with pytest.raises(ValueError, match='is not made from this object'):
        project.generate_signal('Target', 'impulse')
    with pytest.raises(ValueError, match='Nyquist'):
        project.generate_signal('Target', 'random', sample_rate=3000.0)
    with pytest.raises(ValueError, match="no channel's role is reference"):
        project.generate_signal('Channels', 'impulse')
    with pytest.raises(TypeError):
        project.generate_signal('Target', 'random', durations=3.0)
    assert 'generate_signal' in dict(project.verbs('Target'))
    assert 'generate_signal' not in dict(project.verbs(
        hertzforge.TimeHistory(np.arange(8.0), np.zeros((1, 8)),
                                   response_dof='1X+')))


def test_settings_written_by_another_version_still_read():
    values = signals.SignalSettings(kind='burst', bursts=4).to_dict()
    values['a_setting_from_later'] = 3
    del values['taper']
    read = signals.SignalSettings.from_dict(values)
    assert read.kind == 'burst' and read.bursts == 4
    assert read.taper == signals.BURST_TAPER


def _lbf_table(units=('lbf', 'lbf')):
    return ChannelTable({
        'channel': [1, 2, 3],
        'node': ['1', '2', '101'],
        'direction': ['X+', 'Y+', 'Z+'],
        'unit': [*units, 'g'],
        'role': ['reference', 'reference', 'response'],
        'channel_type': ['force', 'force', 'acceleration']})


LBF = 4.4482216152605      # newtons in a pound-force


def test_a_typed_level_and_peak_are_in_each_channels_own_unit():
    # 1.0 typed for an lbf table gave 1 N**2/Hz: 63.1 N RMS over
    # 20-4000 Hz where 63.1 lbf was meant (the BARC table, 2026-10-29)
    project = hertzforge.Project()
    project.add('Channels', _lbf_table(('lbf', 'N')))
    typed = project.generate_signal('Channels', 'random', duration=4.0,
                                    sample_rate=8192.0, band=(20.0, 500.0),
                                    level=2.0, seed=1)
    made = project[typed]
    rms = np.sqrt(2.0 * 480.0)
    assert made.ordinate[0].std() == pytest.approx(rms * LBF, rel=0.03)
    assert made.ordinate[1].std() == pytest.approx(rms, rel=0.03), \
        'the newton channel in newtons'
    impulse = project.generate_signal('Channels', 'impulse', duration=1.0,
                                      sample_rate=10000.0, at=0.1, peak=5.0,
                                      width=0.004)
    peaks = project[impulse].ordinate.max(axis=1)
    assert peaks == pytest.approx([5.0 * LBF, 5.0], rel=1e-9)
    pulse = project.generate_signal('Channels', 'pulse', duration=0.1,
                                    sample_rate=20000.0, peak=100.0,
                                    pulse_duration=0.005)
    assert project[pulse].ordinate.max(axis=1) == pytest.approx(
        [100.0 * LBF, 100.0], rel=1e-3)
    target = signals.target_of(project['Channels'], signals.SignalSettings(
        band=(20.0, 500.0), level=2.0))
    assert target.ordinate[:, 0] == pytest.approx([2.0 * LBF ** 2, 2.0])


def test_the_reference_unit_is_the_one_they_share():
    assert signals.reference_unit(_lbf_table()) == 'lbf'
    assert signals.reference_unit(_lbf_table(('lbf', 'N'))) == ''
    assert signals.reference_unit(_lbf_table(('', ''))) == 'N', \
        "SI's where none is written, as the level is then read"
    assert signals.reference_unit(_lbf_table(), ['1X+']) == 'lbf'
