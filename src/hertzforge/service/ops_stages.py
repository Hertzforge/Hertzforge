"""Visual Dynamics' 3-D stages of two objects together: frequency across,
channels receding, level up, as the waterfall stands.

The paired stage draws two densities at each channel they share, the louder
colored by level and the quieter in the reference gray, or divided as signal
over noise in dB (`core.snr.signal_to_noise`). The banded stage draws a
specification's every channel with its warning and abort bands and, beside a
measured PSD, each channel's measurement with the lines it went past an abort
limit shaded, judged at full resolution by `core.compliance.outside`, the call
the 2-D comparison and the lines-out table use. The measured PSD is scaled by
the comparison's scale, as the Spectra reading scales it.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .descriptors import Operation, Option, Param
from .ops_compare import FIRST, SECOND, _loud_quiet, _refusing, _scale
from .ops_project import PAGE
from .protocol import refused
from .waterfalls import axes, unit_text

PAIRED_MODE = Param('mode', 'Show', 'enum', default='overlay',
                    options=(Option('overlay', 'Overlaid'),
                             Option('ratio', 'Signal to noise')))
SPECIFICATION = Param('specification', 'Specification', 'enum',
                      options_from='objects:specification')
MEASURED = Param('spectrum', 'Measured PSD', 'enum', options_from='objects:psd', default='')

#: points a decade a breakpoint specification is filled in with on the stage
LAW_POINTS = 60


def _pages(arrays: dict[str, Any]) -> dict[str, int] | None:
    return ({'number': arrays['page'] + 1, 'count': arrays['pages'],
             'records': arrays['of_quantity']} if arrays['pages'] > 1 else None)


def paired_stage(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.paired_stage`: two densities on one stage, one station a
    channel they share, matched by record label. Overlaid, the louder is
    colored by level and the quieter drawn in gray behind it; as signal to
    noise, one curve a channel in dB on a linear axis."""
    import hertzforge as hz
    from hertzforge.drawing.viz.waterfall import POINT_BUDGET, drawn_span, waterfall_arrays

    (loud_name, loud), (quiet_name, quiet) = _loud_quiet(project, params)
    if params['mode'] == 'ratio':
        from hertzforge.core.snr import signal_to_noise

        x, rows, dofs, _dims = _refusing(signal_to_noise, loud, quiet)
        if not dofs:
            raise refused(f'{loud_name} and {quiet_name} share no channel')
        with np.errstate(divide='ignore', invalid='ignore'):
            decibels = 10.0 * np.log10(np.real(rows))
        x = np.asarray(x, dtype=float)
        curves, labels = [], []
        for row, dof in zip(decibels, dofs, strict=True):
            if np.isfinite(row).sum() >= 2:
                curves.append((x, row))
                labels.append(str(dof))
        if not curves:
            raise refused(f'{loud_name} is nowhere above {quiet_name}')
        view = v.waterfall(f'{loud_name} over {quiet_name}: signal to noise',
                           v.axis('Frequency', 'Hz', 'linear'),
                           v.axis('Signal to noise', 'dB', 'linear'), labels, curves,
                           drawn_span(curves))
        return [v.staged(view, labels, range(len(labels)), ['measured'] * len(labels))]

    half = POINT_BUDGET // 2
    page = int(params['page']) - 1
    first = waterfall_arrays(loud, None, hz.SI, 'magnitude', half, None, page,
                             exponents=False)
    second = waterfall_arrays(quiet, None, hz.SI, 'magnitude', half, None, page,
                              exponents=False)
    others = dict(zip(second['labels'], second['curves'], strict=True))
    stations, curves, labels, depths, roles = [], [], [], [], []
    for label, curve in zip(first['labels'], first['curves'], strict=True):
        other = others.get(label)
        if other is None:
            continue
        k = len(stations)
        stations.append(label)
        curves += [other, curve]
        labels += [f'{quiet_name}: {label}', f'{loud_name}: {label}']
        depths += [k, k]
        roles += ['reference', 'measured']
    if not stations:
        raise refused(f'{loud_name} and {quiet_name} share no channel on this page')
    x, z = axes(loud, first['drawn'][0], 'magnitude', first)
    view = v.waterfall(f'{loud_name} over {quiet_name}', x, z, labels, curves,
                       drawn_span(curves), page=_pages(first))
    unmatched = len(first['labels']) + len(second['labels']) - 2 * len(stations)
    if unmatched:
        view['note'] = f'{unmatched} records only one of the two has are left out.'
    return [v.staged(view, stations, depths, roles)]


