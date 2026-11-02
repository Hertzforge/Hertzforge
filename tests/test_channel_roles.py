"""Every channel of time data is a reference, a response or a monitor,
and FRFs run from the references to the responses.

Brandon (2026-09-25): *time data should be assigned reference,
response, or monitor; FRFs should be computed between reference and
response.* Before this the references were a guess from the quantities
and every other channel was a response — a monitored accelerometer got
an FRF row, and a monitored load cell became a drive of every FRF. The
roles ride the time history (`roles`), in the channel table's own three
words: seeded from the table an importer reads beside the data, changed
in the grid's Role column, saved with the object, journaled, read by
FRFs and multiple coherence alike, and fingerprinted so a change badges
what was derived.

A role is a channel's, not a point's: a drive point's load cell and the
accelerometer beside it are two channels, and either may be the
reference — an accelerometer as a reference gives acceleration over
acceleration, a transmissibility.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path
from test_compute_frfs import shaken

import hertzforge
from hertzforge.core.averaging import Averaging
from hertzforge.core.data import TimeHistory
from hertzforge.project import Project


def _drive_point(samples=8192, seed=1):
    """A load cell and an accelerometer at one point, and two more
    accelerometers: the impedance-head layout."""
    rng = np.random.default_rng(seed)
    t = np.arange(samples) / 2048.0
    force = rng.normal(0.0, 1.0, samples)
    rows = [force, 2.0 * force, 3.0 * force, 0.5 * force]
    history = TimeHistory(t, np.array(rows),
                          response_dof=['1Z+', '1Z+', '2Z+', '3Z+'],
                          ordinate_dim=['force', 'acceleration',
                                        'acceleration', 'acceleration'])
    history.averaging = Averaging(frame_length=512, overlap=0.5,
                                  window='hann', frames=20)
    return history


def test_the_default_forces_reference_motions_respond_the_rest_monitor():
    """With no channel table (Brandon, 2026-09-25): a force is a
    reference, an acceleration, velocity or displacement a response, and
    everything else a monitor. A moment and a rotation follow the force
    and the motion they are the rotational kind of."""
    assert [TimeHistory.default_role(q) for q in (
        'force', 'moment', 'acceleration', 'velocity', 'length',
        'angular_acceleration', 'angular_velocity', 'angle', 'voltage',
        'pressure', 'strain', 'temperature', 'unknown')] == [
        'reference', 'reference', 'response', 'response', 'response',
        'response', 'response', 'response', 'monitor', 'monitor',
        'monitor', 'monitor', 'monitor']


def test_with_nothing_said_the_force_drives_the_motions():
    history = _drive_point()
    assert history.roles is None
    assert history.channel_roles() == {
        ('1Z+', 'force'): 'reference', ('1Z+', 'acceleration'): 'response',
        ('2Z+', 'acceleration'): 'response',
        ('3Z+', 'acceleration'): 'response'}
    frfs = history.compute_frfs()
    assert set(frfs.reference_dof) == {'1Z+'}
    assert set(zip(frfs.response_dof, frfs.ordinate_dim)) == {
        ('1Z+', 'acceleration/force'), ('2Z+', 'acceleration/force'),
        ('3Z+', 'acceleration/force')}


def test_a_monitor_is_in_no_frf_and_no_coherence():
    history = _drive_point()
    history.roles = {('3Z+', 'acceleration'): 'monitor'}
    assert history.channel_roles()[('2Z+', 'acceleration')] == 'response', (
        'a channel the roles do not name keeps the guess')
    frfs = history.compute_frfs()
    assert '3Z+' not in frfs.response_dof and '3Z+' not in frfs.reference_dof
    assert frfs.num_records == 2
    coherence = history.compute_multiple_coherence()
    assert '3Z+' not in coherence.response_dof


def test_a_monitored_load_cell_is_not_a_drive():
    """The costly half of the old behavior: a force channel was a
    reference whatever it was for, so a load cell recorded to watch
    became a drive of every FRF."""
    history, _gains = shaken(drives=2)
    history.roles = {('210Z+', 'force'): 'monitor'}
    frfs = history.compute_frfs(method='H1')
    assert set(frfs.reference_dof) == {'200Z+'}
    assert '210Z+' not in frfs.response_dof, 'and not explained either'


def test_an_accelerometer_as_the_reference_gives_transmissibilities():
    history = _drive_point()
    history.roles = {('1Z+', 'force'): 'response',
                     ('1Z+', 'acceleration'): 'reference'}
    frfs = history.compute_frfs(method='H1')
    assert list(frfs.reference_dof) == ['1Z+'] * 3
    assert set(frfs.ordinate_dim) == {'acceleration/acceleration',
                                      'force/acceleration'}
    at_2 = frfs.ordinate[list(frfs.response_dof).index('2Z+')]
    assert np.allclose(np.abs(at_2[1:-1]), 1.5, rtol=1e-6), (
        '2Z+ moves 3/2 as much as 1Z+ does')


def test_a_load_cell_and_the_accelerometer_beside_it_can_both_be_references():
    history = _drive_point()
    history.roles = {('1Z+', 'acceleration'): 'reference'}
    frfs = history.compute_frfs(method='Hv')
    assert frfs.num_records == 4, 'two responses against two references'
    assert set(frfs.response_dof) == {'2Z+', '3Z+'}
    assert sorted(set(frfs.ordinate_dim)) == ['acceleration/acceleration',
                                              'acceleration/force']


def test_a_call_may_still_name_the_references():
    """A script names one drive of two: the other is explained, as it
    always was — and a monitor stays out whatever the call says."""
    history, _gains = shaken(drives=2)
    history.roles = {('102Z+', 'acceleration'): 'monitor'}
    frfs = history.compute_frfs(references=['200Z+'])
    assert set(frfs.reference_dof) == {'200Z+'}
    assert '210Z+' in frfs.response_dof
    assert '102Z+' not in frfs.response_dof
    both = history.compute_frfs(references=[('200Z+', 'force'), '210Z+'])
    assert set(both.reference_dof) == {'200Z+', '210Z+'}


def test_no_reference_or_no_response_is_refused_by_name():
    history, _gains = shaken(drives=2)
    history.roles = {('200Z+', 'force'): 'monitor',
                     ('210Z+', 'force'): 'response'}
    with pytest.raises(ValueError, match='no channel has the role reference'):
        history.compute_frfs()
    history.roles = {identity: 'monitor'
                     for identity in history.channel_identities()}
    history.roles[('200Z+', 'force')] = 'reference'
    with pytest.raises(ValueError, match='every channel is a reference or a '
                                          'monitor'):
        history.compute_multiple_coherence()
    with pytest.raises(ValueError, match=r"none of \['999Z\+'\]"):
        history.compute_frfs(references=['999Z+'])


def test_a_rattlesnake_import_takes_the_roles_its_channel_table_states():
    """The file says which channels were driven (a feedback device), and
    the table read beside the data carries it: the history is given the
    same roles, and they agree with the old guess on every fixture."""
    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    table, history = loaded['channel_table'], loaded['time_data']
    assert history.roles is not None
    references = {identity for identity, role in history.roles.items()
                  if role == 'reference'}
    assert {dof for dof, _q in references} == {
        dof for dof, role in zip(table.dof_strings(), table.roles())
        if role == 'reference'}
    assert all(quantity == 'force' for _dof, quantity in references), (
        'the drive rows are the load cells, not the accelerometers beside them')
    assert set(history.roles.values()) <= {'reference', 'response'}


def test_the_roles_ride_the_native_file(tmp_path):
    history = _drive_point()
    history.roles = {('1Z+', 'acceleration'): 'reference',
                     ('3Z+', 'acceleration'): 'monitor'}
    hertzforge.save(history, tmp_path / 'run.vdyn')
    back = hertzforge.load(tmp_path / 'run.vdyn')
    assert back.roles == history.roles
    history.roles = None
    hertzforge.save(history, tmp_path / 'guess.vdyn')
    assert hertzforge.load(tmp_path / 'guess.vdyn').roles is None


def test_changing_a_role_badges_the_frfs_and_the_coherence():
    project = Project()
    history, _gains = shaken(drives=2)
    project.add('Run', history)
    frfs = project.compute_frfs('Run')
    coherence = project.compute_multiple_coherence('Run')
    psds = project.compute_psds('Run')
    from dataclasses import asdict

    assert project.provenance[frfs]['state'] == (
        'averaging', asdict(history.averaging)), (
        'no roles stated, the plain averaging fingerprint: an older '
        'project opens with nothing stale')
    history.roles = dict(history.channel_roles())
    assert project.stale() == {}, (
        'stating what the guess already said changes nothing')
    history.roles[('210Z+', 'force')] = 'monitor'
    stale = project.stale()
    assert set(stale) == {frfs, coherence}, 'the PSDs read no roles'
    assert 'references 200Z+ force' in stale[frfs], stale[frfs]
    project.refresh(frfs)
    assert set(project[frfs].reference_dof) == {'200Z+'}
    assert frfs not in project.stale() and psds not in project.stale()


def _table(roles, types=('force', 'acceleration', 'acceleration',
                          'acceleration'), control=None):
    """A channel table describing `_drive_point`'s four channels."""
    from hertzforge.core.channel_table import ChannelTable

    units = {'force': 'N', 'acceleration': 'm/s**2'}
    columns = {'channel': [1, 2, 3, 4], 'node': [1, 1, 2, 3],
               'direction': ['Z+'] * 4,
               'unit': [units[t] for t in types],
               'channel_type': list(types), 'role': list(roles)}
    if control is not None:
        columns['control'] = [str(bool(c)) for c in control]
    return ChannelTable(columns)


