"""Copying an object, recomputing what is stale, and merging objects into
one, as Visual Dynamics' tree does them."""

from __future__ import annotations

from pathlib import Path

import pytest

from hertzforge.service import projects
from hertzforge.service.ops_objects import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in OPERATIONS}


@pytest.fixture
def state():
    opened, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    return opened


def listed(state):
    return {o['name']: o for o in projects.summary(state)['objects']}


def test_a_copy_is_added_beside_the_original(state, validate):
    projects.duplicate(state, {'names': ['Channel Table']})
    found = listed(state)
    assert 'Channel Table copy' in found and state.modified
    assert state.project['Channel Table copy'] is not state.project['Channel Table']
    validate(projects.summary(state), 'project')
    with pytest.raises(ProtocolError, match="no object named 'Nothing'"):
        projects.duplicate(state, {'names': ['Nothing']})


def test_a_psd_goes_stale_when_its_history_changes_and_recomputes(state, validate):
    project = state.project
    psd = project.compute_psds('Time History')
    assert 'stale' not in listed(state)[psd]
    averaging = project['Time History'].averaging
    project['Time History'].averaging = type(averaging)(
        averaging.frame_length // 2, averaging.overlap, averaging.window, averaging.frames,
        averaging.start, averaging.detrend, start_set=True)
    assert listed(state)[psd]['stale']
    validate(projects.summary(state), 'project')
    projects.refresh(state, {'names': [psd]})
    assert 'stale' not in listed(state)[psd]


def test_recomputing_with_no_names_refreshes_everything_stale(state):
    project = state.project
    psd = project.compute_psds('Time History')
    averaging = project['Time History'].averaging
    project['Time History'].averaging = type(averaging)(
        averaging.frame_length // 2, averaging.overlap, averaging.window, averaging.frames,
        averaging.start, averaging.detrend, start_set=True)
    projects.refresh(state, {'names': []})
    assert not project.stale() and psd in project
    with pytest.raises(ProtocolError, match='records no derivation'):
        projects.refresh(state, {'names': ['Channel Table']})


def test_two_geometries_merge_into_one(validate):
    import copy

    import hertzforge
    from hertzforge import mesh

    both = mesh.assemble(mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 0.5, 'a'),
                         mesh.plane((2, 0, 0), (1, 0, 0), (0, 1, 0), 0.5, 'b'), apart=True)
    project = hertzforge.Project('p')
    for label in ('a', 'b'):
        part = copy.deepcopy(both)
        part.delete_groups([g for g, n in zip(part.groups.ids, part.groups.names)
                            if n != label])
        project.add(label.upper(), part)
    (reading,) = OPS['project.merge'].run(project, {'first': 'A', 'second': 'B',
                                                    'more': '', 'name': 'Both'})
    wire_views([reading], validate)
    assert 'Both' in project and 'A' not in project and 'B' not in project
    assert project['Both'].num_nodes == both.num_nodes


def test_objects_of_two_types_are_refused_in_the_cores_words(state):
    with pytest.raises(ProtocolError, match='same type'):
        OPS['project.merge'].run(state.project, {'first': 'Time History',
                                                 'second': 'Channel Table',
                                                 'more': '', 'name': ''})
    with pytest.raises(ProtocolError, match='named once'):
        OPS['project.merge'].run(state.project, {'first': 'Time History',
                                                 'second': 'Time History',
                                                 'more': '', 'name': ''})
