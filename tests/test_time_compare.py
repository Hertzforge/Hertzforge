"""Two sets of time responses compared station by station: the pairing,
the shared clock, the four scores and the comparison saved in a project.

Every set here is synthetic. The scores are checked against numbers
worked out by hand from the definitions, and against numpy's own
correlation on noise."""

import h5py
import numpy as np
import pytest

from hertzforge import Project
from hertzforge.core.data import TimeComparison, TimeHistory
from hertzforge.core.time_compare import compare, match_records, response_records, scores
from hertzforge.io import native


def _history(t, rows, dofs, dims, **kwargs):
    return TimeHistory(t, rows, response_dof=dofs, ordinate_dim=dims, **kwargs)


def _sets(start=0.0):
    """A test at 1 kHz with two accelerations, a force and a voltage, and
    a model at 2 kHz with the two accelerations, starting at `start`."""
    t = start + np.arange(1000) / 1000.0
    fine = start + np.arange(2000) / 2000.0
    test = _history(t, [np.sin(2 * np.pi * 7 * t), np.cos(2 * np.pi * 11 * t),
                        np.sin(2 * np.pi * 3 * t), 0.1 * t],
                    ['101X+', '102Z-', '101X+', '900X+'],
                    ['acceleration', 'acceleration', 'force', 'voltage'])
    model = _history(fine, [-2.0 * np.sin(2 * np.pi * 7 * fine),
                            0.5 * np.cos(2 * np.pi * 11 * fine)],
                     ['101X+', '102Z-'], ['acceleration', 'acceleration'])
    return {'Test': test, 'Model': model}


# ---- pairing ---------------------------------------------------------------

def test_responses_pair_by_dof_and_quantity_leaving_out_the_force_and_the_monitor():
    sets = _sets()
    assert response_records(sets['Test']) == [0, 1]
    assert match_records(sets) == [['Test', 0, 'Model', 0], ['Test', 1, 'Model', 1]]


def test_a_dof_with_no_partner_is_refused_by_name():
    sets = _sets()
    t = sets['Model'].abscissa
    sets['Model'] = _history(t, [np.sin(t)], ['101X+'], ['acceleration'])
    with pytest.raises(ValueError, match=r'102Z- acceleration: 1 record \(Test\)'):
        match_records(sets)


def test_a_dof_with_three_records_is_refused_by_name():
    sets = _sets()
    t = sets['Model'].abscissa
    sets['Model'] = _history(t, [np.sin(t), np.sin(t), np.cos(t)],
                             ['101X+', '101X+', '102Z-'], ['acceleration'] * 3)
    with pytest.raises(ValueError, match=r'101X\+ acceleration: 3 records \(Model, Test\)'):
        match_records(sets)


def test_the_same_dof_in_another_quantity_is_another_station():
    """An acceleration and a velocity at 101X+ are two channels, so each
    is refused for want of a partner, never paired with the other."""
    sets = _sets()
    t = sets['Model'].abscissa
    sets['Model'] = _history(t, [np.sin(t), np.cos(t)], ['101X+', '102Z-'],
                             ['velocity', 'acceleration'])
    with pytest.raises(ValueError, match=r'101X\+ acceleration: 1 record'):
        match_records(sets)


def test_records_limit_the_automatic_pairing():
    sets = _sets()
    assert match_records(sets, records={'Test': [1], 'Model': [1]}) == [['Test', 1, 'Model', 1]]


def test_records_picked_under_one_set_take_only_their_partners_from_the_other():
    sets = _sets()
    assert match_records(sets, records={'Test': [1]}) == [['Test', 1, 'Model', 1]]
    assert match_records(sets, records={'Model': [0]}) == [['Test', 0, 'Model', 0]]


def test_nothing_to_pair_says_so():
    sets = _sets()
    with pytest.raises(ValueError, match='no response records to compare'):
        match_records(sets, records={'Test': [2, 3], 'Model': []})
    with pytest.raises(ValueError, match='pick the time histories'):
        match_records({})


