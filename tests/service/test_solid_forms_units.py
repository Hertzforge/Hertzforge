"""A solid's form is typed in the display system, and the service takes it to meters; an
undefined geometry's numbers pass as they are."""

from __future__ import annotations

import asyncio

import numpy as np
import pytest

from hertzforge.core.geometry import Geometry
from hertzforge.service import display_units
from hertzforge.service.ops_solids import OPERATIONS
from hertzforge.service.server import OPERATIONS as EVERY

from .test_projects import _exchange

INCH = 0.0254
OPS = {op.id: op for op in EVERY}
#: the geometry made first in each exchange, in meters, the display in inches
START = [('project.new', {}),
         ('units.system', {'name': display_units.START}),
         ('op.run', {'op': 'project.new_geometry', 'params': {'name': 'Part', 'unit': 'm'}})]


def answers(*steps):
    """Each step's last frame, after `START`."""
    return [got[-1] for got in asyncio.run(_exchange([*START, *steps]))][len(START):]


def add_block(**given):
    return ('op.run', {'op': 'project.add_solid_block', 'params': {'geometry': 'Part', **given}})


def solids_table(answer):
    return next(view for view in answer['result']['views']
                if view['view'] == 'table' and view['title'].startswith('Solids'))


def rows(answer):
    return {row['label']: row for row in answer['result']['views'][0]['rows']}


def test_a_two_inch_block_in_a_meter_geometry_is_two_inches():
    (block,) = answers(add_block(width_x=2.0, width_y=1.0, width_z=0.5, x=3.0, corner=0.1))
    cells = solids_table(block)['rows'][0]
    assert cells[3] == '2 in × 1 in × 0.5 in, corner 0.1 in'
    assert cells[4] == '(3 in, 0 in, 0 in)'
    assert rows(block)['Volume']['unit'] == 'in³'
    assert rows(block)['Volume']['value'] == pytest.approx(1.0 - (4 - np.pi) * 0.1 ** 2 * 0.5)


def test_a_two_inch_block_is_held_as_0_0508_m():
    (_block, _si, shown) = answers(
        add_block(width_x=2.0),
        ('units.system', {'name': display_units.DEFAULT}),
        ('op.run', {'op': 'project.solids', 'params': {'geometry': 'Part'}}))
    assert solids_table(shown)['rows'][0][3] == '0.0508 m × 0.0254 m × 0.0254 m'


def test_in_si_the_same_numbers_are_meters():
    steps = [('project.new', {}), ('units.system', {'name': display_units.DEFAULT}),
             ('op.run', {'op': 'project.new_geometry', 'params': {'name': 'Part', 'unit': 'm'}}),
             add_block(width_x=2.0)]
    block = asyncio.run(_exchange(steps))[-1][-1]
    assert solids_table(block)['rows'][0][3] == '2 m × 1 m × 1 m'


@pytest.mark.parametrize(('op', 'given', 'cell'), [
    ('project.add_solid_cylinder', {'radius': 2.0, 'length': 8.0}, 'radius 2 in, length 8 in'),
    ('project.add_solid_cone', {'radius': 2.0, 'top_radius': 1.0, 'height': 4.0},
     'base radius 2 in, top radius 1 in, height 4 in'),
    ('project.add_solid_sphere', {'radius': 3.0}, 'radius 3 in'),
    ('project.add_solid_plate', {'width_x': 4.0, 'width_y': 2.0}, '4 in × 2 in'),
    ('project.add_solid_disk', {'radius': 2.0}, 'radius 2 in'),
    ('project.add_solid_hole', {'diameter': 1.0, 'depth': 2.0}, 'diameter 1 in, depth 2 in'),
    ('project.add_solid_section', {'shape': 'round tube', 'dimensions': '4 0.5', 'length': 10.0},
     'outer diameter 4 in, wall 0.5 in, length 10 in')])
def test_every_shape_is_typed_in_the_display_system(op, given, cell):
    (made,) = answers(('op.run', {'op': op, 'params': {'geometry': 'Part', **given}}))
    assert solids_table(made)['rows'][0][3] == cell


