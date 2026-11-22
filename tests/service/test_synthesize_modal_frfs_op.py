"""Synthesize Modal FRFs runs on the modes picked under its shape set, and its form
previews, so the line spacing its sample rate and frame give is read while they are typed."""

from __future__ import annotations

import asyncio

import numpy as np

import hertzforge
from hertzforge.core.shapes import ShapeSet
from hertzforge.service import projects
from hertzforge.service.server import OPERATIONS

from .test_projects import _exchange

OP = 'project.synthesize_modal_frfs'


def _shapes():
    return ShapeSet(frequency=[10.0, 20.0, 40.0], damping=[0.01, 0.02, 0.03],
                    shape_matrix=np.eye(3), coordinate=['1Z+', '2Z+', '3Z+'])


def _row(state, records=None):
    columns = projects.actions(state, {'objects': ['Modes'], 'records': records or {},
                                       'part': None})['table']['columns']
    rows = [row for column in columns for row in column['rows']]
    return next(row for row in rows if row['id'] == OP)


def test_the_modes_are_the_ones_picked_under_the_set():
    state = projects.new({'title': 'p'})
    state.project.add('Modes', _shapes())
    whole = _row(state)
    picked = _row(state, {'Modes': [0, 2]})
    assert whole['fill']['modes'] == '' and 'modes' in whole['omit']
    assert picked['fill']['modes'] == '1, 3' and 'modes' in picked['omit']
    op = next(o for o in OPERATIONS if o.id == OP)
    for row, count in ((whole, 3), (picked, 2)):
        values = op.resolve(row['fill'], objects={'Modes': 'shape_set'},
                            verbs={'Modes': projects.verbs_of(state.project, 'Modes')})
        views = op.run(state.project, values)
        added = next(r['value'] for v in views if v['view'] == 'reading'
                     for r in v['rows'] if r['label'] == 'Added')
        assert state.project[added].num_records == count


def test_the_form_previews_the_line_spacing_and_adds_nothing(tmp_path):
    project = hertzforge.Project('p')
    project.add('Modes', _shapes())
    path = tmp_path / 'modes.hforge'
    project.save(str(path))
    params = {'source': 'Modes', 'sample_rate': 1000.0, 'frame_length': 400,
              'response': 'acceleration', 'modes': '1, 3'}
    responses = asyncio.run(_exchange([
        ('project.open', {'path': str(path)}),
        ('op.run', {'op': OP, 'params': params, 'preview': True}),
        ('op.run', {'op': OP, 'params': params}),
    ]))
    preview, ran = (got[-1]['result'] for got in responses[1:])
    assert 'project' not in preview
    rows = {row['label']: row for view in preview['views'] if view['view'] == 'reading'
            for row in view['rows']}
    assert rows['Line spacing']['value'] == 2.5 and rows['Line spacing']['unit'] == 'Hz'
    assert rows['Modes']['value'] == '2 of 3'
    assert 'Would add' in rows and 'Added' not in rows
    names = [o['name'] for o in ran['project']['objects']]
    assert names.count('Modes Modal FRFs') == 1, 'the preview added nothing'
