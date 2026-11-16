"""Modal FRFs, a channel table with a force reference and a geometry selected together
show the operating deflection shape of a force at that reference (`show._driven`)."""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.shapes import ShapeSet
from hertzforge.deform import modal_ods_deflection
from hertzforge.demo import plate
from hertzforge.service import projects
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views


def _table(nodes=('101',), roles=('reference',), kinds=('force',)):
    count = len(nodes)
    return hertzforge.ChannelTable({
        'channel': list(range(1, count + 1)), 'node': list(nodes),
        'direction': ['Z+'] * count, 'role': list(roles),
        'unit': ['N' if kind == 'force' else 'g' for kind in kinds],
        'channel_type': list(kinds)})


@pytest.fixture(scope='module')
def state():
    """A free plate with its first twenty modes, six of them rigid, its modal FRFs in
    displacement, and three channel tables: one force, two forces, no force."""
    built = plate.build(4)
    solved = built.eigensolution(num_modes=20)
    shapes = ShapeSet(solved.frequency, np.full(solved.num_shapes, 0.02), solved.coordinate,
                      solved.shape_matrix, modal_mass=solved.modal_mass, mass_unit='kg')
    held = projects.new({'title': 'Plate'})
    project = held.project
    project.add('Plate', built.geometry())
    project.add('Modes', shapes)
    project.link('Plate', 'Modes')
    project.synthesize_modal_frfs('Modes', sample_rate=4096.0, frame_length=64,
                                  quantity='displacement', name='Modal FRFs')
    project.add('Force', _table())
    project.add('Two', _table(('101', '505'), ('reference',) * 2, ('force',) * 2))
    project.add('Quiet', _table(roles=('response',), kinds=('acceleration',)))
    project.add('Other Modes', ShapeSet(shapes.frequency, shapes.damping, shapes.coordinate,
                                        shapes.shape_matrix))
    return held


def shown(state, *objects, records=None, prefs=None, validate=None):
    out = projects.show(state, {'objects': list(objects), 'records': records or {},
                                'part': None, **({'prefs': prefs} if prefs else {})})
    if validate:
        wire_views(out['views'], validate)
        validate({**out, 'views': []}, 'shown')
    return out


def _elastic(state):
    frequency = state.project['Modes'].frequency
    return [k for k in range(len(frequency)) if frequency[k] > 1.0]


def test_modal_frfs_a_force_and_a_geometry_show_the_response_to_that_force(state, validate):
    out = shown(state, 'Modal FRFs', 'Force', 'Plate', validate=validate)
    stage, *curves = out['views']
    assert stage['view'] == 'shape3d' and stage['cursor'] == 'frequency'
    assert all(curve['view'] == 'spectrum' and curve['link'] == 'frequency' for curve in curves)
    assert 'the response to a force at 101Z+: 20 mode shapes of Modes' in out['note']
    # the one arrow is the force, pointing in at its node
    arrows = stage['arrows']
    assert arrows['labels'] == ['101Z+'] and list(arrows['inward']) == [1]
    assert list(arrows['axes']) == [2]
    # nothing else is offered to be marked, and one force leaves no choice of drive
    assert out['offers']['drives'] == ['101Z+']
    assert not out['offers']['arrows'] and out['offers']['arrow_roles'] == []
    # what is drawn is the deflection the project's own calls make, at its strongest line
    project = state.project
    basis, driven = project.driven_modes('Modal FRFs', table='Force')
    deflection = modal_ods_deflection(project['Plate'], project[basis], driven)
    deflection.line = deflection.strongest_line
    frequency = float(np.asarray(project['Modal FRFs'].abscissa)[deflection.line])
    mode = stage['modes'][0]
    assert mode['frequency_hz'] == pytest.approx(frequency)
    assert mode['label'] == f'ODS at {frequency:.4g} Hz'
    drawn = np.asarray(mode['shape']) + 1j * np.asarray(mode.get('shape_imag', 0.0))
    at_zero = deflection.offsets(0.0).copy()
    expected = at_zero - 1j * deflection.offsets(np.pi / 2)
    assert np.abs(drawn.real).max() > np.abs(drawn.imag).max(), 'fullest at phase zero'
    assert np.abs(drawn).max() == pytest.approx(np.abs(expected).max())
    assert np.abs(drawn).max() == pytest.approx(1.0, abs=1e-6), 'each line at full scale'


def test_the_elastic_records_show_the_static_shape_at_zero_hertz(state, validate):
    elastic = _elastic(state)
    whole = shown(state, 'Modal FRFs', 'Force', 'Plate', prefs={'line': 0.0}, validate=validate)
    label = whole['views'][0]['modes'][0]['label']
    assert label == 'ODS at 0 Hz: no shape, a mode is unbounded here'
    assert not np.any(whole['views'][0]['modes'][0]['shape'])
    assert 'no shape at 0 Hz' in whole['note']
    out = shown(state, 'Modal FRFs', 'Force', 'Plate', records={'Modal FRFs': elastic},
                prefs={'line': 0.0}, validate=validate)
    mode = out['views'][0]['modes'][0]
    assert mode['label'] == 'ODS at 0 Hz'
    assert np.abs(np.asarray(mode['shape'])).max() == pytest.approx(1.0, abs=1e-6)
    assert f'{len(elastic)} mode shapes of Modes' in out['note']
    # only the modes kept are plotted beside it
    assert sum(len(curve['series']) for curve in out['views'][1:]) == len(elastic)


def test_several_force_references_offer_the_drive(state, validate):
    first = shown(state, 'Modal FRFs', 'Two', 'Plate', validate=validate)
    assert first['offers']['drives'] == ['101Z+', '505Z+']
    assert first['views'][0]['arrows']['labels'] == ['101Z+']
    other = shown(state, 'Modal FRFs', 'Two', 'Plate', prefs={'drive': '505Z+'},
                  validate=validate)
    assert other['views'][0]['arrows']['labels'] == ['505Z+']
    assert 'a force at 505Z+' in other['note']
    assert not np.allclose(first['views'][0]['modes'][0]['shape'],
                           other['views'][0]['modes'][0]['shape'])
    # a drive the table does not hold is the first
    lost = shown(state, 'Modal FRFs', 'Two', 'Plate', prefs={'drive': '999Z+'})
    assert lost['views'][0]['arrows']['labels'] == ['101Z+']
    with pytest.raises(ProtocolError, match='"drive"'):
        shown(state, 'Modal FRFs', 'Two', 'Plate', prefs={'drive': 5})


def test_without_a_force_it_says_what_is_missing(state):
    alone = shown(state, 'Modal FRFs', 'Plate')
    assert 'select a channel table with a force reference too' in alone['note']
    assert alone['offers']['drives'] == []
    quiet = shown(state, 'Modal FRFs', 'Quiet', 'Plate')
    assert 'Quiet has no reference channel measuring force' in quiet['note']
    assert not any(view['view'] == 'shape3d' and view.get('modes') for view in quiet['views'])


def test_a_shape_set_selected_with_them_has_to_be_theirs(state, validate):
    theirs = shown(state, 'Modal FRFs', 'Force', 'Plate', 'Modes', validate=validate)
    assert theirs['offers']['drives'] == ['101Z+']
    assert 'the response to a force at 101Z+' in theirs['note']
    other = shown(state, 'Modal FRFs', 'Force', 'Plate', 'Other Modes')
    assert other['offers']['drives'] == []
    assert 'the response to a force' not in other['note']
