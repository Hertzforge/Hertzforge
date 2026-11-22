"""Test-analysis correlation: FEM shapes sampled at the test's DOFs.

A finite element model has a dense mesh with three or more DOFs per
node; the test measured a smaller set of directions at its own nodes.
Comparing the two starts by projecting the model onto the test: each
test node is matched to its nearest FEM node, the FEM displacement
there (already in global coordinates, from `deform`) is taken, and its
dot product with the test DOF's global direction is the projected
value. That handles uniaxial measurements and local coordinate systems
in one step. The result is an ordinary ShapeSet at the test's DOFs, so
cross-MAC, overlay animation and reports work on it unchanged.

Both geometries must already be in the same frame; this module does
not align them.

References
----------
The projection here is ordinary; these are where the comparison it
feeds is set out.

1. Allemang, R. J., & Brown, D. L. (1982). "A Correlation Coefficient
   for Modal Vector Analysis." *Proceedings of the 1st International
   Modal Analysis Conference (IMAC)*. The Modal Assurance Criterion.
2. Allemang, R. J. (2003). "The Modal Assurance Criterion - Twenty
   Years of Use and Abuse." *Sound and Vibration*. What the number
   does and does not say, including why a high MAC between a model
   and a test is not by itself agreement.
3. Ewins, D. J. (2000). *Modal Testing: Theory, Practice and
   Application*, 2nd ed. Research Studies Press. Comparing a model
   with a test, and reducing one to the other's degrees of freedom.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

import numpy as np

if TYPE_CHECKING:                                    # pragma: no cover
    from .geometry import Geometry
    from .shapes import ShapeSet


def partial_nodes(shapes: ShapeSet) -> int:
    """How many of a shape set's nodes it holds in fewer than three
    translation directions: the nodes a uni-axial or a bi-axial sensor
    measured. A rotation, and a DOF at no node (a modal coordinate), is
    not counted."""
    # a model's set holds hundreds of thousands of DOFs, and one selection asks this of each
    # set many times (which way a pair goes, why a way cannot be made), so the count is kept
    # with the set while its DOF list is the same one
    held = shapes.coordinate
    kept = getattr(shapes, '_partial_nodes', None)
    if kept is not None and kept[0] is held and kept[1] == len(held):
        return kept[2]
    axes: dict[str, set[str]] = {}
    for dof in held:
        text = str(dof).strip().upper()
        node = text[:len(text) - len(text.lstrip('0123456789'))]
        direction = text[len(node):]
        if node and direction[:1] in ('X', 'Y', 'Z'):
            axes.setdefault(node, set()).add(direction[0])
    count = sum(1 for found in axes.values() if len(found) < 3)
    shapes._partial_nodes = (held, len(held), count)
    return count


def projection_order(first: ShapeSet, second: ShapeSet) -> tuple[bool, str]:
    """Which of two shape sets on two geometries is projected onto the
    other's DOFs before they are compared: (whether it is the first that
    is projected, why).

    A projection samples one set's whole motion at the other's DOFs, so
    the set projected has to have that whole motion to give. A node
    measured along one axis does not: projected onto three directions,
    what it did not measure is taken as no motion at all, and the other
    set's motion there counts against the match. On a plate with twelve
    uni-axial nodes of twenty-eight, two modes that match at 1.000 and
    0.983 the right way read 0.448 and 0.264 the wrong way (Brandon,
    2026-11-16).

    So the set with nodes measured in fewer than three directions
    (`partial_nodes`) is the one projected onto. When neither has any,
    or both have, the set with more DOFs is projected onto the one with
    fewer (the second onto the first when they have as many); with
    partial nodes in both, neither way is exact, and the reason says so.
    """
    ours, theirs = partial_nodes(first), partial_nodes(second)

    def nodes(count: int) -> str:
        return (f'{count} node{"s" * (count != 1)} measured in fewer than three '
                'directions')

    if theirs and not ours:
        return True, f'it has {nodes(theirs)}'
    if ours and not theirs:
        return False, f'it has {nodes(ours)}'
    onto_second = len(second.coordinate) < len(first.coordinate)
    if ours and theirs:
        return onto_second, ('both have nodes measured in fewer than three directions, so '
                             'neither way is exact; it has the fewer DOFs')
    return onto_second, 'it has the fewer DOFs'


class NoNodeMatched(ValueError):
    """No test node has a model node within the match tolerance. `report` says how far
    off they are: `matched` (0), `total`, `worst` (the nearest pair's distance),
    `limit`, `dropped` (none named), `pairs` (none) and `unmatched` (every test node's
    id), as `project_shapes` reports a projection that did match."""

    def __init__(self, message: str, report: dict[str, Any]):
        super().__init__(message)
        self.report: dict[str, Any] = report


def si_points(geometry: Geometry) -> np.ndarray:
    """Each node's place in the global frame, [N, 3] in the geometry's node order: in
    meters when its units are defined, in its own units otherwise. Where `project_shapes`
    measures how far apart two geometries' nodes are."""
    from ..drawing.viz.geometry import display_points
    from ..units import SYSTEMS

    return np.asarray(display_points(geometry, SYSTEMS['m-kg-N-s'])[0], dtype=float)


def _extent(points: np.ndarray) -> float:
    """The largest distance of a node from the nodes' mean position; 1 for none."""
    if not len(points):
        return 1.0
    center = points.mean(axis=0)
    return float(np.sqrt(((points - center) ** 2).sum(axis=1).max())) or 1.0


def _surface(fem_geometry: Geometry) -> float:
    """Half the thickest plate of a geometry with units defined, 0 for one with none.

    A sensor sits on a plate's surface and a plate model is meshed at its mid-surface,
    half the thickness away: the BARC's box sensors all sat 0.125 in off the model's wall
    and every one was dropped (2026-09-26). The thickness is held in meters, so it is
    allowed only when the units are defined."""
    if not fem_geometry.units_defined:
        return 0.0
    thicknesses = [getattr(properties, 'thickness', None) for properties
                   in (getattr(fem_geometry, 'group_properties', None) or {}).values()]
    return 0.5 * max((t for t in thicknesses if t), default=0.0)


def match_distance(fem_geometry: Geometry, test_geometry: Geometry,
                   tolerance: float = 0.02) -> float:
    """How far a test node's nearest FEM node may be when `project_shapes` is given no
    distance: `tolerance` of the test geometry's extent (the largest distance of a test
    node from the nodes' mean position), plus half the FEM geometry's thickest plate,
    because sensors are on surfaces and plate nodes are on mid-surfaces. In meters when
    the geometries' units are defined, in the model's own units otherwise."""
    return float(tolerance) * _extent(si_points(test_geometry)) + _surface(fem_geometry)


def project_shapes(fem_shapes: ShapeSet, fem_geometry: Geometry,
                   test_shapes: ShapeSet, test_geometry: Geometry,
                   tolerance: float = 0.02, *, distance: float | None = None
                   ) -> tuple[ShapeSet, dict[str, Any]]:
    """`fem_shapes` reduced to the DOFs of `test_shapes`.

    A test node whose nearest FEM node is further than `distance` away
    is not matched, and its DOFs are dropped from the result rather
    than estimated. `distance` is in meters when the geometries' units
    are defined, in the model's own units otherwise. Without one, the
    limit is `match_distance`'s: `tolerance` of the test geometry's
    extent, plus half the FEM geometry's thickest plate.

    Returns (shape_set, report): the projected ShapeSet and a dict with
    'matched' and 'total' test node counts, the 'worst' matched
    distance and the 'limit' (both in meters when the units are
    defined), 'dropped', the test DOFs that could not be projected
    (those that are not translations on the test geometry, and those
    at unmatched nodes), 'pairs', each matched test node with the FEM
    node it was matched to as (test node id, FEM node id) in the test
    geometry's node order, and 'unmatched', the ids of the test nodes
    with no FEM node within the limit.

    Raises ValueError when only one geometry has units defined, when
    `distance` is not above zero, when either has no nodes, when none
    of the test DOFs are translations on the test geometry, or when no
    test node is matched.
    """
    from ..deform import dof_directions
    from .shapes import ShapeSet

    if fem_geometry.units_defined != test_geometry.units_defined:
        raise ValueError('both geometries need units defined (or neither)'
                         ' to overlay them in one frame')
    if distance is not None and not (np.isfinite(distance) and distance > 0):
        raise ValueError(f'the node match distance is {distance}; it must be above zero')
    test_points = si_points(test_geometry)
    fem_points = si_points(fem_geometry)
    if not len(test_points) or not len(fem_points):
        raise ValueError('both geometries need nodes')

    dofs = list(test_shapes.coordinate)
    rows, directions, used = dof_directions(test_geometry, dofs)
    if not len(rows):
        raise ValueError("none of the test shapes' DOFs are "
                         'translations on the test geometry')

    limit = (float(distance) if distance is not None
             else float(tolerance) * _extent(test_points) + _surface(fem_geometry))

    # nearest FEM node per unique test node, one row at a time — the
    # full (test x fem) distance matrix would not fit for a real mesh
    unique_rows = np.unique(rows)
    nearest, gap = {}, {}
    for row in unique_rows:
        deltas = fem_points - test_points[row]
        squared = (deltas ** 2).sum(axis=1)
        best = int(np.argmin(squared))
        nearest[int(row)] = best
        gap[int(row)] = float(np.sqrt(squared[best]))
    matched_rows = {row for row in nearest if gap[row] <= limit}
    if not matched_rows:
        closest = min(gap.values())
        unit = (' m' if fem_geometry.units_defined and test_geometry.units_defined
                else ' in model units')
        raise NoNodeMatched(
            'no test node has a FEM node within the match tolerance: the nearest is '
            f'{closest:.4g}{unit} away and the limit is {limit:.4g} — '
            'check that the geometries overlay, or loosen the tolerance',
            {'matched': 0, 'total': len(unique_rows), 'worst': closest, 'limit': limit,
             'dropped': [], 'pairs': [],
             'unmatched': [int(test_geometry.node_id[row]) for row in unique_rows]})

    keep = [i for i, row in enumerate(rows) if int(row) in matched_rows]
    used_indices = np.flatnonzero(used)
    coordinate = [dofs[used_indices[i]] for i in keep]
    fem_rows = np.asarray([nearest[int(rows[i])] for i in keep])
    kept_directions = np.asarray([directions[i] for i in keep])

    complex_shapes = np.iscomplexobj(fem_shapes.shape_matrix)
    values = np.empty((fem_shapes.num_shapes, len(keep)),
                      dtype=np.complex128 if complex_shapes else np.float64)
    # the model's DOFs are parsed once for every mode, and kept with the set: parsed per
    # mode, 38 modes of a 350,000-DOF model took 23 s; and each mode is summed only at the
    # nodes matched, not over the whole mesh
    model_rows, model_directions, model_used = dof_directions(
        fem_geometry, fem_shapes.coordinate, parsed=fem_shapes._parsed_dofs())
    near = np.isin(model_rows, fem_rows)
    model_rows, model_directions = model_rows[near], model_directions[near]
    columns = np.flatnonzero(model_used)[near]
    for m in range(fem_shapes.num_shapes):
        displacement = np.zeros((fem_geometry.num_nodes, 3),
                                dtype=values.dtype)
        if len(model_rows):
            moved = np.asarray(fem_shapes.shape_matrix[m])[columns]
            np.add.at(displacement, model_rows,
                      model_directions * moved[:, np.newaxis])
        sampled = displacement[fem_rows]
        if not complex_shapes:
            sampled = sampled.real
        values[m] = (kept_directions * sampled).sum(axis=1)

    dropped = ([dofs[i] for i in range(len(dofs)) if not used[i]]
               + [dofs[used_indices[i]] for i in range(len(rows))
                  if i not in keep])
    projected = ShapeSet(
        frequency=fem_shapes.frequency, damping=fem_shapes.damping,
        coordinate=coordinate, shape_matrix=values,
        modal_mass=fem_shapes.modal_mass,
        description=list(fem_shapes.description),
        comment=['projected to test DOFs'] * fem_shapes.num_shapes,
        mass_unit=fem_shapes.mass_unit,
        unscaled=getattr(fem_shapes, 'unscaled', False))
    # the pairing itself, by node id, so it can be checked: the counts
    # alone cannot show a test node matched to the wrong FEM node
    # (Brandon, 2026-11-13: which nodes were matched to each other)
    test_ids, fem_ids = test_geometry.node_id, fem_geometry.node_id
    pairs = [(int(test_ids[row]), int(fem_ids[nearest[int(row)]]))
             for row in unique_rows if int(row) in matched_rows]
    unmatched = [int(test_ids[row]) for row in unique_rows
                 if int(row) not in matched_rows]
    report = {'matched': len(matched_rows), 'total': len(unique_rows),
              'worst': max(gap[row] for row in matched_rows),
              'limit': limit, 'dropped': dropped, 'pairs': pairs,
              'unmatched': unmatched}
    return projected, report


def expand_shapes(test_shapes: ShapeSet, test_geometry: Geometry,
                  fem_shapes: ShapeSet, fem_geometry: Geometry, *,
                  basis: Any = None, tolerance: float = 0.02,
                  distance: float | None = None) -> tuple[ShapeSet, dict[str, Any]]:
    """`test_shapes` expanded to every DOF of `fem_shapes` through the model's modes
    (SEREP). The model's modes are reduced to the test DOFs (`project_shapes`, nearest
    node within the distance and each sensor's direction), each test mode is fitted at
    those DOFs as a combination of them by least squares, `psi_a = Q phi_a`, and the
    combination is taken at every model DOF, `psi = Q phi`. A test mode that lies in the
    span of the modes used is recovered exactly; one that does not is the nearest
    combination, and its residual says how far off the fit is at the test DOFs.

    `basis` are the model modes the fit is built on, indices into its modes; left out,
    every one. There may be no more of them than test DOFs matched: with more, the fit
    is not unique.

    Returns (shape_set, report): the expanded set, the test's frequencies and damping
    at the model's DOFs, and `project_shapes`' report with 'modes' (the model modes used),
    'condition' (of their reduced shapes, the larger the less the test DOFs tell them
    apart) and 'residual' (each test mode's misfit at the test DOFs over its size).

    Raises ValueError as `project_shapes`, when a mode named is not the model's, when no
    mode is named, or when more modes are named than test DOFs matched.
    """
    from .shapes import ShapeSet

    reduced, report = project_shapes(fem_shapes, fem_geometry, test_shapes, test_geometry,
                                     tolerance=tolerance, distance=distance)
    kept = [str(dof) for dof in reduced.coordinate]
    where = {str(dof): k for k, dof in enumerate(test_shapes.coordinate)}
    psi = np.asarray(test_shapes.shape_matrix)[:, [where[dof] for dof in kept]]
    modes = list(range(fem_shapes.num_shapes)) if basis is None else [int(k) for k in basis]
    outside = [k for k in modes if not 0 <= k < fem_shapes.num_shapes]
    if outside:
        raise ValueError(f'the model has {fem_shapes.num_shapes} modes; '
                         f'{[k + 1 for k in outside]} are not among them')
    if not modes:
        raise ValueError('no model mode to expand through: pick at least one')
    if len(modes) > len(kept):
        raise ValueError(f'{len(modes)} model modes for {len(kept)} test DOFs matched: '
                         'SEREP needs no more modes than DOFs, so pick fewer model modes')
    phi_a = np.asarray(reduced.shape_matrix)[modes]
    fit = psi @ np.linalg.pinv(phi_a)
    misfit = psi - fit @ phi_a
    size = np.linalg.norm(psi, axis=1)
    residual = np.where(size > 0, np.linalg.norm(misfit, axis=1) / np.where(size > 0, size, 1.0),
                        0.0)
    expanded = ShapeSet(
        frequency=test_shapes.frequency, damping=test_shapes.damping,
        coordinate=list(fem_shapes.coordinate),
        shape_matrix=fit @ np.asarray(fem_shapes.shape_matrix)[modes],
        modal_mass=test_shapes.modal_mass,
        description=list(test_shapes.description),
        comment=['expanded to model DOFs'] * test_shapes.num_shapes,
        mass_unit=test_shapes.mass_unit,
        unscaled=getattr(test_shapes, 'unscaled', False))
    report = {**report, 'modes': modes, 'condition': float(np.linalg.cond(phi_a)),
              'residual': [float(r) for r in residual]}
    return expanded, report
