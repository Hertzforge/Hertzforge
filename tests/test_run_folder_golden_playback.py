"""The bridge from a Playback run to the analysis side, on a folder the
control service wrote: the golden Playback run folder
(`schema/golden/runs/playback/`, written by the control service's own
test `playback_golden_run` on the simulator) converts with
`hertzforge.io.run_convert` into a project that `Project.open` opens,
with the channel table that was set, every input and every drive as
played as time histories with their units and source, and the run
record with the setup (the drive folder, its SHA-256 and its sidecar),
the log, the drive file's check (`playback_check`) and the outcome
(`playback_outcome`); and `docs/run-folder.md` says exactly what a
Playback run saves."""

from __future__ import annotations

import json
import os
import re
import shutil

import numpy as np
import pytest

from hertzforge import Project
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.data import TimeHistory
from hertzforge.core.run_record import RunRecord
from hertzforge.io import run_convert, run_folder

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GOLDEN = os.path.join(ROOT, 'schema', 'golden', 'runs', 'playback')
G = 9.80665


def _json(*parts, root=GOLDEN):
    with open(os.path.join(root, *parts), encoding='utf-8') as f:
        return json.load(f)


def _same(a, b, where='report'):
    if isinstance(b, dict):
        assert list(a) == list(b), where
        for k in b:
            _same(a[k], b[k], f'{where}.{k}')
    elif isinstance(b, list):
        assert len(a) == len(b), where
        for i, (u, v) in enumerate(zip(a, b)):
            _same(u, v, f'{where}[{i}]')
    elif isinstance(b, float) and not isinstance(b, bool):
        assert a == pytest.approx(b, rel=1e-12, abs=0), where
    else:
        assert a == b, where


@pytest.fixture(scope='module')
def converted(tmp_path_factory):
    folder = str(tmp_path_factory.mktemp('runs') / 'playback')
    shutil.copytree(GOLDEN, folder)
    done = run_convert.convert(folder)
    return folder, done, Project.open(done.path)


def test_the_folder_converts_and_opens_with_nothing_lacking(converted):
    _folder, done, project = converted
    assert done.lacked == []
    assert list(project) == done.objects == ['Channel Table', 'inputs', 'drives',
                                             'Run Record']
    assert project.name == 'Golden Playback'
    run = run_folder.read(GOLDEN)
    assert run.test == 'playback' and run.source == 'SIM'


def test_the_channel_table_is_the_table_that_was_set(converted):
    _folder, _done, project = converted
    table = project['Channel Table']
    assert isinstance(table, ChannelTable) and table.has_controller_settings
    assert list(table['channel']) == [1, 2]
    assert list(table['comment']) == ['Control', 'Drive']
    assert list(table['unit']) == ['g', 'V']
    rows = _json('table.json')['rows']
    back = run_convert.rows_from_channel_table(table)
    for given, read in zip(rows, back, strict=True):
        assert {k: v for k, v in read.items() if k not in run_convert.ANALYSIS_MEMBERS} == given


def test_the_inputs_and_the_drives_are_time_histories_with_their_units(converted):
    folder, _done, project = converted
    inputs, drives = project['inputs'], project['drives']
    side = _json('streams', 'inputs', 'stream.json')
    fs, frames = side['fs_hz'], side['frames']
    assert fs == _json('setup.json')['check']['rate_hz'] == _json('run.json')['grid']['fs_hz']
    assert isinstance(inputs, TimeHistory) and isinstance(drives, TimeHistory)
    assert inputs.ordinate.shape == (1, frames) and drives.ordinate.shape == (1, frames)
    assert inputs.response_dof == ['1'] and drives.response_dof == ['2']
    assert inputs.ordinate_unit == ['g'] and drives.ordinate_unit == ['V']
    assert inputs.ordinate_dim == ['acceleration']
    assert inputs.source == drives.source == 'SIM'
    assert drives.roles == {('2', 'voltage'): 'reference'}
    assert inputs.abscissa[0] == 0.0
    assert inputs.abscissa[1] - inputs.abscissa[0] == pytest.approx(1 / fs)
    raw = np.fromfile(os.path.join(folder, 'streams', 'inputs', 'telemetry.f32'), dtype='<f4')
    assert np.array_equal(inputs.ordinate[0], (raw.astype(np.float64) * G).astype(np.float32))
    drive = np.fromfile(os.path.join(folder, 'streams', 'drives', 'telemetry.f32'), dtype='<f4')
    assert np.array_equal(drives.ordinate[0], drive)
    # the pre-roll's zeros, then the file at the level, then the post-roll's zeros: the
    # drives as the device accepted them, so the file plays after the pre-roll and within
    # its own length
    setup = _json('setup.json')
    pre = round(setup['config']['pre_roll_s'] * fs)
    samples = setup['check']['samples']
    played = np.flatnonzero(drive)
    assert played[0] >= pre and played[-1] - played[0] < samples
    assert drive[-1] == 0.0
    played = setup['check']['channels'][0]['played_peak_v']
    assert np.abs(drive).max() == pytest.approx(played, rel=1e-6)
    assert np.abs(drive).max() <= setup['config']['ceiling_v']
    assert inputs.run_identity.run == drives.run_identity.run == _json('run.json')['run_id']


