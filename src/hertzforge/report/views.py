"""A report's blocks as the protocol's views (`schema/PROTOCOL.md`, Views).

Step 2 of the reports plan (the maintainers repo's REPORTS_PLAN.md): the
shell's viz kit, the same `ViewRenderer` every module draws with, draws a
report too. `page_blocks` builds the report as the page builds it
(`report._report_payload`: bound, captioned, numbered, in display units,
thinned for the file) and `block_views` turns each built block into views:

- a plot into a `spectrum`, its control channels into the spectrum's
  `groups` (target, response and limits each), its averaging frames into
  `regions`, a synthesized curve a follower of the curve it models;
- a scene into a `shape3d` with its modes;
- a table and a grid's channel table into a `table`;
- a bar chart into `bars`, judged here as the page judged it;
- a coherence map and a MAC into a `matrix`;
- a verdict into a `reading`; a photo into an `image`;
- a stage into the `waterfall` the app draws for that object.

The page's payload writes a log axis's values as their log10 (it draws in
decades); a view carries the values themselves and says the axis is log,
so they are put back here. A specification's limits, shaded on the old
page, are tolerance and abort lines here, as the app draws them.
"""

from __future__ import annotations

import base64
import re
from collections.abc import Mapping
from typing import Any

import numpy as np

from ..service import views as v

#: a label written "name [unit]", the page's way
_LABELED = re.compile(r'^(?P<name>.*?)\s*\[(?P<unit>[^\]]*)\]\s*$')
#: a zone's severity as the role its lines are drawn in
_ZONE_ROLES = {'warning': 'tolerance', 'abort': 'abort'}
#: a compliance readout's state as a reading's status
_READOUT_STATUS = {'in': 'pass', 'over': 'fail', 'under': 'fail'}


def axis_of(text: str, log: bool) -> dict[str, Any]:
    """An axis from the page's "name [unit]" label: the name with its first
    letter raised, the unit (None when there is none) and the scale."""
    text = str(text or '').removeprefix('log10 ').strip()
    match = _LABELED.match(text)
    name, unit = (match['name'], match['unit'] or None) if match else (text, None)
    name = name[:1].upper() + name[1:]
    return v.axis(name, unit, 'log' if log else 'linear')


def _values(values: Any, log: bool) -> np.ndarray:
    """The page's numbers as floats, a missing one NaN, out of decades on a
    log axis."""
    out = np.array([np.nan if value is None else value for value in values], dtype=float)
    return 10.0 ** out if log else out


def _curve(name: str, role: str, x: np.ndarray, y: np.ndarray, edges: np.ndarray | None,
           follows: int | None = None) -> dict[str, Any]:
    """One series: stepped across `edges` when they fit, else a line."""
    n = min(len(x), len(y))
    x, y = x[:n], y[:n]
    if edges is not None and len(edges) == n + 1:
        return v.series(name, role, x, y, 'steps', edges, follows=follows)
    out = v.series(name, role, x, y, follows=follows)
    return out


