"""The Run Log window's requests: a log kept while a test runs, written
form by form."""

from __future__ import annotations

import asyncio

import pytest

from hertzforge.core.run_log_model import RunLogModel
from hertzforge.service import run_logs
from hertzforge.service.protocol import ProtocolError

from .test_server import _session


@pytest.fixture
def keeper(tmp_path):
    keeper = run_logs.Keeper()
    keeper.new(str(tmp_path / 'shaker.hflog'), 'Bracket')
    yield keeper
    keeper.close()


def call(keeper, method, **params):
    return run_logs.request(keeper, method, params)


def write(keeper, form, author='BZ', axis='Z', **values):
    """Write `form` as a keeper types it: the form opened, its boxes as
    offered with `values` typed over them."""
    spec = call(keeper, 'runlog.form', form=form, axis=axis)
    texts = {f['field']: f['value'] for f in spec['fields'] if f['box'] != 'heading'}
    text = values.pop('text', '')
    texts.update(values)
    return call(keeper, 'runlog.write', form=form, values=texts, text=text, time=spec['time'],
                taken=spec['taken'], author=author, axis=axis)


def opened(keeper):
    return write(keeper, 'open test', test_kind='Random Vibration', title='Z axis random',
                 steps_db='-12, -6, 0', sigma='3', rated_current_peak='60',
                 rail_voltage='120')


def test_a_new_log_opens_with_no_test(keeper):
    state = call(keeper, 'runlog.state')['log']
    assert state['title'] == 'Bracket' and state['writable'] and state['rows'] == []
    assert state['status']['test'] is None and state['holds'] == []


def test_a_test_opened_takes_runs_and_level_rows_up_the_steps(keeper):
    log = opened(keeper)['log']
    assert log['status']['test'] == 1 and log['status']['kind'] == 'Random Vibration'
    assert log['level'] == -12.0, 'the first step of the schedule'
    assert 'sweep' not in log['readings'] and 'notch' in log['readings']
    write(keeper, 'run start', level_db='-12')
    spec = call(keeper, 'runlog.form', form='level row', axis='Z')
    assert spec['live'] and spec['fields'][0]['value'] == '-12'
    said = call(keeper, 'runlog.preview', form='level row', axis='Z',
                values={'level_db': '-12', 'voltage': '20', 'current': '5'})['text']
    assert said.startswith('-12 dB, factor 3.981 to full level: outside')
    assert 'peak voltage: 238.9 V at full level, above the rail of 120 V' in said
    assert said.endswith('Predicted drive: no prediction in a project')
    log = write(keeper, 'level row', level_db='-12', voltage='20', current='5', go='yes')['log']
    assert log['status']['outcome'] == 'running' and log['level'] == -6.0, 'the next step'
    assert log['rows'][-1]['kind'] == 'level_row'


def test_a_bad_box_is_refused_by_its_label_and_writes_nothing(keeper):
    opened(keeper)
    before = len(call(keeper, 'runlog.state')['log']['rows'])
    with pytest.raises(ProtocolError, match='Voltage'):
        write(keeper, 'level row', level_db='-12', voltage='twenty')
    with pytest.raises(ProtocolError, match='Person is needed'):
        write(keeper, 'person in')
    with pytest.raises(ProtocolError, match='initials'):
        write(keeper, 'note', author=' ', text='hello')
    assert len(call(keeper, 'runlog.state')['log']['rows']) == before


def test_an_abort_writes_the_stop_with_it_and_says_what_is_owed(keeper):
    opened(keeper)
    write(keeper, 'run start', level_db='-6')
    done = write(keeper, 'abort', stop='controller ramp', text='overtest on 12')
    kinds = [row['kind'] for row in done['log']['rows']]
    assert kinds[-2:] == ['run_stop', 'abort']
    assert done['said'].startswith(f'Abort #{done["log"]["rows"][-1]["id"]} written. Owed: ')
    assert done['log']['status']['outcome'] == 'aborted'
    assert any('no cause' in hold or 'cause' in hold for hold in done['log']['holds'])
    spec = call(keeper, 'runlog.form', form='cause found')
    refers = next(f for f in spec['fields'] if f['field'] == 'refers_to')
    assert refers['value'] == str(done['log']['rows'][-1]['id']), 'the open abort offered'


