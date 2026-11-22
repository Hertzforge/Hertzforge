"""A zoomed 2-D plot drawn from the samples, not the outline.

An operation sends a long trace peak-thinned (`hertzforge.decimate`): a
plot a few thousand pixels wide cannot show a million samples, and the
thinning keeps every extreme. Zoomed into a fraction of a second, those
few points are joined by straight lines that are not the record. So a
spectrum view whose curves were thinned carries `span`, the request
that drew it (`op.run` or `project.show`), and the shell sends that
again with the span of x it shows (`schema/PROTOCOL.md`). The answer
keeps the samples in the span, thinned to the points the plot can draw
there, which is every sample once they are fewer, and the outline of
the rest.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping
from contextlib import nullcontext
from typing import Any

from hertzforge.decimate import drawing_window, watching

from .protocol import bad_request

#: the most points a plot asks for across its span: two a pixel on a
#: wide screen, with room to spare
MOST_POINTS = 65_536


def checked_span(span: Any) -> tuple[float, float, int]:
    """A request's `span` as (low, high, points), or a `bad_request` that
    names what is wrong with it."""
    if not isinstance(span, Mapping) or set(span) != {'low', 'high', 'points'}:
        raise bad_request('"span" is {"low", "high", "points"}: the span of x a plot shows '
                          'and the points it draws across it')
    low, high, points = span['low'], span['high'], span['points']
    numbers = all(isinstance(value, int | float) and not isinstance(value, bool)
                  for value in (low, high))
    if not numbers or not float('-inf') < low < high < float('inf'):
        raise bad_request(f'"span" runs from a low x to a higher one, not {low!r} '
                          f'to {high!r}')
    if isinstance(points, bool) or not isinstance(points, int) or not 2 <= points <= MOST_POINTS:
        raise bad_request(f'"span" draws from 2 to {MOST_POINTS} points, not {points!r}')
    return float(low), float(high), points


def drawn(span: tuple[float, float, int] | None, call: Callable[..., Any],
          *args: Any) -> tuple[Any, bool]:
    """`call(*args)`'s views, thinned for `span` when one is given, and
    whether any trace in them was thinned. Runs on the thread that
    draws, since the window rides on a context variable."""
    with watching() as thinned, drawing_window(*span) if span else nullcontext():
        views = call(*args)
    return views, bool(thinned)


def mark(views: list[dict], method: str, params: Mapping[str, Any]) -> list[dict]:
    """Each spectrum in `views` marked with how to draw it again over a
    span: the request that drew it, `method` with `params` as the client
    sent them, and the view's place in the answer. In place; returned
    for chaining."""
    sent = {key: value for key, value in params.items() if key != 'span'}
    for index, view in enumerate(views):
        if view.get('view') == 'spectrum':
            view['span'] = {'method': method, 'params': sent, 'view': index}
    return views
