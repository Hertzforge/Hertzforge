"""What a selection in the project tree shows.

The window draws whatever is selected in its tree: one
object by its kind, records picked inside one, or several together when
the combination means something. `project.show` answers that from the
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
two PSDs overlay. Data objects on one abscissa (frequency, time) are drawn
together to be compared, the records picked in each when any are. Anything
else shows each object on its own, and the note says what was left out.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .mesh import drawn_mesh
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


#: how the window reads data, kept across selections: the stage or the flat plot, the
#: component of complex data, and what is drawn with it (a specification's limits, a
#: table's arrows and sensors). The axes' scales are chosen on the plot, not here.
PREFS = {'flat': False, 'component': 'magnitude', 'line': None,
         'projection': False, 'shape_view': None, 'limits': True,
         'values_dofs': 'all', 'values_form': 'shapes', 'arrows': False, 'arrow_role': 'all',
         'drive': None,
         'sensors': True,
         'averaging': None,
         'environment': None, 'table_view': 'channels', 'tone': None, 'sine_reading': 'curves'}

TABLE_VIEWS = ('channels', 'response', 'reference')


def _table_views(table: Any, prefs: dict[str, Any]) -> tuple[list[str], str | None, list[str], str]:
    """(environments, the one picked, the views it holds, the one picked)
    for a channel table: the picked
    environment while it still applies, else the first; the transformation
    views only when the loop holds them."""
    loops = list(table.environments)
    environment = prefs.get('environment')
    if environment not in loops:
        environment = loops[0] if loops else None
    forms = ['channels']
    if environment is not None:
        held = table.environments[environment]
        if held.response_transformation is not None:
            forms.append('response')
        if held.reference_transformation is not None:
            forms.append('reference')
    form = prefs.get('table_view') if prefs.get('table_view') in forms else 'channels'
    return loops, environment, forms, form

#: past this many shape values the table is not offered
SHAPE_VALUES_CELL_LIMIT = 1_000_000

#: a lone shape set's own DOFs are marked with arrows up to this many: a
#: measured set has a few dozen, a finite element set hundreds of thousands,
#: and an arrow on every node of a mesh says nothing
ARROW_DOF_LIMIT = 2_000

#: where a shape set's values are read: every DOF, or a channel choice of the
#: table in its group
VALUES_CHOICES = {'all': 'All DOFs', 'control': 'Control channels', 'reference': 'References',
                  'response': 'Responses', 'monitor': 'Monitors'}

VALUES_FORMS = ('shapes', 'response', 'reference')


def _table_on(project: Any, geometry_name: str) -> tuple[str, Any] | None:
    """(name, table) of the first channel table drawn on a geometry: the
    one in its object group, else the one the active geometry serves."""
    from hertzforge.core.channel_table import ChannelTable

    for name, obj in project.items():
        if isinstance(obj, ChannelTable):
            found = project.geometry_for(name)
            if found is not None and found[0] == geometry_name:
                return name, obj
    return None


def _home(project: Any, views: list[dict], objects: list[str]) -> None:
    """The stage geometry's own view (`Geometry.view`) as `home` on every `shape3d` the
    selection shows, so a model built with Y up opens upright; none when it has no view."""
    stages = [view for view in views if view.get('view') == 'shape3d' and 'home' not in view]
    name = _stage_geometry(project, objects) if stages else None
    own = getattr(project[name], 'view', None) if name is not None else None
    if own is None:
        return
    home = {'eye': [float(v) for v in own.eye], 'up': [float(v) for v in own.up]}
    for view in stages:
        view['home'] = home


def _stage_geometry(project: Any, objects: list[str]) -> str | None:
    """The geometry the selection's stage is drawn on: the geometry
    selected, else the first object's."""
    from hertzforge.core.geometry import Geometry

    for name in objects:
        if isinstance(project[name], Geometry):
            return name
    return _geometry_for(project, objects[0]) if objects else None


def _values_view(project: Any, name: str, shapes: Any, prefs: dict[str, Any]
                 ) -> tuple[dict | None, list[str]]:
    """A lone shape set's values table at the DOFs and in the form the
    bar's two drop-downs say (`core.tables.shape_values` through
    `Project._shape_dofs`), or None and why."""
    from hertzforge.core.tables import shape_values

    choice, form = prefs['values_dofs'], prefs['values_form']
    try:
        dofs = project._shape_dofs(name, None if choice == 'all' else choice)
        headers, rows = shape_values(shapes, dofs, form)
    except ValueError as why:
        return None, [f'shape values: {why}']
    title = {'shapes': f'{name}, shapes',
             'response': f'{name}, response transformation',
             'reference': f'{name}, reference transformation'}[form]
    notes: list[str] = []
    if dofs is not None:
        held = set(shapes.coordinate)
        missing = [dof for dof in dofs if dof not in held]
        notes.append(f'{len(dofs)} {VALUES_CHOICES.get(choice, choice).lower()}'
                     + (f', {len(missing)} not in the set' if missing else ''))
    return v.table(title, headers, rows), notes


def _marked_table(project: Any, objects: list[str], geometry_name: str
                  ) -> tuple[tuple[str, Any] | None, bool]:
    """(the channel table whose channels are marked on the stage, whether it was picked
    with the geometry): a table selected beside the geometry, linked with it or not, whose
    arrows are then always drawn; else the table drawn on the geometry (`_table_on`)."""
    from hertzforge.core.channel_table import ChannelTable

    if len(objects) == 2 and geometry_name in objects:
        other = objects[1] if objects[0] == geometry_name else objects[0]
        if isinstance(project[other], ChannelTable):
            return (other, project[other]), True
    return _table_on(project, geometry_name), False


def _arrow_role(table: Any, prefs: dict[str, Any]) -> str:
    """The channel choice the arrows are drawn for: `prefs.arrow_role` while the table
    holds such channels, else every channel."""
    role = prefs.get('arrow_role', 'all')
    return role if role in table.choices() else 'all'


def _picked_channels(objects: list[str], records: dict[str, list[int]],
                     table: tuple[str, Any] | None) -> list[int] | None:
    """The rows of the channels picked inside a selected channel table, or None when
    the table is whole: only their arrows are drawn."""
    if table is None or table[0] not in objects:
        return None
    rows = sorted({int(i) for i in records.get(table[0], [])
                   if 0 <= int(i) < table[1].num_channels})
    return rows or None


def _arrow_dofs(project: Any, objects: list[str], table: tuple[str, Any] | None,
                role: str = 'all', only: list[int] | None = None
                ) -> tuple[list[str], list[bool]]:
    """The DOFs the arrows mark and whether each points in: the channel
    table's channels in `role` (`ChannelTable.chosen`), or the rows `only` when channels
    are picked inside it, a force or a drive pointing in,
    else a lone shape set's DOFs when it has no more than `ARROW_DOF_LIMIT` of them."""
    from hertzforge.core.report import EXCITATION_QUANTITIES
    from hertzforge.core.shapes import ShapeSet

    if table is not None:
        _name, obj = table
        strings, types = obj.dof_strings(), obj.types()
        rows = [i for i in (obj.chosen(role) if only is None else only)
                if str(obj['node'][i]).strip() and str(obj['direction'][i]).strip()]
        return [strings[i] for i in rows], [types[i] in EXCITATION_QUANTITIES for i in rows]
    if len(objects) == 1 and isinstance(project[objects[0]], ShapeSet):
        dofs = project[objects[0]].coordinate
        if len(dofs) <= ARROW_DOF_LIMIT:
            return [str(dof) for dof in dofs], []
    return [], []


