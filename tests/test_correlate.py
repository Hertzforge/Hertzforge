"""Test-analysis correlation: FEM shapes projected onto test DOFs.

The FEM is a unit cube with an analytically known displacement field,
the test a couple of uniaxial sensors — one in a rotated local frame —
so the projected values are checkable by hand: matching, rotation into
the local frame, and the uniaxial dot product all pin exactly.
"""

from __future__ import annotations

import copy

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


def test_the_report_names_each_matched_pair_and_the_unmatched():
    """The pairing itself, by node id: 101 sits a millimeter from the
    cube's corner 1, 102 a millimeter from corner 8, and 103 is far
    outside the tolerance."""
    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model(extra_node=True)
    _projected, report = project_shapes(fem_shapes, fem_geometry,
                                        test_shapes, test_geometry)
    assert report['pairs'] == [(101, 1), (102, 8)]
    assert report['unmatched'] == [103]
    assert len(report['pairs']) == report['matched']


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


# ---- which set is projected -----------------------------------------------------


def _set(dofs, modes=2):
    rng = np.random.default_rng(len(dofs))
    return ShapeSet(np.arange(1.0, modes + 1), np.zeros(modes), list(dofs),
                    rng.normal(size=(modes, len(dofs))))


def test_nodes_measured_in_fewer_than_three_directions_are_counted():
    from hertzforge.core.correlate import partial_nodes

    assert partial_nodes(_set(['1X+', '1Y+', '1Z+', '2X+', '2Y-', '2Z+'])) == 0
    # a uni-axial node, a bi-axial one, and a skewed sensor named by one axis
    assert partial_nodes(_set(['1X+', '1Y+', '1Z+', '2Z+', '3X+', '3Y+', '4X-'])) == 3
    # a rotation is not a translation, and a modal coordinate is at no node
    assert partial_nodes(_set(['1X+', '1Y+', '1RZ+'])) == 1
    assert partial_nodes(_set(['1X+', '1Y+', '1Z+', '1RX+', 'M1'])) == 0


def test_the_set_with_uniaxial_nodes_is_the_one_projected_onto():
    """A node measured along one axis has no whole motion to sample at
    another set's DOFs, so the other set is projected onto it, whichever
    is named first (Brandon, 2026-11-16)."""
    from hertzforge.core.correlate import projection_order

    full = _set([f'{n}{a}+' for n in range(1, 9) for a in 'XYZ'])
    uniaxial = _set(['101X+', '102X+', '103Z+'])
    onto_second, why = projection_order(full, uniaxial)
    assert onto_second and '3 nodes measured in fewer than three directions' in why
    onto_second, why = projection_order(uniaxial, full)
    assert not onto_second and '3 nodes measured' in why
    # both whole: the one with more DOFs is projected onto the one with fewer
    small = _set([f'{n}{a}+' for n in (201, 202) for a in 'XYZ'])
    assert projection_order(full, small) == (True, 'it has the fewer DOFs')
    assert projection_order(small, full) == (False, 'it has the fewer DOFs')
    assert projection_order(small, small)[0] is False, 'as many DOFs: the second onto the first'
    # both with uni-axial nodes: neither way is exact, and it says so
    other = _set(['301X+', '302Y+', '303Z+', '304Z+'])
    onto_second, why = projection_order(other, uniaxial)
    assert onto_second and 'neither way is exact' in why


def _two_geometries():
    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    project = hertzforge.Project()
    project.add('FEM Geometry', fem_geometry)
    project.add('FEM Shapes', fem_shapes)
    project.add('Test Geometry', test_geometry)
    project.add('Test Shapes', test_shapes)
    project.link('FEM Geometry', 'FEM Shapes')
    project.link('Test Geometry', 'Test Shapes')
    return project


def test_the_mac_is_the_same_whichever_set_is_named_first():
    from hertzforge.core.shapes import cross_mac

    project = _two_geometries()
    forward = project.comparison_mac('FEM Shapes', 'Test Shapes')
    back = project.comparison_mac('Test Shapes', 'FEM Shapes')
    assert np.allclose(forward, back.T, atol=1e-12)
    # it is the model sampled at the test's DOFs, both ways
    sampled, _report = project_shapes(project['FEM Shapes'], project['FEM Geometry'],
                                      project['Test Shapes'], project['Test Geometry'])
    assert np.allclose(back, cross_mac(project['Test Shapes'], sampled))
    rows, columns, said, _report = project.compared_sets('FEM Shapes', 'Test Shapes')
    assert list(rows.coordinate) == list(project['Test Shapes'].coordinate)
    assert columns is project['Test Shapes']
    assert said.startswith('FEM Shapes projected onto Test Shapes: it has 2 nodes')
    rows, columns, said, _report = project.compared_sets('Test Shapes', 'FEM Shapes')
    assert rows is project['Test Shapes']
    assert said.startswith('FEM Shapes projected onto Test Shapes')
    # on one geometry nothing is projected
    assert project.compared_sets('FEM Shapes', 'FEM Shapes')[2:] == ('', None)


