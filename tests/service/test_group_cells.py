"""The Element Groups table's property cells, and Solve Modes on what they
set: models that could not be given their properties in the app before, each
checked against its closed-form answer."""

from __future__ import annotations

import math

import pytest

from hertzforge.core.fem import MATERIALS, Section
from hertzforge.project import Project
from hertzforge.service import ops_geometry, ops_model, ops_project, ops_solids, projects
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

OPS = {op.id: op for module in (ops_geometry, ops_model, ops_project, ops_solids)
       for op in module.OPERATIONS}
ALUMINUM = MATERIALS['6061-T6']


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return op.run(project, values)


def type_cell(project, group, column, value):
    """Type into the Element Groups table, as the shell does, by the row
    and the column key the table gives."""
    (table,) = run(project, 'project.element_groups', geometry='Rod')
    ids = [row[0] for row in table['rows']]
    assert column in table['edit']['columns'], f'{column} is not a typed column'
    edit = table['edit']
    return run(project, edit['op'], **edit['params'], row=ids.index(str(group)) + 1,
               column=column, value=value)


def cells(project, group):
    (table,) = run(project, 'project.element_groups', geometry='Rod')
    row = next(row for row in table['rows'] if row[0] == str(group))
    return dict(zip(table['columns'], row, strict=True))


LENGTH = 0.5
SEGMENTS = 20


@pytest.fixture
def rod():
    """A straight line of beams along X, in meters, every element group of
    which needs properties before it solves."""
    project = Project()
    run(project, 'project.new_geometry', name='Rod', unit='m')
    for k in range(SEGMENTS + 1):
        run(project, 'project.add_node', geometry='Rod', x=LENGTH * k / SEGMENTS, id=k + 1)
    for k in range(SEGMENTS):
        run(project, 'project.add_element', geometry='Rod', family='beams',
            nodes=f'{k + 1} {k + 2}', group=1)
    return project


def points(project, *groups):
    """Point elements in groups of their own: group 2 at the root, 3 at the tip."""
    for group in groups:
        node = 1 if group == 2 else SEGMENTS + 1
        run(project, 'project.add_element', geometry='Rod', family='points', nodes=str(node),
            group=group)


def solved(project, **given):
    before = set(project)
    run(project, 'project.solve_modes', source='Rod', **given)
    (name,) = set(project) - before
    return project[name]


def test_a_beam_takes_a_section_and_a_ground_and_solves_as_a_cantilever(rod, validate):
    (table,) = run(rod, 'project.element_groups', geometry='Rod')
    wire_views([table], validate)
    assert table['edit']['choices']['shape'][1] == 'rod'
    assert '6061-T6' in table['edit']['choices']['material']
    type_cell(rod, 1, 'material', '6061-T6')
    type_cell(rod, 1, 'shape', 'rod')
    assert cells(rod, 1)['Dimensions [m]'] == 'D=?'
    type_cell(rod, 1, 'dimensions', '0.01')
    assert cells(rod, 1)['Dimensions [m]'] == 'D=0.01'
    points(rod, 2)
    type_cell(rod, 2, 'ground', 'all')

    shapes = solved(rod, num_modes=2, damping=0.0)
    section = Section.rod('', 0.01)
    # the first bending mode of a cantilever, Euler-Bernoulli: 1.8751 is the
    # first root of cos(x) cosh(x) = -1
    exact = 1.8751 ** 2 / (2 * math.pi) * math.sqrt(
        ALUMINUM.youngs_modulus * section.iy / (ALUMINUM.density * section.area * LENGTH ** 4))
    assert shapes.frequency[0] == pytest.approx(exact, rel=0.005), 'held: no rigid-body modes'


def test_without_a_ground_the_same_beam_is_free(rod):
    type_cell(rod, 1, 'material', '6061-T6')
    type_cell(rod, 1, 'shape', 'rod')
    type_cell(rod, 1, 'dimensions', '0.01')
    shapes = solved(rod, num_modes=8)
    assert shapes.frequency[5] < 1.0 < shapes.frequency[6], 'six rigid-body modes, then bending'


