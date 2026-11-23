"""Strain and stress shapes: a solved model's modes read element by element.

A mode shape is a displacement at every node. Its strain is the
displacement's derivative, and in a finite element model that derivative
is continuous inside an element and jumps from one element to the next,
so strain and stress belong to the elements, not the nodes. An element
shape set holds them that way, as an Exodus file holds element
variables: for every mode, the six strain components and the six stress
components at each solid element's centroid and at the points of the
volume rule its stiffness was integrated with, in the global axes and
in SI.

The values come from the element's own strain matrix and elasticity
(`fem._solid_strains_many`, the gradients its stiffness is built from),
so nothing about the element is written twice. For the brick, Wilson's
incompatible modes are recovered from the nodal displacements before
the strain is formed, through the same static condensation the
stiffness used. Read at the rule's points with their weights, the
stress and strain give back each mode's strain energy: for a
mass-normalized set, half of the sum over every element of weight times
stress dot strain is omega^2 / 2.

Strain is per unit modal coordinate, as the shapes it is read from are.
The shear strains are engineering shears (gamma_xy = du/dy + dv/dx),
the convention the elasticity matrix and a strain gauge's rosette use.
Each row of the set is a stress point: one element at one point. That
is the row a benchmark's stress table is written in, and the
components, not von Mises or principal values, are what it needs: they
are linear in the modal coordinates, so a response's root mean square
and its rates follow from the modal displacements alone.

Solids are covered: the eight-node brick, the six-node wedge and the
four- and ten-node tetrahedra (element types 115, 112, 111 and 118, and
the thick-element types 104 and 101 that build the same solids). Plates,
beams and every other element are refused by name; point masses,
grounds, springs and rigid links hold no strain field and are passed
over when no groups are named.
"""

from __future__ import annotations

from collections.abc import Sequence
from typing import Any

import numpy as np

from .geometry import ELEMENT_TYPES, Geometry
from .shapes import ShapeSet

#: the six components of a strain or stress, in the order the
#: elasticity matrix takes them; xy, yz and xz strains are engineering
#: shears
COMPONENTS = ('xx', 'yy', 'zz', 'xy', 'yz', 'xz')
#: what an element shape set holds of each component
QUANTITIES = ('strain', 'stress')
#: the most an element shape set's strain and stress may take together,
#: in bytes. A whole model is large: the RC plate's 52,000 bricks read
#: at nine points each, for sixty modes, are 2.7 GB, so a set this size
#: is refused with the count, and its groups or modes chosen instead
LIMIT = 2 ** 30
#: how many elements are read at once, which bounds the working arrays
CHUNK = 4096
#: the kinds of element group that hold no strain field, passed over
#: when no groups are named
_NO_STRAIN = ('mass', 'ground', 'spring', 'rigid')