def test_an_edit_is_typed_in_the_display_system():
    (_block, edited) = answers(
        add_block(),
        ('op.run', {'op': 'project.edit_solid', 'params': {
            'geometry': 'Part', 'solid': 1, 'widths': '2 3 4', 'center': '1, 0, 0'}}))
    cells = solids_table(edited)['rows'][0]
    assert cells[3] == '2 in × 3 in × 4 in' and cells[4] == '(1 in, 0 in, 0 in)'


def test_a_pattern_keeps_its_counts_and_converts_its_spacing():
    (_block, patterned) = answers(
        add_block(),
        ('op.run', {'op': 'project.pattern_solids', 'params': {
            'geometry': 'Part', 'solids': '1', 'count_x': 3, 'spacing_x': 2.0}}))
    assert solids_table(patterned)['rows'][0][3] == '3 × 1 × 1, spacing 2 in, 0 in, 0 in'


def test_move_part_and_its_drag_are_in_the_display_system():
    (_block, looked, dragged) = answers(
        add_block(),
        ('op.run', {'op': 'project.move_solids', 'params': {'geometry': 'Part', 'solids': '1'}}),
        ('op.run', {'op': 'project.move_solids', 'params': {
            'geometry': 'Part', 'solids': '1', 'offset_x': 2.0}}))
    view = next(v for v in looked['result']['views'] if v['view'] == 'shape3d')
    gizmo = view['gizmo']
    assert 'to_si' not in gizmo, 'a drag of 2 in the view is sent as 2, typed in inches'
    assert gizmo['origin'] == [0.0, 0.0, 0.0]
    moved = next(v for v in dragged['result']['views'] if v['view'] == 'shape3d')
    assert moved['gizmo']['origin'] == pytest.approx([2.0, 0.0, 0.0])
    assert solids_table(dragged)['rows'][0][4] == '(2 in, 0 in, 0 in)'


def test_a_coordinate_systems_drag_is_still_sent_in_meters():
    views = [{'view': 'shape3d', 'unit': 'm', 'nodes': np.zeros((1, 3), np.float32), 'modes': [],
              'gizmo': {'origin': [INCH, 0.0, 0.0], 'axes': np.eye(3).tolist(),
                        'slide': {'op': 'x', 'params': {}, 'values': {}},
                        'turn': {'op': 'x', 'params': {}, 'values': {}}}}]
    display_units.convert(views, display_units.START)
    assert views[0]['gizmo']['to_si'] == pytest.approx(INCH)


def test_mesh_sizes_are_typed_in_the_display_system():
    (_block, preview, kept) = answers(
        add_block(width_x=1.0, width_y=1.0, width_z=1.0),
        ('op.run', {'op': 'project.mesh_solids', 'params': {'geometry': 'Part', 'size': 0.5}}),
        ('op.run', {'op': 'project.mesh_solids', 'params': {
            'geometry': 'Part', 'size': 0.5, 'keep': True}}))
    for answer in (preview, kept):
        said = rows(answer)
        assert said['Element size']['unit'] == 'in'
        assert said['Element size']['value'] == pytest.approx(0.5)
        assert said['Bricks']['value'] == 8.0
        assert said['Shortest edge']['value'] == pytest.approx(0.5)


def test_piece_sizes_and_what_a_fold_says_are_in_the_display_system():
    sphere = {'geometry': 'Part', 'radius': 5.0}
    (_one, _two, size_alone, sized) = answers(
        ('op.run', {'op': 'project.add_solid_sphere', 'params': sphere}),
        ('op.run', {'op': 'project.add_solid_sphere', 'params': {**sphere, 'x': 50.0}}),
        ('op.run', {'op': 'project.mesh_solids', 'params': {'geometry': 'Part', 'size': 4.0}}),
        ('op.run', {'op': 'project.mesh_solids', 'params': {
            'geometry': 'Part', 'size': 1.0, 'sizes': '1: 4, 2: 1'}}))
    assert 'at 4 in the fit is likely to fold' in rows(size_alone)['Fit']['value']
    assert 'at 4 in the fit is likely to fold' in rows(sized)['Fit']['value']
    pieces = next(v for v in sized['result']['views'] if v.get('title') == 'Pieces')
    assert pieces['columns'][3:5] == ['Size [in]', 'Likely size [in]']
    assert [float(row[3]) for row in pieces['rows']] == pytest.approx([4.0, 1.0])


