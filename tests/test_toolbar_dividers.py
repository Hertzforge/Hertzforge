"""Dividers that mean something, on every bar.

A control bar holds two kinds of thing and used to run them together:
*choices*, where one of a set is in force and picking one drops the
last, and *settings*, each independent of the rest. The 2-D/3-D toggle
is the clearest case of the second kind — it is not a way of reading a
record, it is how any reading is drawn — and it sat in an undivided row
with four things that do turn each other off (Brandon, 2026-08-27).

`gui/toolbars.fence` puts a divider around every group at build time and
`tidy` decides at show time which of them still divide anything, so the
rule is one implementation rather than a habit each bar has to keep.

The collapsing case is the one that was wrong first and is easy to get
wrong again: two groups fenced side by side put two separators
together, and hiding *both* — which the first version did — runs the
groups into each other, which is the whole thing this exists to stop.
"""

from __future__ import annotations


def shape(toolbar):
    """The bar as a reader sees it: '|' for a visible divider."""
    out = []
    for action in toolbar.actions():
        if action.isSeparator():
            if action.isVisible():
                out.append('|')
        elif action.isVisible():
            out.append(action.text())
    return out
