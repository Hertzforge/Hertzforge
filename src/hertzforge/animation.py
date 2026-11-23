"""A geometry's animation exported as one page that plays offline (`hertzforge export-animation`).

The animation is the one the app's 3-D view plays for the same selection (`service.show`): a
shape set's modes on their geometry, or data played on it, measured motion or modal
coordinates through their shape set (summed in the page, as the app sums them). The page is the
report viewer (`report/viewer.html`) with the animation in place of a report, so it is drawn by
the app's own 3-D view with its own controls, needs no service, and can be shown in an iframe.
"""

from __future__ import annotations

import base64
import html
from collections.abc import Sequence
from pathlib import Path
from typing import Any

import numpy as np

#: the views an animation can open looking from, as (eye, up): the direction from the model's
#: centre to the camera, and which way is up, in the global frame. 'iso' is the geometry's own
#: view (`Geometry.view`), or the app's opening view when it has none.
VIEWS: dict[str, tuple[tuple[float, float, float], tuple[float, float, float]] | None] = {
    'iso': None,
    'front': ((0.0, -1.0, 0.0), (0.0, 0.0, 1.0)),
    'back': ((0.0, 1.0, 0.0), (0.0, 0.0, 1.0)),
    'left': ((-1.0, 0.0, 0.0), (0.0, 0.0, 1.0)),
    'right': ((1.0, 0.0, 0.0), (0.0, 0.0, 1.0)),
    'top': ((0.0, 0.0, 1.0), (0.0, 1.0, 0.0)),
    'bottom': ((0.0, 0.0, -1.0), (0.0, 1.0, 0.0)),
}


#: an amplitude of 1, as a fraction of the drawn model's diagonal: the deflection the 3-D view
#: draws at its slider's default (`amplitudeUnit` in `ui/src/lib/viz/points.ts`)
AMP_UNIT = 0.075


class AnimationError(RuntimeError):
    """An animation that cannot be exported, in words for the person exporting it."""


def opening_camera(view: str = 'iso', eye: Sequence[float] | None = None,
                   up: Sequence[float] | None = None) -> dict[str, list[float]] | None:
    """The camera an animation opens with, as a `shape3d` view's `home` ({'eye', 'up'}): `eye`
    and `up` (Z when not given) when `eye` is given, else the named view's from `VIEWS`. None
    for 'iso', which keeps the geometry's own view."""
    from .core.geometry import View

    if eye is None:
        if up is not None:
            raise AnimationError('up is given with an eye')
        if view not in VIEWS:
            raise AnimationError(f'the view is one of {", ".join(VIEWS)}, not {view!r}')
        if VIEWS[view] is None:
            return None
        eye, up = VIEWS[view]
    up = (0.0, 0.0, 1.0) if up is None else up
    if len(eye) != 3 or len(up) != 3:
        raise AnimationError('the eye and up are three numbers each, as 1,0,0')
    try:
        View(eye, up)
    except ValueError as error:
        raise AnimationError(str(error)) from None
    return {'eye': [float(v) for v in eye], 'up': [float(v) for v in up]}


def amplitude_unit(view: dict[str, Any]) -> float:
    """The deflection an amplitude of 1 draws in a `shape3d` view, in its units: `AMP_UNIT` of
    the diagonal across its nodes, point layers and segment layers, the whole model's whatever
    the view is framed on; `AMP_UNIT` when nothing has an extent. The protocol states the rule
    (`schema/PROTOCOL.md`, shape3d), and the shell draws by it."""
    rows = [np.asarray(view['nodes'], dtype=float).reshape(-1, 3)]
    rows += [np.asarray(layer['xyz'], dtype=float).reshape(-1, 3)
             for key in ('points', 'segments') for layer in view.get(key) or ()]
    xyz = np.concatenate(rows)
    across = float(np.linalg.norm(xyz.max(axis=0) - xyz.min(axis=0))) if len(xyz) else 0.0
    return AMP_UNIT * (across if np.isfinite(across) and across > 0 else 1.0)


def frame_box(geometry: Any, groups: Sequence[str], points: Any, ids: Any) -> dict[str, list[float]]:
    """The box a view is framed on: around the drawn `points` (rows of x, y, z, as drawn) whose
    node ids (`ids`, one per leading row of `points`) the elements of `groups` use, as
    {'lo', 'hi'}. ValueError for an empty list, a name that is not one of `geometry`'s element
    groups, or groups none of whose nodes are drawn."""
    if isinstance(groups, str):
        groups = [groups]
    if not len(groups):
        raise ValueError('the frame names no element group')
    named = dict(zip(geometry.group_name, (int(g) for g in geometry.group_id), strict=True))
    missing = [str(name) for name in groups if name not in named]
    if missing:
        have = ', '.join(repr(n) for n in geometry.group_name if n) or 'none with a name'
        raise ValueError(f'the geometry has no element group named {missing[0]!r}; it has {have}')
    wanted = geometry.nodes_in([named[name] for name in groups])
    ids = np.asarray(ids, dtype=np.int64)
    xyz = np.asarray(points, dtype=float).reshape(-1, 3)[:len(ids)]
    inside = xyz[np.isin(ids, wanted)]
    if not len(inside):
        raise ValueError(f'none of the nodes of {", ".join(map(repr, groups))} is drawn')
    return {'lo': [float(v) for v in inside.min(axis=0)],
            'hi': [float(v) for v in inside.max(axis=0)]}