def test_projecting_a_uniaxial_set_onto_a_whole_one_is_refused():
    project = _two_geometries()
    with pytest.raises(ValueError, match='Test Shapes cannot be projected onto FEM Shapes: '
                                         'Test Shapes has 2 nodes measured in fewer than '
                                         r'three directions\. Project FEM Shapes onto Test Shapes'):
        project.project_onto_basis('Test Shapes', onto='FEM Shapes')
    made = project.project_onto_basis('FEM Shapes', onto='Test Shapes')
    assert list(project[made].coordinate) == list(project['Test Shapes'].coordinate)


def test_a_distance_is_the_match_limit_itself():
    """Node 101 sits 1 mm from the cube's corner and 102 1 mm from another: a limit a
    little above that matches both, one a little below matches neither, and the limit the
    report gives is the distance asked for, nothing added."""
    from hertzforge.core.correlate import NoNodeMatched

    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    projected, report = project_shapes(fem_shapes, fem_geometry, test_shapes,
                                       test_geometry, distance=0.0011)
    assert report['limit'] == 0.0011
    assert projected.coordinate == ['101X+', '102X+']
    with pytest.raises(NoNodeMatched) as error:
        project_shapes(fem_shapes, fem_geometry, test_shapes, test_geometry,
                       distance=0.0009)
    assert error.value.report['limit'] == 0.0009
    # the distance takes the place of the share of the extent, however large that is
    _projected, report = project_shapes(fem_shapes, fem_geometry, test_shapes,
                                        test_geometry, tolerance=50.0, distance=0.0011)
    assert report['limit'] == 0.0011


def test_without_a_distance_the_limit_is_match_distance():
    from hertzforge.core.correlate import match_distance

    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    _projected, report = project_shapes(fem_shapes, fem_geometry, test_shapes,
                                        test_geometry)
    # two test nodes: the extent is half the distance between them
    extent = 0.5 * float(np.linalg.norm(np.subtract([1.0, 1.0, 0.999], [0.001, 0.0, 0.0])))
    assert report['limit'] == pytest.approx(0.02 * extent, rel=1e-12)
    assert match_distance(fem_geometry, test_geometry) == report['limit']
    assert match_distance(fem_geometry, test_geometry, 0.1) == pytest.approx(
        0.1 * extent, rel=1e-12)


@pytest.mark.parametrize('distance', [0.0, -0.001, float('nan'), float('inf')])
def test_a_distance_that_is_not_above_zero_is_refused(distance):
    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    with pytest.raises(ValueError, match='node match distance is .*above zero'):
        project_shapes(fem_shapes, fem_geometry, test_shapes, test_geometry,
                       distance=distance)


def test_a_shape_set_projects_onto_every_node_of_another_geometry():
    """The cube's modes put on the test geometry: X, Y and Z at each node, node 102's in
    its rotated frame, whose X is global Y and whose Y is minus global X. Mode 1 moves a
    node by (x+10, y+20, z+30), so 101 near (0, 0, 0) reads (10, 20, 30) and 102 near
    (1, 1, 1) reads (21, -11, 31)."""
    project = _two_geometries()
    assert 'project_onto_basis' in dict(project.selection_verbs('FEM Shapes',
                                                                'Test Geometry'))
    name = project.project_onto_basis('FEM Shapes', onto='Test Geometry')
    assert name == 'FEM Shapes @ Test Geometry'
    made = project[name]
    assert made.coordinate == ['101X+', '101Y+', '101Z+', '102X+', '102Y+', '102Z+']
    assert made.shape_matrix[0] == pytest.approx([10.0, 20.0, 30.0, 21.0, -11.0, 31.0])
    assert made.shape_matrix[1] == pytest.approx([0.0, 0.0, 0.0, 1.0, 0.0, 0.0])
    assert list(made.frequency) == [10.0, 20.0]
    assert name in project.object_group_of('Test Geometry')
    assert made.projection_report['matched'] == 2


