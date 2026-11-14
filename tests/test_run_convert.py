"""A run folder becomes an ordinary project file: the reader takes a
folder to the layout, finished or not, and refuses one with no source
label or a newer schema; the converter writes a `.hforge` that
`Project.open` opens, with the channel table, a time history per stream,
the specification, the results and the run record, and that survives
save and reopen; the same folder gives the same bytes; a stream is
copied in bounded memory. The folders
are synthetic (`run_folders`), written to the layout."""

from __future__ import annotations

import filecmp
import json
import os
import time
import tracemalloc

import h5py
import numpy as np
import pytest
import run_folders as rf

from hertzforge import Project
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.data import Coherence, Frf, Psd, Specification, TimeHistory
from hertzforge.core.run_record import RunRecord
from hertzforge.core.runs import FROM_CONTROLLER
from hertzforge.io import native, run_convert, run_folder

G = rf.G


@pytest.fixture
def random_project(tmp_path):
    folder = rf.random_run(str(tmp_path / 'qual_random'))
    done = run_convert.convert(folder)
    return folder, done, Project.open(done.path)


# ---- the converted project ---------------------------------------------

def test_the_converted_file_is_an_ordinary_project(random_project):
    folder, done, project = random_project
    assert done.path == os.path.join(folder, 'qual_random.hforge')
    assert done.lacked == []
    assert list(project) == done.objects
    assert project.name == 'Golden random'
    assert project.project_type == 'Random Vibration'
    (group,) = project.object_groups
    assert group['name'] == 'qual_random' and group['members'] == list(project)
    with h5py.File(done.path) as f:
        assert f.attrs[native.STAMP] == native.RUN_RECORD_SCHEMA


def test_every_enabled_input_and_drive_is_there_in_si(random_project):
    folder, _done, project = random_project
    inputs, drives = project['inputs'], project['drives']
    assert isinstance(inputs, TimeHistory) and isinstance(drives, TimeHistory)
    assert inputs.response_dof == ['101Z+', '2'] and drives.response_dof == ['3']
    assert inputs.ordinate.shape == (2, 4096) and drives.ordinate.shape == (1, 4096)
    assert inputs.ordinate_unit == ['g', 'g'] and drives.ordinate_unit == ['V']
    assert inputs.ordinate_dim == ['acceleration'] * 2 and drives.ordinate_dim == ['voltage']
    assert inputs.source == drives.source == 'SIM'
    raw = np.fromfile(os.path.join(folder, 'streams', 'inputs', 'telemetry.f32'),
                      dtype='<f4').reshape(-1, 2)
    expected = (raw.T.astype(np.float64) * G).astype(np.float32)
    assert np.array_equal(inputs.ordinate, expected)
    assert inputs.abscissa[0] == 0.0
    assert inputs.abscissa[1] == pytest.approx(1 / rf.FS)
    with h5py.File(os.path.join(folder, 'qual_random.hforge')) as f:
        kinds = {g.attrs['name']: g for g in f['objects'].values()}
        assert kinds['inputs']['ordinate'].dtype == np.float32


def test_drives_are_references_and_inputs_take_the_table_roles(random_project):
    _folder, _done, project = random_project
    assert project['drives'].roles == {('3', 'voltage'): 'reference'}
    assert project['inputs'].roles == {('101Z+', 'acceleration'): 'response',
                                       ('2', 'acceleration'): 'response'}


def test_the_run_identity_names_the_controller(random_project):
    _folder, _done, project = random_project
    identity = project['inputs'].run_identity
    assert identity.run == 'r-0007'
    assert identity.start == '2026-11-09T09:00:00.250000-07:00'
    assert identity.source('run') == identity.source('start') == FROM_CONTROLLER


def test_the_averaging_is_the_one_the_setup_names(random_project):
    _folder, _done, project = random_project
    averaging = project['inputs'].averaging
    assert (averaging.frame_length, averaging.frames, averaging.window) == (256, 4, 'hann')


