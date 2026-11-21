"""A control loop's transformed responses as a history of their own.

A Rattlesnake run whose environments each controlled the rows of their
own response transformation numbers both sets of rows from 1. Kept in
the recording, Random's row 1 and Sine's row 1 were one channel to every
reader: a PSD averaged the two into a level neither environment saw, at
0.65 of a channel's level where the rows were 0.25 and 1.0 of it. Each
loop's rows are now a history of their own, made from the recording,
framed as the recording is, and judged against their own loop's
specification.
"""

import numpy as np
import pytest
from test_rattlesnake_transformation import CONTROL, DRIVES, MEASURED, RATE, SAMPLES

import hertzforge
from hertzforge.core.compliance import matched_records
from hertzforge.service import projects
from hertzforge.service.ops_settings import OPERATIONS
from hertzforge.service.protocol import ProtocolError

#: Random's rows: the mean of the four control channels, and a rocking pair
RANDOM = np.array([[0.25, 0.25, 0.25, 0.25],
                   [0.5, 0.5, -0.5, -0.5]])

#: Sine's rows: the first and the last control channel alone
SINE = np.array([[1.0, 0.0, 0.0, 0.0],
                 [0.0, 0.0, 0.0, 1.0]])

LINES = 5

OPS = {op.id: op for op in OPERATIONS}


def write_two(path, sine=SINE):
    """A run with two random environments over the same four control
    channels, each with its own response transformation (or none for
    Sine when `sine` is None) and a specification over its rows."""
    import netCDF4

    channels = MEASURED + DRIVES
    raw = np.random.default_rng(11).standard_normal((channels, SAMPLES * 8))
    loops = [('Random', RANDOM, 1.0), ('Sine', sine, 3.0)]
    with netCDF4.Dataset(path, 'w', format='NETCDF4') as ds:
        ds.sample_rate = RATE
        ds.createDimension('response_channels', channels)
        ds.createDimension('time_samples', raw.shape[1])
        ds.createVariable('time_data', 'f8',
                          ('response_channels', 'time_samples'))[...] = raw
        group = ds.createGroup('channels')
        for name, values in (
                ('node_number', [str(101 + i) for i in range(channels)]),
                ('node_direction', ['Z+'] * channels),
                ('unit', ['m/s^2'] * MEASURED + ['N'] * DRIVES),
                ('feedback_device', [''] * MEASURED + ['Input'] * DRIVES)):
            group.createVariable(name, str, ('response_channels',))[:] = \
                np.array(values, dtype=object)
        ds.createDimension('num_environments', len(loops))
        names = ds.createVariable('environment_names', str, ('num_environments',))
        kinds = ds.createVariable('environment_types', int, ('num_environments',))
        for k, (name, _matrix, _level) in enumerate(loops):
            names[k], kinds[k] = name, 0
        for name, matrix, level in loops:
            env = ds.createGroup(name)
            env.samples_per_frame = 128
            env.frames_in_cpsd = 4
            env.createDimension('control_channels', len(CONTROL))
            env.createVariable('control_channel_indices', 'i4',
                               ('control_channels',))[...] = CONTROL
            rows = len(CONTROL) if matrix is None else matrix.shape[0]
            if matrix is not None:
                env.createDimension('response_transformation_rows', rows)
                env.createDimension('response_transformation_cols', len(CONTROL))
                env.createVariable('response_transformation_matrix', 'f8',
                                   ('response_transformation_rows',
                                    'response_transformation_cols'))[...] = matrix
            env.createDimension('fft_lines', LINES)
            env.createDimension('two', 2)
            env.createDimension('specification_channels', rows)
            env.createVariable('specification_frequency_lines', 'f8',
                               ('fft_lines',))[...] = np.arange(LINES) * 2.0
            cpsd = np.zeros((LINES, rows, rows))
            for k in range(rows):
                cpsd[:, k, k] = level
            for part, values in (('specification_cpsd_matrix_real', cpsd),
                                 ('specification_cpsd_matrix_imag', 0 * cpsd)):
                env.createVariable(part, 'f8', ('fft_lines', 'specification_channels',
                                                'specification_channels'))[...] = values
            for part, factor in (('specification_warning_matrix', (0.5, 2.0)),
                                 ('specification_abort_matrix', (0.25, 4.0))):
                var = env.createVariable(part, 'f8', ('two', 'fft_lines',
                                                      'specification_channels'))
                var[0] = level * factor[0]
                var[1] = level * factor[1]
    return path, raw


