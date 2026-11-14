"""Reports read run records: each of the nine golden run folders
(`schema/golden/runs/`, written by the control service's own tests on the
simulator) converts with `hertzforge.io.run_convert`, and its report's
runs block, its chronology and its template's blocks carry what the run
record holds: the level and level changes, the outcome in the record's
words, the operator, the article, the specification document and clause,
the counts each test keeps, SIM labeled SIM, and units on every number.
A project with no run record reports as before. Each block is also
falsified: a record changed on purpose changes what the block says, and
a record that lacks a fact makes the block say so."""

from __future__ import annotations

import copy
import json
import os
import re
import shutil

import numpy as np
import pytest

import hertzforge
from hertzforge import Project
from hertzforge.core.data import TimeHistory
from hertzforge.core.report import TEMPLATE_BUILDERS, TEST_TEMPLATES
from hertzforge.core.run_record import RunRecord
from hertzforge.io import run_convert
from hertzforge.report import render_html
from hertzforge.report import run_records as rec
from hertzforge.report import runs as rr

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GOLDEN = os.path.join(ROOT, 'schema', 'golden', 'runs')

#: what each golden run's record holds, as the report must say it
EXPECTED = {
    'random': {'clause': 'clause 3.1', 'level': 0.0,
               'levels': '-3 dB at 15.0 s, 0 dB at 17.0 s',
               'counts': ('11 control loops', '2 spectra kept'),
               'outcome': 'completed: Test complete: 0.5 s held at full level'},
    'mimo_random': {'clause': 'clause 4.1', 'level': None,
                    'levels': 'no level change in the run record',
                    'counts': ('3 control loops', '3 law calls'),
                    'outcome': 'completed: The run completed its time at full level'},
    'sine': {'clause': 'clause 3.2', 'level': 0.0, 'levels': '0 dB at 18.0 s',
             'counts': ('3 sweep passes',), 'outcome': 'completed: Test complete'},
    'mixed': {'clause': 'clause 3.4', 'level': 0.0, 'levels': '0 dB at 9.0 s',
              'counts': (), 'outcome': 'completed: Test complete: the schedule finished'},
    'classical_shock': {'clause': 'clause 4.2', 'level': 0.0,
                        'levels': 'no level change in the run record',
                        'counts': ('2 pulses fired', '2 pulses passed', '2 pulses logged'),
                        'outcome': 'completed: The schedule finished'},
    'srs': {'clause': 'clause 4.2', 'level': 0.0, 'levels': '0 dB at 16.0 s',
            'counts': ('2 pulses fired', '1 pulse passed'),
            'outcome': 'completed: The schedule finished'},
    'twr': {'clause': 'clause 4.2', 'level': 0.0,
            'levels': 'no level change in the run record',
            'counts': ('4 plays', '1 playback play', '2 law calls'),
            'outcome': 'completed: The playback schedule finished: 1 play at full level'},
    'recorder': {'clause': 'clause 4.2', 'level': None,
                 'levels': 'no level change in the run record',
                 'counts': ('1 recording',),
                 'outcome': 'stopped by the operator: Stopped by Shell'},
    'explorer': {'clause': 'clause 4.2', 'level': None,
                 'levels': 'no level change in the run record',
                 'counts': ('2 steering actions',),
                 'outcome': 'stopped by the operator: Stopped by Shell'},
}


@pytest.fixture(scope='module')
def projects(tmp_path_factory):
    out = {}
    for name in EXPECTED:
        folder = str(tmp_path_factory.mktemp('runs') / name)
        shutil.copytree(os.path.join(GOLDEN, name), folder)
        out[name] = Project.open(run_convert.convert(folder).path)
    return out


def _payload(html):
    data = html.split('<script id="data" type="application/json">')[1]
    return json.loads(data.split('</script>')[0].replace('<\\/', '</'))


def _page(project, report):
    html = render_html(report, project, object_groups=project.object_groups,
                       unit_system=hertzforge.SI)
    blocks = _payload(html)['blocks']
    text = ' '.join(re.sub(r'<[^>]+>', ' ', b.get('html', '')) for b in blocks
                    if b['kind'] == 'text')
    return html, blocks, re.sub(r'\s+', ' ', text)


