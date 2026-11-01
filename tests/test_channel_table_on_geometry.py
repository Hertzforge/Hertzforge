"""A channel table selected with a geometry draws the table's DOFs.

Brandon, 2026-10-22: with an empty channel table to fill in, selecting
it beside its geometry should show where the channels are, and a
drop-down on the 3-D view's bar should pick the references, the
responses or the monitors. The arrows turn on for the pair (and back
to as they were after it), and the drop-down opens on every channel.
"""

from __future__ import annotations

from hertzforge.core.channel_table import ChannelTable


def _table():
    return ChannelTable({
        'channel': [1, 2, 3, 4],
        'node': [101, 102, 103, ''],
        'direction': ['Z+', 'X+', 'Y+', ''],
        'unit': ['g', 'lbf', 'g', ''],
        'role': ['response', 'reference', 'monitor', 'response'],
        'control': ['False', 'True', 'True', 'False']})
