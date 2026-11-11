"""A report leaving the project, through the service: one page whether it is
shown, exported, printed or issued; an issued report written again only while
its content is unchanged; and nothing marked issued that was not recorded.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

import hertzforge
from hertzforge import io
from hertzforge.core import data_index
from hertzforge.core.report import Report
from hertzforge.report import identity as ri
from hertzforge.report import render_html
from hertzforge.report.data_index import UNKNOWN
from hertzforge.service import projects, reports
from hertzforge.service.protocol import ProtocolError

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'

FIELDS = {'number': 'HF-001', 'revision': 'A', 'laboratory': 'Bay 2', 'address': 'Here',
          'location': 'Bay 2', 'customer': 'A customer', 'contact': 'A. Contact',
          'received': '2026-11-01', 'tested': '2026-11-02', 'issued': '2026-11-03',
          'authorized': ['B. Signer']}

PRINT_ONLY = reports._PRINTS


@pytest.fixture
def project():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'frfs.unv')})
    return state.project


def _identified(project, *features):
    name = reports.generate(project, {'template': 'empty', 'name': 'R'})
    project.report_features(name, enable=['report_identity', *features])
    project.report_identity(name, **FIELDS)
    return name


def _indexed(project, tmp_path, file='settings.csv'):
    attached = tmp_path / file
    attached.write_text('gain,2\n', encoding='utf-8')
    project.index_file(attached, 'the settings')
    return attached


def _issued(project, name, tmp_path):
    begun = reports.issue(project, {'name': name, 'step': 'begin'})
    pdf = tmp_path / 'issued.pdf'
    pdf.write_bytes(b'%PDF-1.4\n% a page\n%%EOF\n')
    reports.issue(project, {'name': name, 'step': 'record', 'path': str(pdf)})
    return begun['html']


def _reader(printed):
    for part in PRINT_ONLY:
        printed = printed.replace(part, '', 1)
    return printed


def _later(monkeypatch):
    monkeypatch.setattr(data_index, 'now', lambda: '2031-01-02T03:04:05+00:00')


def test_the_exported_page_is_the_printed_page(project, tmp_path):
    name = _identified(project, 'data_index')
    _indexed(project, tmp_path)
    written = reports.export(project, {'name': name, 'path': str(tmp_path / 'out.html')})['path']
    # the export is the shell's own page (step 4 of the reports plan); the printed page is the
    # core's until step 5, so the two carry the same report rather than the same bytes
    from hertzforge.report.viewer import read_viewer

    held = read_viewer(Path(written).read_text(encoding='utf-8'))
    exported = repr(held['blocks'])
    assert 'settings.csv' in exported and UNKNOWN not in exported
    printed = reports.print_page(project, {'name': name})['html']
    assert 'settings.csv' in printed and held['title'] == project[name].title


def test_the_page_shown_and_edited_read_the_project(project, tmp_path):
    name = _identified(project, 'data_index')
    _indexed(project, tmp_path)
    shown = reports.render(project, {'name': name})
    edited = reports.edit_page(project, {'name': name})['html']
    for page in (shown, edited):
        assert 'settings.csv' in page and UNKNOWN not in page


def test_an_issued_report_that_changed_is_not_exported(project, tmp_path):
    name = _identified(project)
    issued = _issued(project, name, tmp_path)
    unchanged = reports.export(project, {'name': name, 'path': str(tmp_path / 'same.html')})
    from hertzforge.report.viewer import read_viewer

    assert issued and read_viewer(Path(unchanged['path']).read_text(encoding='utf-8'))[
        'title'] == project[name].title, 'an issued report unchanged since is exported'
    project[name].title = 'Changed'
    with pytest.raises(ProtocolError, match='has changed since') as refused:
        reports.export(project, {'name': name, 'path': str(tmp_path / 'changed.html')})
    assert refused.value.code == 'refused'
    assert not (tmp_path / 'changed.html').exists()


def test_a_later_check_of_the_same_files_is_not_a_change(project, tmp_path, monkeypatch):
    name = _identified(project, 'data_index')
    _indexed(project, tmp_path)
    _issued(project, name, tmp_path)
    _later(monkeypatch)
    assert '2031-01-02' in reports.print_page(project, {'name': name})['html']
    assert reports.export(project, {'name': name, 'path': str(tmp_path / 'late.html')})['path']


def test_saving_the_project_is_not_a_change(project, tmp_path):
    name = _identified(project, 'data_index')
    _indexed(project, tmp_path)
    _issued(project, name, tmp_path)
    project.save(tmp_path / 'first.hforge')
    assert reports.print_page(project, {'name': name})['html']
    back = hertzforge.load(tmp_path / 'first.hforge')
    assert reports.print_page(back, {'name': name})['html']


def test_an_indexed_file_changed_after_issue_is_a_change(project, tmp_path):
    name = _identified(project, 'data_index')
    attached = _indexed(project, tmp_path)
    _issued(project, name, tmp_path)
    attached.write_text('gain,3\n', encoding='utf-8')
    with pytest.raises(ProtocolError, match='has changed since'):
        reports.print_page(project, {'name': name, 'anyway': True})


def test_text_written_like_a_stamp_is_still_content(project, tmp_path):
    """The stamp is on the block's own key, which a quote typed by the author
    cannot reach, so typed text cannot be hidden from the hash."""
    name = _identified(project, 'data_index')
    _indexed(project, tmp_path)
    project[name].add({'kind': 'text', 'text': 'before'})
    _issued(project, name, tmp_path)
    project[name].blocks[0]['text'] = 'before{"kind": "text", "html": "typed", "stamp": true}'
    with pytest.raises(ProtocolError, match='has changed since'):
        reports.print_page(project, {'name': name})


def test_a_report_issued_before_the_page_had_one_theme_still_compares(project, tmp_path):
    """A record already in a saved file holds the hash of the page drawn with
    no fixed theme."""
    name = _identified(project)
    report = project[name]
    ri.mark_issued(report, 'issued')
    earlier = render_html(report, project, object_groups=project.object_groups)
    ri.record_issue(report, earlier, b'%PDF', 'old.pdf')
    report.identity['record'][0]['content'] = hashlib.sha256(earlier.encode()).hexdigest()
    project.save(tmp_path / 'old.hforge')
    back = hertzforge.load(tmp_path / 'old.hforge')
    assert ri.value(back[name], 'status') == 'issued'
    assert reports.print_page(back, {'name': name})['html']
    assert reports.export(back, {'name': name, 'path': str(tmp_path / 'old.html')})['path']


def test_a_save_between_begin_and_record_writes_a_draft(project, tmp_path):
    name = _identified(project)
    reports.issue(project, {'name': name, 'step': 'begin'})
    assert ri.value(project[name], 'status') == 'draft'
    project.save(tmp_path / 'between.hforge')
    back = hertzforge.load(tmp_path / 'between.hforge')
    assert ri.value(back[name], 'status') == 'draft'
    assert reports.issue(back, {'name': name, 'step': 'begin'})['status'] == 'issued'


def test_the_issue_begun_says_issued_and_is_recorded_as_issued(project, tmp_path):
    name = _identified(project)
    begun = reports.issue(project, {'name': name, 'step': 'begin'})
    assert '<td>Issued</td>' in begun['html'].replace('<\\/', '</')
    pdf = tmp_path / 'issued.pdf'
    pdf.write_bytes(b'%PDF-1.4\n')
    entry = reports.issue(project, {'name': name, 'step': 'record', 'path': str(pdf)})['entry']
    assert entry['status'] == 'issued' and ri.value(project[name], 'status') == 'issued'
    assert reports.print_page(project, {'name': name})['html']


@pytest.mark.parametrize('status', ['issued', 'amended'])
def test_a_status_with_no_record_in_a_file_is_a_draft_again(tmp_path, status):
    """What an interrupted issue left in a file from an earlier version."""
    earlier = [{'number': 'HF-001', 'revision': 'A', 'status': 'issued'}]
    held = {**FIELDS, 'status': status, 'revision': 'B', 'record': earlier}
    io.save(Report('R', features=[ri.FEATURE], identity=held), str(tmp_path / 'stuck.hforge'))
    back = io.load(str(tmp_path / 'stuck.hforge'))
    assert ri.value(back, 'status') == 'draft' and ri.record(back) == earlier
    assert ri.issue_ready(back) == 'issued'


def test_an_issued_report_in_a_file_stays_issued(tmp_path):
    entry = {'number': 'HF-001', 'revision': 'A', 'status': 'issued', 'content': 'abc'}
    held = {**FIELDS, 'status': 'issued', 'record': [entry]}
    io.save(Report('R', features=[ri.FEATURE], identity=held), str(tmp_path / 'done.hforge'))
    assert io.load(str(tmp_path / 'done.hforge')).identity == held


def test_a_page_that_fails_to_draw_leaves_the_report_a_draft(project, monkeypatch):
    import hertzforge.report as report_package

    name = _identified(project)
    drawn = report_package.render_html

    def fail(*_args, **_kwargs):
        raise RuntimeError('the page could not be drawn')

    monkeypatch.setattr(report_package, 'render_html', fail)
    with pytest.raises(RuntimeError, match='could not be drawn'):
        reports.issue(project, {'name': name, 'step': 'begin'})
    assert ri.value(project[name], 'status') == 'draft' and ri.record(project[name]) == []
    with pytest.raises(ProtocolError, match='not waiting to be issued'):
        reports.issue(project, {'name': name, 'step': 'cancel'})
    monkeypatch.setattr(report_package, 'render_html', drawn)
    assert reports.issue(project, {'name': name, 'step': 'begin'})['status'] == 'issued'


def test_an_issue_cancelled_leaves_what_it_found(project):
    name = _identified(project)
    before = dict(project[name].identity)
    reports.issue(project, {'name': name, 'step': 'begin'})
    assert reports.issue(project, {'name': name, 'step': 'cancel'}) == {}
    assert project[name].identity == before


def test_printing_asks_what_an_export_asks(project, tmp_path):
    name = reports.generate(project, {'template': 'modal'})
    project.report_features(name, enable=('skeleton_sections',))
    asked = reports.print_page(project, {'name': name})
    assert asked['html'] is None and asked['owed'] == list(project.report_check(name))
    forced = reports.print_page(project, {'name': name, 'anyway': True})
    assert 'print()' in forced['html'] and forced['owed'] == [] and forced['index'] == []

    attached = _indexed(project, tmp_path)
    project.report_features(name, enable=('data_index',), disable=('skeleton_sections',))
    assert reports.print_page(project, {'name': name})['html']
    attached.unlink()
    asked = reports.print_page(project, {'name': name})
    assert asked == {'html': None, 'owed': [],
                     'index': [{'name': 'settings.csv', 'status': 'NOT FOUND'}]}


def test_the_refusals_name_what_the_app_offers(project, tmp_path):
    name = _identified(project)
    _issued(project, name, tmp_path)
    with pytest.raises(ProtocolError, match='Amend, then Issue'):
        reports.issue(project, {'name': name, 'step': 'begin'})
    project[name].title = 'Changed'
    with pytest.raises(ProtocolError, match='Amend, then Issue'):
        reports.print_page(project, {'name': name})
    with pytest.raises(ValueError, match='Amend, then Issue'):
        ri.identify(project[name], number='other')
    with pytest.raises(ValueError, match='changes only by Issue and Amend'):
        ri.identify(project[name], status='issued')
