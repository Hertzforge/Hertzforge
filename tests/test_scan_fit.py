"""Accelerometers fitted to the points measured on them."""

import os

import numpy as np
import pytest
from scipy.spatial.transform import Rotation

from hertzforge import Project
from hertzforge.core.scan_fit import (
    fit_cylinder,
    fit_plane,
    fit_sensors,
    fit_triax,
    fit_uniaxial,
    parse_feature,
)
from hertzforge.io.point_files import points_geometry

MM = 1e-3


def _pose(seed):
    """A frame (rows X, Y, Z) and a center, anywhere."""
    rng = np.random.default_rng(seed)
    return Rotation.random(random_state=seed).as_matrix().T, rng.uniform(-0.5, 0.5, 3)


def _face(frame, center, side, label, count, rng, noise=0.0):
    """Points on one face of a cube: `label` 1 is the X+ face, -2 the Y- face."""
    k, sign = abs(label) - 1, np.sign(label)
    spread = rng.uniform(-0.4, 0.4, (count, 3)) * side
    spread[:, k] = sign * side / 2 + rng.normal(0, noise, count) if noise else sign * side / 2
    return center + spread @ frame


def _cube(seed, labels=(1, 2, 3), side=10.2 * MM, count=6, noise=0.0):
    rng = np.random.default_rng(seed)
    frame, center = _pose(seed)
    faces = {str(label): _face(frame, center, side, label, count, rng, noise) for label in labels}
    return frame, center, side, faces


def _can(seed, radius=4 * MM, height=6 * MM, arc=200.0, band=(0.2, 0.7), noise=0.0, label=3):
    """Points on the side and the top of a cylinder whose `label` axis
    points out of its top."""
    rng = np.random.default_rng(seed)
    frame, center = _pose(seed)
    k, sign = abs(label) - 1, np.sign(label)
    up = sign * frame[k]
    a, b = frame[(k + 1) % 3], frame[(k + 2) % 3]
    angle = np.radians(rng.uniform(0, arc, 40))
    along = rng.uniform(*band, 40) * height - height / 2
    reach = radius + (rng.normal(0, noise, 40) if noise else 0.0)
    side = center + np.outer(along, up) + (np.cos(angle) * reach)[:, None] * a \
        + (np.sin(angle) * reach)[:, None] * b
    spot = rng.uniform(-0.6, 0.6, (6, 2)) * radius
    top = center + (height / 2) * up + np.outer(spot[:, 0], a) + np.outer(spot[:, 1], b)
    return frame, center, radius, height, side, top


# ---- one surface -------------------------------------------------------------

def test_a_plane_is_found_with_its_residuals():
    rng = np.random.default_rng(0)
    frame, center = _pose(0)
    flat = rng.uniform(-1, 1, (50, 3)) * [0.01, 0.01, 0.0]
    flat[:, 2] = rng.normal(0, 2e-6, 50)
    plane = fit_plane(center + flat @ frame)
    assert abs(plane.normal @ frame[2]) == pytest.approx(1.0, abs=1e-6)
    assert plane.rms == pytest.approx(2e-6, rel=0.3) and plane.worst >= plane.rms
    assert plane.count == 50
    with pytest.raises(ValueError, match='three points'):
        fit_plane([[0, 0, 0], [1, 0, 0]])


@pytest.mark.parametrize('arc', [360.0, 200.0, 90.0])
def test_a_cylinder_is_found_from_part_of_its_side(arc):
    frame, center, radius, height, side, _top = _can(4, arc=arc)
    cylinder = fit_cylinder(side)
    assert abs(cylinder.axis @ frame[2]) == pytest.approx(1.0, abs=1e-8)
    assert cylinder.radius == pytest.approx(radius, rel=1e-6)
    off = (center - cylinder.point) - ((center - cylinder.point) @ cylinder.axis) * cylinder.axis
    assert np.linalg.norm(off) < 1e-8
    assert cylinder.rms < 1e-9
    # 40 points spread over the arc cover most of it
    assert 0.8 * min(arc, 350) < cylinder.arc <= arc + 1e-6
    assert cylinder.span[1] - cylinder.span[0] == pytest.approx(0.5 * height, rel=0.1)
    with pytest.raises(ValueError, match='five points'):
        fit_cylinder(side[:4])


