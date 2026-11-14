"""The bridge from a Recorder run to the analysis side, on a folder the
control service wrote: the golden Recorder run folder
(`schema/golden/runs/recorder/`, written by the control service's own
test `recorder_golden_run` on the simulator) converts with
`hertzforge.io.run_convert` into a project that `Project.open` opens,
with the channel table that was set, each recording as a time history
whose pre-trigger samples are at negative time, and the run record with
the setup, the log, the marks, the gaps and the outcome; and
`docs/run-folder.md` says exactly what a Recorder run saves."""

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
GOLDEN = os.path.join(ROOT, 'schema', 'golden', 'runs', 'recorder')
G = 9.80665


def _json(*parts, root=GOLDEN):
    with open(os.path.join(root, *parts), encoding='utf-8') as f:
        return json.load(f)


@pytest.fixture(scope='module')
def converted(tmp_path_factory):
    folder = str(tmp_path_factory.mktemp('runs') / 'recorder')
    shutil.copytree(GOLDEN, folder)
    done = run_convert.convert(folder)
    return folder, done, Project.open(done.path)


def test_the_folder_converts_and_opens_with_nothing_lacking(converted):
    _folder, done, project = converted
    assert done.lacked == []
    assert list(project) == done.objects == ['Channel Table', 'rec_0001', 'Run Record']
    assert project.name == 'Golden Recorder'
    assert run_folder.read(GOLDEN).test == 'recorder'


def test_the_channel_table_is_the_table_that_was_set(converted):
    _folder, _done, project = converted
    table = project['Channel Table']
    assert isinstance(table, ChannelTable) and table.has_controller_settings
    assert list(table['channel']) == [1, 2]
    assert list(table['comment']) == ['Markers', 'Tone']
    assert list(table['unit']) == ['g', 'g']
    assert list(table['range_max']) == ['0.5', '0.5']
    assert list(table['role']) == ['response', 'response']
    rows = _json('table.json')['rows']
    back = run_convert.rows_from_channel_table(table)
    for given, read in zip(rows, back, strict=True):
        assert {k: v for k, v in read.items() if k != 'extra'} == given


def test_each_recording_is_a_time_history_from_its_trigger(converted):
    folder, _done, project = converted
    side = _json('streams', 'rec_0001', 'stream.json')
    history = project['rec_0001']
    assert isinstance(history, TimeHistory)
    frames, pre, fs = side['frames'], side['pre_frames'], side['fs_hz']
    assert pre > 0 and frames > pre
    assert history.ordinate.shape == (2, frames)
    assert history.response_dof == ['1', '2']
    assert history.ordinate_unit == ['g', 'g']
    assert history.source == 'SIM'
    # t = 0 is the trigger: the pre-window's samples are at negative time
    assert history.abscissa[0] == pytest.approx(-pre / fs)
    assert history.abscissa[pre] == pytest.approx(0.0, abs=1e-12)
    assert np.count_nonzero(history.abscissa < 0) == pre
    assert history.abscissa[1] - history.abscissa[0] == pytest.approx(1 / fs)
    raw = np.fromfile(os.path.join(folder, 'streams', 'rec_0001', 'telemetry.f32'),
                      dtype='<f4').reshape(-1, 2)
    assert np.array_equal(history.ordinate, (raw.T.astype(np.float64) * G).astype(np.float32))
    # the trigger: row 1 rises through 0.25 g at file frame `pre`, not before
    assert raw[pre, 0] >= 0.25 > raw[pre - 1, 0]
    assert history.run_identity.run == _json('run.json')['run_id']
    assert history.averaging is not None and history.averaging.frame_length == 256


def test_the_run_record_holds_the_setup_the_log_the_marks_and_the_outcome(converted):
    _folder, _done, project = converted
    record = project['Run Record']
    assert isinstance(record, RunRecord)
    assert record.test == 'recorder' and record.source == 'SIM'
    assert record.setup == _json('setup.json')
    assert record.config['trigger']['mode'] == 'level'
    with open(os.path.join(GOLDEN, 'log.jsonl')) as f:
        log = [json.loads(line) for line in f]
    assert record.log == log
    assert [e['seq'] for e in record.log] == list(range(len(log)))
    assert record.complete and record.outcome == 'operator_stop' == _json('end.json')['end'][
        'outcome']
    side = record.streams['rec_0001']
    assert side == _json('streams', 'rec_0001', 'stream.json')
    # the marks: in the recording they fell in, and in the log with their facts
    marked = [e['data'] for e in record.log if 'data' in e]
    assert side['marks'] and marked == side['marks']
    assert all(e['frame'] == e['data']['station_frame'] for e in record.log if 'data' in e)
    # the gaps (none in the golden run) and the trigger
    assert side['gaps'] == []
    assert side['trigger_station_frame'] - side['first_station_frame'] == side['pre_frames']
    assert record.missing == []


def test_a_gap_in_a_recording_is_kept_where_it_happened(tmp_path):
    # The golden recording with 40 frames missing at file frame 150, as the
    # Recorder states a gap: the samples after it follow on, never padded.
    folder = str(tmp_path / 'gap')
    shutil.copytree(GOLDEN, folder)
    where = os.path.join(folder, 'streams', 'rec_0001')
    raw = np.fromfile(os.path.join(where, 'telemetry.f32'), dtype='<f4').reshape(-1, 2)
    kept = np.concatenate([raw[:150], raw[190:]])
    kept.astype('<f4').tofile(os.path.join(where, 'telemetry.f32'))
    side = _json('stream.json', root=where)
    side['frames'] = len(kept)
    side['gaps'] = [{'at_frame': 150, 'dropped_frames': 40}]
    with open(os.path.join(where, 'stream.json'), 'w', encoding='utf-8') as f:
        json.dump(side, f)
    done = run_convert.convert(folder)
    project = Project.open(done.path)
    history = project['rec_0001']
    assert history.ordinate.shape == (2, len(kept))
    said = ('gaps in the recording, not padded (the samples after each follow on): '
            '40 frames at sample 150')
    assert history.comment == [said, said]
    assert project['Run Record'].streams['rec_0001']['gaps'] == side['gaps']


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
    section = text.split('## What a Recorder Run Saves', 1)[1].split('\n## ', 1)[0]
    return dict(re.findall(r'^\| `([^`]+)` \| (.+) \|$', section, flags=re.MULTILINE))


def test_the_guide_says_exactly_what_a_recorder_run_saves():
    named = _saved_by_the_guide()
    assert named, 'the guide lists nothing'
    run = run_folder.read(GOLDEN)
    assert run.streams and all(re.fullmatch(r'rec_\d{4}', n) for n in run.streams)
    on_disk = {'streams/rec_NNNN', 'end.json'}
    assert set(named) == on_disk
    assert not run.results, 'a Recorder run has no results'
    # each recording's folder holds exactly the files its row names
    said = set(re.findall(r'`([^`]+)`', named['streams/rec_NNNN']))
    for name in run.streams:
        held = set(os.listdir(os.path.join(GOLDEN, 'streams', name)))
        assert held <= said and {'telemetry.f32', 'stream.json', 'capture.json'} <= held
