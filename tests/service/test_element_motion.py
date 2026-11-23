"""Modal coordinates played with an element shape set's elements colored: the service sends
each played mode's stress tensor at each element once, in the order the clip plays the modes,
and the top of the color scale, the clip's largest value of the scalar formed from the summed
tensor or a stated one. The shell sums and forms the scalar each frame (`viz/stress.ts`)."""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core.data import TimeHistory
from hertzforge.core.stress_scalars import modal_stress, von_mises
from hertzforge.service import display_units, projects
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views
from .test_element_shapes_op import _run, _state

PLAY = 'project.play_element_shapes'
SAMPLES = 40


def _clip(state, modes=(8, 6), dim='modal_length'):
    """Modes 9 and 7 of 'Cells Modes' (the elastic ones, a rigid one first in neither) played
    in that order, each a sine of its own rate, with strain and stress read at modes 7 to 9."""
    project = state.project
    added = project.element_shapes('Cells Modes', modes=[6, 7, 8])
    time = np.arange(SAMPLES) / 1000.0
    rows = np.vstack([(1e-3 / (k + 1)) * np.sin(2 * np.pi * 37.0 * (k + 1) * time)
                      for k in range(len(modes))])
    project.add('Clip', TimeHistory(time, rows, [f'M{m + 1}' for m in modes], ordinate_dim=dim))
    return added


def _stage(out):
    return next(v for v in out['views'] if v['view'] == 'shape3d')


def test_the_clip_plays_with_each_played_modes_tensors_and_the_clips_largest_von_mises(validate):
    state = _state()
    added = _clip(state)
    out = projects.show(state, {'objects': ['Cells', 'Cells Modes', 'Clip', added],
                                'records': {}, 'prefs': {}})
    wire_views(out['views'], validate)
    stage = _stage(out)
    held = stage['element_values']
    read = state.project[added]
    assert held['scalar'] == 'von_mises' and held['label'] == 'Von Mises stress'
    assert held['unit'] == 'Pa' and held['quantity'] == 'stress'
    # the clip plays mode 9 then mode 7: rows 2 and 0 of the set read at modes 7, 8, 9
    centroid = read.stress[:, read.point == 0]
    np.testing.assert_allclose(held['shapes'], centroid[[2, 0]], rtol=1e-6)
    assert stage['motion']['values'].shape == (2, SAMPLES)
    # one element at one instant by hand: q9 S9 + q7 S7, and von Mises of that sum
    k, e = 13, 3
    q = state.project['Clip'].ordinate[:, k]
    by_hand = q[0] * centroid[2, e] + q[1] * centroid[0, e]
    np.testing.assert_allclose(modal_stress(held['shapes'], stage['motion']['values'][:, k])[e],
                               by_hand, rtol=1e-5)
    # the scale's top is the largest von Mises over every element and sample of the clip
    every = von_mises(np.einsum('ms,mec->sec', state.project['Clip'].ordinate, centroid[[2, 0]]))
    assert held['max'] == pytest.approx([float(every.max())], rel=1e-6)
    assert 'stated' not in held
    assert held['triangles'].max() < len(stage['nodes'])
    assert 'colored by von Mises stress' in out['note']


def test_a_stated_scale_is_the_top_and_reads_in_the_display_systems_pressure():
    state = _state()
    added = _clip(state)
    views = _run(state, PLAY, {'object': added, 'data': 'Clip', 'quantity': 'stress',
                               'component': 'von_mises', 'color_max': 2.5e6})
    held = _stage({'views': views})['element_values']
    assert held['max'] == [2.5e6] and held['stated'] is True
    display_units.convert(views, 'mm-kg-N-s')
    assert held['unit'] == 'MPa' and held['max'] == [pytest.approx(2.5)]


def test_the_colored_elements_are_those_of_the_groups_named():
    state = _state()
    added = _clip(state)
    every = _stage({'views': _run(state, PLAY, {'object': added, 'data': 'Clip'})})
    wedges = _stage({'views': _run(state, PLAY, {'object': added, 'data': 'Clip',
                                                 'color_groups': '2'})})
    # four cells along: three of bricks and one of two wedges
    assert every['element_values']['elements'] == 5
    assert wedges['element_values']['elements'] == 2
    assert wedges['element_values']['shapes'].shape[1] == 2
    assert wedges['element_values']['owner'].max() == 1


def test_a_principal_stress_scale_runs_over_its_largest_magnitude():
    state = _state()
    added = _clip(state)
    views = _run(state, PLAY, {'object': added, 'data': 'Clip', 'component': 'min_principal'})
    held = _stage({'views': views})['element_values']
    from hertzforge.core.stress_scalars import principal_stresses
    read = state.project[added]
    tensors = np.einsum('ms,mec->sec', state.project['Clip'].ordinate,
                        read.stress[:, read.point == 0][[2, 0]])
    assert held['max'][0] == pytest.approx(float(np.abs(principal_stresses(tensors)[..., 2]).max()),
                                           rel=1e-6)


@pytest.mark.parametrize(('fill', 'said'), [
    ({'quantity': 'strain', 'component': 'von_mises'}, 'von mises is a stress; strain is colored'),
    ({'quantity': 'strain', 'component': 'xx', 'color_max': 1.0},
     'a stated color scale is a stress'),
    ({'color_groups': '7'}, r'read no element of group 7; it read groups 1, 2'),
])
def test_a_coloring_that_cannot_be_read_one_way_is_refused(fill, said):
    state = _state()
    added = _clip(state)
    with pytest.raises(ProtocolError, match=said):
        _run(state, PLAY, {'object': added, 'data': 'Clip', **fill})


def test_a_clip_of_modal_velocity_is_refused():
    state = _state()
    added = _clip(state, dim='modal_velocity')
    with pytest.raises(ProtocolError, match='strain and stress follow modal displacement, and '
                                            'Clip holds modal velocity'):
        _run(state, PLAY, {'object': added, 'data': 'Clip'})


def test_a_played_mode_the_set_did_not_read_is_refused_by_number():
    state = _state()
    project = state.project
    added = project.element_shapes('Cells Modes', modes=[6, 7])
    project.add('Clip', TimeHistory(np.arange(4) / 100.0, np.ones((1, 4)), ['M9'],
                                    ordinate_dim='modal_length'))
    with pytest.raises(ProtocolError, match='holds no strain or stress for mode 9 of Cells Modes'):
        _run(state, PLAY, {'object': added, 'data': 'Clip'})


def test_four_objects_that_are_not_a_clip_and_an_element_shape_set_show_as_before():
    """The fourth object of a played clip must be an element shape set: anything else falls to
    the readings four objects otherwise show."""
    state = _state()
    _clip(state)
    state.project.add('Other', TimeHistory(np.arange(4) / 100.0, np.ones((1, 4)), ['1X+']))
    out = projects.show(state, {'objects': ['Cells', 'Cells Modes', 'Clip', 'Other'],
                                'records': {}, 'prefs': {}})
    assert not any(v.get('element_values') for v in out['views'])
