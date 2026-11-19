"""A frame's angles, pinned by value where a mutation pass found them
unguarded (2026-10-11)."""

from __future__ import annotations

import math

from hertzforge.rotate import angles_of, frame_from_angles


def test_a_frame_is_four_rows_and_its_angles_read_back():
    frame = frame_from_angles((10.0, 20.0, 30.0), origin=(1.0, 2.0, 3.0))
    assert frame.shape == (4, 3) and frame[3].tolist() == [1.0, 2.0, 3.0]
    assert angles_of(frame) == (10.0, 20.0, 30.0)
    zero = angles_of(frame_from_angles((0.0, 0.0, 0.0)))
    assert zero == (0.0, 0.0, 0.0)
    assert all(math.copysign(1.0, a) == 1.0 for a in zero), 'never a negative zero'


