"""The run log's verbs for a recorded run's events (`runlog.run_events`,
`runlog.accept_event`): a converted golden run's events are listed with
nothing written, one is written only when accepted with initials, never
twice, and never into a log that is not open for writing."""

from __future__ import annotations

import os
import shutil

import pytest

from hertzforge import Project
from hertzforge.io import run_convert
from hertzforge.service import run_logs
from hertzforge.service.protocol import ProtocolError

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
GOLDEN = os.path.join(ROOT, 'schema', 'golden', 'runs', 'random')


@pytest.fixture(scope='module')
def project(tmp_path_factory):
    folder = str(tmp_path_factory.mktemp('runs') / 'random')
    shutil.copytree(GOLDEN, folder)
    return Project.open(run_convert.convert(folder).path)


@pytest.fixture
def keeper(tmp_path):
    keeper = run_logs.Keeper()
    keeper.new(str(tmp_path / 'floor.hflog'), 'Bracket')
    yield keeper
    keeper.close()


def call(keeper, project, method, **params):
    return run_logs.request(keeper, method, params, project)


def test_the_events_are_listed_and_nothing_is_written(keeper, project):
    listed = call(keeper, project, 'runlog.run_events', record='Run Record')
    events = listed['events']
    assert len(events) == len(project['Run Record'].log) + 1
    levels = [e for e in events if e['kind'] == 'level_row']
    assert [e['fields']['level_db'] for e in levels] == [-3.0, 0.0]
    assert levels[0]['label'] == 'Level' and levels[0]['time'].startswith('2026-11-09T09:00')
    assert events[-1]['kind'] == 'run_stop' and events[-1]['fields']['result'] == 'completed'
    assert not any(e['accepted'] for e in events)
    assert keeper.log.entries == ()


def test_an_event_is_written_once_when_accepted(keeper, project):
    index = next(e['index'] for e in call(keeper, project, 'runlog.run_events',
                                          record='Run Record')['events']
                 if e['kind'] == 'level_row')
    with pytest.raises(ProtocolError, match='initials'):
        call(keeper, project, 'runlog.accept_event', record='Run Record', index=index)
    assert keeper.log.entries == ()
    done = call(keeper, project, 'runlog.accept_event', record='Run Record', index=index,
                author='KK', run=1)
    assert done['entry'] == keeper.log.entries[0].id
    assert keeper.log.entries[0].kind == 'level_row'
    listed = call(keeper, project, 'runlog.run_events', record='Run Record')['events']
    assert listed[index]['accepted'] and sum(e['accepted'] for e in listed) == 1
    with pytest.raises(ProtocolError, match='holds this event already'):
        call(keeper, project, 'runlog.accept_event', record='Run Record', index=index,
             author='KK')
    assert len(keeper.log.entries) == 1


def test_the_verbs_refuse_what_they_cannot_take(keeper, project):
    with pytest.raises(ProtocolError, match='not a run record'):
        call(keeper, project, 'runlog.run_events', record='inputs')
    with pytest.raises(ProtocolError, match='index'):
        call(keeper, project, 'runlog.accept_event', record='Run Record', index=999,
             author='KK')
    closed = run_logs.Keeper()
    with pytest.raises(ProtocolError, match='No log is open'):
        call(closed, project, 'runlog.accept_event', record='Run Record', index=0,
             author='KK')
    # listing needs no log: nothing in it is accepted
    assert not any(e['accepted'] for e in
                   call(closed, project, 'runlog.run_events', record='Run Record')['events'])
