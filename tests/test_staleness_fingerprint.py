"""A fingerprint records the settings, not the class that held them.

The staleness badge is only worth anything if it appears exactly when
the numbers would differ. It stopped being worth that the day a sixth
field (`pad`, the zero-padding experiment) was added to `Averaging`
(2026-08-27): the fingerprint was `astuple`, so the extra field
lengthened it, and every project saved before that opened with a
refresh badge on everything derived — over a story that said the same
thing twice, because nothing about the settings had moved.

So the fingerprint names its fields, and a field a recording predates
reads as the class's own default — and, symmetrically, a *stored* key
the class no longer has is ignored. (The `pad` field itself was
removed a day after it was added, once the mode fitter was shown not
to need it; the demonstration projects were regenerated rather than
carrying compatibility for a two-day window. The mechanism stays,
because the next settings field will come someday and this is what
makes adding or removing one survivable.)
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.averaging import Averaging
from hertzforge.core.data import TimeHistory
from hertzforge.core.filters import Filtering


@pytest.fixture
def project():
    t = np.arange(8192) / 1024.0
    history = TimeHistory(t, np.atleast_2d(np.sin(2 * np.pi * 100.0 * t)),
                          response_dof=['101Z+'])
    history.averaging = Averaging(frame_length=1024, overlap=0.5,
                                  window='hann', frames=4)
    project = hertzforge.Project('Fingerprints')
    project.add('Time History', history)
    project.compute_psds('Time History')
    return project


def age(project, name, *values):
    """Rewrite a record's fingerprint the way an older version wrote it:
    positional, and one field short."""
    kind = project.provenance[name]['state'][0]
    project.provenance[name]['state'] = [kind, list(values)]


def test_a_positional_fingerprint_from_an_older_save_is_not_stale(project):
    """The regression itself: the old positional form, compared by
    field name against today's class."""
    age(project, 'Time History PSDs', 1024, 0.5, 'hann', 4, 0.0)
    assert project.stale() == {}


def test_a_real_change_is_still_caught_through_the_old_shape(project):
    age(project, 'Time History PSDs', 1024, 0.5, 'hann', 9, 0.0)
    assert 'Time History PSDs' in project.stale()


def test_a_filtering_fingerprint_survives_the_same_treatment():
    """Filtering is fingerprinted the same way and would break the same
    way; the rule is the shape of the record, not the class."""
    t = np.arange(8192) / 1024.0
    history = TimeHistory(t, np.atleast_2d(np.sin(2 * np.pi * 100.0 * t)),
                          response_dof=['101Z+'])
    history.filtering = Filtering(high=200.0, order=4)
    project = hertzforge.Project('Filtered')
    project.add('Time History', history)
    project.filter_data('Time History')
    assert project.stale() == {}

    name = next(n for n, r in project.provenance.items()
                if r['verb'] == 'filter_data')
    project.provenance[name]['state'] = ['filtering', [None, 200.0, 4]]
    assert project.stale() == {}, 'the positional form still compares'

    history.filtering = Filtering(high=150.0, order=4)
    assert name in project.stale()
    assert '150 Hz' in project.stale()[name]


def test_a_saved_project_opens_with_nothing_stale(tmp_path, project):
    """End to end, because that is how it was found: opening a file and
    seeing badges on work nobody had touched."""
    path = tmp_path / 'fingerprints.vdyn'
    project.save(str(path))
    assert hertzforge.Project.open(str(path)).stale() == {}
