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
    `verb:` param lists the object under."""
    return [verb for verb, _summary in project.verbs(name)]


def summary(state: OpenProject) -> dict[str, Any]:
    """The project as the shell lists it: title, file, whether it has
    unsaved changes, and each object with its kind, record count, source,
    whether its units are declared, the groups it is in, the core verbs
    that apply to it and the analysis settings it carries."""
    project = state.project
    labels = [group.get('role') or group.get('name') or f'Group {k + 1}'
              for k, group in enumerate(project.object_groups)]
    objects = []
    for name, obj in project.items():
        entry: dict[str, Any] = {'name': name, 'kind': kind_of(obj)}
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
        settings = settings_of(obj)
        if settings:
            entry['settings'] = settings
        objects.append(entry)
    return {'title': project.name, 'file': project.file,
            'modified': state.modified, 'objects': objects,
            'active_geometry': project.active_geometry
            if project.active_geometry in project else None}


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


def import_file(state: OpenProject | None, params: Mapping[str, Any],
                progress: Callable[[int, int], None] | None = None
                ) -> tuple[OpenProject, list[str]]:
    """`project.import`: the file's objects added to the open project, or
    to a new one when none is open; the project and the names added.
    `units` declares what the file does not state, as `import_file`'s
    options (`length_unit`, `ordinate_unit` and the rest)."""
    import hertzforge as hz

    path = _path(params, 'project.import')
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
    (`Project.rename`)."""
    state = _open(state, 'project.rename')
    old = _name(params, 'name', 'project.rename')
    new = _name(params, 'to', 'project.rename').strip()
    if old not in state.project:
        raise not_found(f'the project has no object named {old!r}')
    if new == old:
        return
    try:
        state.project.rename(old, new)
    except ValueError as error:
        raise refused(str(error)) from None
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
    missing = [name for name in names if name not in state.project]
    if missing:
        raise not_found(f'the project has no object named '
                        f'{", ".join(map(repr, missing))}')
    state.project.remove(*names)
    state.modified = True


def units(state: OpenProject | None, params: Mapping[str, Any]) -> dict[str, Any]:
    """`project.units`: the tables an object's units are declared in
    (`service.units.tables`)."""
    from . import units as declared

    state = _open(state, 'project.units')
    return declared.tables(state.project, _name(params, 'name', 'project.units'))


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
