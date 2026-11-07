"""Save a report on its own, as a template another project can use.

A report is a list of blocks with symbolic bindings, such as
`@basis:Frf` or `@other:ShapeSet`, so a report written without its
project is a template: the same figures and text, bound again to
whatever project loads it. The file is JSON with one marker key and the
fields the project file keeps for a report (title, marking, marking
color, blocks, and the report features when any are enabled; see
`native.save_report`), under its own suffix, `.hftemplate`. A lab's house
template therefore carries its choice of report features, and the
laboratory's name, address and reproduction statement from the
report's identification; never a report number, revision, status or
issue record. A block whose binding does not resolve in the new
project is an unbound card there, to be pointed at an object in the
editor's pane, as an unbound block of a built-in template is.

Templates saved in `templates_folder()` are offered by Generate Report
beside the built-in ones in every project. Older templates, under
`OLDER_SUFFIX` with `OLDER_MARKER`, are read the same way and never written.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:                                    # pragma: no cover
    from ..core.report import Report

# Brandon, 2026-09-08: "Is there a way for the user to save a report
# template or load a saved report template?"

SUFFIX = '.hftemplate'
MARKER = 'hertzforge_report_template'
OLDER_SUFFIX = '.vdreport'
OLDER_MARKER = 'visualdynamics_report_template'

#: what a block keeps that belongs to one test, not to the template:
#: the test dates of `report.calibration` (`START`, `END`)
_TEST_ONLY = ('test_date', 'test_end')


def handles(obj: Any) -> bool:
    """True for a Report, which is written as a template: its blocks and
    bindings, no values."""
    from ..core.report import Report

    return isinstance(obj, Report)


def is_template_path(path: str | os.PathLike) -> bool:
    """True for a path ending in `SUFFIX` or `OLDER_SUFFIX`."""
    return str(path).endswith((SUFFIX, OLDER_SUFFIX))


def sniff(path: str | os.PathLike) -> bool:
    """True for a path ending in a template suffix, or a `.json` file whose
    first 4096 characters hold a template marker.

    A template suffix is claimed without being opened, so a damaged one reaches
    `load` and is refused with a reason rather than passed over. Other tools
    write `.json`, so that suffix is opened and checked.
    """
    path = str(path)
    if is_template_path(path):
        return True
    if not path.endswith('.json'):
        return False
    try:
        with open(path, encoding='utf-8') as handle:
            head = handle.read(4096)
        return MARKER in head or OLDER_MARKER in head
    except OSError:
        return False


def save(report: Report, path: str | os.PathLike, unit_system: Any = None,
         **_ignored: Any) -> None:
    """Write the report as a template (JSON).

    `unit_system` is accepted because every exporter takes it, and is not
    used: a template holds no values, only what to draw and how to bind
    it. So the test dates a channel table block keeps
    (`report.calibration`) are left out: the next test is judged against
    its own date, never the last one's.
    """
    out = {MARKER: 1, 'title': report.title, 'marking': report.marking,
           'marking_color': report.marking_color,
           'blocks': [{key: value for key, value in block.items()
                       if key not in _TEST_ONLY}
                      for block in report.blocks]}
    if report.features:
        # only when there are any: a template with none is the file it
        # always was, and opens in a version that has no features
        out['features'] = list(report.features)
    from ..report.identity import TEMPLATE_FIELDS

    # the laboratory's own fields only: a number, a revision, a status
    # or an issue record belongs to one report and never to the next
    laboratory = {key: report.identity[key] for key in TEMPLATE_FIELDS
                  if key in report.identity}
    if laboratory:
        out['identity'] = laboratory
    with open(path, 'w', encoding='utf-8') as handle:
        json.dump(out, handle, indent=1)
        handle.write('\n')


def load(path: str | os.PathLike, **_ignored: Any) -> Report:
    """Read a template as a Report, keeping only the laboratory's
    identification fields.

    Raises ValueError when the file has no template marker, when `features`
    is not a list of keys, or when `identity` is not a mapping.
    """
    from ..core.report import Report

    with open(path, encoding='utf-8') as handle:
        data = json.load(handle)
    if not isinstance(data, dict) or not (MARKER in data or OLDER_MARKER in data):
        raise ValueError(f'{path} is not a report template')
    features = data.get('features', [])
    if (not isinstance(features, list)
            or not all(isinstance(key, str) for key in features)):
        raise ValueError(f'{path}: features must be a list of feature '
                         f'keys, not {features!r}')
    from ..report.identity import TEMPLATE_FIELDS

    identity = data.get('identity', {})
    if (not isinstance(identity, dict)
            or not all(isinstance(key, str) for key in identity)):
        raise ValueError(f'{path}: identity must be a mapping of fields, not '
                         f'{identity!r}')
    return Report(title=str(data.get('title', 'Report')),
                  blocks=data.get('blocks', []),
                  marking=str(data.get('marking', 'UNCLASSIFIED')),
                  marking_color=str(data.get('marking_color', 'ink')),
                  features=features,
                  identity={key: identity[key] for key in TEMPLATE_FIELDS
                            if key in identity})


def templates_folder() -> Path:
    """The folder saved templates are kept in.

    It is `Hertzforge/Report Templates` in the user's application
    data folder for the platform (`~/Library/Application Support` on
    macOS, `APPDATA` on Windows, `XDG_DATA_HOME` or `~/.local/share`
    elsewhere), or the folder `HERTZFORGE_TEMPLATES` names when that
    environment variable is set. The folder is not created here; whatever
    writes to it creates it.
    """
    override = os.environ.get('HERTZFORGE_TEMPLATES')
    if override:
        return Path(override)
    if sys.platform == 'darwin':
        base = Path.home() / 'Library' / 'Application Support'
    elif sys.platform.startswith('win'):
        base = Path(os.environ.get('APPDATA', Path.home() / 'AppData' / 'Roaming'))
    else:
        base = Path(os.environ.get('XDG_DATA_HOME', Path.home() / '.local' / 'share'))
    return base / 'Hertzforge' / 'Report Templates'


def saved_templates() -> list[tuple[str, Path]]:
    """(name, path) for every template file in `templates_folder()`, sorted
    by name, the newer suffix taken when a name has both; empty when the
    folder does not exist."""
    folder = templates_folder()
    if not folder.is_dir():
        return []
    found: dict[str, Path] = {}
    for suffix in (OLDER_SUFFIX, SUFFIX):
        found.update((path.name[:-len(suffix)], path) for path in folder.iterdir()
                     if path.is_file() and path.name.endswith(suffix))
    return sorted(found.items())