def test_a_short_band_is_found_with_the_tops_normal_as_a_hint():
    """A side measured over a tenth of the radius in height, a quarter of
    the way round: the direction the points spread least in is not the
    axis, and the top's normal starts the fit near it."""
    frame, _center, radius, _height, side, _top = _can(7, arc=90.0, band=(0.45, 0.5))
    cylinder = fit_cylinder(side, axis_hint=frame[2])
    assert cylinder.radius == pytest.approx(radius, rel=1e-5)
    assert abs(cylinder.axis @ frame[2]) == pytest.approx(1.0, abs=1e-8)


# ---- a tri-axial accelerometer ----------------------------------------------

@pytest.mark.parametrize('labels', [(1, 2, 3), (-1, 2, 3), (1, -2, -3), (-1, -2, -3), (3, 1, 2)])
def test_three_faces_give_the_frame_the_corner_and_the_center(labels):
    frame, center, side, faces = _cube(11, labels)
    fit = fit_triax('31', faces, meters=1.0)
    assert fit.placed and not fit.flags and fit.kind == 'triax'
    np.testing.assert_allclose(fit.frame, frame, atol=1e-9)
    assert set(fit.axes) == set(labels)
    corner = center + sum(np.sign(k) * (side / 2) * frame[abs(k) - 1] for k in labels)
    np.testing.assert_allclose(fit.anchor, corner, atol=1e-9)
    np.testing.assert_allclose(fit.center(side), center, atol=1e-9)
    np.testing.assert_allclose(fit.center([side, side, side]), center, atol=1e-9)
    # the points reach at most 0.9 of the side from the corner
    assert 0.5 * side < fit.extent <= 0.9 * side + 1e-12
    assert fit.moved < 1e-4 and fit.direction is None


@pytest.mark.parametrize('labels', [(1, 2), (1, 3), (2, 3), (-1, 3), (3, -2)])
def test_two_faces_give_the_frame_and_a_center_along_their_edge(labels):
    frame, center, side, faces = _cube(12, labels, count=40)
    fit = fit_triax('32', faces)
    assert fit.placed and not fit.flags
    np.testing.assert_allclose(fit.frame, frame, atol=1e-9)
    found = fit.center(side)
    missing = 5 - abs(labels[0]) - abs(labels[1])
    # square to the unmeasured axis the center is exact; along it, it is
    # the middle of where the points lie, within a tenth of the side
    off = (found - center) @ frame.T
    assert abs(off[missing]) < 0.1 * side
    np.testing.assert_allclose(np.delete(off, missing), 0.0, atol=1e-9)


def test_a_box_that_is_not_a_cube_takes_a_size_for_each_axis():
    rng = np.random.default_rng(5)
    frame, center = _pose(5)
    sizes = np.array([14.0, 10.0, 6.0]) * MM
    faces = {}
    for label in (1, 2, 3):
        k = label - 1
        spread = rng.uniform(-0.4, 0.4, (6, 3)) * sizes
        spread[:, k] = sizes[k] / 2
        faces[str(label)] = center + spread @ frame
    np.testing.assert_allclose(fit_triax('4', faces).center(sizes), center, atol=1e-9)


def test_noisy_faces_are_squared_and_say_how_far_they_moved():
    frame, center, side, faces = _cube(13, noise=5e-6)
    fit = fit_triax('33', faces, meters=1.0)
    assert fit.placed and not fit.flags
    assert 0 < fit.moved < 0.5
    np.testing.assert_allclose(fit.frame @ fit.frame.T, np.eye(3), atol=1e-12)
    assert np.linalg.det(fit.frame) == pytest.approx(1.0)
    np.testing.assert_allclose(fit.frame, frame, atol=5e-3)
    assert np.linalg.norm(fit.center(side) - center) < 3e-5


