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
PREFS = {'flat': False, 'log_frequency': False, 'component': 'magnitude'}


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
        return _run(project, 'project.waterfall', data=name, component=component), []
    from hertzforge.core.report import Report

    if isinstance(obj, Report):
        blocks = len(getattr(obj, 'blocks', ()) or ())
        return [v.reading(name, [
            v.row('Report', getattr(obj, 'title', '') or name, None, 'info'),
            v.row('Blocks', float(blocks), '', 'info'),
            v.row('Edited in', 'the Report workspace', None, 'info')])], []
    return [], [f'{name} ({_kind(obj).replace("_", " ")}) has no view of its own here yet']


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
                      triangles=triangles, lines=lines, **extra)
    return [stage, plot], notes


def _pattern(project: Any, geometry_name: str, data_name: str, picks: list[int]
             ) -> tuple[list[dict], list[str]]:
    """Frequency data on a geometry: the operating deflection shape of
    complex data at its loudest line (`deform.OdsDeflection`), or a
    PSD's envelope there (`deform.EnvelopeDeflection`), each as one shape
    the view sweeps, beside the records as curves."""
    from hertzforge.core.data import Psd
    from hertzforge.deform import EnvelopeDeflection, OdsDeflection

    geometry, data = project[geometry_name], project[data_name]
    records, _quantity, notes = _played(data, picks)
    dofs = [data.response_dof[i] for i in records]
    ordinate = np.asarray(data.ordinate)[records]
    drawn_rows, triangles, lines, drawn = _drawn_rows(geometry)
    envelope = isinstance(data, Psd)
    deflection = (EnvelopeDeflection if envelope else OdsDeflection)(geometry, dofs, ordinate)
    if not len(deflection.rows) or not deflection.num_lines:
        return [], [f'none of {data_name}\'s records lands on {geometry_name}']
    line = deflection.strongest_line
    deflection.line = line
    frequency = float(np.asarray(data.abscissa, dtype=float)[line])
    shape = np.zeros((drawn_rows.size, 3), dtype=complex)
    at = drawn[deflection.rows]
    kept = at >= 0
    if envelope:
        shape[at[kept]] = deflection.offsets(0.0)[kept]
        label = f'Envelope at {frequency:.4g} Hz'
    else:
        shape[at[kept]] = (deflection.offsets(0.0) - 1j * deflection.offsets(np.pi / 2))[kept]
        label = f'ODS at {frequency:.4g} Hz'
    if not np.all(kept):
        notes.append(f'{int((~kept).sum())} moving nodes are not drawn')
    title = f'{data_name} on {geometry_name}'
    stage = v.shape3d(title, 'm' if geometry.units_defined else 'undefined',
                      np.asarray(geometry.node_xyz, dtype=float)[drawn_rows],
                      [v.mode(1, frequency, None, shape, label=label)],
                      triangles=triangles, lines=lines, normalization='unscaled')
    curves = _run(project, 'project.plot', data=data_name, records=_runs(records))
    return [stage, *curves], notes


def _overlaid(project: Any, first: str, second: str) -> dict:
    """Two geometries in one scene, as a test geometry is laid over the
    model it was built from: one mesh of both, the second's nodes after
    the first's, each keeping its own units."""
    views = []
    for name in (first, second):
        geometry = project[name]
        rows, triangles, lines = _mesh(geometry)
        views.append((np.asarray(geometry.node_xyz, dtype=float)[rows], triangles, lines,
                      geometry.units_defined))
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
                     lines=np.vstack(lines) if lines else None)


def _pair(project: Any, first: str, second: str, records: dict[str, list[int]]
          ) -> tuple[list[dict], list[str]] | None:
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
    kinds = {first: _kind(a), second: _kind(b)}

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
                return _pattern(project, geometry, other, picks)
        return None
    if isinstance(a, ShapeSet) and isinstance(b, ShapeSet):
        return _run(project, 'project.mac', first=first, second=second), []
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
    if kinds[first] == kinds[second] and _is_data(a):
        return None
    return None


def _offers(project: Any, objects: list[str]) -> dict[str, bool]:
    """Which of the bar's choices the selection can take: the stage or the
    flat plot for a data object, a component for complex frequency data,
    decades for anything on a frequency axis."""
    data = [project[name] for name in objects if _is_data(project[name])]
    return {
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
            paired = _pair(project, objects[0], objects[1], records)
            if paired is not None:
                views, notes = paired
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