def _imported(tmp_path, **options):
    path, raw = write_two(str(tmp_path / 'two.nc4'), **options)
    project = hertzforge.Project()
    names = project.import_file(path)
    return project, names, raw


def test_each_loops_rows_are_a_history_of_their_own(tmp_path):
    project, names, raw = _imported(tmp_path)
    assert names[-2:] == ['Time History Transformed (Random)',
                          'Time History Transformed (Sine)']
    history = project['Time History']
    assert history.response_dof == [f'{101 + i}Z+' for i in range(MEASURED + DRIVES)]
    random, sine = (project['Time History Transformed (Random)'],
                    project['Time History Transformed (Sine)'])
    assert random.response_dof == sine.response_dof == ['1', '2']
    assert np.allclose(random.ordinate, RANDOM @ raw[CONTROL])
    assert np.allclose(sine.ordinate, SINE @ raw[CONTROL])
    assert sine.comment[0] == ('row 1 of the Sine response transformation over 4 '
                               'control channels')
    # one flat group: the file's objects and the rows made from them
    assert sorted(project.object_group_of('Time History')) == sorted(names)
    assert project.channel_table_for('Time History Transformed (Sine)')[0] == 'Channel Table'
    assert project.transformed_rows('Time History Transformed (Sine)') == [
        ('Channel Table', 'Random', ['1', '2']), ('Channel Table', 'Sine', ['1', '2'])]


def test_each_loops_psds_are_their_own_rows_and_meet_their_own_specification(tmp_path):
    project, _names, _raw = _imported(tmp_path)
    random = project[project.compute_psds('Time History Transformed (Random)')]
    sine = project[project.compute_psds('Time History Transformed (Sine)')]
    channel = project[project.compute_psds('Time History')]
    assert random.response_dof == sine.response_dof == ['1', '2']
    level = lambda psd, dof: float(np.mean(psd.ordinate[psd.response_dof.index(dof)].real))
    # Random's row 1 is a mean of four unit channels, a quarter of a channel's
    # level; Sine's row 1 is channel 101 itself. Together they read 0.65.
    assert level(random, '1') / level(channel, '101Z+') == pytest.approx(0.25, rel=0.15)
    assert level(sine, '1') / level(channel, '101Z+') == pytest.approx(1.0, abs=1e-9)
    specs = {project[name].ordinate.real.max(): name for name in project.names
             if type(project[name]).__name__ == 'Specification'}
    sine_spec = project[specs[3.0]]
    assert [label for label, _j, _i in matched_records(sine_spec, sine)] == ['1', '2']
    # two random loops, both over rows 1 and 2: which history a specification
    # is read against cannot be told, and is not guessed
    with pytest.raises(ValueError, match="2 histories made from 'Time History' hold 1, 2"):
        project.recording_for(specs[3.0], 'Time History')


def test_one_transformed_loop_gives_one_history_named_plainly(tmp_path):
    project, names, _raw = _imported(tmp_path, sine=None)
    assert names[-1] == 'Time History Transformed'
    assert 'Time History Transformed (Random)' not in project


