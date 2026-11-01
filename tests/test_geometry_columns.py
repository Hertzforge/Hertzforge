"""The geometry tables' own columns: units, colors, ids and references.

`test_table_conventions.py` checks that every table *behaves* the same;
this is what these particular columns mean. Four of them refuse rather
than write — an id that is taken, a coordinate system that does not
exist, a color outside the palette, a frame type that is not one of the
three — and a refusal that quietly wrote would leave a geometry that
cannot be validated.

Coordinates are the other half: they read and write in the *display*
system, so typing 25 into a millimeter view has to arrive as 0.025 m and
come back as 25.
"""

from __future__ import annotations


def column_of(model, title):
    return [c.title for c in model.columns].index(title)