def test_a_tip_mass_lowers_the_cantilever_as_it_should(rod):
    type_cell(rod, 1, 'material', '6061-T6')
    type_cell(rod, 1, 'shape', 'rod')
    type_cell(rod, 1, 'dimensions', '0.01')
    points(rod, 2, 3)
    type_cell(rod, 2, 'ground', 'all')
    type_cell(rod, 3, 'mass', '2.0')
    assert cells(rod, 3)['Mass [kg]'] == '2'
    shapes = solved(rod, num_modes=2)
    section = Section.rod('', 0.01)
    stiffness = 3 * ALUMINUM.youngs_modulus * section.iy / LENGTH ** 3
    beam = ALUMINUM.density * section.area * LENGTH
    # a light cantilever under a heavy tip mass: 33/140 of the beam adds to it
    exact = math.sqrt(stiffness / (2.0 + 33 / 140 * beam)) / (2 * math.pi)
    assert shapes.frequency[0] == pytest.approx(exact, rel=0.01)


def test_springs_to_ground_carry_a_mass_at_its_closed_form_frequency():
    project = Project()
    run(project, 'project.new_geometry', name='Rod', unit='m')
    run(project, 'project.add_node', geometry='Rod', id=1)
    run(project, 'project.add_element', geometry='Rod', family='points', nodes='1', group=1)
    run(project, 'project.add_element', geometry='Rod', family='points', nodes='1', group=2)
    type_cell(project, 1, 'mass', '3')
    type_cell(project, 2, 'stiffness', 'Kx=1200, Ky=1200, Kz=1200')
    assert cells(project, 2)['Stiffness [N/m, N·m/rad]'].startswith('Kx=1200')
    shapes = solved(project, num_modes=3)
    assert max(shapes.frequency) == pytest.approx(math.sqrt(1200 / 3) / (2 * math.pi), rel=1e-6)


def test_a_custom_material_and_section_are_typed_number_by_number(rod):
    type_cell(rod, 1, 'material', 'my alloy')
    type_cell(rod, 1, 'modulus', '70e9')
    type_cell(rod, 1, 'density', '2700')
    type_cell(rod, 1, 'poisson', '0.33')
    type_cell(rod, 1, 'section', 'my bar')
    for key, value in (('area', '1e-4'), ('iy', '1e-9'), ('iz', '2e-9'), ('j', '3e-9')):
        type_cell(rod, 1, key, value)
    type_cell(rod, 1, 'orientation', '0 0 1')
    held = rod['Rod'].group_properties[1]
    assert held.kind == 'beam' and held.material.youngs_modulus == 70e9
    assert held.section.iz == 2e-9 and held.orientation == (0.0, 0.0, 1.0)
    assert cells(rod, 1)['Shape'] == 'custom'
    assert solved(rod, num_modes=8).frequency[7] > 0


def test_a_rod_section_cannot_have_its_numbers_typed(rod):
    type_cell(rod, 1, 'shape', 'rod')
    type_cell(rod, 1, 'dimensions', 'D=0.01')
    assert float(cells(rod, 1)['A [m²]']) == pytest.approx(math.pi * 0.01 ** 2 / 4)
    with pytest.raises(ProtocolError, match='computed from the rod'):
        type_cell(rod, 1, 'area', '1')


@pytest.mark.parametrize(('group', 'column', 'value', 'match'), [
    (1, 'mass', '2', 'a group of beams takes no mass; points do'),
    (1, 'ground', 'all', 'a group of beams takes no ground; points do'),
    (1, 'thickness', '0.01', 'a group of beams takes no thickness; plates do'),
    (2, 'shape', 'rod', 'a group of points takes no section; beams do'),
    (1, 'shape', 'square', 'not a section shape'),
    (1, 'dimensions', '0.01', 'choose a shape first'),
    (1, 'stiffness', '1000', 'by direction'),
    (1, 'stiffness', 'Kz=-1', 'Kz is positive'),
    (2, 'ground', 'W', 'not a direction|W'),
    (1, 'modulus', '-1', 'E cannot be -1'),
    (1, 'modulus', 'stiff', 'not a number'),
    (1, 'poisson', '0.7', 'between -1 and 0.5'),
    (1, 'orientation', '1 0', 'three numbers'),
])
def test_what_a_group_cannot_take_is_refused_and_leaves_it_as_it_was(rod, group, column, value,
                                                                     match):
    points(rod, 2)
    type_cell(rod, 1, 'material', '6061-T6')
    before = dict(rod['Rod'].group_properties)
    with pytest.raises(ProtocolError, match=match) as error:
        type_cell(rod, group, column, value)
    assert error.value.code == 'refused'
    assert rod['Rod'].group_properties == before


