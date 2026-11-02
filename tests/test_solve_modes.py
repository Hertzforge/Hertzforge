"""Solve Modes: a geometry whose element groups carry their properties is a
finite element model, and the app solves it (Brandon, 2026-09-25).

The second slice of the element group model. The Element Groups table grew the property
columns — a material for any element group, a thickness for plates, a section
for beams — writing into `geometry.group_properties` with journal lines
a session script replays; and `Project.solve_modes` builds the model
from the element groups and adds its normal modes in the geometry's group, with
a Solve Modes act on the bar that asks the two numbers the solution
takes.
"""

from __future__ import annotations

import numpy as np
import pytest
from test_fem import ALUMINUM, square_plate

import hertzforge
from hertzforge.core.fem import GroupProperties, Model, Section
from hertzforge.core.shapes import ShapeSet
from hertzforge.project import Project


def _plate_geometry(mesh=4, properties=True):
    geometry = square_plate(mesh).geometry()
    if properties:
        geometry.group_properties = {
            int(geometry.group_id[0]): GroupProperties(ALUMINUM, 0.01)}
    return geometry


def _beam_geometry():
    """A group of eight beams with nothing set: a section is a beam's, and
    the table takes one only for a group of beams."""
    from test_springs import _whole_beam

    geometry = _whole_beam().geometry()
    geometry.group_properties = {}
    return geometry


def _verbs(project, name):
    return [verb for verb, _reading in project.verbs(name)]


def test_solve_modes_applies_only_once_the_blocks_carry_properties():
    project = Project()
    project.add('Bare', _plate_geometry(properties=False))
    project.add('Skin', _plate_geometry())
    assert 'solve_modes' not in _verbs(project, 'Bare')
    assert 'generate_rigid_body_modes' in _verbs(project, 'Bare')
    assert 'solve_modes' in _verbs(project, 'Skin')


def test_solve_modes_adds_the_models_modes_in_the_geometrys_group():
    project = Project()
    geometry = _plate_geometry()
    project.add('Skin', geometry)
    expected = Model.from_geometry(geometry).eigensolution(
        maximum_frequency=2000.0, damping=0.02)
    name = project.solve_modes('Skin', maximum_frequency=2000.0, damping=0.02)
    assert name == 'Skin Modes'
    shapes = project[name]
    assert isinstance(shapes, ShapeSet)
    assert np.allclose(shapes.frequency, expected.frequency, rtol=1e-9,
                       atol=1e-6)
    assert int(np.sum(shapes.frequency == 0.0)) == 6, 'free-free'
    assert np.allclose(shapes.damping, 0.02)
    assert any({'Skin', 'Skin Modes'} <= set(group['members'])
               for group in project.object_groups), 'linked to its geometry'
    assert project.provenance[name]['verb'] == 'solve_modes'


def test_solve_modes_refuses_a_bare_geometry_and_a_non_geometry():
    project = Project()
    project.add('Bare', _plate_geometry(properties=False))
    with pytest.raises(ValueError, match='no element group properties'):
        project.solve_modes('Bare')
    project.add('Shapes', ShapeSet([10.0], [0.01], ['1Z+'], [[1.0]]))
    with pytest.raises(TypeError, match='not a geometry'):
        project.solve_modes('Shapes')


def test_a_saved_geometry_solves_to_the_same_modes(tmp_path):
    project = Project()
    project.add('Skin', _plate_geometry())
    project.solve_modes('Skin', num_modes=8)
    hertzforge.save(project['Skin'], tmp_path / 'skin.vdyn')
    again = Project()
    again.add('Skin', hertzforge.load(tmp_path / 'skin.vdyn'))
    again.solve_modes('Skin', num_modes=8)
    assert np.allclose(again['Skin Modes'].frequency,
                       project['Skin Modes'].frequency)


def test_the_library_is_typical_handbook_values_each_with_its_note():
    from hertzforge.core.fem import MATERIAL_LIBRARY, MATERIALS, LibraryMaterial, material

    names = [entry.material.name for entry in MATERIAL_LIBRARY]
    assert len(names) == len(set(names)) >= 18
    # the lookup is the library and the rigid link, which is picked where
    # a material is but is not one (2026-09-26)
    assert set(MATERIALS) == set(names) | {'rigid (massless)'}
    for entry in MATERIAL_LIBRARY:
        assert isinstance(entry, LibraryMaterial) and entry.note, entry
        m = entry.material
        assert 1e9 < m.youngs_modulus < 500e9, m
        assert 900 < m.density < 20000, m
        assert 0.2 <= m.poissons_ratio < 0.5, m
        assert m.modulus_of_rigidity is None, 'isotropic, derived'
        assert material(m.name) is m
    with pytest.raises(KeyError, match="'unobtainium' is not in the "
                                        'material library; it has 6061-T6'):
        material('unobtainium')


def test_the_demo_plate_is_made_of_the_librarys_6061():
    from hertzforge.core.fem import PSI, material
    from hertzforge.demo import plate

    assert plate.ALUMINUM is material('6061-T6')
    assert plate.ALUMINUM.youngs_modulus == 10.0e6 * PSI, (
        'the handbook number, converted exactly, not a rounding')


def _section_of(geometry):
    return geometry.group_properties[int(geometry.group_id[0])].section


def test_an_unfinished_section_is_refused_when_the_model_is_built():
    frame = hertzforge.Geometry(
        node_id=[1, 2, 3], node_xyz=[[0, 0, 0], [0, 0, 1], [1, 0, 1]],
        elem_id=[1, 2], elem_type=[21, 21], elem_conn=[[1, 2], [2, 3]],
        elem_group=[1, 1], group_id=[1], group_name=['frame'],
        length_unit='m',
        group_properties={1: GroupProperties(
            ALUMINUM, section=Section('tube', 0, 0, 0, 0, 'round tube', ()),
            orientation=(0.0, 1.0, 0.0))})
    with pytest.raises(ValueError, match=r'element group 1 \(frame\): the section is '
                                          r'not finished — its round tube has '
                                          r'no dimensions yet'):
        Model.from_geometry(frame)


