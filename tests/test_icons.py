"""Every object type is tellable apart in the tree.

Icons are drawn rather than shipped, so the thing to guard is that each type
actually has one — a type with no entry falls back to a gray circle, which
is what `Specification` and `Coherence` did until someone noticed — and that
no two of them come out the same.
"""

from __future__ import annotations


# every object type the tree can show, taken from the object model rather
# than from the icon table — asking the icon table what needs an icon is
# what let two types go without one
def showable_types():
    from hertzforge.core.channel_table import ChannelTable
    from hertzforge.core.data import NAMED_CLASSES
    from hertzforge.core.geometry import Geometry
    from hertzforge.core.shapes import ShapeSet

    # 'Test' is the tree's root, not a core class yet, so it is named here
    return sorted({cls.__name__ for cls in NAMED_CLASSES.values()}
                  | {Geometry.__name__, ShapeSet.__name__,
                     ChannelTable.__name__, 'Test'})


TYPES = showable_types()
