"""Strain channels in the window: Expand to Physical Responses at a channel table
answers a strain channel with its strain per force and says it read a gauge, and
Project onto Basis DOFs says which strain channels it left out."""

from __future__ import annotations

import sys
from pathlib import Path

import hertzforge
from hertzforge.service import projects
from hertzforge.service.server import OPERATIONS

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from test_element_shapes import _cells


def _state():
    state = projects.new({'title': 'p'})
    project = state.project
    project.add('Cells', _cells(along=4))
    project.solve_modes('Cells', num_modes=10)
    project.add('Channels', hertzforge.ChannelTable({
        'channel': [1, 2, 3], 'node': ['8', '3', '3'], 'direction': ['Z+', 'X+', 'X+'],
        'role': ['reference', 'response', 'response'], 'unit': ['N', 'm/s**2', ''],
        'channel_type': ['force', 'acceleration', 'strain']}))
    project.link('Cells', 'Channels')
    return state


def _run(state, op_id, fill, verbs=None):
    op = next(o for o in OPERATIONS if o.id == op_id)
    kinds = {name: projects.kind_of(obj) for name, obj in state.project.items()}
    verbs = verbs or {name: projects.verbs_of(state.project, name) for name in state.project}
    return op.run(state.project, op.resolve(fill, objects=kinds, verbs=verbs))


def test_expand_at_a_table_reads_the_strain_channel_as_a_gauge():
    state = _state()
    modal = state.project.synthesize_modal_frfs('Cells Modes', sample_rate=40000.0,
                                                frame_length=256)
    views = _run(state, 'project.expand', {'source': modal, 'shapes': 'Channels'})
    rows = {row['label']: row['value'] for view in views if view['view'] == 'reading'
            for row in view['rows']}
    assert rows['Gauges'] == "1 strain channel read on Cells Modes's model"
    assert 'Skipped' not in rows, 'the strain channel is read, not skipped'
    assert rows['FRFs'] == 2.0
    assert any(view['view'] == 'waterfall' for view in views)


def test_project_onto_the_table_says_which_strain_channels_it_left_out():
    state = _state()
    state.project['Channels'] = hertzforge.ChannelTable({
        'channel': [1, 2], 'node': ['3', '6'], 'direction': ['X+', 'Y+'],
        'role': ['response'] * 2, 'unit': ['m/s**2', ''],
        'channel_type': ['acceleration', 'strain']})
    # offered beside a table picked with the set; here the set is named outright
    views = _run(state, 'project.project_onto_basis',
                 {'source': 'Cells Modes', 'onto': 'Channels'},
                 verbs={'Cells Modes': ['project_onto_basis']})
    rows = {row['label']: row['value'] for view in views if view['view'] == 'reading'
            for row in view['rows']}
    assert rows['Strain channels'].startswith('1 left out, a shape set holding displacements')
