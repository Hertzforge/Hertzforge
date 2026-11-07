"""Editing a report on the connection's project, as Visual Dynamics' report
editor does it (`report.editor`, `report.operate`).

The page is the core's edit page (`render_html(edit=True)`): every block
tagged with its index, the selected one framed, and a click saying which
block was clicked. VD's page says so over Qt's web channel; here
`CHANNEL_JS` stands in for that channel and posts the same message to the
shell, which frames the page. Every act (insert, move, delete, a field,
the text, the title and the marking) is on the shell's bar and pane, as
it is on VD's, and each is one `report.operate` with VD's own operation
names. Inserting binds the new block to the first object in the project
it can show, as VD's Insert does (`BLOCK_TEMPLATES`, VD's
`gui/report_editor.py`).
"""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any

from .protocol import bad_request, refused

#: the edit page's channel to the shell: Qt's QWebChannel answered with a
#: bridge whose `apply` posts the page's message to the frame's parent, and
#: the page's scroll posted as it rests. No `copy_image` or `copy_table`, so
#: a figure's copy button uses the view's own clipboard.
CHANNEL_JS = """
window.qt = window.qt || { webChannelTransport: {} };
window.QWebChannel = function (_transport, ready) {
  ready({ objects: { bridge: {
    apply: payload => window.parent.postMessage({ hforgeReport: JSON.parse(payload) }, '*')
  } } });
};
// how far the page is scrolled, so a re-render after an edit opens there
let hforgeScrolled = null;
addEventListener('scroll', () => {
  clearTimeout(hforgeScrolled);
  hforgeScrolled = setTimeout(() => window.parent.postMessage({ hforgeScroll: scrollY }, '*'), 120);
});
"""

# VD's Insert menu: Markdown, plus the Result drop-down's options. Each
# template's '_bind' fields are pointed at the first compatible object in
# the project on insert, so a result appears at once and the pane's
# drop-downs are there to repoint it.
BLOCK_TEMPLATES: dict[str, dict[str, Any]] = {
    'markdown': {'kind': 'text', 'text': ''},
    'geometry': {'kind': 'scene', 'geometry': '', 'shapes': '',
                 'caption': '', '_bind': ('geometry',)},
    'modes': {'kind': 'scene', 'geometry': '', 'shapes': '',
              'caption': '', '_bind': ('geometry', 'shapes')},
    'cmif': {'kind': 'plot', 'source': '', 'mode': 'cmif', 'shapes': '',
             'caption': '', '_bind': ('source', 'shapes')},
    'coherence': {'kind': 'plot', 'source': '', 'mode': 'map',
                  'caption': '', '_bind': ('source',)},
    'stage': {'kind': 'plot', 'source': '', 'mode': 'stage',
              'caption': '', '_bind': ('source',)},
    'mac': {'kind': 'plot', 'source': '', 'mode': 'mac', 'caption': '',
            '_bind': ('source',)},
    'photos': {'kind': 'photo', 'source': '', 'photo': '', 'caption': '',
               '_bind': ('source', 'photo')},
    'channel_table': {'kind': 'table', 'source': '', 'caption': '',
                      '_bind': ('source',), '_only': 'ChannelTable'},
    'matches': {'kind': 'pairs', 'source': '', 'caption': '',
                '_bind': ('source',), '_only': 'MatchedModes'},
    'overlay': {'kind': 'overlay', 'source': '', 'caption': '',
                '_bind': ('source',), '_only': 'MatchedModes'},
    'bars': {'kind': 'bars', 'mode': 'error', 'source': '', 'measured': '',
             'caption': '', '_bind': ('source', 'measured'),
             '_only': 'Specification'},
    'verdict': {'kind': 'verdict', 'source': '', 'measured': '',
                '_bind': ('source', 'measured'), '_only': 'Specification'},
    'prediction': {'kind': 'prediction', 'source': '', 'measured': '',
                   'level': None, 'caption': '',
                   '_bind': ('source', 'measured')},
}

#: the operations `report.operate` takes, VD's editor's names
OPERATIONS = ('insert', 'remove', 'move', 'field', 'text', 'title', 'marking',
              'marking_color')

#: the fields of a block the pane may set, beside its text and caption
FIELDS = ('source', 'geometry', 'shapes', 'channel', 'dofs_source', 'photo', 'measured')