def _marks(project: Any, views: list[dict], objects: list[str], prefs: dict[str, Any],
           records: dict[str, list[int]] | None = None) -> list[str]:
    """DOF arrows and sensors over every stage the selection shows, in the
    geometry's coordinates: the arrows as solids that stay put (`arrows` on the
    `shape3d`) as `project.dof_arrows` draws them, the sensors as solids
    that move with their nodes (`bodies`) as `project.sensors` draws
    them. A channel table picked with a geometry always has its arrows drawn, for
    the channels `prefs.arrow_role` chooses, or for the channels picked inside the table
    when some are. Returns the notes."""
    stages = [view for view in views if view.get('view') == 'shape3d']
    geometry_name = _stage_geometry(project, objects) if stages else None
    if geometry_name is None:
        return []
    table, picked = _marked_table(project, objects, geometry_name)
    if not (prefs['arrows'] or picked or prefs['sensors']):
        return []
    from hertzforge.drawing.viz.geometry import (
        dof_arrow_length,
        dof_arrow_solids,
        dof_arrow_vectors,
    )
    from hertzforge.drawing.viz.sensors import rows_of, sensor_bodies

    from .ops_geometry import _extent

    geometry = project[geometry_name]
    xyz = np.asarray(geometry.node_xyz, dtype=float).reshape(-1, 3)
    notes: list[str] = []
    if prefs['arrows'] or picked:
        only = _picked_channels(objects, records or {}, table)
        role = _arrow_role(table[1], prefs) if table is not None and only is None else 'all'
        dofs, into = _arrow_dofs(project, objects, table, role, only)
        into = into or [False] * len(dofs)
        if dofs or picked:
            hits = dof_arrow_vectors(geometry, dofs)
            drawn = [(i, hit) for i, hit in zip(into, hits, strict=True) if hit is not None]
            labels = [dof for dof, hit in zip(dofs, hits, strict=True) if hit is not None]
            if drawn:
                length = dof_arrow_length(xyz[[hit[0] for _i, hit in drawn]], _extent(xyz))
                for stage in stages:
                    # each its own, since converting a stage's units writes into it; a
                    # stage that draws a force of its own keeps that arrow alone
                    if 'arrows' not in stage:
                        stage['arrows'] = dof_arrow_solids(xyz, drawn, length, labels)
            said = f'{len(drawn)} DOF arrow{"s" * (len(drawn) != 1)}'
            if only is not None:
                said += f' ({len(only)} channel{"s" * (len(only) != 1)} picked)'
            elif role != 'all':
                said += f' ({VALUES_CHOICES[role].lower()})'
            if len(drawn) < len(dofs):
                said += f', {len(dofs) - len(drawn)} with no node or direction on {geometry_name}'
            notes.append(said)
    if prefs['sensors'] and table is not None:
        try:
            placed, _said = project.sensors(table[0])
        except (TypeError, ValueError):
            placed = []
        if placed:
            for stage in stages:
                ids = stage.get('node_ids')
                stage['bodies'] = sensor_bodies(
                    placed, geometry, rows_of(geometry.node_id if ids is None else ids))[0]
            notes.append(f'{len(placed)} sensors')
    return notes


#: what `prefs.averaging` may hold: the Averaging form's fields
AVERAGING_FIELDS = ('frame_length', 'overlap', 'window', 'frames', 'start', 'detrend')


def averaging_drawn(project: Any, name: str, draft: dict[str, Any]
                    ) -> tuple[list[dict] | None, str]:
    """(regions, note) for an averaging drawn on the time history `name`: what
    `draft` holds of the Averaging form's fields, the history's own for the rest
    (`ops_settings.averaging_regions`: each frame with its window and its row,
    then the span that drags them). No regions, and why, when it cannot be drawn."""
    from hertzforge.core.averaging import Averaging

    from .ops_settings import averaging_regions

    history = project[name]
    held = history.averaging
    stored = {} if held is None else {field: getattr(held, field) for field in AVERAGING_FIELDS}
    fields = {'frame_length': 4096, 'overlap': 0.5, 'window': 'hann', 'frames': 1,
              'start': 0.0, 'detrend': 'none', **stored, **draft}
    try:
        averaging = Averaging(int(fields['frame_length']), float(fields['overlap']),
                              fields['window'], int(fields['frames']), float(fields['start']),
                              fields['detrend'], start_set=True)
        rate = float(history.sample_rate)
    except (TypeError, ValueError) as error:
        return None, f'averaging not drawn: {error}'
    most = averaging.most_frames(len(history.abscissa), rate)
    said = (f'{averaging.frames} frame{"s" * (averaging.frames != 1)} of '
            f'{averaging.frame_length / rate:g} s')
    if averaging.frames > most:
        said += f'; only {most} fit from {averaging.start:g} s'
    return averaging_regions(name, history, averaging), said


