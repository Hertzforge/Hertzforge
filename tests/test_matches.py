"""Matched modes: a project object of their own.

Committed MAC picks live in a MatchedModes object — saveable,
renameable, row-deletable — never in a report's private note. The sets
are referenced by name and read live; the MAC values are stored with
the picks, because the displayed comparison may have been projected.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge import io
from hertzforge.core.matches import MatchedModes
from hertzforge.core.shapes import ShapeSet


def test_add_dedupes_and_delete_removes():
    matched = MatchedModes('A', 'B', pairs=[[0, 0]], macs=[0.9])
    matched.add([[1, 2], [0, 0]], [0.8, 0.95])
    assert matched.pairs == [[0, 0], [1, 2]]
    assert matched.macs == [0.95, 0.8], (
        'recommitting a pair restates its MAC, never doubles the row')
    matched.delete_matches([0])
    assert matched.pairs == [[1, 2]] and matched.macs == [0.8]
    with pytest.raises(ValueError, match='no match at row'):
        matched.delete_matches([5])


def test_matches_ride_the_project_file(tmp_path):
    a = ShapeSet([10.0, 20.0], [0.01, 0.02], ['1X+', '2X+'],
                 np.ones((2, 2)))
    b = ShapeSet([10.5], [0.015], ['1X+', '2X+'], [[1.0, -1.0]])
    matched = MatchedModes('First', 'Second',
                           pairs=[[0, 0], [1, 0]], macs=[0.98, 0.42])
    path = tmp_path / 'matched.vdyn'
    io.save_test(str(path), 'Matched', {'First': a, 'Second': b,
                                        'Matched Modes': matched})
    contents = io.load(str(path))
    back = contents['Matched Modes']
    assert isinstance(back, MatchedModes)
    assert back.first == 'First' and back.second == 'Second'
    assert back.pairs == [[0, 0], [1, 0]]
    assert back.macs == pytest.approx([0.98, 0.42])


def test_the_report_cross_mac_stripes_the_matched_pairs():
    """A cross-MAC block whose two sets have a MatchedModes object
    carries the committed pairs, mapped through name order and the
    tall transpose, so the report stripes them like the GUI."""
    import json

    from hertzforge.core.report import Report
    from hertzforge.report import render_html

    a = ShapeSet([10.0, 20.0], [0.01, 0.01], ['1X+', '2X+'],
                 np.eye(2))
    b = ShapeSet([10.0, 20.0, 30.0], [0.01, 0.01, 0.01],
                 ['1X+', '2X+'], [[1.0, 0.0], [0.0, 1.0], [1.0, 1.0]])
    matched = MatchedModes('A', 'B', pairs=[[0, 0], [1, 2]],
                           macs=[1.0, 0.5])
    objects = {'A': a, 'B': b, 'Matched': matched}
    report = Report('r', [{'kind': 'plot', 'mode': 'mac',
                           'source': 'A', 'shapes': 'B',
                           'caption': ''}])
    payload = json.loads(render_html(report, objects).split(
        'type="application/json">')[1].split('</script>')[0])
    mac = payload['blocks'][0]
    # B has more modes: the grid transposed, and the pairs with it
    assert len(mac['rows']) == 3 and len(mac['columns']) == 2
    assert sorted(mac['pairs']) == [[0, 0], [2, 1]]
    # the block bound the sets the other way around: still marked
    swapped = Report('r', [{'kind': 'plot', 'mode': 'mac',
                            'source': 'B', 'shapes': 'A',
                            'caption': ''}])
    payload = json.loads(render_html(swapped, objects).split(
        'type="application/json">')[1].split('</script>')[0])
    assert sorted(payload['blocks'][0]['pairs']) == [[0, 0], [2, 1]]


def test_matches_are_kept_in_the_first_sets_order():
    """A matched table is read down the page against the mode list beside
    it. Pairs arrive in the order squares were clicked on the MAC, which
    is the order somebody's eye moved — and since a ShapeSet is always in
    frequency order, sorting on the first set's mode index is sorting by
    its frequency."""
    from hertzforge.core.matches import MatchedModes

    matched = MatchedModes('Truth', 'Fitted',
                           pairs=[[5, 2], [0, 0], [3, 1]],
                           macs=[0.7, 0.99, 0.85])
    assert matched.pairs == [[0, 0], [3, 1], [5, 2]]
    assert matched.macs == [0.99, 0.85, 0.7], 'each MAC went with its pair'


def test_a_match_added_later_lands_in_its_place():
    """Committing another square must not append it to the bottom: the
    table would then read in click order, which is no order at all."""
    from hertzforge.core.matches import MatchedModes

    matched = MatchedModes('Truth', 'Fitted', pairs=[[0, 0], [6, 3]],
                           macs=[0.99, 0.8])
    matched.add([[2, 1]], [0.95])
    assert matched.pairs == [[0, 0], [2, 1], [6, 3]]
    assert matched.macs == [0.99, 0.95, 0.8]
    # and restating one already there leaves the order alone
    matched.add([[2, 1]], [0.9])
    assert matched.pairs == [[0, 0], [2, 1], [6, 3]]
    assert matched.macs == [0.99, 0.9, 0.8]


def test_one_mode_matched_twice_reads_low_to_high():
    from hertzforge.core.matches import MatchedModes

    matched = MatchedModes('Truth', 'Fitted', pairs=[[4, 9], [4, 2]],
                           macs=[0.6, 0.9])
    assert matched.pairs == [[4, 2], [4, 9]], 'the second index breaks ties'
