"""Where the sine stage puts a tone's limits, zones and the rectangles past
an abort limit, read by value off the view the service sends."""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core.sine import (
    LINEAR,
    SineLevel,
    SineLevelSet,
    SineSweepSpecification,
    SineTone,
)
from hertzforge.project import Project
from hertzforge.service.show import show
from hertzforge.service.sine_stage import _edges, sine_stage

from .conftest import wire_views

DOFS = ['1Z+', '2Z+']
#: channel 2 sits at twice channel 1's level and limits
LEVEL = np.array([[10.0, 20.0], [10.0, 20.0]])


def _tone(**limits):
    """100 to 300 Hz at 50 Hz/s, four seconds, starting 2 s into the run."""
    return SineTone('T', 2.0, [100.0, 300.0], LEVEL, [LINEAR], [50.0], **limits)


def _all_limits():
    return {'warning_lower': LEVEL * 0.8, 'warning_upper': LEVEL * 1.2,
            'abort_lower': LEVEL * 0.5, 'abort_upper': LEVEL * 1.5}


def _spec(**limits):
    return SineSweepSpecification([_tone(**limits)], DOFS, ordinate_unit='m/s**2')


def _level(first=(10.0, 16.0, 10.0, 4.0, 10.0), dofs=None, onset=1.0, tone='T'):
    """Five lines a second apart from `onset`. By default channel 1 is past
    abort upper (15) on the second line and past abort lower (5) on the
    fourth; channel 2 stays inside its limits."""
    dofs = dofs or DOFS
    rows = [first, [20.0] * 5][:len(dofs)]
    return SineLevel(np.linspace(100.0, 300.0, 5), np.array(rows), response_dof=dofs,
                     ordinate_dim=['acceleration'] * len(dofs),
                     ordinate_unit=['m/s**2'] * len(dofs),
                     tone=tone, onset=onset, seconds=onset + np.arange(5.0))


def _stations(stage):
    """{label: (x, time, amplitude)} of a stage's records."""
    offsets = stage['offsets']
    return {label: (stage['x_values'][offsets[k]:offsets[k + 1]],
                    stage['y_values'][offsets[k]:offsets[k + 1]],
                    stage['z_values'][offsets[k]:offsets[k + 1]])
            for k, label in enumerate(stage['records'])}


def test_a_tones_limits_follow_its_path_through_time():
    stage = sine_stage(_spec(**_all_limits()), [], 'T', ['1Z+'], lines=8)
    assert stage['records'] == ['T requirement', 'T warning lower', 'T warning upper',
                                'T abort lower', 'T abort upper']
    assert stage['roles'] == ['reference', 'tolerance', 'tolerance', 'abort', 'abort']
    drawn = _stations(stage)
    for label, level in (('T requirement', 10.0), ('T warning lower', 8.0),
                         ('T warning upper', 12.0), ('T abort lower', 5.0),
                         ('T abort upper', 15.0)):
        x, t, z = drawn[label]
        assert len(x) == 9, 'eight steps along the path'
        np.testing.assert_allclose(x, np.linspace(100.0, 300.0, 9))
        np.testing.assert_allclose(t, np.linspace(2.0, 6.0, 9), err_msg="on the tone's clock")
        np.testing.assert_allclose(z, level)
    assert stage['x_range'] == [100.0, 300.0] and stage['y_range'] == [2.0, 6.0]
    assert stage['z_range'] == [0.0, 15.0]


def test_the_zones_run_from_each_warning_to_its_abort_and_past_the_abort():
    stage = sine_stage(_spec(**_all_limits()), [], 'T', ['1Z+'], lines=8)
    found = {(z['side'], z['kind']): z for z in stage['zones']}
    assert sorted(found) == [('lower', 'abort'), ('lower', 'warning'),
                             ('upper', 'abort'), ('upper', 'warning')]
    assert all(z['station'] == 0 for z in stage['zones'])
    for key, inner, outer in ((('upper', 'warning'), 12.0, 15.0),
                              (('lower', 'warning'), 8.0, 5.0),
                              (('upper', 'abort'), 15.0, None),
                              (('lower', 'abort'), 5.0, None)):
        zone = found[key]
        np.testing.assert_allclose(zone['inner'], inner)
        np.testing.assert_allclose(zone['x'], np.linspace(100.0, 300.0, 9))
        np.testing.assert_allclose(zone['y'], np.linspace(2.0, 6.0, 9))
        if outer is None:
            assert 'outer' not in zone
        else:
            np.testing.assert_allclose(zone['outer'], outer)