def test_what_is_wrong_with_a_triax_is_flagged():
    frame, center, side, faces = _cube(14)

    def texts(fit, level):
        return [flag.text for flag in fit.flags if flag.level == level]

    # two labels exchanged: a left-handed set
    swapped = fit_triax('1', {'1': faces['2'], '2': faces['1'], '3': faces['3']})
    assert not swapped.placed and 'left-handed' in texts(swapped, 'error')[0]
    # one face
    alone = fit_triax('1', {'1': faces['1']})
    assert not alone.placed and 'a second face' in texts(alone, 'error')[0]
    # the same axis twice
    twice = fit_triax('1', {'1': faces['1'], '-1': faces['2']})
    assert not twice.placed and 'same axis' in texts(twice, 'error')[0]
    # two points on a face
    few = fit_triax('1', {**faces, '3': faces['3'][:2]})
    assert 'face 3 has 2 points' in texts(few, 'error')[0]
    # a face leaning 3 degrees
    lean = Rotation.from_rotvec(np.radians(3) * frame[0]).as_matrix()
    tilted = {**faces, '3': center + (faces['3'] - center) @ lean.T}
    off = fit_triax('1', tilted)
    assert off.placed and any('degrees from square' in text for text in texts(off, 'warn'))
    # a rough face, judged only when the unit is known
    rng = np.random.default_rng(1)
    rough = {**faces, '2': faces['2'] + rng.normal(0, 0.2 * MM, faces['2'].shape)}
    assert any('face 2 is rough' in text for text in texts(fit_triax('1', rough, meters=1.0), 'warn'))
    assert not any('rough' in text for text in texts(fit_triax('1', rough), 'warn'))
    with pytest.raises(ValueError, match='not located'):
        alone.center(side)


# ---- a uni-axial accelerometer ----------------------------------------------

@pytest.mark.parametrize(('label', 'channel'), [(3, 'Z+'), (-3, 'Z+'), (1, 'X+'), (-2, 'Y+')])
def test_a_side_and_a_top_give_the_axis_the_top_center_and_the_center(label, channel):
    frame, center, radius, height, side, top = _can(21, label=label)
    fit = fit_uniaxial('11', side, top, str(label), meters=1.0)
    assert fit.placed and not fit.flags and fit.kind == 'uniaxial'
    k, sign = abs(label) - 1, np.sign(label)
    # the labelled axis points out of the top, or into it for a negative label
    np.testing.assert_allclose(fit.frame[k], frame[k], atol=1e-8)
    np.testing.assert_allclose(fit.frame @ fit.frame.T, np.eye(3), atol=1e-12)
    assert np.linalg.det(fit.frame) == pytest.approx(1.0)
    np.testing.assert_allclose(fit.anchor, center + sign * (height / 2) * frame[k], atol=1e-8)
    np.testing.assert_allclose(fit.center(height), center, atol=1e-8)
    assert fit.extent == pytest.approx(2 * radius, rel=1e-6)
    assert fit.direction == channel and fit.axes == (label,)


@pytest.mark.parametrize(('band', 'label'), [((0.0, 1.0), 3), ((0.2, 0.7), 3), ((0.5, 0.95), -2)])
def test_a_cans_length_is_twice_its_top_from_the_middle_of_its_side(band, label):
    # the middle is the side's points' mean along the axis, so a side measured off center
    # gives a length that is not the true height
    frame, center, _radius, height, side, top = _can(23, band=band, label=label)
    up = np.sign(label) * frame[abs(label) - 1]
    middle = float(np.mean((side - center) @ up))
    fit = fit_uniaxial('11', side, top, str(label), meters=1.0)
    assert fit.length == pytest.approx(2 * (height / 2 - middle), rel=1e-6)
    assert fit.length != pytest.approx(fit.extent, rel=1e-2)


def test_what_is_wrong_with_a_uniaxial_is_flagged():
    frame, _center, _radius, height, side, top = _can(22)

    def texts(fit, level):
        return [flag.text for flag in fit.flags if flag.level == level]

    # the top of another accelerometer, 100 mm away
    far = fit_uniaxial('11', side, top + 0.1 * frame[0])
    assert not far.placed and 'not on this cylinder' in texts(far, 'error')[0]
    # a quarter of the way round
    _f, _c, _r, _h, short, short_top = _can(22, arc=80.0)
    assert any('degrees of its circumference' in text
               for text in texts(fit_uniaxial('11', short, short_top), 'warn'))
    # a top leaning 5 degrees
    lean = Rotation.from_rotvec(np.radians(5) * frame[0]).as_matrix()
    middle = top.mean(axis=0)
    leaning = fit_uniaxial('11', side, middle + (top - middle) @ lean.T)
    assert leaning.placed and any('from square to the side' in text
                                  for text in texts(leaning, 'warn'))
    # a flat inside the measured band is not the top
    inside = fit_uniaxial('11', side, top - 0.45 * height * frame[2])
    assert any('may be another surface' in text for text in texts(inside, 'warn'))
    # too few points
    few = fit_uniaxial('11', side[:4], top[:2])
    assert len(texts(few, 'error')) == 2 and not few.placed


