"""Filter and Rescale with several time histories picked, as the shell
reaches them: the action table's row fills the first as the source and
the rest as `more`, the form previews the first, and Apply adds a copy of
each into a group of their own, which TRAC Overlay then takes as a set."""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.data import TimeHistory
from hertzforge.service import projects
from hertzforge.service.action_table import action_table
from hertzforge.service.ops_project import VERB_OPERATIONS
from hertzforge.service.ops_settings import OPERATIONS as SETTINGS
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

OPS = {op.id: op for op in (*VERB_OPERATIONS, *SETTINGS)}


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return op.run(project, values)


def _files():
    t = np.arange(1000) / 1000.0
    made = hertzforge.Project()
    names = []
    for side, gain in (('Test', 1.0), ('Model', -2.0)):
        for k, dof in enumerate(['1X+', '2X+']):
            name = f'{side} {dof}'
            rows = [gain * np.sin(2 * np.pi * (5 + k) * t), gain * np.cos(2 * np.pi * 7 * t)]
            made.add(name, TimeHistory(t, rows, response_dof=[dof, '9Z+'],
                                       ordinate_dim=['acceleration', 'acceleration']))
            names.append(name)
    made.object_groups.append({'members': names[:2], 'role': None, 'name': 'test.unv'})
    made.object_groups.append({'members': names[2:], 'role': None, 'name': 'model.unv'})
    return made, names


def _rows(project, objects, records=None):
    return {row['id']: row for row in
            action_table(project, objects, records or {})['columns'][0]['rows']}


def test_several_histories_picked_offer_filter_and_rescale_with_the_rest_as_more():
    project, names = _files()
    rows = _rows(project, names[:2], {'Test 2X+': [1]})
    assert rows['project.set_filter']['ready']
    assert rows['project.set_filter']['fill'] == {'source': 'Test 1X+', 'more': 'Test 2X+'}
    assert set(rows['project.set_filter']['omit']) == {'source', 'more', 'record'}
    assert rows['project.rescale']['fill'] == {'source': 'Test 1X+', 'more': 'Test 2X+',
                                               'records': '', 'more_records': '2'}
    assert set(rows['project.rescale']['omit']) == set(rows['project.rescale']['fill'])


def test_one_history_picked_leaves_the_more_fields_off_the_form():
    project, names = _files()
    row = _rows(project, names[:1])['project.set_filter']
    assert row['fill']['more'] == ''
    assert 'more' in row['omit']


def test_a_history_picked_with_another_kind_offers_neither():
    project, names = _files()
    spectra = project.compute_spectra(names[0])
    rows = _rows(project, [names[1], spectra])
    assert 'project.set_filter' not in rows
    assert 'project.rescale' not in rows


def test_the_filter_previews_the_first_and_apply_filters_each_into_a_group(validate):
    project, names = _files()
    fill = _rows(project, names[:2])['project.set_filter']['fill']
    views = run(project, 'project.set_filter', **fill, kind='low-pass', high=100.0, order=4)
    wire_views(views, validate)
    trace, reading = views
    assert trace['title'] == 'Test 1X+, 1X+'
    said = {row['label']: row['value'] for row in reading['rows']}
    assert said['Histories'] == 'Test 1X+ and 1 more, each filtered by Apply'
    assert reading['title'] == 'Filter of Test 1X+'
    assert list(project) == names
    views = run(project, 'project.set_filter', **fill, kind='low-pass', high=100.0, order=4,
                keep=True)
    wire_views(views, validate)
    _waterfall, added = views
    assert added['title'] == '2 added to test.unv Filtered'
    assert [row['label'] for row in added['rows']] == ['Test 1X+ Filtered', 'Test 2X+ Filtered']
    assert [row['value'] for row in added['rows']] == ['from Test 1X+', 'from Test 2X+']
    assert project[names[1]].filtering == project[names[0]].filtering
    assert project[names[1]].filtering.high == 100.0
    assert project.journal[-1].startswith("project.filter_histories('Test 1X+', 'Test 2X+', "
                                          "filtering=Filtering(")


def test_a_refusal_in_the_batch_comes_back_in_the_cores_words_and_adds_nothing():
    project, names = _files()
    rows = project[names[1]].ordinate.copy()
    rows[1, 3] = np.nan
    project[names[1]] = TimeHistory(project[names[1]].abscissa, rows,
                                    response_dof=['2X+', '9Z+'],
                                    ordinate_dim=['acceleration', 'acceleration'])
    fill = _rows(project, names[:2])['project.set_filter']['fill']
    with pytest.raises(ProtocolError, match=r'Test 2X\+: record 2 \(9Z\+\) has a sample'):
        run(project, 'project.set_filter', **fill, kind='low-pass', high=100.0, keep=True)
    assert list(project) == names


def test_rescale_previews_the_first_and_apply_takes_each_ones_picked_records(validate):
    project, names = _files()
    fill = _rows(project, names[:2], {'Test 2X+': [1]})['project.rescale']['fill']
    plot, reading = run(project, 'project.rescale', **fill, factor=3.0)
    assert plot['title'].startswith('Test 1X+: 2 of 2 picked records')
    said = {row['label']: row['value'] for row in reading['rows']}
    assert said['Histories'] == 'Test 1X+ and 1 more, each rescaled by Apply'
    views = run(project, 'project.rescale', **fill, factor=3.0, keep=True)
    wire_views(views, validate)
    first, second = project['Test 1X+ Rescaled'], project['Test 2X+ Rescaled']
    np.testing.assert_array_equal(first.ordinate, 3.0 * project[names[0]].ordinate)
    np.testing.assert_array_equal(second.ordinate[0], project[names[1]].ordinate[0])
    np.testing.assert_array_equal(second.ordinate[1], 3.0 * project[names[1]].ordinate[1])
    assert project.object_group_of('Test 1X+ Rescaled') == ['Test 1X+ Rescaled',
                                                            'Test 2X+ Rescaled']
    assert project.journal[-1] == ("project.rescale_histories('Test 1X+', 'Test 2X+', "
                                   "factor=3.0, records={'Test 2X+': [1]})")


def test_the_two_filtered_groups_offer_trac_overlay_as_two_sets():
    project, names = _files()
    for picked in (names[:2], names[2:]):
        fill = _rows(project, picked)['project.set_filter']['fill']
        run(project, 'project.set_filter', **fill, kind='low-pass', high=100.0, keep=True)
    copies = [f'{name} Filtered' for name in names]
    row = _rows(project, copies)['project.compare_time_histories']
    assert row['ready']
    assert (row['fill']['first'], row['fill']['second']) == ('Test 1X+ Filtered',
                                                             'Model 1X+ Filtered')