class ElementShapeSet:
    """Strain and stress per mode at each solid element's stress points.

    Every row is a stress point: one element at one point, the centroid
    (point 0, weight 0) and then the points of the volume rule the
    element's stiffness was integrated with (points 1 on, each weighted
    by its share of the element's volume). A brick has nine, a wedge
    seven, a ten-node tetrahedron 28 and a four-node one two (its one
    rule point is its centroid).

    Attributes:
        frequency: Hz per mode, as the shape set's.
        damping: Fraction of critical per mode, as the shape set's.
        element: The element id of each row.
        point: Each row's point in its element: 0 the centroid, 1 on
            the rule's points in the element's own order.
        xyz: Each row's position, (rows, 3), global, in meters (in the
            geometry's own numbers while its unit is undeclared).
        weight: Each row's share of its element's volume, m^3; zero at
            the centroid, and summing over an element to its volume.
        strain: (modes, rows, 6), the components in `COMPONENTS` order,
            per unit modal coordinate.
        stress: (modes, rows, 6), Pa per unit modal coordinate.
        mass_unit: The mass unit the modes were normalized against, or
            None when it was not declared.
        length_unit: The geometry's length unit, or None when it was not
            declared.
        comment: One line, what the set was read from.
    """

    def __init__(self, frequency: Any, damping: Any, element: Any, point: Any,
                 xyz: Any, weight: Any, strain: Any, stress: Any,
                 mass_unit: str | None = None, length_unit: str | None = None,
                 comment: str = '') -> None:
        self.frequency: np.ndarray = np.asarray(frequency, dtype=np.float64).reshape(-1)
        self.damping: np.ndarray = np.asarray(damping, dtype=np.float64).reshape(-1)
        self.element: np.ndarray = np.asarray(element, dtype=np.int64).reshape(-1)
        self.point: np.ndarray = np.asarray(point, dtype=np.int64).reshape(-1)
        self.xyz: np.ndarray = np.asarray(xyz, dtype=np.float64).reshape(-1, 3)
        self.weight: np.ndarray = np.asarray(weight, dtype=np.float64).reshape(-1)
        self.strain: np.ndarray = np.asarray(strain)
        self.stress: np.ndarray = np.asarray(stress)
        rows, modes = len(self.element), len(self.frequency)
        if len(self.damping) != modes:
            raise ValueError(f'{modes} frequencies but {len(self.damping)} damping values')
        for name in ('point', 'xyz', 'weight'):
            if len(getattr(self, name)) != rows:
                raise ValueError(f'{rows} elements but {len(getattr(self, name))} '
                                 f'{name} rows')
        for name in QUANTITIES:
            if getattr(self, name).shape != (modes, rows, 6):
                raise ValueError(f'{name} is {getattr(self, name).shape}, not '
                                 f'({modes}, {rows}, 6): modes, stress points, components')
        self.mass_unit: str | None = mass_unit or None
        self.length_unit: str | None = length_unit or None
        self.comment: str = str(comment)

    @property
    def num_shapes(self) -> int:
        """How many modes."""
        return len(self.frequency)

    @property
    def num_points(self) -> int:
        """How many stress points (rows)."""
        return len(self.element)

    @property
    def elements(self) -> np.ndarray:
        """The element ids, each once, in the order the rows hold them."""
        return self.element[self.point == 0]

    def rows_of(self, element: int) -> np.ndarray:
        """The rows of one element, its centroid first.

        Parameters
        ----------
        element : int
            The element's id.

        Returns
        -------
        numpy.ndarray
            Row indices; refused when the set does not hold the element.
        """
        rows = np.flatnonzero(self.element == int(element))
        if not len(rows):
            raise KeyError(f'element {int(element)} is not in this set')
        return rows

    def values(self, quantity: str, component: str, mode: int,
               at: str = 'centroid') -> np.ndarray:
        """One component of one mode: per element at its centroid, or
        at every stress point.

        Parameters
        ----------
        quantity : {'strain', 'stress'}
        component : str
            One of `COMPONENTS`.
        mode : int
            The mode, from 0.
        at : {'centroid', 'points'}, default 'centroid'
            Each element's centroid, in `elements` order, or every row.

        Returns
        -------
        numpy.ndarray
        """
        if quantity not in QUANTITIES:
            raise ValueError(f'{quantity!r} is not strain or stress')
        if component not in COMPONENTS:
            raise ValueError(f'{component!r} is not a component: '
                             + ', '.join(COMPONENTS))
        if not 0 <= int(mode) < self.num_shapes:
            raise IndexError(f'mode {int(mode) + 1} of {self.num_shapes}')
        if at not in ('centroid', 'points'):
            raise ValueError(f'{at!r} is not centroid or points')
        held = getattr(self, quantity)[int(mode), :, COMPONENTS.index(component)]
        return held[self.point == 0] if at == 'centroid' else held

    def strain_energy(self) -> np.ndarray:
        """Each mode's strain energy over the elements held: half the
        sum over the rule's points of weight times stress dot strain.
        For a mass-normalized set read over every element it is
        omega^2 / 2.

        Returns
        -------
        numpy.ndarray
            One value per mode, J per unit modal coordinate squared.
        """
        products = np.real(np.conj(self.stress) * self.strain).sum(axis=2)
        return 0.5 * products @ self.weight

    def kept(self, modes: Sequence[int] | None) -> ElementShapeSet:
        """A copy with only these modes, in this order; every mode when None."""
        chosen = (np.arange(self.num_shapes) if modes is None
                  else np.asarray([int(m) for m in modes], dtype=np.int64))
        return ElementShapeSet(self.frequency[chosen], self.damping[chosen],
                               self.element.copy(), self.point.copy(), self.xyz.copy(),
                               self.weight.copy(), self.strain[chosen], self.stress[chosen],
                               self.mass_unit, self.length_unit, self.comment)

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, ElementShapeSet):
            return NotImplemented
        return (self.mass_unit == other.mass_unit
                and self.length_unit == other.length_unit
                and all(np.array_equal(getattr(self, name), getattr(other, name))
                        for name in ('frequency', 'damping', 'element', 'point', 'xyz',
                                     'weight', 'strain', 'stress')))

    __hash__ = None  # type: ignore[assignment]

    def __repr__(self) -> str:
        return (f'ElementShapeSet({self.num_shapes} modes, {len(self.elements)} elements, '
                f'{self.num_points} stress points)')


