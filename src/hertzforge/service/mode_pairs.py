"""Mode shapes on a survey geometry: a value per DOF as a translation per
drawn node, and a MAC matrix with the shape view it selects, the row
set's modes overlaid by the column set's."""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .mesh import drawn_mesh


def translations(geometry: Any, dofs: Any, values: np.ndarray,
                  rows: np.ndarray) -> np.ndarray:
    """One value per DOF as a translation per drawn node, [N, 3], complex
    when the values are. Nodes with no measured DOF stay where they are."""
    from hertzforge.deform import dof_directions

    dof_rows, dof_vectors, dof_used = dof_directions(geometry, dofs)
    values = np.asarray(values)[dof_used]

    def spread(part: np.ndarray) -> np.ndarray:
        return np.stack([np.bincount(dof_rows, dof_vectors[:, axis] * part,
                                     minlength=geometry.num_nodes)
                         for axis in range(3)], axis=1)[rows]

    if np.iscomplexobj(values):
        return spread(values.real) + 1j * spread(values.imag)
    return spread(values)



def paired(geometry: Any, matrix: dict, rows: Any, row_modes: Any,
           columns: Any, column_modes: Any) -> list[dict]:
    """The MAC `matrix` and, with a `geometry`, the `shape3d`
    view it `selects`: the row set's modes as `modes` and the column set's
    as `overlays`, each a translation per survey node, with each pair's
    `overlay_phase`, the angle that aligns the column mode with the row
    mode (`core.shapes.alignment_factor`: a real mode is defined only up to
    its sign, a complex one up to a rotation), as translations of the geometry's nodes. Without a geometry the
    matrix comes alone."""
    from hertzforge.core.shapes import alignment_factor

    if geometry is None:
        return [matrix]
    drawn, triangles, lines = drawn_mesh(geometry)

    def on_survey(shapes: Any, k: int) -> np.ndarray:
        return translations(geometry, shapes.coordinate, shapes.shape_matrix[k], drawn)

    modes = [v.mode(r + 1, float(rows.frequency[r]), float(rows.damping[r]),
                    on_survey(rows, r)) for r in row_modes]
    named = matrix['columns']['label']
    overlays = [v.mode(int(c) + 1, float(columns.frequency[c]),
                       float(columns.damping[c]) if columns.damping[c] > 0 else None,
                       on_survey(columns, int(c)), label=f'{named} mode {int(c) + 1}')
                for c in column_modes]
    phase = np.array([[np.angle(alignment_factor(rows, r, columns, int(c)))
                       for c in column_modes] for r in row_modes], dtype=np.float32)
    matrix['selects'] = 'shape3d'
    shape = v.shape3d(matrix['title'], 'm' if geometry.units_defined else 'undefined',
                      np.asarray(geometry.node_xyz, dtype=float)[drawn], modes,
                      triangles=triangles, lines=lines, overlays=overlays,
                      overlay_phase=phase)
    return [matrix, shape]