def _averaging(project: Any, views: list[dict], objects: list[str], prefs: dict[str, Any]
               ) -> list[str]:
    """The averaging drawn on a lone time history's own plot, the stage or
    the flat one, while its form is open: `prefs.averaging` holds what the
    form holds, typed or stored, and the frames come as `regions`
    (`averaging_drawn`). Returns the notes."""
    from hertzforge.core.data import TimeHistory

    draft = prefs.get('averaging')
    if draft is None or len(objects) != 1 or not isinstance(project[objects[0]], TimeHistory):
        return []
    regions, said = averaging_drawn(project, objects[0], draft)
    if regions is not None:
        for view in views:
            if view.get('view') in ('spectrum', 'waterfall'):
                view['regions'] = [dict(region) for region in regions]
    return [said]


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
        # a lone set's views, one at a time, its verb in the action table
        # choosing: on its geometry the modes animate, unless the shell asks
        # for the mode list, the auto-MAC or the values. The auto-MAC brings
        # the stage its squares animate, which the shell draws once one is
        # picked. Modes picked in the tree limit every view to those modes.
        from .ops_project import _shape_view, mac_views

        prefs = {**PREFS, **(prefs or {})}
        geometry = _geometry_for(project, name)
        picked = sorted({int(i) for i in picks if 0 <= int(i) < obj.num_shapes})
        count = int(obj.num_shapes) * len(obj.coordinate)
        notes = [f'mode {picked[0] + 1} picked' if len(picked) == 1 else
                 f'{len(picked)} modes picked'] if picked else []
        frame = prefs['shape_view'] or 'animate'
        if frame == 'animate' and geometry is None:
            frame = 'table'
        if frame == 'values' and count > SHAPE_VALUES_CELL_LIMIT:
            notes.append(f'{count:,} shape values, too many for a table')
            frame = 'table'
        views: list[dict] = []
        if frame == 'animate':
            views.append(_shape_view(project, name, geometry, picked or None))
        if frame == 'mac':
            paired = mac_views(project, name, name, picked or None, picked or None)
            selected = next((view for view in paired if view['view'] == 'shape3d'), None)
            if selected is not None:
                # a square picked animates its row mode with its column mode overlaid;
                # the shell leaves the overlay off on the diagonal
                stage = _shape_view(project, name, geometry, picked or None)
                stage['overlays'] = selected['overlays']
                stage['overlay_phase'] = selected['overlay_phase']
                views.append(stage)
            views += [view for view in paired if view['view'] == 'matrix']
        if frame == 'table':
            (table,) = _run(project, 'project.table', object=name)
            if picked and len(table['rows']) == obj.num_shapes:
                table['rows'] = [table['rows'][k] for k in picked]
                if 'edit' in table:
                    table['edit']['rows'] = [k + 1 for k in picked]
            views.append(table)
        if frame == 'values':
            table, said = _values_view(project, name, obj, prefs)
            notes += said
            if table is not None:
                if picked and len(table['columns']) == obj.num_shapes + 1:
                    keep = [0] + [k + 1 for k in picked]
                    table['columns'] = [table['columns'][i] for i in keep]
                    table['rows'] = [[row[i] for i in keep] for row in table['rows']]
                elif picked and len(table['rows']) == obj.num_shapes:
                    table['rows'] = [table['rows'][k] for k in picked]
                views.append(table)
        return views, notes
    if isinstance(obj, ChannelTable):
        # the table itself, typed into, with the Control column of the
        # picked loop, or one of that loop's transformations in its place
        from hertzforge.core.tables import transformation_table

        from .ops_sheets import channel_table

        prefs = {**PREFS, **(prefs or {})}
        _loops, environment, _forms, form = _table_views(obj, prefs)
        notes = [f'{len(picks)} channels picked'] if picks else []
        if form == 'channels':
            return [channel_table(project, name, environment=environment)], notes
        headers, rows = transformation_table(obj, environment, form)
        return [v.table(f'{name}: {environment}, {form} transformation', headers, rows)], notes
    if isinstance(obj, MatchedModes):
        # the pairs overlaid on the first set's geometry, each second-set mode over its
        # first-set mode, and the table of pairs beside them (Brandon, 2026-11-14); pairs
        # picked in the tree are the ones drawn and listed
        from .mode_pairs import matched_stage

        picked = sorted({int(i) for i in picks if 0 <= int(i) < len(obj.pairs)})
        table = _run(project, 'project.table', object=name)
        if picked and table and len(table[0]['rows']) == len(obj.pairs):
            table[0]['rows'] = [table[0]['rows'][k] for k in picked]
            if 'edit' in table[0]:
                table[0]['edit']['rows'] = [k + 1 for k in picked]
        notes = [f'pair {picked[0] + 1} picked' if len(picked) == 1 else
                 f'{len(picked)} pairs picked'] if picked else []
        pairs = [obj.pairs[k] for k in picked] if picked else list(obj.pairs)
        sets = [project.get(obj.first), project.get(obj.second)]
        if not all(isinstance(found, ShapeSet) for found in sets) or not pairs:
            return table, notes
        geometry = _geometry_for(project, obj.first)
        if geometry is None:
            return table, [*notes, f'{obj.first} is on no geometry, so the pairs are not drawn']
        # a second set on a geometry of its own is drawn there, laid over the first's
        other = project.geometry_for(obj.second)
        apart = other is not None and other[1] is not project[geometry]
        try:
            compared = project.compared_sets(obj.first, obj.second)[:2] if apart else None
        except ValueError:
            compared = None
        stage = matched_stage(project[geometry], f'{obj.first} and {obj.second} matched',
                              sets[0], sets[1], pairs, obj.second,
                              column_geometry=other[1] if apart else None, compared=compared)
        return [stage, *table], notes
    from hertzforge.core.photos import Photos
    from hertzforge.core.sine import SineLevelSet, SineSweepSpecification

    if isinstance(obj, Photos):
        # every photo, or the picked ones, stacked
        shown = [i for i in sorted(set(picks)) if i < obj.num_photos] or range(obj.num_photos)
        views = [v.image(obj.names[i], obj.formats[i], obj.images[i]) for i in shown]
        count = len(views)
        return views, [f'{count} photo{"s" * (count != 1)}'] if picks else []
    if isinstance(obj, SineSweepSpecification):
        return _sine(project, name, None, {name: picks}, {**PREFS, **(prefs or {})})
    if isinstance(obj, SineLevelSet):
        return _sine(project, None, name, {name: picks}, {**PREFS, **(prefs or {})})
    if _is_data(obj):
        prefs = {**PREFS, **(prefs or {})}
        component = prefs['component']
        if picks and prefs['flat']:
            return _run(project, 'project.plot', data=name, records=_runs(picks),
                        component=component, limits=bool(prefs['limits'])), []
        if picks:
            from .waterfalls import waterfall_view

            # the sub-items picked on the stage, as the whole object is: 3D is the choice
            # the bar shows, picks or none (Brandon, 2026-11-14)
            view = waterfall_view(name, obj, component=component,
                                  records=sorted(set(picks)))
            _faced([view], obj)
            return [view], []
        if prefs['flat']:
            # the flat plot draws every record
            return _run(project, 'project.plot', data=name,
                        records=_runs(list(range(int(obj.num_records)))),
                        component=component, limits=bool(prefs['limits'])), []
        views = _run(project, 'project.waterfall', data=name, component=component)
        _faced(views, obj)
        return views, []
    from hertzforge.core.element_shapes import ElementShapeSet
    from hertzforge.core.report import Report

    if isinstance(obj, ElementShapeSet):
        # on its model, colored by its stress's largest component until Color
        # By picks another; the modes picked in the tree are the ones drawn
        from .ops_element_shapes import element_view

        if _geometry_for(project, name) is None:
            return [], [f'{name} has no geometry to be drawn on: link it with its model']
        picked = sorted({int(i) for i in picks if 0 <= int(i) < obj.num_shapes})
        notes = [f'mode {picked[0] + 1} picked' if len(picked) == 1 else
                 f'{len(picked)} modes picked'] if picked else []
        return [element_view(project, name, picked or None)], notes
    if isinstance(obj, Report):
        blocks = len(getattr(obj, 'blocks', ()) or ())
        return [v.reading(name, [
            v.row('Report', getattr(obj, 'title', '') or name, None, 'info'),
            v.row('Blocks', float(blocks), '', 'info'),
            v.row('Edited in', 'the Report workspace', None, 'info')])], []
    return [], [f'{name} ({_kind(obj).replace("_", " ")}) has no view of its own here yet']


def _sine(project: Any, spec_name: str | None, levels_name: str | None,
          records: dict[str, list[int]], prefs: dict[str, Any]) -> tuple[list[dict], list[str]]:
    """A sine specification, extracted levels, or both together: one tone at a time (the
    bar's drop-down,
    `prefs['tone']`), the levels at each control channel over the tone's
    requirement and limits, and with both objects the Level Error reading
    (`prefs['sine_reading']` 'bars'): each channel's signed RMS deviation
    in dB. The rows under either object are its control DOFs, and picks on
    them restrict the reading. Without `prefs['flat']` the reading is the
    stage: frequency across, time receding, amplitude up."""
    from hertzforge.core.compliance import SINE_ERROR_DB

    from .ops_sine_shock import sine_dofs, sine_error_rows, sine_lines

    spec = project[spec_name] if spec_name else None
    level_set = project[levels_name] if levels_name else None
    tones = [tone.name for tone in spec.tones] if spec is not None else list(level_set.tone_names)
    if not tones:
        return [], [f'{spec_name or levels_name} holds no tone']
    tone = prefs.get('tone') if prefs.get('tone') in tones else tones[0]
    picked: list[str] = []
    for name, obj in ((spec_name, spec), (levels_name, level_set)):
        if obj is None:
            continue
        dofs = sine_dofs(obj)
        picked += [dofs[i] for i in sorted(set(records.get(name, []))) if i < len(dofs)
                   and dofs[i] not in picked]
    dofs = picked or None
    levels = list(level_set.levels) if level_set is not None else []
    if spec is not None:
        levels = [level for level in levels if level.tone in tones]
    notes: list[str] = []
    if level_set is not None and spec is not None and not levels:
        notes.append(f'{levels_name} holds no tone of {spec_name} ({", ".join(tones)})')
    where = f' at {", ".join(dofs)}' if dofs else ''
    if not prefs.get('flat'):
        # The default reading of a sweep: the stage, frequency across, time
        # receding, amplitude up; the bar's Flat is the per-channel plot
        from .sine_stage import sine_stage

        if spec is not None and levels:
            shown = [level for level in levels if level.tone == tone]
            what = (f'{tone}{where}: {len(shown)} level{"s" * (len(shown) != 1)} against the '
                    f'requirement of {spec_name}')
        elif spec is not None:
            what = f'{tone}{where}: the requirement of {spec_name}'
        else:
            what = f'{tone}{where}: the extracted level'
        stage = sine_stage(spec, levels, tone, dofs, title=what)
        if stage is None:
            return [], notes + [f'nothing to draw for {tone}{where}']
        if prefs.get('sine_reading') == 'bars' and spec is not None and levels:
            try:
                rows = sine_error_rows(spec, levels, SINE_ERROR_DB, dofs)
            except ProtocolError as error:
                return [stage], notes + [str(error)]
            return [stage, v.reading(f'Level Error of {levels_name} against {spec_name}', rows)], notes
        return [stage], notes
    if spec is not None and levels and prefs.get('sine_reading') == 'bars':
        try:
            rows = sine_error_rows(spec, levels, SINE_ERROR_DB, dofs)
        except ProtocolError as error:
            return [], [str(error)]
        return [v.reading(f'Level Error of {levels_name} against {spec_name}', rows)], notes
    lines, unit = sine_lines(spec, tone, levels, dofs)
    if not lines:
        return [], notes + [f'nothing to draw for {tone}{where}']
    if spec is None:
        what = f'{tone}{where}: the extracted level'
    elif levels:
        shown = [level for level in levels if level.tone == tone]
        what = (f'{tone}{where}: {len(shown)} level{"s" * (len(shown) != 1)} against the '
                f'requirement of {spec_name}')
    else:
        what = f'{tone}{where}: the requirement of {spec_name}'
    view = v.spectrum(what, v.axis('Frequency', 'Hz', 'log'), v.axis('Amplitude', unit, 'log'),
                      lines, link='frequency')
    return [view], notes