def _report(project):
    record = project['Run Record']
    return TEMPLATE_BUILDERS[TEST_TEMPLATES[record.test]](project, project.object_groups)


# ---- the chronology and the runs block ---------------------------------------

@pytest.mark.parametrize('name', sorted(EXPECTED))
def test_the_chronology_reads_the_run_record(projects, name):
    project, expected = projects[name], EXPECTED[name]
    found = rr.chronology(project)
    # one run, however many streams the run has
    assert len(found.rows) == 1
    row = found.rows[0]
    assert row.record is not None
    assert set(row.together) | {row.name} == set(rr.histories(project))
    text = rr.draft(found)
    assert expected['outcome'] in text
    assert 'operator Operator; article Bracket; specification Example procedure, ' \
        f'{expected["clause"]}' in text
    assert 'SIM (simulator)' in text
    assert f'levels {expected["levels"]}' in text
    for count in expected['counts']:
        assert count in text
    assert 'Each run is read from the record the control service kept' in text
    # nothing is read from the signal: no running-RMS reading is claimed
    assert 'running RMS' not in text and 'Read from' not in text
    level = '' if expected['level'] is None else '0 dB'
    assert f'| {row.identity.run} |  | {level} |' in text


@pytest.mark.parametrize('name', sorted(EXPECTED))
def test_the_runs_block_takes_the_level_from_the_record(projects, name):
    project, expected = projects[name], EXPECTED[name]
    first = rr.histories(project)[0]
    found = rr.identity(project, first)
    assert found.level_db == expected['level']
    if expected['level'] is not None:
        assert found.source('level_db') == 'controller'
        assert '0 dB' in rr.heading(first, found)
    outcome = rr.outcomes(project)[first]
    assert outcome.text.startswith(expected['outcome'])


def test_a_project_with_no_run_record_reports_as_before(projects):
    """The same histories without their record are read from their signal
    and name, with the table and words of a project that never had one."""
    project = projects['random']
    bare = Project()
    for name in ('inputs', 'drives'):
        bare[name] = copy.deepcopy(project[name])
    found = rr.chronology(bare)
    assert len(found.rows) == 2 and all(row.record is None for row in found.rows)
    text = rr.draft(found)
    assert 'run record' not in text and 'Read from' not in text
    assert 'The time at level is read from the running RMS' in text
    assert rr.outcomes(bare) == {}
    # the level of a record with no run record is never the controller's
    assert rr.identity(bare, 'inputs').source('level_db') != 'controller'
    # and no Run section is added to a template's report
    report = TEMPLATE_BUILDERS['random'](bare, bare.object_groups)
    assert not any(b.get('text', '').startswith('## The Run\n') for b in report.blocks)


def test_runs_with_and_without_a_record_say_which_each_was_read_from(projects):
    project = projects['random']
    mixed = copy.deepcopy(project)
    mixed['extra'] = copy.deepcopy(project['inputs'])
    mixed['extra'].run_identity = None
    mixed.set_run_identity('extra', run='2')
    mixed.set_run_identity('inputs', start='')
    text = rr.draft(rr.chronology(mixed))
    assert '| Read from |' in text
    assert '| run record |' in text and '| signal levels |' in text
    assert 'A run marked "signal levels" has no run record' in text


# ---- the templates' blocks ---------------------------------------------------

@pytest.mark.parametrize('name', sorted(EXPECTED))
def test_each_template_carries_the_record_facts(projects, name):
    project, expected = projects[name], EXPECTED[name]
    report = _report(project)
    html, _blocks, text = _page(project, report)
    assert 'RunRecord' not in re.sub(r'class="[^"]*"', '', text)
    assert '{{@basis:RunRecord' not in html
    assert 'Operator: Operator' in text and 'Article: Bracket' in text
    assert f'Example procedure, {expected["clause"]}' in text
    assert 'Source: SIM (simulator)' in text
    assert expected['outcome'] in text
    for count in expected['counts']:
        assert count in text