def test_hand_pairs_may_join_two_dofs_but_not_two_quantities():
    sets = _sets()
    assert match_records(sets, pairs=[['Test', 0, 'Model', 1]]) == [['Test', 0, 'Model', 1]]
    with pytest.raises(ValueError, match='Test record 3 is force and Model record 1 is '
                                         'acceleration: a pair is one quantity'):
        match_records(sets, pairs=[['Test', 2, 'Model', 0]])


@pytest.mark.parametrize(('pairs', 'message'), [
    ([['Test', 0, 'Model', 0], ['Test', 1, 'Model', 0]], 'Model record 1 is in two pairs'),
    ([['Test', 0, 'Model', 5]], 'Model has no record 6'),
    ([['Test', 0, 'Other', 0]], 'a pair names Other, which is not among the sets'),
])
def test_a_hand_pair_that_cannot_be_read_one_way_is_refused(pairs, message):
    with pytest.raises(ValueError, match=message):
        match_records(_sets(), pairs=pairs)


def test_a_record_with_undeclared_units_is_refused():
    sets = _sets()
    t = sets['Model'].abscissa
    sets['Model'] = _history(t, [np.sin(t), np.cos(t)], ['101X+', '102Z-'],
                             ['acceleration', 'unknown'])
    with pytest.raises(ValueError, match=r'Model record 2 \(102Z-\): declare its units'):
        match_records(sets, pairs=[['Test', 1, 'Model', 1]])


def test_a_source_that_is_not_a_time_history_is_refused():
    sets = _sets()
    sets['Model'] = sets['Model'].compute_spectra()
    with pytest.raises(TypeError, match='Model is not a time history'):
        match_records(sets)


# ---- the shared clock --------------------------------------------------------

def test_the_coarsest_clock_is_kept_and_the_finer_trace_is_interpolated_onto_it():
    sets = _sets()
    made = compare(sets)
    assert isinstance(made, TimeComparison)
    # the test's clock, 0 to 0.999 s, lies inside the model's, so it is kept whole
    np.testing.assert_array_equal(made.abscissa, sets['Test'].abscissa)
    # the model's own samples lie on the test's clock, so they come back exactly
    t = made.abscissa
    np.testing.assert_allclose(made.ordinate[1], -2.0 * np.sin(2 * np.pi * 7 * t), atol=1e-12)
    assert made.response_dof == ['101X+', '101X+', '102Z-', '102Z-']
    assert made.block == ['Test', 'Model', 'Test', 'Model']
    assert made.ordinate_unit == [sets['Test'].ordinate_unit[0]] * 4


def test_interpolation_is_linear_between_samples():
    """A model on a clock offset by half a step: each value on the test's
    clock is the mean of the two around it."""
    t = np.arange(11) / 10.0
    off = 0.05 + np.arange(10) / 10.0
    sets = {'Test': _history(t, [t ** 2], ['1X+'], ['acceleration']),
            'Model': _history(off, [off ** 2], ['1X+'], ['acceleration'])}
    made = compare(sets)
    # the model's clock is as coarse as the test's; the first listed of two
    # equal steps is kept, cut to the overlap 0.05 to 0.95
    np.testing.assert_allclose(made.abscissa, np.arange(1, 10) / 10.0)
    inner = made.abscissa
    expected = ((inner - 0.05) ** 2 + (inner + 0.05) ** 2) / 2
    np.testing.assert_allclose(made.ordinate[1], expected, atol=1e-12)
    np.testing.assert_allclose(made.ordinate[0], inner ** 2, atol=1e-12)


def test_only_the_time_every_record_covers_is_kept_with_a_nonzero_start():
    sets = _sets(start=2.5)
    t = sets['Model'].abscissa
    sets['Model'] = _history(t[200:], sets['Model'].ordinate[:, 200:], ['101X+', '102Z-'],
                             ['acceleration'] * 2)
    made = compare(sets)
    assert made.abscissa[0] == pytest.approx(2.6)
    assert made.abscissa[-1] == pytest.approx(3.499)
    assert made.start == made.abscissa[0] and made.stop == made.abscissa[-1]


