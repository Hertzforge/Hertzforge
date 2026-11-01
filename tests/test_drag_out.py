"""Dragging an object out of the window writes it as `.vdyn`.

The tree's drag already carried its own mime type so objects could be
moved between object groups. It now carries a second reading of the same
gesture — `text/uri-list` — so the same drag, released on the desktop
instead of on another row, saves a file there. Finder, Explorer and
Linux file managers all read that type; the tree ignores it.

The half worth testing hardest is not that a file appears. It is *when*
it appears: a project can be a hundred megabytes, and writing that at
the start of every drag — including the ones that only reorder rows —
would stall the window on every touch.
"""

from __future__ import annotations


def urls_from(mime):
    """The paths a drop target receives — through `urls()`, which is
    what the platform actually calls.

    Not `data('text/uri-list')`. Both are supposed to answer, and for a
    while only the second one did: `urls()` came back empty, macOS
    treated the drag as a *location* rather than a file, and dropping an
    object on the desktop produced `localhost.fileloc`. The tests all
    passed, because they read the half that worked.
    """
    return [url.toLocalFile() for url in mime.urls()]