def staged(view: dict[str, Any], opacity: float = 1.0, camera: dict[str, list[float]] | None = None,
           frame: dict[str, list[float]] | None = None) -> dict[str, Any]:
    """`view` opening at `opacity` (1 solid, down to above 0) and from `camera` (`opening_camera`;
    its own when None), fitted to `frame` (`frame_box`) when given rather than to the whole
    model."""
    if not 0 < opacity <= 1:
        raise AnimationError(f'the opacity is above 0 and at most 1, not {opacity}')
    if opacity < 1:
        view['opacity'] = float(opacity)
    if camera is not None:
        view['home'] = camera
    if frame is not None:
        view['home'] = {**view.get('home', {}), 'frame': frame}
    return view


def framed(project: Any, geometry: str, view: dict[str, Any],
           groups: Sequence[str] | None) -> dict[str, list[float]] | None:
    """The box `view` (`animation_view` of `geometry`) is framed on for the element `groups` of
    `geometry` (`frame_box`), None when no groups are given; AnimationError saying why not."""
    if groups is None:
        return None
    if view.get('node_ids') is None:
        raise AnimationError(f'{geometry} draws no nodes to frame on')
    try:
        return frame_box(project[geometry], groups, view['nodes'], view['node_ids'])
    except ValueError as error:
        raise AnimationError(str(error)) from None


def animation_view(project: Any, geometry: str, data: str | None = None,
                   shapes: str | None = None, records: Sequence[int] = ()) -> dict[str, Any]:
    """The `shape3d` view the app shows for `geometry` with `data` and/or `shapes` (with
    `records` of `data` picked when given), or AnimationError saying why there is none."""
    from .service import show as showing
    from .service.protocol import ProtocolError

    objects = [name for name in (geometry, data, shapes) if name]
    missing = [name for name in objects if name not in project]
    if missing:
        raise AnimationError(f'the project has no object named {missing[0]!r}')
    picks = {data: list(records)} if data and records else {}
    try:
        shown = showing.show(project, objects, picks, None, dict(showing.PREFS))
    except ProtocolError as error:  # the service refuses in its own words
        raise AnimationError(error.message) from None
    view = next((v for v in shown['views'] if v.get('view') == 'shape3d'
                 and (v.get('motion') is not None or v.get('modes'))), None)
    if view is None:
        why = f': {shown["note"]}' if shown.get('note') else ''
        raise AnimationError(f'{" with ".join(objects)} show no animation{why}')
    return view


def animation_page(project: Any, geometry: str, data: str | None = None,
                   shapes: str | None = None, records: Sequence[int] = (), mode: int = 1,
                   scale: float = 1.0, opacity: float = 1.0, view: str = 'iso',
                   eye: Sequence[float] | None = None, up: Sequence[float] | None = None,
                   frame: Sequence[str] | None = None, theme: str | None = None,
                   title: str | None = None) -> str:
    """The animation as one self-contained HTML page: it opens playing, at mode `mode` (from 1)
    when there is no motion, at `scale` times the app's amplitude, the surface at `opacity`,
    looking from the named `view` or from `eye` with `up` (`opening_camera`), framed on the
    element groups `frame` of the geometry when given (the whole model still drawn, and the
    amplitude still a share of the whole model's size), in the `light` or `dark` theme (the
    reader's own when None), titled `title` (the view's own when None)."""
    from .report.viewer import DATA, THEMES, TITLE, viewer_page
    from .service import frames

    if theme is not None and theme not in THEMES:
        raise AnimationError(f'the theme is light or dark, not {theme!r}')
    if scale < 0:
        raise AnimationError(f'the scale is 0 or more, not {scale}')
    camera = opening_camera(view, eye, up)
    shown = animation_view(project, geometry, data, shapes, records)
    shown = staged(shown, opacity, camera, framed(project, geometry, shown, frame))
    setup: dict[str, Any] = {'scale': float(scale)}
    if shown.get('motion') is None:
        if not 1 <= mode <= len(shown['modes']):
            raise AnimationError(f'{shapes or data} has modes 1 to {len(shown["modes"])}; '
                                 f'not {mode}')
        setup['mode'] = mode - 1
    frame = frames.pack({'type': 'response', 'id': 0,
                         'result': {'animation': {'view': shown, 'setup': setup}}})
    encoded = frame.encode('utf-8') if isinstance(frame, str) else frame
    page = viewer_page()
    if TITLE not in page or DATA not in page:
        raise AnimationError('the report viewer holds no place for data; build it again')
    page = page.replace(TITLE, html.escape(title or str(shown.get('title') or geometry)), 1)
    page = page.replace(DATA, base64.b64encode(encoded).decode('ascii'), 1)
    if theme is not None:
        page = page.replace('<html lang="en">', f'<html lang="en" data-theme="{theme}">', 1)
    return page


def export_animation(project: Any, output: str | Path, geometry: str, data: str | None = None,
                     shapes: str | None = None, **options: Any) -> Path:
    """The animation written to `output` as one page (`animation_page`, which takes
    `options`); the path written."""
    page = animation_page(project, geometry, data, shapes, **options)
    out = Path(output)
    out.write_text(page, encoding='utf-8')
    return out
