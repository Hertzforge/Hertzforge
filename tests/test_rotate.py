"""Turning a coordinate system: the geometry, without a window."""

import numpy as np
import pytest

from hertzforge.rotate import identity_frame, rotate_frame, rotation_about


def frame(origin=(0.0, 0.0, 0.0)):
    matrix = np.zeros((4, 3))
    matrix[:3] = np.eye(3)
    matrix[3] = origin
    return matrix


def test_a_quarter_turn_about_z_sends_x_to_y():
    turned = rotate_frame(frame(), 2, np.pi / 2)
    assert np.allclose(turned[0], [0, 1, 0], atol=1e-12)
    assert np.allclose(turned[1], [-1, 0, 0], atol=1e-12)
    assert np.allclose(turned[2], [0, 0, 1], atol=1e-12)


def test_rotation_leaves_the_origin_alone():
    turned = rotate_frame(frame((3.0, -1.0, 2.0)), 0, 0.7)
    assert np.allclose(turned[3], [3.0, -1.0, 2.0])


def test_the_frame_stays_orthonormal_after_many_turns():
    matrix = frame()
    for _ in range(200):
        matrix = rotate_frame(matrix, 1, 0.37)
    basis = matrix[:3]
    assert np.allclose(basis @ basis.T, np.eye(3), atol=1e-9)


def test_the_axis_travels_with_the_frame():
    """Two half turns about the frame's own X is one full turn about it."""
    once = rotate_frame(rotate_frame(frame(), 0, np.pi / 4), 0, np.pi / 4)
    straight = rotate_frame(frame(), 0, np.pi / 2)
    assert np.allclose(once[:3], straight[:3], atol=1e-12)


def test_resetting_keeps_the_origin_but_drops_the_turn():
    matrix = rotate_frame(frame((4.0, 5.0, 6.0)), 1, 1.1)
    reset = identity_frame(matrix)
    assert np.allclose(reset[:3], np.eye(3))
    assert np.allclose(reset[3], [4.0, 5.0, 6.0])


def test_rotation_about_is_a_proper_rotation():
    matrix = rotation_about((1.0, 2.0, 3.0), 0.9)
    assert np.linalg.det(matrix) == pytest.approx(1.0)
    assert np.allclose(matrix @ matrix.T, np.eye(3), atol=1e-12)


def _frame():
    from hertzforge.rotate import rotate_frame

    plain = np.vstack([np.eye(3), [1.0, 2.0, 3.0]])
    return rotate_frame(plain, 2, np.radians(30.0))


def test_sliding_moves_the_origin_along_the_frames_own_axis():
    from hertzforge.rotate import translate_frame

    frame = _frame()
    moved = translate_frame(frame, 0, 2.5)
    assert np.allclose(moved[:3], frame[:3]), 'the basis stays put'
    assert np.allclose(moved[3], frame[3] + 2.5 * frame[0])


def test_angles_turn_about_the_fixed_axes_x_then_y_then_z():
    from hertzforge.rotate import frame_from_angles

    frame = frame_from_angles((90.0, 0.0, 0.0))
    assert np.allclose(frame[1], [0, 0, 1]) and np.allclose(frame[2], [0, -1, 0])
    # X then Z: the frame's X axis is carried round Z, its Y up first
    frame = frame_from_angles((90.0, 0.0, 90.0), origin=(1, 2, 3))
    assert np.allclose(frame[0], [0, 1, 0]) and np.allclose(frame[1], [0, 0, 1])
    assert np.allclose(frame[3], [1, 2, 3])


def test_the_angles_of_a_frame_undo_the_turn():
    from hertzforge.rotate import angles_of, frame_from_angles

    for angles in [(30.0, -20.0, 75.0), (-170.0, 45.0, 10.0), (0.0, 0.0, -90.0),
                   (12.5, 0.0, 0.0)]:
        assert angles_of(frame_from_angles(angles)) == pytest.approx(angles)
    # a quarter turn about Y: X and Z are one turn, said as Z
    frame = frame_from_angles((25.0, 90.0, 40.0))
    back = angles_of(frame)
    assert back[0] == 0.0 and back[1] == pytest.approx(90.0)
    assert np.allclose(frame_from_angles(back), frame)