# ---- features by name --------------------------------------------------------

def test_a_features_name_gives_its_sensor_and_label():
    assert parse_feature('31_1') == ('31', '1')
    assert parse_feature('3_-1') == ('3', '-1')
    assert parse_feature('11_C') == ('11', 'c')
    assert parse_feature(' 5_p ') == ('5', '3')
    assert parse_feature('left_bracket_2') == ('left_bracket', '2')
    for name in ('31', '31_4', '_1', '31_x', '31-1', ''):
        assert parse_feature(name) is None


def test_features_are_grouped_into_sensors_in_file_order():
    _frame, center, side, faces = _cube(31)
    _f, can_center, _radius, height, can_side, can_top = _can(32)
    features = {'12_c': can_side, '31_1': faces['1'], '12_3': can_top, '31_2': faces['2'],
                '31_3': faces['3'], 'datum': faces['1'], '31_1 ': faces['1'], '9_c': can_side}
    fits, notes = fit_sensors(features, meters=1.0)
    assert [(fit.sensor, fit.kind, fit.placed) for fit in fits] == [
        ('12', 'uniaxial', True), ('31', 'triax', True), ('9', 'uniaxial', False)]
    np.testing.assert_allclose(fits[1].center(side), center, atol=1e-9)
    np.testing.assert_allclose(fits[0].center(height), can_center, atol=1e-8)
    assert 'one flat face, its top or its bottom; this sensor has 0' in fits[2].flags[0].text
    assert len(notes) == 2
    assert notes[0].startswith('datum is not named') and 'repeats a label' in notes[1]


def test_a_project_fits_the_measured_points_of_a_geometry():
    _frame, center, side, faces = _cube(41)
    names = ['7_1', '7_2', '7_-3']
    held = [faces['1'], faces['2'], 2 * center - faces['3']]     # the Z- face
    points = np.vstack(held) / MM
    owner = np.repeat(np.arange(3), [len(h) for h in held])
    project = Project()
    project.add('Probed', points_geometry(points, 'mm', names, owner))
    fits, notes = project.fit_sensors('Probed')
    assert not notes and len(fits) == 1 and fits[0].placed and fits[0].axes == (1, 2, -3)
    np.testing.assert_allclose(fits[0].center(side), center, atol=1e-9)
    # with no unit the fit is the same in the file's numbers, and says what it left out
    project.add('Raw', points_geometry(points, None, names, owner))
    fits, notes = project.fit_sensors('Raw')
    np.testing.assert_allclose(fits[0].center(side / MM), center / MM, atol=1e-6)
    assert notes == ['Raw has no length unit, so the roughness of the surfaces was not judged']
    # a group renamed changes what its face is taken for
    project['Probed'].group_name = ['7_1', '7_2', '7_3']
    assert 'left-handed' in project.fit_sensors('Probed')[0][0].flags[0].text
    project.add('Cloud', points_geometry(points))
    with pytest.raises(ValueError, match='holds no measured points'):
        project.fit_sensors('Cloud')
    project.new_channel_table('Table')
    with pytest.raises(TypeError, match='is not a geometry'):
        project.fit_sensors('Table')


# ---- the example files, where they are --------------------------------------

ARM_DATA = os.environ.get('HERTZFORGE_ARM_DATA', '')


