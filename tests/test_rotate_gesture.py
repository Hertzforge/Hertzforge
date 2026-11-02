"""Turning a coordinate system in the window: the rings and the angle.

`test_rotate.py` has the math — what a frame becomes when it is turned
about an axis, with no window in sight. This is the gesture on top of it:
when the rings are offered at all, what appears with them, that a typed
angle turns the frame and only the frame, that Reset puts it back square
without moving it, and that switching off takes the rings out of the
scene.

It is a good example of why a control's *visibility* is worth asserting:
turning is a single-object act, so with two rows picked there is no
answer to which frame the rings belong to, and the button has to be gone
rather than present and confusing.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path


def test_place_coordinate_system_turns_and_moves_one_frame():
    import hertzforge
    from hertzforge.rotate import frame_from_angles

    geometry = hertzforge.import_file(fixture_path('plate',
                                                       'geometry.npz'))
    cs = geometry.add_coordinate_system(origin=(1.0, 2.0, 3.0))
    geometry.place_coordinate_system(cs, angles=(0.0, 0.0, 90.0))
    row = list(geometry.cs_id).index(cs)
    assert np.allclose(geometry.cs_matrix[row, :3],
                       frame_from_angles((0, 0, 90))[:3])
    assert np.allclose(geometry.cs_matrix[row, 3], (1.0, 2.0, 3.0)), \
        'a turn keeps the origin'
    geometry.place_coordinate_system(cs, origin=(4.0, 5.0, 6.0))
    assert np.allclose(geometry.cs_matrix[row, :3],
                       frame_from_angles((0, 0, 90))[:3]), 'a move keeps the turn'
    geometry.place_coordinate_system(cs, rotation=np.eye(3))
    assert np.allclose(geometry.cs_matrix[row, :3], np.eye(3))
    with pytest.raises(ValueError, match='not both'):
        geometry.place_coordinate_system(cs, angles=(0, 0, 0),
                                         rotation=np.eye(3))
    with pytest.raises(KeyError, match='no coordinate system 999'):
        geometry.place_coordinate_system(999, angles=(0, 0, 0))
