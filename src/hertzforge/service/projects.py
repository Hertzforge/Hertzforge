"""A connection's project: the `.hforge` (or new project) its requests
work on, and the summary the shell draws its tree from
(schema/PROTOCOL.md, Projects).

Each function here runs on a worker thread and raises a ProtocolError
with the core's own words when the core refuses a file.
"""

from __future__ import annotations

import os
import re
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import Any

from hertzforge.project import describe

from . import tree
from .ops_settings import settings_of
from .protocol import ProtocolError, bad_request, not_found, refused

#: what `project.open` reads as a whole project (`Project.open`); anything
#: else is a file to import into one
PROJECT_SUFFIXES = ('.hforge', '.vdyn', '.mat', '.h5', '.hdf5', '.escdf')


@dataclass
class OpenProject:
    """The project a connection has open, and whether it has changes its
    file does not."""
    project: Any
    modified: bool = False


def kind_of(obj: Any) -> str:
    """An object's kind: its class name in lower case with underscores
    (`Frf` 'frf', `TimeHistory` 'time_history', `ShapeSet` 'shape_set')."""
    return re.sub(r'(?<=[a-z0-9])(?=[A-Z])', '_', type(obj).__name__).lower()


def verbs_of(project: Any, name: str) -> list[str]:
    """The core's processing verbs that apply to the object `name`, by the
    table the core offers them by (`Project.verbs`), in its order: what a
    `verb:` param lists the object under. A time history that a linked
    channel table describes also lists `overload_check`, which reads it
    against that table's ranges: Visual Dynamics offers its Overload
    reading there and nowhere else. A shock specification does not list
    `generate_signal`: the one signal it makes is a synthesis, and that
    has an operation of its own, `project.srs_synthesis`."""
    verbs = [verb for verb, _summary in project.verbs(name)]
    kind = kind_of(project[name])
    if kind == 'time_history' and project.channel_table_for(name):
        verbs.append('overload_check')
    if kind == 'shock_specification':
        verbs.remove('generate_signal')
    return verbs


def summary(state: OpenProject) -> dict[str, Any]:
    """The project as the shell lists it: title, file, whether it has
    unsaved changes, and each object with its kind, record count, source,
    whether its units are declared, the groups it is in, the core verbs
    that apply to it, the analysis settings it carries, and why it is
    stale when its source's settings changed after it was computed."""
    project = state.project
    labels = [group.get('role') or group.get('name') or f'Group {k + 1}'
              for k, group in enumerate(project.object_groups)]
    stale = project.stale()
    issues = compatibility_issues(project)
    objects = []
    for name, obj in project.items():
        entry: dict[str, Any] = {'name': name, 'kind': kind_of(obj)}
        if name in issues:
            entry['issue'] = issues[name]
        records = getattr(obj, 'num_records', None)
        if records is not None:
            entry['records'] = int(records)
        source = getattr(obj, 'source', None)
        if source is not None:
            entry['source'] = str(source)
        units = getattr(obj, 'units_defined', None)
        if units is not None:
            entry['units_defined'] = bool(units)
        entry['groups'] = [label for label, group
                           in zip(labels, project.object_groups, strict=True)
                           if name in group['members']]
        entry['verbs'] = verbs_of(project, name)
        if tree.banded(obj):
            entry['banded'] = True
        entry['holds'] = describe(obj)
        settings = settings_of(obj)
        if settings:
            entry['settings'] = settings
        if name in stale:
            entry['stale'] = stale[name]
        objects.append(entry)
    out = {'title': project.name, 'file': project.file,
           'modified': state.modified, 'objects': objects,
           'active_geometry': project.active_geometry
           if project.active_geometry in project else None,
           'links': [{'members': [m for m in group['members'] if m in project],
                      'role': group.get('role'), 'name': group.get('name') or ''}
                     for group in project.object_groups
                     if any(m in project for m in group['members'])]}
    if project.unresolved:
        # held by reference to a file that was not found or has changed:
        # not loaded, and kept for the next save (`Project.unresolved`)
        out['unresolved'] = [
            {'name': name, 'kind': reference.kind, 'path': reference.path,
             'reason': reference.reason}
            for name, reference in project.unresolved.items()]
    return out


def compatibility_issues(project: Any) -> dict[str, dict[str, Any]]:
    """What does not fit, by object: Visual Dynamics' red rows
    (`compatibility.check_compatibility`). An object is judged against the
    geometry of its group, else the active one; a modal object against the
    shape sets of its group. `sub_items` are the records, modes or channels
    at fault, to mark in the grid. Incompatibility warns, it never blocks."""
    from hertzforge.compatibility import check_compatibility

    try:
        report = check_compatibility(dict(project.items()), project.active_geometry,
                                     project.object_groups)
    except Exception:  # noqa: BLE001 - a check that cannot run marks nothing
        return {}
    return {name: {'message': issue.message,
                   'sub_items': [int(i) for i in issue.sub_items]}
            for name, issue in report.issues.items()}


