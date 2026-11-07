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
from .projects import kind_of
from .protocol import bad_request, not_found, refused
from .waterfalls import unit_text

FIT_METHODS = ('fit.start', 'fit.find', 'fit.move', 'fit.damping', 'fit.fit',
               'fit.confirm', 'fit.refine', 'fit.delete', 'fit.edit', 'fit.residual',
               'fit.publish', 'fit.end')

#: the most measured singular values drawn: the rest are noise floor
CMIF_LINES = 4

#: the kinds a fit can be weighted by
COHERENCES = ('coherence', 'multiple_coherence')


@dataclass
class Fit:
    """A fitting session on one FRF of the project, and what it was
    started from."""

    session: Any
    frf: str
    shapes: str | None
    #: the measured CMIF (k, lines), computed once
    measured: np.ndarray = field(repr=False)
    #: the coherence the least squares is weighted by, or None
    coherence: str | None = None
    #: whether the view draws the residual's own CMIF
    residual: bool = False


def _number(params: Mapping[str, Any], key: str, method: str) -> float:
    value = params.get(key)
    if isinstance(value, bool) or not isinstance(value, (int, float)) \
            or not np.isfinite(value):
        raise bad_request(f'{method} needs "{key}", a number')
    return float(value)


def start(project: Any, params: Mapping[str, Any]) -> Fit:
    """`fit.start`: a session on the FRF `frf`, on the `records` picked when
    given, from the shape set `shapes` when given (Edit Fit)."""
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
    records = params.get('records') or None
    if records is not None and (
            not isinstance(records, list)
            or not all(isinstance(r, int) and 0 <= r < project[name].num_records for r in records)):
        raise refused(f'{name} has records 0 to {project[name].num_records - 1}; '
                      f'not {records!r}')
    coherence = params.get('coherence')
    if coherence is not None and (not isinstance(coherence, str) or coherence not in project
                                  or kind_of(project[coherence]) not in COHERENCES):
        raise refused(f'{coherence!r} is not a coherence in the project')
    try:
        session = ModalFitSession(project[name], records=records, coherence=None if coherence is None
                                  else project[coherence])
        if shapes is not None:
            session.adopt(project[shapes])
    except ValueError as error:
        raise refused(str(error)) from None
    measured = np.linalg.svd(session.matrix, compute_uv=False).T
    return Fit(session, name, shapes, measured[:CMIF_LINES], coherence)


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
        elif method == 'fit.delete':
            delete(session, _indices(params, method, len(session.modes)))
        elif method == 'fit.edit':
            edit(session, params)
        elif method == 'fit.residual':
            show = params.get('show')
            if not isinstance(show, bool):
                raise bad_request('fit.residual needs "show", true or false')
            fit.residual = show
    except ValueError as error:
        raise refused(str(error)) from None


def _indices(params: Mapping[str, Any], method: str, count: int) -> list[int]:
    modes = params.get('modes')
    if (not isinstance(modes, list) or not modes
            or not all(isinstance(k, int) and not isinstance(k, bool) for k in modes)):
        raise bad_request(f'{method} needs "modes", a list of mode indices from 0')
    outside = [k for k in modes if not 0 <= k < count]
    if outside:
        raise refused(f'the fit has {count} confirmed mode{"s" * (count != 1)}; '
                      f'there is no mode {outside[0] + 1}')
    return modes


def delete(session: Any, indices: list[int]) -> None:
    """`fit.delete`: confirmed modes taken back, as Visual Dynamics' fit
    table deletes rows: each leaves the modes and the replay list, and the
    preview, fitted against a residual that is gone, is dropped."""
    for k in sorted(set(indices), reverse=True):
        pick = (float(session.modes[k]['frequency']), float(session.modes[k]['damping']))
        if pick in session.confirmed:
            session.confirmed.remove(pick)
        del session.modes[k]
    session.preview = None


def edit(session: Any, params: Mapping[str, Any]) -> None:
    """`fit.edit`: a confirmed mode's damping (a fraction of critical,
    above 0 and up to 1) or description typed into its row, as Visual
    Dynamics' fit table takes them."""
    k = params.get('mode')
    if isinstance(k, bool) or not isinstance(k, int):
        raise bad_request('fit.edit needs "mode", a mode index from 0')
    _indices({'modes': [k]}, 'fit.edit', len(session.modes))
    damping, description = params.get('damping'), params.get('description')
    if damping is None and description is None:
        raise bad_request('fit.edit needs "damping" or "description"')
    if damping is not None:
        value = _number(params, 'damping', 'fit.edit')
        if not 0.0 < value <= 1.0:
            raise refused('damping is a fraction of critical, above 0 and up to 1 (100 %)')
        session.modes[k]['damping'] = value
    if description is not None:
        if not isinstance(description, str):
            raise bad_request('fit.edit "description" is text')
        session.modes[k]['description'] = description.strip()


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
        'mac': _mac(fit),
        'coherence': fit.coherence,
        'residual': fit.residual,
        'half_power': _half_power(fit)}
    return out


def _half_power(fit: Fit) -> dict[str, Any]:
    """The half-power reading of the residual CMIF peak under the cursor
    (`ModalFitSession.half_power`), as Visual Dynamics shows it beside the
    fit: the peak's frequency, the two crossings, Q and the damping they
    give, or why it could not be read."""
    reading = fit.session.half_power(warn=False)

    def number(value: Any) -> float | None:
        return None if value is None or not np.isfinite(value) else float(value)

    return {'frequency': float(reading.frequency), 'low': number(reading.low),
            'high': number(reading.high), 'q': number(reading.q),
            'damping': number(reading.damping), 'notes': list(reading.notes)}


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
    if fit.residual and session.modes:
        residual = np.linalg.svd(session.residual_matrix(), compute_uv=False).T
        floor = residual.max() * 1e-10
        for k in range(min(len(residual), CMIF_LINES)):
            if residual[k].max() > floor:
                lines.append(v.series(f'CMIF {k + 1}, residual', 'drive',
                                      *_finite(f, residual[k])))
    curve = session.pending_curve()
    if curve is not None and len(curve[0]):
        lines.append(v.series('Pending mode', 'reference', *_finite(*curve)))
    return v.spectrum(f'{fit.frf}: CMIF', v.axis('Frequency', 'Hz', 'linear'),
                      v.axis('CMIF', unit, 'log'), lines, link='frequency')


def _finite(x: Any, y: Any) -> tuple[np.ndarray, np.ndarray]:
    x, y = np.asarray(x, dtype=float), np.asarray(y, dtype=float)
    kept = np.isfinite(x) & np.isfinite(y) & (y > 0)
    return x[kept], y[kept]