def test_the_mixed_template_shows_each_part_with_its_unit(projects):
    _html, blocks, text = _page(projects['mixed'], _report(projects['mixed']))
    assert 'narrowband 1 at 419.375 Hz: 0.002134 g^2/Hz measured, 0.0025 g^2/Hz its ' \
        'reference, error -0.86 dB (at 5.75 s from engage, 0 dB)' in text
    assert 'the floor: 0.3147 g rms measured, 0.4472 g rms its reference' in text
    assert 'tone 1: no point recorded' in text
    captions = [b.get('caption', '') for b in blocks]
    assert any(c.startswith('Floor with the tones taken out') for c in captions)
    assert any(c.startswith('Random part of the drive') for c in captions)


def test_the_explorer_template_gives_the_survey_and_steering(projects):
    _html, _blocks, text = _page(projects['explorer'], _report(projects['explorer']))
    assert '1 point (200 Hz 0 g peak); the highest 0 g peak at 200 Hz' in text
    assert 'at 3.0 s: steer: 0.1 V pk' in text
    assert 'channel 1 0 g rms' in text and 'measured frequency not recorded' in text


def test_the_recorder_template_gives_each_recording(projects):
    _html, _blocks, text = _page(projects['recorder'], _report(projects['recorder']))
    assert 'rec_0001: trigger level, 300 frames, 0.3 s, 100 frames before its trigger, ' \
        '0 gaps' in text
    assert 'row 1: mean 0 g, rms 0 g, peak 0 g' in text
    assert 'row 1 overload: 0.4755 reached 0.45' in text


def test_the_mimo_template_binds_its_owed_blocks_to_the_run(projects):
    project = projects['mimo_random']
    report = _report(project)
    _html, blocks, text = _page(project, report)
    assert 'condition number (dimensionless) of the identified transfer function over 4 ' \
        'lines runs from 4.709 to 6.926 at 166 Hz; the gate allowed at most 1000, and ' \
        'passed' in text
    assert "the control law's seed, before the loop engaged, asked for Shaker 1 0.4847 V " \
        'rms (ceiling 5 V rms)' in text
    assert 'over the run the drives played Shaker 1 0.5049 V rms' in text
    assert 'Control 1 2.736 g rms against its reference 2.401 g rms, +1.33 dB' in text
    assert any(b.get('caption', '').startswith('Drive spectra') for b in blocks)
    owed = [b['text'] for b in report.blocks if b['kind'] == 'text'
            and b['text'].startswith(('## Drives', '## Conditioning'))]
    assert owed and not any('NOT YET WRITTEN' in t for t in owed)


def test_the_drive_prediction_is_the_seed_diagonal(projects):
    """The seed's rms, read by hand from the packed matrix: the first,
    fifth, eighth and tenth packed rows are the diagonal."""
    record = projects['mimo_random']['Run Record']
    seed = record.arrays['law_calls']['drive'][0]
    hand = np.sqrt(np.real(seed[[0, 4, 7, 9]]).sum(axis=1) * 2.0)
    text = rec.drive_prediction_text(record)
    for value in hand:
        assert f'{value:.4g} V rms' in text


def test_the_mimo_shock_template_says_what_waits_for_its_record(projects):
    project = projects['classical_shock']
    report = TEMPLATE_BUILDERS['mimo_shock'](project, project.object_groups)
    _html, blocks, text = _page(project, report)
    assert 'Verdicts NOT YET WRITTEN' in text
    assert 'which the control service does not write yet' in text
    assert any(b.get('caption', '').startswith('Measured shock response spectra')
               for b in blocks)
    # an empty project keeps the outline
    outline = TEMPLATE_BUILDERS['mimo_shock']({}, None)
    assert any('{{@basis:RunRecord.operator}}' in b.get('text', '')
               for b in outline.blocks)


@pytest.mark.parametrize('key', ['mixed_mode', 'explorer', 'recorder', 'mimo_shock'])
def test_the_new_templates_are_registered(key):
    from hertzforge.service import reports

    assert key in TEMPLATE_BUILDERS and key in reports.TEMPLATES
    assert key in TEST_TEMPLATES.values()


