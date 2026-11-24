"""Filtering and rescaling several time histories at once: a copy of each
with the same settings, every one checked before any is added, the
copies in a group of their own named for the sources' group, each
refreshing from its own source, and a filtered test set and a filtered
model set then compared as two groups."""

from __future__ import annotations

import re

import numpy as np
import pytest

from hertzforge import Project
from hertzforge.core.data import TimeHistory
from hertzforge.core.filters import Filtering

RATE = 1000.0
LOW_PASS = Filtering(order=4, high=50.0)


def _history(rows, dofs, rate=RATE, dims=None):
    t = np.arange(np.shape(rows)[-1]) / rate
    return TimeHistory(t, rows, response_dof=dofs,
                       ordinate_dim=dims or ['acceleration'] * len(dofs))


def _signal(k, rate=RATE, samples=1000):
    """A tone below the corner and one far above it."""
    t = np.arange(samples) / rate
    return np.sin(2 * np.pi * (5 + k) * t) + 0.5 * np.sin(2 * np.pi * 300 * t)


def _files(gain=-2.0):
    """Two imported files of single-record histories, each file a group."""
    project = Project()
    names = []
    for side, scale in (('Test', 1.0), ('Model', gain)):
        for k, dof in enumerate(['1X+', '2X+', '3X+']):
            name = f'{side} {dof}'
            project.add(name, _history([scale * _signal(k)], [dof]))
            names.append(name)
    project.object_groups.append({'members': names[:3], 'role': None, 'name': 'test.unv'})
    project.object_groups.append({'members': names[3:], 'role': None, 'name': 'model.unv'})
    return project, names


def test_each_history_is_filtered_the_same_way_into_a_group_named_for_its_file():
    project, names = _files()
    before = {name: project[name].ordinate.copy() for name in names}
    made = project.filter_histories(*names[:3], filtering=LOW_PASS)
    assert made == ['Test 1X+ Filtered', 'Test 2X+ Filtered', 'Test 3X+ Filtered']
    for name, copy in zip(names[:3], made):
        np.testing.assert_allclose(project[copy].ordinate, project[name].filter(LOW_PASS).ordinate)
        # the tone above the corner is gone, the one below it kept
        assert np.max(np.abs(project[copy].ordinate)) == pytest.approx(1.0, abs=0.05)
        assert project.provenance[copy]['source'] == name
        assert project.provenance[copy]['verb'] == 'filter_data'
        assert project[name].filtering == LOW_PASS
    assert project.object_group_of(made[0]) == made
    assert project.object_groups[-1]['name'] == 'test.unv Filtered'
    # the sources keep their values and their own group, the copies are in none of theirs
    for name in names:
        np.testing.assert_array_equal(project[name].ordinate, before[name])
    assert project.object_group_of(names[0]) == names[:3]


def test_the_first_historys_own_filter_is_used_when_none_is_given():
    project, names = _files()
    project[names[0]].filtering = LOW_PASS
    project.filter_histories(*names[:2])
    assert project[names[1]].filtering == LOW_PASS


def test_one_that_cannot_be_filtered_adds_nothing_and_is_named():
    project, names = _files()
    project.add('Slow', _history([_signal(0, rate=80.0, samples=400)], ['4X+'], rate=80.0))
    project.link(*names[:3], 'Slow')
    with pytest.raises(ValueError, match='^Slow: .*Nyquist'):
        project.filter_histories(*names[:3], 'Slow', filtering=LOW_PASS)
    assert list(project) == [*names, 'Slow']
    assert len(project.object_groups) == 2
    assert all(project[name].filtering is None for name in names)


def test_a_nan_sample_refuses_the_batch_with_the_history_and_the_record_named():
    project, names = _files()
    rows = project[names[1]].ordinate.copy()
    rows[0, 17] = np.nan
    project[names[1]] = _history(rows, ['2X+'])
    with pytest.raises(ValueError, match=r'^Test 2X\+: record 1 \(2X\+\) has a sample that is '
                                         r'not a number'):
        project.filter_histories(*names[:3], filtering=LOW_PASS)
    assert list(project) == names


def test_a_dead_and_a_constant_channel_filter_to_what_they_were():
    project = Project()
    project.add('Dead', _history([np.zeros(1000)], ['1X+']))
    project.add('Steady', _history([np.full(1000, 3.0)], ['2X+']))
    dead, steady = project.filter_histories('Dead', 'Steady', filtering=LOW_PASS)
    np.testing.assert_array_equal(project[dead].ordinate, 0.0)
    np.testing.assert_allclose(project[steady].ordinate, 3.0, rtol=1e-9)
    assert project.object_groups[-1]['name'] == 'Filtered 1'


@pytest.mark.parametrize('picked, says', [
    ((), 'pick two or more time histories'),
    (('Test 1X+',), 'pick two or more time histories'),
    (('Test 1X+', 'Test 1X+'), 'pick two or more time histories'),
])
def test_fewer_than_two_histories_are_refused(picked, says):
    project, names = _files()
    with pytest.raises(ValueError, match=says):
        project.filter_histories(*picked)
    with pytest.raises(ValueError, match=says):
        project.rescale_histories(*picked, factor=2.0)
    assert list(project) == names


def test_an_object_that_is_not_a_time_history_is_refused_by_name():
    project, names = _files()
    spectra = project.compute_spectra(names[0])
    with pytest.raises(ValueError, match=rf'^{re.escape(spectra)} is not a time history'):
        project.filter_histories(names[1], spectra)
    with pytest.raises(ValueError, match=rf'^{re.escape(spectra)} is not a time history'):
        project.rescale_histories(names[1], spectra, factor=2.0)


