"""Links and compatibility in the summary: the object groups the tree
brackets, and the objects that do not fit what they answer to, marked
red as Visual Dynamics marks them and never refused."""

from __future__ import annotations

from pathlib import Path

import numpy as np

from hertzforge.core.data import TimeHistory
from hertzforge.service import projects
from hertzforge.service.ops_geometry import OPERATIONS

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in OPERATIONS}


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()})
    return op.run(project, values)


def test_the_summary_brackets_the_groups_and_marks_what_does_not_fit(validate):
    state, added = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    projects.import_file(state, {'path': str(PLATE / 'geometry.unv'),
                                 'units': {'length_unit': 'mm'}})
    summary = projects.summary(state)
    validate(summary, 'project')
    (group,) = summary['links']
    assert set(group['members']) == set(added), 'the run\'s objects arrive linked'
    assert group['role'] is None and group['name'] == ''
    assert all('issue' not in o for o in summary['objects']), 'the plate data fits its geometry'

    project = state.project
    project.add('Stray', TimeHistory(abscissa=np.linspace(0, 1, 8), ordinate=np.zeros((2, 8)),
                                     response_dof=['101Z+', '999999Z+'],
                                     ordinate_unit=['m/s**2'] * 2))
    summary = projects.summary(state)
    validate(summary, 'project')
    stray = next(o for o in summary['objects'] if o['name'] == 'Stray')
    assert '999999' in stray['issue']['message'] and stray['issue']['sub_items'] == [1]
    assert stray['groups'] == [], 'in no group, judged against the active geometry'

    run(project, 'project.set_basis', object=added[0])
    summary = projects.summary(state)
    assert summary['links'][0]['role'] == 'Basis'
    run(project, 'project.name_object_group', object=added[0], name='Survey')
    assert projects.summary(state)['links'][0]['name'] == 'Survey'
    run(project, 'project.link', first='Stray', second=added[0], more='', name='')
    summary = projects.summary(state)
    assert 'Stray' in summary['links'][0]['members'], 'linked, and still marked'
    assert 'issue' in next(o for o in summary['objects'] if o['name'] == 'Stray')


def test_a_project_with_no_groups_has_no_links():
    state = projects.new({'title': 'Empty'})
    assert projects.summary(state)['links'] == []
