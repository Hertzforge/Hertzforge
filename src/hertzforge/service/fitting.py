"""Fitting modes by hand on the connection's project: Visual Dynamics'
fitting screen over the protocol (`fit.*`).

One mode at a time, on the residual. Find Mode moves the cursor to the
most prominent peak left in the residual CMIF inside the frequencies on
screen, Fit Mode fits there without adding the mode so its synthesis can
be judged, and Confirm Mode adds it; Refine fits every confirmed mode's
residues again together, and Publish puts the fit in the project as a
shape set. The session is the core's `ModalFitSession`, so a fit made
here and one made in a script are the same fit. A session started from a
shape set adopts it (Edit Fit), and publishing replaces that set.

Every answer carries the session's state and one `spectrum` view: each
measured CMIF singular value solid, the synthesis's following it dashed in
its color, and the pending mode's curve under the cursor.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from typing import Any

import numpy as np

from . import views as v
from .protocol import bad_request, not_found, refused
from .waterfalls import unit_text

FIT_METHODS = ('fit.start', 'fit.find', 'fit.move', 'fit.damping', 'fit.fit',
               'fit.confirm', 'fit.refine', 'fit.publish', 'fit.end')

#: the most measured singular values drawn: the rest are noise floor
CMIF_LINES = 4


@dataclass
class Fit:
    """A fitting session on one FRF of the project, and what it was
    started from."""

    session: Any
    frf: str
    shapes: str | None
    #: the measured CMIF (k, lines), computed once
    measured: np.ndarray = field(repr=False)


def _number(params: Mapping[str, Any], key: str, method: str) -> float:
    value = params.get(key)
    if isinstance(value, bool) or not isinstance(value, (int, float)) \
            or not np.isfinite(value):
        raise bad_request(f'{method} needs "{key}", a number')
    return float(value)


def start(project: Any, params: Mapping[str, Any]) -> Fit:
    """`fit.start`: a session on the FRF `frf`, from the shape set `shapes`
    when given (Edit Fit)."""
    from hertzforge.core.data import Frf
    from hertzforge.core.modal_fit import ModalFitSession
    from hertzforge.core.shapes import ShapeSet

    name = params.get('frf')
    if not isinstance(name, str) or not name:
        raise bad_request('fit.start needs "frf", an FRF\'s name')
    if name not in project:
        raise not_found(f'the project has no object named {name!r}')
    if not isinstance(project[name], Frf):
        raise refused(f'{name} is not an FRF; modes are fitted to an FRF')
    shapes = params.get('shapes')
    if shapes is not None and (not isinstance(shapes, str) or shapes not in project
                               or not isinstance(project[shapes], ShapeSet)):
        raise refused(f'{shapes!r} is not a shape set in the project')
    try:
        session = ModalFitSession(project[name])
        if shapes is not None:
            session.adopt(project[shapes])
    except ValueError as error:
        raise refused(str(error)) from None
    measured = np.linalg.svd(session.matrix, compute_uv=False).T
    return Fit(session, name, shapes, measured[:CMIF_LINES])


def act(fit: Fit, method: str, params: Mapping[str, Any]) -> None:
    """One step of the fit, in the core's words when it refuses."""
    session = fit.session
    try:
        if method == 'fit.find':
            low, high = params.get('low'), params.get('high')
            within = None
            if low is not None or high is not None:
                within = (_number(params, 'low', method), _number(params, 'high', method))
            session.suggest(within)
        elif method == 'fit.move':
            session.move_to(_number(params, 'frequency', method))
        elif method == 'fit.damping':
            session.override_damping(_number(params, 'damping', method))
        elif method == 'fit.fit':
            session.fit_pending()
        elif method == 'fit.confirm':
            description = params.get('description')
            if description is not None and not isinstance(description, str):
                raise bad_request('fit.confirm "description" is text')
            session.confirm(description=description)
        elif method == 'fit.refine':
            if not session.modes:
                raise refused('there are no confirmed modes to refine')
            session.refine_residues()
    except ValueError as error:
        raise refused(str(error)) from None