def test_sets_with_no_common_time_are_refused():
    sets = _sets()
    model = sets['Model']
    sets['Model'] = _history(model.abscissa + 10, model.ordinate, model.response_dof,
                             model.ordinate_dim)
    with pytest.raises(ValueError, match='share no stretch of time'):
        compare(sets)


def test_a_clock_that_is_not_increasing_is_refused():
    sets = _sets()
    model = sets['Model']
    x = model.abscissa.copy()
    x[5] = x[4]
    sets['Model'] = _history(x, model.ordinate, model.response_dof, model.ordinate_dim)
    with pytest.raises(ValueError, match='Model: its time samples are not finite and increasing'):
        compare(sets)


def test_a_simulated_source_marks_the_comparison_simulated():
    sets = _sets()
    sets['Test'].source = 'HARDWARE'
    assert compare(sets).source == 'HARDWARE'
    sets['Model'].source = 'SIM'
    assert compare(sets).source == 'SIM'


# ---- the scores --------------------------------------------------------------

def test_the_scores_of_a_reversed_double_and_a_half():
    """The model is -2 times the test at 101X+ and half of it at 102Z-.
    TRAC is blind to both; the correlation, the RMS ratio and sTRAC say
    which is which."""
    rows = scores(compare(_sets()))
    first, second = rows
    assert first['dof'] == '101X+' and first['quantity'] == 'acceleration'
    assert first['trac'] == pytest.approx(1.0)
    assert first['correlation'] == pytest.approx(-1.0)
    assert first['rms_ratio'] == pytest.approx(2.0)
    assert first['strac'] == pytest.approx(-0.5)
    assert second['trac'] == pytest.approx(1.0)
    assert second['correlation'] == pytest.approx(1.0)
    assert second['rms_ratio'] == pytest.approx(0.5)
    assert second['strac'] == pytest.approx(2.0)
    assert first['reason'] == second['reason'] == ''


def test_the_scores_against_numpy_on_noise():
    rng = np.random.default_rng(7)
    t = np.arange(4096) / 1024.0
    a = rng.standard_normal(4096)
    b = 0.3 * a + rng.standard_normal(4096)
    sets = {'Test': _history(t, [a], ['5Y+'], ['velocity']),
            'Model': _history(t, [b], ['5Y+'], ['velocity'])}
    row, = scores(compare(sets))
    expected = (a @ b) / np.sqrt((a @ a) * (b @ b))
    assert row['correlation'] == pytest.approx(expected, rel=1e-12)
    assert row['trac'] == pytest.approx(expected ** 2, rel=1e-12)
    assert row['rms_ratio'] == pytest.approx(np.linalg.norm(b) / np.linalg.norm(a), rel=1e-12)
    assert row['strac'] == pytest.approx((a @ b) / (b @ b), rel=1e-12)
    assert row['samples'] == 4096


def test_two_tones_that_only_agree_inside_the_window_score_one_there():
    """The model follows the test for the first half second and then
    leaves it: the whole clock scores below one, the first half one."""
    t = np.arange(1000) / 1000.0
    a = np.sin(2 * np.pi * 9 * t)
    b = np.where(t < 0.5, a, np.sin(2 * np.pi * 13 * t))
    made = compare({'Test': _history(t, [a], ['1X+'], ['acceleration']),
                    'Model': _history(t, [b], ['1X+'], ['acceleration'])})
    assert scores(made)[0]['trac'] < 0.6
    inside, = scores(made, 0.0, 0.499)
    assert inside['trac'] == pytest.approx(1.0) and inside['samples'] == 500