def _grid(spec: Any) -> tuple[np.ndarray, Any]:
    """The abscissa every curve of `spec` is drawn on, and how a record's
    values land on it, by the one drawing rule (`plot.drawing_shape`): a
    breakpoint law filled in at `LAW_POINTS` a decade, a density per bin
    flat across each bin, anything else through its points."""
    from hertzforge.core.compliance import log_interpolate
    from hertzforge.drawing.plot import drawing_shape, step_outline

    x = np.asarray(spec.abscissa, dtype=float)
    shape = drawing_shape(spec)
    if shape == 'law':
        positive = x[x > 0.0]
        decades = np.log10(positive.max() / positive.min())
        grid = np.unique(np.concatenate([
            positive, np.geomspace(positive.min(), positive.max(),
                                   max(int(decades * LAW_POINTS), 2))]))
        return grid, lambda values: log_interpolate(grid, x, values)
    if shape == 'steps':
        widths = spec.bin_edges() if getattr(spec, 'bandwidth', None) is not None else None
        grid, _ = step_outline(x, np.zeros(len(x)), widths)

        def stepped(values):
            values = np.where(np.asarray(values, float) > 0.0, values, np.nan)
            return step_outline(x, values, widths)[1]
        return grid, stepped
    return x, lambda values: np.where(np.asarray(values, float) > 0.0, values, np.nan)


def _zones(station: int, limits: dict[str, np.ndarray], x: np.ndarray) -> list[dict]:
    """A station's limit bands, as the 2-D comparison shades them: between
    warning and abort, and past abort to the stage's top or floor."""
    out = []
    for side in ('upper', 'lower'):
        warning, abort = limits.get(f'warning_{side}'), limits.get(f'abort_{side}')
        if warning is not None:
            zone = {'station': station, 'kind': 'warning', 'side': side, 'x': x,
                    'inner': warning}
            if abort is not None:
                zone['outer'] = abort
            out.append(zone)
        if abort is not None:
            out.append({'station': station, 'kind': 'abort', 'side': side, 'x': x,
                        'inner': abort})
    return out


def _exceed(station: int, spec: Any, row: int, x: np.ndarray, values: np.ndarray) -> list[dict]:
    """The lines of one measured record outside the abort limits of `spec`'s
    record `row`, each from its bin's edges and its limit's level there."""
    from hertzforge.core.compliance import log_interpolate, outside
    from hertzforge.drawing.plot import bin_edges

    spec_x = np.asarray(spec.abscissa, dtype=float)
    edges = bin_edges(x)
    out = []
    for side, over in (('upper', True), ('lower', False)):
        written = spec.limits.get(f'abort_{side}')
        if written is None:
            continue
        written = np.real(np.asarray(written[row]))
        level = log_interpolate(x, spec_x, written)
        hits = np.flatnonzero(outside(x, values, spec_x, written, over) & np.isfinite(level))
        if hits.size:
            out.append({'station': station, 'side': side, 'x0': edges[hits],
                        'x1': edges[hits + 1], 'z': level[hits]})
    return out


