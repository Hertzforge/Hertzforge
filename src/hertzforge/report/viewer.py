"""A report written as the shell's own page (step 4 of the reports plan).

`viewer.html` is the viewer the UI builds alone (`npm --prefix ui run
build:report`): `ReportPage` and the shell's viz kit, with their script,
styles and fonts inlined. `viewer_html` fills in a report's title and its
blocks (`views.page_blocks`) as one protocol frame, base64, so the file is
one page that opens offline in any browser and draws the report as the app
does.
"""

from __future__ import annotations

import base64
import html
from collections.abc import Mapping
from importlib import resources
from typing import Any

#: where the viewer takes the report's title and its data
TITLE = '<!--HERTZFORGE-REPORT-TITLE-->'
DATA = '<!--HERTZFORGE-REPORT-DATA-->'
THEMES = ('light', 'dark')


def viewer_page() -> str:
    """The viewer as the UI built it, with nothing filled in."""
    return resources.files('hertzforge.report').joinpath('viewer.html').read_text(
        encoding='utf-8')


def viewer_html(report: Any, objects: Mapping[str, Any], unit_system: Any = None,
                object_groups: Any = None, theme: str | None = None) -> str:
    """The report as one self-contained HTML page drawn by the shell's own
    viewers: its blocks (`views.page_blocks`) in `unit_system`, in the
    `light` or `dark` theme (the reader's own when None), for the reader
    to switch."""
    from ..service import frames
    from .views import page_blocks, page_style

    if theme is not None and theme not in THEMES:
        raise ValueError(f'theme is light or dark, not {theme!r}')
    result = {'name': str(report.title), 'title': str(report.title),
              'marking': str(report.marking), 'css': page_style(report, objects),
              'blocks': page_blocks(report, objects, unit_system,
                                    object_groups=object_groups)}
    frame = frames.pack({'type': 'response', 'id': 0, 'result': result})
    data = frame.encode('utf-8') if isinstance(frame, str) else frame
    page = viewer_page()
    if TITLE not in page or DATA not in page:
        raise RuntimeError('the report viewer holds no place for a report; build it again')
    page = page.replace(TITLE, html.escape(str(report.title)), 1)
    page = page.replace(DATA, base64.b64encode(data).decode('ascii'), 1)
    if theme is not None:
        page = page.replace('<html lang="en">', f'<html lang="en" data-theme="{theme}">', 1)
    return page


def read_viewer(page: str) -> dict[str, Any]:
    """The report a page from `viewer_html` holds: `{'name', 'title',
    'marking', 'blocks'}`, its views' arrays as numpy arrays. A page with
    no report raises ValueError."""
    from ..service import frames

    start = page.find('<script id="report-data" type="application/octet-stream">')
    if start < 0:
        raise ValueError('this page holds no report')
    start = page.index('>', start) + 1
    encoded = page[start:page.index('</script>', start)].strip()
    if not encoded or encoded == DATA:
        raise ValueError('this page holds no report')
    data = base64.b64decode(encoded)
    message = frames.decode(data.decode('utf-8') if data[:1] == b'{' else data)
    return message['result']


def page_text(page: str) -> str:
    """The words a page from `viewer_html` shows, one line each: its title,
    marking and stylesheet, and each block's text, number, caption and note,
    its tables' cells and its readings' rows. What a reader can search for,
    and what a test looks for."""
    held = read_viewer(page)
    lines = [str(held.get('title', '')), str(held.get('marking', '')), str(held.get('css', ''))]
    for block in held['blocks']:
        lines += [str(block.get(key, '')) for key in ('html', 'label', 'caption', 'note')]
        for view in block.get('views', []):
            lines.append(str(view.get('title', '')))
            if view['view'] == 'table':
                lines += ['\t'.join(view['columns'])] + ['\t'.join(row) for row in view['rows']]
            elif view['view'] == 'reading':
                lines += [f"{row['label']}\t{row['value']}" for row in view['rows']]
    return '\n'.join(line for line in lines if line)
