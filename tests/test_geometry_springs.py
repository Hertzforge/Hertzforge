"""Springs and ground on a geometry (Brandon, 2026-10-08: a spring is a
kind of line element, ground a kind of point, so the two-beam
substructuring cases can be built in the window as well as by script).

A group of two-node lines given a stiffness in some of the six global
directions is a set of springs, `Model.add_spring` per direction; a
group of points given a stiffness is springs to ground; a group of
points given ground holds its nodes in all six directions."""

from __future__ import annotations

import math

import pytest
from test_fem import ALUMINUM
from test_springs import ROD, _two_beams, _whole_beam

import hertzforge
from hertzforge.core.fem import GroupProperties, Model

POINT, LINE = 161, 21

SIX = ('X+', 'Y+', 'Z+', 'RX+', 'RY+', 'RZ+')


def _beams_geometry(model):
    """A model's beams as a geometry, every beam group given the rod."""
    geometry = model.geometry()
    geometry.group_properties = {int(g): GroupProperties(ALUMINUM, section=ROD)
                                 for g in geometry.group_id}
    return geometry


def _group(geometry, name, nodes, code, props):
    """A new element group of one element per node list, given `props`."""
    group = geometry.add_group(name)
    geometry.add_elements(nodes, [code] * len(nodes), [group] * len(nodes))
    geometry.group_properties[group] = props
    return group


def test_a_spring_line_is_the_springs_add_spring_makes():
    """The two half-beams joined at their coincident nodes by a spring
    group of five directions solve to the model built with add_spring,
    to the last digit: one line, five springs."""
    joints = ('X+', 'Y+', 'Z+', 'RX+', 'RZ+')
    by_script = _two_beams(joints, stiffness=1e5)
    halves = _two_beams((), stiffness=1e5)
    geometry = _beams_geometry(halves)
    stiffness = tuple(1e5 if d in joints else None for d in SIX)
    _group(geometry, 'joint', [[104, 200]], LINE,
           GroupProperties(stiffness=stiffness))
    built = Model.from_geometry(geometry)
    assert len(built.springs) == 5
    assert built.eigensolution(num_modes=14).frequency == pytest.approx(
        by_script.eigensolution(num_modes=14).frequency, rel=1e-9, abs=1e-6)


def test_a_ground_point_holds_its_node_as_fixed_does():
    """A ground point on a beam's end node makes the cantilever, the
    same modes as the free beam solved with that node fixed."""
    free = _whole_beam()
    geometry = _beams_geometry(free)
    _group(geometry, 'support', [[1]], POINT, GroupProperties(ground=True))
    built = Model.from_geometry(geometry)
    assert built.grounds == {1: (0, 1, 2, 3, 4, 5)}
    expected = free.eigensolution(num_modes=8, fixed=['1']).frequency
    got = built.eigensolution(num_modes=8).frequency
    assert got == pytest.approx(expected, rel=1e-9)
    assert got[0] > 0.0, 'no rigid-body modes left'


def test_a_spring_to_ground_is_a_point_or_a_line_to_a_ground_point():
    """Two ways to draw one thing: a point in a group of springs at the
    beam's end, or a line from that end to a ground point at a node of
    its own. Both are add_spring with no second end."""
    k = 4e3
    expected_model = _whole_beam()
    for node in (1, 9):
        expected_model.add_spring(f'{node}Z+', None, k)
    expected = expected_model.eigensolution(num_modes=10).frequency

    on_points = _beams_geometry(_whole_beam())
    _group(on_points, 'end springs', [[1], [9]], POINT,
           GroupProperties(stiffness=(None, None, k, None, None, None)))
    assert Model.from_geometry(on_points).eigensolution(
        num_modes=10).frequency == pytest.approx(expected, rel=1e-9, abs=1e-6)

    to_ground = _beams_geometry(_whole_beam())
    far = [to_ground.add_node(to_ground.node_xyz[to_ground.node_index([n])[0]])
           for n in (1, 9)]
    _group(to_ground, 'ground', [[n] for n in far], POINT,
           GroupProperties(ground=True))
    _group(to_ground, 'end springs', [[1, far[0]], [9, far[1]]], LINE,
           GroupProperties(stiffness=(None, None, k, None, None, None)))
    assert Model.from_geometry(to_ground).eigensolution(
        num_modes=10).frequency == pytest.approx(expected, rel=1e-9, abs=1e-6)