def _plot(built: Mapping[str, Any], title: str) -> dict[str, Any] | None:
    """A plot block as one `spectrum`, its channels as groups."""
    logx, logy = bool(built.get('logx')), bool(built.get('logy'))
    x = _values(built.get('x') or [], logx)
    block_edges = (_values(built['edges'], logx)
                   if built.get('steps') and built.get('edges') else None)
    curves = built.get('curves') or []
    series: list[dict[str, Any]] = []
    groups: list[dict[str, Any]] = []

    def own_x(curve: Mapping[str, Any]) -> np.ndarray:
        return x if curve.get('x') is None else _values(curve['x'], logx)

    def own_edges(curve: Mapping[str, Any]) -> np.ndarray | None:
        if curve.get('edges'):
            return _values(curve['edges'], logx)
        if curve.get('x') is None:
            return block_edges
        return None

    channels = built.get('channels') or []
    if channels:
        target = curves[0] if curves else {}
        tx, tedges = own_x(target), own_edges(target)
        for channel in channels:
            first = len(series)
            series.append(_curve(str(channel['label']), 'reference', tx,
                                 _values(channel['y'], logy), tedges))
            if channel.get('response') is not None:
                series.append(_curve('measured', 'measured', x,
                                     _values(channel['response'], logy), block_edges))
            for response in channel.get('responses') or []:
                series.append(_curve(str(response.get('label') or 'measured'), 'measured', x,
                                     _values(response['y'], logy), block_edges))
            for zone in channel.get('zones') or []:
                role = _ZONE_ROLES.get(zone.get('severity'), 'tolerance')
                for side in ('lower', 'upper'):
                    if zone.get(side) is not None:
                        series.append(_curve(f"{zone.get('severity', 'limit')} {side}", role,
                                             tx, _values(zone[side], logy), tedges))
            group: dict[str, Any] = {'label': str(channel['label']),
                                     'series': list(range(first, len(series)))}
            readouts = [{'label': str(r['label']), 'text': str(r['text']),
                         'status': _READOUT_STATUS.get(r.get('state'), 'info')}
                        for r in channel.get('readouts') or []]
            if readouts:
                group['readouts'] = readouts
            groups.append(group)
    else:
        colors: dict[Any, int] = {}
        for curve in curves:
            dashed = bool(curve.get('dash'))
            color = curve.get('color')
            follows = colors.get(color) if dashed and color is not None else None
            role = ('reference' if curve.get('gray') or (dashed and follows is None)
                    else 'measured')
            if not dashed and color is not None and color not in colors:
                colors[color] = len(series)
            series.append(_curve(str(curve.get('label') or ''), role, own_x(curve),
                                 _values(curve['y'], logy), own_edges(curve), follows))
    if not series:
        return None
    regions = []
    averaging = built.get('averaging') or {}
    for x0, x1 in averaging.get('frames') or []:
        regions.append(v.region(float(x0), float(x1), 'frame', averaging.get('label') or None))
    view = v.spectrum(title, axis_of(built.get('xlabel', ''), logx),
                      axis_of(built.get('ylabel', ''), logy), series, regions=regions)
    return v.grouped(view, groups) if len(groups) > 1 else view


def _scene(built: Mapping[str, Any], title: str, unit: str) -> dict[str, Any]:
    """A scene as one `shape3d`: its faces as triangles (a fan each), its
    lines, and its modes."""
    nodes = np.asarray(built.get('points') or np.zeros((0, 3)), dtype=float).reshape(-1, 3)
    triangles = []
    for face in built.get('faces') or []:
        corners = [int(n) for n in face['nodes']]
        for k in range(1, len(corners) - 1):
            triangles.append((corners[0], corners[k], corners[k + 1]))
    lines = [(int(line[0]), int(line[1])) for line in built.get('lines') or []]
    modes = []
    for k, mode in enumerate(built.get('modes') or []):
        info = mode.get('info') or {}
        shape = np.asarray(mode['real'], dtype=float).reshape(-1, 3)
        entry = v.mode(int(info.get('mode', k + 1)), float(info.get('frequency', 0.0)),
                       None if info.get('damping') is None else float(info['damping']) / 100.0,
                       shape, label=mode.get('label'))
        if mode.get('imag') is not None:
            entry['shape_imag'] = np.asarray(mode['imag'], np.float32).reshape(-1, 3)
        modes.append(entry)
    return v.shape3d(title, unit, nodes, modes,
                     triangles=np.asarray(triangles, dtype=np.uint32).reshape(-1, 3),
                     lines=np.asarray(lines, dtype=np.uint32).reshape(-1, 2))


def _bars(built: Mapping[str, Any], title: str) -> dict[str, Any]:
    """A bar chart as `bars`, judged as the page judged it: a one-sided
    floor (a signal-to-noise) counts a missing number under; a one-sided
    reading with no `high` carries its ceiling in `low`, at which a share
    of a band is in and a waveform error is out; otherwise both sides."""
    low, high = built.get('low'), built.get('high')
    name = axis_of(built.get('ylabel', ''), False)
    unit = name['unit'] or (str(built.get('units') or '').strip() or None)
    common = {'notes': built.get('notes'), 'baseline': float(built.get('baseline') or 0.0),
              'floor': built.get('floor')}
    if built.get('side') == 'under' and high is None:
        return v.bars(title, name['label'], unit, built['labels'], built['values'],
                      under=low, missing='under', **common)
    if high is None:
        return v.bars(title, name['label'], unit, built['labels'], built['values'],
                      over=low, at_limit='in' if built.get('inside_at_ceiling') else 'out',
                      **common)
    return v.bars(title, name['label'], unit, built['labels'], built['values'],
                  under=low, over=high, **common)


