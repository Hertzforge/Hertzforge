"""The shock view: the button, the windows on the plot, the table.

The same shape as the averaging view, and tested the same way — the
parameters live on the history, the view shows and edits them, and the
calculator uses whatever is there when it runs.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.data import Srs, TimeHistory
from hertzforge.core.shocks import Shock


def _history_with_shocks():
    import numpy as np

    from hertzforge.core.shocks import Shock

    t = np.linspace(0.0, 3.0, 3072)
    rng = np.random.default_rng(2)
    values = 0.01 * rng.standard_normal((2, 3072))
    history = TimeHistory(t, values, response_dof=['101Z+', '102Z+'],
                          ordinate_dim=['acceleration'] * 2,
                          ordinate_unit=['m/s**2'] * 2)
    history.shocks = (Shock(0.5, 0.1), Shock(1.5, 0.1), Shock(2.5, 0.1))
    return history


def test_the_report_carries_the_shock_windows():
    """Brandon: the shock report showed the trace and not the bands the
    spectra came from. The SRS figures beside it are one curve per
    event, so 'shock 2' has to point at something findable."""
    from hertzforge.report import _plot_block

    history = _history_with_shocks()
    built = _plot_block({'kind': 'plot', 'source': 'T', 'mode': 'curves'},
                        history, {}, hertzforge.SI)
    marks = built['shocks']
    assert len(marks['windows']) == 3
    assert marks['windows'][0] == [0.5, 0.6], 'start and end, in seconds'
    assert '3 shocks' in marks['label']


def test_a_history_with_no_shocks_carries_no_windows():
    from hertzforge.report import _plot_block

    history = _history_with_shocks()
    history.shocks = None
    built = _plot_block({'kind': 'plot', 'source': 'T', 'mode': 'curves'},
                        history, {}, hertzforge.SI)
    assert 'shocks' not in built


def test_the_page_draws_what_the_payload_carries():
    """The canvas is JavaScript and cannot import anything, so the two
    halves drift silently — which is exactly how the MAC figure kept
    its old stripes for a week. This holds the drawing to the field."""
    from hertzforge.report.page import _JS

    assert 'block.shocks' in _JS, 'the page reads the field'
    assert 'block.shocks.windows' in _JS, 'and draws the windows'


def _bounded_spec_and_events():
    """A one-channel shock specification with its band, and a measured
    SRS carrying two events of that channel."""
    from hertzforge.core.data import ShockSpecification

    frequencies = np.geomspace(10.0, 1000.0, 40)
    level = np.full(40, 100.0)
    spec = ShockSpecification(
        abscissa=frequencies, ordinate=level[None, :],
        response_dof=['101Z+'], ordinate_dim='acceleration',
        ordinate_unit='m/s**2',
        abort_upper=level[None, :] * 2.0,
        abort_lower=level[None, :] * 0.5)
    measured = Srs(
        abscissa=frequencies,
        ordinate=np.vstack([level * 0.8, level * 1.1]),
        response_dof=['101Z+', '101Z+'],
        ordinate_dim=['acceleration'] * 2,
        ordinate_unit=['m/s**2'] * 2,
        block=['shock 1', 'shock 2'])
    return spec, measured


def test_figure_2_carries_the_tolerance_band():
    """Brandon: Figure 2 does not look like it shows the SRS limits.
    The report gated the band on `Specification` — the PSD kind — and
    a shock specification is `Bounded, Srs`: same four limit curves,
    different spectrum underneath. The gate reads the mixin now."""
    from hertzforge.report import _plot_block

    spec, _measured = _bounded_spec_and_events()
    built = _plot_block({'kind': 'plot', 'source': 'S', 'mode': 'curves'},
                        spec, {}, hertzforge.SI)
    zones = built['channels'][0]['zones']
    assert zones, 'the band is in the payload'
    assert {z['severity'] for z in zones} == {'abort'}, \
        'this specification wrote abort limits and nothing else'


def test_figure_3_is_the_events_over_the_specification():
    """Brandon: Figure 3 should show the control channel SRS on top of
    the specification — the reading the app gives when the two are
    selected together. Every event of the channel, the target and its
    band behind, no comparison scaling and no outside marks: a
    spectrum sitting low is the finding, and with several events
    overlaid a mark cannot say whose line went out."""
    from hertzforge.report import _plot_block

    spec, measured = _bounded_spec_and_events()
    built = _plot_block(
        {'kind': 'plot', 'source': 'M', 'mode': 'curves',
         'specification': 'S'},
        measured, {'S': spec}, hertzforge.SI)
    channel = built['channels'][0]
    assert [r['label'] for r in channel['responses']] == \
        ['shock 1', 'shock 2'], 'every event, named as the app names it'
    assert channel['zones'], 'the band rides behind them'
    assert 'over' not in channel and 'under' not in channel, \
        'no outside marks over stacked events'
    assert built['curves'][0].get('gray'), 'the target is the reference'


def test_the_shock_template_names_the_specification():
    from hertzforge.core.report import shock_template

    project = hertzforge.Project()
    report = shock_template(project, object_groups=[])
    srs_blocks = [b for b in report.blocks
                  if b.get('source') == '@basis:Srs'
                  and b.get('kind') == 'plot']
    assert srs_blocks and srs_blocks[0].get('specification') == \
        '@basis:ShockSpecification'


def test_the_page_draws_the_response_list():
    """The canvas cannot import the field name — the same seam the
    shock windows and the MAC checker drifted through."""
    from hertzforge.report.page import _JS

    assert 'ch.responses' in _JS


def test_with_added_places_a_window_in_the_largest_gap():
    from hertzforge.core.shocks import with_added

    grown = with_added((Shock(1.0, 0.5), Shock(4.0, 0.5)), 10.0)
    assert len(grown) == 3
    new = next(s for s in grown if s.start not in (1.0, 4.0))
    assert new.duration == pytest.approx(0.5), "the series' own length"
    assert 4.5 < new.start and new.stop < 10.0, \
        'in the stretch after the last window — the largest gap'
    assert new.start == pytest.approx(4.5 + (5.5 - 0.5) / 2), 'centered'


def test_with_added_serves_the_empty_list():
    from hertzforge.core.shocks import with_added

    (only,) = with_added((), 20.0)
    assert only.duration == pytest.approx(2.0), 'a tenth of the record'
    assert (only.start + only.stop) / 2 == pytest.approx(10.0), 'centered'


def test_with_added_shrinks_into_a_tight_gap():
    from hertzforge.core.shocks import with_added

    grown = with_added((Shock(0.0, 4.0), Shock(5.0, 5.0)), 10.0)
    new = [s for s in grown if s.duration != 4.0 and s.duration != 5.0]
    assert len(new) == 1 and new[0].duration == pytest.approx(0.9), \
        'shrunk to nine tenths of the only gap'
    assert new[0].start >= 4.0 and new[0].stop <= 5.0


def test_with_added_refuses_a_wall_to_wall_record():
    from hertzforge.core.shocks import with_added

    packed = (Shock(0.0, 5.0), Shock(5.0, 5.0))
    assert with_added(packed, 10.0) == packed
