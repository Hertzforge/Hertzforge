"""Objects combined: Visual Dynamics' Merge, which makes one object of
several of one type (`Project.merge`, by `core.merge`'s rules) and puts
it in place of its parts.
"""

from __future__ import annotations

from typing import Any

from . import views as v
from .descriptors import Operation, Param
from .protocol import refused

#: every kind `core.merge` combines
MERGEABLE = ('objects:channel_table|geometry|shape_set|time_history|spectrum|psd|frf|'
             'coherence|multiple_coherence|srs|specification|shock_specification|'
             'transient_specification|sine_sweep_specification')
FIRST = Param('first', 'First', 'enum', options_from=MERGEABLE)
SECOND = Param('second', 'Second', 'enum', options_from=MERGEABLE)
MORE = Param('more', 'And', 'string', default='')
NAME = Param('name', 'Name', 'string', default='')


def merge(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.merge`: two or more objects of one type made into one
    (`Project.merge`), which replaces them, under the first one's name
    unless another is given. `more` names further objects, separated by
    semicolons. Refused in the core's words when the objects do not
    merge (`core.merge.mergeable`)."""
    from hertzforge.core.merge import mergeable

    names = [params['first'], params['second']]
    names += [n.strip() for n in params['more'].split(';') if n.strip()]
    missing = [n for n in names if n not in project]
    if missing:
        raise refused(f'the project has no object named {", ".join(map(repr, missing))}')
    if len(set(names)) < len(names):
        raise refused('each object is named once: merging one with itself is not a merge')
    reason = mergeable([project[n] for n in names])
    if reason:
        raise refused(reason)
    added = project.merge(*names, name=params['name'].strip() or None)
    merged = project[added]
    rows = [v.row('Merged', ', '.join(names), None, 'info'),
            v.row('Into', added, None, 'info')]
    count = getattr(merged, 'num_records', None) or getattr(merged, 'num_shapes', None)
    if count is not None:
        rows.append(v.row('Records' if hasattr(merged, 'num_records') else 'Modes',
                          float(count), '', 'info'))
    return [v.reading(f'Merged into {added}', rows)]


OPERATIONS = (
    Operation(
        id='project.merge', title='Merge into One', workspace='signals', everywhere=True, accepts='project',
        summary='Two or more objects of one type made into one, in place of its parts: '
                'geometries, shape sets, channel tables, or data on one abscissa. Name more '
                'after the second, separated by semicolons.',
        params=(FIRST, SECOND, MORE, NAME), outputs=('reading',), run=merge),
)
