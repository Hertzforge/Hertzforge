"""The BARC's removable component on a fixture plate (`hertzforge.demo.rc_plate`).

What is pinned: the component is the BARC's as `barc.solid_geometry` builds it
to match the wiki's calibrated model (2026-11-10), at its sixteenth of an inch on a plate
at an eighth, each of the plate's nodes under a foot tied to the foot's node
there; its drives are at the plate's corners pointing in, its accelerometers
are on the component's surfaces in their frames, and the project links them
as the Basis.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.demo import barc, rc_plate

INCH = 0.0254


@pytest.fixture(scope='module')
def built():
    return rc_plate.project()


def test_the_plate_carries_the_component_joined_at_its_bolts(built):
    model = built['RC Plate']
    assert len(model.node_id) == 69402
    assert list(model.group_name) == ['plate', 'right channel', 'left channel', 'beam',
                                      'foot bolts', 'top bolts']
    assert sorted(mate.name for mate in model.mates) == [
        'left foot bolts', 'left top bolts', 'right foot bolts', 'right top bolts']
    # the plate's 9 x 9 under each foot, each on a node of the foot's 17 x 17
    plate = int(model.group_id[0])
    for mate in model.mates:
        if 'foot' in mate.name:
            assert mate.groups[0] == plate
            assert (len(mate.nodes_a), len(mate.nodes_b)) == (81, 289)
            a = model.node_xyz[model.node_index(list(mate.nodes_a))]
            b = model.node_xyz[model.node_index(list(mate.nodes_b))]
            gap = np.linalg.norm(a[:, None] - b[None], axis=2).min(axis=1) / INCH
            assert gap.max() < 0.01
    inch = model.node_xyz / INCH
    # the plate's top is the BARC's box top, the component at its own coordinates
    assert inch[:, 1].min() == pytest.approx(2.0) and inch[:, 1].max() == pytest.approx(
        barc.HEAD_Y['top'])
    assert inch[:, 0].min() == pytest.approx(-5.0) and inch[:, 2].max() == pytest.approx(6.5)


def test_the_drives_are_at_the_corners_and_point_in():
    drives = rc_plate.drives()
    assert len(drives) == 12
    (cx, cy, cz), (wx, _wy, wz) = rc_plate.PLATE
    corners = set()
    for label, ((x, y, z), direction) in drives.items():
        # on a face, INSET in from both side edges: each corner's three meet at one point
        corners.add((round(cx + np.sign(x - cx) * (wx / 2 - rc_plate.INSET), 9),
                     round(cz + np.sign(z - cz) * (wz / 2 - rc_plate.INSET), 9)))
        assert abs(x - cx) in (wx / 2, wx / 2 - rc_plate.INSET), label
        assert abs(z - cz) in (wz / 2, wz / 2 - rc_plate.INSET), label
        axis, sign = 'XYZ'.index(direction[0]), direction[1]
        here = (x - cx, y - cy, z - cz)[axis]
        # along the face's inward normal: from the -X face it pushes +X, and so on
        assert (here < 0) == (sign == '+'), label
    assert len(corners) == 4


def test_the_channels_are_the_drives_and_the_accelerometers_in_their_frames(built):
    geometry = built['RC Plate Test Geometry']
    table = built['RC Plate Channels']
    assert list(geometry.node_id) == [*range(rc_plate.DRIVE_IDS, rc_plate.DRIVE_IDS + 12),
                                      *range(rc_plate.ACCEL_IDS, rc_plate.ACCEL_IDS + 16)]
    roles = list(table['role'])
    assert roles.count('reference') == 12 and roles.count('response') == 48
    units = {(role, unit) for role, unit in zip(roles, table['unit'], strict=True)}
    assert units == {('reference', 'lbf'), ('response', 'g')}
    # each accelerometer measures in its surface's frame, +Z the outward normal
    for node, (_label, (point, normal)) in zip(range(rc_plate.ACCEL_IDS, rc_plate.ACCEL_IDS + 16),
                                                rc_plate.CONTROLS.items(), strict=True):
        k = list(geometry.node_id).index(node)
        assert geometry.node_xyz[k] / INCH == pytest.approx(point, abs=rc_plate.NEAR)
        frame = np.asarray(geometry.cs_matrix)[int(geometry.node_disp_cs[k]) - 1]
        assert frame[2] == pytest.approx(normal, abs=1e-12)
    assert built.object_groups == [{'members': ['RC Plate Test Geometry', 'RC Plate Channels'],
                                    'role': 'Basis'}]


def test_a_place_with_no_node_is_refused_in_words(built):
    with pytest.raises(ValueError, match='no node at'):
        rc_plate.node_at(built['RC Plate'], (0.05, 3.0, 0.0))


#: the first five elastic modes as built on 2026-11-10, Hz
CHECKED = [413.72, 1116.62, 1217.77, 1347.74, 1762.50]


@pytest.mark.slow
def test_the_solved_example_is_the_studys_system():
    """Two and a half minutes and about 18 GB: every mode to 10 kHz and their
    projection."""
    solved = rc_plate.project(solved=True)
    shapes = solved['RC Plate Modes']
    assert shapes.num_shapes == 38 and int(np.sum(shapes.frequency < 1.0)) == 6
    elastic = [f for f in shapes.frequency if f > 1.0][:5]
    assert np.allclose(elastic, CHECKED, rtol=1e-4), np.round(elastic, 2)
    report = solved['RC Plate Modes @ Test DOFs'].projection_report
    assert report['matched'] == report['total'] == 28
