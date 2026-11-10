"""Table files by path, as `tables.save` and `tables.load` requests.

The shell's channel table and breakpoint tables are saved to and opened from
files through the analysis service, as projects are: a file reaches the
service by its path on the service's machine, and nothing is uploaded. The
forms and the reading rules are `hertzforge.io.tables`'. These requests do not
touch the connection's project.
"""

from __future__ import annotations

import os
from collections.abc import Mapping
from typing import Any

from hertzforge.io import tables

from .descriptors import json_type
from .protocol import bad_request, not_found, refused

#: the requests on table files
TABLE_METHODS = ('tables.save', 'tables.load')

#: the params each takes
PARAMS = {'tables.save': ('kind', 'table_kind', 'columns', 'rows', 'path', 'x_unit', 'y_unit'),
          'tables.load': ('path', 'kind', 'table_kind', 'x_unit', 'y_unit')}


def request(method: str, params: Any) -> dict[str, Any]:
    """Answer one `tables.*` request, run on a worker thread."""
    if not isinstance(params, Mapping):
        raise bad_request(f'{method} "params" must be an object, not {json_type(params)}')
    extra = sorted(set(params) - set(PARAMS[method]))
    if extra:
        raise bad_request(f'{method} takes {", ".join(map(repr, PARAMS[method]))}; it has no '
                          f'{", ".join(map(repr, extra))}')
    path = _path(params, method)
    kind = params.get('kind')
    if kind not in tables.KINDS:
        raise bad_request(f'{method} "kind" is one of {", ".join(tables.KINDS)}')
    table_kind = _optional_text(params, 'table_kind', method)
    if kind == 'breakpoints' and method == 'tables.save' and not table_kind:
        raise bad_request('tables.save of a breakpoint table needs "table_kind"')
    try:
        if method == 'tables.save':
            return {'path': tables.write(path, _table(params, kind, table_kind))}
        table = tables.read(path, kind, table_kind, *_units(params, kind, method))
    except tables.TableContentError as error:
        raise bad_request(f'{method}: {error}') from None
    except FileNotFoundError as error:
        raise not_found(str(error)) from None
    except (tables.TableFileError, OSError) as error:
        raise refused(str(error)) from None
    out: dict[str, Any] = {'kind': table.kind, 'table_kind': table.table_kind,
                           'columns': table.columns, 'rows': table.rows,
                           'warnings': table.warnings}
    if table.kind == 'breakpoints':
        out.update(x_unit=table.x_unit, y_unit=table.y_unit)
    return out


def _path(params: Mapping[str, Any], method: str) -> str:
    path = params.get('path')
    if not isinstance(path, str) or not path.strip():
        raise bad_request(f'{method} needs "path", the file\'s path on the analysis '
                          'service\'s machine')
    return os.path.abspath(os.path.expanduser(path.strip()))


def _optional_text(params: Mapping[str, Any], key: str, method: str) -> str | None:
    value = params.get(key)
    if value is not None and not isinstance(value, str):
        raise bad_request(f'{method} "{key}" is text, not {json_type(value)}')
    return value or None


def _units(params: Mapping[str, Any], kind: str, method: str) -> tuple[str | None, str | None]:
    """A request's `x_unit` and `y_unit`, refused on a channel table."""
    x_unit = _optional_text(params, 'x_unit', method)
    y_unit = _optional_text(params, 'y_unit', method)
    if kind == 'channels' and (x_unit is not None or y_unit is not None):
        raise bad_request(f'{method}: a channel table has no x_unit or y_unit')
    return (params.get('x_unit') if 'x_unit' in params else None,
            params.get('y_unit') if 'y_unit' in params else None)


def _table(params: Mapping[str, Any], kind: str, table_kind: str | None) -> tables.Table:
    columns, rows = params.get('columns'), params.get('rows')
    if not isinstance(columns, list) or not all(isinstance(c, str) for c in columns):
        raise bad_request('tables.save "columns" is a list of column names')
    if not isinstance(rows, list) or not all(isinstance(r, list) for r in rows):
        raise bad_request('tables.save "rows" is a list of rows, each a list of values')
    return tables.Table(kind, columns, rows, table_kind, *_units(params, kind, 'tables.save'))