def test_a_correction_strikes_the_entry_and_says_what_changed(keeper):
    opened(keeper)
    person = write(keeper, 'person in', person='K. Cross')['log']['rows'][-1]
    spec = call(keeper, 'runlog.form', form='correct', target=person['id'])
    assert spec['target'] == person['id'] and spec['title'].startswith(f'Correct #{person["id"]}')
    texts = {f['field']: f['value'] for f in spec['fields']}
    texts['person'] = 'K. Crosse'
    log = call(keeper, 'runlog.write', form='correct', target=person['id'], values=texts,
               text='misspelled', taken=spec['taken'], author='BZ')['log']
    row = next(r for r in log['rows'] if r['id'] == person['id'])
    assert 'K. Cross' in row['line'], 'shown as written, struck through'
    assert row['corrections'] and 'K. Crosse' in row['corrections'][0]
    with pytest.raises(ProtocolError, match='Nothing was changed'):
        call(keeper, 'runlog.write', form='correct', target=person['id'], values=texts,
             taken=spec['taken'], author='BZ')


def test_a_late_event_is_written_when_it_happened(keeper):
    opened(keeper)
    spec = call(keeper, 'runlog.form', form='note')
    log = call(keeper, 'runlog.write', form='note', values={}, text='door left open',
               time='00:00:01', taken=spec['taken'], author='BZ')['log']
    assert log['rows'][-1]['line'].startswith('00:00:01')
    assert log['rows'][-1]['late'] is not None
    with pytest.raises(ProtocolError, match='not a time of day'):
        call(keeper, 'runlog.write', form='note', values={}, text='x', time='noon',
             taken=spec['taken'], author='BZ')


def test_an_event_time_from_another_zone_is_written_in_this_computers(keeper, monkeypatch):
    import time
    from datetime import UTC, datetime

    if not hasattr(time, 'tzset'):
        pytest.skip('the zone can only be set for a test where time.tzset exists')
    # a computer away from UTC, or a shell's UTC time would pass as local
    monkeypatch.setenv('TZ', 'America/Denver')
    time.tzset()
    opened(keeper)
    write(keeper, 'run start', level_db='-12')
    moment = datetime(2026, 11, 3, 15, 54, 51, tzinfo=UTC)
    log = call(keeper, 'runlog.write', form='level row', values={'level_db': '-12'},
               taken=moment.isoformat().replace('+00:00', 'Z'), author='BZ', axis='Z')['log']
    try:
        assert log['rows'][-1]['line'].startswith('08:54:51'), 'Denver is seven hours behind'
    finally:
        monkeypatch.undo()
        time.tzset()


def test_the_roll_carries_the_header_and_the_header_form_confirms_it(keeper):
    opened(keeper)
    log = write(keeper, 'roll', title='Y axis random', reason='axis done')['log']
    assert log['status']['test'] == 2 and log['tests'][0]['state'] == 'closed to runs'
    spec = call(keeper, 'runlog.form', form='header')
    labels = [f['label'] for f in spec['fields']]
    assert 'Sigma (carried)' in labels and 'Title' in labels, 'the title was changed, not carried'
    assert 'Carried from the last test' in spec['note']
    write(keeper, 'header', confirm='yes')
    spec = call(keeper, 'runlog.form', form='header')
    assert not any(label.endswith('(carried)') for label in
                   (f['label'] for f in spec['fields']))
    with pytest.raises(ProtocolError, match='Nothing was changed'):
        write(keeper, 'header')


