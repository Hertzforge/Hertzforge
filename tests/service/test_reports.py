"""Reports on the connection's project: generated from a starter template,
rendered as the page the core writes, and written to a file."""

from __future__ import annotations

import asyncio
from pathlib import Path

import pytest

from hertzforge.service import projects, reports
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
    written = reports.export(project, {'name': name, 'path': str(tmp_path / 'plate')})
    assert written == str(tmp_path / 'plate.html')
    assert Path(written).read_text(encoding='utf-8').startswith('<!DOCTYPE html>')
    assert not list(tmp_path.glob('*.partial'))


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
        ('report.export', {'name': 'Plate Report', 'path': str(tmp_path / 'out.html')}),
        ('report.render', {}),
    ]))
    none, _, made, shown, written, unnamed = (got[-1] for got in responses)
    assert none['error']['code'] == 'refused'
    validate(made['result'], 'reportGenerateResult')
    assert made['result']['name'] == 'Plate Report' and made['result']['project']['modified']
    validate(shown['result'], 'reportRenderResult')
    validate(written['result'], 'reportExportResult')
    assert (tmp_path / 'out.html').exists()
    assert unnamed['error']['code'] == 'bad_request'