def _expected_columns():
    """What each column of the converted table holds, row by row, for
    `run_folders.random_rows`, written out by hand."""
    return {
        'channel': [1, 2, 3, 4], 'node': ['101', '2', '3', '4'],
        'direction': ['Z+', '', '', ''], 'role': ['response', 'response', 'reference',
                                                  'response'],
        'control': ['True', 'False', 'False', 'False'],
        'channel_type': ['acceleration', 'acceleration', 'voltage', 'acceleration'],
        'unit': ['g', 'g', 'V', 'g'], 'sensitivity': ['100.0', '100.0', '1000.0', '100.0'],
        'serial_number': ['00417', '', '', ''], 'model': ['A100', '', '', ''],
        'expiration': ['2027-01-31', '', '', ''],
        'comment': ['Control', 'Monitor', 'Drive', 'Spare'],
        'enabled': ['True', 'True', 'True', 'False'],
        'drive': ['False', 'False', 'True', 'False'],
        'physical': ['Dev1/ai0', 'Dev1/ai1', 'Dev1/ao0', ''],
        'iepe': ['True', 'False', 'False', 'False'], 'iepe_bias': ['4.0', '', '', ''],
        'polarity': ['positive', 'negative', 'positive', 'positive'],
        'range_max': ['50.0', '50.0', '10.0', '50.0'], 'control_group': ['0', '', '', ''],
        'abort_accel_g': ['40.0', '', '', ''], 'abort_velocity_ips': ['', '', '', ''],
        'abort_displacement_mil': ['250.0', '', '', '']}


def test_the_channel_table_is_the_table_set_column_by_column(random_project):
    _folder, _done, project = random_project
    table = project['Channel Table']
    assert isinstance(table, ChannelTable) and table.has_controller_settings
    for name, values in _expected_columns().items():
        assert list(table[name]) == [str(v) if name != 'channel' else v
                                     for v in values], name


def test_the_table_mapping_runs_both_ways():
    rows = rf.random_rows()
    back = run_convert.rows_from_channel_table(run_convert.channel_table_from_rows(rows))
    for given, read in zip(rows, back, strict=True):
        assert {k: v for k, v in read.items() if k != 'extra'} == {
            k: v for k, v in given.items() if k != 'extra'}
    assert back[0]['extra'] == {'node': '101', 'direction': 'Z+', 'serial_number': '00417',
                                'model': 'A100', 'expiration': '2027-01-31'}
    # a channel with no node carries its channel number as one
    assert back[1]['extra'] == {'node': '2'}


def test_a_monitor_is_never_made_a_reference():
    table = run_convert.channel_table_from_rows([rf.row(1, 'monitor',
                                                        extra={'role': 'reference'})])
    assert list(table['role']) == ['response']
    table = run_convert.channel_table_from_rows([rf.row(1, 'reference')])
    assert run_convert.rows_from_channel_table(table)[0]['role'] == 'reference'


def test_the_specification_is_the_resolved_profile(random_project):
    _folder, _done, project = random_project
    spec = project['Specification']
    assert isinstance(spec, Specification) and spec.source is None
    profile = rf.random_setup()['resolved']['profile']['breakpoints']
    frequencies = [p['freq_hz'] for p in profile]
    levels = np.array([p['psd_g2_hz'] for p in profile]) * G ** 2
    assert spec.response_dof == ['101Z+']
    assert list(spec.abscissa) == frequencies
    assert np.allclose(spec.ordinate.real, levels[np.newaxis])
    assert spec.ordinate_unit == ['g'] and spec.ordinate_dim == ['acceleration**2/frequency']
    assert spec.interpolation == 'log_log'
    for name, db in (('warning_lower', -3), ('warning_upper', 3),
                     ('abort_lower', -6), ('abort_upper', 6)):
        assert np.allclose(spec.limits[name], levels * 10 ** (db / 10)), name