def _template(kind: str) -> dict[str, Any] | None:
    """The block VD's Insert makes for `kind`, one of the menu's kinds."""
    if kind.startswith(('time:', 'spectrum:', 'psd:')):
        # Time (Acceleration), Spectra (Force), PSD (...): one per quantity
        # that kind of data holds
        prefix, _, dimension = kind.partition(':')
        return {'kind': 'plot', 'source': '', 'mode': 'curves',
                'select': f'dim:{dimension}', 'caption': '', '_bind': ('source',),
                '_only': {'time': 'TimeHistory', 'spectrum': 'Spectrum',
                          'psd': 'Psd'}[prefix]}
    if kind.startswith('dofs:'):
        return {'kind': 'scene', 'geometry': '', 'shapes': '',
                'dofs': kind.partition(':')[2], 'dofs_source': '', 'caption': '',
                '_bind': ('geometry', 'dofs_source')}
    if kind.startswith('frf_drive'):
        return {'kind': 'plot', 'source': '', 'mode': 'curves', 'select': 'drive',
                'component': kind.partition(':')[2] or 'magnitude', 'shapes': '',
                'caption': '', '_bind': ('source', 'shapes')}
    return BLOCK_TEMPLATES.get(kind)


def _options(field: str, block: Mapping[str, Any], objects: Mapping[str, Any],
             object_groups: Any = None) -> list[str]:
    """The project's objects a block's `field` may name, as VD's pane lists
    them."""
    from hertzforge.core.report import (
        dofs_source_options,
        measured_options,
        photo_options,
        shape_options,
        source_options,
    )
    from hertzforge.report import scalogram_channel_options

    if field == 'shapes':
        return shape_options(objects)
    if field == 'photo':
        return photo_options(block, objects)
    if field == 'dofs_source':
        return dofs_source_options(block, objects)
    if field == 'measured' and block.get('kind') == 'prediction':
        return measured_options(objects)
    if field == 'channel':
        return list(scalogram_channel_options(block, objects, object_groups))
    return source_options(block, objects)


def insert(report: Any, kind: str, at: int | None, objects: Mapping[str, Any]) -> int:
    """A block of `kind` inserted at `at` (the end when None), its sources
    bound to the first compatible objects; its index."""
    from hertzforge.core.report import record_dimensions

    template = _template(kind)
    if not template:
        raise refused(f'{kind!r} is not something the report can insert')
    block = {key: value for key, value in template.items() if not key.startswith('_')}
    for field in template.get('_bind', ()):
        options = _options(field, block, objects)
        only = template.get('_only')
        if field == 'source' and only:
            options = [name for name in options if type(objects[name]).__name__ == only]
        select = block.get('select', '')
        if field == 'source' and select.startswith('dim:'):
            # bound to data that holds the quantity, not any data of the kind
            options = [name for name in options
                       if select[4:] in record_dimensions(objects[name])]
        if options:
            block[field] = options[0]
    if at is not None and not 0 <= at <= report.num_blocks:
        raise refused(f'the report has {report.num_blocks} blocks; it cannot insert at {at}')
    report.add(block, at=at)
    return report.num_blocks - 1 if at is None else at


def _index(report: Any, params: Mapping[str, Any], key: str = 'at') -> int:
    at = params.get(key)
    if isinstance(at, bool) or not isinstance(at, int):
        raise bad_request(f'report.operate "{key}" is a block index')
    if not 0 <= at < report.num_blocks:
        raise refused(f'the report has {report.num_blocks} blocks; there is no block {at + 1}')
    return at


def operate(project: Any, name: str, params: Mapping[str, Any]) -> int | None:
    """One of VD's editor operations on the report `name`; the block
    selected after it, or None for the report itself."""
    report = project[name]
    objects = dict(project)
    op = params.get('op')
    selected = params.get('selected')
    if op not in OPERATIONS:
        raise bad_request(f'report.operate "op" is one of {", ".join(OPERATIONS)}')
    if op == 'insert':
        kind = params.get('kind')
        if not isinstance(kind, str):
            raise bad_request('report.operate insert needs "kind", one of the Insert menu\'s')
        at = params.get('at')
        if at is not None and (isinstance(at, bool) or not isinstance(at, int)):
            raise bad_request('report.operate "at" is a block index')
        return insert(report, kind, at, objects)
    if op == 'remove':
        report.remove([_index(report, params)])
        return None
    if op == 'move':
        source = _index(report, params, 'from')
        to = params.get('to')
        if isinstance(to, bool) or not isinstance(to, int):
            raise bad_request('report.operate move needs "to", the index to move before')
        if not 0 <= to <= report.num_blocks:
            raise refused(f'the report has {report.num_blocks} blocks, so there is no place to '
                          f'move to at {to}')
        report.move([source], to)
        return to - 1 if to > source else to
    if op in ('title', 'marking', 'marking_color'):
        value = params.get('value')
        if not isinstance(value, str):
            raise bad_request(f'report.operate {op} needs "value", text')
        if op == 'title':
            if not value.strip():
                raise refused('a report needs a title')
            report.title = value.strip()
        elif op == 'marking':
            # one value behind both banners
            report.marking = value.strip()
        else:
            report.marking_color = value if value in ('ink', 'red') else 'ink'
        return selected if isinstance(selected, int) else None
    at = _index(report, params)
    block = report.blocks[at]
    value = params.get('value')
    if op == 'text':
        if block.get('kind') != 'text' or not isinstance(value, str):
            raise refused(f'block {at + 1} is not a text block')
        block['text'] = value
        return at
    field = params.get('field')
    if field == 'caption':
        if not isinstance(value, str):
            raise bad_request('a caption is text')
        block['caption'] = value
        return at
    if field not in FIELDS:
        raise bad_request(f'report.operate field is caption or one of {", ".join(FIELDS)}')
    if not isinstance(value, str):
        raise bad_request(f'{field} names an object, or is empty')
    if field == 'source' and block.get('kind') == 'scene':
        field = 'geometry'
    allowed = _options(field, block, objects, project.object_groups)
    symbolic = value.startswith('@')
    if value and not symbolic and value not in allowed:
        raise refused(f'{value!r} is not something block {at + 1} can show as its {field}')
    block[field] = value
    return at


