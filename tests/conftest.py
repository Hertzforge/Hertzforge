"""Shared fixtures for the package's tests.

The test data is under `testdata/` at the repository root. The small
fixtures are in the tree; the plate's runs and models (55 MB) are fetched
by `scripts/fetch_testdata.sh`, and a test that needs them is skipped with
that command named when they are not there.
"""

from __future__ import annotations

import os

import pytest

TESTDATA = os.path.join(os.path.dirname(__file__), '..', 'testdata')
PLATE = os.path.join(TESTDATA, 'plate')
FETCH = 'scripts/fetch_testdata.sh'


def fixture_path(*parts):
    """A path into testdata/. Not named test* — pytest would collect it."""
    if parts and parts[0] == 'plate' and not os.path.isdir(PLATE):
        pytest.skip(f'the plate test data is not fetched; run {FETCH}')
    return os.path.join(TESTDATA, *parts)


def banded_pair_on_target(cut_at=1000.0, per_octave=6):
    """(banded specification, banded measurement sitting exactly on its
    target): the plate's target cut off above `cut_at`, mid-band, so the
    last written band is only partly covered, banded, and a measurement
    whose matched records carry the banded target itself. Any mark on it
    is a false one."""
    import copy

    import numpy as np

    import hertzforge
    from hertzforge.core.compliance import matched_records

    loaded = hertzforge.import_file(fixture_path('plate', 'random.nc4'))
    spec = copy.deepcopy(loaded['Random_specification'])
    beyond = np.asarray(spec.abscissa) > cut_at
    spec.ordinate = np.where(beyond[None, :], np.nan, np.asarray(spec.ordinate))
    spec.limits = {name: np.where(beyond[None, :], np.nan, np.asarray(values))
                   for name, values in spec.limits.items()}
    # the measurement carries the narrowband target on the lines it is
    # written, and nothing beyond, so the two band onto the very same
    # bins, the end band of each cut at the same place
    narrow = loaded['time_data'].compute_psds()
    narrow.ordinate[:, ~spec.written()] = np.nan
    for _label, si, mi in matched_records(spec, narrow):
        target = np.real(spec.ordinate[si])
        narrow.ordinate[mi] = np.where(np.isfinite(target) & (target > 0),
                                       target, np.nan)
    banded = spec.to_octave(per_octave)
    measured = narrow.to_octave(per_octave)
    measured.scale_db = 0
    return banded, measured


def plate_geometry_and_shapes():
    """The plate's geometry in meters and its truth shapes."""
    import hertzforge

    return (hertzforge.import_file(fixture_path('plate', 'geometry.npz'),
                                   length_unit='m'),
            hertzforge.import_file(fixture_path('plate', 'shapes.npy')))


@pytest.fixture
def project(survey):
    """The plate's geometry, shapes and FRFs as a project dict, what a
    report is generated from."""
    import hertzforge

    shapes, frfs = survey
    return {
        'Geometry': hertzforge.import_file(fixture_path('plate', 'geometry.npz')),
        'Shape Set': shapes,
        'FRF': frfs,
    }


@pytest.fixture
def survey():
    """(truth shapes, matching FRFs): fixtures born from one model, so a
    fit or a synthesis can be checked against exact truth. Fresh objects
    per test, since several tests define units on them."""
    import hertzforge

    return (hertzforge.import_file(fixture_path('plate', 'shapes.npy')),
            hertzforge.import_file(fixture_path('plate', 'frfs.npz')))


def pytest_runtest_logstart(nodeid, location):
    """On CI only: the test about to run, on stderr, unbuffered, so a run
    that stops shows which test it stopped in."""
    import sys

    if os.environ.get('CI'):
        sys.stderr.write(f'>> {nodeid}\n')
        sys.stderr.flush()
