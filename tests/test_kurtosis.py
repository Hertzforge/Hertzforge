"""How Gaussian a record is, per channel.

A spectrum says nothing about the shape of the distribution that
produced it: a smooth hiss and a train of rare hard peaks can have
identical PSDs and fatigue an article differently. These hold the
reading to scipy's own — Pearson, not Fisher — and hold the chart to
the band Brandon asked for (2026-08-24).
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.kurtosis import (
    HIGH,
    LOW,
    NOMINAL,
    channel_kurtosis,
    kurtosis,
)


def test_it_is_pearson_and_agrees_with_scipy():
    """Pearson's kurtosis is 3 for a Gaussian; Fisher's excess form is
    that minus three. The two differ by exactly the number a report
    would be claiming, so this pins which one we compute."""
    scipy_stats = pytest.importorskip('scipy.stats')

    rng = np.random.default_rng(4)
    for values in (rng.standard_normal(20000),
                   rng.uniform(-1, 1, 20000),
                   rng.standard_t(6, 20000),
                   rng.standard_normal(500) * 3.0 + 7.0):
        assert kurtosis(values) == pytest.approx(
            scipy_stats.kurtosis(values, fisher=False, bias=True))


def test_a_gaussian_record_reads_about_nominal():
    rng = np.random.default_rng(11)
    assert kurtosis(rng.standard_normal(200000)) == pytest.approx(
        NOMINAL, abs=0.05)
    assert LOW < NOMINAL < HIGH, 'the band brackets the nominal'


def test_the_shapes_that_matter_land_outside_the_band():
    """The two faults the reading exists to catch: rare hard peaks
    (heavy tails, high) and a clipped or non-random record (low)."""
    rng = np.random.default_rng(5)
    gauss = rng.standard_normal(100000)

    peaky = gauss.copy()
    peaky[::5000] *= 12.0            # a few rare, very hard hits
    assert kurtosis(peaky) > HIGH

    clipped = np.clip(gauss, -1.0, 1.0)
    assert kurtosis(clipped) < LOW

    sine = np.sin(np.linspace(0, 400 * np.pi, 100000))
    assert kurtosis(sine) == pytest.approx(1.5, abs=0.01), \
        'a pure sine is 1.5, the textbook value — and well under the band'


def test_a_dead_channel_has_no_shape_to_describe():
    """NaN, never zero: zero would draw as an impossibly flat
    distribution rather than as the absence of one."""
    assert np.isnan(kurtosis(np.zeros(1000)))
    assert np.isnan(kurtosis([1.0]))
    assert np.isnan(kurtosis([]))


def test_every_channel_gets_one_bar_whatever_it_measures():
    """Kurtosis is dimensionless, so accelerations and forces share an
    axis honestly — the one reading here where mixing quantities is
    not a lie (Brandon, 2026-08-24)."""
    rng = np.random.default_rng(7)
    t = np.arange(4096) / 1024.0
    data = hertzforge.TimeHistory(
        t, rng.standard_normal((3, len(t))),
        response_dof=['9001X+', '101Z+', '104Z+'],
        ordinate_dim=['force', 'acceleration', 'acceleration'],
        ordinate_unit=['N', 'm/s**2', 'm/s**2'])
    rows = channel_kurtosis(data)
    assert len(rows) == 3, 'the force channel is on the chart too'
    assert [label for label, _v in rows] == [
        data.record_label(i) for i in range(3)]
    assert all(abs(value - NOMINAL) < 0.5 for _l, value in rows)
    # and a subset when the caller names one
    assert len(channel_kurtosis(data, [1])) == 1


def test_the_shock_and_modal_reports_do_not_claim_a_band_they_cannot_use():
    """A shock record is a transient in a long quiet stretch and never
    reads near three — the plate's own reads 16 to 27 over its
    windows. A band at 2 to 4 would mark every channel of every shock
    test in red and say nothing anyone could act on, so that report
    does not carry the figure (Brandon, 2026-08-24). Nor does the
    modal report (Brandon, 2026-09-08): a modal test is driven with
    burst random, whose kurtosis read whole is about three over its
    duty and says nothing about the article. The toolbar toggle still
    offers the reading; what a report asserts is the judgment."""
    from hertzforge.core.report import modal_template, shock_template

    for template in (shock_template, modal_template):
        assert not [b for b in template({}).blocks
                    if b.get('mode') == 'kurtosis'], template.__name__


def test_every_other_report_carries_the_reading():
    """A spectrum cannot answer the question it asks, whatever kind of
    test produced the record — wherever the nominal means something."""
    from hertzforge.core import report as templates

    for name in ('random', 'sine', 'sysid', 'transient'):
        blocks = getattr(templates, f'{name}_template')({}).blocks
        kurtosis = [b for b in blocks if b.get('mode') == 'kurtosis']
        assert len(kurtosis) == 1, name
        assert kurtosis[0]['source'] == '@basis:TimeHistory', name
        # a computed judgment, so it sits with the others rather than
        # beside the traces — the standing order
        assert blocks.index(kurtosis[0]) > max(
            (i for i, b in enumerate(blocks)
             if b.get('mode') == 'stage'), default=-1), name


def test_the_report_figure_bands_and_colors_the_bars():
    """Gray inside the band, red above, blue below, with the ground
    past each threshold shaded to match — the same reading the app
    gives and the same one every other bar chart here gives."""
    from hertzforge.core.kurtosis import HIGH, LOW
    from hertzforge.report import _build_block

    rng = np.random.default_rng(9)
    t = np.arange(8192) / 2048.0
    rows = rng.standard_normal((3, len(t)))
    rows[0, ::300] *= 18.0                 # peaky: over the band
    rows[2] = np.clip(rows[2], -0.9, 0.9)  # clipped: under it
    data = hertzforge.TimeHistory(
        t, rows, response_dof=['101Z+', '104Z+', '110Z+'],
        ordinate_dim='acceleration', ordinate_unit='m/s**2')
    built = _build_block({'kind': 'bars', 'mode': 'kurtosis',
                          'source': 'T', 'caption': 'Kurtosis'},
                         {'T': data}, hertzforge.SI, [])
    assert built['kind'] == 'bars'
    assert (built['low'], built['high']) == (LOW, HIGH)
    # the bars grow from the nominal, so what a bar's length says is
    # the departure from Gaussian and its side says which way
    assert built['baseline'] == NOMINAL
    assert built['floor'] is None, 'the axis follows the bars'
    assert 'Pearson' in built['ylabel']
    assert built['values'][0] > HIGH, 'the peaky channel is over'
    assert built['values'][2] < LOW, 'the clipped one is under'
    assert LOW <= built['values'][1] <= HIGH, 'and the plain one is in'


def _quiet_headed(rate=2048.0, seconds=8.0, live=(3.0, 6.0)):
    """A record like a real run: a Gaussian middle with silence before
    and after it, and the analysis pointed at the middle."""
    from hertzforge.core.averaging import Averaging

    rng = np.random.default_rng(1)
    n = int(rate * seconds)
    row = rng.standard_normal(n) * 1e-4          # the silent lead-in
    first, last = int(live[0] * rate), int(live[1] * rate)
    row[first:last] = rng.standard_normal(last - first)
    history = hertzforge.TimeHistory(
        np.arange(n) / rate, np.stack([row]), response_dof=['101Z+'],
        ordinate_dim='acceleration', ordinate_unit='m/s**2')
    history.averaging = Averaging(
        frame_length=2048, frames=5, overlap=0.5, window='hann',
        start=live[0])
    return history


def test_the_reading_covers_what_the_spectra_cover():
    """A system-ID excitation read whole came out at 4.23 where every
    steady frame of it read 2.92 (Brandon, 2026-08-24). Nothing was
    wrong with the arithmetic: the record is only a quarter analyzed,
    and whole it is a mixture of a loud distribution and a quiet one,
    which genuinely is not Gaussian. A number that disagrees with the
    PSD printed beside it is worse than no number."""
    from hertzforge.core.kurtosis import analyzed_span

    history = _quiet_headed()
    whole = kurtosis(np.real(history.ordinate[0]))
    assert whole > HIGH, 'read whole, the silence dominates the shape'

    spans, phrase = analyzed_span(history)
    assert len(spans) == 1 and spans[0][0] > 0
    assert 'averaged over' in phrase
    (_label, value), = channel_kurtosis(history)
    assert value == pytest.approx(NOMINAL, abs=0.2), \
        'over its own frames the record reads Gaussian, as it is'
    assert LOW < value < HIGH


def test_a_shock_record_is_read_over_its_windows():
    """All of them, pooled: the shape of a distribution does not care
    what order its samples arrived in, and the chart is one bar per
    channel (Brandon, 2026-08-24). The silence between events is not
    part of what was analyzed, and read whole it swamps everything."""
    from hertzforge.core.kurtosis import analyzed_span
    from hertzforge.core.shocks import Shock

    rng = np.random.default_rng(8)
    rate, n = 4096.0, 4096 * 4
    row = rng.standard_normal(n) * 1e-3
    for start in (0.5, 1.5, 2.5):
        at = int(start * rate)
        row[at:at + 400] = rng.standard_normal(400)
    history = hertzforge.TimeHistory(
        np.arange(n) / rate, np.stack([row]), response_dof=['101Z+'],
        ordinate_dim='acceleration', ordinate_unit='m/s**2')
    history.shocks = tuple(Shock(start, 400 / rate)
                           for start in (0.5, 1.5, 2.5))
    spans, phrase = analyzed_span(history)
    assert len(spans) == 3, 'every window, not just the first'
    assert 'shock windows' in phrase
    (_label, value), = channel_kurtosis(history)
    assert value == pytest.approx(NOMINAL, abs=0.5), \
        'the events themselves are Gaussian here; the silence is not'
    assert kurtosis(np.real(history.ordinate[0])) > 10, \
        'and read whole, the silence between them swamps it'


def test_a_record_with_neither_is_read_whole():
    """Nothing has said which part matters, so all of it does."""
    from hertzforge.core.kurtosis import analyzed_span

    rng = np.random.default_rng(6)
    history = hertzforge.TimeHistory(
        np.arange(2048) / 1024.0, rng.standard_normal((1, 2048)),
        response_dof=['101Z+'], ordinate_dim='acceleration',
        ordinate_unit='m/s**2')
    spans, phrase = analyzed_span(history)
    assert spans == [(0, 2048)] and phrase == 'over the record'


def test_the_system_id_reads_both_of_its_streams():
    """An ambient recording that is not Gaussian is a noisy room or a
    rattling fixture; a drive that is not is a different fault
    entirely. Both streams get a figure (Brandon, 2026-08-24), the
    quiet one first, as the time data reads."""
    import numpy as np

    from hertzforge.core.report import sysid_template

    t = np.arange(4096) / 1024.0
    rng = np.random.default_rng(12)

    def stream(scale):
        return hertzforge.TimeHistory(
            t, rng.standard_normal((2, len(t))) * scale,
            response_dof=['101Z+', '104Z+'],
            ordinate_dim='acceleration', ordinate_unit='m/s**2')

    objects = {'Quiet': stream(0.01), 'Loud': stream(1.0)}
    blocks = [b for b in sysid_template(objects).blocks
              if b.get('mode') == 'kurtosis']
    assert [b['source'] for b in blocks] == ['Quiet', 'Loud']
    assert 'ambient' in blocks[0]['caption']
    assert 'excitation' in blocks[1]['caption']
    # a spectral package has no streams at all, and keeps one slot
    package = [b for b in sysid_template({}).blocks
               if b.get('mode') == 'kurtosis']
    assert [b['source'] for b in package] == ['@basis:TimeHistory']


def test_a_channel_that_recorded_nothing_is_counted_out_loud():
    """The ambient stream's drive channels are exactly zero — the
    shakers were off — so they have no shape to describe. Eight bars
    from a twelve-channel record otherwise reads as a chart of
    everything."""
    import numpy as np

    from hertzforge.report import _build_block

    t = np.arange(2048) / 1024.0
    rng = np.random.default_rng(13)
    rows = rng.standard_normal((3, len(t)))
    rows[2] = 0.0                      # a dead channel
    data = hertzforge.TimeHistory(
        t, rows, response_dof=['101Z+', '104Z+', '9001X+'],
        ordinate_dim=['acceleration'] * 2 + ['force'],
        ordinate_unit=['m/s**2'] * 2 + ['N'])
    built = _build_block({'kind': 'bars', 'mode': 'kurtosis',
                          'source': 'T', 'caption': 'K'},
                         {'T': data}, hertzforge.SI, [])
    assert len(built['values']) == 2, 'the dead channel has no bar'
    assert '1 channel recorded nothing' in built['caption']


def test_frame_kurtosis_is_kurtosis_row_by_row():
    """Read at once, the frames give the numbers `kurtosis` gives one
    at a time: a frame with a NaN drops it, a dead frame is NaN."""
    from hertzforge.core.kurtosis import frame_kurtosis, kurtosis

    rows = np.random.default_rng(1).standard_normal((40, 300)) ** 3
    rows[5, 7] = np.nan
    rows[9] = 1.0
    one_by_one = np.array([kurtosis(row) for row in rows])
    at_once = frame_kurtosis(rows)
    assert np.isnan(at_once[9]) and not np.isnan(at_once[5])
    np.testing.assert_allclose(at_once, one_by_one, rtol=1e-12)