def test_a_lag_of_a_few_milliseconds_is_not_removed_and_scores_low():
    """A decaying 37 Hz response and the same response 4 ms late: nothing
    aligns them, so the pair scores well below one."""
    t = np.arange(4096) / 4096.0
    a = np.exp(-3 * t) * np.sin(2 * np.pi * 37 * t)
    b = np.exp(-3 * (t - 0.004)) * np.sin(2 * np.pi * 37 * (t - 0.004))
    row, = scores(compare({'Test': _history(t, [a], ['1X+'], ['acceleration']),
                           'Model': _history(t, [b], ['1X+'], ['acceleration'])}))
    assert row['trac'] < 0.5
    assert row['trac'] == pytest.approx(np.cos(2 * np.pi * 37 * 0.004) ** 2, abs=0.02)


@pytest.mark.parametrize(('trace', 'reason'), [
    (np.zeros(1000), 'a trace is zero in the window'),
    (np.full(1000, 3.0), 'a trace is constant in the window'),
    (np.r_[np.ones(10), np.nan, np.ones(989)], 'a trace is not finite in the window'),
])
def test_a_dead_trace_has_no_score_and_says_why(trace, reason):
    t = np.arange(1000) / 1000.0
    good = np.sin(2 * np.pi * 5 * t)
    sets = {'Test': _history(t, [good, good], ['1X+', '2X+'], ['acceleration'] * 2),
            'Model': _history(t, [trace, good], ['1X+', '2X+'], ['acceleration'] * 2)}
    dead, alive = scores(compare(sets))
    assert dead['reason'] == reason
    assert dead['trac'] is dead['correlation'] is dead['rms_ratio'] is dead['strac'] is None
    assert alive['trac'] == pytest.approx(1.0) and alive['reason'] == ''


@pytest.mark.parametrize(('start', 'stop', 'message'), [
    (0.5, 0.5, 'ends after it starts'),
    (0.6, 0.2, 'ends after it starts'),
    (-0.1, 0.5, r'must lie inside the shared clock, 0 s to 0\.999 s'),
    (0.2, 1.5, 'must lie inside the shared clock'),
    (0.1995, 0.2005, 'holds fewer than two samples'),  # one sample, at 0.2 s
    (0.0, np.inf, 'ends after it starts'),
])
def test_a_window_that_does_not_fit_is_refused(start, stop, message):
    made = compare(_sets())
    with pytest.raises(ValueError, match=message):
        scores(made, start, stop)
    with pytest.raises(ValueError, match=message):
        compare(_sets(), start=start, stop=stop)


def test_a_window_of_two_samples_at_the_ends_of_the_clock_is_scored():
    made = compare(_sets())
    row = scores(made, made.abscissa[-2], made.abscissa[-1])[0]
    assert row['samples'] == 2 and row['trac'] == pytest.approx(1.0)


# ---- in a project --------------------------------------------------------------

def _project():
    project = Project()
    for name, data in _sets().items():
        project.add(name, data)
    return project


def test_the_comparison_saves_and_opens_under_schema_7(tmp_path):
    project = _project()
    name = project.compare_time_histories('Test', 'Model')
    project.set_time_comparison_window(name, 0.2, 0.7)
    path = tmp_path / 'compared.hforge'
    project.save(path)
    with h5py.File(path) as f:
        assert f.attrs[native.STAMP] == native.TIME_COMPARISON_SCHEMA == 7
    opened = Project.open(path)
    held = opened[name]
    assert isinstance(held, TimeComparison)
    assert held.pairs == [['Test', 0, 'Model', 0], ['Test', 1, 'Model', 1]]
    assert held.sources == ['Test', 'Model']
    assert (held.start, held.stop) == (0.2, 0.7)
    assert opened.time_comparisons == [held]
    assert opened.stale() == {}
    assert scores(held)[0]['strac'] == pytest.approx(-0.5)


