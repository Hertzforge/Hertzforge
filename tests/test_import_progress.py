"""A progress bar for imports, honest about what it knows.

A project file is minutes of someone's day, and its reader is the only
thing that knows how far along it is — so `native.load` reports per
object, a multi-file drop reports per file, and a Rattlesnake run
reports per slab of samples (2026-10-02). A foreign file whose reader
cannot count — a UFF file is one parse — shows no bar: a busy-bar that
cannot move reads as a hang. A lone file's read runs off the event
loop's thread through `_run_long`, so its bar is painted by the loop
turning; the multi-file drop still ticks from the loop's own thread,
where the tick repaints and pumps without user input, and reopening
the loop mid-import is the re-entrancy that once crashed a drop.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge import io


@pytest.fixture
def project_file(tmp_path):
    """A three-object project on disk."""
    geometry = hertzforge.Geometry(
        node_id=[1, 2, 3], node_xyz=[[0, 0, 0], [1, 0, 0], [2, 0, 0]],
        length_unit='m')
    time = hertzforge.TimeHistory(
        np.arange(8) / 256.0, np.ones((1, 8)), response_dof=['101Z+'])
    from hertzforge.core.channel_table import ChannelTable

    table = ChannelTable({'channel': [1], 'node': ['101'],
                          'direction': ['Z+'], 'unit': ['g']})
    path = tmp_path / 'survey.vdyn'
    io.save_test(path, 'Survey', {'Geometry': geometry, 'Time': time,
                                  'Channels': table})
    return str(path)


def test_the_loader_reports_per_object(project_file):
    ticks = []
    io.load(project_file, progress=lambda done, total: ticks.append(
        (done, total)))
    assert ticks == [(0, 3), (1, 3), (2, 3), (3, 3)], (
        'once up front, once per object')


def test_a_single_object_file_reports_nothing(tmp_path):
    """One object is one step, and a bar with one step is a light bulb."""
    geometry = hertzforge.Geometry(
        node_id=[1], node_xyz=[[0, 0, 0]], length_unit='m')
    path = tmp_path / 'geometry.vdyn'
    io.save(geometry, path)
    ticks = []
    io.load(str(path), progress=lambda *call: ticks.append(call))
    assert ticks == []


def test_the_nc4_reader_counts_its_samples():
    heard = []
    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'),
                                        progress=lambda d, t: heard.append((d, t)))
    history = loaded['time_data']
    assert heard, 'nothing heard'
    assert heard[-1] == (history.ordinate.shape[1], history.ordinate.shape[1])
    done = [d for d, _t in heard]
    assert done == sorted(done) and all(d <= t for d, t in heard)


def test_a_reader_without_a_progress_hook_is_not_handed_one():
    """A UFF file is one parse; the dispatcher must not pass it a
    keyword it does not take."""
    loaded = hertzforge.import_file(fixture_path('plate', 'geometry.unv'),
                                        progress=lambda d, t: None)
    plain = hertzforge.import_file(fixture_path('plate', 'geometry.unv'))
    assert isinstance(loaded, hertzforge.Geometry)
    assert loaded.num_nodes == plain.num_nodes == 169, 'read whole, as without it'