def _path(params: Mapping[str, Any], method: str) -> str:
    path = params.get('path')
    if not isinstance(path, str) or not path:
        raise bad_request(f'{method} needs "path", the file\'s path on the '
                          'analysis service\'s machine')
    return os.path.abspath(os.path.expanduser(path))


def _refusal(error: Exception) -> ProtocolError:
    """The core's refusal of a file as an error response."""
    if isinstance(error, FileNotFoundError):
        return not_found(str(error))
    return refused(str(error))


def new(params: Mapping[str, Any]) -> OpenProject:
    """`project.new`: an empty project named `title`."""
    import hertzforge as hz

    title = params.get('title', 'Project')
    if not isinstance(title, str) or not title.strip():
        raise bad_request('project.new "title" must be a name, not empty')
    return OpenProject(hz.Project(title.strip()))


def open_file(params: Mapping[str, Any]) -> OpenProject:
    """`project.open`: the project a `.hforge`, `.vdyn`, `.mat` or ESCDF
    file holds. Any other file is refused with a pointer to import it."""
    import hertzforge as hz

    path = _path(params, 'project.open')
    if not path.lower().endswith(PROJECT_SUFFIXES):
        raise refused(f'{os.path.basename(path)} is not a project file '
                      f'({", ".join(PROJECT_SUFFIXES)}); import it into a '
                      'project instead')
    if not os.path.exists(path):
        raise not_found(f'no file at {path}')
    try:
        project = hz.Project.open(path)
    except (ValueError, OSError) as error:
        raise _refusal(error) from None
    return OpenProject(project)


#: the pictures `project.import` takes, by suffix (`core.photos.FORMATS`)
PHOTO_FORMATS = ('.png', '.jpg', '.jpeg')


def import_file(state: OpenProject | None, params: Mapping[str, Any],
                progress: Callable[[int, int], None] | None = None
                ) -> tuple[OpenProject, list[str]]:
    """`project.import`: the file's objects added to the open project, or
    to a new one when none is open; the project and the names added.
    `units` declares what the file does not state, as `import_file`'s
    options (`length_unit`, `ordinate_unit` and the rest)."""
    import hertzforge as hz

    path = _path(params, 'project.import')
    if os.path.splitext(path)[1].lower() in PHOTO_FORMATS:
        return _import_photo(state, path)
    units = params.get('units', {})
    if not isinstance(units, Mapping) or not all(
            isinstance(key, str) and isinstance(value, str)
            for key, value in units.items()):
        raise bad_request('project.import "units" must be an object of '
                          'option names to unit strings')
    if state is None:
        state = OpenProject(hz.Project())
    project = state.project
    call = ', '.join([repr(path)] + [f'{key}={value!r}'
                                     for key, value in units.items()])
    try:
        # journaled as the one call a script makes, without the progress
        # callable the service passes
        with project.journal_as(f'project.import_file({call})'):
            added = project.import_file(path, progress=progress, **units)
    except (ValueError, OSError) as error:
        raise _refusal(error) from None
    state.modified = True
    return state, added


def _import_photo(state: OpenProject | None, path: str) -> tuple[OpenProject, list[str]]:
    """A png or jpeg added to the project's Photos object, created on
    first use (`Photos()` is the replayable line; a photo is its bytes, so
    the file is added by its path), as Visual Dynamics takes a dropped
    picture. The Photos object is what was added."""
    import hertzforge as hz
    from hertzforge.core.photos import Photos

    if state is None:
        state = OpenProject(hz.Project())
    project = state.project
    name = next((n for n, obj in project.items() if isinstance(obj, Photos)), None)
    if name is None:
        with project.journal_as(None):
            name = project.add('Photos', Photos())
        project.journal.append(f'project.add({name!r}, Photos())')
    try:
        project[name].add_file(path)
    except (ValueError, OSError) as error:
        raise _refusal(error) from None
    project.journal.append(f'project[{name!r}].add_file({path!r})')
    state.modified = True
    return state, [name]