def test_histories_that_share_no_named_group_go_to_a_numbered_group():
    project, names = _files()
    first = project.filter_histories(names[0], names[3], filtering=LOW_PASS)
    second = project.rescale_histories(names[0], names[3], factor=2.0)
    third = project.filter_histories(names[1], names[4], filtering=LOW_PASS)
    named = {project.object_groups[i]['name']: project.object_groups[i]['members']
             for i in range(2, 5)}
    assert named == {'Filtered 1': first, 'Rescaled 1': second, 'Filtered 2': third}


def test_a_second_batch_of_one_group_gets_a_numbered_suffix_as_its_copies_do():
    project, names = _files()
    project.filter_histories(*names[:3], filtering=LOW_PASS)
    again = project.filter_histories(*names[:3], filtering=Filtering(order=2, high=100.0))
    assert again == ['Test 1X+ Filtered (2)', 'Test 2X+ Filtered (2)', 'Test 3X+ Filtered (2)']
    assert project.object_groups[-1]['name'] == 'test.unv Filtered (2)'


def test_rescale_takes_every_record_unless_some_are_picked():
    project = Project()
    two = np.vstack([_signal(0), _signal(1)])
    project.add('A', _history(two, ['1X+', '2X+']))
    project.add('B', _history(two, ['1X+', '2X+']))
    a, b = project.rescale_histories('A', 'B', factor=-3.0, records={'B': [1]})
    np.testing.assert_array_equal(project[a].ordinate, -3.0 * two)
    np.testing.assert_array_equal(project[b].ordinate[0], two[0])
    np.testing.assert_array_equal(project[b].ordinate[1], -3.0 * two[1])
    assert project.provenance[a]['params'] == {'factor': -3.0, 'records': None}
    assert project.provenance[b]['params'] == {'factor': -3.0, 'records': [1]}
    np.testing.assert_array_equal(project['B'].ordinate, two)


def test_rescale_by_sensitivities_and_its_refusals_add_nothing():
    project, names = _files()
    made = project.rescale_histories(*names[:2], recorded_sensitivity=100.0,
                                     correct_sensitivity=10.0)
    np.testing.assert_allclose(project[made[1]].ordinate, 10.0 * project[names[1]].ordinate)
    assert project.object_groups[-1]['name'] == 'test.unv Rescaled'
    count = len(project)
    with pytest.raises(ValueError, match='finite number other than zero'):
        project.rescale_histories(*names[:2], factor=0.0)
    with pytest.raises(ValueError, match='give a factor or two sensitivities'):
        project.rescale_histories(*names[:2], factor=2.0, recorded_sensitivity=1.0,
                                  correct_sensitivity=2.0)
    with pytest.raises(ValueError, match=r'^Test 2X\+: .*record'):
        project.rescale_histories(*names[:2], factor=2.0, records={'Test 2X+': [3]})
    with pytest.raises(ValueError, match=r'records are given for Model 1X\+, which is not'):
        project.rescale_histories(*names[:2], factor=2.0, records={'Model 1X+': [0]})
    assert len(project) == count


def test_a_nan_sample_rides_through_a_rescale_where_it_was():
    project, names = _files()
    rows = project[names[0]].ordinate.copy()
    rows[0, 5] = np.nan
    project[names[0]] = _history(rows, ['1X+'])
    made = project.rescale_histories(*names[:2], factor=2.0)
    copy = project[made[0]].ordinate[0]
    assert np.flatnonzero(np.isnan(copy)).tolist() == [5]
    np.testing.assert_allclose(np.delete(copy, 5), 2.0 * np.delete(rows[0], 5))


def test_each_copy_refreshes_from_its_own_source():
    project, names = _files()
    filtered = project.filter_histories(*names[:2], filtering=LOW_PASS)
    rescaled = project.rescale_histories(*names[:2], factor=2.0)
    project[names[1]].filtering = Filtering(order=4, high=400.0)
    assert set(project.stale()) == {filtered[1]}
    project.refresh(filtered[1])
    # the 300 Hz tone now passes, on the second copy only
    assert np.max(np.abs(project[filtered[1]].ordinate)) > 1.3
    assert np.max(np.abs(project[filtered[0]].ordinate)) == pytest.approx(1.0, abs=0.05)
    project[names[0]] = _history([4.0 * _signal(0)], ['1X+'])
    assert rescaled[0] in project.stale() and rescaled[1] not in project.stale()
    project.refresh(rescaled[0])
    np.testing.assert_allclose(project[rescaled[0]].ordinate, 8.0 * _signal(0)[None, :])
    # a refresh keeps each copy in its batch's group
    assert project.object_group_of(rescaled[0]) == rescaled


def test_two_filtered_groups_compare_as_the_test_set_and_the_model_set():
    project, names = _files()
    # as the guide writes it: each file's group filtered whole
    tests = project.filter_histories(*project.object_group_of('Test 1X+'), filtering=LOW_PASS)
    models = project.filter_histories(*project.object_group_of('Model 1X+'), filtering=LOW_PASS)
    assert tests == [f'{name} Filtered' for name in names[:3]]
    assert project.time_history_sets(*models, *tests) == (tests, models)
    name = project.compare_time_histories(*tests, *models)
    comparison = project[name]
    assert [pair[0] for pair in comparison.pairs] == tests
    assert [pair[2] for pair in comparison.pairs] == models


def test_the_batch_is_journaled_as_one_line_with_its_filter():
    project, names = _files()
    project.filter_histories(*names[:2], filtering=LOW_PASS)
    assert project.journal[-1] == (f"project.filter_histories('Test 1X+', 'Test 2X+', "
                                   f"filtering={LOW_PASS!r})")