def test_a_linked_table_mirrors_the_time_data_both_ways():
    """The time data's roles are the truth; the linked table shows them,
    and a change made on either lands on both."""
    project = Project()
    project.add('Run', _drive_point())
    project.add('Channels', _table(['reference', 'response', 'response',
                                    'response']))
    project.link('Run', 'Channels')
    changed = project.set_channel_role('Run', '3Z+', 'acceleration',
                                       'monitor')
    assert changed == ['Run', 'Channels']
    assert project['Channels'].roles()[3] == 'monitor'
    changed = project.set_channel_role('Channels', '2Z+', 'acceleration',
                                       'monitor')
    assert changed == ['Run', 'Channels']
    assert project['Run'].channel_roles()[('2Z+', 'acceleration')] == 'monitor'
    frfs = project['Run'].compute_frfs()
    assert set(frfs.response_dof) == {'1Z+'}, 'only the drive-point accel'
    assert "project.set_channel_role('Channels', '2Z+', 'acceleration', " \
        "'monitor')" in project.journal[-1]


def test_an_unlinked_table_keeps_its_own_roles():
    project = Project()
    project.add('Run', _drive_point())
    project.add('Channels', _table(['reference', 'response', 'response',
                                    'response']))
    assert project.set_channel_role('Channels', '3Z+', 'acceleration',
                                    'monitor') == ['Channels']
    assert project['Run'].channel_roles()[('3Z+', 'acceleration')] == \
        'response', 'nothing linked, nothing mirrored'