def test_results_become_analysis_objects_in_si_as_written(random_project):
    folder, _done, project = random_project
    psd, frf, coherence = (project['response_psd'], project['identification_frf'],
                           project['identification_coherence'])
    assert isinstance(psd, Psd) and isinstance(frf, Frf) and isinstance(coherence, Coherence)
    assert psd.response_dof == ['101Z+', '2'] and psd.reference_dof is None
    raw = np.fromfile(os.path.join(folder, 'results', 'response_psd.bin'), '<f8')
    assert np.allclose(psd.ordinate, raw.reshape(2, -1) * G ** 2)
    assert psd.ordinate_dim == ['acceleration**2/frequency'] * 2
    assert frf.response_dof == ['101Z+', '2'] and frf.reference_dof == ['3', '3']
    assert frf.ordinate_dim == ['acceleration/voltage'] * 2
    written = np.fromfile(os.path.join(folder, 'results', 'identification_frf.bin'),
                          '<c16', offset=128 * 8).reshape(2, 128)
    # cross terms as written: never conjugated here
    assert np.allclose(frf.ordinate, written * G)
    assert np.all(frf.ordinate.imag > 0)
    assert coherence.reference_dof == ['3', '3']
    assert np.allclose(frf.abscissa, 4.0 * np.arange(128))
    assert psd.source == frf.source == coherence.source == 'SIM'


def test_a_repeating_result_is_one_record_per_line(random_project):
    _folder, _done, project = random_project
    loops = project['control_loops']
    assert loops.block == ['record 0', 'record 1', 'record 2']
    assert np.allclose(loops.ordinate[2] / loops.ordinate[0], 1.2)


def test_the_run_record_holds_what_no_object_does(random_project):
    folder, _done, project = random_project
    record = project['Run Record']
    assert isinstance(record, RunRecord)
    assert (record.test, record.run_uid, record.run_id, record.source) == (
        'random', 'uid-random-0001', 'r-0007', 'SIM')
    assert record.complete and record.outcome == 'completed'
    with open(os.path.join(folder, 'setup.json')) as f:
        assert record.setup == json.load(f)
    assert record.config['acquisition']['n_pts'] == 256
    assert record.resolved['profile'] == rf.random_setup()['resolved']['profile']
    assert [entry['seq'] for entry in record.log] == list(range(6))
    assert record.log == rf.random_log()
    # a column the analysis table has none for is kept here
    assert record.table['rows'] == rf.random_rows()
    assert record.table['rows'][0]['extra']['mount'] == 'stud'
    assert record.law['name'] == 'Diagonal'
    assert record.law_file == ('diagonal.py', b'def iterate(state):\n    return state\n')
    assert record.streams['drives']['output_latency_frames'] == {'frames': 38,
                                                                  'how': 'declared'}
    assert np.array_equal(record.arrays['verdicts']['lines_out'], [0.0, 1.0, 0.0])
    assert record.results['control_loops']['records'][1]['frame'] == 2512
    assert record.missing == []


def test_save_and_reopen_loses_nothing(random_project, tmp_path):
    _folder, _done, project = random_project
    again = Project.open(project.save(tmp_path / 'again.hforge'))
    assert list(again) == list(project)
    for name in project:
        assert again[name] == project[name], name
    assert again.object_groups == project.object_groups
    held = again['inputs']
    assert held.roles == project['inputs'].roles
    assert held.run_identity == project['inputs'].run_identity
    assert held.averaging == project['inputs'].averaging


def test_the_same_folder_converts_to_the_same_bytes(tmp_path):
    folder = rf.random_run(str(tmp_path / 'run'))
    first = run_convert.convert(folder, tmp_path / 'first').path
    second = run_convert.convert(folder, tmp_path / 'second').path
    assert filecmp.cmp(first, second, shallow=False)


def test_a_failed_conversion_leaves_no_file(tmp_path):
    folder = rf.random_run(str(tmp_path / 'run'))
    side = os.path.join(folder, 'streams', 'drives', 'stream.json')
    with open(side) as f:
        stream = json.load(f)
    stream['channels'] = [{'index': 9, 'unit': 'V'}]
    with open(side, 'w') as f:
        json.dump(stream, f)
    with pytest.raises(run_folder.RunFolderError, match='channels \\[9\\]'):
        run_convert.convert(folder)
    assert not [n for n in os.listdir(folder) if n.endswith(('.hforge', '.partial'))]


