"""The Model, Specify and Signals modules' report templates
(`hertzforge.core.report`: `model_template`, `specify_template`,
`signals_template`), each module's default in the Report module.

What is pinned: each builds on the project it is for and draws its figures,
its live references fill in, and on a project without its objects it is
slots, not errors.
"""

from __future__ import annotations

import re

import pytest
from conftest import fixture_path

import hertzforge
from hertzforge.report import _report_payload


def _drawn(project, template):
    name = project.generate_report(template)
    payload = _report_payload(project[name], project, None,
                              object_groups=project.object_groups)
    kinds = [block['kind'] for block in payload['blocks']]
    text = ' '.join(re.sub('<[^>]+>', '', b.get('html', '')) for b in payload['blocks'])
    return kinds, text


@pytest.fixture(scope='module')
def run():
    return hertzforge.random_vibration_run(
        fixture_path('plate', 'random.nc4'),
        geometry=fixture_path('plate', 'geometry.npz'), length_unit='m')


def test_the_model_report_draws_the_model_and_its_modes():
    from hertzforge.demo import barc

    kinds, text = _drawn(barc.project(solved=True), 'model')
    assert kinds.count('scene') == 2 and 'table' in kinds and 'mac' in kinds
    assert '5470 nodes' in text and '21 modes' in text and 'no value' not in text


def test_the_specification_report_draws_what_the_project_holds(run):
    kinds, text = _drawn(run, 'specify')
    assert kinds.count('plot') >= 1
    assert '8 control channels' in text and 'no value' not in text


def test_the_signals_report_draws_each_quantity(run):
    kinds, text = _drawn(run, 'signals')
    assert kinds.count('plot') >= 2 and 'table' in kinds
    assert '12 channels' in text


@pytest.mark.parametrize('template', ['model', 'specify', 'signals'])
def test_an_empty_project_gives_slots_not_errors(template):
    kinds, _text = _drawn(hertzforge.Project('nothing'), template)
    assert kinds and set(kinds) <= {'text', 'unbound'}


def test_the_service_offers_them():
    from hertzforge.service import reports

    assert {'model', 'specify', 'signals'} <= set(reports.TEMPLATES)