def save(state: OpenProject | None, params: Mapping[str, Any]) -> str:
    """`project.save`: the project written to `path`, or to the file it
    came from; the path written."""
    if state is None:
        raise refused('there is no project open to save')
    if 'path' in params:
        path = _path(params, 'project.save')
    elif state.project.file and not state.project.file.endswith('.vdyn'):
        path = state.project.file
    else:
        raise bad_request('project.save needs "path": this project has no '
                          'file of its own to save to yet')
    compress = params.get('compress', False)
    if not isinstance(compress, bool):
        raise bad_request('project.save "compress" is true or false')
    try:
        written = state.project.save(path, compress=compress)
    except (ValueError, OSError) as error:
        raise _refusal(error) from None
    state.modified = False
    return written


def _open(state: OpenProject | None, method: str) -> OpenProject:
    if state is None:
        raise refused(f'{method} works on a project, and none is open')
    return state


def _name(params: Mapping[str, Any], key: str, method: str) -> str:
    value = params.get(key)
    if not isinstance(value, str) or not value:
        raise bad_request(f'{method} needs "{key}", an object\'s name')
    return value


def rename(state: OpenProject | None, params: Mapping[str, Any]) -> None:
    """`project.rename`: the object `name` called `to`, and every group,
    provenance record and report block that names it following
    (`Project.rename`); with `index`, one photo of a Photos object
    renamed instead, the report blocks that show it following."""
    state = _open(state, 'project.rename')
    old = _name(params, 'name', 'project.rename')
    new = _name(params, 'to', 'project.rename').strip()
    if old not in state.project:
        raise not_found(f'the project has no object named {old!r}')
    if params.get('index') is not None:
        return _rename_photo(state, old, params['index'], new)
    if new == old:
        return
    try:
        state.project.rename(old, new)
    except ValueError as error:
        raise refused(str(error)) from None
    state.modified = True


def _rename_photo(state: OpenProject, name: str, index: Any, new: str) -> None:
    """One photo renamed (`Photos.rename`), journaled as the call, and the
    report blocks bound to it pointed at the new name, as VD's
    `_apply_photo_name` does."""
    from hertzforge.core.photos import Photos
    from hertzforge.core.report import Report

    photos = state.project[name]
    if not isinstance(photos, Photos):
        raise refused(f'{name} is not a Photos object; only a photo is renamed by index')
    if not isinstance(index, int) or isinstance(index, bool) or index < 0:
        raise bad_request('project.rename "index" is a photo\'s index, from 0')
    if index >= photos.num_photos:
        raise not_found(f'{name} has {photos.num_photos} photos; there is no photo {index}')
    was = photos.names[index]
    if new == was:
        return
    try:
        photos.rename(index, new)
    except ValueError as error:
        raise refused(str(error)) from None
    state.project.record_call(photos, 'rename', index, new)
    for obj in state.project.values():
        if isinstance(obj, Report):
            for block in obj.blocks:
                if (block.get('kind') == 'photo' and block.get('source') == name
                        and block.get('photo') == was):
                    block['photo'] = new
    state.modified = True


#: how each kind of object deletes its sub-items, by the tree's word for them
_DELETERS = (('photos', 'delete_photos', 'photo'), ('shapes', 'delete_modes', 'mode'),
             ('channel_table', 'delete_channels', 'channel'), ('data', 'delete_records', 'record'))


def delete_parts(state: OpenProject | None, params: Mapping[str, Any]) -> None:
    """`project.delete_parts`: the sub-items picked in the tree deleted
    from their objects, `parts` mapping each object's name to the indices
    picked: a data object's records, a shape set's modes, a channel table's
    channels, a Photos object's photos, as Visual Dynamics' Delete takes
    grid picks. Every item of an object is refused (delete the object
    instead), nothing is deleted when any part is refused, and each
    deletion is journaled as the call a script would make."""
    from hertzforge.core.channel_table import ChannelTable
    from hertzforge.core.data import DataArray
    from hertzforge.core.photos import Photos
    from hertzforge.core.shapes import ShapeSet

    state = _open(state, 'project.delete_parts')
    parts = params.get('parts')
    if (not isinstance(parts, Mapping) or not parts or not all(
            isinstance(name, str) and isinstance(picks, list) and picks
            and all(isinstance(i, int) and not isinstance(i, bool) and i >= 0 for i in picks)
            for name, picks in parts.items())):
        raise bad_request('project.delete_parts needs "parts", an object\'s name to the '
                          'indices picked in it')
    kinds = {'photos': Photos, 'shapes': ShapeSet, 'channel_table': ChannelTable,
             'data': DataArray}
    counts = {'photos': 'num_photos', 'shapes': 'num_shapes', 'channel_table': 'num_channels',
              'data': 'num_records'}
    planned = []
    for name, picks in parts.items():
        if name not in state.project:
            raise not_found(f'the project has no object named {name!r}')
        obj = state.project[name]
        found = next(((kind, deleter, word) for kind, deleter, word in _DELETERS
                      if isinstance(obj, kinds[kind])), None)
        if found is None:
            raise refused(f'{name} has no parts to delete; remove the object instead')
        kind, deleter, word = found
        count = int(getattr(obj, counts[kind]))
        chosen = sorted(set(picks))
        if chosen[-1] >= count:
            raise not_found(f'{name} has {count} {word}s; there is no {word} {chosen[-1] + 1}')
        if len(chosen) >= count:
            raise refused(f'that is every {word} of {name}; delete the object instead')
        planned.append((name, obj, deleter, chosen))
    for name, obj, deleter, chosen in planned:
        try:
            getattr(obj, deleter)(chosen)
        except ValueError as error:
            raise refused(f'{name}: {error}') from None
        state.project.record_call(obj, deleter, chosen)
    state.modified = True