def publish(project: Any, fit: Fit, params: Mapping[str, Any]) -> str:
    """`fit.publish`: the confirmed modes added to the project as a shape
    set, or put in place of the set the fit was started from; its name."""
    if not fit.session.modes:
        raise refused('confirm a mode before publishing the fit')
    shapes = fit.session.shape_set()
    # what a simulated run gives is simulated, however far it is carried
    if getattr(project[fit.frf], 'source', None) is not None:
        shapes.source = project[fit.frf].source
    if fit.shapes is not None and fit.shapes in project:
        project[fit.shapes] = shapes
        return fit.shapes
    name = params.get('name') or f'{fit.frf} Modes'
    if not isinstance(name, str) or not name.strip():
        raise bad_request('fit.publish "name" is a name')
    added = project.add(name.strip(), shapes)
    # in the FRF's group, as the core's own fit is (`Project.fit_modes`), so
    # it animates on that group's geometry
    try:
        project.link(fit.frf, added)
    except ValueError:
        pass
    fit.shapes = added
    return added


def state(fit: Fit) -> dict[str, Any]:
    """What the shell shows: the pending mode, the preview, the confirmed
    modes and the CMIF view."""
    session = fit.session
    preview = session.preview
    pending = session.pending
    out: dict[str, Any] = {
        'frf': fit.frf, 'shapes': fit.shapes,
        'pending': {'frequency': float(pending['frequency']),
                    'damping': float(pending['damping']),
                    'held': bool(pending['overridden'])},
        'preview': None if not preview else {
            'frequency': float(preview['frequency']),
            'damping': float(preview['damping'])},
        'modes': [{'frequency': float(mode['frequency']),
                   'damping': float(mode['damping']),
                   'description': str(mode.get('description') or '')}
                  for mode in session.modes],
        'view': _view(fit),
        'mac': _mac(fit)}
    return out


def _mac(fit: Fit) -> dict[str, Any] | None:
    """The auto-MAC of the confirmed modes, with the preview's row and
    column last, as VD shows it beside the fit: how alike the modes'
    shapes are, which a close pair or a mode split in two shows off the
    diagonal. None for fewer than two."""
    from hertzforge.core.shapes import mac_matrix

    session = fit.session
    modes = list(session.modes)
    ticks = [f'{k + 1}: {mode["frequency"]:.1f} Hz' for k, mode in enumerate(modes)]
    if session.preview is not None:
        modes.append(session.preview)
        ticks.append(f'New: {session.preview["frequency"]:.1f} Hz')
    if len(modes) < 2:
        return None
    values = mac_matrix(np.array([mode['shape'] for mode in modes]))
    axis = {'label': 'Mode', 'ticks': ticks}
    return v.matrix(f'{fit.frf}: MAC of the fit', axis, axis, values, 'MAC', (0.0, 1.0))


def _view(fit: Fit) -> dict[str, Any]:
    session = fit.session
    frf = session.frf
    f = session.frequencies
    unit = unit_text(frf.known_dim(session.rows[0])) if frf.num_records else None
    lines = [v.series(f'CMIF {k + 1}', 'measured', *_finite(f, fit.measured[k]))
             for k in range(len(fit.measured))]
    synthesized = session.synthesis_singular_values()
    if synthesized is not None:
        for k in range(min(len(synthesized), len(fit.measured))):
            lines.append(v.series(f'CMIF {k + 1}, synthesized', 'measured',
                                  *_finite(f, synthesized[k]), follows=k))
    curve = session.pending_curve()
    if curve is not None and len(curve[0]):
        lines.append(v.series('Pending mode', 'reference', *_finite(*curve)))
    return v.spectrum(f'{fit.frf}: CMIF', v.axis('Frequency', 'Hz', 'linear'),
                      v.axis('CMIF', unit, 'log'), lines, link='frequency')


def _finite(x: Any, y: Any) -> tuple[np.ndarray, np.ndarray]:
    x, y = np.asarray(x, dtype=float), np.asarray(y, dtype=float)
    kept = np.isfinite(x) & np.isfinite(y) & (y > 0)
    return x[kept], y[kept]
