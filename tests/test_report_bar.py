"""The report editor's acts on the bar, its settings in the pane.

Brandon, 2026-09-08: the report's toolbar should be the same bar every
other object has. The page stays the preview — the very HTML the export
writes — and does two things: frame the selected block and say which
block was clicked. Insert, Reference, Move Up, Move Down, Delete and
Export are on the bar; the selected block's sources, caption or Markdown
are in the pane beside; every edit is one operation on the Report model.
"""

from __future__ import annotations


def _labels(menu):
    return [a.text() for a in menu.actions() if not a.isSeparator()]
