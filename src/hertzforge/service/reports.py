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
                  'report.operate', 'report.print', 'report.issue')

#: the steps of an issue: the page that says issued is printed by the shell
#: between `begin` and `record`
ISSUE_STEPS = ('begin', 'record', 'cancel')

#: a report waiting for its PDF, by the report object: the status it will
#: carry and the page the shell prints. The report stays as it was until
#: `record`, so a save in between writes nothing that was not recorded.
_PENDING: dict[int, tuple[str, str]] = {}

#: run first by the page printed: it draws its figures at print
#: resolution (the page's `#print`), then asks for the print dialog once
#: they are drawn
_PRINTS = ("<script>history.replaceState(null, '', '#print');</script>",
           "<script>addEventListener('load', () => setTimeout(() => print(), 600));</script>")

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


def render(project: Any, params: Mapping[str, Any], method: str = 'report.render',
           units: Any = None) -> str:
    """The report `name` as the HTML page the core writes, in `theme` and
    the unit system `units` (the core's default when None)."""
    from hertzforge.report import render_html

    name = _report(project, params, method)
    theme = params.get('theme')
    if theme is not None and theme not in THEMES:
        raise bad_request(f'{method} "theme" is light or dark')
    return render_html(project[name], project, units, object_groups=project.object_groups,
                       theme=theme)


def _anyway(params: Mapping[str, Any], method: str) -> bool:
    anyway = params.get('anyway', False)
    if not isinstance(anyway, bool):
        raise bad_request(f'{method} "anyway" is true or false')
    return anyway


def export_checks(project: Any, name: str) -> dict[str, Any]:
    """What is asked about before a report leaves the project: with
    skeleton sections on, the sections (a flash report's parts) the report
    still owes; with the data index on, every indexed file hashed again and
    those that do not match the checksum indexed, each with what the check
    found. `{'owed': [...], 'index': [{'name', 'status'}]}`, both empty
    when nothing is asked."""
    from hertzforge.core.data_index import MATCHES
    from hertzforge.report.data_index import CHECK_WORDS
    from hertzforge.report.data_index import FEATURE as INDEX
    from hertzforge.report.features import enabled
    from hertzforge.report.skeleton import FEATURE as SKELETON

    report = project[name]
    features = enabled(report)
    owed = list(project.report_check(name)) if SKELETON in features else []
    astray: list[dict[str, str]] = []
    if INDEX in features:
        try:
            project.verify_data_index()
        except OSError as error:
            raise refused(f'the data index could not be checked: {error}') from None
        astray = [{'name': entry.name, 'status': CHECK_WORDS[entry.status]}
                  for entry in project.data_index if entry.status != MATCHES]
    return {'owed': owed, 'index': astray}


def export(project: Any, params: Mapping[str, Any], units: Any = None) -> dict[str, Any]:
    """`report.export`: the page written to `path` (`.html` added when it
    has another suffix); `{'path'}`, the path written. Written beside the
    target and renamed over it, so a failed write leaves an earlier file
    whole. First the checks (`export_checks`): a report that still owes
    sections or whose indexed files do not all match is not written unless
    `anyway`; the answer then carries `owed` and `index` and no `path`, for
    the shell to ask "Export anyway?". The page is the one printed and
    issued, and refused when the report was issued and has changed since."""
    name = _report(project, params, 'report.export')
    path = params.get('path')
    if not isinstance(path, str) or not path.strip():
        raise bad_request('report.export needs "path", where to write the page')
    path = os.path.expanduser(path.strip())
    if not path.lower().endswith(('.html', '.htm')):
        path += '.html'
    checks = export_checks(project, name)
    if (checks['owed'] or checks['index']) and not _anyway(params, 'report.export'):
        return {'path': None, **checks}
    try:
        html = project._export_page(name, units)
    except ValueError as error:
        raise refused(str(error)) from None
    partial = f'{path}.partial'
    try:
        with open(partial, 'w', encoding='utf-8') as handle:
            handle.write(html)
        os.replace(partial, path)
    except OSError as error:
        if os.path.exists(partial):
            os.remove(partial)
        raise refused(f'could not write {path}: {error.strerror or error}') from None
    return {'path': path, 'owed': [], 'index': []}


