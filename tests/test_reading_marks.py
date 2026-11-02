"""The readings the plot bar's toggles draw — averaging frames, shock
windows, the cut, the filter's preview, the octave steps — from a
script, flat and on the 3-D stage (Principle 4: anything the interface
can do the API can do; found missing 2026-10-09).

Each is read from the object as the window reads it, through the same
drawing code: `plot.octave_preview`, `viz.marks.octave_steps`,
`stage_filter_preview` and `add_stage_reading` are what the window's
own previews call.
"""

from __future__ import annotations

import pytest
from conftest import fixture_path

import hertzforge


@pytest.fixture(scope='module')
def run():
    return hertzforge.random_vibration_run(fixture_path('plate',
                                                            'random.nc4'))


@pytest.fixture(scope='module')
def psd():
    return hertzforge.import_file(fixture_path('plate', 'psd.npz'))


@pytest.fixture(scope='module')
def averaged(run):
    """A PSD computed here, so it carries the degrees of freedom of the
    frames it was averaged over."""
    return run.time_history.compute_psds()
