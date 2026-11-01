"""Saving the whole test as one .vdyn file, and getting it back.

A test file holds every object under the user's own name, in order, plus
what the test itself carried: its name and the active geometry. Loading
one into an untouched window puts everything back exactly; loading into a
project already underway just adds the objects, suffixing collisions.
"""

from __future__ import annotations

from conftest import fixture_path

from hertzforge import io


def test_a_single_object_file_still_loads_as_itself(tmp_path):
    """The container is additive: one-object .vdyn files are untouched."""
    from hertzforge.core.channel_table import ChannelTable

    table = io.load(fixture_path('plate', 'channel_table.vdyn'))
    assert isinstance(table, ChannelTable)
    assert not isinstance(table, io.TestContents)
