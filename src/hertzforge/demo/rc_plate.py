"""The BARC's removable component bolted to a fixture plate, a synthetic test system.

The removable component (the two C-channels and the beam, in bricks, as
`barc.solid_geometry` builds it) is bolted at its feet to a 10 x 1 x 10 in
6061-T6 plate, free-free. The plate's top face is where the BARC's box top
is, y = 3 in, and the plate is centered under the component, so the
component sits at the BARC's own coordinates and its accelerometers are at
the same places. The feet are joined to the plate as they are to the box: a
rigid mated surface under each bolt's washer, and each bolt's mass at its
head.

Twelve forces drive the plate, three at each corner. Each corner's three
lines meet at a point `INSET` in from both side edges at the plate's
mid-thickness: one force on the side face normal to x, one on the side face
normal to z, both at mid-thickness, and one on the bottom face, each along
its face's inward normal. Sixteen triaxial accelerometers sit on the
component's top and the channels' outer faces (`CONTROLS`), each measured in
its own frame, +Z the outward normal of its mounting surface.

The channels sit on a test geometry made the way Plan Instrumentation makes
one: a node per channel location, the drives from `DRIVE_IDS` in the global
system and the accelerometers from `ACCEL_IDS`, each in its surface's frame.
The test geometry and the channel table are linked as the Basis group. With
the modes solved (to `SOLVE_TO`, each `DAMPING` of critical), they are
projected onto every channel's DOF ('RC Plate Modes @ Test DOFs'), which
joins that group: the twelve drives and the 48 accelerometer axes, what a
state-space export for a controller's simulation takes.

This is the system the band-average control law study drives (2026-11-10).
At `barc.SIZE` it has 60,903 nodes and 38 modes to 10 kHz, the first elastic
one at 418.15 Hz; solving takes a few minutes and about 6 GB.

    from hertzforge.demo import rc_plate

    rc_plate.project(solved=True).save('rc-plate.hforge')

Run it directly to print the model and its first modes:

    python3 -m hertzforge.demo.rc_plate
"""

from __future__ import annotations

from typing import Any

import numpy as np

from hertzforge.demo import barc

#: the plate, inches: (center, widths); x -5..5, y 2..3, z -3.5..6.5
PLATE = ((0.0, 2.5, 1.5), (10.0, 1.0, 10.0))
#: how far in from both side edges each corner's three drives meet, inches
INSET = 0.5
#: the modes: every mode to this frequency, Hz, each given this fraction of
#: critical damping, as the BARC study's are
SOLVE_TO = 10000.0
DAMPING = 0.01
#: the test geometry's node numbers, first of each run
ACCEL_IDS = 101
DRIVE_IDS = 201

#: the accelerometers in pairs, (x, y) in inches and the outward normal of the
#: surface each is on: the beam's top and the right channel's web lie on lines
#: parallel to z at z = 1.25 and 1.75. The left channel's outer face is its web
#: at z = 1 (its C opens toward +z), so its pairs run along x instead.
_PAIRS = {
    **{f'beam top x={x:+g}': ((x, 5.125), (0, 1, 0)) for x in (-1.25, -0.5, 0.5, 1.25)},
    'right channel web, upper': ((2.5, 4.5), (1, 0, 0)),
    'right channel web, lower': ((2.5, 3.5), (1, 0, 0)),
}
#: {label: (point, outward normal)} for the sixteen triaxial accelerometers, inches
CONTROLS = {f'{label}, z={z:g}': ((x, y, z), normal)
            for label, ((x, y), normal) in _PAIRS.items() for z in (1.25, 1.75)}
for _level, _y in (('upper', 4.5), ('lower', 3.5)):
    for _x in (-2.25, -1.75):
        CONTROLS[f'left channel web, {_level}, x={_x:g}'] = ((_x, _y, 1.0), (0, 0, -1))


def drives() -> dict[str, tuple[tuple[float, float, float], str]]:
    """{label: (point, direction)} for the twelve forces, inches, each along its
    face's inward normal."""
    (cx, cy, cz), (wx, wy, wz) = PLATE
    out = {}
    for sx in (-1, 1):
        for sz in (-1, 1):
            corner = f'{"-+"[sx > 0]}X {"-+"[sz > 0]}Z corner'
            x_face, z_face = cx + sx * wx / 2, cz + sz * wz / 2
            x_in, z_in = x_face - sx * INSET, z_face - sz * INSET
            out[f'{corner}, x face'] = ((x_face, cy, z_in), 'X+' if sx < 0 else 'X-')
            out[f'{corner}, z face'] = ((x_in, cy, z_face), 'Z+' if sz < 0 else 'Z-')
            out[f'{corner}, bottom'] = ((x_in, cy - wy / 2, z_in), 'Y+')
    return out


