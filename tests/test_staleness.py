"""Derived objects know when their source's settings have moved.

Brandon's design (2026-08-23): recomputing is an act, never
automatic — a report must not rewrite itself — so a settings change
marks the downstream objects stale, a refresh badge says why, and one
click recomputes in place. Fingerprints of the settings, never
timestamps: the badge appears exactly when the numbers would differ,
and survives a save and a load.
"""

from __future__ import annotations

from dataclasses import replace

import pytest
from conftest import fixture_path

import hertzforge


@pytest.fixture
def worked_up():
    project = hertzforge.Project('t')
    project.import_file(fixture_path('plate', 'modal.nc4'))
    psds = project.compute_psds('Time History')
    return project, psds


def test_fresh_is_fresh(worked_up):
    project, _psds = worked_up
    assert project.stale() == {}


def test_a_framing_change_marks_the_psds(worked_up):
    project, psds = worked_up
    history = project['Time History']
    history.averaging = replace(history.averaging, frames=3)
    stale = project.stale()
    assert list(stale) == [psds]
    assert 'now says 3 frames' in stale[psds]


def test_a_shock_change_marks_the_srs_and_only_the_srs(worked_up):
    from hertzforge.core.shocks import Shock

    project, psds = worked_up
    history = project['Time History']
    history.shocks = (Shock(0.1, 0.2),)
    srs = project.compute_srs('Time History')
    assert project.stale() == {}, 'freshly computed'
    history.shocks = (Shock(0.1, 0.2), Shock(0.6, 0.2))
    stale = project.stale()
    assert srs in stale and psds not in stale, \
        'the fingerprint follows the dependency: shocks never mark PSDs'
    assert '2 windows' in stale[srs]


def test_refresh_recomputes_in_place_and_references_hold(worked_up):
    project, psds = worked_up
    project.link('Time History', psds)
    history = project['Time History']
    history.averaging = replace(history.averaging, frames=4)
    before = project[psds]
    project.refresh(psds)
    assert project.stale() == {}
    after = project[psds]
    assert after is not before, 'genuinely recomputed'
    assert psds in project, 'under its own name'
    assert any(psds in group['members'] for group in project.object_groups), \
        'the link held through the refresh'
    assert len(after.abscissa) == history.averaging.frame_length // 2 + 1


def test_the_cascade_flows_by_content(worked_up):
    project, psds = worked_up
    octave = project.compute_octave(psds)
    history = project['Time History']
    history.averaging = replace(history.averaging, frames=6)
    assert list(project.stale()) == [psds], \
        'the octave banding is honest until its own source changes'
    project.refresh(psds)
    assert list(project.stale()) == [octave], \
        'refreshing the PSDs is what stales the banding of them'
    project.refresh(octave)
    assert project.stale() == {}


def test_refresh_stale_settles_the_whole_chain(worked_up):
    project, psds = worked_up
    octave = project.compute_octave(psds)
    history = project['Time History']
    history.averaging = replace(history.averaging, frames=8)
    done = project.refresh_stale()
    assert set(done) == {psds, octave}
    assert project.stale() == {}


def test_staleness_survives_the_file(worked_up, tmp_path):
    project, psds = worked_up
    history = project['Time History']
    history.averaging = replace(history.averaging, frames=3)
    path = str(tmp_path / 't.vdyn')
    project.save(path)
    back = hertzforge.Project.open(path)
    stale = back.stale()
    assert list(stale) == [psds]
    back.refresh(psds)
    assert back.stale() == {}


def test_a_rename_follows_into_the_provenance(worked_up):
    project, psds = worked_up
    project.rename('Time History', 'Recording')
    project.rename(psds, 'Densities')
    history = project['Recording']
    history.averaging = replace(history.averaging, frames=3)
    assert list(project.stale()) == ['Densities']
    project.refresh('Densities')
    assert project.stale() == {}


def test_a_deleted_source_is_not_stale(worked_up):
    project, psds = worked_up
    del project['Time History']
    assert project.stale() == {}, \
        'absence of evidence is not staleness'
    with pytest.raises(ValueError, match='gone'):
        project.refresh(psds)


def test_the_frf_refresh_keeps_its_estimator(worked_up):
    project, _psds = worked_up
    frfs = project.compute_frfs('Time History', 'H1')
    history = project['Time History']
    history.averaging = replace(history.averaging, frames=3)
    assert frfs in project.stale()
    project.refresh(frfs)
    assert frfs not in project.stale(), \
        'refreshed (the PSDs beside it stay honestly stale)'
    assert 'H1' in frfs, 'the choice made once is the choice reused'


def test_a_fingerprint_from_before_a_rename_compares_clean(worked_up):
    """The other way a fingerprint drifts without the settings moving:
    a *rename*. 'rectangle' became 'boxcar' (the scipy spelling), the
    loader normalizes the object's averaging through the class, and
    every project saved before the rename opened with a refresh badge
    whose story named the same window twice (Brandon, 2026-08-30 —
    modal.vdyn did exactly this). Legacy fingerprints go through the
    class's own normalization, positional shape and old spelling both."""
    project, psds = worked_up
    averaging = project['Time History'].averaging
    # what an old file recorded: positional values, pre-rename spelling,
    # fields added since absent — the same computation in old clothes
    project.provenance[psds]['state'] = [
        'averaging', [averaging.frame_length, averaging.overlap,
                      'rectangle', averaging.frames, averaging.start]]
    assert averaging.window == 'boxcar', 'the live object is normalized'
    assert project.stale() == {}, \
        'same computation, older vocabulary: no badge'