def _picked_modes(shapes: ShapeSet, modes: Sequence[int] | None) -> np.ndarray:
    """The modes to read, by index: every one when None; a set with no
    modes, an empty choice and a mode past the set's last are refused."""
    if not shapes.num_shapes:
        raise ValueError('the shape set has no modes')
    picked = (np.arange(shapes.num_shapes) if modes is None
              else np.asarray([int(m) for m in modes], dtype=np.int64))
    if not len(picked):
        raise ValueError('no modes were chosen')
    outside = picked[(picked < 0) | (picked >= shapes.num_shapes)]
    if len(outside):
        raise ValueError(f'mode {int(outside[0]) + 1} is not in the shape set, '
                         f'which has {shapes.num_shapes}')
    return picked


def _elements_chosen(geometry: Geometry, groups: Sequence[int] | None) -> np.ndarray:
    """The rows of the geometry's elements to read, in its order: those
    of `groups`, or of every group that holds solids. An element that is
    not a solid is refused by its group and type."""
    from .fem import SOLID_CODES

    group_of = np.asarray(geometry.elem_group, dtype=np.int64)
    names = {int(g): name for g, name in zip(geometry.group_id, geometry.group_name)}
    properties = dict(getattr(geometry, 'group_properties', {}) or {})

    def named(group: int) -> str:
        label = names.get(group, '')
        return f'element group {group}' + (f' ({label})' if label else '')

    if groups is None:
        chosen = np.ones(len(geometry.elem_type), dtype=bool)
    else:
        wanted = [int(g) for g in groups]
        if not wanted:
            raise ValueError('no element groups were chosen: choose the solid groups '
                             'to read, or leave groups out for all of them')
        missing = [g for g in wanted if g not in names and not np.any(group_of == g)]
        if missing:
            raise ValueError(f'the geometry has no element group {missing[0]}')
        chosen = np.isin(group_of, wanted)
    rows = []
    for row in np.flatnonzero(chosen):
        kind = int(geometry.elem_type[row])
        shape_name, _count, shape = ELEMENT_TYPES.get(kind, (f'type {kind}', 0, 'unknown'))
        nodes = len(geometry.elem_conn[row])
        if shape == 'volume' and nodes in SOLID_CODES:
            rows.append(row)
        elif (groups is None and getattr(properties.get(int(group_of[row])), 'kind', '')
              in _NO_STRAIN):
            continue
        else:
            group = int(group_of[row])
            raise ValueError(
                f'{named(group)} holds {shape_name} elements (type {kind}), which have '
                'no strain and stress recovery: bricks, wedges and tetrahedra only. '
                'Choose the solid groups')
    if not rows:
        where = ('the groups chosen' if groups is not None else 'the geometry')
        raise ValueError(f'{where} hold no solid elements to read strain and stress at')
    return np.asarray(rows, dtype=np.int64)


def _translations(shapes: ShapeSet, geometry: Geometry, nodes: np.ndarray,
                  modes: np.ndarray) -> np.ndarray:
    """(nodes, 3, modes): each node's global translations in each mode.
    A node in a turned displacement frame has its components turned back
    to the global axes; a node the set has no translation at, or one
    with no value, is refused by name."""
    from .fem import _displacement_frames

    lookup = shapes._dof_lookup()
    matrix = shapes.shape_matrix[modes]
    out = np.zeros((len(nodes), 3, len(modes)), dtype=matrix.dtype)
    for axis, letter in enumerate('XYZ'):
        columns = np.empty(len(nodes), dtype=np.int64)
        signs = np.empty(len(nodes))
        for k, node in enumerate(nodes):
            held = lookup.get(f'{int(node)}{letter}')
            if held is None:
                raise ValueError(
                    f'the shape set has no {int(node)}{letter}+: strain is read from '
                    "the modes at every node of the model's solids, and a set at a "
                    "test's DOFs does not have them. Choose the solved shape set")
            columns[k], signs[k] = held
        out[:, axis] = (matrix[:, columns] * signs).T
    bad = ~np.isfinite(out)
    if bad.any():
        node, axis, mode = (int(v) for v in np.argwhere(bad)[0])
        raise ValueError(f'mode {int(modes[mode]) + 1} has no value at '
                         f'{int(nodes[node])}{"XYZ"[axis]}+')
    frames = _displacement_frames(geometry)
    if frames:
        for k, node in enumerate(nodes):
            rotation = frames.get(int(node))
            if rotation is not None:
                # local = R global, so global = R^T local
                out[k] = rotation.T @ out[k]
    return out


