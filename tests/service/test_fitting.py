"""Fitting modes by hand over the protocol: the core's fitting session, one
mode at a time on the residual, published to the project and edited again."""

from __future__ import annotations

import asyncio
from pathlib import Path

import numpy as np
import pytest

from hertzforge import Project
from hertzforge.core.data import Frf, MultipleCoherence
from hertzforge.core.shapes import ShapeSet, synthesize_overlay
from hertzforge.service import fitting, frames, projects
from hertzforge.service.action_table import action_table
from hertzforge.service.protocol import ProtocolError

from .test_projects import _exchange

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'


@pytest.fixture
def project():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'frfs.unv')})
    return state.project


def test_a_fit_finds_fits_and_confirms_one_mode_at_a_time(project, validate):
    fit = fitting.start(project, {'frf': 'FRF'})
    first = fitting.state(fit)
    validate(frames.to_wire(first)[0], 'fitState')
    assert first['modes'] == [] and first['preview'] is None
    assert first['pending']['frequency'] > 0, 'a suggestion is made at once'
    fitting.act(fit, 'fit.fit', {})
    previewed = fitting.state(fit)
    assert previewed['preview']['frequency'] == pytest.approx(first['pending']['frequency'])
    names = [s['name'] for s in previewed['view']['series']]
    assert 'CMIF 1, synthesized' in names and 'Pending mode' in names
    fitting.act(fit, 'fit.confirm', {'description': 'first bending'})
    fitting.act(fit, 'fit.find', {})
    second = fitting.state(fit)
    assert len(second['modes']) == 1 and second['modes'][0]['description'] == 'first bending'
    assert second['pending']['frequency'] != pytest.approx(first['pending']['frequency']), \
        'the next suggestion is on the residual'


def test_the_zoom_is_the_search_band(project):
    fit = fitting.start(project, {'frf': 'FRF'})
    fitting.act(fit, 'fit.find', {'low': 600.0, 'high': 900.0})
    assert 600.0 <= fitting.state(fit)['pending']['frequency'] <= 900.0


def test_the_cursor_and_a_held_damping_are_the_users(project):
    fit = fitting.start(project, {'frf': 'FRF'})
    fitting.act(fit, 'fit.move', {'frequency': 700.0})
    assert fitting.state(fit)['pending']['frequency'] == pytest.approx(700.0)
    fitting.act(fit, 'fit.damping', {'damping': 0.05})
    held = fitting.state(fit)['pending']
    assert held['held'] and held['damping'] == pytest.approx(0.05)


def test_publishing_adds_a_shape_set_and_edit_fit_replaces_it(project):
    fit = fitting.start(project, {'frf': 'FRF'})
    for _ in range(2):
        fitting.act(fit, 'fit.confirm', {})
        fitting.act(fit, 'fit.find', {})
    name = fitting.publish(project, fit, {})
    assert name == 'FRF Modes' and project[name].num_shapes == 2
    edit = fitting.start(project, {'frf': 'FRF', 'shapes': name})
    assert len(fitting.state(edit)['modes']) == 2
    fitting.act(edit, 'fit.confirm', {})
    assert fitting.publish(project, edit, {}) == name
    assert project[name].num_shapes == 3 and list(project).count(name) == 1


def test_records_picked_limit_the_fit(project):
    every = fitting.start(project, {'frf': 'FRF'})
    picked = fitting.start(project, {'frf': 'FRF', 'records': [0, 1]})
    assert picked.session.rows == [0, 1]
    assert len(every.session.rows) > 2
    with pytest.raises(ProtocolError, match='not \\[1000000\\]'):
        fitting.start(project, {'frf': 'FRF', 'records': [1000000]})