def pane(project: Any, name: str, selected: int | None) -> dict[str, Any]:
    """What the shell's pane shows for the selection: the report's own
    title and marking with nothing selected, a text block's text, or a
    figure block's sources and caption, each with the options VD's pane
    lists."""
    from hertzforge.core.report import insert_options, symbolic_options, takes_shapes

    report = project[name]
    objects = dict(project)
    out: dict[str, Any] = {
        'title': report.title, 'marking': report.marking,
        'marking_color': report.marking_color, 'blocks': report.num_blocks,
        'insert': [['markdown', 'Markdown']] + [list(pair) for pair in insert_options(objects)],
        'block': None}
    if selected is None or not 0 <= selected < report.num_blocks:
        return out
    block = report.blocks[selected]
    entry: dict[str, Any] = {'index': selected, 'kind': block.get('kind', ''),
                             'mode': block.get('mode') or '', 'fields': []}
    if block.get('kind') == 'text':
        entry['text'] = block.get('text', '')
        # a skeleton section, a check or a requirement has a pane of its own in VD
        entry['special'] = next((key for key in ('section', 'check', 'requirement')
                                 if key in block), None)
    else:
        source_key = 'geometry' if block.get('kind') == 'scene' else 'source'
        entry['fields'].append(_field('source', 'Source', block.get(source_key, ''),
                                      [('', '(source)'), *symbolic_options(block)]
                                      + [(n, n) for n in _options('source', block, objects)]))
        if takes_shapes(block):
            entry['fields'].append(_field(
                'shapes', 'Shapes', block.get('shapes', ''),
                [('', '(no shapes)'), ('@basis:ShapeSet', 'Basis Shape Set (auto)'),
                 ('@other:ShapeSet', 'Other Shape Set (auto)')]
                + [(n, n) for n in _options('shapes', block, objects)]))
        if block.get('mode') == 'scalogram':
            entry['fields'].append(_field(
                'channel', 'Channel', block.get('channel', ''),
                [(n, n) for n in _options('channel', block, objects, project.object_groups)]))
        if block.get('dofs'):
            entry['fields'].append(_field(
                'dofs_source', 'DOFs from', block.get('dofs_source', ''),
                [('', '(DOFs from)')] + [(n, n) for n in _options('dofs_source', block,
                                                                  objects)]))
        if block.get('kind') == 'photo':
            entry['fields'].append(_field(
                'photo', 'Photo', block.get('photo', ''),
                [('', '(photo)')] + [(n, n) for n in _options('photo', block, objects)]))
        if 'measured' in block:
            entry['fields'].append(_field(
                'measured', 'Measured', block.get('measured', ''),
                [('', '(measured)')] + [(n, n) for n in _options('measured', block,
                                                                 objects)]))
        entry['caption'] = block.get('caption', '')
    out['block'] = entry
    return out


def _field(field: str, label: str, value: Any, options: list[tuple[str, str]]
           ) -> dict[str, Any]:
    seen = dict.fromkeys(options)
    if value and value not in {v for v, _ in seen}:
        # a name bound before and gone since: kept, so the pane says it
        seen[(value, f'{value} (not in the project)')] = None
    return {'field': field, 'label': label, 'value': value or '',
            'options': [[v, lab] for v, lab in seen]}


def editor(project: Any, name: str, selected: int | None, theme: str | None,
           page: bool = True, units: Any = None) -> dict[str, Any]:
    """`report.editor`: the edit page with `selected` framed, the pane for
    it, and the labels a text block may reference (`{{figure:...}}`).
    Without `page` the pane alone, and no labels, since nothing was
    rendered: a click on the page frames the block there, so selecting
    re-renders nothing, as in Visual Dynamics."""
    from hertzforge.report import render_html

    report = project[name]
    if selected is not None and not 0 <= selected < report.num_blocks:
        selected = None
    out = pane(project, name, selected)
    out['selected'] = selected
    if page:
        labels: list[tuple[str, str]] = []
        out['html'] = render_html(report, project, units, edit=True, channel_js=CHANNEL_JS,
                                  object_groups=project.object_groups, selected=selected,
                                  labels=labels, theme=theme)
        out['references'] = [[label, caption] for label, caption in labels]
    return out
