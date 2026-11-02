"""Screen-space picking: is the right thing under the cursor?"""

import hertzforge


def solid_box():
    """A genuinely three-dimensional wireframe — the plate is planar,
    which is exactly what this test needs the model not to be."""
    corners = [(x, y, z) for z in (0.0, 0.4)
               for y in (0.0, 0.3) for x in (0.0, 0.5)]
    lines = [[1, 2, 4, 3, 1], [5, 6, 8, 7, 5],
             [1, 5], [2, 6], [3, 7], [4, 8]]
    geometry = hertzforge.Geometry(
        node_id=list(range(1, 9)), node_xyz=corners, length_unit='m')
    for line in lines:
        geometry.add_beams(line)
    return geometry