def test_refusals_are_in_the_cores_words_or_name_the_problem(project):
    from hertzforge.core.data import TimeHistory

    project.add('Run', TimeHistory(np.arange(4.0), np.ones((1, 4)), response_dof='1Z+'))
    with pytest.raises(ProtocolError, match='not an FRF'):
        fitting.start(project, {'frf': 'Run'})
    with pytest.raises(ProtocolError, match='not a shape set'):
        fitting.start(project, {'frf': 'FRF', 'shapes': 'Run'})
    with pytest.raises(ProtocolError) as error:
        fitting.start(project, {'frf': 'Nothing'})
    assert error.value.code == 'not_found'
    fit = fitting.start(project, {'frf': 'FRF'})
    with pytest.raises(ProtocolError, match='confirm a mode'):
        fitting.publish(project, fit, {})
    with pytest.raises(ProtocolError, match='no confirmed modes'):
        fitting.act(fit, 'fit.refine', {})
    with pytest.raises(ProtocolError, match='"frequency", a number') as error:
        fitting.act(fit, 'fit.move', {'frequency': 'high'})
    assert error.value.code == 'bad_request'


def test_a_fit_runs_over_the_socket_and_ends_with_its_project(validate):
    responses = asyncio.run(_exchange([
        ('fit.find', {}),
        ('project.import', {'path': str(PLATE / 'frfs.unv')}),
        ('fit.start', {'frf': 'FRF'}),
        ('fit.fit', {}),
        ('fit.confirm', {}),
        ('fit.publish', {'name': 'Plate Modes'}),
        ('project.new', {}),
        ('fit.find', {}),
        ('fit.end', {}),
    ]))
    results = [got[-1] for got in responses]
    assert results[0]['error']['code'] == 'refused', 'no project open'
    started = results[2]['result']
    validate(frames.to_wire(started)[0], 'fitResult')
    assert started['fit']['view']['series'][0]['name'] == 'CMIF 1'
    assert results[4]['result']['fit']['modes']
    published = results[5]['result']
    validate(frames.to_wire(published)[0], 'fitPublishResult')
    assert published['name'] == 'Plate Modes'
    assert 'Plate Modes' in [o['name'] for o in published['project']['objects']]
    assert published['project']['modified']
    assert 'needs a fit under way' in results[7]['error']['message']
    assert results[8]['result'] == {}


def test_a_fit_ends_when_its_frf_leaves_the_project():
    responses = asyncio.run(_exchange([
        ('project.import', {'path': str(PLATE / 'frfs.unv')}),
        ('fit.start', {'frf': 'FRF'}),
        ('project.remove', {'names': ['FRF']}),
        ('fit.find', {}),
        ('fit.find', {}),
    ]))
    removed, gone, after = (got[-1] for got in responses[2:])
    assert 'result' in removed
    assert gone['error']['code'] == 'refused'
    assert 'no longer in the project' in gone['error']['message']
    assert 'needs a fit under way' in after['error']['message'], 'the fit ended'


def test_a_published_fit_joins_its_frfs_group_as_the_cores_fit_does(project):
    projects.import_file(projects.OpenProject(project), {'path': str(PLATE / 'geometry.unv')})
    project.link('FRF', 'Geometry')
    fit = fitting.start(project, {'frf': 'FRF'})
    fitting.act(fit, 'fit.confirm', {})
    name = fitting.publish(project, fit, {})
    assert any({'FRF', name, 'Geometry'} <= set(group['members'])
               for group in project.object_groups)
    assert project.geometry_for(name)[0] == 'Geometry'


def test_the_fits_mac_holds_the_confirmed_modes_and_the_preview_last(project, validate):
    fit = fitting.start(project, {'frf': 'FRF'})
    assert fitting.state(fit)['mac'] is None, 'one mode is no matrix'
    fitting.act(fit, 'fit.confirm', {})
    fitting.act(fit, 'fit.find', {})
    fitting.act(fit, 'fit.fit', {})
    state = fitting.state(fit)
    validate(frames.to_wire(state)[0], 'fitState')
    mac = state['mac']
    assert mac['rows']['ticks'][0].startswith('1: ')
    assert mac['rows']['ticks'][-1].startswith('New: ')
    values = np.asarray(mac['values'])
    assert values.shape == (2, 2) and np.allclose(np.diag(values), 1.0)