def test_a_refusal_says_its_lengths_in_the_display_system():
    sphere = {'geometry': 'Part', 'radius': 5.0}
    (_one, refusal) = answers(
        ('op.run', {'op': 'project.add_solid_sphere', 'params': sphere}),
        ('op.run', {'op': 'project.mesh_solids', 'params': {
            'geometry': 'Part', 'size': 4.0, 'keep': True}}))
    assert 'at 4 in the fit' in refusal['error']['message']


def test_a_mate_tolerance_is_typed_in_the_display_system():
    block = {'width_x': 1.0, 'width_y': 1.0, 'width_z': 1.0}
    (_a, _b, _meshed, mated) = answers(
        add_block(**block), add_block(z=1.0, **block),
        ('op.run', {'op': 'project.mesh_solids', 'params': {
            'geometry': 'Part', 'size': 0.5, 'parts_apart': True, 'keep': True}}),
        ('op.run', {'op': 'project.mate_surfaces', 'params': {
            'geometry': 'Part', 'tolerance': 0.01}}))
    said = rows(mated)
    assert said['Tolerance']['unit'] == 'in'
    assert said['Tolerance']['value'] == pytest.approx(0.01)
    assert said['Patches found']['value'] == 1.0


def test_a_live_preview_is_typed_in_the_display_system_and_changes_nothing():
    (previewed, after) = answers(
        ('op.run', {'op': 'project.add_solid_block', 'params': {
            'geometry': 'Part', 'width_x': 2.0}, 'preview': True}),
        ('op.run', {'op': 'project.solids', 'params': {'geometry': 'Part'}}))
    assert solids_table(previewed)['rows'][0][3] == '2 in × 1 in × 1 in'
    assert rows(previewed)['Would add']
    assert solids_table(after)['rows'] == []


def test_changing_the_display_system_says_its_length_and_leaves_the_model():
    (_block, si, shown, inches, again) = answers(
        add_block(width_x=2.0),
        ('units.system', {'name': display_units.DEFAULT}),
        ('op.run', {'op': 'project.solids', 'params': {'geometry': 'Part'}}),
        ('units.system', {'name': display_units.START}),
        ('op.run', {'op': 'project.solids', 'params': {'geometry': 'Part'}}))
    assert si['result']['length'] == {'unit': 'm', 'to_si': 1.0}
    assert inches['result']['length']['unit'] == 'in'
    assert solids_table(shown)['rows'][0][3] == '0.0508 m × 0.0254 m × 0.0254 m'
    assert solids_table(again)['rows'][0][3] == '2 in × 1 in × 1 in'


def test_the_system_says_its_length_unit():
    assert display_units.length_of(display_units.START) == {'unit': 'in',
                                                            'to_si': pytest.approx(INCH)}
    assert display_units.length_of('m-kg-N-s') == {'unit': 'm', 'to_si': 1.0}


def test_an_undefined_geometry_takes_its_numbers_as_given():
    geometry = Geometry(node_id=[], node_xyz=np.empty((0, 3)))
    assert not geometry.units_defined
    op = OPS['project.add_solid_block']
    given = op.resolve({'geometry': 'Raw', 'width_x': 2.0}, objects={'Raw': 'geometry'})
    assert display_units.typed_lengths(op.params, given, display_units.START, geometry) == given


def test_a_text_of_lengths_keeps_its_piece_ids_and_its_shape():
    op = OPS['project.mesh_solids']
    defined = Geometry(node_id=[], node_xyz=np.empty((0, 3)), length_unit='m')
    given = op.resolve({'geometry': 'P', 'sizes': '3: 2, 5: 0.5', 'size': 4.0},
                       objects={'P': 'geometry'})
    out = display_units.typed_lengths(op.params, given, display_units.START, defined)
    assert out['size'] == pytest.approx(4 * INCH)
    pieces = dict(entry.split(':') for entry in out['sizes'].replace(' ', '').split(','))
    assert set(pieces) == {'3', '5'}
    assert float(pieces['3']) == pytest.approx(2 * INCH)
    assert float(pieces['5']) == pytest.approx(0.5 * INCH)


