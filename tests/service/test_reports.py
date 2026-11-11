"""Reports on the connection's project: generated from a starter template,
rendered as the page the core writes, and written to a file."""

from __future__ import annotations

import asyncio
from pathlib import Path

import pytest

from hertzforge.service import frames, projects, reports
from hertzforge.service.protocol import ProtocolError

from .test_projects import _exchange

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'


@pytest.fixture
def project():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'frfs.unv')})
    return state.project


def test_a_report_is_generated_rendered_and_exported(project, tmp_path):
    name = reports.generate(project, {'template': 'modal'})
    assert projects.kind_of(project[name]) == 'report'
    html = reports.render(project, {'name': name, 'theme': 'dark'})
    assert html.startswith('<!DOCTYPE html>') and 'data-theme="dark"' in html
    assert 'http://' not in html.split('<script id="data"')[0].lower(), 'nothing from a network'
    answer = reports.export(project, {'name': name, 'path': str(tmp_path / 'plate')})
    written = answer['path']
    assert written == str(tmp_path / 'plate.html')
    assert answer['owed'] == [] and answer['index'] == [] and answer['note'].endswith('unbound blocks left out.')
    assert Path(written).read_text(encoding='utf-8').startswith('<!DOCTYPE html>')
    assert not list(tmp_path.glob('*.partial'))


def test_an_export_asks_about_owed_sections_and_astray_files_first(project, tmp_path):
    """With skeleton sections on, a
    report that still owes sections is not written until told to anyway;
    with the data index on, the files are hashed again and one changed or
    not found holds the export the same way. An issue makes the index
    check too."""
    name = reports.generate(project, {'template': 'modal'})
    project.report_features(name, enable=('skeleton_sections',))
    asked = reports.export(project, {'name': name, 'path': str(tmp_path / 'owed')})
    assert asked['path'] is None and asked['owed'] and asked['index'] == []
    assert asked['owed'] == list(project.report_check(name))
    assert not (tmp_path / 'owed.html').exists(), 'nothing written while asking'
    forced = reports.export(project, {'name': name, 'path': str(tmp_path / 'owed'), 'anyway': True})
    assert forced['path'] == str(tmp_path / 'owed.html') and (tmp_path / 'owed.html').exists()

    attached = tmp_path / 'settings.txt'
    attached.write_text('gain 2', encoding='utf-8')
    project.index_file(attached, 'the settings')
    project.report_features(name, enable=('data_index',), disable=('skeleton_sections',))
    clean = reports.export(project, {'name': name, 'path': str(tmp_path / 'clean')})
    assert clean['path'] and clean['index'] == []
    attached.unlink()
    asked = reports.export(project, {'name': name, 'path': str(tmp_path / 'astray')})
    assert asked['path'] is None and asked['owed'] == []
    assert asked['index'] == [{'name': 'settings.txt', 'status': 'NOT FOUND'}]
    with pytest.raises(ProtocolError) as error:
        reports.export(project, {'name': name, 'path': str(tmp_path / 'x'), 'anyway': 'yes'})
    assert error.value.code == 'bad_request'
    forced = reports.export(project, {'name': name, 'path': str(tmp_path / 'astray'), 'anyway': True})
    assert forced['path'] and forced['index'] == []


@pytest.mark.parametrize(('params', 'code'), [
    ({'template': 'thesis'}, 'bad_request'),
    ({'name': ' '}, 'bad_request'),
])
def test_a_template_or_name_that_is_not_one_is_refused(project, params, code):
    with pytest.raises(ProtocolError) as error:
        reports.generate(project, params)
    assert error.value.code == code


def test_rendering_names_a_report_in_the_project(project):
    with pytest.raises(ProtocolError) as error:
        reports.render(project, {'name': 'Nothing'})
    assert error.value.code == 'not_found'
    with pytest.raises(ProtocolError, match='not a report'):
        reports.render(project, {'name': 'FRF'})
    name = reports.generate(project, {})
    with pytest.raises(ProtocolError, match='light or dark'):
        reports.render(project, {'name': name, 'theme': 'sepia'})


