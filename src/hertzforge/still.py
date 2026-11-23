"""A still picture of a geometry with DOFs marked as arrows (`hertzforge still`).

The picture is drawn by the app's own 3-D view, as the recorder draws a video's frames
(`record`): the geometry's surface and mesh lines as the app shows it, in the light or dark
theme, with a solid arrow at each DOF the named objects measure, in its axis's color. A force
or a drive voltage points in and ends on its node; every other DOF points out from it. Free
nodes, those no element is drawn through, are left off: on a finite element model they are
masses and spider centers that read as specks.

A test's DOFs name the nodes of the test geometry they were measured on, which are other nodes
of a finite element mesh, so the DOFs are placed by position when they are drawn on a geometry
other than their own (`drawing.viz.geometry.dof_arrows_on`): each at the nearest node of the
geometry drawn, as DOF Mapping matches a test to a model. A DOF with no node near enough is
refused by name, never guessed.
"""

from __future__ import annotations

import base64
import os
import tempfile
from collections.abc import Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import numpy as np

from .animation import AnimationError, framed, opening_camera, staged
from .record import RecordError


class StillError(RecordError):
    """A picture that cannot be made, in words for the person making it."""


@dataclass
class Still:
    """What a still picture is made of, and how it is drawn.

    `geometry` is drawn, with an arrow at every DOF each object in `dofs` measures: every
    record's DOF of a data object (a frequency response's references as well as its
    responses), or every channel's of a channel table. `on` names the geometry their node ids
    belong to; when it is None each object's is the geometry linked with it, and an object
    linked with none is refused, since its ids then name no place. On a geometry other than
    `geometry` the DOFs are placed by position, no further than `distance` from a node of
    `geometry` (meters when the units are defined; the match distance DOF Mapping opens on
    when None). `opacity`, `view`, `eye`, `up`, `frame` and `theme` are the recorder's
    (`record.Recording`): how solid the surface is, the camera, the element groups it is
    framed on, and the light or dark look.
    """

    geometry: str
    dofs: Sequence[str] = field(default_factory=tuple)
    on: str | None = None
    distance: float | None = None
    width: int = 1920
    height: int = 1080
    opacity: float = 1.0
    view: str = 'iso'
    eye: Sequence[float] | None = None
    up: Sequence[float] | None = None
    frame: Sequence[str] | None = None
    theme: str = 'dark'


def measured_on(project: Any, name: str) -> str | None:
    """The geometry linked with `name` in its object group, or None. The active geometry is not
    a fallback here: a data object no geometry is linked with may name any nodes."""
    from .core.geometry import Geometry

    for member in project.object_group_of(name) or ():
        if isinstance(project.get(member), Geometry):
            return member
    return None


def marked_dofs(obj: Any, name: str) -> tuple[list[str], list[bool]]:
    """The DOFs `obj` measures and whether each points in (a force or a drive voltage): a data
    object's at every quantity it measures (`core.report.series_quantity_dofs`), a channel
    table's at every channel with a node and a direction."""
    from .core.channel_table import ChannelTable
    from .core.data import DataArray
    from .core.report import EXCITATION_QUANTITIES, series_dof_quantities, series_quantity_dofs

    if isinstance(obj, ChannelTable):
        strings, types = obj.dof_strings(), obj.types()
        rows = [i for i in range(obj.num_channels)
                if str(obj['node'][i]).strip() and str(obj['direction'][i]).strip()]
        return [strings[i] for i in rows], [types[i] in EXCITATION_QUANTITIES for i in rows]
    if not isinstance(obj, DataArray):
        raise StillError(f'{name} is not data or a channel table, so it measures no DOFs')
    series = [(name, obj, None)]
    quantities = series_dof_quantities(series)
    if not quantities:
        raise StillError(f'{name} measures no quantity at a DOF; declare its units first')
    dofs: list[str] = []
    into: list[bool] = []
    for quantity in quantities:
        found = series_quantity_dofs(series, quantity)
        dofs += [str(dof) for dof in found]
        into += [quantity in EXCITATION_QUANTITIES] * len(found)
    return dofs, into