def element_shapes(geometry: Geometry, shapes: ShapeSet,
                   groups: Sequence[int] | None = None,
                   modes: Sequence[int] | None = None,
                   comment: str = '') -> ElementShapeSet:
    """The strain and stress of a solved model's modes at each solid
    element's stress points.

    Each element is read at its centroid and at its volume rule's points
    from its own strain matrix and elasticity (`fem._solid_strains_many`),
    in the coordinates the model was solved in: a node a mated surface
    ties is moved onto the point it is tied to, as `fem.Model.from_geometry`
    moves it. The brick's incompatible modes are recovered from the nodal
    displacements first.

    Parameters
    ----------
    geometry : Geometry
        The model the shapes were solved on, its element groups carrying
        their materials (`geometry.group_properties`).
    shapes : ShapeSet
        The solved modes, at every node of the elements read.
    groups : sequence of int, optional
        The element groups to read, by id. Every group of solids when
        omitted, point elements passed over; a named group holding
        anything but solids is refused.
    modes : sequence of int, optional
        The modes, by index from 0. Every mode when omitted.
    comment : str, optional
        What the set was read from.

    Returns
    -------
    ElementShapeSet
    """
    from .fem import _solid_strains_many, mated_ties

    picked = _picked_modes(shapes, modes)
    rows = _elements_chosen(geometry, groups)
    sizes = np.array([len(geometry.elem_conn[r]) for r in rows])
    # a centroid and then the rule's points: 9, 7, 2 and 28 rows
    per_element = {8: 9, 6: 7, 4: 2, 10: 28}
    total = int(sum(per_element[n] for n in sizes))
    needed = 2 * total * 6 * len(picked) * 8
    if needed > LIMIT:
        raise ValueError(
            f'{len(rows):,} elements at {total:,} stress points for {len(picked)} modes '
            f'is {needed / 2 ** 30:.1f} GB of strain and stress, past the '
            f'{LIMIT / 2 ** 30:.0f} GB an element shape set holds: choose fewer element '
            'groups or fewer modes')

    properties = dict(getattr(geometry, 'group_properties', {}) or {})
    group_of = np.asarray(geometry.elem_group, dtype=np.int64)
    names = {int(g): name for g, name in zip(geometry.group_id, geometry.group_name)}
    for group in sorted({int(group_of[r]) for r in rows}):
        props = properties.get(group)
        label = f'element group {group}' + (f' ({names[group]})' if names.get(group) else '')
        if props is None or props.material is None:
            raise ValueError(f'{label} has no material, so its stress cannot be read')
        if props.kind != 'solid':
            raise ValueError(f'{label} holds solids and has {props.kind} properties: '
                             'a group of solids takes a material alone')

    positions = np.asarray(geometry.node_xyz, dtype=np.float64).copy()
    index = {int(n): k for k, n in enumerate(geometry.node_id)}
    if getattr(geometry, 'mates', None):
        for _mated, _nodes, _weights, node, _normal, point in mated_ties(geometry):
            positions[index[int(node)]] = point
    connectivity = [np.asarray(geometry.elem_conn[r], dtype=np.int64) for r in rows]
    used = np.unique(np.concatenate(connectivity))
    motion = _translations(shapes, geometry, used, picked)
    where = {int(n): k for k, n in enumerate(used)}

    count, dtype = len(picked), motion.dtype
    strain = np.empty((count, total, 6), dtype=dtype)
    stress = np.empty((count, total, 6), dtype=dtype)
    element = np.empty(total, dtype=np.int64)
    point = np.empty(total, dtype=np.int64)
    xyz = np.empty((total, 3))
    weight = np.empty(total)
    starts = np.concatenate([[0], np.cumsum([per_element[n] for n in sizes])])
    # one material and one kind at a time, as the stiffness is built
    batches: dict[tuple[int, int], list[int]] = {}
    for k, r in enumerate(rows):
        batches.setdefault((int(group_of[r]), int(sizes[k])), []).append(k)
    for (group, nodes), members in batches.items():
        material = properties[group].material
        width = per_element[nodes]
        for lo in range(0, len(members), CHUNK):
            chunk = np.asarray(members[lo:lo + CHUNK])
            conn = np.array([connectivity[k] for k in chunk])
            local = np.vectorize(where.__getitem__, otypes=[np.int64])(conn)
            u = motion[local].reshape(len(chunk), 3 * nodes, count)
            node_rows = np.vectorize(index.__getitem__, otypes=[np.int64])(conn)
            e, s, p, w = _solid_strains_many(material, positions[node_rows], u)
            at = (starts[chunk][:, None] + np.arange(width)).ravel()
            strain[:, at] = e.reshape(-1, 6, count).transpose(2, 0, 1)
            stress[:, at] = s.reshape(-1, 6, count).transpose(2, 0, 1)
            xyz[at] = p.reshape(-1, 3)
            weight[at] = w.ravel()
            element[at] = np.repeat(np.asarray(geometry.elem_id)[rows[chunk]], width)
            point[at] = np.tile(np.arange(width), len(chunk))
    return ElementShapeSet(shapes.frequency[picked], shapes.damping[picked], element,
                           point, xyz, weight, strain, stress,
                           mass_unit=shapes.mass_unit,
                           length_unit=geometry.length_unit if geometry.units_defined else None,
                           comment=comment)


