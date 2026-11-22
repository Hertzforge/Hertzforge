"""A zoomed 2-D plot is drawn from the samples in its span, not from the
record's thinned outline (`service.zoom`, `decimate.drawing_window`)."""

from __future__ import annotations

import asyncio
import json

import numpy as np
import pytest

from hertzforge.core.data import TimeHistory
from hertzforge.decimate import drawing_window, peak_decimate, peak_decimate_rows
from hertzforge.service import display_units, frames, projects
from hertzforge.service import views as v
from hertzforge.service.descriptors import Operation
from hertzforge.service.protocol import ProtocolError
from hertzforge.service.server import Connection, Service

from .conftest import GOLDEN

RATE = 2048.0
SAMPLES = 200_000
#: the span Brandon zoomed a force record to
LOW, HIGH = 0.34, 0.55
POINTS = 2400


def _time() -> np.ndarray:
    return np.arange(SAMPLES) / RATE


def _force(seed: int = 1) -> np.ndarray:
    return np.random.default_rng(seed).standard_normal(SAMPLES)


@pytest.fixture
def connection():
    service = Service()
    connection = Connection(service, None)
    connection.project = projects.new({'title': 't'})
    # values as computed; the display system's lbf would scale every force
    connection.units = display_units.DEFAULT
    connection.project.project.add('Force', TimeHistory(
        _time(), np.vstack([_force(1), _force(2)]), ['1Z+', '2Z+'], ordinate_dim='force'))
    yield connection
    service.close()


def _wire(message):
    return frames.to_wire(message)[0]


def _op(connection, params):
    return asyncio.run(connection.project_op(params))


def _show(connection, params):
    return asyncio.run(connection.project_request('project.show', params))


def _spanned(t, y, low, high):
    """The samples a span keeps whole: those from low to high and one on each side."""
    inside = np.flatnonzero((t >= low) & (t <= high))
    keep = np.arange(inside[0] - 1, inside[-1] + 2)
    return t[keep], y[keep]


def _in_span(series, low, high):
    x, y = np.asarray(series['x']), np.asarray(series['y'])
    first = np.searchsorted(x, low) - 1
    last = np.searchsorted(x, high, side='right')
    return x[first:last + 1], y[first:last + 1]


def test_a_plot_zoomed_to_a_short_span_gets_every_sample_in_it_in_order(connection, validate):
    params = {'op': 'project.plot', 'params': {'data': 'Force', 'records': '1-2'}}
    whole = _op(connection, params)['views'][0]
    validate(_wire(whole), 'spectrumView')
    assert whole['span'] == {'method': 'op.run', 'params': params, 'view': 0}
    t = _time()
    samples = int(((t >= LOW) & (t <= HIGH)).sum())
    # the outline the plot opens on holds a handful of points in the span
    outline = np.asarray(whole['series'][0]['x'])
    assert ((outline >= LOW) & (outline <= HIGH)).sum() < 20 < samples

    sent = {**params, 'span': {'low': LOW, 'high': HIGH, 'points': POINTS}}
    validate(sent, 'opRunParams')
    zoomed = _op(connection, sent)['views'][0]
    validate(_wire(zoomed), 'spectrumView')
    assert [s['name'] for s in zoomed['series']] == [s['name'] for s in whole['series']]
    for series, seed in zip(zoomed['series'], (1, 2), strict=True):
        want_x, want_y = _spanned(t, _force(seed), LOW, HIGH)
        got_x, got_y = _in_span(series, LOW, HIGH)
        assert got_x.size == want_x.size == samples + 2
        np.testing.assert_array_equal(got_x, want_x)
        np.testing.assert_array_equal(got_y, want_y)
        # in order, and the rest of the record is still there to pan to
        assert np.all(np.diff(series['x']) > 0)
        # one slice of the outline is about 0.05 s
        assert series['x'][0] < 0.1 and series['x'][-1] > t[-1] - 0.1
    # time is seconds in every display system, so the span means the same in lbf
    connection.units = display_units.START
    lbf = _op(connection, sent)['views'][0]['series'][0]
    np.testing.assert_array_equal(_in_span(lbf, LOW, HIGH)[0], _spanned(t, t, LOW, HIGH)[0])


