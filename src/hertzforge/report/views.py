"""A report's blocks as the protocol's views (`schema/PROTOCOL.md`, Views).

Step 2 of the reports plan (the maintainers repo's REPORTS_PLAN.md): the
shell's viz kit, the same `ViewRenderer` every module draws with, draws a
report too. `page_blocks` builds the report as the page builds it
(`report._report_payload`: bound, captioned, numbered, in display units,
thinned for the file) and `block_views` turns each built block into views:

- a plot into a `spectrum`, its averaging frames into `regions`, a
  synthesized curve a follower of the curve it models. One control channel
  against its requirement has its zones shaded (`bands`, named by `key`),
  its lines outside an abort limit marked (`regions` of kind `over` and
  `under`), its numbers boxed (`readouts`) and the requirement's band to
  open on (`x_home`); several channels on one plot are the spectrum's
  `groups` (target, response and limits each), the limits as lines;
- a grid into a `spectrum` a channel and the layout that places them, a
  row a node and a column a direction (`_grid`);
- a scene into a `shape3d` with its modes;
- a table into a `table`;
- a bar chart into `bars`, judged here as the page judged it;
- a coherence map and a MAC into a `matrix`;
- a verdict into a `reading`; a photo into an `image`;
- a stage into the `waterfall` the app draws for that object.

The page's payload writes a log axis's values as their log10 (it draws in
decades); a view carries the values themselves and says the axis is log,
so they are put back here.
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
    return v.series(name, role, x, y, follows=follows)


def _plot(built: Mapping[str, Any], title: str) -> dict[str, Any] | None:
    """A plot block as one `spectrum`. Several channels are its groups, shown one at a
    time with their limits as lines; one channel alone has its zones shaded (`bands`,
    `key`), its lines outside abort marked (`regions`), its numbers (`readouts`) and
    the band it opens on (`x_home`)."""
    logx, logy = bool(built.get('logx')), bool(built.get('logy'))
    x = _values(built.get('x') or [], logx)
    block_edges = (_values(built['edges'], logx)
                   if built.get('steps') and built.get('edges') else None)
    curves = built.get('curves') or []
    series: list[dict[str, Any]] = []
    groups: list[dict[str, Any]] = []
    bands: list[dict[str, Any]] = []
    regions: list[dict[str, Any]] = []
    lone: dict[str, Any] = {}

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
            limits_from = len(series)
            for zone in channel.get('zones') or []:
                role = _ZONE_ROLES.get(zone.get('severity'), 'tolerance')
                sides: dict[str, Any] = {'lower': None, 'upper': None}
                for side in ('lower', 'upper'):
                    if zone.get(side) is not None:
                        sides[side] = _values(zone[side], logy)
                        series.append(_curve(f"{zone.get('severity', 'limit')} {side}", role,
                                             tx, sides[side], tedges))
                # the zone shaded between its two edges, or from its one edge to the
                # plot's: above the upper abort, below the lower
                kind = ('warning' if zone.get('severity') == 'warning'
                        else 'over' if sides['upper'] is None else 'under')
                bands.append({'kind': kind, 'x': _listed(tx),
                              'lower': None if sides['lower'] is None else _listed(sides['lower']),
                              'upper': None if sides['upper'] is None else _listed(sides['upper'])})
            # the lines outside an abort limit, each across its own bin
            for key in ('over', 'under'):
                regions += _runs(channel.get(key), x, block_edges, key)
            group: dict[str, Any] = {'label': str(channel['label']),
                                     'series': list(range(first, len(series)))}
            readouts = [{'label': str(r['label']), 'text': str(r['text']),
                         'status': _READOUT_STATUS.get(r.get('state'), 'info')}
                        for r in channel.get('readouts') or []]
            if readouts:
                group['readouts'] = readouts
            groups.append(group)
            lone = {'bands': bands, 'readouts': readouts, 'limits_from': limits_from,
                    'limits_to': len(series), 'key': channel.get('key') or []}
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
    averaging = built.get('averaging') or {}
    for x0, x1 in averaging.get('frames') or []:
        regions.append(v.region(float(x0), float(x1), 'frame', averaging.get('label') or None))
    view = v.spectrum(title, axis_of(built.get('xlabel', ''), logx),
                      axis_of(built.get('ylabel', ''), logy), series, regions=regions)
    if len(groups) > 1:
        # several channels on one plot, one shown at a time: the zones and marks of each
        # would lie over the others', so they are drawn where a channel has a plot alone
        view.pop('regions', None)
        kept = [region for region in regions if region['kind'] == 'frame']
        if kept:
            view['regions'] = kept
        return v.grouped(view, groups)
    if lone.get('bands'):
        # one channel alone: its zones are shaded, so their edges are not drawn as lines
        # too, which is how the page has always drawn a channel against its requirement
        del view['series'][lone['limits_from']:lone['limits_to']]
        view['bands'] = lone['bands']
    if lone.get('key'):
        view['key'] = [[str(code), str(label)] for code, label in lone['key']]
    if lone.get('readouts'):
        view['readouts'] = lone['readouts']
    home = built.get('home_x')
    if home and len(home) == 2 and all(value is not None for value in home):
        low, high = (float(value) for value in _values(home, logx))
        if high > low:
            view['x_home'] = [low, high]
    return view


def _listed(values: np.ndarray) -> list[float | None]:
    """An edge of a zone as the frame carries it: its numbers, a missing one null."""
    return [float(value) if np.isfinite(value) else None for value in values]


def _runs(marks: Any, x: np.ndarray, edges: np.ndarray | None, kind: str) -> list[dict[str, Any]]:
    """The lines a channel has outside an abort limit (`marks`, a level where a line is
    out and None where it is not) as spans of x, neighbors joined: each line across its
    own bin when the plot has `edges`, else from halfway to its neighbors."""
    if not marks:
        return []
    out = [value is not None for value in marks][:len(x)]
    if edges is not None and len(edges) == len(x) + 1:
        low, high = edges[:-1], edges[1:]
    else:
        mid = 0.5 * (x[:-1] + x[1:])
        low = np.concatenate([x[:1], mid])
        high = np.concatenate([mid, x[-1:]])
    spans: list[dict[str, Any]] = []
    k = 0
    while k < len(out):
        if not out[k]:
            k += 1
            continue
        start = k
        while k + 1 < len(out) and out[k + 1]:
            k += 1
        if np.isfinite(low[start]) and np.isfinite(high[k]) and high[k] > low[start]:
            spans.append(v.region(float(low[start]), float(high[k]), kind))
        k += 1
    return spans


def _rgb(colors: list[str]) -> np.ndarray:
    """'#rrggbb' colors as u8 RGB rows [N, 3]."""
    return np.array([[int(color[k:k + 2], 16) for k in (1, 3, 5)] for color in colors],
                    dtype=np.uint8).reshape(-1, 3)


def _scene(built: Mapping[str, Any], title: str, unit: str) -> dict[str, Any]:
    """A scene as one `shape3d`: its faces as triangles (a fan each), its
    lines, and its modes; with no modes, each face and line in its
    element's color (`element_colors`), as the app's 3-D view draws a
    geometry."""
    nodes = np.asarray(built.get('points') or np.zeros((0, 3)), dtype=float).reshape(-1, 3)
    triangles, triangle_colors = [], []
    for face in built.get('faces') or []:
        corners = [int(n) for n in face['nodes']]
        for k in range(1, len(corners) - 1):
            triangles.append((corners[0], corners[k], corners[k + 1]))
            triangle_colors.append(face['color'])
    lines = [(int(line[0]), int(line[1])) for line in built.get('lines') or []]
    line_colors = [line[2] for line in built.get('lines') or []]
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
    view = v.shape3d(title, unit, nodes, modes,
                     triangles=np.asarray(triangles, dtype=np.uint32).reshape(-1, 3),
                     lines=np.asarray(lines, dtype=np.uint32).reshape(-1, 2))
    home = built.get('home')
    if home:
        # the page's (right, up) back to the eye the 3-D view opens from: the
        # geometry's own view, or a model built Y up opens on its side
        right, up = (np.asarray(axis, dtype=float) for axis in home)
        view['home'] = {'eye': [float(c) for c in np.cross(right, up)],
                        'up': [float(c) for c in up]}
    # the colored elements stand still, so a scene that animates its modes
    # keeps the surface colored by displacement
    if (triangles or lines) and not modes:
        view['element_colors'] = {
            'triangles': np.asarray(triangles, dtype=np.uint32).reshape(-1, 3),
            'triangle_colors': _rgb(triangle_colors),
            'lines': np.asarray(lines, dtype=np.uint32).reshape(-1, 2),
            'line_colors': _rgb(line_colors)}
    if built.get('arrows'):
        # the solids the 3-D view draws for a channel table's DOFs
        from ..drawing.viz.geometry import dof_arrow_solids

        into = bool(built.get('arrows_incoming'))
        view['arrows'] = dof_arrow_solids(
            nodes, [(into, (int(arrow['node']), np.asarray(arrow['vector'], dtype=float),
                            int(arrow['axis'])))
                    for arrow in built['arrows']],
            float(built['arrow_length']), [arrow['label'] for arrow in built['arrows']])
    bodies = built.get('bodies')
    if bodies:
        view['bodies'] = {
            'nodes': np.asarray(bodies['nodes'], dtype=np.float32).reshape(-1, 3),
            'triangles': np.asarray(bodies['triangles'], dtype=np.uint32).reshape(-1, 3),
            'colors': np.asarray(bodies['colors'], dtype=np.uint8).reshape(-1, 3),
            'follow': np.asarray(bodies['follow'], dtype=np.uint32)}
    return view


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


def _grid(built: Mapping[str, Any], title: str
          ) -> tuple[list[dict[str, Any]], dict[str, Any]] | None:
    """A grid of channel figures as the page lays it out: a `spectrum` a figure, and the
    layout that places them, a row a node and a column a direction (`report.channel_grid`).
    Each cell lists its figures by their place among the views, each with its caption
    (the channel), its `note` (how far its direction is off the column's axis) and its
    `facts`, the lines read under it. A grid was one plot with its channels picked from a
    list, which drew neither the table nor the shading (Brandon, 2026-11-17)."""
    views: list[dict[str, Any]] = []
    rows: list[dict[str, Any]] = []
    for row in built.get('rows') or []:
        cells: list[list[dict[str, Any]]] = []
        for cell in row.get('cells') or []:
            placed: list[dict[str, Any]] = []
            for figure in cell:
                caption = str(figure.get('caption', ''))
                view = _plot(figure, caption or title)
                if view is None:
                    continue
                entry: dict[str, Any] = {'view': len(views), 'caption': caption}
                if figure.get('note'):
                    entry['note'] = str(figure['note'])
                facts = [[str(label), str(text)] for label, text in figure.get('facts') or []]
                if facts:
                    entry['facts'] = facts
                placed.append(entry)
                views.append(view)
            cells.append(placed)
        rows.append({'label': str(row.get('label', '')), 'cells': cells})
    if not views:
        return None
    return views, {'columns': [str(column) for column in built.get('columns') or []],
                   'rows': rows}


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
           objects: Mapping[str, Any], object_groups: Any = None) -> list[dict[str, Any]]:
    """A stage: a scalogram's sheet as it is, or else the waterfall the app
    draws for the block's source (the page's stage is in the stage's own
    coordinates, which cannot be read back as data), one view a page of
    it. The block's `select` picks the records, and a time history carries
    its averaging frames on the floor and the back wall, as the app's
    averaging stage draws them, without the span that drags them."""
    if built.get('sheet'):
        return [_scalogram(built, title)]
    from ..core.data import TimeHistory
    from ..core.report import resolve_binding
    from ..service.waterfalls import waterfall_view
    from . import _scalogram_candidates

    # the block names its source by a binding ('@basis:TimeHistory'), not by
    # the object's name: looked up as it stood, a stage found nothing and
    # dropped out of the page (2026-11-18)
    source = objects.get(resolve_binding(block.get('source', ''), objects, object_groups) or '')
    if source is None:
        return []
    records = _scalogram_candidates(block, source) if block.get('select') else None
    frames = None
    averaging = getattr(source, 'averaging', None)
    if isinstance(source, TimeHistory) and averaging is not None and block.get('averaging', True):
        from ..service.ops_settings import frame_regions

        try:
            frames = frame_regions(averaging, source.sample_rate, float(source.abscissa[0]))
        except ValueError:
            frames = None           # unevenly sampled: no frames to speak of
    views = []
    page, pages = 1, 1
    while page <= pages:
        try:
            view = waterfall_view(title, source, page=page, records=records)
        except Exception:  # noqa: BLE001 - a stage the app cannot draw is left out, as unbound
            break
        pages = int((view.pop('page', None) or {}).get('count', 1))
        view.pop('quantities', None)
        if frames:
            view['regions'] = frames
        views.append(view)
        page += 1
    return views


def block_views(built: Mapping[str, Any], block: Mapping[str, Any] | None = None,
                objects: Mapping[str, Any] | None = None,
                length_unit: str = '', object_groups: Any = None) -> list[dict[str, Any]]:
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
        # the plots alone: `page_blocks` asks `_grid` itself, for the layout with them
        laid = _grid(built, title)
        return [] if laid is None else laid[0]
    elif kind == 'stage':
        return _stage(block or {}, built, title, objects or {}, object_groups)
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
    staged: set[int] = set()
    for built in payload['blocks']:
        index = built.get('index')
        if built.get('kind') == 'stage' and not built.get('sheet'):
            # a stage block's further pages: the app's waterfall pages of its
            # records are drawn with its first, so they are not drawn again
            if index in staged:
                continue
            staged.add(index)
        source_block = (report.blocks[index] if isinstance(index, int)
                        and 0 <= index < len(report.blocks) else {})
        kind = built.get('kind')
        entry: dict[str, Any] = {'block': index, 'label': built.get('label') or '',
                                 'caption': str(built.get('caption') or '')}
        if built.get('note'):
            entry['note'] = str(built['note'])
        if kind == 'text':
            entry.update(kind='text', html=str(built.get('html', '')))
            if built.get('stamp'):
                # when it was checked rather than what the report holds
                # (`report.identity.STAMP`): out of the issued content's hash
                entry['stamp'] = True
        elif kind == 'unbound':
            entry.update(kind='unbound', was=str(built.get('was', 'block')),
                         empty=bool(built.get('empty')))
        else:
            if kind == 'grid':
                # its plots and where each goes, a row a node and a column a direction
                laid = _grid(built, str(built.get('label') or ''))
                views = [] if laid is None else laid[0]
                if laid is not None:
                    entry['grid'] = laid[1]
            else:
                views = block_views(built, source_block, objects, length, object_groups)
            entry.update(kind='figure' if views else 'unbound', views=views)
            if not views:
                entry['was'] = str(kind)
        out.append(entry)
    return out


def page_style(report: Any, objects: Mapping[str, Any]) -> str:
    """The stylesheet a report's page adds, as its own page adds it: with
    the report identity on, the printed page's margins (the laboratory, the
    report's number and revision, its title and page n of N); with the flash
    report on and a run log held, the log's tight setting. Empty otherwise."""
    from .features import enabled

    active = enabled(report)
    css = ''
    if 'report_identity' in active:
        from .identity import page_css

        css = page_css(report)
    if 'flash_report' in active:
        from ..core.run_log_extract import held
        from .flash import LOG_CSS

        if held(objects or {}):
            css += LOG_CSS
    return css