def _two_confirmed(project):
    fit = fitting.start(project, {'frf': 'FRF'})
    for _ in range(2):
        fitting.act(fit, 'fit.confirm', {})
        fitting.act(fit, 'fit.find', {})
    assert len(fit.session.modes) == 2
    return fit


def test_a_confirmed_mode_is_taken_back(project, validate):
    fit = _two_confirmed(project)
    kept = fit.session.modes[1]['frequency']
    fitting.act(fit, 'fit.fit', {})
    fitting.act(fit, 'fit.delete', {'modes': [0]})
    after = fitting.state(fit)
    validate(frames.to_wire(after)[0], 'fitState')
    assert [m['frequency'] for m in after['modes']] == [kept]
    assert after['preview'] is None, 'fitted against a residual that is gone'
    assert len(fit.session.confirmed) == 1
    with pytest.raises(ProtocolError, match='there is no mode 3'):
        fitting.act(fit, 'fit.delete', {'modes': [2]})


def test_a_modes_damping_and_description_are_typed_into_its_row(project):
    fit = _two_confirmed(project)
    residual = fit.session.residual_matrix().copy()
    fitting.act(fit, 'fit.edit', {'mode': 0, 'damping': 0.05, 'description': ' twist '})
    mode = fitting.state(fit)['modes'][0]
    assert mode['damping'] == pytest.approx(0.05) and mode['description'] == 'twist'
    assert not np.allclose(residual, fit.session.residual_matrix()), \
        'the typed damping moves the residual'
    with pytest.raises(ProtocolError, match='fraction of critical'):
        fitting.act(fit, 'fit.edit', {'mode': 0, 'damping': 1.5})
    with pytest.raises(ProtocolError, match='"damping" or "description"'):
        fitting.act(fit, 'fit.edit', {'mode': 0})


def test_the_residual_cmif_is_drawn_on_request(project, validate):
    fit = _two_confirmed(project)
    names = lambda: [s['name'] for s in fitting.state(fit)['view']['series']]
    assert 'CMIF 1, residual' not in names()
    fitting.act(fit, 'fit.residual', {'show': True})
    state = fitting.state(fit)
    validate(frames.to_wire(state)[0], 'fitState')
    assert state['residual'] and 'CMIF 1, residual' in names()


def test_the_half_power_reading_is_under_the_cursor(project):
    fit = fitting.start(project, {'frf': 'FRF'})
    reading = fitting.state(fit)['half_power']
    assert reading['frequency'] == pytest.approx(fit.session.pending['frequency'],
                                                 rel=0.05)
    if reading['q'] is not None:
        assert reading['damping'] == pytest.approx(0.5 / reading['q'])
        assert reading['low'] < reading['frequency'] < reading['high']


def test_a_fit_weighted_by_coherence(validate):
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    project = state.project
    frf = project.compute_frfs('Time History')
    coherence = project.compute_multiple_coherence('Time History')
    fit = fitting.start(project, {'frf': frf, 'coherence': coherence})
    assert fit.session.weights is not None
    out = fitting.state(fit)
    validate(frames.to_wire(out)[0], 'fitState')
    assert out['coherence'] == coherence
    with pytest.raises(ProtocolError, match='is not a coherence'):
        fitting.start(project, {'frf': frf, 'coherence': 'Time History'})


def _through_the_server(*requests):
    """Each (method, params) answered by a `Connection` holding the plate's
    FRFs, past the parameter check `fitting.start` alone never sees; the fit
    left and the last answer."""
    from hertzforge.service import server

    async def main():
        service = server.Service()
        connection = server.Connection(service, None)
        connection.project, _ = projects.import_file(None, {'path': str(PLATE / 'frfs.unv')})
        answer = None
        for method, params in requests:
            answer = await connection.fit_request(method, params)
        service.close()
        return connection.fit, answer

    return asyncio.run(main())


