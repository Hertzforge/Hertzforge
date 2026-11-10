"""The fixture generators finish: what they save is a format that can be saved."""

from __future__ import annotations

import os
import sys

import hertzforge
from hertzforge.core.channel_table import ChannelTable

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'testdata'))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..'))


def test_the_plate_generator_writes_its_channel_table_as_hforge(tmp_path, monkeypatch):
    from testdata import generate_plate

    monkeypatch.setattr(generate_plate, 'OUT', tmp_path)
    generate_plate.main()
    table = hertzforge.load(str(tmp_path / 'channel_table.hforge'))
    assert isinstance(table, ChannelTable)
    assert table.num_channels == 29
    assert not list(tmp_path.glob('*.vdyn'))