def test_a_warning_with_no_abort_has_a_zone_with_no_outer_edge():
    stage = sine_stage(_spec(warning_upper=LEVEL * 1.2), [], 'T', ['1Z+'], lines=8)
    (zone,) = stage['zones']
    assert (zone['side'], zone['kind']) == ('upper', 'warning') and 'outer' not in zone
    assert stage['roles'] == ['reference', 'tolerance']


def test_an_abort_with_no_warning_has_its_zone_alone():
    stage = sine_stage(_spec(abort_lower=LEVEL * 0.5), [], 'T', ['1Z+'], lines=8)
    (zone,) = stage['zones']
    assert (zone['side'], zone['kind']) == ('lower', 'abort')
    assert stage['roles'] == ['reference', 'abort']


def test_a_limit_with_no_number_anywhere_is_not_drawn():
    limits = {'abort_upper': np.full_like(LEVEL, np.nan), 'warning_upper': LEVEL * 1.2}
    stage = sine_stage(_spec(**limits), [], 'T', ['1Z+'], lines=8)
    assert stage['records'] == ['T requirement', 'T warning upper']
    assert [z['kind'] for z in stage['zones']] == ['warning']


def test_a_rectangle_covers_each_line_past_an_abort_limit_and_stands_on_it():
    stage = sine_stage(_spec(**_all_limits()), [_level()], 'T', ['1Z+'])
    station = stage['records'].index('T level')
    found = {rect['side']: rect for rect in stage['exceed']}
    assert sorted(found) == ['lower', 'upper']
    assert found['upper']['station'] == found['lower']['station'] == station
    upper, lower = found['upper'], found['lower']
    # the second line, 150 Hz, 1 s into the tone, on the tone's clock from 2 s;
    # its bin reaches halfway to each neighbor
    np.testing.assert_allclose(upper['x0'], [125.0])
    np.testing.assert_allclose(upper['x1'], [175.0])
    np.testing.assert_allclose(upper['y0'], [2.5])
    np.testing.assert_allclose(upper['y1'], [3.5])
    np.testing.assert_allclose(upper['z'], [15.0])
    # the fourth line, 250 Hz, 3 s in
    np.testing.assert_allclose(lower['x0'], [225.0])
    np.testing.assert_allclose(lower['x1'], [275.0])
    np.testing.assert_allclose(lower['y0'], [4.5])
    np.testing.assert_allclose(lower['y1'], [5.5])
    np.testing.assert_allclose(lower['z'], [5.0])


def test_a_line_exactly_on_an_abort_limit_or_with_no_number_has_no_rectangle():
    on_the_limits = _level((15.0, 5.0, 10.0, np.nan, 10.0))
    stage = sine_stage(_spec(**_all_limits()), [on_the_limits], 'T', ['1Z+'])
    assert 'exceed' not in stage


def test_each_channel_is_read_against_its_own_limits():
    stage = sine_stage(_spec(**_all_limits()), [_level()], 'T')
    assert stage['records'][:5] == ['T requirement, 1Z+', 'T warning lower, 1Z+',
                                    'T warning upper, 1Z+', 'T abort lower, 1Z+',
                                    'T abort upper, 1Z+']
    drawn = _stations(stage)
    np.testing.assert_allclose(drawn['T abort upper, 2Z+'][2], 30.0)
    np.testing.assert_allclose(drawn['T level, 2Z+'][2], 20.0)
    assert {z['station'] for z in stage['zones']} == {
        stage['records'].index('T requirement, 1Z+'),
        stage['records'].index('T requirement, 2Z+')}
    # channel 1 is out twice; channel 2 at 20 is inside its 10 to 30
    assert {rect['station'] for rect in stage['exceed']} == {
        stage['records'].index('T level, 1Z+')}


def test_a_level_is_placed_on_the_tones_clock_with_its_own_onset():
    stage = sine_stage(_spec(), [_level(onset=7.5)], 'T', ['1Z+'])
    x, t, z = _stations(stage)['T level']
    np.testing.assert_allclose(t, [2.0, 3.0, 4.0, 5.0, 6.0])
    np.testing.assert_allclose(x, np.linspace(100.0, 300.0, 5))
    np.testing.assert_allclose(z, [10.0, 16.0, 10.0, 4.0, 10.0])