def test_an_unplanned_notch_holds_the_test_until_a_deviation_names_it(keeper):
    write(keeper, 'open test', test_kind='Random Vibration', title='Z',
          notches_planned='Z: 180-220 Hz, 6 dB')
    write(keeper, 'run start', level_db='0')
    said = call(keeper, 'runlog.preview', form='notch', axis='Z',
                values={'axis': 'Z', 'band': '400-450', 'depth_db': '3'})['text']
    assert said.startswith('Not planned') and 'Predicted: no prediction' in said
    done = write(keeper, 'notch', band='400-450', depth_db='3')
    notch = done['log']['rows'][-1]['id']
    assert done['said'].startswith(f'Notch #{notch} is not planned')
    spec = call(keeper, 'runlog.form', form='deviation')
    assert next(f for f in spec['fields'] if f['field'] == 'refers_to')['value'] == str(notch)
    assert spec['preview'].startswith(f'Answers the hold at #{notch}')
    log = write(keeper, 'deviation', fact='notch at 420 Hz', agreed_by='KC')['log']
    assert not any(f'#{notch})' in hold for hold in log['holds'])


def test_a_sweep_and_a_shot_read_as_they_are_typed(keeper):
    write(keeper, 'open test', test_kind='Sine Sweep', title='sine', rated_current_rms='40')
    log = write(keeper, 'run start', level_db='0')['log']
    assert 'sweep' in log['readings'] and 'shot' not in log['readings']
    said = call(keeper, 'runlog.preview', form='sweep', axis='Z',
                values={'n': '1', 'of': '2', 'level_db': '0', 'current': '30'})['text']
    assert said.startswith('Sweep 1 of 2. Continuous rating: ')
    write(keeper, 'sweep', direction='up', of='2', level_db='0', current='30')
    spec = call(keeper, 'runlog.form', form='sweep')
    offered = {f['field']: f['value'] for f in spec['fields']}
    assert offered['n'] == '2' and offered['of'] == '2', 'the next sweep, as the last had it'
    with pytest.raises(ProtocolError):
        call(keeper, 'runlog.form', form='nope')


def test_a_log_held_by_another_writer_opens_read_only(keeper):
    path = call(keeper, 'runlog.state')['log']['path']
    other = run_logs.Keeper()
    try:
        log = call(other, 'runlog.open', path=path)['log']
        assert not log['writable'] and log['read_only']
        with pytest.raises(ProtocolError, match='read-only'):
            call(other, 'runlog.write', form='note', values={}, text='x', author='KC',
                 taken='')
        call(keeper, 'runlog.close')
        assert call(keeper, 'runlog.state')['log'] is None
        assert call(other, 'runlog.open', path=path)['log']['writable'], 'the lock released'
    finally:
        other.close()


def test_the_server_keeps_one_log_a_connection_and_releases_it(tmp_path):
    path = str(tmp_path / 'served.hflog')

    async def check(client, socket):
        await socket.recv()        # hello
        made = await client.call('runlog.new', {'path': path, 'title': 'served'})
        assert made['result']['log']['title'] == 'served'
        spec = (await client.call('runlog.form', {'form': 'note'}))['result']
        wrote = await client.call('runlog.write', {
            'form': 'note', 'values': {}, 'text': 'hello', 'time': spec['time'],
            'taken': spec['taken'], 'author': 'BZ'})
        assert wrote['result']['log']['rows'][-1]['kind'] == 'note'
        missing = await client.call('runlog.form', {'form': 'nope'})
        assert missing['error']['code'] == 'bad_request'

    asyncio.run(_session(check))
    other = run_logs.Keeper()
    try:
        assert call(other, 'runlog.open', path=path)['log']['writable'], \
            'the connection closed released its log'
    finally:
        other.close()