def _sine_pair(project: Any, first: str, second: str) -> tuple[str, str] | None:
    """(specification, level set) when the two are one of each, else None."""
    from hertzforge.core.sine import SineLevelSet, SineSweepSpecification

    a, b = project[first], project[second]
    if isinstance(a, SineSweepSpecification) and isinstance(b, SineLevelSet):
        return first, second
    if isinstance(b, SineSweepSpecification) and isinstance(a, SineLevelSet):
        return second, first
    return None


def _faced(views: list[dict], obj: Any) -> None:
    """A coherence opens on the map, the stage seen from above with the
    channels down and frequency across; the bar's
    Curves is the perspective."""
    from hertzforge.core.data import Coherence, MultipleCoherence

    if isinstance(obj, (Coherence, MultipleCoherence)):
        for view in views:
            if view.get('view') == 'waterfall':
                view['facing'] = 'top'


#: how many records a cross-object view draws before standing down: two whole
#: surveys together are thousands of curves, and each object is then shown alone
STACK_LIMIT = 500


def _picked_records(data: Any, picks: list[int]) -> list[int]:
    """The records of `data` picked in the tree, or all of them when none is."""
    picked = sorted({int(i) for i in picks if 0 <= int(i) < int(data.num_records)})
    return picked or list(range(int(data.num_records)))


def _picked_data(project: Any, names: list[str], records: dict[str, list[int]]) -> bool:
    """Whether records are picked in a selection of several objects, all of them data:
    what `_together` compares."""
    return (len(names) > 1 and all(_is_data(project[name]) for name in names)
            and any(records.get(name) for name in names))


def _together(project: Any, names: list[str], records: dict[str, list[int]],
              prefs: dict[str, Any]) -> tuple[list[dict], list[str]] | None:
    """Data objects on one abscissa drawn together to be compared: the
    records picked in each (all of an object picked whole) on one stage, or
    on one flat plot with `prefs['flat']`, each curve named by its object.
    The objects need not be one type or share their abscissa values, only
    its quantity (frequency, time). An object whose values do not share the
    first's axis (another quantity or unit) is left out, and the note says
    so. Nothing is merged or stored. None when fewer than two of the objects
    can be drawn this way, or they hold more than `STACK_LIMIT` records
    together, and the caller shows each on its own."""
    import hertzforge as hz
    from hertzforge.drawing.viz.waterfall import drawn_span, waterfall_arrays

    from .ops_project import _curve, _limit_lines
    from .waterfalls import axes

    objects = [project[name] for name in names]
    if len(objects) < 2 or not all(_is_data(obj) for obj in objects):
        return None
    if len({obj.abscissa_dim for obj in objects}) != 1:
        return None
    chosen = [_picked_records(obj, records.get(name, [])) for name, obj in zip(names, objects,
                                                                                 strict=True)]
    if sum(len(picks) for picks in chosen) > STACK_LIMIT:
        return None
    component, flat = prefs['component'], bool(prefs['flat'])
    first: Any = None
    x_axis: dict = {}
    z_axis: dict = {}
    z_labels: list[str] = []
    drawn_from: list[str] = []
    apart: list[str] = []
    other = paged = 0
    lines: list[dict] = []
    labels: list[str] = []
    curves: list[tuple[Any, Any]] = []
    for name, data, picks in zip(names, objects, chosen, strict=True):
        arrays = waterfall_arrays(data, picks, unit_system=hz.SI, exponents=False,
                                  component=component)
        if not arrays['drawn']:
            apart.append(name)
            continue
        x, z = axes(data, arrays['drawn'][0], component, arrays)
        if first is None:
            first, x_axis, z_axis = data, x, z
        elif (x['label'], x['unit'], z['unit']) != (x_axis['label'], x_axis['unit'],
                                                    z_axis['unit']):
            apart.append(name)
            continue
        drawn_from.append(name)
        if z['label'] not in z_labels:
            z_labels.append(z['label'])
        other += len(arrays['left_out'])
        paged += len(picks) - len(arrays['left_out']) - len(arrays['drawn'])
        for record, (cx, cz) in zip(arrays['drawn'], arrays['curves'], strict=True):
            label = f'{name}: {data.record_label(record)}'
            if flat:
                lines.append(_curve(label, 'measured', data, cx, cz, component))
            else:
                labels.append(label)
                curves.append((cx, cz))
        if flat and prefs['limits'] and component == 'magnitude':
            lines += [{**line, 'name': f'{name}: {line["name"]}'}
                      for line in _limit_lines(data, arrays['drawn'])]
    if len(drawn_from) < 2:
        return None
    z_axis = {**z_axis, 'label': ' / '.join(z_labels)}
    title = ' + '.join(dict.fromkeys(drawn_from))
    notes = [f'{len(drawn_from)} objects on one {"plot" if flat else "stage"}']
    if apart:
        notes.append(f'{", ".join(apart)} left out: not on the axis of {drawn_from[0]}')
    if other:
        notes.append(f'{other} record{"s" * (other != 1)} of another quantity left out')
    if paged:
        notes.append(f'{paged} record{"s" * (paged != 1)} past the first page left out')
    if flat:
        return [v.spectrum(title, x_axis, z_axis, lines)], notes
    limits = getattr(first, 'ordinate_limits', None)
    signed = component != 'magnitude' and any(obj.ordinate.dtype.kind == 'c' for obj in objects)
    view = v.waterfall(title, x_axis, z_axis, labels, curves, drawn_span(curves),
                       z_range=None if limits is None or z_axis['scale'] == 'log' or signed
                       else limits)
    _faced([view], first)
    return [view], notes

def _column(data: Any, picks: list[int]) -> list[int] | None:
    """The picked records as one CPSD reference column, or None: a column
    against one reference carries each response's magnitude and phase
    relative to it, an honest complex shape; a whole CPSD has no
    reference chosen and the diagonal is the autos."""
    from hertzforge.core.data import autospectrum, channel_quantities

    if len(picks) < 2 or data.reference_dof is None or not np.iscomplexobj(data.ordinate):
        return None
    quantities = [channel_quantities(data.known_dim(i)) for i in picks]
    references = {(data.reference_dof[i], quantities[k][1]) for k, i in enumerate(picks)}
    if len(references) != 1:
        return None
    if all(autospectrum(data, i) for i in picks):
        return None
    kinds = [response for response, _reference in quantities]
    wanted = max(set(kinds), key=kinds.count)
    return [i for i, kind in zip(picks, kinds, strict=True) if kind == wanted]


def _matched(project: Any, first: str, second: str) -> tuple[str, Any] | None:
    """The Matched Modes object between these two sets, (name, object), or
    None."""
    from hertzforge.core.matches import MatchedModes

    return next(((name, obj) for name, obj in project.items()
                 if isinstance(obj, MatchedModes) and obj.first == first
                 and obj.second == second), None)


def _none_matched(first: str, second: str, homes: Any, error: Any
                  ) -> tuple[list[dict], list[str]]:
    """Two sets whose geometries share no node within the tolerance: how far off they
    are, said in the window and not refused. The tolerance is set beside what is shown,
    and a refusal shows nothing to set it beside."""
    from .ops_modal_plan import _report_rows

    both = homes[0][1].units_defined and homes[1][1].units_defined
    return ([v.reading(f'No node of {first} and {second} matched',
                       _report_rows(error.report, both))],
            [f'{error}. Project sets the node match tolerance'])