def test_clearing_a_groups_cells_clears_what_it_was(rod):
    points(rod, 2, 3)
    type_cell(rod, 3, 'mass', '2')
    type_cell(rod, 3, 'mass', '')
    assert 3 not in rod['Rod'].group_properties
    type_cell(rod, 2, 'ground', 'X, Y, Z')
    assert cells(rod, 2)['Ground'] == 'X, Y, Z' and cells(rod, 2)['Material'] == 'ground (fixed)'
    type_cell(rod, 2, 'ground', '')
    assert 2 not in rod['Rod'].group_properties
    type_cell(rod, 1, 'section', 'bar')
    type_cell(rod, 1, 'section', '')
    assert 1 not in rod['Rod'].group_properties, 'nothing left to say'


def test_a_library_material_fills_a_ground_group_and_makes_it_something_else(rod):
    points(rod, 2)
    type_cell(rod, 2, 'material', 'ground (fixed)')
    assert rod['Rod'].group_properties[2].kind == 'ground'
    with pytest.raises(ProtocolError, match='takes nothing else'):
        type_cell(rod, 2, 'modulus', '1e9')
    type_cell(rod, 2, 'material', '6061-T6')
    assert not rod['Rod'].group_properties[2].ground


def test_group_properties_leaves_beams_and_points_to_the_table(rod):
    with pytest.raises(ProtocolError, match='Element Groups table'):
        run(rod, 'project.group_properties', geometry='Rod', groups='1', material='7075-T6')
    for k, (x, y) in enumerate(((0, 1), (1, 1), (1, 2), (0, 2))):
        run(rod, 'project.add_node', geometry='Rod', x=x, y=y, id=100 + k)
    run(rod, 'project.add_element', geometry='Rod', family='quads', nodes='100 101 102 103',
        group=4)
    (reading,) = run(rod, 'project.group_properties', geometry='Rod', groups='', material='7075-T6',
                     thickness=2.0)
    assert [row['value'] for row in reading['rows']] == ['not set', '7075-T6, 2 mm'], (
        'the plates took it, and the beams were left alone')


def test_solve_modes_takes_a_highest_frequency_and_a_damping(rod):
    type_cell(rod, 1, 'material', '6061-T6')
    type_cell(rod, 1, 'shape', 'rod')
    type_cell(rod, 1, 'dimensions', '0.01')
    points(rod, 2)
    type_cell(rod, 2, 'ground', 'all')
    shapes = solved(rod, maximum_frequency=2000.0, damping=3.0)
    assert 2 <= shapes.num_shapes < 30 and shapes.frequency.max() <= 2000.0
    assert shapes.damping == pytest.approx(0.03)


def test_every_property_survives_saving_the_project(rod, tmp_path):
    type_cell(rod, 1, 'material', '6061-T6')
    type_cell(rod, 1, 'shape', 'I-beam')
    type_cell(rod, 1, 'dimensions', 'd=0.1, bf=0.05, tf=0.005, tw=0.003')
    type_cell(rod, 1, 'orientation', '0 1 0')
    points(rod, 2, 3)
    type_cell(rod, 2, 'ground', 'X, Y, Z')
    type_cell(rod, 3, 'stiffness', 'Kz=5000')
    path = tmp_path / 'rod.hforge'
    rod.save(path)
    back = Project.open(path)['Rod'].group_properties
    assert back[1].section.shape == 'I-beam' and back[1].orientation == (0.0, 1.0, 0.0)
    assert back[1].section.iy == pytest.approx(rod['Rod'].group_properties[1].section.iy)
    assert back[2].ground == ('X', 'Y', 'Z') and back[3].stiffness[2] == 5000.0