def test_every_length_of_a_solids_form_is_marked():
    marked = {op.id: {p.name for p in op.params if p.length} for op in OPERATIONS}
    assert marked['project.add_solid_block'] == {'x', 'y', 'z', 'width_x', 'width_y', 'width_z',
                                                 'corner'}
    assert marked['project.edit_solid'] == {'center', 'widths', 'radius', 'top_radius', 'length',
                                            'corner', 'dimensions', 'spacing'}
    assert marked['project.move_solids'] == {'offset_x', 'offset_y', 'offset_z'}
    assert marked['project.mesh_solids'] == {'size', 'sizes'}
    assert marked['project.mate_surfaces'] == {'tolerance'}
    assert marked['project.add_solid_section'] >= {'dimensions', 'length'}
    assert marked['project.pattern_solids'] == {'spacing_x', 'spacing_y', 'spacing_z'}


LBF_PER_IN = 4.4482216152605 / INCH


def test_a_mate_stiffness_is_typed_and_said_in_the_display_system():
    # Brandon, 2026-11-11: the mate's stiffness was asked for, and said, in N/m
    block = {'width_x': 1.0, 'width_y': 1.0, 'width_z': 1.0}
    (_a, _b, _meshed, mated, _si, shown) = answers(
        add_block(**block), add_block(z=1.0, **block),
        ('op.run', {'op': 'project.mesh_solids', 'params': {
            'geometry': 'Part', 'size': 0.5, 'parts_apart': True, 'keep': True}}),
        ('op.run', {'op': 'project.mate_surfaces', 'params': {
            'geometry': 'Part', 'tolerance': 0.01, 'normal': 'stiffness',
            'normal_stiffness': 1000.0, 'keep': True}}),
        ('units.system', {'name': display_units.DEFAULT}),
        ('op.run', {'op': 'project.mate_surfaces', 'params': {'geometry': 'Part'}}))
    held = lambda answer: next(view for view in answer['result']['views']
                               if view['view'] == 'table'
                               and view['title'].startswith('Mated'))['rows'][0][-1]
    assert 'normal 1000 lbf/in' in held(mated)
    assert f'normal {1000 * LBF_PER_IN:.4g} N/m' in held(shown)


def test_the_system_says_its_stiffness_unit():
    (inches,) = answers(('units.system', {}))
    assert inches['result']['quantities'] == {
        'force/length': {'unit': 'lbf/in', 'to_si': pytest.approx(LBF_PER_IN)}}


def test_every_stiffness_of_a_solids_form_is_marked():
    marked = {op.id: {p.name for p in op.params if p.quantity == 'force/length'}
              for op in OPERATIONS}
    assert marked['project.mate_surfaces'] == {'normal_stiffness', 'shear_stiffness'}


def test_a_mate_picked_opens_the_pane_on_its_stiffness_in_the_display_system():
    block = {'width_x': 1.0, 'width_y': 1.0, 'width_z': 1.0}
    (*_made, picked) = answers(
        add_block(**block), add_block(z=1.0, **block),
        ('op.run', {'op': 'project.mesh_solids', 'params': {
            'geometry': 'Part', 'size': 0.5, 'parts_apart': True, 'keep': True}}),
        ('op.run', {'op': 'project.mate_surfaces', 'params': {
            'geometry': 'Part', 'normal': 'stiffness', 'normal_stiffness': 1000.0,
            'keep': True}}),
        ('project.actions', {'objects': ['Part'], 'part': {'name': 'Part', 'kind': 'mate',
                                                            'id': 1}}))
    rows = picked['result']['table']['columns'][0]['rows']
    edit = next(row for row in rows if row['verb'] == 'Edit')
    assert float(edit['fill']['normal_stiffness']) == pytest.approx(1000.0)
    assert 'shear_stiffness' not in edit['fill'], 'a rigid shear opens on the default'