def test_records_picked_in_the_tree_reach_the_fit_through_the_server():
    fit, answer = _through_the_server(('fit.start', {'frf': 'FRF', 'records': [0, 1]}))
    assert fit.session.rows == [0, 1] and answer['fit']['frf'] == 'FRF'


PHI = np.array([[1, 1, 1, 1], [1, 0.5, -0.5, -1], [1, -1, -1, 1.0]]) * 0.3
DOFS = ['1Z+', '2Z+', '3Z+', '4Z+']
LINES = np.arange(0.0, 200.01, 0.25)


def _modes(**more):
    return ShapeSet([20.0, 50.0, 90.0], [0.01, 0.02, 0.015], DOFS, PHI, mass_unit='kg', **more)


def _frf(pairs=None, truth=None, power=2, dim='acceleration/force'):
    pairs = pairs or [(dof, '1Z+') for dof in DOFS]
    responses, references = zip(*pairs)
    values = (truth or _modes()).synthesize_frf(LINES, responses, references, power=power)
    return Frf(LINES, values, response_dof=list(responses), reference_dof=list(references),
               ordinate_dim=[dim] * len(pairs))


def _row_of(project, objects, row_id, records=None):
    table = action_table(project, objects, records or {})
    return next(row for column in table['columns'] for row in column['rows']
                if row['id'] == row_id)


def _project_of(**objects):
    project = Project()
    for name, obj in objects.items():
        project.add(name, obj)
    return project


def _confirm_all(fit, count=3):
    for _ in range(count):
        fitting.act(fit, 'fit.find', {})
        fitting.act(fit, 'fit.fit', {})
        fitting.act(fit, 'fit.confirm', {})


def test_edit_fit_refuses_a_set_sharing_no_dofs_and_leaves_it_alone():
    other = ShapeSet([30.0], [0.01], ['7Z+', '8Z+'], [[1.0, 2.0]], mass_unit='kg')
    project = _project_of(H=_frf(), FE=other)
    row = _row_of(project, ['H', 'FE'], 'self.edit_fit')
    assert not row['ready'] and 'no value' in row['reason']
    assert _row_of(project, ['H'], 'self.edit_fit')['candidates'] == []
    with pytest.raises(ProtocolError, match='no value'):
        fitting.start(project, {'frf': 'H', 'shapes': 'FE'})
    assert project['FE'].coordinate == ['7Z+', '8Z+']


def test_edit_fit_refuses_a_complex_set_and_says_so_before_it_is_clicked():
    complex_set = ShapeSet([20.0, 50.0, 90.0], [0.01, 0.02, 0.015], DOFS, PHI * np.exp(0.2j),
                           modal_mass=[2j, 2j, 2j], mass_unit='kg')
    project = _project_of(H=_frf(), C=complex_set)
    row = _row_of(project, ['H', 'C'], 'self.edit_fit')
    assert not row['ready'] and 'complex' in row['reason']
    with pytest.raises(ProtocolError, match='complex'):
        fitting.start(project, {'frf': 'H', 'shapes': 'C'})


def test_edit_fit_keeps_the_dofs_it_does_not_fit_and_what_else_the_set_held():
    extra = np.array([[0.7, 0.1], [0.2, -0.3], [-0.4, 0.5]])
    big = ShapeSet([20.0, 50.0, 90.0], [0.01, 0.02, 0.015], [*DOFS, '5Z+', '6Z+'],
                   np.hstack([PHI, extra]), mass_unit='kg', comment=['a', 'b', 'c'],
                   description=['d1', 'd2', 'd3'], modal_damping=[1j, 2j, 3j])
    project = _project_of(H=_frf(), Big=big)
    assert _row_of(project, ['H', 'Big'], 'self.edit_fit')['ready']
    fit = fitting.start(project, {'frf': 'H', 'shapes': 'Big'})
    with pytest.raises(ProtocolError, match='5Z\\+ and 1 more'):
        fitting.act(fit, 'fit.confirm', {})
    assert fitting.publish(project, fit, {}) == 'Big'
    out = project['Big']
    assert out.coordinate == [*DOFS, '5Z+', '6Z+']
    assert np.allclose(out.shape_matrix, np.hstack([PHI, extra]))
    assert out.comment == ['a', 'b', 'c'] and out.description == ['d1', 'd2', 'd3']
    assert list(out.modal_damping) == [1j, 2j, 3j]