def _hears(solver):
    """(done, total) as the solve told it, and the shapes."""
    heard = []
    shapes = _plate_geometry(mesh=6)
    model = Model.from_geometry(shapes)
    found = model.eigensolution(num_modes=12, solver=solver,
                                progress=lambda d, t: heard.append((d, t)))
    return heard, found


@pytest.mark.parametrize('solver', ['sparse', 'dense'])
def test_the_solve_tells_its_progress(solver):
    """`progress` hears (done, total) on either path: done never falls,
    never passes a counted total, and the last word is done == total.
    The dense path's eigensolution, one call that cannot count itself,
    is the one report with the total BUSY instead."""
    from hertzforge.core.progress import BUSY

    heard, found = _hears(solver)
    assert heard and found.num_shapes == 12
    done = [d for d, _t in heard]
    assert done == sorted(done)
    counted = [(d, t) for d, t in heard if t != BUSY]
    assert all(d <= t for d, t in counted)
    assert len(heard) - len(counted) == (1 if solver == 'dense' else 0)
    assert heard[-1][0] == heard[-1][1] > 0


def test_the_sparse_solve_counts_the_elements_and_the_iterations():
    """The sparse path's total holds the assembly per element, the
    factorization, the eigen iterations and the polish — many more
    words than the dense path's two steps."""
    sparse, _ = _hears('sparse')
    dense, _ = _hears('dense')
    assert sparse[-1][1] > 36 + 1 + 12, sparse[-1]
    assert dense[-1][1] == 2


def test_the_eigen_stage_grows_the_total_while_it_runs():
    """The iterations' length is an estimate, extended while they run
    (`Ticker.extend_to`): the total is raised past what is done
    whenever it would be reached, so the bar sits short of full until
    the stage ends."""
    from hertzforge.core.progress import Ticker

    words = []
    ticker = Ticker(lambda d, t: words.append((d, t)), interval=0.0)
    ticker.add(2)
    ticker.tick(2)
    ticker.extend_to(1)
    assert (ticker.done, ticker.total) == (2, 3)
    ticker.tick()
    ticker.extend_to(1)
    assert (ticker.done, ticker.total) == (3, 4)
    ticker.extend_to(5)
    assert ticker.total == 8, 'raised to the estimate past what is done'
    assert words[-1] == (3, 8)


def test_what_the_progress_raises_stops_the_solve():
    class Stop(Exception):
        pass

    def cancel(done, total):
        if done > 0:
            raise Stop

    model = Model.from_geometry(_plate_geometry(mesh=6))
    with pytest.raises(Stop):
        model.eigensolution(num_modes=12, solver='sparse', progress=cancel)


def test_sparse_frequencies_are_unchanged_by_the_shared_factor():
    """eigsh solves with the model's own factor now (`OPinv`), the one
    the polish uses: the answer is the dense answer still."""
    model = Model.from_geometry(_plate_geometry(mesh=6))
    dense = model.eigensolution(num_modes=12, solver='dense').frequency
    sparse = model.eigensolution(num_modes=12, solver='sparse').frequency
    assert sparse == pytest.approx(dense, rel=1e-7, abs=1e-6)


def test_a_solved_mode_is_written_in_each_nodes_displacement_frame():
    """Every reader takes a DOF's direction in its node's displacement
    frame (`deform.dof_directions`), and the solve wrote global
    components under those labels: the matrices came out identical with
    a node's frame turned or not (Brandon, 2026-10-23). The node's rows
    are now its frame's, and the motion every reader reconstructs is the
    same physical motion either way."""
    import copy

    from hertzforge.deform import node_displacements

    plain = _plate_geometry()
    turned = copy.deepcopy(plain)
    # 90 degrees about Z: the node's X is global +Y, its Y global -X
    frame = turned.add_coordinate_system(
        rotation=[[0, 1, 0], [-1, 0, 0], [0, 0, 1]])
    node = int(turned.node_id[7])
    turned.node_disp_cs[7] = frame
    a = Model.from_geometry(plain).eigensolution(num_modes=12)
    b = Model.from_geometry(turned).eigensolution(num_modes=12)
    assert np.allclose(a.frequency, b.frequency)
    column = {dof: k for k, dof in enumerate(a.coordinate)}
    rows = [column[f'{node}{d}'] for d in ('X+', 'Y+', 'Z+', 'RX+', 'RY+', 'RZ+')]
    others = np.setdiff1d(np.arange(len(a.coordinate)), rows)
    moved = 0
    for m in range(12):
        # a mode's sign is the sign rule's, read on what is stored
        sign = np.sign(a.shape_matrix[m, others] @ b.shape_matrix[m, others])
        x, y, z, rx, ry, rz = a.shape_matrix[m, rows]
        assert np.allclose(sign * b.shape_matrix[m, rows],
                           [y, -x, z, ry, -rx, rz], atol=1e-12), m
        assert np.allclose(sign * b.shape_matrix[m, others],
                           a.shape_matrix[m, others]), 'the other nodes as they were'
        globally = node_displacements(plain, a.coordinate, a.shape_matrix[m])
        read = node_displacements(turned, b.coordinate, sign * b.shape_matrix[m])
        assert np.allclose(read, globally, atol=1e-12), \
            f'mode {m + 1} moves the same way as the reader reads it'
        moved += abs(x) + abs(y) > 1e-6
    assert moved >= 3, 'modes that move the node in its plane were checked'