def test_a_geometry_takes_the_distance_and_drops_a_node_beyond_it():
    project = _two_geometries()
    project['Test Geometry'] = _test_model(extra_node=True)[0]
    made = project[project.project_onto_basis('FEM Shapes', onto='Test Geometry',
                                              distance=0.0011)]
    assert made.projection_report['limit'] == 0.0011
    assert made.projection_report['unmatched'] == [103]
    assert [str(d) for d in made.coordinate if str(d).startswith('103')] == []


def test_what_cannot_go_onto_a_geometry_is_refused_and_not_offered():
    from hertzforge.project import projection_why

    project = _two_geometries()
    project.add('Empty', hertzforge.Geometry(node_id=[], node_xyz=np.zeros((0, 3))))
    cases = [
        ('FEM Shapes', 'FEM Geometry', 'FEM Shapes is on FEM Geometry already'),
        # each test node holds one direction: its other two would be taken as no motion
        ('Test Shapes', 'FEM Geometry',
         'Test Shapes has 2 nodes measured in fewer than three directions'),
        ('FEM Shapes', 'Empty', 'Empty has no nodes')]
    for source, onto, why in cases:
        assert projection_why(project, source, onto) == why
        assert 'project_onto_basis' not in dict(project.selection_verbs(source, onto))
        with pytest.raises(ValueError, match=f'{source} cannot be projected onto {onto}: {why}'):
            project.project_onto_basis(source, onto=onto)
    assert projection_why(project, 'FEM Shapes', 'Test Geometry') == ''


def test_the_match_distance_is_said_for_the_pair_it_projects():
    from hertzforge.core.correlate import match_distance

    project = _two_geometries()
    onto_set = project.match_distance('FEM Shapes', 'Test Shapes')
    assert onto_set == match_distance(project['FEM Geometry'], project['Test Geometry'])
    assert project.match_distance('FEM Shapes', 'Test Geometry') == onto_set
    # the other way is the cube's size, not the test's
    assert project.match_distance('Test Shapes', 'FEM Shapes') == pytest.approx(
        0.02 * np.sqrt(3) / 2, rel=1e-12)
    made = project.project_onto_basis('FEM Shapes', onto='Test Shapes')
    assert project[made].projection_report['limit'] == onto_set
    project['Test Geometry'].define_units('m')
    assert project.match_distance('FEM Shapes', 'Test Shapes') is None, \
        'one geometry in meters and one in its own units overlay in no frame'


def test_a_pair_is_projected_the_way_it_is_set():
    """Unset, the cube's modes go onto the test's (the test holds one direction at each node)
    within 2 % of the test geometry's size; set, with the distance asked for, and the line that
    replays it is in the journal."""
    project = _two_geometries()
    assert project.projection_of('Test Shapes', 'FEM Shapes') == ('FEM Shapes', 'Test Shapes',
                                                                  None)
    default = project.compared_sets('FEM Shapes', 'Test Shapes')[3]['limit']
    project.set_projection('Test Shapes', 'FEM Shapes', distance=0.0011)
    assert project.projection_of('FEM Shapes', 'Test Shapes') == ('FEM Shapes', 'Test Shapes',
                                                                  0.0011)
    _rows, _columns, said, report = project.compared_sets('Test Shapes', 'FEM Shapes')
    assert report['limit'] == 0.0011 != default
    assert said.startswith('FEM Shapes projected onto Test Shapes')
    assert project.journal[-1] == ("project.set_projection('Test Shapes', 'FEM Shapes', "
                                   'distance=0.0011)')
    # a distance asked for in the call still takes the place of the one set
    assert project.compared_sets('FEM Shapes', 'Test Shapes', distance=0.002)[3]['limit'] == 0.002


def test_two_sets_that_both_hold_every_direction_go_the_way_they_are_set():
    project = _two_geometries()
    project.add('Model B', copy.deepcopy(project['FEM Geometry']))
    project.add('Modes B', copy.deepcopy(project['FEM Shapes']))
    project.link('Model B', 'Modes B')
    chosen = project.projection_of('FEM Shapes', 'Modes B')[0]
    other = 'Modes B' if chosen == 'FEM Shapes' else 'FEM Shapes'
    project.set_projection('FEM Shapes', 'Modes B', source=other)
    assert project.projection_of('Modes B', 'FEM Shapes')[0] == other
    said = project.compared_sets('FEM Shapes', 'Modes B')[2]
    assert said == f'{other} projected onto {chosen}: the direction set in Project'