def _mac(project: Any, first: str, second: str, geometry: str | None,
         prefs: dict[str, Any], records: dict[str, list[int]] | None = None
         ) -> tuple[list[dict], list[str]]:
    """Two shape sets: their MAC, of the modes picked under each (every mode
    of a set picked whole), each axis ticked by its own frequencies,
    and on a geometry the stage the matrix selects a pair on, the column
    mode overlaid on the row mode (`mode_pairs.paired`). Sets on two
    geometries compare after projection onto the first's DOFs
    (`Project.comparison_mac`), and `prefs['projection']` animates the
    projection on the basis geometry instead of both sets by DOF name.
    Pairs already committed to a Matched Modes object ride on the matrix
    as `matched`."""
    from hertzforge.core.correlate import NoNodeMatched

    from .mode_pairs import matched_cells, mode_axis, paired, picked_modes

    records = records or {}
    rows, columns = project[first], project[second]
    if not rows.num_shapes or not columns.num_shapes:
        raise refused(f'{first if not rows.num_shapes else second} has no modes')
    row_modes = picked_modes(rows, records.get(first))
    column_modes = picked_modes(columns, records.get(second))
    homes = project.geometry_for(first), project.geometry_for(second)
    apart = all(home is not None for home in homes) and homes[0][1] is not homes[1][1]
    notes: list[str] = []
    # how the pair is projected, and how far a node may be from its match, are Project's
    # (`Project.set_projection`): the MAC is drawn the way Project sets it
    if apart and prefs.get('projection'):
        try:
            views = _run(project, 'project.projection_overlay', first=first, second=second,
                         first_modes=_runs(records.get(first) or []),
                         second_modes=_runs(records.get(second) or []))
        except ProtocolError as error:
            if not isinstance(error.__cause__, NoNodeMatched):
                raise
            # nothing to animate: answered as the comparison answers it, with how far off
            return _mac(project, first, second, geometry, {**prefs, 'projection': False},
                        records)
    else:
        from hertzforge.core.shapes import cross_mac

        try:
            # the two sets as they are compared: one projected onto the other's DOFs when
            # they are on two geometries, for the MAC and for each pair's alignment
            ours, theirs, said, report = ((rows, columns, '', None) if first == second
                                          else project.compared_sets(first, second))
            values = rows.auto_mac() if first == second else cross_mac(ours, theirs)
        except NoNodeMatched as error:
            return _none_matched(first, second, homes, error)
        except ValueError as error:
            raise refused(str(error)) from None
        # the whole matrix is computed and the picked rows and columns read from it: a
        # pair's MAC does not depend on which other modes are compared
        values = np.asarray(values)[np.ix_(row_modes, column_modes)]
        title = (f'Auto-MAC of {first}' if first == second
                 else f'MAC of {first} Against {second}')
        matrix = v.matrix(title, mode_axis(first, rows, row_modes),
                          mode_axis(second, columns, column_modes), values, 'MAC', (0.0, 1.0))
        if apart:
            # each set on its own geometry, the second's laid over the first's
            views = paired(homes[0][1], matrix, rows, row_modes, columns, column_modes,
                           column_geometry=homes[1][1], compared=(ours, theirs))
            notes.append(f'the MAC with {said}; {first} drawn on '
                         f'{homes[0][0]} and {second} on {homes[1][0]}, Projection animates '
                         'the projection')
            if report is not None:
                # the projection behind the MAC is made whether or not it is animated: what
                # it matched is shown with it, as beside the Projection view
                from .ops_modal_plan import _report_rows
                views.append(v.reading(said, _report_rows(
                    report, homes[0][1].units_defined and homes[1][1].units_defined)))
        else:
            views = paired(None if geometry is None else project[geometry], matrix, rows,
                           row_modes, columns, column_modes)
    for name, shapes, modes in ((first, rows, row_modes), (second, columns, column_modes)):
        if len(modes) < shapes.num_shapes and not (name == second and first == second):
            notes.append(f'{len(modes)} of the {shapes.num_shapes} modes of {name}')
    found = _matched(project, first, second)
    if found is not None:
        views[0]['matched'] = matched_cells(found[1].pairs, row_modes, column_modes)
        notes.append(f'{len(found[1].pairs)} pairs matched in {found[0]}')
    return views, notes


def _drawn_rows(geometry: Any) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    """The mesh as the shape view draws it, and each geometry node's drawn
    index (-1 when it is not drawn)."""
    drawn_rows, triangles, lines = drawn_mesh(geometry)
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


def _three_played(project: Any, objects: list[str], records: dict[str, list[int]]
                  ) -> tuple[list[dict], list[str]] | None:
    """A geometry, a shape set and modal coordinates selected together: the coordinates
    played through that set, with the elements of an element shape set selected as well
    colored by the von Mises stress of their tensor summed each frame, on the clip's largest
    value (Play Strain/Stress chooses otherwise). None for any other three or four."""
    from hertzforge.core.data import TimeHistory
    from hertzforge.core.element_shapes import ElementShapeSet
    from hertzforge.core.geometry import Geometry
    from hertzforge.core.shapes import ShapeSet

    def only(cls: Any) -> str | None:
        found = [name for name in objects if isinstance(project[name], cls)]
        return found[0] if len(found) == 1 else None

    geometry, shapes, history = only(Geometry), only(ShapeSet), only(TimeHistory)
    if not (geometry and shapes and history):
        return None
    coloring = None
    if len(objects) == 4:
        held = only(ElementShapeSet)
        if held is None:
            return None
        coloring = element_coloring(held)
    return modal_motion_of(project, geometry, history, records.get(history, []), shapes,
                           coloring)


def element_coloring(name: str, quantity: str = 'stress', component: str = 'von_mises',
                     groups: str = '', color_max: float = 0.0) -> dict[str, Any]:
    """How a modal motion colors the elements of the element shape set `name`: by
    `component` of its `quantity` (`ops_element_shapes.color_scalar`), the elements of
    the element groups `groups` (all for blank), on a scale up to `color_max` in SI (the
    clip's largest value for 0)."""
    return {'name': name, 'quantity': quantity, 'component': component, 'groups': groups,
            'color_max': float(color_max)}


def modal_motion_of(project: Any, geometry_name: str, data_name: str, picks: list[int],
                    shapes_name: str | None = None, coloring: dict[str, Any] | None = None
                    ) -> tuple[list[dict], list[str]] | None:
    """`_modal_motion`, with the elements colored as `coloring` (`element_coloring`) says, and
    the geometry's own view as its camera, as `show` gives it."""
    played = _modal_motion(project, geometry_name, data_name, picks, shapes_name, coloring)
    if played is not None:
        _home(project, played[0], [geometry_name])
    return played


