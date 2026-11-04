"""What a selection in the project tree shows.

Visual Dynamics' window draws whatever is selected in its tree
(`gui.main_window.render_current`, `docs/guide/project-tree.md`): one
object by its kind, records picked inside one, or several together when
the combination means something. `project.show` answers the same from the
operations the service already has, so the shell draws a selection the
way a result is drawn.

One object: a geometry draws; a data object is its waterfall, or the
records picked as curves; a shape set is its modes on the geometry of its
group (or the active one) with its mode table, or the table and its MAC
against itself without one; a channel table and matched modes are their
tables. Two together: data on a geometry animates it (a shape set's modes;
a time history played back sample by sample; an FRF's or a spectrum's
operating deflection shape at its loudest line; a PSD's envelope); a
channel table on a geometry marks its channels; two shape sets give their
MAC; a measurement beside its requirement gives the comparison's spectra;
two PSDs overlay. Anything else shows each object on its own, and the note
says what was left out.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .datasets import _mesh
from .protocol import ProtocolError, refused

#: how the shell numbers records when it asks an operation for some
FIRST_RECORD = 1


def _run(project: Any, op_id: str, **params: Any) -> list[dict]:
    """Run an operation as the shell would, its other params at their
    defaults, and return its views."""
    from . import projects
    from .server import OPERATIONS

    op = next(o for o in OPERATIONS if o.id == op_id)
    values = op.resolve(params, objects={name: projects.kind_of(obj)
                                         for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return op.run(project, values)


def _runs(indices: list[int]) -> str:
    """'1-3, 7' for 0-based `indices`, as Plot Records takes them."""
    out: list[str] = []
    for i in sorted(set(indices)):
        n = i + FIRST_RECORD
        if out and out[-1].endswith(f'-{n - 1}'):
            out[-1] = out[-1].rsplit('-', 1)[0] + f'-{n}'
        elif out and out[-1] == str(n - 1):
            out[-1] = f'{n - 1}-{n}'
        else:
            out.append(str(n))
    return ', '.join(out)


def _kind(obj: Any) -> str:
    from . import projects

    return projects.kind_of(obj)


def _is_data(obj: Any) -> bool:
    from hertzforge.core.data import DataArray

    return isinstance(obj, DataArray)


def _geometry_for(project: Any, name: str) -> str | None:
    """The geometry an object is drawn on: its group's, else the active
    one, else the only one (`Project.geometry_for`)."""
    found = project.geometry_for(name)
    return found[0] if found else None


#: how the window reads data, kept across selections as Visual Dynamics keeps
#: its bar: the stage or the flat plot, decades on the frequency axis, and the
#: component of complex data
PREFS = {'flat': False, 'log_frequency': False, 'component': 'magnitude', 'line': None,
         'projection': False}


def _alone(project: Any, name: str, picks: list[int],
           prefs: dict[str, Any] | None = None) -> tuple[list[dict], list[str]]:
    """One object as the window shows it; (views, notes)."""
    from hertzforge.core.channel_table import ChannelTable
    from hertzforge.core.geometry import Geometry
    from hertzforge.core.matches import MatchedModes
    from hertzforge.core.shapes import ShapeSet

    obj = project[name]
    if isinstance(obj, Geometry):
        return _run(project, 'project.view_geometry', geometry=name), []
    if isinstance(obj, ShapeSet):
        geometry = _geometry_for(project, name)
        views: list[dict] = []
        if geometry is not None:
            views += _run(project, 'project.mode_shapes', shapes=name, geometry=geometry)
            views += _run(project, 'project.table', object=name)
        else:
            views += _run(project, 'project.table', object=name)
            views += _run(project, 'project.mac', first=name, second=name)
        notes = [f'mode {picks[0] + 1} picked' if len(picks) == 1 else
                 f'{len(picks)} modes picked'] if picks else []
        return views, notes
    if isinstance(obj, (ChannelTable, MatchedModes)):
        return _run(project, 'project.table', object=name), []
    if _is_data(obj):
        prefs = {**PREFS, **(prefs or {})}
        component = prefs['component']
        if picks:
            return _run(project, 'project.plot', data=name, records=_runs(picks),
                        component=component), []
        if prefs['flat']:
            # the flat plot draws every record, as VD's does when 3D is off
            return _run(project, 'project.plot', data=name,
                        records=_runs(list(range(int(obj.num_records)))),
                        component=component), []
        views = _run(project, 'project.waterfall', data=name, component=component)
        _faced(views, obj)
        return views, []
    from hertzforge.core.report import Report

    if isinstance(obj, Report):
        blocks = len(getattr(obj, 'blocks', ()) or ())
        return [v.reading(name, [
            v.row('Report', getattr(obj, 'title', '') or name, None, 'info'),
            v.row('Blocks', float(blocks), '', 'info'),
            v.row('Edited in', 'the Report workspace', None, 'info')])], []
    return [], [f'{name} ({_kind(obj).replace("_", " ")}) has no view of its own here yet']


def _faced(views: list[dict], obj: Any) -> None:
    """A coherence opens on the map, the stage seen from above with the
    channels down and frequency across, as VD's coherence does; the bar's
    Curves is the perspective."""
    from hertzforge.core.data import Coherence, MultipleCoherence

    if isinstance(obj, (Coherence, MultipleCoherence)):
        for view in views:
            if view.get('view') == 'waterfall':
                view['facing'] = 'top'


#: how many records a cross-object stage stacks before standing down: the
#: stack copies the rows drawn, and two whole surveys would be gigabytes
STACK_LIMIT = 500


def _stacked(project: Any, names: list[str], prefs: dict[str, Any]
             ) -> tuple[list[dict], list[str]] | None:
    """Several data objects of one type on one stage, as VD's
    `_stacked_entry` draws two SRS or an FRF from each of two surveys
    station by station: the same concrete type and the same abscissa,
    their records stacked into one object for the drawing only. Nothing
    is merged or stored, so the merge rules' refusals (duplicate pairs,
    mixed capture counts) do not apply: they protect stored objects from
    telling false stories, and a drawing tells none. None otherwise, and
    the caller shows each on its own."""
    from .waterfalls import waterfall_view

    objects = [project[name] for name in names]
    if len(objects) < 2 or not all(_is_data(obj) for obj in objects):
        return None
    first = objects[0]
    if any(type(obj) is not type(first) for obj in objects):
        return None
    if any(not np.array_equal(obj.abscissa, first.abscissa) for obj in objects[1:]):
        return None
    if sum(int(obj.num_records) for obj in objects) > STACK_LIMIT:
        return None
    referenced = first.reference_dof is not None
    rows: list[Any] = []
    resp: list[str] = []
    ref: list[str] = []
    dims: list[str] = []
    units: list[Any] = []
    runits: list[Any] = []
    hints: list[Any] = []
    for data in objects:
        ordinate = np.asarray(data.ordinate)
        n = int(data.num_records)
        for i in range(n):
            rows.append(ordinate[i])
            resp.append(data.response_dof[i])
            if referenced:
                ref.append(data.reference_dof[i])
            dims.append(data.ordinate_dim[i])
            units.append((data.ordinate_unit or [None] * n)[i])
            runits.append((data.reference_unit or [None] * n)[i])
            hints.append((data.dimension_hint or [None] * n)[i])
    stacked = type(first)(first.abscissa, np.asarray(rows), response_dof=resp,
                          reference_dof=ref if referenced else None, ordinate_dim=dims,
                          ordinate_unit=units, reference_unit=runits, dimension_hint=hints)
    title = ' + '.join(dict.fromkeys(names))
    note = f'{len(objects)} objects on one stage'
    if prefs['flat']:
        import hertzforge as hz
        from hertzforge.drawing.viz.waterfall import waterfall_arrays

        from .waterfalls import axes

        records = list(range(int(stacked.num_records)))
        arrays = waterfall_arrays(stacked, records, unit_system=hz.SI, exponents=False,
                                  component=prefs['component'])
        lines = [v.series(stacked.record_label(r), 'measured', x, y)
                 for r, (x, y) in zip(arrays['drawn'], arrays['curves'], strict=True)]
        return [v.spectrum(title, *axes(stacked, arrays['drawn'][0], prefs['component'], arrays),
                           lines)], [note]
    view = waterfall_view(title, stacked, component=prefs['component'])
    _faced([view], first)
    return [view], [note]


def _column(data: Any, picks: list[int]) -> list[int] | None:
    """The picked records as one CPSD reference column, or None: a column
    against one reference carries each response's magnitude and phase
    relative to it, an honest complex shape; a whole CPSD has no
    reference chosen and the diagonal is the autos (VD's `_cpsd_column`)."""
    from hertzforge.core.data import channel_quantities

    if len(picks) < 2 or data.reference_dof is None or not np.iscomplexobj(data.ordinate):
        return None
    quantities = [channel_quantities(data.known_dim(i)) for i in picks]
    references = {(data.reference_dof[i], quantities[k][1]) for k, i in enumerate(picks)}
    if len(references) != 1:
        return None
    if all(data.response_dof[i] == data.reference_dof[i] for i in picks):
        return None
    kinds = [response for response, _reference in quantities]
    wanted = max(set(kinds), key=kinds.count)
    return [i for i, kind in zip(picks, kinds, strict=True) if kind == wanted]


def _matched(project: Any, first: str, second: str) -> tuple[str, Any] | None:
    """The Matched Modes object between these two sets, (name, object), or
    None (VD's `_matched_for`)."""
    from hertzforge.core.matches import MatchedModes

    return next(((name, obj) for name, obj in project.items()
                 if isinstance(obj, MatchedModes) and obj.first == first
                 and obj.second == second), None)


def _mac(project: Any, first: str, second: str, geometry: str | None,
         prefs: dict[str, Any]) -> tuple[list[dict], list[str]]:
    """Two shape sets: their MAC, each axis ticked by its own frequencies,
    and on a geometry the stage the matrix selects a pair on, the column
    mode overlaid on the row mode (`ops_modal_run.paired`). Sets on two
    geometries compare after projection onto the first's DOFs
    (`Project.comparison_mac`), and `prefs['projection']` animates the
    projection on the basis geometry instead of both sets by DOF name.
    Pairs already committed to a Matched Modes object ride on the matrix
    as `matched`, as VD's red checker."""
    from .ops_modal_run import paired

    rows, columns = project[first], project[second]
    if not rows.num_shapes or not columns.num_shapes:
        raise refused(f'{first if not rows.num_shapes else second} has no modes')
    homes = project.geometry_for(first), project.geometry_for(second)
    apart = all(home is not None for home in homes) and homes[0][1] is not homes[1][1]
    notes: list[str] = []
    if apart and prefs.get('projection'):
        views = _run(project, 'project.projection_overlay', first=first, second=second,
                     tolerance=2.0)
    else:
        try:
            values = (rows.auto_mac() if first == second
                      else project.comparison_mac(first, second))
        except ValueError as error:
            raise refused(str(error)) from None

        def axis(name: str, shapes: Any) -> dict:
            return {'label': name, 'ticks': [f'{f:.1f} Hz' for f in shapes.frequency]}

        title = (f'Auto-MAC of {first}' if first == second
                 else f'MAC of {first} Against {second}')
        matrix = v.matrix(title, axis(first, rows), axis(second, columns), values, 'MAC',
                          (0.0, 1.0))
        views = paired(None if geometry is None else project[geometry], matrix, rows,
                       range(rows.num_shapes), columns, range(columns.num_shapes))
        if apart:
            notes.append(f'the MAC after projecting {second} onto {first}; both drawn on '
                         f'{geometry} by DOF name, Projection animates the projection')
    found = _matched(project, first, second)
    if found is not None:
        views[0]['matched'] = [[int(a), int(b)] for a, b in found[1].pairs]
        notes.append(f'{len(found[1].pairs)} pairs matched in {found[0]}')
    return views, notes


def _drawn_rows(geometry: Any) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    """The mesh as the shape view draws it, and each geometry node's drawn
    index (-1 when it is not drawn)."""
    drawn_rows, triangles, lines = _mesh(geometry)
    drawn = np.full(geometry.num_nodes, -1, dtype=np.int64)
    drawn[drawn_rows] = np.arange(drawn_rows.size)
    return drawn_rows, triangles, lines, drawn


def _played(data: Any, picks: list[int]) -> tuple[list[int], str, list[str]]:
    """The records an animation plays (`deform.animation_records`): one
    quantity, one record per DOF; the notes say what was left out."""
    from hertzforge.deform import animation_records, quantity_records

    records, quantity, other = quantity_records(data, picks or None)
    records, repeated = animation_records(data, records)
    notes = []
    if other:
        notes.append(f'{other} records of another quantity are not shown')
    if repeated:
        notes.append(repeated.lstrip(' —'))
    return records, quantity, notes


def _motion(project: Any, geometry_name: str, data_name: str, picks: list[int]
            ) -> tuple[list[dict], list[str]]:
    """A time history played back on a geometry, sample by sample, with
    the records it plays as a plot under it, the two sharing the `time`
    link (as `modal_run.motion` plays a survey's capture)."""
    from hertzforge.deform import TimeDeflection, dof_directions

    from .waterfalls import unit_text

    geometry, data = project[geometry_name], project[data_name]
    records, quantity, notes = _played(data, picks)
    dofs = [data.response_dof[i] for i in records]
    rows, directions, used = dof_directions(geometry, dofs)
    drawn_rows, triangles, lines, drawn = _drawn_rows(geometry)
    shown = drawn[rows] >= 0
    played = np.asarray(records)[used][shown]
    values = np.asarray(data.ordinate)[played].real
    if not values.shape[0]:
        return [], [f'none of {data_name}\'s {quantity} records lands on {geometry_name}']
    if not np.all(used) or not np.all(shown):
        notes.append(f'{len(dofs) - values.shape[0]} records have no node on the geometry')
    abscissa = np.asarray(data.abscissa, dtype=float)
    value_unit = unit_text(data.known_dim(records[0])) or ''
    title = f'{data_name} on {geometry_name}'
    extra: dict[str, Any] = {'link': 'time', 'motion': {
        'label': 'Time', 'unit': 's', 'start': float(abscissa[0]),
        'step': float(abscissa[1] - abscissa[0]) if abscissa.size > 1 else 0.0,
        'quantity': quantity.capitalize(), 'value_unit': value_unit,
        'node': drawn[rows[shown]].astype(np.uint32),
        'direction': np.asarray(directions, dtype=np.float32)[shown],
        'values': np.ascontiguousarray(values, np.float32),
        'peak': TimeDeflection(geometry, list(np.asarray(dofs)[used][shown]),
                               values).peak_magnitude}}
    plot = v.spectrum(title, v.axis('Time', 's', 'linear'),
                      v.axis(quantity.capitalize(), value_unit or None, 'linear'),
                      [v.series(data.record_label(i), 'measured', abscissa, values[d])
                       for d, i in enumerate(played)], link='time')
    stage = v.shape3d(title, 'm' if geometry.units_defined else 'undefined',
                      np.asarray(geometry.node_xyz, dtype=float)[drawn_rows], [],
                      triangles=triangles, lines=lines,
                      node_ids=np.asarray(geometry.node_id)[drawn_rows], **extra)
    return [stage, plot], notes


def _pattern(project: Any, geometry_name: str, data_name: str, picks: list[int],
             prefs: dict[str, Any] | None = None) -> tuple[list[dict], list[str]]:
    """Frequency data on a geometry: the operating deflection shape of
    complex data (`deform.OdsDeflection`); a CPSD's principal shape, the
    cross-spectral matrix's dominant eigenvector per line, or, with one
    reference column picked, the shape relative to that reference; a PSD's
    envelope (`deform.EnvelopeDeflection`). At the line the plot's cursor
    asks for (`prefs['line']`, Hz), else the loudest. One shape the view
    sweeps, beside the records as curves, the two sharing the frequency
    cursor."""
    from hertzforge.core.data import Psd
    from hertzforge.deform import EnvelopeDeflection, OdsDeflection

    prefs = {**PREFS, **(prefs or {})}
    geometry, data = project[geometry_name], project[data_name]
    notes: list[str] = []
    flavor = 'ODS'
    envelope = False
    column = _column(data, picks) if isinstance(data, Psd) else None
    if column is not None:
        records = column
        dofs = [data.response_dof[i] for i in records]
        ordinate = np.asarray(data.ordinate)[records]
        notes.append(f'relative to reference {data.reference_dof[records[0]]}')
    elif isinstance(data, Psd) and data.reference_dof is not None:
        try:
            dofs, ordinate, used = data.principal_shapes()
            records = [i for i in range(int(data.num_records))
                       if data.response_dof[i] == data.reference_dof[i]]
            flavor = f'Principal {used} shape'
        except ValueError as why:
            records, _quantity, notes = _played(data, picks)
            dofs = [data.response_dof[i] for i in records]
            ordinate = np.asarray(data.ordinate)[records]
            envelope = True
            notes.append(f'the envelope: {why}')
    else:
        records, _quantity, notes = _played(data, picks)
        dofs = [data.response_dof[i] for i in records]
        ordinate = np.asarray(data.ordinate)[records]
        envelope = isinstance(data, Psd)
    drawn_rows, triangles, lines, drawn = _drawn_rows(geometry)
    deflection = (EnvelopeDeflection if envelope else OdsDeflection)(geometry, dofs, ordinate)
    if not len(deflection.rows) or not deflection.num_lines:
        return [], [f'none of {data_name}\'s records lands on {geometry_name}']
    abscissa = np.asarray(data.abscissa, dtype=float)
    if prefs.get('line') is not None:
        line = int(np.argmin(np.abs(abscissa - float(prefs['line']))))
    else:
        line = deflection.strongest_line
    deflection.line = line
    frequency = float(abscissa[line])
    shape = np.zeros((drawn_rows.size, 3), dtype=complex)
    at = drawn[deflection.rows]
    kept = at >= 0
    if envelope:
        shape[at[kept]] = deflection.offsets(0.0)[kept]
        label = f'Envelope at {frequency:.4g} Hz'
    else:
        shape[at[kept]] = (deflection.offsets(0.0) - 1j * deflection.offsets(np.pi / 2))[kept]
        label = f'{flavor} at {frequency:.4g} Hz'
    if not np.all(kept):
        notes.append(f'{int((~kept).sum())} moving nodes are not drawn')
    title = f'{data_name} on {geometry_name}'
    stage = v.shape3d(title, 'm' if geometry.units_defined else 'undefined',
                      np.asarray(geometry.node_xyz, dtype=float)[drawn_rows],
                      [v.mode(1, frequency, None, shape, label=label)],
                      triangles=triangles, lines=lines, normalization='unscaled',
                      node_ids=np.asarray(geometry.node_id)[drawn_rows],
                      cursor='frequency')
    curves = _run(project, 'project.plot', data=data_name, records=_runs(records),
                  component=prefs['component'])
    for curve in curves:
        if curve.get('view') == 'spectrum':
            curve['link'] = 'frequency'
    return [stage, *curves], notes


def _overlaid(project: Any, first: str, second: str) -> dict:
    """Two geometries in one scene, as a test geometry is laid over the
    model it was built from: one mesh of both, the second's nodes after
    the first's, each keeping its own units."""
    views = []
    ids = []
    for name in (first, second):
        geometry = project[name]
        rows, triangles, lines = _mesh(geometry)
        views.append((np.asarray(geometry.node_xyz, dtype=float)[rows], triangles, lines,
                      geometry.units_defined))
        ids.append(np.asarray(geometry.node_id)[rows])
    (nodes_a, tri_a, lines_a, defined_a), (nodes_b, tri_b, lines_b, defined_b) = views
    offset = len(nodes_a)
    triangles = [t for t in (tri_a, None if tri_b is None or not len(tri_b) else tri_b + offset)
                 if t is not None and len(t)]
    lines = [t for t in (lines_a, None if lines_b is None or not len(lines_b) else lines_b + offset)
             if t is not None and len(t)]
    return v.shape3d(f'{second} over {first}',
                     'm' if defined_a and defined_b else 'undefined',
                     np.vstack([nodes_a, nodes_b]), [],
                     triangles=np.vstack(triangles) if triangles else None,
                     lines=np.vstack(lines) if lines else None,
                     node_ids=np.concatenate(ids))


def _pair(project: Any, first: str, second: str, records: dict[str, list[int]],
          prefs: dict[str, Any] | None = None) -> tuple[list[dict], list[str]] | None:
    """What two objects show together, or None when the pair has no joint
    reading."""
    from hertzforge.core.channel_table import ChannelTable
    from hertzforge.core.data import (
        Psd,
        ShockSpecification,
        Specification,
        Srs,
        TimeHistory,
        TransientSpecification,
    )
    from hertzforge.core.geometry import Geometry
    from hertzforge.core.shapes import ShapeSet

    a, b = project[first], project[second]

    def of(cls: Any) -> str | None:
        for name, obj in ((first, a), (second, b)):
            if isinstance(obj, cls):
                return name
        return None

    geometry = of(Geometry)
    if isinstance(a, Geometry) and isinstance(b, Geometry):
        return [_overlaid(project, first, second)], []
    if geometry is not None:
        other = second if geometry == first else first
        obj = project[other]
        picks = records.get(other, [])
        if isinstance(obj, ShapeSet):
            views = _run(project, 'project.mode_shapes', shapes=other, geometry=geometry)
            return views + _run(project, 'project.table', object=other), []
        if isinstance(obj, ChannelTable):
            return _run(project, 'project.dof_arrows', source=other), []
        if isinstance(obj, TimeHistory):
            return _motion(project, geometry, other, picks)
        if _is_data(obj):
            abscissa = getattr(obj, 'abscissa_dim', '')
            if isinstance(obj, Psd) or (abscissa == 'frequency'
                                        and np.iscomplexobj(obj.ordinate)):
                return _pattern(project, geometry, other, picks, prefs)
        return None
    if isinstance(a, ShapeSet) and isinstance(b, ShapeSet):
        return _mac(project, first, second, _geometry_for(project, first),
                    {**PREFS, **(prefs or {})})
    spec = of(Specification)
    if spec is not None and all(isinstance(o, Psd) for o in (a, b)):
        measured = second if spec == first else first
        if not isinstance(project[measured], Specification):
            return _run(project, 'project.spec_spectra', spectrum=measured, specification=spec), []
    shock = of(ShockSpecification)
    if shock is not None and all(isinstance(o, Srs) for o in (a, b)):
        measured = second if shock == first else first
        if not isinstance(project[measured], ShockSpecification):
            return _run(project, 'project.srs_spectra', srs=measured, requirement=shock), []
    transient = of(TransientSpecification)
    if transient is not None and all(isinstance(o, TimeHistory) for o in (a, b)):
        measured = second if transient == first else first
        if not isinstance(project[measured], TransientSpecification):
            return _run(project, 'project.replication_waveforms', history=measured,
                        reference=transient), []
    if all(type(o) is Psd for o in (a, b)):
        return _run(project, 'project.psd_overlay', first=first, second=second), []
    return _stacked(project, [first, second], {**PREFS, **(prefs or {})})


def _compared(project: Any, objects: list[str], prefs: dict[str, Any]
              ) -> tuple[list[dict], list[str]] | None:
    """A geometry and two shape sets: their MAC and the pair it selects on
    that geometry; None for any other three."""
    from hertzforge.core.geometry import Geometry
    from hertzforge.core.shapes import ShapeSet

    geometries = [name for name in objects if isinstance(project[name], Geometry)]
    sets = [name for name in objects if isinstance(project[name], ShapeSet)]
    if len(geometries) != 1 or len(sets) != 2:
        return None
    return _mac(project, sets[0], sets[1], geometries[0], prefs)


def _offers(project: Any, objects: list[str]) -> dict[str, bool]:
    """Which of the bar's choices the selection can take: the stage or the
    flat plot for a data object, a component for complex frequency data,
    decades for anything on a frequency axis."""
    from hertzforge.core.shapes import ShapeSet

    data = [project[name] for name in objects if _is_data(project[name])]
    sets = [name for name in objects if isinstance(project[name], ShapeSet)]
    homes = [project.geometry_for(name) for name in sets]
    return {
        'projection': (len(sets) == 2 and all(home is not None for home in homes)
                       and homes[0][1] is not homes[1][1]),
        'flat': bool(data),
        'complex': any(getattr(obj, 'abscissa_dim', '') == 'frequency'
                       and np.iscomplexobj(obj.ordinate) for obj in data),
        'frequency': any(getattr(obj, 'abscissa_dim', '') == 'frequency' for obj in data),
    }


def _decades(views: list[dict]) -> None:
    """Every frequency axis in log, as VD's Log f draws them for the session."""
    for view in views:
        for key in ('x', 'z'):
            axis = view.get(key)
            if isinstance(axis, dict) and axis.get('unit') == 'Hz':
                axis['scale'] = 'log'


def show(project: Any, objects: list[str], records: dict[str, list[int]],
         part: dict[str, Any] | None, prefs: dict[str, Any] | None = None) -> dict[str, Any]:
    """What `objects` show together: `{'title', 'views', 'note', 'offers'}`,
    read as `prefs` ask (`PREFS`)."""
    prefs = {**PREFS, **(prefs or {})}
    notes: list[str] = []
    views: list[dict] = []
    try:
        if len(objects) == 2:
            paired = _pair(project, objects[0], objects[1], records, prefs)
            if paired is not None:
                views, notes = paired
        elif len(objects) == 3:
            compared = _compared(project, objects, prefs)
            if compared is not None:
                views, notes = compared
        if len(objects) > 2 and not views:
            stacked = _stacked(project, objects, prefs)
            if stacked is not None:
                views, notes = stacked
        if not views:
            for name in objects:
                more, said = _alone(project, name, records.get(name, []), prefs)
                views += more
                notes += said
            if len(objects) > 1 and views:
                notes.append('each object on its own')
    except ProtocolError as error:
        raise refused(str(error)) from None
    if prefs['log_frequency']:
        _decades(views)
    if part and part.get('kind') and len(objects) == 1:
        notes.append(f'{part["kind"]} {part.get("id", "")} picked; the whole geometry is drawn'.strip())
    title = ' and '.join(objects) if len(objects) <= 3 else f'{objects[0]} and {len(objects) - 1} more'
    return {'title': title, 'views': views, 'note': '; '.join(n for n in notes if n),
            'offers': _offers(project, objects)}
