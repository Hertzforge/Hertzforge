"""The `bars` view and a spectrum's groups (schema/PROTOCOL.md, Views).

What is pinned: the service judges every bar, so the shell computes nothing:
under or over a threshold is out, at one exactly is in unless the reading says
otherwise, a row with no number draws no bar and is out only when the reading
counts a missing number as its worst; and a spectrum's groups name its series
by index. Both travel as the schema says.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.service import views

from .conftest import wire_views


def test_bars_are_judged_against_both_thresholds(validate):
    view = views.bars('RMS Error', 'RMS error', 'dB', ['101X+', '102X+', '103X+', '104X+'],
                      [-4.0, 0.5, 3.0, None], under=-3.0, over=3.0)
    assert view['states'] == ['under', None, None, None]
    assert view['values'] == [-4.0, 0.5, 3.0, None]
    validate(view, 'barsView')


def test_a_ceiling_reached_can_be_out(validate):
    """A waveform error at its ceiling has failed it; a share of a band at its
    ceiling has not."""
    waveform = views.bars('Waveform', 'waveform error', '%', ['a', 'b'], [20.0, 19.0],
                          over=20.0, at_limit='out', floor=0.0)
    lines = views.bars('Lines', 'band outside abort', '%', ['a', 'b'], [5.0, 6.0],
                       over=5.0, floor=0.0)
    assert waveform['states'] == ['over', None]
    assert lines['states'] == [None, 'over']
    assert waveform['floor'] == 0.0
    validate(waveform, 'barsView')


def test_a_missing_number_counts_under_where_the_reading_says_so(validate):
    view = views.bars('Signal to Noise', 'RMS signal to noise', 'dB', ['a', 'b', 'c'],
                      [12.0, float('nan'), 2.0], under=6.0, missing='under',
                      notes=[None, 'at the noise floor', None])
    assert view['values'] == [12.0, None, 2.0]
    assert view['states'] == [None, 'under', 'under']
    assert view['notes'][1] == 'at the noise floor'
    validate(view, 'barsView')


def test_a_kurtosis_grows_from_its_gaussian(validate):
    view = views.bars('Kurtosis', 'Pearson kurtosis', None, ['a'], [4.2], under=2.5,
                      over=3.5, baseline=3.0)
    assert view['baseline'] == 3.0 and view['states'] == ['over']
    assert 'notes' not in view, 'no notes, no field'
    validate(view, 'barsView')


def test_mismatched_bars_are_refused():
    with pytest.raises(ValueError, match='3 values for 2 labels'):
        views.bars('t', 'v', None, ['a', 'b'], [1, 2, 3])
    with pytest.raises(ValueError, match='1 notes for 2 labels'):
        views.bars('t', 'v', None, ['a', 'b'], [1, 2], notes=['x'])


def test_a_spectrum_shows_its_groups_one_at_a_time(validate):
    x = np.linspace(10, 100, 5)
    view = views.spectrum('Control', views.axis('Frequency', 'Hz', 'log'),
                          views.axis('PSD', 'g²/Hz', 'log'),
                          [views.series(f'{dof} {role}', role, x, x)
                           for dof in ('101X+', '102X+')
                           for role in ('reference', 'measured')])
    view = views.grouped(view, [
        {'label': '101X+', 'series': [0, 1],
         'readouts': [{'label': 'RMS error', 'text': '0.4 dB', 'status': 'pass'}]},
        {'label': '102X+', 'series': [2, 3]}])
    assert [g['series'] for g in view['groups']] == [[0, 1], [2, 3]]
    assert 'readouts' not in view['groups'][1]
    wire_views([view], validate)
    with pytest.raises(ValueError, match='outside'):
        views.grouped(view, [{'label': 'x', 'series': [4]}])