def remove(state: OpenProject | None, params: Mapping[str, Any]) -> None:
    """`project.remove`: the objects named taken out of the project and its
    groups (`Project.remove`). A name the project does not have is
    refused before anything is removed."""
    state = _open(state, 'project.remove')
    names = params.get('names')
    if (not isinstance(names, list) or not names
            or not all(isinstance(name, str) for name in names)):
        raise bad_request('project.remove needs "names", a list of object names')
    missing = [name for name in names if name not in state.project
               and name not in state.project.unresolved]
    if missing:
        raise not_found(f'the project has no object named '
                        f'{", ".join(map(repr, missing))}')
    state.project.remove(*names)
    state.modified = True


def _names(params: Mapping[str, Any], method: str, empty: bool = False) -> list[str]:
    names = params.get('names')
    if (not isinstance(names, list) or not (names or empty)
            or not all(isinstance(name, str) for name in names)):
        raise bad_request(f'{method} needs "names", a list of object names')
    return names


def duplicate(state: OpenProject | None, params: Mapping[str, Any]) -> None:
    """`project.duplicate`: a copy of each object named added beside it,
    '<name> copy' (`Project.duplicate`), as Copy and Paste do in Visual
    Dynamics' tree. A copy's arrays are its own; it is in no group."""
    state = _open(state, 'project.duplicate')
    names = _names(params, 'project.duplicate')
    missing = [name for name in names if name not in state.project]
    if missing:
        raise not_found(f'the project has no object named {", ".join(map(repr, missing))}')
    state.project.duplicate(*names)
    state.modified = True


def refresh(state: OpenProject | None, params: Mapping[str, Any]) -> None:
    """`project.refresh`: each derived object named computed again from its
    source in place, under its own name (`Project.refresh`); with no names,
    every stale object, sources first (`Project.refresh_stale`)."""
    state = _open(state, 'project.refresh')
    names = _names(params, 'project.refresh', empty=True)
    try:
        if names:
            for name in names:
                state.project.refresh(name)
        else:
            state.project.refresh_stale()
    except (KeyError, ValueError) as error:
        raise refused(str(error).strip("'")) from None
    state.modified = True


def journal(state: OpenProject | None, params: Mapping[str, Any]) -> dict[str, Any]:
    """`project.journal`: the session as Python, every verb and settings
    change since the project was made or opened as the line that replays
    it (`Project.journal`), for Visual Dynamics' console: read-only, to be
    copied into a script."""
    state = _open(state, 'project.journal')
    return {'lines': [str(line) for line in state.project.journal]}


def units(state: OpenProject | None, params: Mapping[str, Any]) -> dict[str, Any]:
    """`project.units`: the tables an object's units are declared in
    (`service.units.tables`)."""
    from . import units as declared

    state = _open(state, 'project.units')
    return declared.tables(state.project, _name(params, 'name', 'project.units'))


def children(state: OpenProject | None, params: Mapping[str, Any]) -> dict[str, Any]:
    """`project.children`: what an object holds, for the tree to open it
    into (`service.tree.children`)."""
    state = _open(state, 'project.children')
    name = _name(params, 'name', 'project.children')
    if name not in state.project:
        raise not_found(f'no object named {name!r}')
    return tree.children(state.project, name)