def solid_geometry(size: float = barc.SIZE) -> Any:
    """The plate and the removable component as bricks and point masses, joined
    at the feet and at the beam as `barc.solid_geometry` joins them.

    Parameters
    ----------
    size : float
        The element size aimed at, in inches.

    Returns
    -------
    Geometry
    """
    from hertzforge.core import fem
    from hertzforge.project import Project

    project = Project('RC Plate')
    name = project.new_geometry('RC Plate', unit='in')
    parts = [project.add_solid_block(name, *PLATE, name='plate', unit='in')]
    for side, blocks in barc.CHANNEL_SOLIDS.items():
        made = [project.add_solid_block(name, center, widths,
                                        name=side if k == 0 else '', unit='in')
                for k, (center, widths) in enumerate(blocks)]
        parts.append(project.combine_solids(name, 'subtract', made))
    parts.append(project.add_solid_block(name, *barc.BEAM_SOLID, name='beam', unit='in'))
    project.mesh_solids(name, parts, size, unit='in', parts_apart=True)
    whole = project[name]
    whole.view = barc.VIEW
    whole.group_properties = {int(group): fem.GroupProperties(barc.MATERIAL)
                              for group in whole.group_id}
    ids = dict(zip(whole.group_name, whole.group_id.tolist(), strict=True))
    for kind in ('foot', 'top'):
        for side in ('right channel', 'left channel'):
            below, above = ('plate', side) if kind == 'foot' else (side, 'beam')
            places = sorted({node for (bx, bz), k in barc.SOLID_BOLTS if k == kind
                             and (bx > 0) == (side == 'right channel')
                             for node in barc.bolt_places(whole, (bx, bz), kind)})
            groups = [ids[below], ids[above]]
            patch, = project.detect_mated_surfaces(
                name, groups, 0.25 * whole.node_spacing(groups))
            clamped = set(places) | set(barc._beneath(whole, ids[below], places))
            project.add_mated_surfaces(
                name, patch['groups'], [n for n in patch['nodes_a'] if n in clamped],
                patch['nodes_b'], name=f'{side.split()[0]} {kind} bolts')
    xyz = whole.node_xyz / barc.INCH
    for kind in ('foot', 'top'):
        nodes = []
        for (bx, bz), k in barc.SOLID_BOLTS:
            if k != kind:
                continue
            on_face = np.flatnonzero(np.abs(xyz[:, 1] - barc.HEAD_Y[kind]) < 1e-6)
            far = np.hypot(xyz[on_face, 0] - bx, xyz[on_face, 2] - bz)
            tied = on_face[far <= far.min() + 1e-9]
            nearest = tied[np.lexsort((xyz[tied, 2], xyz[tied, 0]))[0]]
            nodes.append([int(whole.node_id[nearest])])
        group = whole.add_group(f'{kind} bolts')
        whole.add_elements(nodes, [161] * len(nodes), [group] * len(nodes))
        whole.group_properties[group] = fem.GroupProperties(
            mass=barc.BOLT_MASS[kind] * barc.POUND)
    return whole


def node_at(geometry: Any, point: tuple[float, float, float]) -> int:
    """The id of the node of `geometry` at `point`, inches."""
    xyz = geometry.node_xyz / barc.INCH
    gap = np.linalg.norm(xyz - np.asarray(point), axis=1)
    k = int(gap.argmin())
    if gap[k] > 1e-9:
        raise ValueError(f'no node at {point} in; the nearest is {gap[k]:.3g} in off')
    return int(geometry.node_id[k])


