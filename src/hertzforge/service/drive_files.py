"""`drive.*`: Playback's drive files, by path, as the analysis service's requests.

A drive file reaches the service by its path on the computer that runs the
test, as a project does; nothing is uploaded. `drive.inspect` says what a file,
or a set of the open project's time histories, holds; `drive.convert` writes the
chosen signals as a new native drive folder; `drive.fix_ends` writes a corrected
copy of one. The forms, the checks and the refusals are
`hertzforge.io.drive_convert`'s. These requests only read and write files: the
analysis service never drives a station and never arms a test.
"""

from __future__ import annotations

import math
import os
from collections.abc import Mapping
from typing import Any

from hertzforge.io import drive_convert

from .descriptors import json_type
from .protocol import bad_request, not_found, refused

#: the requests on drive files
DRIVE_METHODS = ('drive.inspect', 'drive.convert', 'drive.fix_ends')

#: the params each takes
PARAMS = {'drive.inspect': ('path', 'objects'),
          'drive.convert': ('path', 'objects', 'selections', 'rate_hz', 'out', 'fix_ends',
                            'taper_s'),
          'drive.fix_ends': ('folder', 'taper_s', 'out')}


def needs_project(params: Any) -> bool:
    """Whether a request reads the connection's open project (it names
    `objects`)."""
    return isinstance(params, Mapping) and params.get('objects') is not None


def request(method: str, params: Any, project: Any = None) -> dict[str, Any]:
    """Answer one `drive.*` request, run on a worker thread. `project` is
    the connection's open project, or None."""
    if not isinstance(params, Mapping):
        raise bad_request(f'{method} "params" must be an object, not {json_type(params)}')
    extra = sorted(set(params) - set(PARAMS[method]))
    if extra:
        raise bad_request(f'{method} takes {", ".join(map(repr, PARAMS[method]))}; it has no '
                          f'{", ".join(map(repr, extra))}')
    try:
        if method == 'drive.fix_ends':
            return drive_convert.fix_ends(_path(params, 'folder', method),
                                          taper_s=_seconds(params, method),
                                          out=_optional_path(params, 'out', method))
        source, project_file = _source(params, method, project)
        if method == 'drive.inspect':
            return drive_convert.inspect(source, project_file).as_dict()
        selections = params.get('selections')
        if not isinstance(selections, list) or not selections:
            raise bad_request('drive.convert needs "selections", the signals in drive order: '
                              'each a key drive.inspect gave, or {"signal", "name"}')
        fix = params.get('fix_ends', False)
        if not isinstance(fix, bool):
            raise bad_request('drive.convert "fix_ends" is true or false')
        return drive_convert.convert(
            source, selections, rate_hz=_rate(params), out=_optional_path(params, 'out', method),
            fix_ends=fix, taper_s=_seconds(params, method),
            project_file=project_file).as_dict()
    except FileNotFoundError as error:
        raise not_found(str(error)) from None
    except (ValueError, TypeError, OSError) as error:
        raise refused(str(error)) from None


def _source(params: Mapping[str, Any], method: str, project: Any) -> tuple[Any, str | None]:
    """A file's path, or the open project's objects `objects` names."""
    has_path, has_objects = params.get('path') is not None, params.get('objects') is not None
    if has_path == has_objects:
        raise bad_request(f'{method} needs "path", a file\'s path on the analysis service\'s '
                          'machine, or "objects", names in the open project, not both')
    if has_path:
        return _path(params, 'path', method), None
    names = params['objects']
    if (not isinstance(names, list) or not names
            or not all(isinstance(n, str) and n for n in names)):
        raise bad_request(f'{method} "objects" is a list of object names')
    if project is None:
        raise refused(f'{method}: no project is open, so it has no objects')
    missing = [n for n in names if n not in project]
    if missing:
        raise not_found(f'the project has no object {missing[0]!r}')
    return {n: project[n] for n in names}, getattr(project, 'file', None) or None


def _path(params: Mapping[str, Any], key: str, method: str) -> str:
    path = params.get(key)
    if not isinstance(path, str) or not path.strip():
        raise bad_request(f'{method} needs "{key}", its path on the analysis service\'s '
                          'machine')
    return os.path.abspath(os.path.expanduser(path.strip()))


def _optional_path(params: Mapping[str, Any], key: str, method: str) -> str | None:
    return None if params.get(key) is None else _path(params, key, method)


def _number(value: Any, what: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(
            value):
        raise bad_request(f'{what} is a number, not {json_type(value)}')
    return float(value)


def _rate(params: Mapping[str, Any]) -> float | None:
    value = params.get('rate_hz')
    return None if value is None else _number(value, 'drive.convert "rate_hz"')


def _seconds(params: Mapping[str, Any], method: str) -> float | None:
    value = params.get('taper_s')
    return None if value is None else _number(value, f'{method} "taper_s"')
