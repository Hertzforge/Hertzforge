"""A projection's node match tolerance is a distance typed in the display system, and its
row opens on the distance 2 % of the target's size comes to; a shape set beside another
group's geometry is offered Project, onto every node of that geometry."""

from __future__ import annotations

import asyncio
import copy
import sys
from pathlib import Path

import pytest

import hertzforge
from hertzforge.service import display_units
from hertzforge.service.action_table import action_table

from .test_projects import _exchange

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from test_correlate import _fem, _test_model

MM = 'mm-kg-N-s'


def _project(defined=True):
    """The cube's modes on their geometry and a test's on another, each node of the test
    1 mm from a corner of the cube, both in meters unless `defined` is False."""
    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    if defined:
        fem_geometry.define_units('m')
        test_geometry.define_units('m')
    project = hertzforge.Project()
    project.add('FEM Geometry', fem_geometry)
    project.add('FEM Shapes', fem_shapes)
    project.add('Test Geometry', test_geometry)
    project.add('Test Shapes', test_shapes)
    project.link('FEM Geometry', 'FEM Shapes')
    project.link('Test Geometry', 'Test Shapes')
    return project


def _row(project, names, system=display_units.DEFAULT, op='project.project_onto_basis'):
    table = display_units.running_in(system, action_table, project, names, {})
    return next(row for row in table['columns'][0]['rows'] if row['id'] == op)


def test_the_rows_open_on_the_default_distance_in_the_display_units():
    project = _project()
    default = project.match_distance('FEM Shapes', 'Test Shapes')
    for system, scale in ((display_units.DEFAULT, 1.0), (MM, 1000.0)):
        for names, op in ((['FEM Shapes', 'Test Shapes'], 'project.project_onto_basis'),
                          (['FEM Shapes', 'Test Geometry'], 'project.project_onto_basis'),
                          (['FEM Shapes', 'Test Shapes'], 'project.projection_overlay')):
            row = _row(project, names, system, op)
            assert row['ready'], row
            assert float(row['fill']['distance']) == pytest.approx(default * scale, rel=1e-3)
    assert _row(project, ['FEM Shapes', 'Test Geometry'])['fill']['onto'] == 'Test Geometry'


def test_an_undefined_geometry_opens_on_its_own_numbers():
    project = _project(defined=False)
    default = project.match_distance('FEM Shapes', 'Test Shapes')
    row = _row(project, ['FEM Shapes', 'Test Shapes'], MM)
    assert float(row['fill']['distance']) == pytest.approx(default, rel=1e-3)


def test_project_is_greyed_out_with_why_it_cannot_project():
    project = _project()
    project.add('Other Modes', copy.deepcopy(project['FEM Shapes']))
    project.link('FEM Geometry', 'Other Modes')
    said = {
        ('FEM Shapes', 'Other Modes'): 'Both are on FEM Geometry, so there is nothing to project',
        ('FEM Shapes', 'FEM Geometry'): None,
        ('Test Shapes', 'FEM Geometry'):
            'Test Shapes has 2 nodes measured in fewer than three directions'}
    for names, why in said.items():
        table = action_table(project, list(names), {})
        row = next((r for r in table['columns'][0]['rows']
                    if r['id'] == 'project.project_onto_basis'), None)
        if why is None:
            # a set beside the geometry it is on: that geometry is only the stage
            assert row is None or not row['ready'], row
            continue
        assert row is not None and not row['ready']
        assert row['reason'] == why
        assert 'distance' not in (row.get('fill') or {})


def _save(project, tmp_path):
    path = str(tmp_path / 'two.hforge')
    project.save(path)
    return path


def _run(path, op, params, system=MM):
    return [got[-1] for got in asyncio.run(_exchange([
        ('project.open', {'path': path}),
        ('units.system', {'name': system}),
        ('op.run', {'op': op, 'params': params})]))][-1]


@pytest.mark.parametrize('onto', ['Test Shapes', 'Test Geometry'])
def test_a_distance_typed_in_millimeters_is_taken_to_meters(tmp_path, onto):
    """Each test node is 1 mm from its corner: 1.1 mm matches them, 0.9 mm none. Taken as
    meters, 0.9 would match as well."""
    path = _save(_project(), tmp_path)
    params = {'source': 'FEM Shapes', 'onto': onto}
    near = _run(path, 'project.project_onto_basis', {**params, 'distance': 1.1})
    assert 'result' in near, near
    far = _run(path, 'project.project_onto_basis', {**params, 'distance': 0.9})
    assert far['error']['code'] == 'refused'
    assert 'within the match tolerance' in far['error']['message']


def test_the_overlay_and_the_channels_take_the_distance_too(tmp_path):
    path = _save(_project(), tmp_path)
    pair = {'first': 'FEM Shapes', 'second': 'Test Shapes'}
    assert 'result' in _run(path, 'project.projection_overlay', {**pair, 'distance': 1.1})
    far = _run(path, 'project.projection_overlay', {**pair, 'distance': 0.9})
    assert far['error']['code'] == 'refused'


def test_an_undefined_geometry_takes_the_distance_as_it_is_typed(tmp_path):
    path = _save(_project(defined=False), tmp_path)
    params = {'source': 'FEM Shapes', 'onto': 'Test Shapes'}
    assert 'result' in _run(path, 'project.project_onto_basis', {**params, 'distance': 0.0011})
    far = _run(path, 'project.project_onto_basis', {**params, 'distance': 0.0009})
    assert far['error']['code'] == 'refused'


def test_zero_projects_within_the_default(tmp_path):
    project = _project()
    path = _save(project, tmp_path)
    answer = _run(path, 'project.project_onto_basis',
                  {'source': 'FEM Shapes', 'onto': 'Test Shapes', 'distance': 0.0})
    assert 'result' in answer, answer