@pytest.mark.parametrize(('first', 'second', 'given', 'refused'), [
    ('Test Shapes', 'FEM Shapes', {'source': 'Test Shapes'},
     'Test Shapes cannot be projected onto FEM Shapes: Test Shapes has 2 nodes'),
    ('Test Shapes', 'FEM Shapes', {'source': 'FEM Geometry'}, 'is Test Shapes or FEM Shapes'),
    ('Test Shapes', 'FEM Shapes', {'distance': -1.0}, 'must be above zero'),
    ('FEM Shapes', 'FEM Geometry', {}, 'FEM Shapes is on FEM Geometry already'),
    ('FEM Shapes', 'Test Geometry', {'source': 'Test Geometry'},
     'Test Geometry is a geometry, with no modes to bring onto the DOFs of FEM Shapes'),
    ('FEM Shapes', 'Test Geometry', {'method': 'expansion'},
     'Test Geometry holds no modes to expand through$'),
    ('Test Shapes', 'FEM Geometry', {'method': 'expansion'},
     'FEM Geometry holds no modes to expand through; pick Test Shapes with FEM Shapes'),
    ('FEM Geometry', 'Test Geometry', {}, 'Test Geometry is not a shape set'),
    ('FEM Shapes', 'RC Channels', {}, 'RC Channels is not a shape set'),
])
def test_a_projection_that_cannot_be_made_is_refused_and_not_kept(first, second, given, refused):
    project = _two_geometries()
    project.add('RC Channels', _channels())
    with pytest.raises((ValueError, TypeError), match=refused):
        project.set_projection(first, second, **given)
    assert project.projections == {}


def test_a_set_and_a_geometry_it_is_not_on_keep_a_reduction_onto_its_nodes():
    """The set is the one moved, onto the geometry's nodes, at the distance kept; the pair is
    kept in the file and follows a rename like two sets."""
    project = _two_geometries()
    assert project.projection_of('Test Geometry', 'FEM Shapes') == (
        'FEM Shapes', 'Test Geometry', None)
    project.set_projection('Test Geometry', 'FEM Shapes', distance=0.004)
    assert project.projection_of('FEM Shapes', 'Test Geometry') == (
        'FEM Shapes', 'Test Geometry', 0.004)
    assert project.projection_method('FEM Shapes', 'Test Geometry') == 'reduction'
    project.rename('Test Geometry', 'Bench')
    assert project.projection_of('FEM Shapes', 'Bench')[2] == 0.004


def test_expansion_goes_through_the_set_named_never_one_found_by_a_link():
    project = _two_geometries()
    with pytest.raises(ValueError, match='FEM Geometry holds no modes to expand through'):
        project.expand_modes('Test Shapes', onto='FEM Geometry')
    added = project.expand_modes('Test Shapes', onto='FEM Shapes')
    assert project.geometry_for(added)[0] == 'FEM Geometry'


def test_two_sets_on_one_geometry_have_no_projection_to_set():
    project = _two_geometries()
    project.add('Other Modes', copy.deepcopy(project['FEM Shapes']))
    project.link('FEM Geometry', 'Other Modes')
    with pytest.raises(ValueError, match='Both are on FEM Geometry, so there is nothing'):
        project.set_projection('FEM Shapes', 'Other Modes')


def test_the_projection_is_kept_in_the_file_and_follows_a_rename(tmp_path):
    project = _two_geometries()
    project.set_projection('Test Shapes', 'FEM Shapes', distance=0.0011)
    path = str(tmp_path / 'kept.hforge')
    project.save(path)
    opened = hertzforge.Project.open(path)
    assert opened.projection_of('Test Shapes', 'FEM Shapes') == ('FEM Shapes', 'Test Shapes',
                                                                 0.0011)
    opened.rename('FEM Shapes', 'Model Modes')
    assert opened.projection_of('Test Shapes', 'Model Modes') == ('Model Modes', 'Test Shapes',
                                                                  0.0011)
    assert list(opened.projections) == [('Model Modes', 'Test Shapes')], 'the old name is gone'
    opened.remove('Test Shapes')
    assert opened.projections == {}, 'gone with either set'


def test_a_file_written_before_projections_were_kept_opens_with_none(tmp_path):
    import h5py

    project = _two_geometries()
    project.set_projection('Test Shapes', 'FEM Shapes', distance=0.0011)
    path = str(tmp_path / 'older.hforge')
    project.save(path)
    with h5py.File(path, 'a') as f:
        del f.attrs['projections']
    opened = hertzforge.Project.open(path)
    assert opened.projections == {}
    assert opened.projection_of('Test Shapes', 'FEM Shapes')[2] is None


