"""Reports on the connection's project: Visual Dynamics' report, built from
a starter template bound to the project's objects, shown as the page the
core writes, and saved as a file (`report.*`).

The page is the core's own (`report.render_html`): one HTML document with
its stylesheet, data and viewers inline and nothing loaded from a
network, so the shell shows it in a sandboxed frame exactly as a reader
of the exported file sees it.
"""

from __future__ import annotations

import os
from collections.abc import Mapping
from typing import Any

from .protocol import bad_request, not_found, refused

REPORT_METHODS = ('report.generate', 'report.render', 'report.export', 'report.editor',
                  'report.operate')

#: the starter templates, as VD offers them; 'empty' starts blank
TEMPLATES = ('modal', 'random', 'shock', 'transient', 'sine', 'sysid', 'mimo', 'empty')

THEMES = ('light', 'dark')


def _report(project: Any, params: Mapping[str, Any], method: str) -> str:
    from hertzforge.core.report import Report

    name = params.get('name')
    if not isinstance(name, str) or not name:
        raise bad_request(f'{method} needs "name", a report\'s name')
    if name not in project:
        raise not_found(f'the project has no object named {name!r}')
    if not isinstance(project[name], Report):
        raise refused(f'{name} is not a report')
    return name


def generate(project: Any, params: Mapping[str, Any]) -> str:
    """`report.generate`: a report built from `template`, bound by type to
    the project's objects (`Project.generate_report`); its name."""
    template = params.get('template', 'modal')
    if template not in TEMPLATES:
        raise bad_request(f'report.generate "template" is one of {", ".join(TEMPLATES)}')
    name = params.get('name', 'Report')
    if not isinstance(name, str) or not name.strip():
        raise bad_request('report.generate "name" is a name')
    try:
        return project.generate_report(template, name=name.strip())
    except ValueError as error:
        raise refused(str(error)) from None


def render(project: Any, params: Mapping[str, Any], method: str = 'report.render') -> str:
    """The report `name` as the HTML page the core writes, in `theme`."""
    from hertzforge.report import render_html

    name = _report(project, params, method)
    theme = params.get('theme')
    if theme is not None and theme not in THEMES:
        raise bad_request(f'{method} "theme" is light or dark')
    return render_html(project[name], dict(project), object_groups=project.object_groups,
                       theme=theme)


def export(project: Any, params: Mapping[str, Any]) -> str:
    """`report.export`: the page written to `path` (`.html` added when it
    has another suffix); the path written. Written beside the target and
    renamed over it, so a failed write leaves an earlier file whole."""
    path = params.get('path')
    if not isinstance(path, str) or not path.strip():
        raise bad_request('report.export needs "path", where to write the page')
    path = os.path.expanduser(path.strip())
    if not path.lower().endswith(('.html', '.htm')):
        path += '.html'
    html = render(project, {'name': params.get('name'), 'theme': 'light'}, 'report.export')
    partial = f'{path}.partial'
    try:
        with open(partial, 'w', encoding='utf-8') as handle:
            handle.write(html)
        os.replace(partial, path)
    except OSError as error:
        if os.path.exists(partial):
            os.remove(partial)
        raise refused(f'could not write {path}: {error.strerror or error}') from None
    return path


def _selected(params: Mapping[str, Any], method: str) -> int | None:
    selected = params.get('selected')
    if selected is not None and (isinstance(selected, bool) or not isinstance(selected, int)):
        raise bad_request(f'{method} "selected" is a block index, or null')
    return selected


def edit_page(project: Any, params: Mapping[str, Any]) -> dict[str, Any]:
    """`report.editor`: the report's edit page and its pane
    (`report_editor.editor`)."""
    from . import report_editor

    name = _report(project, params, 'report.editor')
    theme = params.get('theme')
    if theme is not None and theme not in THEMES:
        raise bad_request('report.editor "theme" is light or dark')
    page = params.get('page', True)
    if not isinstance(page, bool):
        raise bad_request('report.editor "page" is true or false')
    return report_editor.editor(project, name, _selected(params, 'report.editor'), theme, page)


def operate(project: Any, params: Mapping[str, Any]) -> dict[str, Any]:
    """`report.operate`: one of the editor's operations, then the edit
    page and pane again with the block it leaves selected."""
    from . import report_editor

    name = _report(project, params, 'report.operate')
    _selected(params, 'report.operate')
    selected = report_editor.operate(project, name, params)
    theme = params.get('theme')
    return report_editor.editor(project, name, selected,
                                theme if theme in THEMES else None)