def test_a_build_that_reads_only_schema_6_refuses_the_file(tmp_path, monkeypatch):
    project = _project()
    project.compare_time_histories('Test', 'Model')
    path = tmp_path / 'compared.hforge'
    project.save(path)
    monkeypatch.setattr(native, 'SCHEMA_VERSION', native.ELEMENT_SHAPES_SCHEMA)
    with pytest.raises(ValueError, match=r'newer Hertzforge \(schema 7'):
        Project.open(path)


def test_a_project_with_no_comparison_keeps_its_older_stamp(tmp_path):
    path = tmp_path / 'plain.hforge'
    _project().save(path)
    with h5py.File(path) as f:
        assert f.attrs[native.STAMP] == native.BASE_SCHEMA


def test_a_changed_source_marks_the_comparison_stale_and_a_refresh_keeps_the_window():
    project = _project()
    name = project.compare_time_histories('Test', 'Model')
    project.set_time_comparison_window(name, 0.1, 0.6)
    assert project.stale() == {}
    project['Model'] = project['Model'].rescale(2.0)
    assert project.stale() == {name: "a source's values have changed"}
    project.refresh(name)
    assert project.stale() == {}
    assert (project[name].start, project[name].stop) == (0.1, 0.6)
    assert scores(project[name])[0]['strac'] == pytest.approx(-0.25)


def test_the_second_source_is_watched_as_well_as_the_first():
    project = _project()
    name = project.compare_time_histories('Test', 'Model')
    project['Model'] = project['Model'].rescale(3.0)
    assert name in project.stale()


def test_a_renamed_source_is_followed():
    project = _project()
    name = project.compare_time_histories('Test', 'Model')
    project.rename('Model', 'FE Prediction')
    assert project[name].sources == ['Test', 'FE Prediction']
    assert project[name].pairs[0] == ['Test', 0, 'FE Prediction', 0]
    assert project.provenance[name]['params']['sources'] == ['Test', 'FE Prediction']
    project['FE Prediction'] = project['FE Prediction'].rescale(2.0)
    project.refresh(name)
    assert project[name].pairs[0][2] == 'FE Prediction'


def test_hand_pairs_and_picked_records_come_through_the_project():
    project = _project()
    name = project.compare_time_histories('Test', 'Model', pairs=[['Test', 0, 'Model', 1]])
    assert project[name].pairs == [['Test', 0, 'Model', 1]]
    picked = project.compare_time_histories('Test', 'Model',
                                            records={'Test': [1], 'Model': [1]})
    assert project[picked].pairs == [['Test', 1, 'Model', 1]]
    assert picked == 'Time Response Comparison (2)'


def test_the_window_is_kept_only_on_a_comparison_and_only_inside_its_clock():
    project = _project()
    name = project.compare_time_histories('Test', 'Model', start=0.1, stop=0.9)
    with pytest.raises(ValueError, match='must lie inside the shared clock'):
        project.set_time_comparison_window(name, 0.5, 2.0)
    assert (project[name].start, project[name].stop) == (0.1, 0.9)
    with pytest.raises(TypeError, match="'Test' is not a time comparison"):
        project.set_time_comparison_window('Test', 0.1, 0.2)


def test_a_comparison_is_not_a_time_history():
    """Its two records at a DOF are two signals, not repeats of one
    channel, so nothing that averages a time history's records takes it."""
    project = _project()
    name = project.compare_time_histories('Test', 'Model')
    assert not isinstance(project[name], TimeHistory)
    assert name not in [project.name_of(h) for h in project.time_histories]


def test_the_tree_lists_it_by_its_pairs_under_the_time_histories():
    from hertzforge.project import describe, type_rank

    project = _project()
    name = project.compare_time_histories('Test', 'Model')
    assert describe(project[name]) == '2 pairs, 1000 samples'
    assert type_rank(project['Test']) < type_rank(project[name]) < type_rank(
        project['Test'].compute_spectra())