def test_a_kept_dof_follows_its_mode_when_the_fit_flips_it():
    big = ShapeSet([20.0], [0.01], [*DOFS, '5Z+'], [[*PHI[0], 0.7]], mass_unit='kg')
    project = _project_of(H=_frf(), Big=big)
    fit = fitting.start(project, {'frf': 'H', 'shapes': 'Big'})
    fit.session.modes[0]['shape'] = -2.0 * fit.session.modes[0]['shape']
    published = fit.session.shape_set()
    assert np.allclose(published.shape_matrix, [[*(-2.0 * PHI[0]), -1.4]])


def test_edit_fit_honors_a_modal_mass():
    scaled = ShapeSet([20.0, 50.0, 90.0], [0.01, 0.02, 0.015], DOFS, PHI * 2.0,
                      modal_mass=[4.0, 4.0, 4.0], mass_unit='kg')
    measured = _frf(truth=scaled)
    project = _project_of(H=measured, Set=scaled)
    fit = fitting.start(project, {'frf': 'H', 'shapes': 'Set'})
    assert np.abs(fit.session.residual_matrix()).max() < 1e-9 * np.abs(fit.session.matrix).max()
    fitting.publish(project, fit, {})
    out = project['Set']
    assert list(out.modal_mass) == [4.0, 4.0, 4.0]
    assert np.allclose(out.shape_matrix, PHI * 2.0)
    assert np.allclose(out.synthesize_frf(LINES, DOFS, ['1Z+'] * 4, power=2), measured.ordinate)


def test_edit_fit_works_on_the_records_its_set_holds():
    part = ShapeSet([20.0, 50.0, 90.0], [0.01, 0.02, 0.015], DOFS[:2], PHI[:, :2], mass_unit='kg')
    project = _project_of(H=_frf(), Part=part)
    fit = fitting.start(project, {'frf': 'H', 'shapes': 'Part'})
    assert fit.session.rows == [0, 1] and fit.session.coordinate == DOFS[:2]
    fitting.publish(project, fit, {})
    assert project['Part'].coordinate == DOFS[:2]
    assert np.allclose(project['Part'].shape_matrix, PHI[:, :2])


def test_an_unmeasured_pair_is_refused_rather_than_fitted_as_a_zero():
    pairs = [(dof, '1Z+') for dof in DOFS] + [('1Z+', '4Z+'), ('4Z+', '4Z+')]
    project = _project_of(H=_frf(pairs))
    row = _row_of(project, ['H'], 'self.fit')
    assert not row['ready'] and 'measures 2Z+ against 4Z+' in row['reason']
    with pytest.raises(ProtocolError, match='2Z\\+ against 4Z\\+, and 1 more'):
        fitting.start(project, {'frf': 'H'})
    assert _row_of(project, ['H'], 'self.fit', {'H': [0, 1, 2, 3]})['ready']
    fit = fitting.start(project, {'frf': 'H', 'records': [0, 1, 2, 3]})
    _confirm_all(fit)
    shapes = np.array([mode['shape'] for mode in fit.session.modes])
    assert np.allclose(np.abs(shapes), np.abs(PHI), atol=1e-3)


