"""The sine specification on a 3-D stage: frequency across, time receding,
amplitude up, as Visual Dynamics' `viz/sinespec.py` draws it.

A sine sweep specification has three dimensions: each tone is a path
through time and frequency with an amplitude along it, and its warning
and abort limits follow the same path above and below. Drawn as
amplitude against frequency the timing is lost (which tones overlap,
which cross, which are done before the others begin), so the
specification's own view is this stage. The numbers are VD's
`sine_stage_arrays` and `add_sine_stage`; the drawing is the shell's
waterfall pane with a depth of its own per point (`y_values`).
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .waterfalls import unit_text

#: about how many points each tone's path is sampled at (VD's LINES)
LINES = 400


def _edges(values: np.ndarray) -> np.ndarray:
    """Bin edges along a path: midpoints between neighboring lines, the
    ends extended half a bin."""
    if len(values) == 1:
        return np.array([values[0], values[0]])
    mids = (values[:-1] + values[1:]) / 2.0
    first = values[0] - (mids[0] - values[0])
    last = values[-1] + (values[-1] - mids[-1])
    return np.concatenate([[first], mids, [last]])


def sine_stage(spec: Any, levels: list[Any], tone: str, dofs: list[str] | None = None,
               title: str = '', lines: int = LINES) -> dict[str, Any] | None:
    """The stage of one `tone` at each control DOF in `dofs` (every DOF
    when None): the tone's target with its warning and abort limits as
    curves and as the translucent zones the flat comparison shades, and
    each extracted level of the tone as its own path, placed in time by
    its own clock aligned to the tone's start, with a rectangle where a
    line went past an abort limit. Either `spec` or `levels` may be
    missing. None when nothing is drawn."""
    labels: list[str] = []
    curves: list[tuple[np.ndarray, np.ndarray]] = []
    times: list[np.ndarray] = []
    roles: list[str] = []
    zones: list[dict] = []
    exceed: list[dict] = []
    unit: str | None = None
    spec_dofs = [str(d) for d in spec.response_dof] if spec is not None else []
    tone_obj = spec.tone(tone) if spec is not None and tone in {t.name for t in spec.tones} else None
    wanted = list(dofs) if dofs else None
    if wanted is None:
        wanted = list(spec_dofs)
        for level in levels:
            for dof in level.response_dof:
                if str(dof) not in wanted:
                    wanted.append(str(dof))
    several = len(wanted) > 1

    def name(what: str, dof: str) -> str:
        return f'{what} {dof}' if several else what

    if tone_obj is not None:
        unit = unit_text(spec.ordinate_dim) if spec.ordinate_unit is not None else None
        t, f = tone_obj.trajectory(tone_obj.duration() / lines)
        t = np.asarray(t, dtype=float) + float(tone_obj.start_time)
        f = np.asarray(f, dtype=float)
        for dof in wanted:
            if dof not in spec_dofs:
                continue
            k = spec_dofs.index(dof)
            target = np.asarray(tone_obj.target(f, seconds=t - float(tone_obj.start_time)),
                                dtype=float)[:, k]
            limits = {}
            for limit in tone_obj.limits:
                values = np.asarray(tone_obj.target(f, limit, t - float(tone_obj.start_time)),
                                    dtype=float)[:, k]
                if np.isfinite(values).any():
                    limits[limit] = values
            station = len(labels)
            labels.append(name(f'{tone} requirement,', dof) if several else f'{tone} requirement')
            curves.append((f, target))
            times.append(t)
            roles.append('reference')
            for key in ('warning_lower', 'warning_upper', 'abort_lower', 'abort_upper'):
                if key in limits:
                    labels.append(name(f'{tone} {key.replace("_", " ")},', dof) if several
                                  else f'{tone} {key.replace("_", " ")}')
                    curves.append((f, limits[key]))
                    times.append(t)
                    roles.append('tolerance' if key.startswith('warning') else 'abort')
            for side in ('upper', 'lower'):
                warning, abort = limits.get(f'warning_{side}'), limits.get(f'abort_{side}')
                if warning is not None:
                    zone = {'station': station, 'kind': 'warning', 'side': side, 'x': f,
                            'y': t, 'inner': warning}
                    if abort is not None:
                        zone['outer'] = abort
                    zones.append(zone)
                if abort is not None:
                    zones.append({'station': station, 'kind': 'abort', 'side': side, 'x': f,
                                  'y': t, 'inner': abort})
    for level in levels:
        if level.tone != tone or level.seconds is None:
            continue
        level_dofs = [str(d) for d in level.response_dof]
        for dof in wanted:
            if dof not in level_dofs:
                continue
            i = level_dofs.index(dof)
            # a level's lines are in frequency order; a sweep up and back
            # would draw as a zigzag between its passes
            path = np.argsort(level.seconds)
            amplitude = np.abs(np.asarray(level.ordinate[i]))[path]
            seconds = np.asarray(level.seconds, dtype=float)[path]
            if tone_obj is not None:
                seconds = seconds - float(level.onset) + float(tone_obj.start_time)
            frequencies = np.asarray(level.abscissa, dtype=float)[path]
            if unit is None and level.ordinate_unit is not None and level.ordinate_unit[i]:
                unit = unit_text(level.ordinate_dim[i])
            station = len(labels)
            labels.append(name(f'{tone} level,', dof) if several else f'{tone} level')
            curves.append((frequencies, amplitude))
            times.append(seconds)
            roles.append('measured')
            if tone_obj is None or dof not in spec_dofs:
                continue
            k = spec_dofs.index(dof)
            good = np.isfinite(amplitude)
            ex, ey = _edges(frequencies), _edges(seconds)
            for limit, side, over in (('abort_upper', 'upper', True), ('abort_lower', 'lower', False)):
                if limit not in tone_obj.limits:
                    continue
                edge = np.asarray(level.target_on(tone_obj, limit), dtype=float)[path, k]
                out = good & np.isfinite(edge) & (amplitude > edge if over else amplitude < edge)
                hits = np.flatnonzero(out)
                if hits.size:
                    exceed.append({'station': station, 'side': side, 'x0': ex[hits],
                                   'x1': ex[hits + 1], 'y0': ey[hits], 'y1': ey[hits + 1],
                                   'z': edge[hits]})
    if not curves:
        return None
    xs = np.concatenate([c[0] for c in curves])
    ts = np.concatenate(times)
    zs = np.concatenate([c[1] for c in curves])
    finite = zs[np.isfinite(zs)]
    z_top = float(finite.max()) if finite.size else 1.0
    z_low = min(0.0, float(finite.min())) if finite.size else 0.0
    view = v.waterfall(title or f'{tone} on the stage', v.axis('Frequency', 'Hz', 'linear'),
                       v.axis('Amplitude', unit, 'linear'), labels, curves,
                       (float(xs.min()), float(xs.max())), (z_low, z_top or 1.0))
    return v.pathed(view, v.axis('Time', 's', 'linear'), ts,
                    (float(ts.min()), float(ts.max())), roles, zones, exceed)
