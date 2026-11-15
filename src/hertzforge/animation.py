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


def staged(view: dict[str, Any], opacity: float = 1.0, camera: dict[str, list[float]] | None = None
           ) -> dict[str, Any]:
    """`view` opening at `opacity` (1 solid, down to above 0) and from `camera` (`opening_camera`;
    its own when None)."""
    if not 0 < opacity <= 1:
        raise AnimationError(f'the opacity is above 0 and at most 1, not {opacity}')
    if opacity < 1:
        view['opacity'] = float(opacity)
    if camera is not None:
        view['home'] = camera
    return view


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
                   theme: str | None = None, title: str | None = None) -> str:
    """The animation as one self-contained HTML page: it opens playing, at mode `mode` (from 1)
    when there is no motion, at `scale` times the app's amplitude, the surface at `opacity`,
    looking from the named `view` or from `eye` with `up` (`opening_camera`), in the `light` or
    `dark` theme (the reader's own when None), titled `title` (the view's own when None)."""
    from .report.viewer import DATA, THEMES, TITLE, viewer_page
    from .service import frames

    if theme is not None and theme not in THEMES:
        raise AnimationError(f'the theme is light or dark, not {theme!r}')
    if scale < 0:
        raise AnimationError(f'the scale is 0 or more, not {scale}')
    camera = opening_camera(view, eye, up)
    shown = staged(animation_view(project, geometry, data, shapes, records), opacity, camera)
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
