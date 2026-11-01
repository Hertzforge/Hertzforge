"""What a geometry's entities read as in the tree, and in the status bar.

Expanding a category lists its entities, and each row has to say which
one it is: an id for a node, an id and a type for a coordinate system, a
node count for a line, a type and a node count for an element. They are
built lazily — a 200 000-node model must not build 200 000 tree items on
import — and the list is what the selection and the delete act on, so
the labels are load-bearing rather than decoration.

The status bar is the other half of the same reading: one picked node
says where it is, in the display units, and several say how many.
"""

from __future__ import annotations


def category(item, label):
    return next(item.child(i) for i in range(item.childCount())
                if item.child(i).text(0).startswith(label))