def show(state: OpenProject | None, params: Mapping[str, Any]) -> dict[str, Any]:
    """`project.show`: what a selection in the tree shows (`service.show`)."""
    from . import show as showing

    state = _open(state, 'project.show')
    objects = params.get('objects')
    if not isinstance(objects, list) or not objects or not all(isinstance(n, str) for n in objects):
        raise bad_request('project.show needs "objects", the selected objects\' names')
    missing = [n for n in objects if n not in state.project]
    if missing:
        raise not_found(f'no object named {missing[0]!r}')
    records = params.get('records') or {}
    if not isinstance(records, Mapping) or not all(
            isinstance(v, list) and all(isinstance(i, int) and i >= 0 for i in v)
            for v in records.values()):
        raise bad_request('project.show "records" maps an object\'s name to its picked record '
                          'indices')
    part = params.get('part')
    if part is not None and not (isinstance(part, Mapping) and isinstance(part.get('kind'), str)):
        raise bad_request('project.show "part" is {"kind", "id"} or null')
    prefs = params.get('prefs') or {}
    if not isinstance(prefs, Mapping) or set(prefs) - set(showing.PREFS):
        raise bad_request('project.show "prefs" takes ' + ', '.join(showing.PREFS))
    if 'component' in prefs and prefs['component'] not in ('magnitude', 'real', 'imag', 'phase'):
        raise bad_request('project.show "component" is magnitude, real, imag or phase')
    if prefs.get('line') is not None and not isinstance(prefs['line'], (int, float)):
        raise bad_request('project.show "line" is a frequency in Hz, or null')
    if prefs.get('shape_view') not in (None, 'animate', 'table', 'mac', 'values'):
        raise bad_request('project.show "shape_view" is animate, table, mac or values, or null')
    for key in ('projection', 'arrows', 'sensors'):
        if key in prefs and not isinstance(prefs[key], bool):
            raise bad_request(f'project.show "{key}" is true or false')
    from .show import TABLE_VIEWS, VALUES_CHOICES, VALUES_FORMS

    if 'values_dofs' in prefs and prefs['values_dofs'] not in VALUES_CHOICES:
        raise bad_request('project.show "values_dofs" is all, control, reference, response or '
                          'monitor')
    if 'values_form' in prefs and prefs['values_form'] not in VALUES_FORMS:
        raise bad_request('project.show "values_form" is shapes, response or reference')
    if prefs.get('environment') is not None and not isinstance(prefs['environment'], str):
        raise bad_request('project.show "environment" is a name, or null')
    if 'table_view' in prefs and prefs['table_view'] not in TABLE_VIEWS:
        raise bad_request('project.show "table_view" is channels, response or reference')
    if prefs.get('tone') is not None and not isinstance(prefs['tone'], str):
        raise bad_request('project.show "tone" is a tone\'s name, or null')
    if 'sine_reading' in prefs and prefs['sine_reading'] not in ('curves', 'bars'):
        raise bad_request('project.show "sine_reading" is curves or bars')
    return showing.show(state.project, list(objects), {str(k): list(v) for k, v in records.items()},
                        dict(part) if part else None, dict(prefs))


def actions(state: OpenProject | None, params: Mapping[str, Any]) -> dict[str, Any]:
    """`project.actions`: what a selection can take (`service.actions`)."""
    from . import actions as acting

    state = _open(state, 'project.actions')
    objects = params.get('objects', [])
    if not isinstance(objects, list) or not all(isinstance(n, str) for n in objects):
        raise bad_request('project.actions needs "objects", the selected objects\' names')
    missing = [n for n in objects if n not in state.project]
    if missing:
        raise not_found(f'no object named {missing[0]!r}')
    records = params.get('records') or {}
    if not isinstance(records, Mapping):
        raise bad_request('project.actions "records" maps an object\'s name to its picked records')
    part = params.get('part')
    if part is not None and not isinstance(part, Mapping):
        raise bad_request('project.actions "part" names a part of a geometry in the tree')
    return acting.actions(state.project, list(objects),
                          {str(k): [int(i) for i in v] for k, v in records.items()}, part)


def define_units(state: OpenProject | None, params: Mapping[str, Any]) -> list[int]:
    """`project.define_units`: units declared on an object by table and
    row (`service.units.define`); the records converted."""
    from . import units as declared

    state = _open(state, 'project.define_units')
    name = _name(params, 'name', 'project.define_units')
    with state.project.journal_as(None):
        converted = declared.define(state.project, name, params.get('units'))
    state.modified = True
    return converted


def activate(state: OpenProject | None, params: Mapping[str, Any]) -> None:
    """`project.activate`: the geometry data is drawn on when nothing else
    names one."""
    from hertzforge.core.geometry import Geometry

    state = _open(state, 'project.activate')
    name = _name(params, 'geometry', 'project.activate')
    if name not in state.project:
        raise not_found(f'the project has no object named {name!r}')
    if not isinstance(state.project[name], Geometry):
        raise refused(f'{name} is a {kind_of(state.project[name]).replace("_", " ")}, '
                      'not a geometry')
    if state.project.active_geometry != name:
        state.project.active_geometry = name
        state.modified = True