def test_a_drive_point_written_with_opposite_signs_is_a_drive_point():
    plain = _frf()
    negated = Frf(LINES, -plain.ordinate, response_dof=DOFS, reference_dof=['1Z-'] * 4,
                  ordinate_dim=['acceleration/force'] * 4)
    project = _project_of(H=negated)
    fit = fitting.start(project, {'frf': 'H'})
    _confirm_all(fit)
    published = fit.session.shape_set()
    assert not published.unscaled and published.coordinate == DOFS
    assert np.allclose(np.abs(published.shape_matrix), np.abs(PHI), atol=1e-3)
    overlay = synthesize_overlay(published, negated)
    scale = np.abs(negated.ordinate).max()
    assert np.abs(overlay.ordinate - negated.ordinate).max() < 1e-3 * scale
    assert np.allclose(fit.session.synthesis_records(), overlay.ordinate)


def test_fit_modes_stops_when_no_mode_is_left():
    from hertzforge.service.ops_project import OPERATIONS

    project = _project_of(H=_frf())
    op = next(op for op in OPERATIONS if op.id == 'project.fit_modes')
    values = op.resolve({'frf': 'H'}, objects={'H': 'frf'},
                        verbs={'H': projects.verbs_of(project, 'H')})
    assert values['count'] == 10
    op.run(project, values)
    found = project['H Modes']
    assert found.frequency == pytest.approx([20.0, 50.0, 90.0], abs=1e-3)
    assert found.damping == pytest.approx([0.01, 0.02, 0.015], abs=1e-4)


@pytest.mark.parametrize('dim', ['acceleration/voltage', 'acceleration/acceleration',
                                 'force/force'])
def test_a_fit_not_per_force_is_not_labeled_mass_normalized(dim):
    project = _project_of(H=_frf(dim=dim))
    fit = fitting.start(project, {'frf': 'H'})
    _confirm_all(fit)
    published = fit.session.shape_set()
    assert published.unscaled and published.mass_unit is None


def test_a_fit_per_force_stays_labeled_mass_normalized():
    project = _project_of(H=_frf())
    fit = fitting.start(project, {'frf': 'H'})
    _confirm_all(fit)
    published = fit.session.shape_set()
    assert not published.unscaled and published.mass_unit == 'kg'


def test_an_uneven_grid_is_searched_at_its_own_spacing():
    logs = np.geomspace(1.0, 200.0, 600)
    values = _modes().synthesize_frf(logs, DOFS, ['1Z+'] * 4, power=2)
    measured = Frf(logs, values, response_dof=DOFS, reference_dof=['1Z+'] * 4,
                   ordinate_dim=['acceleration/force'] * 4)
    fit = fitting.start(_project_of(H=measured), {'frf': 'H'})
    _confirm_all(fit)
    found = [mode['frequency'] for mode in fit.session.modes]
    assert found == pytest.approx([20.0, 50.0, 90.0], abs=5e-3)
    left = np.abs(fit.session.residual_matrix()).max() / np.abs(fit.session.matrix).max()
    assert left < 1e-2


def test_refine_takes_an_adopted_rigid_mode_on_a_zero_hertz_line():
    rigid = ShapeSet([0.0, 20.0, 20.6], [0.0, 0.01, 0.01], DOFS, PHI, mass_unit='kg')
    project = _project_of(H=_frf(truth=rigid), Set=rigid)
    assert project['H'].abscissa[0] == 0.0
    fit = fitting.start(project, {'frf': 'H', 'shapes': 'Set'})
    fitting.act(fit, 'fit.refine', {})
    assert all(np.isfinite(mode['shape']).all() for mode in fit.session.modes)


@pytest.mark.parametrize('lines', [1, 2])
def test_too_few_lines_is_a_reading_of_nothing_through_the_server(lines):
    from hertzforge.service import server

    plain = _frf()
    short = Frf(LINES[40:40 + lines], plain.ordinate[:, 40:40 + lines], response_dof=DOFS,
                reference_dof=['1Z+'] * 4, ordinate_dim=['acceleration/force'] * 4)

    async def main():
        service = server.Service()
        connection = server.Connection(service, None)
        connection.project = projects.OpenProject(_project_of(H=short))
        answer = await connection.fit_request('fit.start', {'frf': 'H'})
        service.close()
        return answer

    reading = asyncio.run(main())['fit']['half_power']
    assert reading['q'] is None and 'three lines' in reading['notes'][0]


