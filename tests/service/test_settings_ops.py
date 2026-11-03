"""A time history's analysis settings set from the shell: averaging, filter
and truncation, which the verbs then read, and the summary that lets a
form open on what the history holds."""

from __future__ import annotations

from pathlib import Path

import pytest

from hertzforge.service import projects
from hertzforge.service.ops_settings import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .test_ops import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in OPERATIONS}


@pytest.fixture
def state():
    opened, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    return opened


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()})
    return op.run(project, values)


def settings(state, name='Time History'):
    return next(o for o in projects.summary(state)['objects']
                if o['name'] == name).get('settings', {})


def test_the_descriptors_validate_and_open_on_settings(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')
        assert op.edits
        assert all('setting' in p.to_json() for p in op.params
                   if p.name not in ('source', 'record'))


def test_averaging_set_is_stored_read_back_and_listed(state, validate):
    project = state.project
    trace, reading = run(project, 'project.set_averaging', frame_length=2048, overlap=0.5,
                         window='flattop', frames=4, start=0.5)
    wire_views([trace, reading], validate)
    rate = project['Time History'].sample_rate
    frames = trace['regions']
    assert [r['kind'] for r in frames] == ['frame'] * 4
    assert frames[0]['x0'] == pytest.approx(0.5, abs=1 / rate)
    assert frames[1]['x0'] - frames[0]['x0'] == pytest.approx(1024 / rate)
    assert frames[0]['label'].startswith('Frame 1, ')
    averaging = project['Time History'].averaging
    assert (averaging.frame_length, averaging.window, averaging.frames) == (2048, 'flattop', 4)
    rows = {row['label']: row['value'] for row in reading['rows']}
    assert rows['Line spacing'] == pytest.approx(project['Time History'].sample_rate / 2048)
    assert settings(state)['averaging'] == {'frame_length': 2048, 'overlap': 0.5,
                                            'window': 'flattop', 'frames': 4,
                                            'start': 0.5, 'detrend': 'none'}
    validate(projects.summary(state), 'project')


def test_averaging_the_record_cannot_hold_is_refused(state):
    with pytest.raises(ProtocolError, match='holds .* such frames') as error:
        run(state.project, 'project.set_averaging', frame_length=65536, frames=100)
    assert error.value.code == 'refused'
    assert state.project['Time History'].averaging is None or \
        state.project['Time History'].averaging.frames != 100


def test_detected_averaging_frames_the_steady_stretch(state):
    trace, reading = run(state.project, 'project.detect_averaging')
    assert trace['regions'] and trace['regions'][0]['kind'] == 'frame'
    assert state.project['Time History'].averaging is not None
    assert {row['label'] for row in reading['rows']} >= {'Frames averaged', 'Line spacing'}


@pytest.mark.parametrize(('kind', 'said'), [('low-pass', 'low-pass at 400 Hz'),
                                            ('high-pass', 'high-pass at 20 Hz'),
                                            ('band-pass', 'band-pass')])
def test_a_filter_takes_the_edges_its_kind_has(state, kind, said):
    trace, reading = run(state.project, 'project.set_filter', kind=kind, low=20.0, high=400.0)
    assert [line['role'] for line in trace['series']] == ['measured', 'drive']
    assert trace['series'][0]['name'].endswith(', filtered')
    assert said in reading['rows'][0]['value']
    listed = settings(state)['filtering']
    assert listed['kind'] == kind
    assert ('low' in listed) == (kind != 'low-pass')
    assert ('high' in listed) == (kind != 'high-pass')


def test_a_filter_edge_above_nyquist_is_refused(state):
    with pytest.raises(ProtocolError, match='below half the sample rate'):
        run(state.project, 'project.set_filter', kind='low-pass', high=1e6)
    assert state.project['Time History'].filtering is None


def test_a_truncation_span_lets_apply_truncation_run(state):
    project = state.project
    trace, _reading = run(project, 'project.set_truncation', start=0.5, stop=1.5)
    assert settings(state)['truncation'] == {'start': 0.5, 'stop': 1.5}
    cut = trace['regions']
    assert [r['kind'] for r in cut] == ['cut', 'cut']
    assert (cut[0]['x1'], cut[1]['x0']) == (0.5, 1.5)
    added = project.truncate_data('Time History')
    assert float(project[added].abscissa[-1]) <= 1.5


def test_a_span_outside_the_record_is_refused(state):
    with pytest.raises(ProtocolError, match='is not inside it'):
        run(state.project, 'project.set_truncation', start=0.0, stop=1e6)


def test_many_frames_shade_as_one_span(state):
    trace, _reading = run(state.project, 'project.set_averaging', frame_length=64,
                          overlap=0.0, frames=300)
    (span,) = trace['regions']
    assert span['label'].startswith('300 frames')


def test_the_record_drawn_on_must_be_in_the_history(state):
    with pytest.raises(ProtocolError, match='there is no record 99'):
        run(state.project, 'project.set_truncation', start=0.5, stop=1.5, record=99)