def test_a_mass_on_springs_rings_where_the_textbook_says():
    """A point mass hung from a ground point by a spring line stiff in
    X, Y and Z: three modes at sqrt(k/m)/2pi. Its node is no beam's, so
    its rotations have nothing to act on and are grounded, as a solid's
    are; left free, the solve could not factor."""
    m, k = 0.5, 2e4
    geometry = hertzforge.Geometry(
        node_id=[1, 2], node_xyz=[[0, 0, 0], [0, 0, 0.1]], length_unit='m')
    _group(geometry, 'ground', [[1]], POINT, GroupProperties(ground=True))
    _group(geometry, 'mass', [[2]], POINT, GroupProperties(mass=m))
    _group(geometry, 'mount', [[1, 2]], LINE,
           GroupProperties(stiffness=(k, k, k, None, None, None)))
    frequency = Model.from_geometry(geometry).eigensolution().frequency
    assert frequency == pytest.approx([math.sqrt(k / m) / (2 * math.pi)] * 3)


def test_springs_and_ground_are_refused_by_what_is_wrong():
    geometry = _beams_geometry(_whole_beam())
    group = _group(geometry, 'joint', [[1, 2]], LINE,
                   GroupProperties(stiffness=(None,) * 6))
    with pytest.raises(ValueError, match='no stiffness in any direction'):
        Model.from_geometry(geometry)
    geometry.group_properties[group] = GroupProperties(ground=True)
    with pytest.raises(ValueError, match='a ground group holds point elements'):
        Model.from_geometry(geometry)
    beams = int(geometry.group_id[0])
    geometry.group_properties[group] = GroupProperties(ALUMINUM, section=ROD)
    geometry.group_properties[beams] = GroupProperties(
        stiffness=(1.0,) + (None,) * 5)
    plate = hertzforge.Geometry(
        node_id=[1, 2, 3, 4], node_xyz=[[0, 0, 0], [1, 0, 0], [1, 1, 0],
                                        [0, 1, 0]], length_unit='m')
    _group(plate, 'skin', [[1, 2, 3, 4]], 44,
           GroupProperties(stiffness=(1.0,) + (None,) * 5))
    with pytest.raises(ValueError, match='a group of springs holds two-node'):
        Model.from_geometry(plate)
    # a rotation spring needs a node something rotational holds, or ground
    hung = hertzforge.Geometry(
        node_id=[1, 2], node_xyz=[[0, 0, 0], [0, 0, 0.1]], length_unit='m')
    _group(hung, 'ground', [[1]], POINT, GroupProperties(ground=True))
    _group(hung, 'mass', [[2]], POINT, GroupProperties(mass=1.0))
    _group(hung, 'mount', [[1, 2]], LINE,
           GroupProperties(stiffness=(1e3, 1e3, 1e3, None, 5.0, None)))
    with pytest.raises(ValueError, match='spring acts on a rotation of node 2'):
        Model.from_geometry(hung).eigensolution()


def test_springs_and_ground_ride_the_native_file(tmp_path):
    geometry = _beams_geometry(_whole_beam())
    springs = _group(geometry, 'springs', [[1]], POINT,
                     GroupProperties(stiffness=(None, None, 4e3, None, 7.0,
                                                None)))
    ground = _group(geometry, 'ground', [[9]], POINT,
                    GroupProperties(ground=True))
    hertzforge.save(geometry, tmp_path / 'g.vdyn')
    back = hertzforge.load(tmp_path / 'g.vdyn')
    assert back.group_properties[springs] == geometry.group_properties[springs]
    assert back.group_properties[ground] == GroupProperties(ground=True)
    assert back.group_properties[ground].kind == 'ground'
