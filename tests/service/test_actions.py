"""The actions a selection can take (`project.actions`): only what applies
to what is selected, as Visual Dynamics' bar offers its acts, with the
selection filled into the operation's params."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

from hertzforge.core.data import Psd, Specification
from hertzforge.core.shapes import ShapeSet
from hertzforge.project import Project
from hertzforge.service import actions, projects
from hertzforge.service.protocol import ProtocolError

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'


@pytest.fixture(scope='module')
def plate():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    projects.import_file(state, {'path': str(PLATE / 'modal_spectra.nc4')})
    projects.import_file(state, {'path': str(PLATE / 'geometry.unv'),
                                 'units': {'length_unit': 'mm'}})
    return state


def named(state, kind):
    return next(name for name, obj in state.project.items() if projects.kind_of(obj) == kind)


def offered(state, *objects, records=None, validate=None):
    out = projects.actions(state, {'objects': list(objects), 'records': records or {}})
    if validate:
        validate(out, 'actions')
    return out


def ids(entries):
    return {e['id'] for e in entries}


def test_a_geometry_alone_is_offered_what_a_geometry_takes(plate, validate):
    geometry = named(plate, 'geometry')
    out = offered(plate, geometry, validate=validate)
    assert 'project.view_geometry' in ids(out['readings'])
    assert 'project.element_groups' in ids(out['readings'])
    assert 'project.solve_modes' not in ids(out['acts']), 'no element group properties yet'
    assert 'project.mesh_solids' in ids(out['acts'])
    assert 'project.spec_spectra' not in ids(out['readings']), 'that takes a PSD and a specification'
    assert all(e['fill'] == {next(iter(e['fill']))
                             : geometry} for e in out['readings'] if len(e['fill']) == 1)
    assert out['project'] == []


def test_an_frf_alone_and_its_picked_records(plate, validate):
    frf = named(plate, 'frf')
    out = offered(plate, frf, validate=validate)
    reading_ids = ids(out['readings'])
    assert {'project.plot', 'project.waterfall', 'project.cmif', 'project.drive_points'} <= reading_ids
    assert 'project.view_geometry' not in reading_ids
    plot = next(e for e in out['readings'] if e['id'] == 'project.plot')
    assert plot['fill'] == {'data': frf}
    picked = offered(plate, frf, records={frf: [0, 1, 2, 6]})
    plot = next(e for e in picked['readings'] if e['id'] == 'project.plot')
    assert plot['fill'] == {'data': frf, 'records': '1-3, 7'}, 'the picks, as the shell numbers them'
    assert 'project.fit_modes' in ids(out['acts'])


def test_a_pair_is_offered_what_the_pair_takes_and_nothing_for_one_of_them(plate, validate):
    geometry, table = named(plate, 'geometry'), named(plate, 'channel_table')
    out = offered(plate, geometry, table, validate=validate)
    assert 'project.view_geometry' not in ids(out['readings']), 'the table would go unused'
    arrows = next(e for e in out['readings'] if e['id'] == 'project.dof_arrows')
    assert arrows['fill'] == {'source': table}, 'the geometry is the stage, not a param'
    with_frf = offered(plate, geometry, named(plate, 'frf'))
    assert 'project.plot' in ids(with_frf['readings']) and 'project.view_geometry' not in ids(
        with_frf['readings'])
    project = plate.project
    f = np.linspace(20, 2000, 50)
    project.add('Measured', Psd(abscissa=f, ordinate=np.full((2, 50), 0.01),
                                response_dof=['1Z+', '2Z+'], ordinate_unit=['g**2/Hz'] * 2))
    project.add('Spec', Specification(abscissa=f, ordinate=np.full((2, 50), 0.01),
                                      response_dof=['1Z+', '2Z+'], ordinate_unit=['g**2/Hz'] * 2))
    try:
        pair = offered(plate, 'Spec', 'Measured', validate=validate)
        spectra = next(e for e in pair['readings'] if e['id'] == 'project.spec_spectra')
        assert spectra['fill'] == {'spectrum': 'Measured', 'specification': 'Spec'}, (
            'each object on the param that takes it, whichever was picked first')
        assert 'project.psd_overlay' not in ids(pair['readings']), (
            'two PSDs, neither a specification, as VD has it')
        assert 'project.plot' not in ids(pair['readings'])
    finally:
        project.remove('Measured')
        project.remove('Spec')


def test_two_shape_sets_are_offered_their_mac_and_matching(plate, validate):
    project = plate.project
    shapes = ShapeSet(frequency=[10.0, 20.0], damping=[0.01, 0.01], shape_matrix=np.eye(2),
                      coordinate=['1Z+', '2Z+'])
    project.add('A', shapes)
    project.add('B', shapes)
    try:
        out = offered(plate, 'A', 'B', validate=validate)
        assert 'project.mac' in ids(out['readings'])
        assert 'project.match_modes' in ids(out['acts'])
        assert 'project.merge' in ids(out['acts'])
    finally:
        project.remove('A')
        project.remove('B')


def test_nothing_selected_offers_what_takes_no_object(plate, validate):
    out = offered(plate, validate=validate)
    assert out['readings'] == [] and out['acts'] == []
    assert {'project.new_geometry', 'project.new_channel_table'} <= ids(out['project'])
    assert all(e['fill'] == {} for e in out['project'])


def test_acts_are_the_journaled_verbs_and_the_edits(plate):
    history = named(plate, 'time_history')
    out = offered(plate, history)
    assert {'project.compute_psds', 'project.compute_frfs', 'project.integrate'} <= ids(out['acts'])
    assert {'project.waterfall', 'project.kurtosis'} <= ids(out['readings'])
    assert not ids(out['acts']) & ids(out['readings'])


def test_unknown_objects_and_an_empty_project_are_refused(plate):
    with pytest.raises(ProtocolError, match='no object named'):
        offered(plate, 'nope')
    with pytest.raises(ProtocolError, match='none is open'):
        projects.actions(None, {'objects': []})
    with pytest.raises(ProtocolError, match='"objects"'):
        projects.actions(plate, {'objects': 'FRF'})
    state = projects.OpenProject(project=Project())
    assert offered(state)['readings'] == []


def test_the_fill_places_each_object_on_a_distinct_param():
    from hertzforge.service.descriptors import Operation, Param

    op = Operation(id='x.y', title='Y', workspace='signals', accepts='project', summary='',
                   params=(Param('first', 'First', 'enum', options_from='objects:psd'),
                           Param('second', 'Second', 'enum', options_from='objects:psd'),
                           Param('extra', 'Extra', 'enum', default='',
                                 options_from='objects:geometry')),
                   outputs=('reading',), run=lambda p, v: [])
    chosen = [('A', 'psd', []), ('B', 'psd', [])]
    assert actions._fill(op, chosen) == {'first': 'A', 'second': 'B'}
    assert actions._fill(op, chosen[:1]) is None, 'the second PSD is required'
    assert actions._fill(op, [*chosen, ('G', 'geometry', [])]) == {'first': 'A', 'second': 'B',
                                                                    'extra': 'G'}
    assert actions._fill(op, [('G', 'geometry', [])]) is None
