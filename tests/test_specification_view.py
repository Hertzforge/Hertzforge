"""A specification on its own: one channel at a time, and its levels.

Six targets and their two dozen limit lines stacked on one axis are
unreadable, so a specification is read the way a comparison is — one
channel drawn, the rest offered in the bar and listed in the table
beneath, and the two pointing at each other.
"""

from __future__ import annotations

import numpy as np
from conftest import fixture_path

import hertzforge


def report_plot():
    from hertzforge.report import _plot_block

    spec = hertzforge.import_file(
        fixture_path('plate', 'random.nc4'))['Random_specification']
    return spec, _plot_block(
        {'kind': 'plot', 'source': 'S', 'mode': 'curves', 'caption': 'Spec'},
        spec, {}, hertzforge.SI)


def test_the_report_plot_stays_on_the_data():
    """It used to plot a specification's zeros as log10(1e-300), which
    is -300 — an axis reaching three hundred decades down to reach
    numbers that were never there."""
    _spec, built = report_plot()
    values = [v for curve in built['curves'] for v in curve['y']
              if v is not None]
    assert min(values) > -20.0
    assert max(values) - min(values) < 10.0


def test_the_report_plot_shades_the_limits_it_claims():
    """Shaded, not drawn as four lines crowding the curve they bound —
    the same reading the app gives. A response is either inside the
    band or it is not, and a filled band says so without asking anyone
    to tell four dashed lines apart."""
    _spec, built = report_plot()
    zones = built['channels'][0]['zones']
    assert [z['severity'] for z in zones] == [
        'warning', 'abort', 'warning', 'abort']
    # the zones past abort have no far side: they run to the plot edge
    assert zones[1]['upper'] is None
    assert zones[3]['lower'] is None


def test_the_report_plot_draws_one_channel_and_offers_the_rest():
    spec, built = report_plot()
    assert spec.num_records > 1
    assert len(built['curves']) == 1, 'one target drawn'
    assert len(built['channels']) == spec.num_records, 'the rest a pick away'
    assert 'drop-down' in built['caption']


def test_every_array_of_a_channel_is_on_one_grid():
    """`_decimate` keeps the extremes of each bin, so which points it
    keeps depends on the values — two curves through it separately come
    back on two different frequency grids, and a band shaded between
    them would be shaded between points that are not above each other."""
    _spec, built = report_plot()
    width = len(built['x'])
    for channel in built['channels']:
        assert len(channel['y']) == width
        for zone in channel['zones']:
            for edge in (zone['lower'], zone['upper']):
                assert edge is None or len(edge) == width


def test_a_specification_with_no_limits_is_still_drawn():
    """Not every target carries bounds, and one that does not is a plain
    curve rather than nothing."""
    from hertzforge.core.data import Specification
    from hertzforge.report import _plot_block

    axis = np.linspace(1.0, 100.0, 64)
    bare = Specification(abscissa=axis,
                         ordinate=np.atleast_2d(np.full(64, 1e-3)),
                         response_dof=['101Z+'],
                         ordinate_dim=['acceleration**2/frequency'])
    built = _plot_block({'kind': 'plot', 'source': 'S', 'mode': 'curves'},
                        bare, {}, hertzforge.SI)
    assert len(built['curves']) == 1
