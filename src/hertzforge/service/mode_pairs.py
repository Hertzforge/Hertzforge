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
    """One value per DOF as a translation per drawn node, [N, 3], or
    [modes, N, 3] for a matrix of `values` with a row per mode, complex
    when the values are. Nodes with no measured DOF stay where they are.
    The DOFs are parsed once for all the modes."""
    from scipy.sparse import csr_array

    from hertzforge.deform import dof_directions

    dof_rows, dof_vectors, dof_used = dof_directions(geometry, dofs)
    values = np.asarray(values)[..., dof_used]
    kept = np.asarray(rows)
    # [used DOFs, 3 * drawn nodes]: each DOF's unit vector on its node, so one product spreads every mode
    place = np.full(geometry.num_nodes, -1, dtype=np.int64)
    place[kept] = np.arange(len(kept))
    drawn = place[dof_rows]
    on = drawn >= 0
    entries = dof_vectors[on]
    columns = (3 * drawn[on][:, None] + np.arange(3)[None, :]).ravel()
    lines = np.repeat(np.flatnonzero(on), 3)
    spread = csr_array((entries.ravel(), (lines, columns)), shape=(len(dof_rows), 3 * len(kept)))
    moved = np.asarray(values @ spread)
    return moved.reshape(*values.shape[:-1], len(kept), 3)


def paired(geometry: Any, matrix: dict, rows: Any, row_modes: Any,
           columns: Any, column_modes: Any) -> list[dict]:
    """The MAC `matrix` and, with a `geometry`, the `shape3d`
    view it `selects`: the row set's modes as `modes` and the column set's
    as `overlays`, each a translation per survey node, with each pair's
    `overlay_phase`, the angle that aligns the column mode with the row
    mode (`core.shapes.alignment_phases`: a real mode is defined only up to
    its sign, a complex one up to a rotation), as translations of the geometry's nodes. Without a geometry the
    matrix comes alone."""
    from hertzforge.core.shapes import alignment_phases

    if geometry is None:
        return [matrix]
    drawn, triangles, lines = drawn_mesh(geometry)

    row_modes, column_modes = [int(r) for r in row_modes], [int(c) for c in column_modes]
    moved = translations(geometry, rows.coordinate, rows.shape_matrix[row_modes], drawn)
    modes = [v.mode(r + 1, float(rows.frequency[r]), float(rows.damping[r]), moved[k])
             for k, r in enumerate(row_modes)]
    named = matrix['columns']['label']
    moved = (moved if columns is rows and column_modes == row_modes else
             translations(geometry, columns.coordinate, columns.shape_matrix[column_modes], drawn))
    overlays = [v.mode(c + 1, float(columns.frequency[c]),
                       float(columns.damping[c]) if columns.damping[c] > 0 else None,
                       moved[k], label=f'{named} mode {c + 1}')
                for k, c in enumerate(column_modes)]
    phase = alignment_phases(rows, row_modes, columns, column_modes).astype(np.float32)
    matrix['selects'] = 'shape3d'
    shape = v.shape3d(matrix['title'], 'm' if geometry.units_defined else 'undefined',
                      np.asarray(geometry.node_xyz, dtype=float)[drawn], modes,
                      triangles=triangles, lines=lines, overlays=overlays,
                      overlay_phase=phase)
    return [matrix, shape]