def test_a_span_wider_than_its_points_is_thinned_to_them_and_keeps_its_extremes(connection):
    low, high, points = 10.0, 40.0, 1000
    params = {'op': 'project.plot', 'params': {'data': 'Force', 'records': '1'},
              'span': {'low': low, 'high': high, 'points': points}}
    (series,) = _op(connection, params)['views'][0]['series']
    t, y = _time(), _force(1)
    inside = (t >= low) & (t <= high)
    x = np.asarray(series['x'])
    got = (x >= low) & (x <= high)
    assert 0.9 * points <= got.sum() <= points
    kept = np.asarray(series['y'])[got]
    assert kept.max() == y[inside].max() and kept.min() == y[inside].min()


def test_a_plot_shown_whole_is_marked_by_the_show_and_zooms_by_it(connection, validate):
    params = {'objects': ['Force'], 'records': {'Force': [0]}, 'prefs': {'flat': True}}
    whole = _show(connection, params)['views'][0]
    assert whole['span'] == {'method': 'project.show', 'params': params, 'view': 0}
    zoomed = _show(connection, {**params, 'span': {'low': LOW, 'high': HIGH, 'points': POINTS}})
    validate(_wire(zoomed['views'][0]), 'spectrumView')
    got_x, got_y = _in_span(zoomed['views'][0]['series'][0], LOW, HIGH)
    want_x, want_y = _spanned(_time(), _force(1), LOW, HIGH)
    np.testing.assert_array_equal(got_x, want_x)
    np.testing.assert_array_equal(got_y, want_y)


def test_a_computed_trace_zooms_too_as_rescale_previews_one(connection):
    params = {'op': 'project.rescale', 'params': {'source': 'Force', 'factor': 2.0,
                                                  'records': '1', 'keep': False}}
    whole = _op(connection, params)['views'][0]
    assert whole['span']['params'] == params
    zoomed = _op(connection, {**params, 'span': {'low': LOW, 'high': HIGH, 'points': POINTS}})
    recorded, rescaled = zoomed['views'][0]['series']
    want_x, want_y = _spanned(_time(), _force(1), LOW, HIGH)
    for series, factor in ((recorded, 1.0), (rescaled, 2.0)):
        got_x, got_y = _in_span(series, LOW, HIGH)
        np.testing.assert_array_equal(got_x, want_x)
        np.testing.assert_allclose(got_y, factor * want_y, rtol=1e-12)


def test_a_record_drawn_whole_is_not_marked(connection):
    connection.project.project.add('Short', TimeHistory(
        np.arange(500) / RATE, np.ones((1, 500)), ['3Z+'], ordinate_dim='force'))
    view = _op(connection, {'op': 'project.plot', 'params': {'data': 'Short'}})['views'][0]
    assert 'span' not in view


def _adding_op(edits: bool) -> Operation:
    def run(project, values):
        project.add('Added', TimeHistory(np.arange(10) / RATE, np.zeros((1, 10)), ['4Z+']))
        x, y = peak_decimate(_time(), _force(), 4000)
        return [v.spectrum('Added', v.axis('Time', 's', 'linear'),
                           v.axis('Force', 'N', 'linear'), [v.series('1Z+', 'measured', x, y)])]
    return Operation(id='test.adds', title='Adds', workspace='signals', summary='Adds one.',
                     params=(), outputs=('spectrum',), run=run, edits=edits)


def test_a_run_that_adds_an_object_is_not_marked_to_run_again(connection):
    connection.service.operations['test.adds'] = _adding_op(edits=False)
    view = _op(connection, {'op': 'test.adds', 'params': {}})['views'][0]
    assert 'Added' in connection.project.project and 'span' not in view


def test_an_operation_that_edits_is_refused_a_span_and_runs_nothing(connection):
    connection.service.operations['test.adds'] = _adding_op(edits=True)
    with pytest.raises(ProtocolError, match='changes the project') as error:
        _op(connection, {'op': 'test.adds', 'params': {},
                         'span': {'low': LOW, 'high': HIGH, 'points': POINTS}})
    assert error.value.code == 'bad_request' and 'Added' not in connection.project.project