def test_a_report_over_the_socket(validate, tmp_path):
    responses = asyncio.run(_exchange([
        ('report.generate', {}),
        ('project.import', {'path': str(PLATE / 'frfs.unv')}),
        ('report.generate', {'template': 'modal', 'name': 'Plate Report'}),
        ('report.render', {'name': 'Plate Report'}),
        ('report.views', {'name': 'Plate Report'}),
        ('report.export', {'name': 'Plate Report', 'path': str(tmp_path / 'out.html')}),
        ('report.print', {'name': 'Plate Report', 'anyway': True}),
        ('report.render', {}),
    ]))
    empty, _, made, shown, drawn, written, printed, unnamed = (got[-1] for got in responses)
    # the connection's empty project takes a report like any other
    assert empty['result']['name'] == 'Report'
    validate(made['result'], 'reportGenerateResult')
    assert made['result']['name'] == 'Plate Report' and made['result']['project']['modified']
    validate(shown['result'], 'reportRenderResult')
    wire, _arrays = frames.to_wire(drawn['result'])
    validate(wire, 'reportViewsResult')
    figures = [b for b in drawn['result']['blocks'] if b['kind'] == 'figure']
    assert figures and all(b['views'] for b in figures), 'the plate survey draws figures'
    validate(written['result'], 'reportExportResult')
    validate(printed['result'], 'reportPrintResult')
    assert (tmp_path / 'out.html').exists()
    assert unnamed['error']['code'] == 'bad_request'


@pytest.fixture
def edited(project):
    name = reports.generate(project, {'template': 'modal', 'name': 'R'})
    return project, name


def test_the_editor_gives_the_edit_page_and_the_reports_pane(edited, validate):
    project, name = edited
    out = reports.edit_page(project, {'name': name})
    validate(out, 'reportEditorResult')
    assert 'hforgeReport' in out['html'] and 'qt.webChannelTransport' in out['html']
    assert out['block'] is None and out['title'] == project[name].title
    assert out['insert'][0] == ['markdown', 'Markdown']
    assert out['blocks'] == project[name].num_blocks


def test_inserting_markdown_and_typing_into_it(edited, validate):
    project, name = edited
    count = project[name].num_blocks
    out = reports.operate(project, {'name': name, 'op': 'insert', 'kind': 'markdown', 'at': 0})
    validate(out, 'reportEditorResult')
    assert out['selected'] == 0 and out['block']['kind'] == 'text'
    assert project[name].num_blocks == count + 1
    reports.operate(project, {'name': name, 'op': 'text', 'at': 0, 'value': '# Setup'})
    assert project[name].blocks[0]['text'] == '# Setup'


def test_a_result_inserted_binds_to_the_first_object_it_can_show(edited):
    project, name = edited
    out = reports.operate(project, {'name': name, 'op': 'insert', 'kind': 'cmif'})
    block = project[name].blocks[out['selected']]
    assert block['kind'] == 'plot' and block['mode'] == 'cmif' and block['source'] == 'FRF'
    fields = {f['field']: f for f in out['block']['fields']}
    assert fields['source']['value'] == 'FRF'
    assert ['FRF', 'FRF'] in fields['source']['options']


def test_a_block_is_moved_captioned_repointed_and_removed(edited):
    project, name = edited
    report = project[name]
    first = report.blocks[0]
    out = reports.operate(project, {'name': name, 'op': 'move', 'from': 0, 'to': 2})
    assert report.blocks[1] is first and out['selected'] == 1
    at = reports.operate(project, {'name': name, 'op': 'insert', 'kind': 'cmif'})['selected']
    reports.operate(project, {'name': name, 'op': 'field', 'at': at, 'field': 'caption',
                              'value': 'The CMIF'})
    assert report.blocks[at]['caption'] == 'The CMIF'
    with pytest.raises(ProtocolError, match='is not something block'):
        reports.operate(project, {'name': name, 'op': 'field', 'at': at, 'field': 'source',
                                  'value': 'Nothing'})
    for to in (-1, report.num_blocks + 1):
        with pytest.raises(ProtocolError, match='no place to move'):
            reports.operate(project, {'name': name, 'op': 'move', 'from': 0, 'to': to})
    assert reports.operate(project, {'name': name, 'op': 'move', 'from': at,
                                     'to': report.num_blocks})['selected'] == report.num_blocks - 1
    at = report.num_blocks - 1
    count = report.num_blocks
    out = reports.operate(project, {'name': name, 'op': 'remove', 'at': at})
    assert report.num_blocks == count - 1 and out['selected'] is None


