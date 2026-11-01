"""The console tab: the session as Python, at the bottom edge.

Piece three (Brandon, 2026-08-30): a tab, not a menu — collapsed by
default, one click to expand this sitting's journal, one to put it
away. Read-only here; typing into it is a later piece.
"""

from __future__ import annotations


def _flank_ink(tab, image, side, wanted):
    """The pixels of one chevron's flank that `wanted` accepts."""
    flank = tab.FOOT + tab.INSET + tab.GLYPH + tab.GAP // 2
    columns = (range(flank) if side == 'left'
               else range(image.width() - flank, image.width()))
    return {(x, y) for y in range(image.height()) for x in columns
            if wanted(image.pixelColor(x, y))}