def _map(built: Mapping[str, Any], title: str) -> dict[str, Any]:
    """A coherence map as a `matrix`: a row a record, a column a frequency
    line, pinned 0 to 1."""
    x = axis_of(built.get('xlabel', ''), False)
    unit = f" {x['unit']}" if x['unit'] else ''
    return v.matrix(title, {'label': 'Record', 'ticks': [str(t) for t in built['labels']]},
                    {'label': x['label'], 'ticks': [f'{f:.4g}{unit}' for f in built['x']]},
                    _values([c for row in built['rows'] for c in row], False),
                    str(built.get('zlabel') or 'coherence').capitalize(), (0.0, 1.0))


def _mac(built: Mapping[str, Any], title: str) -> dict[str, Any]:
    """A MAC as a `matrix`, pinned 0 to 1."""
    return v.matrix(title, {'label': 'Mode', 'ticks': [str(t) for t in built['rows']]},
                    {'label': 'Mode', 'ticks': [str(t) for t in built['columns']]},
                    _values([c for row in built['matrix'] for c in row], False),
                    'MAC', (0.0, 1.0))


def _verdict(built: Mapping[str, Any], title: str) -> dict[str, Any]:
    """The pass/fail box as a `reading`."""
    passed = bool(built['passed'])
    lines_ok = built['lines_percent'] <= built['lines_limit']
    rms_ok = built['rms_percent'] <= built['rms_limit']
    return v.reading(title or 'Result', [
        v.row('Result', 'Pass' if passed else 'Fail', None, 'pass' if passed else 'fail'),
        v.row(str(built.get('level_label') or 'Test level'),
              float(built['test_level_db']), 'dB', 'info'),
        v.row('Control channels', float(built['channels']), None, 'info'),
        v.row('Lines outside abort', float(built['lines_percent']), '%',
              'pass' if lines_ok else 'fail'),
        v.row('Channels over RMS tolerance', float(built['rms_percent']), '%',
              'pass' if rms_ok else 'fail')])


def _image(built: Mapping[str, Any], title: str) -> dict[str, Any] | None:
    """A photo, its bytes as they were embedded."""
    head, _, encoded = str(built.get('src', '')).partition(',')
    fmt = head.removeprefix('data:image/').removesuffix(';base64')
    fmt = 'jpeg' if fmt in ('jpg', 'jpeg') else fmt
    if fmt not in ('png', 'jpeg'):
        return None
    return v.image(title, fmt, base64.b64decode(encoded))


def _grid(built: Mapping[str, Any], title: str) -> dict[str, Any] | None:
    """A grid of channel figures as one `spectrum`, a group a cell, named
    by its row and column, on the first cell's axes."""
    series: list[dict[str, Any]] = []
    groups: list[dict[str, Any]] = []
    first_view = None
    columns = built.get('columns') or []
    for row in built.get('rows') or []:
        for c, cell in enumerate(row.get('cells') or []):
            for figure in cell:
                view = _plot(figure, title)
                if view is None:
                    continue
                first_view = first_view or view
                start = len(series)
                for s in view['series']:
                    if 'follows' in s:
                        s = {**s, 'follows': s['follows'] + start}
                    series.append(s)
                column = columns[c] if c < len(columns) else ''
                groups.append({'label': f"{row.get('label', '')} {column}".strip()
                               or str(figure.get('caption', '')),
                               'series': list(range(start, len(series)))})
    if first_view is None:
        return None
    view = {**first_view, 'series': series}
    view.pop('groups', None)
    return v.grouped(view, groups)