def test_a_transformed_history_takes_its_recordings_averaging(tmp_path):
    project, _names, _raw = _imported(tmp_path)
    rows = 'Time History Transformed (Sine)'

    def run(op_id, **given):
        op = OPS[op_id]
        values = op.resolve(given, objects={name: projects.kind_of(obj)
                                            for name, obj in project.items()})
        return op.run(project, values)

    # set on the recording, given to both histories of rows, in one journal line each
    run('project.set_averaging', source='Time History', frame_length=256, overlap=0.5,
        window='hann', frames=10, start=0.25)
    averaging = project['Time History'].averaging
    assert averaging.frames == 10
    assert project[rows].averaging == averaging
    assert project['Time History Transformed (Random)'].averaging == averaging
    assert project.journal[-1] == "project.follow_averaging('Time History')"
    # set on the rows alone: refused, and nothing moves
    with pytest.raises(ProtocolError, match='takes its averaging from Time History'):
        run('project.set_averaging', source=rows, frame_length=128, overlap=0.0,
            window='boxcar', frames=2, start=0.0)
    for op_id, given in (('project.detect_averaging', {}),
                         ('project.drag_averaging', {'frame_length': 128, 'overlap': 0.5,
                                                     'window': 'hann', 'start': 0.0,
                                                     'stop': 1.0})):
        with pytest.raises(ProtocolError, match='set it on Time History'):
            run(op_id, source=rows, **given)
    assert project[rows].averaging == averaging
    # a script's assignment marks the rows stale, saying what moved; a refresh
    # gives them the recording's again
    from dataclasses import replace

    project['Time History'].averaging = replace(averaging, frames=4)
    stale = project.stale()
    assert rows in stale and '4 frames of 256' in stale[rows]
    project.refresh(rows)
    assert project[rows].averaging.frames == 4 and rows not in project.stale()


def test_the_rows_psds_are_framed_by_the_recordings_detected_averaging(tmp_path):
    project, _names, _raw = _imported(tmp_path)
    for name in project.names:
        if type(project[name]).__name__ == 'TimeHistory':
            project[name].averaging = None
    project.compute_psds('Time History Transformed (Random)')
    found = project['Time History'].averaging
    assert found is not None, 'detected on the recording, not on the rows'
    assert project['Time History Transformed (Random)'].averaging == found
    assert project['Time History Transformed (Sine)'].averaging == found


def test_reports_see_one_run(tmp_path):
    from hertzforge.project import _quiet_and_driven
    from hertzforge.report.runs import histories

    project, _names, _raw = _imported(tmp_path)
    assert histories(project) == ['Time History']
    assert _quiet_and_driven(project) == (None, 'Time History')


def test_a_rows_values_follow_a_recomputed_recording(tmp_path):
    project, _names, raw = _imported(tmp_path)
    rows = 'Time History Transformed (Random)'
    project['Time History'].ordinate[0] *= 2.0
    assert "its source's values have changed" in project.stale()[rows]
    project.refresh(rows)
    scaled = raw[CONTROL].copy()
    scaled[0] *= 2.0
    assert np.allclose(project[rows].ordinate, RANDOM @ scaled)


def test_a_subset_without_a_control_channel_makes_no_rows(tmp_path):
    path, _raw = write_two(str(tmp_path / 'two.nc4'))
    project = hertzforge.Project()
    names = project.import_file(path, channels=['101Z+', '102Z+'])
    assert not any('Transformed' in name for name in names)
    with pytest.raises(ValueError, match='no record of control channels 103Z\\+, 104Z\\+'):
        project.transform_responses('Time History', 'Random')


def test_a_sweep_over_a_virtual_point_is_read_from_its_rows(tmp_path):
    from test_rattlesnake_sine import _write_run

    path = _write_run(tmp_path / 'mixed.nc4', random=True, sine_transformation=[[0.5, 0.5]])
    project = hertzforge.Project()
    project.import_file(path)
    # the recording holds no row 1; the refusal names the history that does
    with pytest.raises(ValueError, match="which are in 'Time History Transformed': "
                                         'extract from that'):
        project.extract_sine('Time History')
    assert project.recording_for('Sine Sweep Specification', 'Time History') == \
        'Time History Transformed'


def test_a_random_run_over_its_rows_is_judged_on_their_psds(tmp_path):
    from test_rattlesnake_transformation import write_run

    path, _raw = write_run(str(tmp_path / 'virtual.nc4'))
    project = hertzforge.random_vibration_run(path)
    assert 'Time History Transformed PSDs' in project
    assert project.provenance['Time History Transformed PSDs']['source'] == \
        'Time History Transformed'
    assert project['Time History Transformed'].averaging == project['Time History'].averaging