def _modal_motion(project: Any, geometry_name: str, data_name: str, picks: list[int],
                  shapes_name: str | None = None, coloring: dict[str, Any] | None = None
                  ) -> tuple[list[dict], list[str]] | None:
    """Modal coordinates played on a geometry through their shape set: the set selected
    with them, or the one they came from
    (`Project.modal_basis`). Each frame is the modes' shapes scaled by their values at that
    sample and summed, by the shell, so the physical response is never formed
    (`deform.modal_deflection`). With `coloring` (`element_coloring`), an element shape
    set's elements are colored the same way: each mode's tensor is sent once, and the shell
    sums them and forms the scalar each frame (`element_values`). None when the records are
    not modal coordinates."""
    from hertzforge.core.validate import modal_coordinate
    from hertzforge.deform import modal_deflection

    from .ops_element_shapes import with_rows
    from .waterfalls import unit_text

    geometry, data = project[geometry_name], project[data_name]
    records, quantity, notes = _played(data, picks)
    modal = [i for i in records if modal_coordinate(data.response_dof[i]) is not None]
    if not modal:
        return None
    basis = shapes_name or project.modal_basis(data_name, modal)
    if basis is None:
        said = (f'{data_name}: its records are modal coordinates; select their shape set too, '
                'to play them')
        return [], [said]
    try:
        deflection = modal_deflection(geometry, project[basis], data, modal)
    except ValueError as error:
        return [], [f'{data_name}: {error}']
    played = [i for i in modal if modal_coordinate(data.response_dof[i]) <= project[basis].num_shapes]
    values = np.asarray(data.ordinate)[played].real
    # the modal factors cancel: a mass-normalized shape times its coordinate is the motion
    dim = data.known_dim(played[0]).removeprefix('modal_')
    drawn_rows, triangles, lines, drawn = _drawn_rows(geometry)
    field = None
    if coloring is not None:
        try:
            field = _element_field(project, geometry, basis, data_name, played, dim, values,
                                   coloring)
        except ValueError as error:
            return [], [f'{coloring["name"]}: {error}']
        drawn_rows, triangles, lines, drawn = with_rows(
            drawn_rows, triangles, lines, field['triangles'], geometry.num_nodes)
        field['triangles'] = np.ascontiguousarray(drawn[field['triangles']], np.uint32)
    shown = drawn[deflection.rows] >= 0
    if not shown.any():
        return [], [f'{basis} has no DOF on {geometry_name}']
    fields = deflection.fields.reshape(len(played), -1, 3)[:, shown, :]
    abscissa = np.asarray(data.abscissa, dtype=float)
    title = f'{data_name} on {geometry_name}'
    plot = v.spectrum(title, v.axis('Time', 's', 'linear'),
                      v.axis(f'Modal {quantity}', unit_text(data.known_dim(played[0])) or None,
                             'linear'),
                      [v.series(data.record_label(i), 'measured', abscissa, values[d])
                       for d, i in enumerate(played)], link='time')
    stage = v.shape3d(
        title, 'm' if geometry.units_defined else 'undefined',
        np.asarray(geometry.node_xyz, dtype=float)[drawn_rows], [], triangles=triangles,
        lines=lines, node_ids=np.asarray(geometry.node_id)[drawn_rows], link='time',
        motion={'label': 'Time', 'unit': 's', 'start': float(abscissa[0]),
                'step': float(abscissa[1] - abscissa[0]) if abscissa.size > 1 else 0.0,
                'quantity': dim.capitalize(), 'value_unit': unit_text(dim) or '',
                'node': drawn[deflection.rows[shown]].astype(np.uint32),
                'direction': np.empty((0, 3), dtype=np.float32),
                'modes': np.ascontiguousarray(fields, np.float32),
                'values': np.ascontiguousarray(values, np.float32),
                'peak': deflection.peak_magnitude})
    notes.append(f'{len(played)} mode shapes of {basis} scaled by the record and summed')
    if field is not None:
        stage['element_values'] = field
        scale = 'stated' if field.get('stated') else "the clip's largest"
        notes.append(f'{field["elements"]:,} elements of {coloring["name"]} colored by '
                     f'{field["label"][0].lower()}{field["label"][1:]}, summed over the modes '
                     f'at each instant, on {scale} value')
        if not field.get('stated') and not field['max'][0] > 0:
            notes.append(f'{data_name} never strains these elements: they are drawn at zero')
    return [stage, plot], notes


def _element_field(project: Any, geometry: Any, basis: str, data_name: str, played: list[int],
                   dim: str, values: Any, coloring: dict[str, Any]) -> dict:
    """The `element_values` of a modal motion (`ops_element_shapes.element_field`): the
    element shape set's tensors for the modes the records play, in their order, scaled by the
    clip. ValueError saying why the set cannot color this clip."""
    from hertzforge.core.validate import modal_coordinate

    from .ops_element_shapes import colored_elements, element_field, played_rows

    name = coloring['name']
    held = project[name]
    if dim not in ('length', 'displacement'):
        raise ValueError(f'strain and stress follow modal displacement, and {data_name} holds '
                         f'modal {dim or "values of no stated quantity"}')
    data = project[data_name]
    sources = [modal_coordinate(data.response_dof[i]) - 1 for i in played]
    rows = played_rows(project, name, basis, sources)
    mask = colored_elements(geometry, name, held, coloring.get('groups', ''))
    return element_field(geometry, name, held, rows, coloring['quantity'],
                         coloring['component'], mask, coloring.get('color_max', 0.0), values)


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


def _swept(deflection: Any) -> np.ndarray:
    """A swept pattern as one complex shape, nodes × 3: its offsets at phase zero and a
    quarter turn on. `offsets` answers in one buffer it writes again on every call, so
    the first answer is copied before the second is asked for; read in one expression,
    both were the quarter-turn offsets and the pattern at phase zero was lost."""
    at_zero = deflection.offsets(0.0).copy()
    return at_zero - 1j * deflection.offsets(np.pi / 2)


def _modal_frf_records(data: Any, picks: list[int]) -> list[int] | None:
    """The records of modal FRFs a selection reads, every one when none is picked; None
    when `data` is not an FRF or a record read is not a mode against a mode."""
    from hertzforge.core.data import Frf
    from hertzforge.core.validate import modal_coordinate

    if not isinstance(data, Frf) or data.reference_dof is None:
        return None
    records = list(picks) or list(range(int(data.num_records)))
    if not records or any(modal_coordinate(data.response_dof[i]) is None
                          or modal_coordinate(data.reference_dof[i]) is None for i in records):
        return None
    return records


def _linked_geometry(project: Any, name: str) -> str | None:
    """The geometry an object is linked to (the one in its object group), by name."""
    from hertzforge.core.geometry import Geometry

    for member in project.object_group_of(name) or ():
        if isinstance(project.get(member), Geometry):
            return member
    return None


def _driven_selection(project: Any, objects: list[str], records: dict[str, list[int]]
                      ) -> tuple[str, str, str, list[int]] | None:
    """(geometry, modal FRFs, channel table, the records read) when the selection is one
    geometry, modal FRFs and one channel table, with or without the shape set the modal
    FRFs are of: what shows the response to a force at the table's force reference. None
    for anything else."""
    from hertzforge.core.channel_table import ChannelTable
    from hertzforge.core.data import Frf
    from hertzforge.core.geometry import Geometry
    from hertzforge.core.shapes import ShapeSet

    def only(cls: Any) -> str | None:
        found = [name for name in objects if isinstance(project[name], cls)]
        return found[0] if len(found) == 1 else None

    geometry, modal, table = only(Geometry), only(Frf), only(ChannelTable)
    if not (geometry and modal and table) or len(objects) not in (3, 4):
        return None
    read = _modal_frf_records(project[modal], records.get(modal, []))
    if read is None:
        return None
    if len(objects) == 4:
        shapes = only(ShapeSet)
        if shapes is None or shapes != project.modal_basis(modal, read):
            return None
    return geometry, modal, table, read