def test_the_title_and_marking_are_the_reports(edited):
    project, name = edited
    reports.operate(project, {'name': name, 'op': 'title', 'value': ' Plate survey '})
    reports.operate(project, {'name': name, 'op': 'marking', 'value': ''})
    out = reports.operate(project, {'name': name, 'op': 'marking_color', 'value': 'red'})
    assert (out['title'], out['marking'], out['marking_color']) == ('Plate survey', '', 'red')
    with pytest.raises(ProtocolError, match='needs a title'):
        reports.operate(project, {'name': name, 'op': 'title', 'value': ' '})
    with pytest.raises(ProtocolError, match='there is no block 999'):
        reports.operate(project, {'name': name, 'op': 'remove', 'at': 998})


def test_a_selection_from_the_page_comes_back_without_the_page(edited, validate):
    project, name = edited
    out = reports.edit_page(project, {'name': name, 'selected': 0, 'page': False})
    validate(out, 'reportEditorResult')
    assert 'html' not in out and out['block']['index'] == 0


FIELDS = {'number': 'HF-001', 'revision': 'A', 'laboratory': 'The lab', 'address': '1 Road',
          'location': 'Bay 2', 'customer': 'Someone', 'contact': 'A person',
          'received': '2026-11-01', 'tested': '2026-11-02', 'issued': '2026-11-03',
          'authorized': ['B. Signer']}


@pytest.fixture
def identified(project):
    name = reports.generate(project, {'template': 'empty', 'name': 'R'})
    project.report_features(name, enable=['report_identity'])
    project.report_identity(name, **FIELDS)
    return project, name


def test_the_printed_page_draws_finely_and_asks_to_print(edited):
    project, name = edited
    page = reports.print_page(project, {'name': name})['html']
    assert "replaceState(null, '', '#print')" in page and 'print()' in page


def test_an_issue_is_begun_printed_and_recorded_with_its_hashes(identified, tmp_path):
    import hashlib

    project, name = identified
    begun = reports.issue(project, {'name': name, 'step': 'begin'})
    assert begun['status'] == 'issued' and 'print()' in begun['html']
    pdf = tmp_path / 'issued.pdf'
    pdf.write_bytes(b'%PDF-1.4\n% a page\n%%EOF\n')
    done = reports.issue(project, {'name': name, 'step': 'record', 'path': str(pdf)})
    assert done['entry']['sha256'] == hashlib.sha256(pdf.read_bytes()).hexdigest()
    assert done['entry']['number'] == 'HF-001' and done['entry']['file'] == 'issued.pdf'
    with pytest.raises(ProtocolError, match='already issued'):
        reports.issue(project, {'name': name, 'step': 'begin'})


def test_an_issue_cancelled_leaves_a_draft(identified, tmp_path):
    from hertzforge.report.identity import value

    project, name = identified
    reports.issue(project, {'name': name, 'step': 'begin'})
    reports.issue(project, {'name': name, 'step': 'cancel'})
    assert value(project[name], 'status') == 'draft'
    with pytest.raises(ProtocolError, match='not waiting to be issued'):
        reports.issue(project, {'name': name, 'step': 'record', 'path': str(tmp_path / 'x.pdf')})


def test_a_file_that_is_not_a_pdf_is_not_recorded(identified, tmp_path):
    project, name = identified
    reports.issue(project, {'name': name, 'step': 'begin'})
    page = tmp_path / 'page.html'
    page.write_text('<html></html>')
    with pytest.raises(ProtocolError, match='is not a PDF'):
        reports.issue(project, {'name': name, 'step': 'record', 'path': str(page)})


def test_a_report_without_identity_is_not_issued(edited):
    project, name = edited
    with pytest.raises(ProtocolError, match='does not have Identity and issue'):
        reports.issue(project, {'name': name, 'step': 'begin'})


def test_a_written_export_says_what_it_left_out_and_what_the_check_found(project, tmp_path):
    name = reports.generate(project, {'template': 'empty', 'name': 'R'})
    project[name].add({'kind': 'plot', 'source': 'Gone', 'caption': 'x'})
    project[name].add({'kind': 'plot', 'source': 'Gone too', 'caption': 'y'})
    answer = reports.export(project, {'name': name, 'path': str(tmp_path / 'a')})
    assert answer['note'] == '2 unbound blocks left out.'
    attached = tmp_path / 'settings.txt'
    attached.write_text('gain 2', encoding='utf-8')
    project.index_file(attached, 'the settings')
    project.report_features(name, enable=('data_index',))
    project[name].remove([0, 1])
    answer = reports.export(project, {'name': name, 'path': str(tmp_path / 'b')})
    assert answer['note'].startswith('Checked ') and 'match' in answer['note']