def test_the_log_exports_as_text_csv_and_the_machine_log(keeper, tmp_path):
    """`runlog.export`: one view of the open log
    written to a path, the whole log or one test, the view's extension
    added; a view that is not one, no log, or a bad test is refused."""
    opened(keeper)
    write(keeper, 'note', text='shaker warmed up')
    written = call(keeper, 'runlog.export', path=str(tmp_path / 'log'), view='text')['path']
    assert written == str(tmp_path / 'log.txt')
    text = (tmp_path / 'log.txt').read_text(encoding='utf-8')
    assert 'shaker warmed up' in text and 'Z axis random' in text
    csv = call(keeper, 'runlog.export', path=str(tmp_path / 'log.csv'), view='csv', test=1)['path']
    assert (tmp_path / 'log.csv').read_text(encoding='utf-8').startswith('id,kind,')
    machine = call(keeper, 'runlog.export', path=str(tmp_path / 'machine'), view='machine')['path']
    assert csv.endswith('.csv') and machine.endswith('.csv')
    for params, code in (({'path': str(tmp_path / 'x'), 'view': 'pdf'}, 'bad_request'),
                         ({'path': '', 'view': 'text'}, 'bad_request'),
                         ({'path': str(tmp_path / 'x'), 'view': 'text', 'test': 'two'}, 'bad_request'),
                         ({'path': str(tmp_path / 'shaker.hflog'), 'view': 'text'}, 'refused')):
        with pytest.raises(ProtocolError) as error:
            call(keeper, 'runlog.export', **params)
        assert error.value.code == code, params
    keeper.close()
    with pytest.raises(ProtocolError) as error:
        call(keeper, 'runlog.export', path=str(tmp_path / 'x'), view='text')
    assert error.value.code == 'refused'


def closed(keeper):
    opened(keeper)
    write(keeper, 'close test', reason='done')


def test_a_finding_after_a_test_closes_belongs_to_that_test(keeper):
    closed(keeper)
    assert write(keeper, 'deviation', fact='late', agreed_by='EF')['log']['status']['test'] is None
    assert keeper.log.entries[-1].test == 1
    write(keeper, 'anomaly', fact='later', owner='EF')
    assert keeper.log.entries[-1].test == 1
    model = RunLogModel(keeper.log)
    assert [found.entry.fields['fact'] for found in model.findings(1)] == ['late', 'later']


def test_a_finding_after_two_tests_goes_to_the_latest(keeper):
    closed(keeper)
    write(keeper, 'open test', test_kind='Random Vibration', title='Y axis random',
          steps_db='-12, 0', sigma='3', rated_current_peak='60', rail_voltage='120')
    write(keeper, 'close test', reason='done')
    write(keeper, 'deviation', fact='late', agreed_by='EF')
    assert keeper.log.entries[-1].test == 2


def test_a_released_test_refuses_a_finding_from_the_form(keeper):
    closed(keeper)
    write(keeper, 'release', test='1')
    held = len(keeper.log.entries)
    with pytest.raises(ProtocolError, match='test 1 is released'):
        write(keeper, 'deviation', fact='late', agreed_by='EF')
    with pytest.raises(ProtocolError, match='test 1 is released'):
        write(keeper, 'anomaly', fact='later', owner='EF')
    assert len(keeper.log.entries) == held


def test_a_finding_in_a_log_with_no_test_belongs_to_the_log(keeper):
    write(keeper, 'deviation', fact='before any test', agreed_by='EF')
    assert keeper.log.entries[-1].test is None


def test_a_log_is_started_and_opened_at_a_path_from_home(tmp_path, monkeypatch):
    monkeypatch.setenv('HOME', str(tmp_path))
    keeper = run_logs.Keeper()
    assert keeper.new('~/from_home.hflog', 'Home')['log']['title'] == 'Home'
    keeper.close()
    assert (tmp_path / 'from_home.hflog').exists()
    assert keeper.open('~/from_home.hflog')['log']['title'] == 'Home'
    keeper.close()


def test_an_export_of_a_test_the_log_does_not_hold_is_refused(keeper, tmp_path):
    opened(keeper)
    with pytest.raises(ProtocolError, match='no test 99; it holds test 1') as refused:
        call(keeper, 'runlog.export', path=str(tmp_path / 'none.csv'), view='csv', test=99)
    assert refused.value.code == 'refused'
    assert not (tmp_path / 'none.csv').exists()
    assert call(keeper, 'runlog.export', path=str(tmp_path / 'one.csv'), view='csv', test=1)['path']