def test_a_level_alone_stays_on_its_recording_clock_and_names_its_unit():
    stage = sine_stage(None, [_level(onset=7.5)], 'T', ['1Z+'])
    assert stage['records'] == ['T level'] and stage['roles'] == ['measured']
    np.testing.assert_allclose(_stations(stage)['T level'][1], 7.5 + np.arange(5.0))
    assert stage['z']['unit'] == 'm/s²'
    assert 'zones' not in stage and 'exceed' not in stage


def test_a_channel_the_specification_does_not_hold_is_drawn_and_not_judged():
    stage = sine_stage(_spec(**_all_limits()), [_level(dofs=['9Z+'])], 'T')
    assert 'T level, 9Z+' in stage['records']
    assert 'T requirement, 1Z+' in stage['records'], 'and every channel it does hold'
    assert 'exceed' not in stage


def test_naming_a_channel_draws_that_channel_alone():
    stage = sine_stage(_spec(**_all_limits()), [_level()], 'T', ['2Z+'])
    assert 'T requirement' in stage['records'], 'one channel carries no suffix'
    np.testing.assert_allclose(_stations(stage)['T requirement'][2], 20.0)
    assert 'exceed' not in stage


def test_nothing_to_draw_is_none():
    assert sine_stage(None, [], 'T') is None
    assert sine_stage(_spec(), [], 'Other') is None
    assert sine_stage(None, [_level(tone='Other')], 'T') is None
    no_clock = _level()
    no_clock.seconds = None
    assert sine_stage(None, [no_clock], 'T') is None


def test_the_amplitude_range_starts_at_zero_or_below():
    assert sine_stage(_spec(), [], 'T', ['2Z+'])['z_range'] == [0.0, 20.0]
    empty = _level((np.nan,) * 5)
    assert sine_stage(None, [empty], 'T', ['1Z+'])['z_range'] == [0.0, 1.0]


def test_a_specification_with_no_unit_leaves_the_axis_unnamed():
    spec = SineSweepSpecification([_tone()], DOFS)
    assert sine_stage(spec, [], 'T', ['1Z+'])['z']['unit'] is None


def test_each_pass_of_an_up_and_back_tone_has_its_own_limits_when_it_starts_late():
    """The limit is read on the pass nearest the time from the tone's own
    start, so a tone starting 2 s into the run reads the same as one at 0."""
    amplitude = np.array([[10.0], [10.0], [2.5]])
    tone = SineTone('T', 2.0, [100.0, 300.0, 100.0], amplitude, [LINEAR, LINEAR], [50.0, -50.0],
                    abort_upper=amplitude * 1.5)
    spec = SineSweepSpecification([tone], ['1Z+'], ordinate_unit='m/s**2')
    drawn = _stations(sine_stage(spec, [], 'T', lines=800))
    for label, scale in (('T requirement', 1.0), ('T abort upper', 1.5)):
        _x, t, z = drawn[label]
        assert z[np.argmin(np.abs(t - 3.0))] == pytest.approx(10.0 * scale)
        assert z[np.argmin(np.abs(t - 9.0))] == pytest.approx(4.375 * scale, rel=1e-2)


def test_bin_edges_are_midpoints_with_the_ends_extended_half_a_bin():
    np.testing.assert_allclose(_edges(np.array([1.0, 2.0, 4.0])), [0.5, 1.5, 3.0, 5.0])
    np.testing.assert_allclose(_edges(np.array([3.0])), [3.0, 3.0])


def test_the_stage_reaches_the_wire_with_its_zones_and_rectangles(validate):
    project = Project()
    project.add('S', _spec(**_all_limits()))
    project.add('Sine Levels', SineLevelSet([_level()]))
    (stage,) = show(project, ['S', 'Sine Levels'], {}, {})['views']
    assert len(stage['zones']) == 8 and len(stage['exceed']) == 2
    assert stage['zones'][0]['inner'].dtype == np.float32
    assert stage['exceed'][0]['x0'].dtype == np.float32
    assert stage['title'].startswith('T: 1 level against the requirement of S')
    wire_views([stage], validate)


def test_a_path_with_one_depth_short_is_refused():
    from hertzforge.service import views as v

    view = v.waterfall('t', v.axis('x', 'Hz', 'linear'), v.axis('z', None, 'linear'), ['a'],
                       [(np.arange(3.0), np.arange(3.0))], (0.0, 2.0))
    with pytest.raises(ValueError, match='one depth a point'):
        v.pathed(view, v.axis('y', 's', 'linear'), np.arange(2.0), (0.0, 1.0), ['measured'])
