"""Fingerprints recorded by older code still read (2026-09-08).

A drone shock project saved in August carried a filter fingerprint of
two values — `(corner, order)`, the shape `Filtering` had before its
edges were named — and `Project.stale()` raised on it, which would have
taken the window down on load. A fingerprint is a record of a
computation, and the reading of it must never refuse.
"""

from __future__ import annotations

import numpy as np

import hertzforge
from hertzforge.project import _named_state, _summarize_state


def _project():
    p = hertzforge.Project()
    t = np.arange(0, 1, 1 / 1024.0)
    run = hertzforge.TimeHistory(
        t, np.random.default_rng(0).normal(size=(1, t.size)),
        response_dof=['1Z+'], ordinate_dim=['acceleration'])
    p.add('Run', run)
    p.add('Filtered', run)
    return p


def test_a_two_value_filter_fingerprint_is_the_low_pass_it_was():
    assert _named_state('filtering', [819.2, 4]) == \
        {'low': None, 'high': 819.2, 'order': 4}
    assert 'low-pass' in _summarize_state(('filtering', [819.2, 4])).lower()
    assert '819.2' in _summarize_state(('filtering', [819.2, 4]))
    p = _project()
    p.provenance['Filtered'] = {'verb': 'filter_data', 'source': 'Run',
                                'state': ['filtering', [819.2, 4]]}
    stale = p.stale()          # raised before: low above high
    assert 'Filtered' in stale
    assert '819.2' in stale['Filtered'] and 'no filter' in stale['Filtered']


def test_a_fingerprint_the_class_refuses_is_still_a_story():
    p = _project()
    p.provenance['Filtered'] = {
        'verb': 'filter_data', 'source': 'Run',
        'state': ['filtering', {'low': 819.2, 'high': 4.0, 'order': 4}]}
    story = p.stale()['Filtered']
    assert 'low 819.2' in story and 'high 4.0' in story, \
        'said as its fields, since no filter can be built from them'