def channels(model: Any) -> tuple[Any, Any]:
    """The test geometry and channel table on `model`: a node per drive and per
    accelerometer, at the FE node there, each accelerometer in its surface's
    frame; the drives in lbf and the accelerometers in g.

    Returns
    -------
    (Geometry, ChannelTable)
    """
    from hertzforge import ChannelTable, Geometry
    from hertzforge.core.instrumentation import surface_frame

    rows, ids, fe_nodes, frames = [], [], [], []
    for node, (label, (point, direction)) in enumerate(drives().items(), start=DRIVE_IDS):
        fe = node_at(model, point)
        ids.append(node)
        fe_nodes.append(fe)
        frames.append(None)
        rows.append({'node': node, 'direction': direction, 'role': 'reference',
                     'control': 'False', 'channel_type': 'force', 'unit': 'lbf',
                     'comment': f'{label}, {direction[0]} drive (FE node {fe})'})
    for node, (label, (point, normal)) in enumerate(CONTROLS.items(), start=ACCEL_IDS):
        fe = node_at(model, point)
        ids.append(node)
        fe_nodes.append(fe)
        frames.append(surface_frame(normal))
        for axis in 'XYZ':
            rows.append({'node': node, 'direction': f'{axis}+', 'role': 'response',
                         'control': 'True', 'channel_type': 'acceleration',
                         'unit': 'g', 'triax_dof': axis, 'comment': f'{label} (FE node {fe})'})
    columns = {name: [row.get(name, '') for row in rows]
               for name in ('node', 'direction', 'role', 'control',
                            'channel_type', 'unit', 'triax_dof', 'comment')}
    columns['channel'] = list(range(1, len(rows) + 1))

    xyz = model.node_xyz[model.node_index(fe_nodes)]
    cs_matrix = [np.vstack([np.eye(3), np.zeros(3)])]
    disp = []
    for position, frame in zip(xyz, frames, strict=True):
        if frame is None:
            disp.append(1)
        else:
            cs_matrix.append(np.vstack([frame, position]))
            disp.append(len(cs_matrix))
    count = len(cs_matrix)
    geometry = Geometry(
        node_id=np.asarray(ids, dtype=np.int64), node_xyz=xyz,
        node_def_cs=np.ones(len(ids), dtype=np.int64),
        node_disp_cs=np.asarray(disp, dtype=np.int64),
        cs_id=np.arange(1, count + 1), cs_name=[''] * count,
        cs_type=np.zeros(count, dtype=np.int64), cs_matrix=np.array(cs_matrix),
        length_unit=model.length_unit)
    geometry.view = model.view
    return geometry, ChannelTable(columns)


def project(size: float = barc.SIZE, solved: bool = False) -> Any:
    """A project to open in the app: the plate with the removable component, its
    test geometry and channel table linked as the Basis group.

        rc_plate.project(solved=True).save('rc-plate.hforge')

    With `solved`, the modes are solved to `SOLVE_TO` and projected onto every
    channel's DOF ('RC Plate Modes @ Test DOFs', in the Basis group). That is the
    example project the downloads page offers.

    Parameters
    ----------
    size : float
        The element size aimed at, in inches.
    solved : bool, default False
        Solve the modes and project them as well.

    Returns
    -------
    Project
    """
    from hertzforge.project import Project

    model = solid_geometry(size)
    out = Project('RC Plate')
    out.add('RC Plate', model)
    geometry, table = channels(model)
    out.add('RC Plate Test Geometry', geometry)
    out.add('RC Plate Channels', table)
    out.set_basis('RC Plate Test Geometry', 'RC Plate Channels')
    if solved:
        modes = out.solve_modes('RC Plate', maximum_frequency=SOLVE_TO, damping=DAMPING)
        out.project_onto_basis(modes, onto='RC Plate Channels',
                               name='RC Plate Modes @ Test DOFs')
    return out


def describe(size: float = barc.SIZE, modes: int = 10) -> None:
    """Print the model's size and its first `modes` elastic frequencies (those
    above 1 Hz)."""
    from hertzforge.core import fem

    model = fem.Model.from_geometry(solid_geometry(size), name='RC Plate')
    shapes = model.eigensolution(maximum_frequency=SOLVE_TO)
    print(f'RC Plate from bricks: {model.num_nodes} nodes, {len(model.solids)} bricks, '
          f'{model.total_mass:.3f} kg with the bolts')
    elastic = [f for f in shapes.frequency if f > 1.0][:modes]
    print('  elastic modes (Hz): ' + ', '.join(f'{f:.1f}' for f in elastic))


if __name__ == '__main__':
    describe()