def test_a_preview_is_marked_as_one_and_zooms_as_one(connection):
    def run(project, values):
        x, y = peak_decimate(_time(), _force(), 4000)
        return [v.spectrum('Preview', v.axis('Time', 's', 'linear'),
                           v.axis('Force', 'N', 'linear'), [v.series('1Z+', 'measured', x, y)])]
    connection.service.operations['test.previews'] = Operation(
        id='test.previews', title='Previews', workspace='signals', summary='Previews one.',
        params=(), outputs=('spectrum',), run=run, edits=True, previews=True)
    params = {'op': 'test.previews', 'params': {}, 'preview': True}
    (view,) = _op(connection, params)['views']
    assert view['span'] == {'method': 'op.run', 'params': params, 'view': 0}
    (zoomed,) = _op(connection, {**params, 'span': {'low': LOW, 'high': HIGH,
                                                    'points': POINTS}})['views']
    got_x, got_y = _in_span(zoomed['series'][0], LOW, HIGH)
    want_x, want_y = _spanned(_time(), _force(), LOW, HIGH)
    np.testing.assert_array_equal(got_x, want_x)
    np.testing.assert_array_equal(got_y, want_y)


@pytest.mark.parametrize(('span', 'said'), [
    ({'low': 0.5, 'high': 0.5, 'points': 100}, 'runs from a low x'),
    ({'low': 0.6, 'high': 0.5, 'points': 100}, 'runs from a low x'),
    ({'low': float('nan'), 'high': 0.5, 'points': 100}, 'runs from a low x'),
    ({'low': True, 'high': 0.5, 'points': 100}, 'runs from a low x'),
    ({'low': 0.1, 'high': 0.5, 'points': 1}, 'from 2 to 65536 points'),
    ({'low': 0.1, 'high': 0.5, 'points': 100.0}, 'from 2 to 65536 points'),
    ({'low': 0.1, 'high': 0.5, 'points': 70_000}, 'from 2 to 65536 points'),
    ({'low': 0.1, 'high': 0.5}, '"low", "high", "points"'),
    ([0.1, 0.5, 100], '"low", "high", "points"'),
])
def test_a_span_that_cannot_be_read_one_way_is_refused(connection, span, said):
    with pytest.raises(ProtocolError, match=said) as error:
        _op(connection, {'op': 'project.plot', 'params': {'data': 'Force'}, 'span': span})
    assert error.value.code == 'bad_request'
    with pytest.raises(ProtocolError, match=said):
        _show(connection, {'objects': ['Force'], 'prefs': {'flat': True}, 'span': span})


def test_the_window_keeps_the_two_samples_around_a_span_narrower_than_one():
    t = np.arange(10_000) / 100.0
    y = np.sin(t)
    with drawing_window(5.004, 5.006, 100):
        x, kept = peak_decimate(t, y, 200)
    at = np.searchsorted(x, 5.0)
    np.testing.assert_array_equal(x[at:at + 2], [5.0, 5.01])
    np.testing.assert_array_equal(kept[at:at + 2], np.sin([5.0, 5.01]))


def test_the_window_reaches_every_record_and_leaves_gaps_as_gaps():
    t = np.arange(50_000) / 1000.0
    rows = np.vstack([np.cos(t), np.sin(t)])
    rows[1, 20_100] = np.nan
    with drawing_window(20.0, 20.2, 1000):
        drawn = peak_decimate_rows(t, rows, 400)
    for (x, y), row in zip(drawn, rows, strict=True):
        span = (x >= 20.0) & (x <= 20.2)
        np.testing.assert_array_equal(x[span], t[20_000:20_201])
        np.testing.assert_array_equal(y[span], row[20_000:20_201])
    assert np.isnan(drawn[1][1][drawn[1][0] == t[20_100]]).all()
    # outside a window the thinning is as it was
    x, _y = peak_decimate_rows(t, rows, 400)[0]
    assert x.size <= 400


def test_a_window_that_cannot_be_drawn_is_refused():
    with pytest.raises(ValueError, match='low to high'), drawing_window(1.0, 1.0, 10):
        pass
    with pytest.raises(ValueError, match='at least 2'), drawing_window(0.0, 1.0, 1):
        pass


def test_the_span_goldens_validate_and_round_trip(validate):
    text = (GOLDEN / 'request_op_run_span.json').read_text(encoding='utf-8')
    request = frames.decode(text)
    validate(request)
    validate(request['params'], 'opRunParams')
    assert frames.pack(request) == json.dumps(request, separators=(',', ':'), ensure_ascii=False)
    response = frames.decode((GOLDEN / 'response_spectrum_span.bin').read_bytes())
    validate(_wire(response))
    (view,) = response['result']['views']
    validate(_wire(view), 'spectrumView')
    assert view['span']['params'] == {key: value for key, value in request['params'].items()
                                      if key != 'span'}
