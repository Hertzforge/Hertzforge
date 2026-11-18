"""A time history's analysis settings set from the shell: averaging, filter
and truncation, which the verbs then read, and the summary that lets a
form open on what the history holds."""

from __future__ import annotations

from pathlib import Path

import pytest

from hertzforge.service import projects
from hertzforge.service.ops_settings import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

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
        # a drag's span is where the pointer let go, not a setting to open on
        assert all('setting' in p.to_json() for p in op.params
                   if p.name not in ('source', 'record', 'keep')
                   and not (op.id == 'project.drag_averaging'
                            and p.name in ('start', 'stop', 'was_stop')))


def test_averaging_set_is_stored_read_back_and_listed(state, validate):
    project = state.project
    trace, reading = run(project, 'project.set_averaging', frame_length=2048, overlap=0.5,
                         window='flattop', frames=4, start=0.5)
    wire_views([trace, reading], validate)
    rate = project['Time History'].sample_rate
    *frames, span = trace['regions']
    assert [r['kind'] for r in frames] == ['frame'] * 4
    # the span over every frame is dragged: its middle slides them, an edge adds or removes
    history = project['Time History']
    assert span['snap'] == pytest.approx({
        'frame': 2048 / rate, 'hop': 1024 / rate, 'low': float(history.abscissa[0]),
        'high': float(history.abscissa[0]) + len(history.abscissa) / rate})
    assert span['kind'] == 'span' and span['drag'] == {
        'op': 'project.drag_averaging', 'low': 'start', 'high': 'stop',
        'params': {'source': 'Time History', 'frame_length': 2048, 'overlap': 0.5,
                   'window': 'flattop', 'detrend': 'none', 'was_stop': span['x1']}}
    # each frame carries its window and its row: at half overlap they alternate between two
    assert [r['level'] for r in frames] == [0, 1, 0, 1] and {r['rows'] for r in frames} == {2}
    window = frames[0]['window']
    assert len(window) == 33 and max(window) == 1.0 and abs(window[0]) < 0.01
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
    *cut, kept = trace['regions']
    assert [r['kind'] for r in cut] == ['cut', 'cut']
    assert (cut[0]['x1'], cut[1]['x0']) == (0.5, 1.5)
    assert (kept['x0'], kept['x1']) == (0.5, 1.5)
    assert kept['drag'] == {'op': 'project.set_truncation',
                            'params': {'source': 'Time History', 'record': 1},
                            'low': 'start', 'high': 'stop'}
    added = project.truncate_data('Time History')
    assert float(project[added].abscissa[-1]) <= 1.5


def test_a_span_outside_the_record_is_refused(state):
    with pytest.raises(ProtocolError, match='is not inside it'):
        run(state.project, 'project.set_truncation', start=0.0, stop=1e6)


def test_many_frames_shade_as_one_span(state):
    trace, _reading = run(state.project, 'project.set_averaging', frame_length=64,
                          overlap=0.0, frames=300)
    assert len(trace['regions']) == 301 and 'window' in trace['regions'][0], 'each still drawn'
    trace, _reading = run(state.project, 'project.set_averaging', frame_length=64,
                          overlap=0.0, frames=1200)
    span, _drag = trace['regions']
    assert span['label'].startswith('1200 frames') and 'window' not in span


def test_the_record_drawn_on_must_be_in_the_history(state):
    with pytest.raises(ProtocolError, match='there is no record 99'):
        run(state.project, 'project.set_truncation', start=0.5, stop=1.5, record=99)


def test_a_dragged_frame_span_slides_the_frames_and_keeps_the_rest(state):
    project = state.project
    trace, _ = run(project, 'project.set_averaging', frame_length=2048, overlap=0.5,
                   window='flattop', frames=4, start=0.5)
    span = trace['regions'][-1]
    drag = span['drag']
    history = project['Time History']
    rate = history.sample_rate
    hop = 1024 / rate

    def dragged(x0, x1, was_stop=-1.0):
        run(project, drag['op'], **{**drag['params'], drag['low']: x0, drag['high']: x1,
                                    'was_stop': was_stop})
        return history.averaging

    # the middle dragged: the same frames, half a second on
    averaging = dragged(span['x0'] + 0.5, span['x1'] + 0.5)
    assert averaging.start == pytest.approx(1.0, abs=1 / rate)
    assert (averaging.frame_length, averaging.window, averaging.frames) == (2048, 'flattop', 4)
    assert averaging.overlap == 0.5 and averaging.start_set
    # the right edge dragged out two and a half hops: two more whole frames
    low = 1.0 + float(history.abscissa[0])
    averaging = dragged(low, low + (2048 + 3 * 1024) / rate + 2.5 * hop)
    assert averaging.frames == 6 and averaging.start == pytest.approx(1.0, abs=1 / rate)
    # the left edge dragged out two and a half hops: two more whole frames before the first,
    # and the frames end where they did
    origin = float(history.abscissa[0])
    stop = averaging.stop(rate, origin)
    averaging = dragged(low - 2.5 * hop, stop, was_stop=stop)
    assert averaging.frames == 8 and averaging.stop(rate, origin) == pytest.approx(stop)
    assert averaging.start == pytest.approx(1.0 - 2 * hop, abs=1 / rate)
    # dragged in three and a half hops, three frames go from the start
    averaging = dragged(origin + averaging.start + 3.5 * hop, stop, was_stop=stop)
    assert averaging.frames == 4 and averaging.stop(rate, origin) == pytest.approx(stop)
    # dragged out past the start of the record, the frames that fit before the end
    averaging = dragged(origin - 5.0, stop, was_stop=stop)
    assert averaging.stop(rate, origin) == pytest.approx(stop) and averaging.start < hop
    assert averaging.frames == 1 + round((stop - origin) * rate - 2048) // 1024
    # an edge dragged in past one frame leaves one
    assert dragged(low, low + 0.2 * hop).frames == 1
    # slid past the end of the record, the frames stop at its end
    end = float(history.abscissa[-1])
    averaging = dragged(end, end + 2048 / rate)
    assert averaging.frames == 1 and averaging.fits(len(history.abscissa), rate)
    assert averaging.stop(rate, float(history.abscissa[0])) == pytest.approx(end, abs=2 / rate)
    assert settings(state)['averaging']['frames'] == 1


def test_filter_is_set_previewed_and_applied_in_one_action(state, validate):
    project = state.project
    before = list(project)
    # the preview stores the settings and adds nothing
    trace, reading = run(project, 'project.set_filter', kind='low-pass', high=300.0)
    wire_views([trace, reading], validate)
    assert list(project) == before
    history = next(obj for obj in project.values() if type(obj).__name__ == 'TimeHistory')
    assert history.filtering.describe() == reading['rows'][0]['value']
    # Apply adds the filtered history, made with those settings
    (drawn,) = run(project, 'project.set_filter', kind='low-pass', high=300.0, keep=True)
    wire_views([drawn], validate)
    added = [name for name in project if name not in before]
    assert len(added) == 1 and added[0].endswith('Filtered')
    assert drawn['view'] == 'waterfall' and drawn['title'] == added[0]
    op = OPS['project.set_filter']
    assert op.commits == 'keep' and op.title == 'Filter'
