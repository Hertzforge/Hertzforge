"""The session journal: every act, as the line that replays it.

Piece one of the console (Brandon, 2026-08-30): the project records
its own verbs — clicked in the GUI or called from a script, they are
the same calls — as runnable Python, in memory, for this sitting. The
anchor test replays `session_script` with exec and compares projects;
it is what keeps the journal honest forever.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge


def _worked_up():
    project = hertzforge.Project('Journaled')
    project.import_file(fixture_path('plate', 'modal.nc4'))
    project.compute_psds('Time History')
    project.rename('Time History PSDs', 'PSDs')
    return project


def test_each_verb_records_one_runnable_line():
    project = _worked_up()
    assert project.journal[0] == "project = hertzforge.Project('Journaled')"
    assert project.journal[1] == \
        f"project.import_file({fixture_path('plate', 'modal.nc4')!r})"
    assert "project.compute_psds('Time History')" in project.journal
    assert "project.rename('Time History PSDs', 'PSDs')" in project.journal


def test_a_verb_calling_verbs_records_once():
    """merge removes, adds and links; the journal says merge."""
    project = _worked_up()
    before = len(project.journal)
    said = [line for line in project.journal if 'add(' in line]
    assert not said, 'the adds inside import and compute stayed quiet'
    assert len(project.journal) == before


def test_an_object_argument_records_as_its_name():
    """`project.compute_psds(project.basis.time_history)` is how the
    console will often be driven; the journal writes the name the verb
    resolved, which is what a script would say."""
    project = hertzforge.Project('N')
    project.import_file(fixture_path('plate', 'modal.nc4'))
    project.set_basis(*project.names)
    project.compute_psds(project.basis.time_history)
    assert "project.compute_psds('Time History')" in project.journal


def test_a_refusal_records_nothing():
    project = hertzforge.Project('R')
    before = list(project.journal)
    with pytest.raises(KeyError, match='nothing here'):
        project.compute_psds('nothing here')
    assert project.journal == before, 'a verb that raised changed nothing'


def test_opening_a_file_is_the_whole_genesis(tmp_path):
    project = _worked_up()
    path = project.save(tmp_path / 'j.vdyn')
    back = hertzforge.Project.open(path)
    assert back.journal == [
        f'project = hertzforge.Project.open({path!r})'], \
        'whatever the loader did on the way, the story starts here'


def test_the_script_replays_the_session(tmp_path):
    """The anchor: exec the script, get the same project."""
    project = _worked_up()
    project.save(tmp_path / 'out.vdyn')
    script = project.session_script()
    assert script.startswith('import hertzforge\n')

    room: dict = {}
    exec(script, room)                                  # noqa: S102
    replayed = room['project']
    assert replayed.names == project.names
    for name in project.names:
        ours, theirs = project[name], replayed[name]
        assert type(ours) is type(theirs), name
        if hasattr(ours, 'ordinate'):
            assert np.allclose(np.asarray(ours.ordinate),
                               np.asarray(theirs.ordinate)), name
    assert (tmp_path / 'out.vdyn').exists(), \
        'the save replayed too — the script writes what the session wrote'


def test_a_setting_records_as_the_assignment_a_script_makes():
    from hertzforge.core.averaging import Averaging

    project = hertzforge.Project('S')
    project.import_file(fixture_path('plate', 'modal.nc4'))
    framing = Averaging(frame_length=4096, frames=6, window='hann')
    project['Time History'].averaging = framing
    project.record_setting(project['Time History'], 'averaging', framing)
    assert project.journal[-1] == \
        f"project['Time History'].averaging = {framing!r}"


def test_nudging_a_setting_settles_to_one_line():
    """A drag session is many writes to the same slot; the journal
    keeps the assignment that stands, not a line per pixel."""
    from hertzforge.core.truncate import Truncation

    project = hertzforge.Project('S')
    project.import_file(fixture_path('plate', 'modal.nc4'))
    history = project['Time History']
    before = len(project.journal)
    for stop in (0.5, 0.4, 0.3):
        project.record_setting(history, 'truncation', Truncation(0.0, stop))
    assert len(project.journal) == before + 1
    assert 'Truncation(start=0.0, stop=0.3)' in project.journal[-1]


def test_the_script_replays_the_settings_too(tmp_path):
    """The anchor, piece-two edition: a framing stored by assignment
    shapes the PSDs the replay computes."""
    from hertzforge.core.averaging import Averaging

    project = hertzforge.Project('S')
    project.import_file(fixture_path('plate', 'modal.nc4'))
    history = project['Time History']
    framing = Averaging(frame_length=4096, frames=6, window='hann')
    history.averaging = framing
    project.record_setting(history, 'averaging', framing)
    project.compute_psds('Time History')

    script = project.session_script()
    assert 'from hertzforge.core.averaging import Averaging' in script
    room: dict = {}
    exec(script, room)                                  # noqa: S102
    replayed = room['project']
    assert replayed['Time History'].averaging == framing
    assert np.allclose(
        np.asarray(project['Time History PSDs'].ordinate),
        np.asarray(replayed['Time History PSDs'].ordinate)), \
        'the framing shaped the replayed PSDs exactly'


def test_deleting_a_capture_renumbers_the_survivors():
    """'avg 1, avg 3' after deleting the second capture is a gap the
    tree's columns faithfully showed (Brandon, 2026-08-30). Block
    labels renumber by the merge rule — bookkeeping, not identity —
    so the survivors read contiguous."""
    import numpy as np

    t = np.arange(256) / 256.0
    history = hertzforge.TimeHistory(
        t, np.ones((6, 256)),
        response_dof=['1Z+', '2Z+'] * 3,
        block=['avg 1', 'avg 1', 'avg 2', 'avg 2', 'avg 3', 'avg 3'])
    history.delete_records(capture=1)
    assert history.block == ['avg 1', 'avg 1', 'avg 2', 'avg 2'], \
        'the third capture became the second'
    assert history.capture_indices() == [0, 0, 1, 1]