def spec_stage(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.spec_stage`: a specification's every channel with its
    warning and abort bands, one station a channel. With a measured PSD,
    the stations are the channels the two share, each with its measured
    curve colored by level, the requirement in gray, and the lines outside
    an abort limit shaded; the measurement is scaled as the comparison
    scales it."""
    import hertzforge as hz
    from hertzforge.core.compliance import matched_records
    from hertzforge.drawing.viz.waterfall import drawn_span, waterfall_arrays

    spec_name, name = params['specification'], params['spectrum']
    spec = project[spec_name]
    dofs = [str(d) for d in spec.response_dof]
    references = spec.reference_dof
    autos = [i for i, dof in enumerate(dofs)
             if references is None or str(references[i]) == dof]
    if not autos:
        raise refused(f'{spec_name} has no channel of its own to stand on the stage')
    grid, on_grid = _grid(spec)
    dimension = str(spec.ordinate_dim[autos[0]])
    x_axis = v.axis('Frequency', 'Hz', 'log' if spec.log_abscissa else 'linear')
    z_axis = v.axis('PSD', unit_text(dimension), 'log')
    title = spec_name

    pairs: list[tuple[str, int, int]] = []
    measured = None
    scale = 0
    if name:
        measured = project[name]
        scale, _source = _scale(project, spec_name, name)
        pairs = [(label, j, i) for label, j, i in matched_records(spec, measured, autos)]
        if not pairs:
            raise refused(f'{name} and {spec_name} have no channel in common')
        title = f'{name}{f" ({scale:+d} dB)" if scale else ""} against {spec_name}'
    else:
        pairs = [(dofs[j], j, -1) for j in autos]

    gain = 10.0 ** (scale / 10.0)
    stations, curves, labels, depths, roles = [], [], [], [], []
    zones, exceed = [], []
    left_out = 0
    for k, (label, j, i) in enumerate(pairs):
        stations.append(label)
        limits = {key: on_grid(np.real(np.asarray(values[j])))
                  for key, values in spec.limits.items() if values is not None
                  and np.isfinite(np.real(np.asarray(values[j]))).any()}
        curves.append((grid, on_grid(np.real(np.asarray(spec.ordinate[j])))))
        labels.append(f'{spec_name}: {label}')
        depths.append(k)
        roles.append('reference' if measured is not None else 'measured')
        for key in spec.LIMITS:
            if key in limits:
                curves.append((grid, limits[key]))
                labels.append(f'{key.replace("_", " ").capitalize()}: {label}')
                depths.append(k)
                roles.append('tolerance' if key.startswith('warning') else 'abort')
        zones += _zones(k, limits, grid)
        if measured is None:
            continue
        arrays = waterfall_arrays(measured, [i], hz.SI, exponents=False)
        if not arrays['drawn']:
            left_out += 1
            continue
        cx, cz = arrays['curves'][0]
        curves.append((cx, np.asarray(cz, dtype=float) * gain))
        labels.append(f'{name}: {label}')
        depths.append(k)
        roles.append('measured')
        full = np.real(np.asarray(measured.ordinate[i])) * gain
        exceed += _exceed(k, spec, j, np.asarray(measured.abscissa, dtype=float), full)

    view = v.waterfall(title, x_axis, z_axis, labels, curves, drawn_span(curves))
    if left_out:
        view['note'] = f'{left_out} channels of another quantity are left out.'
    return [v.staged(view, stations, depths, roles, zones, exceed)]


OPERATIONS = (
    Operation(
        id='project.paired_stage', title='Paired Stage', workspace='random', accepts='project',
        summary='Two densities in 3-D, channel by channel: overlaid, the louder colored by '
                'level and the quieter in gray, or divided as signal to noise in dB.',
        params=(FIRST, SECOND, PAIRED_MODE, PAGE), outputs=('waterfall',), run=paired_stage),
    Operation(
        id='project.spec_stage', title='Specification Stage', workspace='random',
        accepts='project',
        summary='A specification\'s every channel in 3-D with its warning and abort bands; '
                'with a measured PSD, each channel\'s measurement and the lines it went past '
                'an abort limit.',
        params=(SPECIFICATION, MEASURED), outputs=('waterfall',), run=spec_stage),
)