def _known_test(fem_shapes, fem_geometry, test_geometry, test_shapes, combination):
    """A test set that is `combination` of the model's modes at the test DOFs, so the exact
    expansion is `combination` of the model's modes at every DOF."""
    reduced, _report = project_shapes(fem_shapes, fem_geometry, test_shapes, test_geometry)
    values = np.asarray(combination) @ np.asarray(reduced.shape_matrix)
    return ShapeSet([30.0, 40.0], [0.01, 0.02], list(reduced.coordinate), values)


@pytest.mark.parametrize('combination', [[[1.0, 2.0], [3.0, -1.0]],
                                         [[1.0 + 2.0j, 0.5], [-1.0j, 2.0 - 1.0j]]])
def test_a_test_set_in_the_models_span_expands_to_it_exactly(combination):
    """Two model modes, two test DOFs: each test mode is a known combination of the modes at
    the test DOFs, and the expansion is that combination at all 24 model DOFs, phase kept."""
    from hertzforge.core.correlate import expand_shapes

    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    known = _known_test(fem_shapes, fem_geometry, test_geometry, test_shapes, combination)
    expanded, report = expand_shapes(known, test_geometry, fem_shapes, fem_geometry,
                                     modes=[0, 1])
    assert list(expanded.coordinate) == list(fem_shapes.coordinate)
    assert np.allclose(expanded.shape_matrix,
                       np.asarray(combination) @ np.asarray(fem_shapes.shape_matrix))
    assert list(expanded.frequency) == [30.0, 40.0], "the test's own modal parameters"
    assert report['modes'] == [0, 1] and report['matched'] == 2
    assert report['residual'] == pytest.approx([0.0, 0.0], abs=1e-12)
    assert report['condition'] > 1


def test_a_test_mode_outside_the_span_is_the_nearest_fit_and_says_how_far():
    """With one model mode, a test mode that is not a multiple of it is fitted by least
    squares: its residual at the test DOFs is what the fit leaves, over the mode's size."""
    from hertzforge.core.correlate import expand_shapes

    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    known = _known_test(fem_shapes, fem_geometry, test_geometry, test_shapes,
                        [[1.0, 0.0], [1.0, 1.0]])
    expanded, report = expand_shapes(known, test_geometry, fem_shapes, fem_geometry, modes=[0])
    phi = np.array([10.0, 21.0])  # mode 1 at the two test DOFs
    psi = np.asarray(known.shape_matrix)
    scale = psi @ phi / (phi @ phi)
    left = psi - np.outer(scale, phi)
    assert report['residual'] == pytest.approx(
        np.linalg.norm(left, axis=1) / np.linalg.norm(psi, axis=1))
    assert report['residual'][0] == pytest.approx(0.0, abs=1e-12)
    assert np.allclose(expanded.shape_matrix,
                       np.outer(scale, np.asarray(fem_shapes.shape_matrix)[0]))


def test_more_modes_than_test_dofs_are_refused():
    from hertzforge.core.correlate import expand_shapes

    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    three = ShapeSet([10.0, 20.0, 30.0], [0.0] * 3, fem_shapes.coordinate,
                     np.vstack([fem_shapes.shape_matrix, np.ones(24)]))
    with pytest.raises(ValueError, match='3 model modes for 2 test DOFs matched'):
        expand_shapes(test_shapes, test_geometry, three, fem_geometry, modes=[0, 1, 2])
    with pytest.raises(ValueError, match='the model has 3 modes; \\[5\\] are not among them'):
        expand_shapes(test_shapes, test_geometry, three, fem_geometry, modes=[4])


def test_the_default_basis_is_the_band_up_to_one_and_a_half_times_the_test_capped_at_its_dofs():
    from hertzforge.core.correlate import expansion_modes

    fem = [0.0, 0.0, 100.0, 140.0, 160.0, 400.0]
    assert expansion_modes(fem, [50.0, 100.0], 10) == [0, 1, 2, 3]
    assert expansion_modes(fem, [50.0, 100.0], 3) == [0, 1, 2], 'the lowest, no more than DOFs'
    assert expansion_modes([300.0, 100.0, 200.0], [100.0], 5) == [1], 'by frequency, not order'
    assert expansion_modes([300.0, 100.0, 200.0], [10.0], 2) == [1, 2], \
        'none in the band: the lowest, as many as the DOFs'
