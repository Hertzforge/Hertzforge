"""Mode shapes on a survey geometry: a value per DOF as a translation per
drawn node, and a MAC matrix with the shape view it selects, the row
set's modes overlaid by the column set's."""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .descriptors import Param
from .mesh import drawn_mesh


def translations(geometry: Any, dofs: Any, values: np.ndarray,
                  rows: np.ndarray) -> np.ndarray:
    """One value per DOF as a translation per drawn node, [N, 3], or
    [modes, N, 3] for a matrix of `values` with a row per mode, complex
    when the values are. Nodes with no measured DOF stay where they are.
    The DOFs are parsed once for all the modes; `dofs` given as a shape set
    are its own, parsed once for as long as it keeps them
    (`ShapeSet._parsed_dofs`)."""
    from scipy.sparse import csr_array

    from hertzforge.core.shapes import ShapeSet
    from hertzforge.deform import dof_directions

    if isinstance(dofs, ShapeSet):
        dof_rows, dof_vectors, dof_used = dof_directions(geometry, dofs.coordinate,
                                                         parsed=dofs._parsed_dofs())
    else:
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


def _stage(geometry: Any, title: str, rows: Any, row_modes: list[int], columns: Any,
           column_modes: list[int], named: str, column_geometry: Any = None,
           compared: Any = None) -> dict:
    """The `shape3d` of two sets of shapes: the row set's as `modes`, the column set's as
    `overlays`, with each pair's `overlay_phase` (`core.shapes.alignment_phases`).

    On one `geometry` both are translations of its nodes. With `column_geometry`, the
    geometry the column set is on, each set moves its own: the scene's nodes are the first
    geometry's followed by the second's, `triangles` and `lines` are the first's mesh and
    `overlay_triangles` and `overlay_lines` the second's, so a model's modes are drawn on the
    model and a test's on the test geometry laid over it (Brandon, 2026-11-16). A row mode
    moves no node of the second geometry and a column mode none of the first.

    `compared` is the two sets as they are compared (`Project.compared_sets`): sets on
    two geometries share no DOF, so the angle that aligns a pair is read from them with one
    projected onto the other's DOFs, and the two then move together and not half a cycle
    apart."""
    from hertzforge.core.shapes import alignment_phases

    drawn, triangles, lines = drawn_mesh(geometry)
    nodes = np.asarray(geometry.node_xyz, dtype=float)[drawn]
    moved = translations(geometry, rows, rows.shape_matrix[row_modes], drawn)
    extra: dict[str, Any] = {}
    defined = geometry.units_defined
    if column_geometry is None:
        over = (moved if columns is rows and column_modes == row_modes else
                translations(geometry, columns, columns.shape_matrix[column_modes],
                             drawn))
    else:
        there, its_triangles, its_lines = drawn_mesh(column_geometry)
        over = translations(column_geometry, columns,
                            columns.shape_matrix[column_modes], there)
        first, second = len(drawn), len(there)
        # each set's shapes on its own nodes, at rest on the other's
        moved = np.concatenate([moved, np.zeros((*moved.shape[:-2], second, 3), moved.dtype)],
                               axis=-2)
        over = np.concatenate([np.zeros((*over.shape[:-2], first, 3), over.dtype), over], axis=-2)
        nodes = np.vstack([nodes, np.asarray(column_geometry.node_xyz, dtype=float)[there]])
        defined = defined and column_geometry.units_defined
        extra['overlay_triangles'] = (np.ascontiguousarray(its_triangles + first, np.uint32)
                                      if its_triangles is not None and len(its_triangles)
                                      else np.zeros((0, 3), np.uint32))
        extra['overlay_lines'] = (np.ascontiguousarray(its_lines + first, np.uint32)
                                  if its_lines is not None and len(its_lines)
                                  else np.zeros((0, 2), np.uint32))
    modes = [v.mode(r + 1, float(rows.frequency[r]), float(rows.damping[r]), moved[k])
             for k, r in enumerate(row_modes)]
    overlays = [v.mode(c + 1, float(columns.frequency[c]),
                       float(columns.damping[c]) if columns.damping[c] > 0 else None,
                       over[k], label=f'{named} mode {c + 1}')
                for k, c in enumerate(column_modes)]
    ours, theirs = (rows, columns) if compared is None else compared
    phase = alignment_phases(ours, row_modes, theirs, column_modes).astype(np.float32)
    return v.shape3d(title, 'm' if defined else 'undefined', nodes, modes,
                     triangles=triangles, lines=lines, overlays=overlays,
                     overlay_phase=phase, **extra)