def still_view(project: Any, still: Still) -> dict[str, Any]:
    """The `shape3d` view the picture is drawn from: the app's view of the geometry alone, its
    free nodes left off, with the DOF arrows, opening at the still's opacity and camera and
    framed on its element groups; StillError saying why it cannot be drawn."""
    from .core.geometry import Geometry
    from .drawing.viz.geometry import dof_arrow_length, dof_arrow_solids, dof_arrows_on
    from .service import show
    from .service.ops_geometry import _extent
    from .service.protocol import ProtocolError

    names = [still.geometry, *still.dofs, *([still.on] if still.on else [])]
    missing = [name for name in names if name not in project]
    if missing:
        raise StillError(f'the project has no object named {missing[0]!r}')
    for name in (still.geometry, still.on):
        if name and not isinstance(project[name], Geometry):
            raise StillError(f'{name} is not a geometry')
    geometry = project[still.geometry]
    try:
        shown = show.show(project, [still.geometry], {}, None,
                          {**show.PREFS, 'arrows': False, 'sensors': False})
    except ProtocolError as error:
        raise StillError(error.message) from None
    view = next((v for v in shown['views'] if v.get('view') == 'shape3d'), None)
    if view is None:
        raise StillError(f'{still.geometry} draws nothing in 3-D')
    view.pop('points', None)
    view.pop('arrows', None)
    view.pop('bodies', None)
    hits: list[tuple[bool, tuple[int, np.ndarray, int]]] = []
    labels: list[str] = []
    for name in still.dofs:
        dofs, into = marked_dofs(project[name], name)
        if not dofs:
            raise StillError(f'{name} has no channel with a node and a direction')
        on = still.on or measured_on(project, name)
        if on is None:
            raise StillError(
                f'{name} is linked with no geometry, so its node ids name no place on '
                f'{still.geometry}; link it with the geometry it was measured on, or name that '
                'geometry (on)')
        try:
            placed = dof_arrows_on(geometry, dofs, project[on], still.distance)
        except ValueError as error:
            raise StillError(f'{name} on {still.geometry}: {error}') from None
        hits += list(zip(into, placed, strict=True))
        labels += dofs
    if hits:
        xyz = np.asarray(geometry.node_xyz, dtype=float).reshape(-1, 3)
        length = dof_arrow_length(xyz[[hit[0] for _into, hit in hits]], _extent(xyz))
        view['arrows'] = dof_arrow_solids(xyz, hits, length, labels)
    try:
        return staged(view, still.opacity, opening_camera(still.view, still.eye, still.up),
                      framed(project, still.geometry, view, still.frame))
    except AnimationError as error:
        raise StillError(str(error)) from None


def still_page(project: Any, still: Still) -> str:
    """The report viewer's page with the picture's view on it, ready for a browser to draw."""
    from .record import stage_page

    if still.width < 16 or still.height < 16:
        raise StillError('the picture is at least 16 by 16 pixels')
    return stage_page(still_view(project, still),
                      {'width': int(still.width), 'height': int(still.height)}, still.theme)


def write_still(project: Any, still: Still, output: str | os.PathLike) -> Path:
    """The picture drawn in headless Chrome and written to `output`, a PNG; its path."""
    from .record import _Chrome, find_chrome, opened

    page = still_page(project, still)
    chrome = find_chrome()
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hertzforge-still-') as folder:
        path = Path(folder) / 'still.html'
        path.write_text(page, encoding='utf-8')
        browser = _Chrome(chrome, still.width, still.height)
        try:
            opened(browser, path)
            url = browser.evaluate('window.__hertzforgeRecorder.frame(0)')
        finally:
            browser.close()
    output.write_bytes(base64.b64decode(url.split(',', 1)[1]))
    return output