@pytest.mark.skipif(not os.path.isdir(ARM_DATA), reason='HERTZFORGE_ARM_DATA names no folder')
def test_the_example_files_fit_as_the_plan_records():
    """The two labelled files measured on real accelerometers: sizes,
    squareness and the flags the plan quotes, with sensors 11 and 12 of
    the frame file crossed as they were named."""
    project = Project()
    frame = project.import_file(os.path.join(ARM_DATA, 'Frame_Probed_Labeled.csv'))[0]
    fits = {fit.sensor: fit for fit in project.fit_sensors(frame)[0]}
    assert [fits[s].placed for s in ('11', '12', '31', '32', '33', '34')] == [
        False, False, True, True, True, True]
    for sensor in ('11', '12'):
        assert any('not on this cylinder' in flag.text for flag in fits[sensor].flags)
    extents = [round(fits[s].extent / MM, 1) for s in ('31', '32', '33', '34')]
    assert extents == [5.0, 8.7, 8.7, 17.5]
    assert all(not fits[s].flags and fits[s].moved < 0.3 for s in ('31', '32', '33', '34'))
    assert fits['11'].extent / MM == pytest.approx(7.07, abs=0.01)
    assert fits['12'].extent / MM == pytest.approx(15.98, abs=0.01)
    other = project.import_file(os.path.join(ARM_DATA, 'Cylinder_Object_python.csv'))[0]
    fits = {fit.sensor: fit for fit in project.fit_sensors(other)[0]}
    assert all(fits[s].placed for s in ('2', '3', '4', '5'))
    assert fits['3'].axes == (-1, 2, 3)
    assert any('3.4 degrees from square' in flag.text for flag in fits['3'].flags)
    assert (fits['2'].direction, fits['5'].direction) == ('Z+', 'Z+')
    assert fits['2'].extent / MM == pytest.approx(28.19, abs=0.01)
    assert fits['5'].extent / MM == pytest.approx(3.38, abs=0.01)


def test_tops_named_for_each_others_sensor_are_said_to_look_exchanged():
    """Two cans whose tops were named for each other's sensor: each is
    refused, and each says which two features to rename. Nothing is
    exchanged for the person."""
    _f, _c, _r, _h, side_a, top_a = _can(51)
    _f, _c, _r, _h, side_b, top_b = _can(52)
    fits, _notes = fit_sensors({'11_c': side_a, '11_3': top_b, '12_c': side_b, '12_3': top_a})
    assert [fit.placed for fit in fits] == [False, False]
    said = [[flag.text for flag in fit.flags] for fit in fits]
    assert "its flat face is on sensor 12's cylinder: 11_3 and 12_3 look exchanged" in said[0]
    assert "its flat face is on sensor 11's cylinder: 12_3 and 11_3 look exchanged" in said[1]
    # named right, neither says it
    fits, _notes = fit_sensors({'11_c': side_a, '11_3': top_a, '12_c': side_b, '12_3': top_b})
    assert all(fit.placed and not fit.flags for fit in fits)


def test_a_surface_is_called_by_name_and_written_as_a_label():
    from hertzforge.core.scan_fit import SURFACES, label_of, surface_of, written_label

    assert parse_feature('11_b') == ('11', '-3') and parse_feature('11_P') == ('11', '3')
    assert written_label('11_b') == ('11', 'b') and written_label('11_x') is None
    assert [surface_of(f'5_{label}') for label in ('1', '-2', '3', 'c', 'p', 'b')] == [
        'X+', 'Y-', 'Z+', 'Cylinder', 'Uni-axial Top', 'Uni-axial Bottom']
    # beside a cylinder a Z face is the top and a Z- face the bottom, and is called so
    assert surface_of('5_3', beside_cylinder=True) == 'Uni-axial Top'
    assert surface_of('5_-3', beside_cylinder=True) == 'Uni-axial Bottom'
    assert surface_of('5_1', beside_cylinder=True) == 'X+' and surface_of('datum') == ''
    for label, called in SURFACES.items():
        assert label_of(called) == label == label_of(label)
    assert label_of(' uni-axial   BOTTOM ') == 'b'
    with pytest.raises(ValueError, match="'sideways' is not a surface; one is X\\+, X-"):
        label_of('sideways')


def test_a_bottom_measured_turns_the_axis_through_the_body():
    """The flat face measured is the one the accelerometer is mounted by:
    Z+ points away from it, and the center is half the length above it."""
    frame, center, _radius, height, side, top = _can(61)
    bottom = top - height * frame[2]
    fits, notes = fit_sensors({'7_c': side, '7_b': bottom}, meters=1.0)
    assert not notes and fits[0].placed and fits[0].axes == (-3,)
    np.testing.assert_allclose(fits[0].frame[2], frame[2], atol=1e-8)
    np.testing.assert_allclose(fits[0].anchor, center - (height / 2) * frame[2], atol=1e-8)
    np.testing.assert_allclose(fits[0].center(height), center, atol=1e-8)
    assert fits[0].direction == 'Z+'