def test_the_run_record_holds_the_setup_the_log_the_check_and_the_outcome(converted):
    _folder, _done, project = converted
    record = project['Run Record']
    assert isinstance(record, RunRecord)
    assert record.test == 'playback' and record.source == 'SIM'
    setup = _json('setup.json')
    assert record.setup == setup
    assert record.config == setup['config']
    resolved = record.setup['resolved']
    assert resolved['drive']['sha256'] == record.config['drive']['sha256']
    assert resolved['drive']['sidecar']['sha256'] == record.config['drive']['sha256']
    assert [c['drive_row'] for c in resolved['drive']['channels']] == record.config['map']
    assert record.manifest['operator'] == 'A. Operator'
    assert record.manifest['specification'] == {'document': 'Test procedure', 'clause': '4.2'}
    with open(os.path.join(GOLDEN, 'log.jsonl'), encoding='utf-8') as f:
        log = [json.loads(line) for line in f]
    assert record.log == log
    assert [e['seq'] for e in record.log] == list(range(len(log)))
    events = [e['data']['event'] for e in record.log if 'event' in e.get('data', {})]
    assert events == ['check_passed', 'armed', 'pre_roll', 'playing', 'post_roll',
                      'post_roll_done']
    assert record.log[-1]['kind'] == 'outcome'
    # the check: the report whole, and each channel's numbers and envelopes as arrays
    check = record.results['playback_check']
    assert check['kind'] == 'drive_check'
    # the report the run was configured with, number for number (each file writes its
    # own shortest form of a float)
    _same(check['report'], setup['check'])
    assert check['channels'] == [{'file_channel': 0, 'name': 'drive', 'drive_row': 2}]
    arrays = record.arrays['playback_check']
    one = setup['check']['channels'][0]
    assert arrays['peak'][0] == pytest.approx(one['peak_v'])
    assert arrays['velocity_end'][0] == pytest.approx(one['velocity_v_s']['end'])
    assert arrays['displacement_peak'][0] == pytest.approx(one['displacement_v_s2']['peak'])
    assert arrays['drive_min'].shape == arrays['drive_max'].shape
    assert np.all(arrays['drive_min'] <= arrays['drive_max'])
    # the outcome
    end = _json('end.json')
    outcome = record.results['playback_outcome']
    assert record.complete and record.outcome == 'completed' == end['end']['outcome']
    assert (outcome['ended'], outcome['outcome'], outcome['emergency_stop']) == (
        'completed', 'completed', False)
    out = record.arrays['playback_outcome']
    assert out['played'][0] == end['end']['played'] == setup['check']['samples']
    assert out['played_s'][0] == pytest.approx(setup['check']['duration_s'])
    rolls = setup['derived']['pre_roll_frames'] + setup['derived']['post_roll_frames']
    assert out['generated'][0] >= out['played'][0] + rolls
    assert record.streams['inputs'] == _json('streams', 'inputs', 'stream.json')
    assert record.streams['drives'] == _json('streams', 'drives', 'stream.json')
    assert record.missing == []


def test_save_and_reopen_loses_nothing(converted, tmp_path):
    _folder, _done, project = converted
    again = Project.open(project.save(tmp_path / 'again.hforge'))
    assert list(again) == list(project)
    for name in project:
        assert again[name] == project[name], name
    assert again.object_groups == project.object_groups


def _saved_by_the_guide():
    with open(os.path.join(ROOT, 'docs', 'run-folder.md'), encoding='utf-8') as f:
        text = f.read()
    section = text.split('## What a Playback Run Saves', 1)[1].split('\n## ', 1)[0]
    return dict(re.findall(r'^\| `([^`]+)` \| (.+) \|$', section, flags=re.MULTILINE))


def test_the_guide_says_exactly_what_a_playback_run_saves():
    named = _saved_by_the_guide()
    assert named, 'the guide lists nothing'
    run = run_folder.read(GOLDEN)
    on_disk = {f'streams/{n}' for n in run.streams} | {f'results/{n}' for n in run.results}
    on_disk.add('end.json')
    assert set(named) == on_disk
    for name, result in run.results.items():
        assert f'`kind` `{result.kind}`' in named[f'results/{name}'], name
    for name, stream in run.streams.items():
        assert f'`kind` `{stream.kind}`' in named[f'streams/{name}'], name


def test_the_playback_guide_maps_the_run_to_the_objects_it_becomes(converted):
    _folder, done, _project = converted
    with open(os.path.join(ROOT, 'docs', 'playback.md'), encoding='utf-8') as f:
        text = f.read()
    section = text.split('## What a Run Saves', 1)[1].split('\n## ', 1)[0]
    objects = set(re.findall(r'^\| `[^`]+` \| (?:[^|]+) \| ([^|]+) \|$', section,
                             flags=re.MULTILINE))
    named = {o.strip() for cell in objects for o in re.findall(r'\*\*([^*]+)\*\*', cell)}
    assert named == set(done.objects)
