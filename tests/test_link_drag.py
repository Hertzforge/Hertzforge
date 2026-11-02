"""Dragging an object from one object group into another.

A typed project sorts arriving objects into its groups by itself, and it
guesses: import an FEM geometry and its mode shapes into a Modal Test and
both land in the Basis, because from the outside they satisfy the same
slots the measured pair does. Until this there was no way to say
otherwise — Unlink took an object out of every group and Link merges the
two groups it names, which is the opposite of moving one thing between
them.

The gesture is the obvious one, so what is worth pinning is the parts
that are not: that a refused move is never offered rather than being
taken and complained about, that the file drop the tree already handled
is untouched, and that the project's own rule decides both.
"""

from __future__ import annotations


def _body(function):
    import inspect

    return inspect.getsource(function)