# ---- the Recorder, pulses, a run cut short ------------------------------

def test_a_recording_is_timed_from_its_trigger_and_says_where_it_lost_frames(tmp_path):
    folder = rf.recorder_run(str(tmp_path / 'rec'), frames=2048, pre=256, gap=(1024, 64))
    project = Project.open(run_convert.convert(folder).path)
    history = project['rec_0001']
    assert history.abscissa[0] == pytest.approx(-256 / rf.FS)
    assert np.argmin(np.abs(history.abscissa)) == 256
    assert history.ordinate.shape == (2, 2048)
    assert history.response_dof == ['1', '7X+']
    assert all('64 frames at sample 1024' in comment for comment in history.comment)
    assert (history.averaging.frame_length, history.averaging.overlap,
            history.averaging.frames) == (256, 0.5, 8)
    assert project.project_type is None
    assert project['Run Record'].outcome == 'operator_stop'


def test_a_pulse_stream_is_one_record_per_shot(tmp_path):
    folder = rf.pulse_run(str(tmp_path / 'pulses'), shots=3, frames=512)
    project = Project.open(run_convert.convert(folder).path)
    pulses = project['inputs']
    assert pulses.block == ['shot 1', 'shot 2', 'shot 3']
    assert pulses.ordinate.shape == (3, 512)
    assert pulses.abscissa[0] == pytest.approx(-64 / rf.FS)
    raw = np.fromfile(os.path.join(folder, 'streams', 'inputs', 'telemetry.f32'), '<f4')
    assert np.allclose(pulses.ordinate[1], raw[512:1024] * G, rtol=1e-6)
    assert pulses.comment == ['shot 1 at -12 dB', 'shot 2 at -6 dB', 'shot 3 at 0 dB']


def test_an_unfinished_run_converts_what_it_wrote_and_says_what_it_lacked(tmp_path):
    folder = rf.unfinished_run(str(tmp_path / 'cut'), frames=2048)
    done = run_convert.convert(folder)
    assert done.lacked == [
        'the manifest says the run is not complete',
        'the run did not end cleanly: there is no end.json',
        'the last line of the log was cut short (27 bytes), so it is left out',
        "the stream 'inputs' ends 6 bytes into a frame, which is left out",
        "the stream 'inputs' did not complete", "the stream 'drives' did not complete"]
    project = Project.open(done.path)
    assert project['inputs'].ordinate.shape == (2, 2048)
    record = project['Run Record']
    assert not record.complete and record.outcome == '' and record.end is None
    assert len(record.log) == 6 and record.missing == done.lacked


def test_a_stream_is_as_long_as_its_file_holds(tmp_path):
    folder = rf.random_run(str(tmp_path / 'run'), frames=1000)
    with open(os.path.join(folder, 'streams', 'inputs', 'telemetry.f32'), 'r+b') as f:
        f.truncate(600 * 2 * 4)
    run = run_folder.read(folder)
    assert run.streams['inputs'].frames == 600
    assert "the stream 'inputs' says 1000 frames and its file holds 600; the file is taken" \
        in run.missing


# ---- refusals ----------------------------------------------------------

def _edit_manifest(folder, **changes):
    path = os.path.join(folder, 'run.json')
    with open(path) as f:
        manifest = json.load(f)
    for key, value in changes.items():
        if value is None:
            manifest.pop(key)
        else:
            manifest[key] = value
    with open(path, 'w') as f:
        json.dump(manifest, f)


def test_a_run_with_no_source_label_is_refused_not_guessed(tmp_path):
    folder = rf.random_run(str(tmp_path / 'run'))
    _edit_manifest(folder, source=None)
    with pytest.raises(run_folder.RunFolderError, match='has no source label.*never guessed'):
        run_convert.convert(folder)
    _edit_manifest(folder, source='SIMULATED')
    with pytest.raises(run_folder.RunFolderError, match="the source 'SIMULATED'"):
        run_convert.convert(folder)
    assert not [n for n in os.listdir(folder) if n.endswith('.hforge')]