def test_the_service_picks_the_template_of_the_run(projects):
    from hertzforge.service import reports
    from hertzforge.service.protocol import ProtocolError

    project = copy.deepcopy(projects['recorder'])
    name = reports.generate(project, {'template': 'run', 'name': 'From the run'})
    assert project[name].title == 'Recording Report'
    with pytest.raises(ProtocolError, match='no run record'):
        reports.generate(Project(), {'template': 'run'})


# ---- falsifications: the blocks follow the record -----------------------------

def _changed(project, change):
    other = copy.deepcopy(project)
    change(other['Run Record'])
    return other


def test_a_changed_operator_article_and_clause_change_every_block(projects):
    def change(record):
        record.manifest['operator'] = 'Second Shift'
        record.manifest['article'] = 'Housing'
        record.manifest['specification'] = {'document': 'Other procedure', 'clause': '9'}
    project = _changed(projects['random'], change)
    text = rr.draft(rr.chronology(project))
    assert 'operator Second Shift; article Housing; specification Other procedure, ' \
        'clause 9' in text
    _html, _blocks, page = _page(project, _report(project))
    assert 'Operator: Second Shift' in page and 'Bracket' not in page


def test_a_record_with_no_end_says_the_run_did_not_end_cleanly(projects):
    def change(record):
        record.end = None
        record.manifest['complete'] = False
    project = _changed(projects['srs'], change)
    text = rr.draft(rr.chronology(project))
    assert 'did not end cleanly: the run record holds no end' in text
    assert 'completed' not in text


def test_a_trip_and_a_hold_in_the_log_are_interruptions(projects):
    def change(record):
        record.log.append({'seq': 99, 'at_unix': record.log[-1]['at_unix'] + 1,
                           'frame': None, 'kind': 'state', 'text': 'running to held',
                           'data': {'from': 'running', 'to': 'held'}})
        record.log.append({'seq': 100, 'at_unix': record.log[-1]['at_unix'] + 1,
                           'frame': None, 'kind': 'trip', 'text': 'drive over its abort',
                           'data': {'level_db': -6.0}})
    project = _changed(projects['random'], change)
    text = rr.draft(rr.chronology(project))
    assert 'hold at 23.0 s: running to held; trip at 24.0 s: drive over its abort' in text


def test_a_record_with_no_identification_leaves_the_conditioning_owed(projects):
    def change(record):
        record.arrays.pop('identification')
        record.arrays.pop('law_calls')
        record.end = {**record.end, 'report': None}
    project = _changed(projects['mimo_random'], change)
    report = _report(project)
    conditioning = next(b['text'] for b in report.blocks
                        if b.get('text', '').startswith('## Conditioning'))
    assert 'NOT YET WRITTEN' in conditioning
    drives = next(b['text'] for b in report.blocks
                  if b.get('text', '').startswith('## Drives'))
    assert 'drive prediction, from the run record' not in drives


def test_a_hardware_run_is_not_labeled_sim_and_no_label_is_never_guessed(projects):
    hardware = _changed(projects['random'], lambda r: setattr(r, 'source', 'HARDWARE'))
    text = rr.draft(rr.chronology(hardware))
    assert 'Random run r-0001, HARDWARE;' in text and 'SIM' not in text
    unlabeled = _changed(projects['random'], lambda r: setattr(r, 'source', None))
    assert 'source not stated in the run record' in rr.draft(rr.chronology(unlabeled))


def test_a_history_with_no_group_has_no_record():
    history = TimeHistory(abscissa=np.arange(4) / 4.0, ordinate=np.zeros((1, 4)),
                          response_dof=['1X+'], ordinate_dim=['acceleration'],
                          ordinate_unit=['g'])
    project = Project()
    project['h'] = history
    project['Run Record'] = RunRecord({'test': 'random'}, {}, {})
    assert rec.record_of(project, 'h') is None


def test_a_band_of_a_few_lines_has_no_octave_scale_to_read(projects):
    """A run controlled on four lines inside one third-octave band has no
    octave cells to read a scale from: the reading says none, where it
    raised before and took the whole MIMO report with it."""
    from hertzforge.core.compliance import comparison_scale_db, read_scale_db

    project = projects['mimo_random']
    spec, measured = project['Specification'], project['control_spectra']
    assert read_scale_db(spec, measured) is None
    assert comparison_scale_db(spec, measured) == 0
