"""Which operations a selection fills (`project.actions`), each object on a
param that takes it. The table it answers with has its own tests,
test_action_table.py."""

from __future__ import annotations

from pathlib import Path

import pytest

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


def test_unknown_objects_and_an_empty_project_are_refused(plate):
    with pytest.raises(ProtocolError, match='no object named'):
        offered(plate, 'nope')
    with pytest.raises(ProtocolError, match='none is open'):
        projects.actions(None, {'objects': []})
    with pytest.raises(ProtocolError, match='"objects"'):
        projects.actions(plate, {'objects': 'FRF'})
    state = projects.OpenProject(project=Project())
    assert len(offered(state)['table']['columns']) == 1


def test_the_fill_places_each_object_on_a_distinct_param():
    from hertzforge.service.descriptors import Operation, Param

    op = Operation(id='x.y', title='Y', workspace='signals', summary='',
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