def _selected(params: Mapping[str, Any], method: str) -> int | None:
    selected = params.get('selected')
    if selected is not None and (isinstance(selected, bool) or not isinstance(selected, int)):
        raise bad_request(f'{method} "selected" is a block index, or null')
    return selected


def edit_page(project: Any, params: Mapping[str, Any], units: Any = None) -> dict[str, Any]:
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
    return report_editor.editor(project, name, _selected(params, 'report.editor'), theme, page,
                                units)


def operate(project: Any, params: Mapping[str, Any], units: Any = None) -> dict[str, Any]:
    """`report.operate`: one of the editor's operations, then the edit
    page and pane again with the block it leaves selected."""
    from . import report_editor

    name = _report(project, params, 'report.operate')
    _selected(params, 'report.operate')
    selected = report_editor.operate(project, name, params)
    theme = params.get('theme')
    return report_editor.editor(project, name, selected,
                                theme if theme in THEMES else None, units=units)


def _printable(page: str) -> str:
    head, body = _PRINTS
    page = page.replace('<head>', '<head>' + head, 1)
    return page.replace('</body>', body + '</body>', 1)


def print_page(project: Any, params: Mapping[str, Any], units: Any = None) -> dict[str, Any]:
    """`report.print`: the reader's page, light, as it is printed: drawn at
    print resolution and asking for the print dialog once its figures are
    drawn, where the reader saves a PDF. Its print stylesheet carries the
    margins the identity rides in (`report.identity.MARGINS`). `{'html',
    'owed', 'index'}`. First the checks an export makes (`export_checks`):
    unless `anyway`, a report that owes sections or whose indexed files do
    not all match answers with them and `html` null, for the shell to ask.
    Refused when the report was issued and has changed since."""
    name = _report(project, params, 'report.print')
    checks = export_checks(project, name)
    if (checks['owed'] or checks['index']) and not _anyway(params, 'report.print'):
        return {'html': None, **checks}
    try:
        return {'html': _printable(project._export_page(name, units)), 'owed': [], 'index': []}
    except ValueError as error:
        raise refused(str(error)) from None


def issue(project: Any, params: Mapping[str, Any], units: Any = None) -> dict[str, Any]:
    """`report.issue`, in three steps around the print the shell makes.
    `begin` checks the report can be issued (`issue_ready`) and gives the
    page to print, which says issued; the report itself is unchanged.
    `record` reads the PDF printed from it at `path`, records the issue
    with the SHA-256 of the PDF and of the page (`record_issue`), which is
    what makes the report issued, and gives the record's entry. `cancel`
    forgets the page."""
    from hertzforge.report.identity import issue_ready, mark_issued, record_issue, unmark

    name = _report(project, params, 'report.issue')
    report = project[name]
    step = params.get('step')
    if step not in ISSUE_STEPS:
        raise bad_request(f'report.issue "step" is one of {", ".join(ISSUE_STEPS)}')
    if step == 'begin':
        _PENDING.pop(id(report), None)
        try:
            status = issue_ready(report, project, project.object_groups)
        except ValueError as error:
            raise refused(str(error)) from None
        # the files hashed again, and an issue of a report whose files do
        # not all match is asked about
        checks = export_checks(project, name)
        if checks['index'] and not _anyway(params, 'report.issue'):
            return {'index': checks['index']}
        before = mark_issued(report, status)
        try:
            page = project._report_page(name, units)
        finally:
            unmark(report, before)
        _PENDING[id(report)] = (status, page)
        return {'html': _printable(page), 'status': status}
    pending = _PENDING.get(id(report))
    if pending is None:
        raise refused(f'{name} is not waiting to be issued; begin the issue first')
    if step == 'cancel':
        _PENDING.pop(id(report))
        return {}
    path = params.get('path')
    if not isinstance(path, str) or not path.strip():
        raise bad_request('report.issue record needs "path", the PDF printed from the page')
    path = os.path.expanduser(path.strip())
    try:
        with open(path, 'rb') as handle:
            pdf = handle.read()
    except OSError as error:
        raise refused(f'could not read {path}: {error.strerror or error}') from None
    if not pdf.startswith(b'%PDF'):
        raise refused(f'{path} is not a PDF; save the printed page as PDF')
    status, page = _PENDING.pop(id(report))
    mark_issued(report, status)
    return {'entry': record_issue(report, page, pdf, os.path.basename(path))}