def test_a_newer_run_folder_schema_is_refused(tmp_path):
    folder = rf.random_run(str(tmp_path / 'run'))
    _edit_manifest(folder, schema='hertzforge-run/2')
    with pytest.raises(run_folder.RunFolderError, match=(
            r'newer Hertzforge \(run folder schema 2; this build reads up to 1\)')):
        run_folder.read(folder)


def test_unknown_members_are_ignored_and_kept(tmp_path):
    folder = rf.random_run(str(tmp_path / 'run'))
    _edit_manifest(folder, later_member={'a': 1})
    record = Project.open(run_convert.convert(folder).path)['Run Record']
    assert record.manifest['later_member'] == {'a': 1}


def test_a_result_whose_unit_its_channels_do_not_give_is_kept_as_arrays(tmp_path):
    folder = rf.random_run(str(tmp_path / 'run'))
    side = os.path.join(folder, 'results', 'response_psd.json')
    with open(side) as f:
        description = json.load(f)
    description['arrays'][0]['unit'] = 'm^2/s^4/Hz'
    with open(side, 'w') as f:
        json.dump(description, f)
    done = run_convert.convert(folder)
    assert 'response_psd' not in done.objects
    assert any("'response_psd' is kept as arrays" in lack for lack in done.lacked)
    record = Project.open(done.path)['Run Record']
    assert record.arrays['response_psd']['values'].shape == (2, 128)


# ---- bounded memory ----------------------------------------------------

def _big_run(folder, frames, channels=8):
    """A run whose one stream is `frames` by `channels` float32, written a
    block at a time so the test itself stays small."""
    rows = [rf.row(k + 1, 'control' if k == 0 else 'monitor') for k in range(channels)]
    rf.write_run(folder, rf._manifest('recorder'), {'rows': rows}, {'config': {}},
                 streams={'long': ({'kind': 'inputs', 'fs_hz': rf.FS,
                                    'channels': [{'index': k + 1, 'unit': 'g'}
                                                 for k in range(channels)],
                                    'frames': frames, 'gaps': []},
                                   np.zeros((0, channels), np.float32))},
                 end={'end': {'outcome': 'completed'}})
    block = np.tile(np.arange(65536, dtype=np.float32)[:, None] / 65536, (1, channels))
    with open(os.path.join(folder, 'streams', 'long', 'telemetry.f32'), 'wb') as f:
        f.writelines(block[:min(65536, frames - first)].tobytes() for first in range(0, frames, 65536))
    return folder


def _peak_converting(folder):
    tracemalloc.start()
    started = time.perf_counter()
    done = run_convert.convert(folder)
    seconds = time.perf_counter() - started
    _current, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    return done, peak, seconds


def test_a_stream_is_copied_in_bounded_memory(tmp_path, monkeypatch):
    """A 64 MiB stream, copied with 1 MiB blocks, peaks under 8 MiB of
    allocations: the bound is the block, not the stream."""
    monkeypatch.setattr(native, 'STREAM_BLOCK_BYTES', 2**20)
    folder = _big_run(str(tmp_path / 'long'), frames=2**21)
    done, peak, _seconds = _peak_converting(folder)
    assert peak < 8 * 2**20, f'peak {peak / 2**20:.1f} MiB'
    with h5py.File(done.path) as f:
        stream = next(g for g in f['objects'].values() if g.attrs['name'] == 'long')
        assert stream['ordinate'].shape == (8, 2**21)
        assert stream['ordinate'].dtype == np.float32
        assert stream['ordinate'][3, 65537] == pytest.approx(G / 65536)


@pytest.mark.slow
def test_a_gigabyte_stream_converts_under_a_stated_ceiling(tmp_path):
    """A 1 GiB stream (8 channels, 32 Mi frames) converts with the default
    block, peaking under 128 MiB of allocations."""
    folder = _big_run(str(tmp_path / 'long'), frames=2**25)
    _done, peak, seconds = _peak_converting(folder)
    print(f'1 GiB stream: peak {peak / 2**20:.1f} MiB, {seconds:.1f} s')
    assert peak < 128 * 2**20