def test_the_cursor_stays_on_the_data_and_a_search_above_zero():
    project = _project_of(H=_frf())
    fit = fitting.start(project, {'frf': 'H'})
    for outside in (-5.0, 1e6):
        with pytest.raises(ProtocolError, match='outside the 0 to 200 Hz'):
            fitting.act(fit, 'fit.move', {'frequency': outside})
    zeros = Frf(LINES, 0.0 * _frf().ordinate, response_dof=DOFS, reference_dof=['1Z+'] * 4,
                ordinate_dim=['acceleration/force'] * 4)
    silent = fitting.start(_project_of(H=zeros), {'frf': 'H'})
    assert fitting.state(silent)['pending']['frequency'] >= 0.0


def test_one_damping_limit_refused_alike_by_the_row_and_the_hold():
    fit = fitting.start(_project_of(H=_frf()), {'frf': 'H'})
    _confirm_all(fit, 1)
    for bad in (-1.0, 3.0, 0.6, 1e-5):
        with pytest.raises(ProtocolError, match='from 0.0001'):
            fitting.act(fit, 'fit.damping', {'damping': bad})
        with pytest.raises(ProtocolError, match='from 0.0001'):
            fitting.act(fit, 'fit.edit', {'mode': 0, 'damping': bad})
    fitting.act(fit, 'fit.damping', {'damping': 0.5})
    assert fit.session.pending['damping'] == 0.5


def test_a_typed_damping_keeps_the_replay_list_in_step():
    fit = fitting.start(_project_of(H=_frf()), {'frf': 'H'})
    _confirm_all(fit, 3)
    fitting.act(fit, 'fit.edit', {'mode': 1, 'damping': 0.03})
    assert sorted(fit.session.confirmed)[1][1] == 0.03
    fitting.act(fit, 'fit.delete', {'modes': [1]})
    assert len(fit.session.modes) == len(fit.session.confirmed) == 2


def test_a_published_fit_journals_the_line_that_replays_it():
    project = _project_of(H=_frf())
    project.journal.clear()
    fit = fitting.start(project, {'frf': 'H'})
    _confirm_all(fit, 2)
    assert fitting.publish(project, fit, {'name': 'Modes'}) == 'Modes'
    _confirm_all(fit, 1)
    fitting.act(fit, 'fit.refine', {})
    fitting.publish(project, fit, {})
    assert len(project.journal) == 1, 'the line settles as the fit grows'
    line = project.journal[0]
    assert line.startswith("project.fit_modes('H', at=[") and "name='Modes'" in line
    published = project['Modes']
    again = _project_of(H=_frf())
    exec(line, {'project': again})
    assert np.allclose(again['Modes'].shape_matrix, published.shape_matrix)
    assert project.provenance['Modes']['verb'] == 'fit_modes'
    project.refresh('Modes')
    assert np.allclose(project['Modes'].shape_matrix, published.shape_matrix)


def test_an_edit_no_call_can_make_journals_that_it_cannot_replay():
    big = ShapeSet([20.0], [0.01], [*DOFS, '5Z+'], [[*PHI[0], 0.7]], mass_unit='kg')
    project = _project_of(H=_frf(), Big=big)
    project.journal.clear()
    fit = fitting.start(project, {'frf': 'H', 'shapes': 'Big'})
    fitting.publish(project, fit, {})
    assert project.journal[-1].startswith('#') and 'not replayable' in project.journal[-1]
    assert 'Big' not in project.provenance


