"""Dragging the project or an object out of the window (Brandon, 2026-11-11: "if I want
anything, it is the ability to drag the project out to save it"). The service writes a copy
the shell hands the operating system; the project is left as it was."""

from __future__ import annotations

import os

import numpy as np
import pytest

import hertzforge
from hertzforge.core.data import TimeHistory
from hertzforge.service import projects
from hertzforge.service.protocol import ProtocolError


@pytest.fixture
def state(tmp_path):
    project = hertzforge.Project('Shaker: run 3/4')
    project.add('Run', TimeHistory(np.arange(8.0) / 8, np.ones((1, 8)), response_dof=['1X+'],
                                   ordinate_dim=['acceleration']))
    own = str(tmp_path / 'mine.hforge')
    project.save(own)
    opened = projects.OpenProject(project=project, modified=True)
    return opened, own


def test_the_project_drags_out_as_a_copy_named_for_it(state):
    opened, own = state
    (path,) = projects.drag_out(opened, {'project': True})
    assert os.path.basename(path) == 'Shaker- run 3-4.hforge', 'no character a path reads'
    again = hertzforge.Project.open(path)
    assert list(again) == ['Run'] and again.name == 'Shaker: run 3/4'
    assert opened.project.file == own, 'still saved to its own file'
    assert opened.modified, 'and its unsaved changes are still unsaved'


def test_objects_drag_out_one_file_each_in_a_folder_of_their_own(state):
    opened, _own = state
    first = projects.drag_out(opened, {'names': ['Run']})
    second = projects.drag_out(opened, {'names': ['Run']})
    assert [os.path.basename(p) for p in first] == ['Run.hforge']
    assert os.path.dirname(first[0]) != os.path.dirname(second[0]), 'a name is never taken'
    assert os.path.isfile(first[0]) and os.path.isfile(second[0])


@pytest.mark.parametrize('params, words', [
    ({}, 'one of them'),
    ({'project': True, 'names': ['Run']}, 'one of them'),
    ({'names': ['Gone']}, "no object named 'Gone'"),
    ({'names': []}, 'list of object names'),
])
def test_what_cannot_be_dragged_is_refused(state, params, words):
    opened, _own = state
    with pytest.raises(ProtocolError, match=words):
        projects.drag_out(opened, params)


def test_with_no_project_open_it_is_refused():
    with pytest.raises(ProtocolError, match='none is open'):
        projects.drag_out(None, {'project': True})


def test_over_the_wire_the_answer_is_the_paths(validate):
    import asyncio

    from .test_projects import PLATE, _exchange

    imported, dragged = asyncio.run(_exchange([
        ('project.import', {'path': str(PLATE / 'modal.nc4')}),
        ('project.drag_out', {'project': True}),
    ]))
    for message in imported + dragged:
        validate(message)
    result = dragged[-1]['result']
    validate(result, 'projectDragOutResult')
    assert len(result['paths']) == 1 and os.path.isfile(result['paths'][0])