def _driven(project: Any, objects: list[str], records: dict[str, list[int]],
            prefs: dict[str, Any]) -> tuple[list[dict], list[str]] | None:
    """Modal FRFs, a channel table with a force reference and a geometry: the operating
    deflection shape of a force at that reference. At the line the cursor asks for, each
    mode's shape scaled by its modal FRF and its shape value at the drive, summed
    (`Project.driven_modes`), at every DOF the shape set has on the geometry, so no FRF
    is made for any of them (`deform.ModalOdsDeflection`). The records picked are the
    modes included: without the rigid-body modes, displacement at 0 Hz is the elastic
    static deflection. `prefs.drive` picks among several force references. None for any
    other selection."""
    from hertzforge.deform import modal_ods_deflection
    from hertzforge.drawing.viz.geometry import (
        dof_arrow_length,
        dof_arrow_solids,
        dof_arrow_vectors,
    )

    from .ops_geometry import _extent

    chosen = _driven_selection(project, objects, records)
    if chosen is None:
        return None
    geometry_name, modal, table, read = chosen
    points = project.drive_points(table)
    if not points:
        return [], [(f'{table} has no reference channel measuring force: the response of '
                    f'{modal} to a force needs one')]
    drive = prefs.get('drive') if prefs.get('drive') in points else points[0]
    geometry, data = project[geometry_name], project[modal]
    picked = records.get(modal) or None
    try:
        basis, driven = project.driven_modes(modal, drive, table=table, records=picked)
        deflection = modal_ods_deflection(geometry, project[basis], driven)
    except (TypeError, ValueError) as refusal:
        return [], [f'{modal}: {refusal}']
    if not len(deflection.rows):
        return [], [f'{modal}: {basis} has no DOF on {geometry_name}']
    if deflection.peak == 0.0:
        return [], [f'{modal}: a force at {drive} moves none of its modes']
    abscissa = np.asarray(data.abscissa, dtype=float)
    if prefs.get('line') is not None:
        line = int(np.argmin(np.abs(abscissa - float(prefs['line']))))
    else:
        line = deflection.strongest_line
    deflection.line = line
    frequency = float(abscissa[line])
    drawn_rows, triangles, lines, drawn = _drawn_rows(geometry)
    shape = np.zeros((drawn_rows.size, 3), dtype=complex)
    at = drawn[deflection.rows]
    kept = at >= 0
    shape[at[kept]] = _swept(deflection)[kept]
    label = f'ODS at {frequency:.4g} Hz'
    count = len(driven.modes)
    notes = [(f'the response to a force at {drive}: {count} mode shape{"s" * (count != 1)} of '
             f'{basis} scaled by their modal FRFs and summed')]
    if not deflection.line_defined:
        # displacement at 0 Hz with a rigid-body mode among the modes is unbounded
        label += ': no shape, a mode is unbounded here'
        notes.append(f'no shape at {frequency:.4g} Hz: a mode is unbounded there')
    if not np.all(kept):
        notes.append(f'{int((~kept).sum())} moving nodes are not drawn')
    title = f'{modal} on {geometry_name}'
    stage = v.shape3d(title, 'm' if geometry.units_defined else 'undefined',
                      np.asarray(geometry.node_xyz, dtype=float)[drawn_rows],
                      [v.mode(1, frequency, None, shape, label=label)],
                      triangles=triangles, lines=lines, normalization='unscaled',
                      node_ids=np.asarray(geometry.node_id)[drawn_rows],
                      cursor='frequency')
    # the one arrow is the force: modal coordinates are at no node. The drive is a DOF of
    # the table's geometry: when the table is linked to one, the shapes were read at its
    # node's place (`Project.channel_shapes`), so the arrow is that node's place and
    # coordinate system, not the node with the same number on the geometry drawn (a table
    # on a sparse test geometry over a finite element model put it at the model's node of
    # that number, or nowhere). It is drawn at the drawn node nearest that place and
    # rides it (`follow`).
    home = _linked_geometry(project, table)
    on = geometry if home is None else project[home]
    hit = dof_arrow_vectors(on, [drive])[0]
    xyz = np.asarray(geometry.node_xyz, dtype=float).reshape(-1, 3)
    if hit is not None and drawn_rows.size:
        place = np.asarray(on.node_xyz, dtype=float).reshape(-1, 3)[hit[0]]
        row = int(np.argmin(np.linalg.norm(xyz[drawn_rows] - place, axis=1)))
        at_node = xyz[drawn_rows][[row]]
        stage['arrows'] = dof_arrow_solids(at_node, [(True, (0, hit[1], hit[2]))],
                                 dof_arrow_length(at_node, _extent(xyz)), [drive])
        stage['arrows']['follow'] = np.array([row], dtype=np.uint32)
    else:
        notes.append(f'{drive} is not a node of {geometry_name if home is None else home}: '
                     'the force is not drawn')
    curves = _run(project, 'project.plot', data=modal, records=_runs(read),
                  component=prefs['component'])
    for curve in curves:
        if curve.get('view') == 'spectrum':
            curve['link'] = 'frequency'
    return [stage, *curves], notes


def _drives(project: Any, objects: list[str], records: dict[str, list[int]] | None
            ) -> list[str]:
    """The force references a driven selection (`_driven`) can be driven at."""
    chosen = _driven_selection(project, objects, records or {})
    return project.drive_points(chosen[2]) if chosen is not None else []


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
    from hertzforge.core.data import Psd, autospectrum
    from hertzforge.deform import EnvelopeDeflection, OdsDeflection

    prefs = {**PREFS, **(prefs or {})}
    geometry, data = project[geometry_name], project[data_name]
    if _modal_frf_records(data, picks) is not None:
        return [], [(f'{data_name}: its records are modal FRFs; select a channel table with a '
                    'force reference too, to see the response to that force')]
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
            records = [i for i in range(int(data.num_records)) if autospectrum(data, i)]
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
        shape[at[kept]] = _swept(deflection)[kept]
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
        rows, triangles, lines = drawn_mesh(geometry)
        views.append((np.asarray(geometry.node_xyz, dtype=float)[rows], triangles, lines,
                      geometry.units_defined))
        ids.append(np.asarray(geometry.node_id)[rows])
    (nodes_a, tri_a, lines_a, defined_a), (nodes_b, tri_b, lines_b, defined_b) = views
    offset = len(nodes_a)
    triangles = [t for t in (tri_a, None if tri_b is None or not len(tri_b) else tri_b + offset)
                 if t is not None and len(t)]
    lines = [t for t in (lines_a, None if lines_b is None or not len(lines_b) else lines_b + offset)
             if t is not None and len(t)]
    from . import scans

    # the nodes neither geometry draws a line or a face through, as points: measured points
    # laid over a scan of the same part, or over the model
    points = [layer for name in (first, second) for layer in scans.layers(project[name])[0]]
    return v.shape3d(f'{second} over {first}',
                     'm' if defined_a and defined_b else 'undefined',
                     np.vstack([nodes_a, nodes_b]), [],
                     triangles=np.vstack(triangles) if triangles else None,
                     lines=np.vstack(lines) if lines else None,
                     node_ids=np.concatenate(ids),
                     **({'points': points} if points else {}))