def _noisy_channel(seed=1):
    """Three separated modes with noise on 3Z+, and a coherence that says so."""
    measured = _frf()
    rng = np.random.default_rng(seed)
    measured.ordinate[2] += rng.standard_normal(len(LINES)) + 1j * rng.standard_normal(len(LINES))
    gamma = np.full((4, len(LINES)), 0.98)
    gamma[2] = 0.1
    return _project_of(H=measured, C=MultipleCoherence(LINES, gamma, response_dof=DOFS))


def _fitted(project, **more):
    fit = fitting.start(project, {'frf': 'H', **more})
    _confirm_all(fit)
    return fit.session


@pytest.mark.parametrize('seed', [1, 2, 3])
def test_coherence_keeps_a_noisy_channels_lines_from_being_offered_and_fit(seed):
    project = _noisy_channel(seed)
    weighted = _fitted(project, coherence='C')
    found = [mode['frequency'] for mode in weighted.modes]
    assert found == pytest.approx([20.0, 50.0, 90.0], abs=0.25)
    shapes = np.array([mode['shape'] for mode in weighted.modes])
    plain_found = np.array([mode['frequency'] for mode in _fitted(project).modes])
    off = np.abs(np.array(found) - [20.0, 50.0, 90.0]).max()
    assert off < 0.1 * np.abs(plain_found - [20.0, 50.0, 90.0]).max(), 'plain it chases the noise'
    good = [0, 1, 3]
    assert np.abs(np.abs(shapes[:, good]) - np.abs(PHI[:, good])).max() < 0.02


def test_coherence_moves_a_noisy_channels_shape_toward_its_better_lines():
    measured = _frf()
    rng = np.random.default_rng(4)
    bad = np.abs(LINES - 50.0) < 0.6
    measured.ordinate[2, bad] += (rng.standard_normal(bad.sum())
                                  + 1j * rng.standard_normal(bad.sum()))
    gamma = np.full((4, len(LINES)), 0.98)
    gamma[2, bad] = 0.1
    project = _project_of(H=measured, C=MultipleCoherence(LINES, gamma, response_dof=DOFS))
    error = {}
    for label, more in (('plain', {}), ('weighted', {'coherence': 'C'})):
        fit = fitting.start(project, {'frf': 'H', **more})
        fitting.act(fit, 'fit.move', {'frequency': 50.0})
        fitting.act(fit, 'fit.confirm', {})
        shape = fit.session.modes[0]['shape']
        error[label] = abs(abs(shape[2]) - abs(PHI[1, 2]))
    assert error['weighted'] < 0.5 * error['plain']


def test_coherence_leaves_clean_data_where_it_was_and_a_dip_at_a_peak_does_not_split_it():
    project = _project_of(H=_frf())
    gamma = np.full((4, len(LINES)), 0.999)
    for peak in (20.0, 50.0, 90.0):
        gamma -= 0.09 * np.exp(-((LINES - peak) / 1.0) ** 2)
    project.add('C', MultipleCoherence(LINES, gamma, response_dof=DOFS))
    session = _fitted(project, coherence='C')
    assert [m['frequency'] for m in session.modes] == pytest.approx([20.0, 50.0, 90.0], abs=5e-3)
    assert [m['damping'] for m in session.modes] == pytest.approx([0.01, 0.02, 0.015], abs=1e-4)
    shapes = np.array([mode['shape'] for mode in session.modes])
    assert np.allclose(np.abs(shapes), np.abs(PHI), atol=5e-3)
    assert not session.shape_set().unscaled


def test_a_line_that_is_not_finite_is_left_out_of_the_fit():
    rigid = ShapeSet([0.0, 20.0, 50.0], [0.0, 0.01, 0.02], DOFS, PHI, mass_unit='kg')
    project = _project_of(H=_frf(truth=rigid, power=0, dim='length/force'))
    assert np.isnan(project['H'].ordinate[:, 0]).all()
    fit = fitting.start(project, {'frf': 'H'})
    assert fit.session.frequencies[0] == 0.25
    fitting.act(fit, 'fit.find', {})
    assert fitting.state(fit)['pending']['frequency'] > 0.0