#: the modes of each of two sets an action compares, filled from what is picked under each in
#: the tree (`action_table.PICKED_MODES`): blank for every mode
FIRST_MODES = Param('first_modes', 'Modes of the first set (blank: all)', 'string', default='')
SECOND_MODES = Param('second_modes', 'Modes of the second set (blank: all)', 'string',
                     default='')


def modes_param(shapes: Any, text: str) -> list[int]:
    """The modes a `FIRST_MODES`-like param names, by index from 0: every
    mode when it is blank."""
    from .parsing import parse_optional_picks

    picks = parse_optional_picks(text, shapes.num_shapes, 'mode')
    return picked_modes(shapes, picks)


def picked_modes(shapes: Any, picks: Any) -> list[int]:
    """The modes picked under a shape set, by index from 0, in the set's
    order and once each; every mode when none is picked."""
    from hertzforge.core.modal_frf import picked_modes as chosen

    from .protocol import refused

    try:
        return chosen(shapes, list(picks) if picks else None)
    except ValueError as error:
        raise refused(str(error)) from None


def mode_axis(label: str, shapes: Any, modes: list[int]) -> dict:
    """A MAC axis of the `modes` of `shapes`: each ticked by its frequency,
    with its index in the set, which a picked square is named by."""
    return {'label': label, 'ticks': [f'{shapes.frequency[k]:.1f} Hz' for k in modes],
            'indices': list(modes)}


def matched_cells(pairs: Any, row_modes: list[int], column_modes: list[int]) -> list[list[int]]:
    """Matched (row mode, column mode) pairs as the cells of a MAC of
    `row_modes` against `column_modes` that hold them; a pair with a mode
    not on the grid is not drawn."""
    rows = {mode: i for i, mode in enumerate(row_modes)}
    columns = {mode: j for j, mode in enumerate(column_modes)}
    return [[rows[int(a)], columns[int(b)]] for a, b in pairs
            if int(a) in rows and int(b) in columns]


def paired(geometry: Any, matrix: dict, rows: Any, row_modes: Any,
           columns: Any, column_modes: Any, column_geometry: Any = None,
           compared: Any = None) -> list[dict]:
    """The MAC `matrix` and, with a `geometry`, the `shape3d`
    view it `selects`: the row set's modes as `modes` and the column set's
    as `overlays`, with each pair's `overlay_phase`, the angle that aligns the
    column mode with the row mode (`core.shapes.alignment_phases`: a real mode is
    defined only up to its sign, a complex one up to a rotation). Both are
    translations of the geometry's nodes, or with `column_geometry` each set moves its
    own geometry in one scene (`_stage`). Without a geometry the matrix comes alone."""
    if geometry is None:
        return [matrix]
    row_modes, column_modes = [int(r) for r in row_modes], [int(c) for c in column_modes]
    matrix['selects'] = 'shape3d'
    shape = _stage(geometry, matrix['title'], rows, row_modes, columns, column_modes,
                   matrix['columns']['label'], column_geometry, compared)
    return [matrix, shape]


def matched_stage(geometry: Any, title: str, rows: Any, columns: Any, pairs: Any,
                  second: str, column_geometry: Any = None, compared: Any = None) -> dict:
    """A Matched Modes object's pairs: the first set's mode of each pair as
    `modes`, the second set's as `overlays`, `paired` so the shell draws overlay k over mode k as
    it steps through them, each aligned by its pair's phase (`core.shapes.alignment_phases`).
    On `geometry`, or with `column_geometry` each set on its own geometry (`_stage`).
    The pairs overlaid, where the earlier toolset showed only their table (Brandon,
    2026-11-14)."""
    shape = _stage(geometry, title, rows, [int(a) for a, _b in pairs], columns,
                   [int(b) for _a, b in pairs], second, column_geometry, compared)
    shape['paired'] = True
    return shape