def _scalogram(built: Mapping[str, Any], title: str) -> dict[str, Any]:
    """A scalogram's sheet as a `waterfall` surface: a row a frequency, its
    cone of influence as two walls."""
    sheet = built['sheet']
    xs = _values(sheet['xs'], bool(built.get('logx')))
    stations = [float(s) for s in sheet['stations']]
    levels = [_values(row, bool(built.get('log'))) for row in sheet['levels']]
    labels = [str(label) or f'{s:.4g}' for label, s in zip(built.get('labels') or
                                                         [''] * len(stations), stations,
                                                         strict=False)]
    curves = [(xs, row) for row in levels]
    finite = np.concatenate([row[np.isfinite(row)] for row in levels]) if levels else []
    z_range = (float(np.min(finite)), float(np.max(finite))) if len(finite) else None
    view = v.waterfall(title, axis_of(built.get('xlabel', ''), bool(built.get('logx'))),
                       axis_of(built.get('zlabel', ''), bool(built.get('log'))), labels,
                       curves, (float(np.nanmin(xs)), float(np.nanmax(xs))), z_range)
    return v.staged(view, labels, range(len(labels)), ['measured'] * len(labels),
                    surface=True)


def _stage(block: Mapping[str, Any], built: Mapping[str, Any], title: str,
           objects: Mapping[str, Any]) -> dict[str, Any] | None:
    """A stage: a scalogram's sheet as it is, or else the waterfall the app
    draws for the block's source (the page's stage is in the stage's own
    coordinates, which cannot be read back as data)."""
    if built.get('sheet'):
        return _scalogram(built, title)
    from ..service.waterfalls import waterfall_view

    source = objects.get(block.get('source', ''))
    if source is None:
        return None
    try:
        return waterfall_view(title, source)
    except Exception:  # noqa: BLE001 - a stage the app cannot draw is left out, as unbound
        return None


def block_views(built: Mapping[str, Any], block: Mapping[str, Any] | None = None,
                objects: Mapping[str, Any] | None = None,
                length_unit: str = '') -> list[dict[str, Any]]:
    """The views one built block draws as (none for text or an unbound
    slot). `block` and `objects` are the block as the report holds it and
    the project's objects, which only a stage needs; `length_unit` is the
    unit a scene's coordinates are in."""
    kind = built.get('kind')
    title = str(built.get('label') or '')
    if kind == 'plot':
        made = _plot(built, title)
    elif kind == 'scene':
        made = _scene(built, title, length_unit)
    elif kind == 'table':
        made = v.table(title, [str(h) for h in built['headers']],
                       [[str(cell) for cell in row] for row in built['rows']])
    elif kind == 'bars':
        made = _bars(built, title)
    elif kind == 'map':
        made = _map(built, title)
    elif kind == 'mac':
        made = _mac(built, title)
    elif kind == 'verdict':
        made = _verdict(built, title)
    elif kind == 'image':
        made = _image(built, title)
    elif kind == 'grid':
        made = _grid(built, title)
    elif kind == 'stage':
        made = _stage(block or {}, built, title, objects or {})
    else:
        made = None
    return [] if made is None else [made]


def page_blocks(report: Any, objects: Mapping[str, Any], unit_system: Any = None,
                object_groups: Any = None) -> list[dict[str, Any]]:
    """The report as the shell's page draws it: each drawn block its
    `kind` ('text' with its `html`, 'figure' with its `views`, or
    'unbound' with what it was), its number (`label`), `caption`, `note`
    and the index of the report block it came from (`block`), so an edit
    reaches the block whichever of its figures was clicked."""
    from ..units import DEFAULT_SYSTEM
    from . import _report_payload

    us = unit_system or DEFAULT_SYSTEM
    payload = _report_payload(report, objects, us, edit=True, object_groups=object_groups)
    length = us.label_text('length')
    out = []
    for built in payload['blocks']:
        index = built.get('index')
        source_block = (report.blocks[index] if isinstance(index, int)
                        and 0 <= index < len(report.blocks) else {})
        kind = built.get('kind')
        entry: dict[str, Any] = {'block': index, 'label': built.get('label') or '',
                                 'caption': str(built.get('caption') or '')}
        if built.get('note'):
            entry['note'] = str(built['note'])
        if kind == 'text':
            entry.update(kind='text', html=str(built.get('html', '')))
        elif kind == 'unbound':
            entry.update(kind='unbound', was=str(built.get('was', 'block')),
                         empty=bool(built.get('empty')))
        else:
            views = block_views(built, source_block, objects, length)
            entry.update(kind='figure' if views else 'unbound', views=views)
            if not views:
                entry['was'] = str(kind)
        out.append(entry)
    return out
