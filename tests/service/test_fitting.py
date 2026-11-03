"""Fitting modes by hand over the protocol: the core's fitting session, one
mode at a time on the residual, published to the project and edited again."""

from __future__ import annotations

import asyncio
from pathlib import Path

import numpy as np
import pytest

from hertzforge.service import fitting, frames, projects
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
