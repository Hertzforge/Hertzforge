"""Test-analysis correlation: FEM shapes projected onto test DOFs.

The FEM is a unit cube with an analytically known displacement field,
the test a couple of uniaxial sensors — one in a rotated local frame —
so the projected values are checkable by hand: matching, rotation into
the local frame, and the uniaxial dot product all pin exactly.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.correlate import project_shapes
from hertzforge.core.shapes import ShapeSet


def _fem():
    corners = [(x, y, z) for x in (0, 1) for y in (0, 1) for z in (0, 1)]
    geometry = hertzforge.Geometry(node_id=list(range(1, 9)), node_xyz=corners)
    dofs = [f'{n}{d}+' for n in range(1, 9) for d in 'XYZ']
    # mode 1 displaces every node by (x+10, y+20, z+30); mode 2 purely
    # in Y by the node's own x — both fields checkable by hand
    m1 = np.array([v for (x, y, z) in corners
                   for v in (x + 10.0, y + 20.0, z + 30.0)])
    m2 = np.array([v for (x, y, _z) in corners for v in (0.0, x, 0.0)])
    shapes = ShapeSet([10.0, 20.0], [0.01, 0.02], dofs,
                      np.vstack([m1, m2]))
    return geometry, shapes


def _test_model(extra_node=False):
    node_id = [101, 102] + ([103] if extra_node else [])
    node_xyz = [[0.001, 0.0, 0.0], [1.0, 1.0, 0.999]] \
        + ([[5.0, 5.0, 5.0]] if extra_node else [])
    # node 102 measures along a local X that is global Y
    geometry = hertzforge.Geometry(
        node_id=node_id, node_xyz=node_xyz,
        node_disp_cs=[0, 5] + ([0] if extra_node else []),
        cs_id=[5], cs_type=[0], cs_name=['rotated'],
        cs_matrix=[[[0, 1, 0], [-1, 0, 0], [0, 0, 1], [0, 0, 0]]])
    dofs = ['101X+', '102X+'] + (['103X+'] if extra_node else [])
    shapes = ShapeSet([1.0, 2.0], [0.01, 0.01], dofs,
                      np.ones((2, len(dofs))))
    return geometry, shapes


def test_projection_matches_rotates_and_samples():
    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    projected, report = project_shapes(fem_shapes, fem_geometry,
                                       test_shapes, test_geometry)
    assert projected.coordinate == ['101X+', '102X+']
    assert list(projected.frequency) == [10.0, 20.0], (
        "the projected set keeps the FEM's own modal parameters")
    # 101X+ reads the global X displacement at corner (0,0,0);
    # 102X+ is a local X that is global Y, read at corner (1,1,1)
    assert projected.shape_matrix[0] == pytest.approx([10.0, 21.0])
    assert projected.shape_matrix[1] == pytest.approx([0.0, 1.0])
    assert report['matched'] == 2 and report['total'] == 2
    assert report['dropped'] == []
    assert report['worst'] == pytest.approx(0.001, rel=0.2)


def test_the_tolerance_drops_far_nodes_until_loosened():
    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model(extra_node=True)
    projected, report = project_shapes(fem_shapes, fem_geometry,
                                       test_shapes, test_geometry)
    assert '103X+' in report['dropped'], 'far outside the tolerance'
    assert projected.coordinate == ['101X+', '102X+']
    loosened, report = project_shapes(fem_shapes, fem_geometry,
                                      test_shapes, test_geometry,
                                      tolerance=2.0)
    assert loosened.coordinate == ['101X+', '102X+', '103X+']
    assert report['dropped'] == []


def test_mismatched_unit_definitions_are_refused():
    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    test_geometry.define_units('m')
    with pytest.raises(ValueError, match='units defined'):
        project_shapes(fem_shapes, fem_geometry,
                       test_shapes, test_geometry)


def test_the_reduced_test_geometry_matches_the_modal_run():
    """test_geometry.npz is the survey cut to the 13 nodes the
    Rattlesnake modal test measured — a strict subset of the model,
    with tracelines threaded through the sensors."""
    from conftest import fixture_path

    from hertzforge.core.data import parse_dof

    reduced = hertzforge.import_file(fixture_path('plate',
                                            'test_geometry.npz'))
    full = hertzforge.import_file(fixture_path('plate',
                                         'geometry.npz'))
    contents = hertzforge.import_file(fixture_path('plate',
                                             'modal_spectra.nc4'))
    measured = set()
    for obj in contents.values():
        for dof in getattr(obj, 'response_dof', []) or []:
            measured.add(parse_dof(dof)[0])
        for dof in getattr(obj, 'reference_dof', None) or []:
            measured.add(parse_dof(dof)[0])
        if hasattr(obj, 'dof_strings'):
            measured |= {parse_dof(dof)[0] for dof in obj.dof_strings()}
    measured.discard(None)
    reduced_nodes = {int(node) for node in reduced.node_id}
    assert reduced_nodes == measured
    assert reduced_nodes < {int(node) for node in full.node_id}
    assert len(reduced.drawn_lines()) >= 2, (
        'the sensors thread into drawn lines, not a dot cloud')


def test_the_report_cross_mac_binds_the_projected_set():
    """The modal template's correlation block: unbound without a second
    set, the cross-MAC against the projection when it exists."""
    import json

    from hertzforge.core.report import modal_template
    from hertzforge.core.shapes import cross_mac
    from hertzforge.report import render_html

    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    projected, _report = project_shapes(fem_shapes, fem_geometry,
                                        test_shapes, test_geometry)
    bare = {'Geometry': test_geometry, 'Shape Set': test_shapes}
    template = modal_template(bare)
    correlation = next(b for b in template.blocks
                       if b.get('mode') == 'mac' and b.get('shapes'))
    assert correlation['shapes'] == 'FEM Shapes @ Basis DOFs', (
        'bound to the name the projection produces — a slot until then')
    payload = json.loads(render_html(template, bare).split(
        'type="application/json">')[1].split('</script>')[0])
    macs = [b for b in payload['blocks'] if b['kind'] == 'mac']
    assert len(macs) == 1, 'without the projection, only the auto-MAC'

    # the old '@ Test DOFs' suffix still binds — saved projects keep it
    full = dict(bare)
    full['FEM Shapes @ Test DOFs'] = projected
    payload = json.loads(render_html(modal_template(full), full).split(
        'type="application/json">')[1].split('</script>')[0])
    macs = [b for b in payload['blocks'] if b['kind'] == 'mac']
    assert len(macs) == 2, 'the cross-MAC renders beside the auto-MAC'
    cross = macs[1]
    expected = cross_mac(test_shapes, projected)
    assert np.asarray(cross['matrix']) == pytest.approx(expected)
    assert len(cross['rows']) == test_shapes.num_shapes
    assert len(cross['columns']) == projected.num_shapes


def test_sensors_on_a_plates_surface_are_found_on_its_mid_surface():
    """A plate model's nodes are on mid-surfaces and its sensors on
    surfaces, half a thickness away. The match limit allows half the
    thickest plate beyond its fraction of the extent — without it, the
    BARC's box lost every sensor on its quarter-inch walls
    (2026-09-26)."""
    import numpy as np

    from hertzforge import fem, mesh
    from hertzforge.core.correlate import project_shapes
    from hertzforge.core.geometry import Geometry
    from hertzforge.core.shapes import ShapeSet

    plate = mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 0.25, 'plate')
    plate.group_properties = {1: fem.GroupProperties(
        fem.material('6061-T6'), 0.1)}
    dofs = [f'{n}Z+' for n in plate.node_id]
    fem_shapes = ShapeSet([10.0], [0.0], dofs,
                          plate.node_xyz[:, 0][np.newaxis] + 1.0)
    corners = np.array([[0.0, 0.0, 0.05], [1.0, 0.0, 0.05],
                        [0.0, 1.0, 0.05], [1.0, 1.0, 0.05]])
    sensors = Geometry(node_id=[1, 2, 3, 4], node_xyz=corners,
                       length_unit='m')
    test_shapes = ShapeSet([10.0], [0.0], ['1Z+', '2Z+', '3Z+', '4Z+'],
                           np.ones((1, 4)))
    _projected, report = project_shapes(fem_shapes, plate, test_shapes,
                                        sensors)
    assert report['matched'] == report['total'] == 4
    assert not report['dropped']


def test_the_match_limit_is_the_tolerance_of_the_test_geometrys_radius():
    """The limit is a fraction of the test geometry's radius about its
    centroid — the farthest test node from the middle — so it scales
    with the article and not with the units it is drawn in. Pinned by
    value: any other measure of size passed every test before
    (mutation testing, 2026-10-10)."""
    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    points = np.asarray(test_geometry.node_xyz, dtype=float)
    radius = np.linalg.norm(points - points.mean(axis=0), axis=1).max()
    _projected, report = project_shapes(fem_shapes, fem_geometry,
                                        test_shapes, test_geometry)
    assert report['limit'] == pytest.approx(0.02 * radius, rel=1e-12)
    _projected, report = project_shapes(fem_shapes, fem_geometry,
                                        test_shapes, test_geometry,
                                        tolerance=0.5)
    assert report['limit'] == pytest.approx(0.5 * radius, rel=1e-12)


def test_the_surface_allowance_is_half_the_thickest_plate():
    """Half the thickness, not the whole of it: a sensor a full
    thickness off the mid-surface is off the plate."""
    from hertzforge import fem, mesh
    from hertzforge.core.geometry import Geometry

    plate = mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 0.25, 'plate')
    plate.group_properties = {1: fem.GroupProperties(
        fem.material('6061-T6'), 0.1)}
    dofs = [f'{n}Z+' for n in plate.node_id]
    fem_shapes = ShapeSet([10.0], [0.0], dofs, np.ones((1, len(dofs))))
    corners = np.array([[0.0, 0.0, 0.05], [1.0, 0.0, 0.05],
                        [0.0, 1.0, 0.05], [1.0, 1.0, 0.05]])
    sensors = Geometry(node_id=[1, 2, 3, 4], node_xyz=corners,
                       length_unit='m')
    test_shapes = ShapeSet([10.0], [0.0], ['1Z+', '2Z+', '3Z+', '4Z+'],
                           np.ones((1, 4)))
    radius = np.linalg.norm(corners - corners.mean(axis=0), axis=1).max()
    _projected, report = project_shapes(fem_shapes, plate, test_shapes,
                                        sensors)
    assert report['limit'] == pytest.approx(0.02 * radius + 0.05, rel=1e-12)


def test_a_geometry_without_nodes_is_refused():
    fem_geometry, fem_shapes = _fem()
    _test_geometry, test_shapes = _test_model()
    empty = hertzforge.Geometry(node_id=[], node_xyz=np.empty((0, 3)))
    with pytest.raises(ValueError, match='both geometries need nodes'):
        project_shapes(fem_shapes, fem_geometry, test_shapes, empty)
    with pytest.raises(ValueError, match='both geometries need nodes'):
        project_shapes(fem_shapes, empty, test_shapes, _test_model()[0])


def test_complex_shapes_keep_their_phase_through_the_projection():
    """A complex FEM set — a damped model's modes — projects with its
    imaginary part; only a real set is read as real."""
    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    twisted = ShapeSet(fem_shapes.frequency, fem_shapes.damping,
                       fem_shapes.coordinate,
                       fem_shapes.shape_matrix * (1.0 + 1.0j))
    projected, _report = project_shapes(twisted, fem_geometry,
                                        test_shapes, test_geometry)
    assert projected.shape_matrix[0] == pytest.approx(
        np.array([10.0, 21.0]) * (1.0 + 1.0j))


def _channels():
    """The test model's channels: 101 along X, a drive at 102 along its
    rotated X read negative, and a row with no node yet."""
    return hertzforge.ChannelTable({
        'channel': [1, 2, 3],
        'node': ['101', '102', ''],
        'direction': ['X+', 'X-', ''],
        'role': ['response', 'reference', 'response'],
        'unit': ['g', 'N', 'g'],
        'channel_type': ['acceleration', 'force', 'acceleration']})


def test_a_channel_table_is_a_target_before_there_are_test_modes():
    # the BARC's FE modes at its test geometry's sensors took a
    # placeholder shape set built by script (Brandon, 2026-10-29)
    fem_geometry, fem_shapes = _fem()
    test_geometry, _shapes = _test_model()
    project = hertzforge.Project()
    project.add('FEM Geometry', fem_geometry)
    project.add('FEM Shapes', fem_shapes)
    project.add('Test Geometry', test_geometry)
    project.add('Channels', _channels())
    project.link('FEM Geometry', 'FEM Shapes')
    project.link('Test Geometry', 'Channels')
    assert 'project_onto_basis' in dict(
        project.selection_verbs('FEM Shapes', 'Channels'))
    name = project.project_onto_basis('FEM Shapes', onto='Channels')
    assert name == 'FEM Shapes @ Channels'
    projected = project[name]
    assert projected.coordinate == ['101X+', '102X-']
    # 102X- is minus the local X, which is global Y, at corner (1,1,1)
    assert projected.shape_matrix[0] == pytest.approx([10.0, -21.0])
    assert projected.shape_matrix[1] == pytest.approx([0.0, -1.0])
    assert name in project.object_group_of('Channels')
    alone = hertzforge.Project()
    alone.add('FEM Shapes', fem_shapes)
    alone.add('Channels', _channels())
    assert 'project_onto_basis' not in dict(
        alone.selection_verbs('FEM Shapes', 'Channels')), 'no geometries'


def _projection_project():
    fem_geometry, fem_shapes = _fem()
    test_geometry, _shapes = _test_model()
    project = hertzforge.Project()
    project.add('FEM Geometry', fem_geometry)
    project.add('FEM Shapes', fem_shapes)
    project.add('Test Geometry', test_geometry)
    project.add('Channels', _channels())
    project.link('FEM Geometry', 'FEM Shapes')
    project.link('Test Geometry', 'Channels')
    return project


def test_picked_channels_are_the_whole_tables_projection_at_their_dofs():
    """The drives alone (Brandon, 2026-10-31, the band-average BARC):
    the rows picked are the target, each row's values the same as in the
    projection onto the whole table, and the name says how many."""
    project = _projection_project()
    whole = project[project.project_onto_basis('FEM Shapes', onto='Channels')]
    name = project.project_onto_basis('FEM Shapes', onto='Channels',
                                      channels=[1])
    assert name == 'FEM Shapes @ Channels (1 channel)'
    picked = project[name]
    assert picked.coordinate == ['102X-']
    assert picked.shape_matrix[:, 0] == pytest.approx(whole.shape_matrix[:, 1])
    assert list(picked.frequency) == list(whole.frequency)
    assert name in project.object_group_of('Channels')
    # a row with no node is skipped, and not counted in the name
    both = project.project_onto_basis('FEM Shapes', onto='Channels',
                                      channels=[2, 0, 1, 0])
    assert both == 'FEM Shapes @ Channels (2 channels)'
    assert project[both].coordinate == ['101X+', '102X-']
    assert project[both].shape_matrix == pytest.approx(whole.shape_matrix)


def test_a_pick_with_no_dof_or_no_table_is_refused():
    project = _projection_project()
    with pytest.raises(ValueError, match='none of the picked channels has '
                                         'a node and direction'):
        project.project_onto_basis('FEM Shapes', onto='Channels', channels=[2])
    with pytest.raises(ValueError, match=r'has 3 channels; rows \[7\]'):
        project.project_onto_basis('FEM Shapes', onto='Channels',
                                   channels=[0, 7])
    project.add('Other Shapes', _fem()[1])
    with pytest.raises(ValueError, match='picks rows of a channel table; '
                                         'Other Shapes is not one'):
        project.project_onto_basis('FEM Shapes', onto='Other Shapes',
                                   channels=[0])
    assert 'FEM Shapes @ Channels (1 channel)' not in project
