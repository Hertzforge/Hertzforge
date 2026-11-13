"""Rescale as the shell reaches it: the form previews the picked records
as recorded and rescaled with the factor, the count and the largest
value before and after, and Apply adds `<name> Rescaled` (Brandon,
2026-11-12)."""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.data import TimeHistory
from hertzforge.service import projects
from hertzforge.service.action_table import action_table
from hertzforge.service.ops_project import VERB_OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

OP = next(op for op in VERB_OPERATIONS if op.id == 'project.rescale')


def run(project, **given):
    values = OP.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return OP.run(project, values)


@pytest.fixture
def project():
    t = np.arange(512) / 512.0
    rows = np.vstack([np.sin(2 * np.pi * 8 * t), 2 * np.sin(2 * np.pi * 5 * t),
                      3 * np.cos(2 * np.pi * 3 * t)])
    made = hertzforge.Project()
    made.add('Run', TimeHistory(t, rows, response_dof=['1X+', '2X+', '3X+'],
                                ordinate_dim=['acceleration', 'acceleration', 'force']))
    return made


def test_the_preview_draws_the_picked_records_and_adds_nothing(project, validate):
    views = run(project, source='Run', by='sensitivities', recorded=100.0, correct=10.0,
                records='2', keep=False)
    wire_views(views, validate)
    assert [view['view'] for view in views] == ['spectrum', 'reading']
    plot, reading = views
    assert [s['name'] for s in plot['series']] == ['2X+', '2X+ rescaled']
    assert plot['series'][1]['follows'] == 0
    assert np.max(plot['series'][1]['y']) == pytest.approx(10 * np.max(plot['series'][0]['y']))
    rows = {row['label']: row for row in reading['rows']}
    assert rows['Factor']['value'] == 10.0
    assert rows['Records']['value'] == '1 of 3'
    assert rows['Largest, as recorded']['value'] == pytest.approx(2.0, rel=1e-3)
    assert rows['Largest, rescaled']['value'] == pytest.approx(20.0, rel=1e-3)
    assert rows['Largest, rescaled']['unit'] == 'm/s²'
    assert list(project) == ['Run']


def test_a_blank_pick_is_every_record_and_another_quantity_is_left_off_the_plot(project):
    plot, reading = run(project, source='Run', factor=-1.0, keep=False)
    assert [s['name'] for s in plot['series']] == ['1X+', '1X+ rescaled', '2X+', '2X+ rescaled']
    assert 'of another quantity left out' in plot['title']
    assert {row['label']: row['value'] for row in reading['rows']}['Records'] == '3 of 3'


def test_apply_adds_the_rescaled_record_with_the_others_unchanged(project, validate):
    views = run(project, source='Run', factor=-2.0, records='1, 3', keep=True)
    wire_views(views, validate)
    made = project['Run Rescaled']
    np.testing.assert_array_equal(made.ordinate[0], -2.0 * project['Run'].ordinate[0])
    np.testing.assert_array_equal(made.ordinate[1], project['Run'].ordinate[1])
    np.testing.assert_array_equal(made.ordinate[2], -2.0 * project['Run'].ordinate[2])
    assert project.journal[-1] == "project.rescale('Run', -2.0, records=[0, 2])"


@pytest.mark.parametrize('given, says', [
    ({'factor': 0.0}, 'finite number other than zero'),
    ({'by': 'sensitivities', 'recorded': 100.0, 'correct': 0.0}, 'correct sensitivity'),
    ({'records': '4'}, 'record'),
])
def test_what_cannot_be_rescaled_is_refused_and_nothing_added(project, given, says):
    with pytest.raises(ProtocolError, match=says):
        run(project, source='Run', keep=True, **given)
    assert list(project) == ['Run']


def test_a_time_history_offers_it_with_its_picked_records(project):
    rows = {row['id']: row for column in action_table(project, ['Run'], {'Run': [0, 1]})['columns']
            for row in column['rows']}
    assert rows['project.rescale']['ready']
    assert rows['project.rescale']['fill'] == {'source': 'Run', 'records': '1-2'}
    assert 'rescale' in projects.verbs_of(project, 'Run')