def _pair(project: Any, first: str, second: str, records: dict[str, list[int]],
          prefs: dict[str, Any] | None = None) -> tuple[list[dict], list[str]] | None:
    """What two objects show together, or None when the pair has no joint
    reading."""
    sine = _sine_pair(project, first, second)
    if sine is not None:
        return _sine(project, sine[0], sine[1], records, {**PREFS, **(prefs or {})})
    from hertzforge.core.channel_table import ChannelTable
    from hertzforge.core.data import (
        Frf,
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
            from hertzforge.project import drawn_elsewhere

            if drawn_elsewhere(project, other, geometry):
                # not where the set's DOFs are: the projection onto it, not a drawing by number
                from .ops_project import projection_views

                return projection_views(project, other, geometry), []
            return _run(project, 'project.mode_shapes', shapes=other, geometry=geometry), []
        if isinstance(obj, ChannelTable):
            # the geometry alone; `_marks` draws the table's arrows over it
            return _run(project, 'project.view_geometry', geometry=geometry), []
        if isinstance(obj, TimeHistory):
            return (_modal_motion(project, geometry, other, picks)
                    or _motion(project, geometry, other, picks))
        if _is_data(obj):
            abscissa = getattr(obj, 'abscissa_dim', '')
            if isinstance(obj, Psd) or (abscissa == 'frequency'
                                        and np.iscomplexobj(obj.ordinate)):
                return _pattern(project, geometry, other, picks, prefs)
        return None
    if isinstance(a, ShapeSet) and isinstance(b, ShapeSet):
        return _mac(project, first, second, _geometry_for(project, first),
                    {**PREFS, **(prefs or {})}, records)
    frf_name, set_name = of(Frf), of(ShapeSet)
    if frf_name is not None and set_name is not None:
        return _resynthesis(project, frf_name, set_name, records, {**PREFS, **(prefs or {})})
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
        try:
            return _run(project, 'project.psd_overlay', first=first, second=second), []
        except ProtocolError:
            # two densities on different lines, or with no channel in common, have no
            # channel-by-channel overlay; they are still drawn together
            pass
    return _together(project, [first, second], records, {**PREFS, **(prefs or {})})


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


def _resynthesis(project: Any, frf_name: str, set_name: str, records: dict[str, list[int]],
                 prefs: dict[str, Any]) -> tuple[list[dict], list[str]]:
    """An FRF beside a shape set: the modal model's prediction of the
    picked records (the first few when none are picked), each synthesis
    following its measurement dashed in its colour
    (`core.shapes.synthesize_overlay`, the Resynthesis reading); picked
    modes limit the synthesis to them. Edit Fit is the act beside it."""
    import hertzforge as hz
    from hertzforge.core.shapes import synthesize_overlay
    from hertzforge.drawing.viz.waterfall import waterfall_arrays

    from .ops_project import FIRST_RECORDS
    from .waterfalls import axes

    frf, shapes = project[frf_name], project[set_name]
    picks = records.get(frf_name, []) or list(range(min(int(frf.num_records), FIRST_RECORDS)))
    modes = records.get(set_name) or None
    try:
        synth = synthesize_overlay(shapes, frf, records=picks, modes=modes)
    except ValueError as why:
        raise refused(str(why)) from None
    if synth is None:
        raise refused(f'no picked record of {frf_name} is at a DOF {set_name} has')
    component = prefs['component']
    arrays = waterfall_arrays(frf, picks, unit_system=hz.SI, exponents=False,
                              component=component)
    lines = []
    for record, (x, y) in zip(arrays['drawn'], arrays['curves'], strict=True):
        kept = np.isfinite(x) & np.isfinite(y)
        lines.append(v.series(frf.record_label(record), 'measured', x[kept], y[kept]))
    pairs = {(frf.response_dof[r], frf.reference_dof[r]): k
             for k, r in enumerate(arrays['drawn'])}
    synth_arrays = waterfall_arrays(synth, list(range(int(synth.num_records))),
                                    unit_system=hz.SI, exponents=False, component=component)
    followed = 0
    for record, (x, y) in zip(synth_arrays['drawn'], synth_arrays['curves'], strict=True):
        k = pairs.get((synth.response_dof[record], synth.reference_dof[record]))
        if k is None:
            continue
        kept = np.isfinite(x) & np.isfinite(y)
        lines.append(v.series(f'{lines[k]["name"]}, synthesized', 'measured', x[kept], y[kept],
                              follows=k))
        followed += 1
    view = v.spectrum(f'{frf_name} with {set_name} resynthesized',
                      *axes(frf, arrays['drawn'][0], component, arrays), lines)
    used = shapes.num_shapes if modes is None else len(modes)
    note = (f'{followed} of {len(arrays["drawn"])} records resynthesized from {used} '
            f'mode{"s" * (used != 1)}')
    return [view], [note]


def _offers(project: Any, objects: list[str], prefs: dict[str, Any],
            records: dict[str, list[int]] | None = None) -> dict[str, Any]:
    """Which of the bar's choices the selection can take: the stage or the
    flat plot for a data object, a component for complex frequency data.
    A frequency axis is made linear or log on the plot itself."""
    from hertzforge.core.shapes import ShapeSet

    data = [project[name] for name in objects if _is_data(project[name])]
    sets = [name for name in objects if isinstance(project[name], ShapeSet)]
    homes = [project.geometry_for(name) for name in sets]
    apart = (len(sets) == 2 and all(home is not None for home in homes)
             and homes[0][1] is not homes[1][1])
    alone = len(objects) == 1 and len(sets) == 1
    drives = _drives(project, objects, records)
    choices = ['all']
    if alone:
        found = project.channel_table_for(sets[0])
        if found is not None:
            choices += [c for c in found[1].choices() if c in VALUES_CHOICES and c != 'all']
    from hertzforge.core.channel_table import ChannelTable

    loops: list[str] = []
    forms: list[str] = []
    if len(objects) == 1 and isinstance(project[objects[0]], ChannelTable):
        loops, _environment, forms, _form = _table_views(project[objects[0]], prefs)
    stage = _stage_geometry(project, objects)
    table, picked = _marked_table(project, objects, stage) if stage is not None else (None, False)
    marked = stage is not None and bool(_arrow_dofs(project, objects, table)[0])
    placed: list = []
    if table is not None:
        try:
            placed = project.sensors(table[0])[0]
        except (TypeError, ValueError):
            placed = []
    from hertzforge.core.sine import SineLevelSet, SineSweepSpecification

    specs = [project[name] for name in objects if isinstance(project[name], SineSweepSpecification)]
    level_sets = [project[name] for name in objects if isinstance(project[name], SineLevelSet)]
    sine = (len(objects) == len(specs) + len(level_sets) and len(specs) <= 1
            and len(level_sets) <= 1 and objects)
    if sine:
        data = [*specs, *level_sets]
    tones: list[str] = []
    if sine:
        tones = ([tone.name for tone in specs[0].tones] if specs
                 else list(level_sets[0].tone_names))
    return {
        'tones': tones,
        'sine_readings': bool(sine and specs and level_sets and any(
            level.tone in tones for level in level_sets[0].levels)),
        'shapes': alone,
        'values_dofs': choices,
        'environments': loops,
        'table_views': forms,
        # modal FRFs driven at a force draw that force alone
        'arrows': marked and not picked and not drives,
        # the force references modal FRFs can be driven at (`prefs.drive`)
        'drives': drives,
        # channels picked inside the table are the choice, so no role is offered
        'arrow_roles': (table[1].choices() if table is not None and (marked or picked)
                        and not drives
                        and _picked_channels(objects, records or {}, table) is None else []),
        'sensors': bool(placed),
        'projection': apart,
        'flat': bool(data),
        # an object with warning and abort limits to draw on the 2-D plot, or leave off
        'limits': any(bool(getattr(obj, 'limits', None)) for obj in data),
        'complex': any(getattr(obj, 'abscissa_dim', '') == 'frequency'
                       and np.iscomplexobj(obj.ordinate) for obj in data),
    }


def show(project: Any, objects: list[str], records: dict[str, list[int]],
         part: dict[str, Any] | None, prefs: dict[str, Any] | None = None) -> dict[str, Any]:
    """What `objects` show together: `{'title', 'views', 'note', 'offers'}`,
    read as `prefs` ask (`PREFS`)."""
    prefs = {**PREFS, **(prefs or {})}
    notes: list[str] = []
    views: list[dict] = []
    picked_data = _picked_data(project, objects, records)
    try:
        if picked_data:
            # records picked across data objects are what is compared: no reading of the
            # whole objects (an overlay of every shared channel, a check against a
            # specification) stands in for them
            views, notes = _together(project, objects, records, prefs) or ([], [])
        elif len(objects) == 2:
            paired = _pair(project, objects[0], objects[1], records, prefs)
            if paired is not None:
                views, notes = paired
        elif len(objects) in (3, 4) and (played := _three_played(project, objects, records)):
            views, notes = played
        elif (driven := _driven(project, objects, records, prefs)) is not None:
            views, notes = driven
        elif len(objects) == 3:
            compared = _compared(project, objects, prefs)
            if compared is not None:
                views, notes = compared
        if len(objects) > 2 and not views and not picked_data:
            together = _together(project, objects, records, prefs)
            if together is not None:
                views, notes = together
        if not views:
            for name in objects:
                more, said = _alone(project, name, records.get(name, []), prefs)
                views += more
                notes += said
            if len(objects) > 1 and views:
                notes.append('each object on its own')
    except ProtocolError as error:
        raise refused(str(error)) from None
    notes += _marks(project, views, objects, prefs, records)
    notes += _averaging(project, views, objects, prefs)
    _home(project, views, objects)
    if part and part.get('kind') and len(objects) == 1:
        from .ops_geometry import part_entities
        from .ops_solids import part_solids
        from .verbs import part_key, picked_ids

        held = project[objects[0]]
        entities = part_entities(objects[0], held, part) if hasattr(held, 'elem_group') else None
        if entities is None and part_key(part) in ('solids', 'solid'):
            moved = [int(i) for i in picked_ids(part)] if part_key(part) == 'solid' else None
            entities = part_solids(objects[0], held, moved)
        if entities is not None:
            views = entities
        else:
            notes.append(f'{part["kind"]} {part.get("id", "")} picked; the whole geometry is drawn'.strip())
    title = ' and '.join(objects) if len(objects) <= 3 else f'{objects[0]} and {len(objects) - 1} more'
    return {'title': title, 'views': views, 'note': '; '.join(n for n in notes if n),
            'offers': _offers(project, objects, prefs, records)}
