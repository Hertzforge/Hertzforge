"""A recorded run's events reach the run log as proposals the log keeper
confirms (`core.run_log_events`): every event of each golden run folder's
log is offered, with its time and facts, nothing is written until it is
accepted, an accepted event is written at the controller's time and
cannot be written twice, and a log already in the run log's own format
beside the run is read as it is."""

from __future__ import annotations

import copy
import datetime as dt
import os
import shutil

import pytest

from hertzforge import Project
from hertzforge.core import run_log_events as events
from hertzforge.core.run_log import RunLog
from hertzforge.io import run_convert

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GOLDEN = os.path.join(ROOT, 'schema', 'golden', 'runs')
#: every golden run folder, a module's own or one of the runs a module keeps several of
NAMES = sorted(os.path.relpath(top, GOLDEN) for top, _, files in os.walk(GOLDEN)
               if 'run.json' in files)


@pytest.fixture(scope='module')
def records(tmp_path_factory):
    out = {}
    for name in NAMES:
        folder = str(tmp_path_factory.mktemp('runs') / name)
        shutil.copytree(os.path.join(GOLDEN, name), folder)
        out[name] = Project.open(run_convert.convert(folder).path)['Run Record']
    return out


@pytest.mark.parametrize('name', NAMES)
def test_every_event_and_the_end_is_offered(records, name):
    record = records[name]
    offered = events.offer(record)
    assert len(offered) == len(record.log) + 1
    zone = dt.timezone(dt.timedelta(seconds=record.manifest['start']['utc_offset_s']))
    for proposal, entry in zip(offered, record.log, strict=False):
        assert proposal.event_time == dt.datetime.fromtimestamp(entry['at_unix'], zone)
        assert proposal.text.startswith(entry['text'].strip())
        assert f'controller log entry {entry["seq"]}, {entry["kind"]}]' in proposal.marker
        assert 'SIM' in proposal.marker
    end = offered[-1]
    assert end.kind == 'run_stop'
    assert end.fields['result'] == record.outcome


def test_levels_and_pulses_are_offered_with_their_facts(records):
    levels = [p for p in events.offer(records['random']) if p.kind == 'level_row']
    assert [p.fields['level_db'] for p in levels] == [-3.0, 0.0]
    shots = [p for p in events.offer(records['srs']) if p.kind == 'shot']
    assert [(p.fields['n'], p.fields['level_db'], p.fields['in_tolerance'])
            for p in shots] == [(1, -6.0, False), (2, 0.0, True)]
    # a kind the run log has no kind for is a note, never dropped
    notes = [p for p in events.offer(records['explorer']) if 'intent]' in p.marker]
    assert len(notes) == 2 and all(p.kind == 'note' for p in notes)


def test_a_trip_is_offered_as_a_controller_stop(records):
    record = copy.deepcopy(records['random'])
    record.log.append({'seq': 50, 'at_unix': 1794240030.25, 'frame': 4000, 'kind': 'trip',
                       'text': 'drive over its abort', 'data': {'level_db': 0.0}})
    trip = [p for p in events.offer(record) if p.kind == 'abort']
    assert len(trip) == 1 and trip[0].fields == {'stop': 'controller ramp', 'level_db': 0.0}


def test_nothing_is_written_until_the_keeper_accepts(records, tmp_path):
    with RunLog.create(tmp_path / 'floor.hflog', who=('keeper', 'bench')) as log:
        offered = events.offer(records['random'])
        assert log.entries == ()
        level = next(p for p in offered if p.kind == 'level_row')
        entry = events.accept(log, level, author='KK', test=None, run=1)
        assert entry.kind == 'level_row' and entry.fields['level_db'] == -3.0
        assert entry.event_time == level.event_time
        assert len(log.entries) == 1
        with pytest.raises(events.RunEventsError, match='holds this event already'):
            events.accept(log, level, author='KK')
        stop = offered[-1]
        events.accept(log, stop, author='KK', run=1)
    reread = RunLog.read(tmp_path / 'floor.hflog')
    assert [e.kind for e in reread.entries] == ['level_row', 'run_stop']
    assert reread.entries[1].fields['result'] == 'completed'


def test_an_event_with_no_time_needs_one(records, tmp_path):
    record = copy.deepcopy(records['random'])
    record.log[0]['at_unix'] = None
    first = events.offer(record)[0]
    assert first.event_time is None
    with RunLog.create(tmp_path / 'floor.hflog', who=('keeper', 'bench')) as log:
        with pytest.raises(events.RunEventsError, match='no time'):
            events.accept(log, first, author='KK')
        when = dt.datetime(2026, 11, 9, 9, 0, tzinfo=dt.UTC)
        assert events.accept(log, first, author='KK', event_time=when).event_time == when


def test_a_log_in_its_own_format_is_read_not_offered(records, tmp_path):
    folder = tmp_path / 'run'
    folder.mkdir()
    kind, held = events.offered_or_read(records['random'], folder)
    assert kind == 'offered' and len(held) == len(records['random'].log) + 1
    with RunLog.create(folder / 'run.hflog', who=('keeper', 'bench')) as log:
        log.append('note', author='KK', text='kept beside the run')
    kind, held = events.offered_or_read(records['random'], folder)
    assert kind == 'read' and held[0].entries[0].text == 'kept beside the run'