#: each node of a solid in its natural coordinates, by node count, in the
#: element's node order: where a gauge on that node reads the element
_NATURAL_NODES = {
    8: [tuple(float(v) for v in corner) for corner in
        ((-1, -1, -1), (1, -1, -1), (1, 1, -1), (-1, 1, -1),
         (-1, -1, 1), (1, -1, 1), (1, 1, 1), (-1, 1, 1))],
    6: [(0.0, 0.0, -1.0), (1.0, 0.0, -1.0), (0.0, 1.0, -1.0),
        (0.0, 0.0, 1.0), (1.0, 0.0, 1.0), (0.0, 1.0, 1.0)],
    4: [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0)],
    # the corners, then the midsides of edges 1-2, 2-3, 1-3, 1-4, 2-4, 3-4
    10: [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0),
         (0.5, 0.0, 0.0), (0.5, 0.5, 0.0), (0.0, 0.5, 0.0),
         (0.0, 0.0, 0.5), (0.5, 0.0, 0.5), (0.0, 0.5, 0.5)],
}
#: how far a gauge's direction may lean out of the surface it is bonded
#: to, in degrees: a faceted mesh of a curved surface leans its faces by
#: a few degrees, and a direction further out than this is not one a
#: gauge on that surface can read
SURFACE_LEAN = 10.0


def _surface_normals(geometry: Geometry, node: int, rows: list[int]) -> list[np.ndarray]:
    """The unit normals of the outside faces at `node` of the solids in
    `rows` (geometry element rows): the faces holding the node that no
    second solid there shares. A face two solids share has both of them
    on the node, so the solids at the node are all it takes."""
    from .geometry import solid_faces

    index = {int(n): k for k, n in enumerate(geometry.node_id)}
    xyz = np.asarray(geometry.node_xyz, dtype=np.float64)
    faces: dict[tuple[int, ...], tuple[int, ...]] = {}
    for row in rows:
        conn = [int(n) for n in geometry.elem_conn[row]]
        for face in solid_faces(len(conn)):
            corners = tuple(conn[k] for k in face)
            if node not in corners:
                continue
            key = tuple(sorted(corners))
            if key in faces:
                del faces[key]
            else:
                faces[key] = corners
    normals = []
    for corners in faces.values():
        p = xyz[[index[c] for c in corners]]
        normal = np.cross(p[1] - p[0], p[-1] - p[0])
        size = np.linalg.norm(normal)
        if size > 0.0:
            normals.append(normal / size)
    return normals


