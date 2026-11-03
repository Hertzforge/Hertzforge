"""Report features and the data a report rests on, as operations on a
report generated from the plate's random run: the feature switches, the
editor's panes (requirement, findings, check blocks, dates and
calibration, prediction, runs and chronology, flash report), the data
index, templates, Reports from Runs and run logs."""

from __future__ import annotations

import hashlib
import os
from datetime import datetime, timedelta, timezone
from pathlib import Path

import numpy as np
import pytest

from hertzforge.service import projects
from hertzforge.service.ops_report import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .test_ops import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in OPERATIONS}

HEADER = {'title': 'Random, Z', 'article': 'Unit 7', 'configuration': 'flight',
          'steps_db': [-12, -6, -3, 0], 'sigma': 3.0, 'rated_current_peak': 800.0,
          'rail_voltage': 250.0, 'duration_s': 600.0, 'duration_minus_pct': 0.0,
          'duration_plus_pct': 10.0}


@pytest.fixture
def project():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    found = state.project
    found.compute_psds('Time History')
    found.generate_report('random')
    return found


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()})
    return op.run(project, values)


def rows(reading):
    return {row['label']: row for row in reading['rows']}


def features(project, *keys):
    project.report_features('Report', enable=keys)


def test_the_descriptors_validate(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')
    switches = [p for p in OPS['project.report_features'].params if p.type == 'boolean']
    assert len(switches) == 15 and all(p.setting == f'features.{p.name}' for p in switches)


# ---- features ------------------------------------------------------------------------

def test_features_switch_on_and_off_and_a_preset_replaces_them(project, validate):
    (reading,) = run(project, 'project.report_features', source='Report',
                     skeleton_sections=True, findings=True)
    wire_views([reading], validate)
    found = rows(reading)
    assert found['Structure: Skeleton sections']['value'] == 'on'
    assert found['Results: Findings register']['status'] == 'pass'
    assert found['Structure: Runs and chronology']['value'] == 'off'
    assert project['Report'].features == ['skeleton_sections', 'findings']
    (reading,) = run(project, 'project.report_features', source='Report', preset='test_report')
    assert rows(reading)['Preset']['value'] == 'Test report'
    assert 'flash_report' not in project['Report'].features
    (reading,) = run(project, 'project.report_features', source='Report')
    assert project['Report'].features == [] and rows(reading)['Preset']['value'] == 'Hertzforge default'


def test_a_feature_whose_requirement_is_off_is_refused(project):
    with pytest.raises(ProtocolError, match='needs Skeleton sections'):
        run(project, 'project.report_features', source='Report', findings=True)
    assert project['Report'].features == []


# ---- the requirement -----------------------------------------------------------------

def test_a_requirement_reads_each_criterion(project, validate):
    from hertzforge.report.requirement import read

    features(project, 'test_requirement')
    (reading,) = run(project, 'project.report_requirement', report='Report',
                     document='Test plan 7', dofs='104Z+', grms_percent=10.0)
    wire_views([reading], validate)
    found = rows(reading)
    block = next(b for b in project['Report'].blocks if 'requirement' in b)
    expected = read(block, project, project.object_groups, roles=False)
    assert found['Result']['value'] == expected.state
    assert found['Run']['value'] == 'Time History PSDs'
    assert sum(r['label'] not in ('Specification', 'Run', 'Requirement', 'Result')
               for r in reading['rows']) == len(expected.rows)


def test_a_single_point_requirement_needs_its_channel(project):
    with pytest.raises(ProtocolError, match='name the one meant'):
        run(project, 'project.report_requirement', report='Report', document='Plan')


# ---- findings ------------------------------------------------------------------------

def register(table):
    return {row[0]: dict(zip(table['columns'], row, strict=True)) for row in table['rows']}


def test_a_finding_is_entered_changed_and_removed(project, validate):
    features(project, 'skeleton_sections', 'findings')
    (table,) = run(project, 'project.report_finding', report='Report', kind='anomaly',
                   fact='a rattle at 300 Hz', low=280.0, high=320.0, status='open')
    wire_views([table], validate)
    entry = register(table)['A-1']
    assert entry['Band'] == '280 to 320 Hz' and entry['Owes'] == 'cause, effect, disposition'
    (table,) = run(project, 'project.report_finding', report='Report', number='A-1',
                   cause='a loose bolt')
    assert register(table)['A-1']['Cause'] == 'a loose bolt'
    assert register(table)['A-1']['Fact'] == 'a rattle at 300 Hz'
    (table,) = run(project, 'project.report_finding', report='Report', number='A-1',
                   remove=True)
    assert table['rows'] == []


def test_a_finding_needs_the_register_on(project):
    with pytest.raises(ProtocolError, match='does not have the Findings register'):
        run(project, 'project.report_finding', report='Report', fact='x')


def test_a_fact_is_drafted_off_the_comparison_and_accepted(project, validate):
    from hertzforge.report.findings import fact_from_comparison

    features(project, 'skeleton_sections', 'findings')
    run(project, 'project.report_finding', report='Report')
    reading, table = run(project, 'project.draft_finding_fact', report='Report', number='D-1',
                         dof='101Z+')
    wire_views([reading, table], validate)
    expected = fact_from_comparison(project, 'Time History PSDs', 'Specification', '101Z+',
                                    pair='abort', provenance=project.provenance).text
    assert rows(reading)['Draft']['value'] == expected
    assert '101Z+ above its abort limit' in expected
    assert register(table)['D-1']['Draft'].startswith(expected)
    (table,) = run(project, 'project.accept_finding', report='Report', number='D-1')
    assert register(table)['D-1']['Fact'] == expected and register(table)['D-1']['Draft'] == ''
    with pytest.raises(ProtocolError, match='no fact draft waiting'):
        run(project, 'project.accept_finding', report='Report', number='D-1')


def test_a_draft_needs_its_finding(project):
    features(project, 'skeleton_sections', 'findings')
    with pytest.raises(ProtocolError, match='name the finding'):
        run(project, 'project.draft_finding_fact', report='Report')


def test_a_requirement_row_that_offers_nothing_is_refused(project):
    features(project, 'skeleton_sections', 'findings', 'test_requirement')
    run(project, 'project.report_finding', report='Report')
    run(project, 'project.report_requirement', report='Report', document='Plan', dofs='104Z+')
    with pytest.raises(ProtocolError, match='offers no finding'):
        run(project, 'project.draft_finding_from_requirement', report='Report', number='D-1',
            criterion='1')


def test_a_requirement_draft_needs_its_requirement(project):
    features(project, 'skeleton_sections', 'findings', 'test_requirement')
    run(project, 'project.report_finding', report='Report')
    with pytest.raises(ProtocolError, match='no requirement is bound'):
        run(project, 'project.draft_finding_from_requirement', report='Report', number='D-1',
            criterion='1')


def write_log(path, files=()):
    """A run log of one random test that names `files` and logs a
    deviation."""
    from hertzforge.core import run_log_model as model
    from hertzforge.core.run_log import RunLog

    moment = [datetime(2026, 10, 30, 8, 0, tzinfo=timezone(timedelta(hours=-6)))]

    def clock():
        moment[0] += timedelta(minutes=1)
        return moment[0]

    opened = (RunLog.open(path, clock=clock, who=('tester', 'bench')) if os.path.exists(path)
              else RunLog.create(path, title='Article 7', clock=clock, who=('tester', 'bench')))
    with opened as log:
        if len(log.entries) == 0 or not any(e.kind == 'test_opened' for e in log.entries):
            model.open_test(log, author='AB', test_kind='Random Vibration', header=HEADER,
                            files=list(files))
            model.append(log, 'deviation', author='AB', test=1,
                         fields={'fact': 'a notch at 180 Hz not in the plan', 'agreed_by': 'EF'})
        else:
            log.append('note', author='AB', test=1, text='later')


def test_the_run_logs_findings_are_offered_and_one_accepted(project, tmp_path, validate):
    features(project, 'skeleton_sections', 'findings')
    log = tmp_path / 'article.hflog'
    write_log(log)
    project.attach_run_log(log)
    reading, register_table, offered = run(project, 'project.draft_findings_from_run_log',
                                           report='Report')
    wire_views([reading, register_table, offered], validate)
    assert offered['rows'][0][0] == 'D-1' and register_table['rows'] == []
    (table,) = run(project, 'project.accept_finding', report='Report', number='D-1',
                   offered='run_log')
    assert register(table)['D-1']['Fact'].startswith('a notch at 180 Hz not in the plan')


def test_drafting_from_the_run_log_needs_an_extract(project):
    features(project, 'skeleton_sections', 'findings')
    with pytest.raises(ProtocolError, match='run log extract'):
        run(project, 'project.draft_findings_from_run_log', report='Report')


# ---- check blocks and the report check -------------------------------------------------

def test_a_check_block_holds_its_settings(project, validate):
    (reading,) = run(project, 'project.report_check_block', report='Report', check='pulse',
                     settings='shape="half_sine"; peak=490; duration=0.011')
    wire_views([reading], validate)
    found = rows(reading)
    assert found['Check']['value'] == 'Classical pulse'
    assert found['peak']['value'] == '490' and found['duration']['value'] == '0.011'
    assert found['source']['value'] == '@basis:TimeHistory'
    assert project['Report'].blocks[-1]['settings']['peak'] == 490


def test_a_check_block_missing_a_required_setting_is_refused(project):
    count = project['Report'].num_blocks
    with pytest.raises(ProtocolError, match='needs peak'):
        run(project, 'project.report_check_block', report='Report', check='pulse',
            settings='shape=half_sine; duration=0.011')
    assert project['Report'].num_blocks == count


def test_the_report_check_lists_what_is_owed(project, validate):
    features(project, 'skeleton_sections')
    (reading,) = run(project, 'project.report_check', report='Report')
    wire_views([reading], validate)
    owed = project.report_check('Report')
    assert len(reading['rows']) == len(owed)
    assert rows(reading)['2. Summary']['value'] == 'not yet written'
    project.report_features('Report', disable=['skeleton_sections'])
    with pytest.raises(ProtocolError, match='skeleton'):
        run(project, 'project.report_check', report='Report')


# ---- dates and calibration -------------------------------------------------------------

def test_test_dates_judge_each_sensor(project, validate):
    features(project, 'calibration_in_date')
    (reading,) = run(project, 'project.report_test_date', report='Report', start='2026-10-01',
                     end='2026-10-02')
    wire_views([reading], validate)
    found = rows(reading)
    assert reading['title'].endswith('2026-10-01 to 2026-10-02')
    assert found['Channel 1, 101Z+']['value'] == 'no date given, not checked'
    assert sum(label.startswith('Channel ') for label in found) == 12
    with pytest.raises(ProtocolError, match='YYYY-MM-DD'):
        run(project, 'project.report_test_date', report='Report', start='10/01/2026')


def test_the_calibration_status_is_offered_and_inserted(project, validate):
    from hertzforge.report.skeleton import words

    features(project, 'skeleton_sections', 'calibration_in_date')
    project.report_test_date('Report', '2026-10-01')
    views = run(project, 'project.report_calibration', report='Report', insert=True)
    wire_views(views, validate)
    table = next(view for view in views if view['view'] == 'table')
    assert table['columns'][0] == 'Channel' and len(table['rows']) == 12
    assert table['rows'][0][1] == '101Z+'
    assert words(project['Report'], 'setup') == project.report_calibration_text('Report')
    project.report_features('Report', disable=['calibration_in_date'])
    with pytest.raises(ProtocolError, match='Calibration in date|calibration'):
        run(project, 'project.report_calibration', report='Report')


# ---- prediction ------------------------------------------------------------------------

def test_a_response_is_read_against_its_prediction(project, validate):
    psd = project['Time History PSDs']
    record = next(k for k in range(psd.num_records) if psd.record_label(k).startswith('101Z+'))
    f = np.asarray(psd.abscissa, dtype=float)
    band = (f > 100) & (f < 300)
    peak = float(f[band][np.argmax(np.abs(psd.ordinate[record])[band])])
    project.predict('Predictions', '101Z+', 'asd', [(peak, 1.0, 20.0, (100.0, 300.0))],
                    unit='m/s^2', level_db=0.0)
    features(project, 'prediction')
    views = run(project, 'project.report_prediction', report='Report',
                measured='Time History PSDs')
    wire_views(views, validate)
    table = next(view for view in views if view['view'] == 'table')
    assert table['rows'][0][0].startswith('101Z+')
    assert float(table['rows'][0][4]) == pytest.approx(peak, rel=0.01)
    assert project['Report'].blocks[-1]['kind'] == 'prediction'


def test_an_octave_band_psd_is_not_read_against_a_prediction(project):
    project.predict('Predictions', '101Z+', 'asd', [(150.0, 1.0, 20.0, (100.0, 300.0))],
                    unit='m/s^2', level_db=0.0)
    banded = project.compute_octave('Time History PSDs')
    count = project['Report'].num_blocks
    with pytest.raises(ProtocolError, match='octave'):
        run(project, 'project.report_prediction', report='Report', measured=banded)
    assert project['Report'].num_blocks == count


# ---- runs and chronology ---------------------------------------------------------------

def test_a_runs_axis_is_entered(project, validate):
    features(project, 'skeleton_sections', 'run_sections')
    (table,) = run(project, 'project.report_run_identity', report='Report')
    wire_views([table], validate)
    assert table['rows'] == [['Time History', '', '', '', '', '']]
    (table,) = run(project, 'project.report_run_identity', report='Report', field='axis',
                   value='Z')
    assert table['rows'][0][1] == 'Z'
    (table,) = run(project, 'project.report_run_identity', report='Report', field='level_db',
                   value='-6')
    assert table['rows'][0][3].startswith('-6')


def test_the_runs_need_their_feature(project):
    with pytest.raises(ProtocolError, match='Runs and chronology'):
        run(project, 'project.report_run_identity', report='Report')
    with pytest.raises(ProtocolError, match='Runs and chronology'):
        run(project, 'project.report_runs', report='Report')


def test_results_for_every_run_say_what_was_added(project, validate):
    features(project, 'skeleton_sections', 'run_sections')
    reading, table = run(project, 'project.report_runs', report='Report')
    wire_views([reading, table], validate)
    assert reading['rows'][0]['value'] == project.report_runs('Report')[0]


def test_the_chronology_is_drafted_and_accepted(project, validate):
    features(project, 'skeleton_sections', 'run_sections')
    with pytest.raises(ProtocolError, match='draft'):
        run(project, 'project.accept_chronology', report='Report')
    views = run(project, 'project.report_chronology', report='Report')
    wire_views(views, validate)
    table = views[0]
    row = dict(zip(table['columns'], table['rows'][0], strict=True))
    assert row['Record'] == 'Time History' and row['At level'] == '28.0 s'
    views = run(project, 'project.accept_chronology', report='Report')
    assert views[0]['rows'] == table['rows']
    with pytest.raises(ProtocolError, match='an interruption reaches further below the level'):
        run(project, 'project.report_chronology', report='Report', drop=0.1)


# ---- the flash report ------------------------------------------------------------------

def test_the_flash_reports_typed_parts(project, validate):
    features(project, 'skeleton_sections', 'findings', 'test_requirement', 'report_identity',
             'flash_report')
    (reading,) = run(project, 'project.report_flash', report='Report',
                     condition='released to the customer', opinions='EF: within limits')
    wire_views([reading], validate)
    found = rows(reading)
    assert found['Condition on release']['value'] == 'released to the customer'
    assert found['Opinion, EF']['value'] == 'within limits'
    assert 'Condition on release' not in [line.split(':')[0]
                                          for line in project.report_check('Report')]
    with pytest.raises(ProtocolError, match='also said to be none'):
        run(project, 'project.report_flash', report='Report', opinions_none='none')


def test_the_flash_report_needs_its_feature(project):
    with pytest.raises(ProtocolError, match='flash|Flash'):
        run(project, 'project.report_flash', report='Report', condition='x')


# ---- the data index --------------------------------------------------------------------

def test_a_file_is_attached_checked_and_removed(project, tmp_path, validate):
    path = tmp_path / 'settings.txt'
    path.write_bytes(b'limits as planned\n')
    (table,) = run(project, 'project.index_file', path=str(path), holds='settings list')
    wire_views([table], validate)
    entry = dict(zip(table['columns'], table['rows'][-1], strict=True))
    assert entry['SHA-256'] == hashlib.sha256(b'limits as planned\n').hexdigest()
    assert entry['Bytes'] == '18' and entry['Role'] == 'attached'
    path.write_bytes(b'limits changed\n')
    reading, table = run(project, 'project.verify_data_index')
    wire_views([reading, table], validate)
    assert rows(reading)['settings.txt']['status'] == 'fail'
    assert rows(reading)['random.nc4']['value'] == 'matches'
    (table,) = run(project, 'project.unindex_file', file='settings.txt')
    assert [row[0] for row in table['rows']] == ['random.nc4']
    with pytest.raises(ProtocolError, match='settings.txt'):
        run(project, 'project.unindex_file', file='settings.txt')


def test_a_file_that_is_not_there_is_refused(project, tmp_path):
    with pytest.raises(ProtocolError, match='No such file|not a file|nothing.txt'):
        run(project, 'project.index_file', path=str(tmp_path / 'nothing.txt'), holds='x')


# ---- templates -------------------------------------------------------------------------

def test_a_report_saved_as_a_template_generates_again(project, tmp_path, validate):
    features(project, 'skeleton_sections')
    (reading,) = run(project, 'project.export_report_template', report='Report',
                     path=str(tmp_path / 'random'))
    wire_views([reading], validate)
    written = tmp_path / 'random.vdreport'
    assert rows(reading)['Saved']['value'] == str(written) and written.is_file()
    (reading,) = run(project, 'project.generate_from_template', template=str(written),
                     name='Again')
    made = rows(reading)['Report']['value']
    assert project[made].features == ['skeleton_sections']
    assert rows(reading)['Blocks']['value'] == project['Report'].num_blocks
    with pytest.raises(ProtocolError, match='No such file'):
        run(project, 'project.export_report_template', report='Report',
            path=str(tmp_path / 'missing' / 'random'))
    with pytest.raises(ProtocolError, match='unknown template'):
        run(project, 'project.generate_from_template', template=str(tmp_path / 'none.vdreport'))


# ---- reports from runs -----------------------------------------------------------------

def test_a_run_gets_the_report_its_type_calls_for(project, tmp_path, validate):
    before = list(project)
    (table,) = run(project, 'project.reports_from_runs', runs=str(PLATE / 'random.nc4'),
                   folder=str(tmp_path))
    wire_views([table], validate)
    assert table['rows'] == [['random.nc4', 'Random Vibration', str(tmp_path / 'random.html')]]
    assert (tmp_path / 'random.html').read_text(encoding='utf-8').startswith('<!DOCTYPE html>')
    assert list(project) == before
    with pytest.raises(ProtocolError, match='no one-call report'):
        run(project, 'project.reports_from_runs', runs=str(PLATE / 'modal.nc4'),
            folder=str(tmp_path))


# ---- run logs --------------------------------------------------------------------------

def test_a_run_log_is_attached_indexed_refreshed_and_detached(project, tmp_path, validate):
    data = tmp_path / 'run1.csv'
    data.write_bytes(b'1,2,3\n')
    log = tmp_path / 'article.hflog'
    write_log(log, files=[str(data)])
    (reading,) = run(project, 'project.attach_run_log', path=str(log))
    wire_views([reading], validate)
    found = rows(reading)
    name = reading['title']
    assert found['Test']['value'] == '1: Random, Z (Random Vibration)'
    assert found['Taken to entry']['value'] == 2.0 and found['Findings']['value'] == 1.0
    reading, table = run(project, 'project.index_run_log_files')
    wire_views([reading, table], validate)
    assert rows(reading)['run1.csv']['value'] == 'indexed.'
    assert 'run1.csv' in [row[0] for row in table['rows']]
    write_log(log)
    (reading,) = run(project, 'project.refresh_run_log', run_log=name)
    assert rows(reading)['Taken to entry']['value'] == 3.0
    (reading,) = run(project, 'project.detach_run_log', run_log=name)
    assert name not in project
    with pytest.raises(ProtocolError, match='run log extract'):
        run(project, 'project.index_run_log_files')


def test_a_log_gone_from_its_path_is_not_refreshed(project, tmp_path):
    log = tmp_path / 'article.hflog'
    write_log(log)
    name = project.attach_run_log(log)
    log.unlink()
    with pytest.raises(ProtocolError, match='no run log at'):
        run(project, 'project.refresh_run_log', run_log=name)


def test_a_file_that_is_not_a_run_log_is_refused(project, tmp_path):
    other = tmp_path / 'notes.hflog'
    other.write_text('not a log\n')
    with pytest.raises(ProtocolError):
        run(project, 'project.attach_run_log', path=str(other))
    with pytest.raises(ProtocolError, match='name the run log'):
        run(project, 'project.attach_run_log')