def test_a_table_linked_after_the_fact_brings_its_declared_roles():
    """Linking is the table's import: what it declares, the time data
    takes — even over roles the data already had, since a table linked
    later is usually the one with the monitors marked — and a channel
    it leaves blank keeps the data's role, which the table then shows.
    What moved is reported, because each role moves an FRF."""
    project = Project()
    history = _drive_point()
    history.roles = {('3Z+', 'acceleration'): 'reference'}
    project.add('Run', history)
    project.add('Channels', _table(['reference', '', 'monitor', '']))
    project.link('Run', 'Channels')
    roles = history.channel_roles()
    assert roles[('2Z+', 'acceleration')] == 'monitor', 'declared: taken'
    assert roles[('3Z+', 'acceleration')] == 'reference', (
        'blank in the table: the data keeps its own')
    assert project['Channels'].roles() == ['reference', 'response',
                                           'monitor', 'reference'], (
        'and the table shows the truth from then on')
    assert project.last_role_changes == [
        ('Run', ('2Z+', 'acceleration'), 'response', 'monitor')]


def test_a_table_describing_other_channels_is_left_alone():
    project = Project()
    project.add('Run', _drive_point())
    from hertzforge.core.channel_table import ChannelTable

    other = ChannelTable({'channel': [1], 'node': [99], 'direction': ['X+'],
                          'unit': ['m/s**2'], 'role': ['monitor']})
    project.add('Elsewhere', other)
    project.link('Run', 'Elsewhere')
    assert project.last_role_changes == []
    assert project['Run'].roles is None, 'not compatible, not a pointer'
    assert project['Elsewhere'].roles() == ['monitor']


def test_a_control_channel_takes_any_role():
    """Control and role are independent (Brandon, 2026-09-26): a
    controlled force is an FRF's reference, and a controlled
    accelerometer can be set aside as a monitor; the table follows the
    data either way, and Control stays checked."""
    project = Project()
    project.add('Run', _drive_point())
    project.add('Channels', _table(['reference', 'response', 'response',
                                    'response'],
                                   control=[1, 0, 1, 0]))
    project.link('Run', 'Channels')
    table = project['Channels']
    assert table.roles()[0] == 'reference' and bool(table.controls()[0]), (
        'a controlled force, the reference')
    project.set_channel_role('Run', '2Z+', 'acceleration', 'monitor')
    assert table.roles()[2] == 'monitor' and bool(table.controls()[2])
    frfs = project['Run'].compute_frfs()
    assert set(frfs.reference_dof) == {'1Z+'}
    assert '2Z+' not in frfs.response_dof