def gauge_strains(geometry: Geometry, shapes: ShapeSet,
                  gauges: Sequence[tuple[int, Sequence[float]]],
                  modes: Sequence[int] | None = None) -> np.ndarray:
    """The strain a gauge on a solid's surface reads in each mode: at a
    node, along a direction in the surface.

    Each solid meeting at the node is read at that node through its own
    strain matrix (`fem._solid_strains_many` at the node's natural
    coordinates, the brick's incompatible modes recovered), and the
    tensors are averaged over those solids. The gauge reads the normal
    strain along its direction d, the tensor's d . E . d, so a gauge
    turned end for end reads the same. It is the strain at a point: a
    gauge averages over its grid, which a mesh finer than the grid
    resolves and a coarser one does not.

    Parameters
    ----------
    geometry : Geometry
        The model the shapes were solved on.
    shapes : ShapeSet
        The solved modes, with translations at every node of the solids
        at each gauge.
    gauges : sequence of (int, sequence of float)
        Each gauge's node and its direction in the global axes (any
        length; it is made a unit vector).
    modes : sequence of int, optional
        The modes, by index from 0. Every mode when omitted.

    Returns
    -------
    numpy.ndarray
        (modes, gauges), strain per unit modal coordinate.
    """
    from .fem import SOLID_CODES, _solid_strains_many

    picked = _picked_modes(shapes, modes)
    if not len(gauges):
        raise ValueError('no gauges were given')
    properties = dict(getattr(geometry, 'group_properties', {}) or {})
    group_of = np.asarray(geometry.elem_group, dtype=np.int64)
    solid_groups = {int(g) for g, props in properties.items()
                    if getattr(props, 'kind', '') == 'solid'}
    at_node: dict[int, list[int]] = {}
    for row, conn in enumerate(geometry.elem_conn):
        if int(group_of[row]) in solid_groups and len(conn) in SOLID_CODES:
            for n in conn:
                at_node.setdefault(int(n), []).append(row)
    index = {int(n): k for k, n in enumerate(geometry.node_id)}
    xyz = np.asarray(geometry.node_xyz, dtype=np.float64)
    lean = np.sin(np.radians(SURFACE_LEAN))
    out = np.zeros((len(picked), len(gauges)), dtype=shapes.shape_matrix.dtype)
    for g, (node, direction) in enumerate(gauges):
        node = int(node)
        d = np.asarray(direction, dtype=np.float64).reshape(3)
        size = np.linalg.norm(d)
        if not np.isfinite(size) or size == 0.0:
            raise ValueError(f'the gauge at node {node} has no direction')
        d = d / size
        if node not in index:
            raise ValueError(f'the geometry has no node {node} for a gauge')
        rows = at_node.get(node)
        if not rows:
            raise ValueError(f'node {node} is on no solid element with a material, so a '
                             'gauge there reads nothing')
        normals = _surface_normals(geometry, node, rows)
        if not normals:
            raise ValueError(f'node {node} is inside the solid: a gauge sits on its surface')
        leans = [abs(float(d @ n)) for n in normals]
        if min(leans) > lean:
            angle = float(np.degrees(np.arcsin(min(min(leans), 1.0))))
            raise ValueError(
                f'the gauge at node {node} points {angle:.0f} degrees out of the surface '
                f'there, past the {SURFACE_LEAN:g} a gauge bonded to it can read: give '
                'it a direction in the surface')
        tensors = []
        for row in rows:
            conn = [int(n) for n in geometry.elem_conn[row]]
            corner = _NATURAL_NODES[len(conn)][conn.index(node)]
            motion = _translations(shapes, geometry, np.asarray(conn), picked)
            strain, _stress, _p, _w = _solid_strains_many(
                properties[int(group_of[row])].material, xyz[[index[c] for c in conn]][None],
                motion.reshape(1, 3 * len(conn), len(picked)), at=corner)
            tensors.append(strain[0, 0])                       # (6, modes)
        e = np.mean(tensors, axis=0)
        out[:, g] = (e[0] * d[0] ** 2 + e[1] * d[1] ** 2 + e[2] * d[2] ** 2
                     + e[3] * d[0] * d[1] + e[4] * d[1] * d[2] + e[5] * d[0] * d[2])
    return out
