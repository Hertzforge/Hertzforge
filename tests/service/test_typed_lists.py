"""One id parser and one number parser for what is typed: commas and spaces both separate,
and a run reads as the Element Groups table prints it, so a space never glues two into one."""

from __future__ import annotations

import pytest

from hertzforge.project import Project
from hertzforge.service import ops_generate, ops_modal_plan, ops_project, projects
from hertzforge.service.parsing import parse_id_runs, parse_numbers, parse_picks
from hertzforge.service.protocol import ProtocolError
from hertzforge.service.server import OPERATIONS

OPS = {op.id: op for op in OPERATIONS}


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()})
    return op.run(project, values)


def two_blocks():
    project = Project()
    project.new_geometry('Part', unit='m')
    for z in (0.0, 1.0):
        run(project, 'project.add_solid_block', z=z)
    run(project, 'project.mesh_solids', size=0.5, parts_apart=True, keep=True)
    return project


def test_ids_take_commas_spaces_and_runs():
    assert parse_id_runs('1-3 7') == [1, 2, 3, 7]
    assert parse_id_runs('1-16 33-48') == [*range(1, 17), *range(33, 49)]
    assert parse_id_runs('3, 7,9') == [3, 7, 9]
    with pytest.raises(ProtocolError, match='is not a solid id'):
        parse_id_runs('1 a', 'a solid id')
    with pytest.raises(ProtocolError, match='is not an id'):
        parse_id_runs('1.5')


def test_numbers_take_commas_and_spaces_and_refuse_what_is_not_finite():
    assert parse_numbers('0.5 1') == [0.5, 1.0]
    assert parse_numbers('0, 0,1e-3') == [0.0, 0.0, 0.001]
    assert parse_numbers('') == []
    for bad in ('nan', '1 inf', 'one'):
        with pytest.raises(ProtocolError, match='is not the widths'):
            parse_numbers(bad, 'the widths')


def test_picks_count_from_one_and_stay_within_the_count():
    assert parse_picks('1 2', 20, 'record') == [0, 1]
    assert parse_picks('1-3, 7', 20, 'record') == [0, 1, 2, 6]
    with pytest.raises(ProtocolError, match='records run from 1 to 3; 4 is outside them'):
        parse_picks('2 4', 3, 'record')
    with pytest.raises(ProtocolError, match='not a record number'):
        parse_picks('x', 3, 'record')


def test_records_and_modes_typed_with_a_space_are_two():
    assert ops_project.chosen_records('1 2', 20) == [0, 1]
    assert ops_modal_plan._picks('1 2', 20, 'mode') == [0, 1]
    assert ops_generate._picked('1 2', 20, 'mode') == [0, 1]


def test_a_patch_typed_as_the_groups_table_prints_it_is_those_elements():
    project = two_blocks()
    _reading, view = run(project, 'project.tie_elements', elements='1-3 7')
    assert view['pick']['kept'] == [1, 2, 3, 7]


def test_groups_typed_with_a_space_are_two_groups():
    project = two_blocks()
    (table,) = run(project, 'project.merge_element_groups', groups='1 2')
    assert len(table['rows']) == 1


def test_solids_typed_with_a_space_are_two_solids():
    project = two_blocks()
    run(project, 'project.combine_solids', solids='1 2', operation='union')
    assert project['Part'].solids[-1].operands == (1, 2)


def test_places_left_out_with_a_space_are_two_places():
    project = two_blocks()
    places = [spot[0] for spot in project['Part'].coincident_spots(1e-6, None)]
    reading, _view = run(project, 'project.merge_coincident_nodes',
                         left_out=f'{places[0]} {places[1]}')
    rows = {row['label']: row['value'] for row in reading['rows']}
    assert rows['Left out'] == f'{places[0]}, {places[1]}'


def test_a_radius_of_two_numbers_is_refused_and_a_center_of_three_is_read():
    project = two_blocks()
    run(project, 'project.add_solid_cylinder')
    with pytest.raises(ProtocolError, match='the radius is one number'):
        run(project, 'project.edit_solid', solid=3, radius='0.5 1')
    run(project, 'project.edit_solid', solid=1, center='0 0 0.25')
    assert project['Part'].solids[0].center == (0.0, 0.0, 0.25)


def test_patches_and_mates_typed_with_a_space_are_two():
    project = Project()
    project.new_geometry('Part', unit='m')
    run(project, 'project.add_solid_block')
    run(project, 'project.add_solid_block', z=1.0)
    run(project, 'project.add_solid_block', z=2.0)
    run(project, 'project.mesh_solids', size=0.5, parts_apart=True, keep=True)
    run(project, 'project.mate_surfaces', patches='1 2', keep=True)
    assert len(project['Part'].mates) == 2
    run(project, 'project.delete_mated_surfaces', mates='1 2')
    assert not project['Part'].mates


def test_places_left_out_read_in_order():
    from hertzforge.service.ops_model import _id_text

    assert _id_text({1000, 3, 65, 70000, 8}) == '3, 8, 65, 1000, 70000'