def test_a_comparison_written_as_a_time_history_with_its_pairs_opens_as_a_comparison(tmp_path):
    """An older file stored its comparison as a time history carrying the
    same `time_comparison` JSON beside its records."""
    import json

    project = _project()
    name = project.compare_time_histories('Test', 'Model')
    path = tmp_path / 'older.hforge'
    project.save(path)
    with h5py.File(path, 'r+') as f:
        group = next(g for g in f['objects'].values() if g.attrs.get('name') == name)
        group.attrs['kind'] = 'data'
        group.attrs['data_class'] = 'TimeHistory'
        group.attrs['time_comparison'] = json.dumps({
            'pairs': [['Test', 0, 'Model', 0], ['Test', 1, 'Model', 1]],
            'sources': ['Test', 'Model'], 'start': 0.1, 'stop': 0.8})
    reopened = Project.open(path)
    reopened.provenance[name]['state'] = ['content', 'what the other program recorded']
    assert reopened.stale() == {name: 'it was made by another program; refresh it to check '
                                      'it against its sources'}
    opened = reopened[name]
    assert isinstance(opened, TimeComparison)
    assert (opened.start, opened.stop) == (0.1, 0.8)
    assert scores(opened)[1]['strac'] == pytest.approx(2.0)


# ---- two groups picked together -----------------------------------------------

def _files():
    """Two imported files' histories, one record each at three DOFs, each
    file's histories in a group named for it, and a geometry in the test's
    group."""
    from hertzforge.core.geometry import Geometry

    t = np.arange(500) / 500.0
    project = Project()
    tests, models = [], []
    for side, gain, held in (('Test', 1.0, tests), ('Model', -2.0, models)):
        for k, dof in enumerate(['1X+', '2X+', '3X+']):
            name = f'{side} {dof}'
            project.add(name, _history(t, [gain * np.sin(2 * np.pi * (5 + k) * t)], [dof],
                                       ['acceleration']))
            held.append(name)
    project.add('Rig', Geometry(node_id=[1], node_xyz=[[0.0, 0.0, 0.0]]))
    project.object_groups.append({'members': [*tests, 'Rig'], 'role': None, 'name': 'test.unv'})
    project.object_groups.append({'members': models, 'role': None, 'name': 'model.unv'})
    return project, tests, models


def test_the_histories_of_two_groups_are_two_sets_in_project_order():
    project, tests, models = _files()
    # the model's picked first, each set backward: the sets still come in project order
    assert project.time_history_sets(*reversed(models), *reversed(tests)) == (tests, models)
    assert project.time_history_sets('Model 1X+', 'Test 2X+') == (['Model 1X+'], ['Test 2X+'])


@pytest.mark.parametrize(('picked', 'message'), [
    (['Test 1X+'], 'pick two time histories, or the histories of two groups'),
    (['Test 1X+', 'Test 2X+', 'Model 1X+'], 'Test 3X\\+ of test.unv is not picked'),
    (['Test 1X+', 'Test 2X+', 'Test 3X+'], 'the histories picked are all in one group'),
    (['Test 1X+', 'Model 1X+', 'Rig'], 'Rig is not a time history'),
])
def test_a_pick_that_is_not_two_whole_groups_is_refused(picked, message):
    project, _tests, _models = _files()
    with pytest.raises(ValueError, match=message):
        project.time_history_sets(*picked)


def test_a_history_in_no_group_is_refused_by_name_among_many():
    project, tests, models = _files()
    project.add('Loose', project['Test 1X+'])
    with pytest.raises(ValueError, match='Loose is in no group'):
        project.time_history_sets(*tests, *models, 'Loose')


def test_two_groups_compare_station_by_station_across_their_histories():
    project, tests, models = _files()
    name = project.compare_time_histories(*project.time_history_sets(*tests, *models)[0],
                                          *models)
    assert [row['strac'] for row in scores(project[name])] == pytest.approx([-0.5] * 3)
    assert project[name].pairs[2] == ['Test 3X+', 0, 'Model 3X+', 0]
