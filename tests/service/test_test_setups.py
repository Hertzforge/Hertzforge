"""Test setups through the analysis service: every module's configuration
saved with `project.save_setup`, written to the project file, reopened and
opened with `project.open_setup` is the configuration it was, and is what
that module's configure request takes (the protocol's schema); a setup
opened from each converted golden run folder is the configuration that
run was accepted with, which that module's configure request takes; the
verbs' messages follow the schema; and what is not a setup is refused."""

from __future__ import annotations

import json
import os
import shutil
from pathlib import Path

import pytest

from hertzforge.core.test_setup import CONFIGURE, MODULES
from hertzforge.io import run_convert
from hertzforge.service import projects
from hertzforge.service.protocol import ProtocolError

GOLDEN = Path(__file__).resolve().parents[2] / 'schema' / 'golden'
RUNS = GOLDEN / 'runs'

#: each module's configure request golden and its params' schema definition
REQUESTS = {'random': ('request_random_configure', 'randomConfigureParams'),
            'sine': ('request_sine_configure', 'sineConfigureParams'),
            'mixed': ('request_mixed_configure', 'mixedConfigureParams'),
            'classical_shock': ('request_shock_configure', 'shockConfigureParams'),
            'srs': ('request_srs_configure', 'srsConfigureParams'),
            'twr': ('request_twr_configure', 'twrConfigureParams'),
            'explorer': ('request_explorer_configure', 'explorerConfigureParams'),
            'recorder': ('request_recorder_configure', 'recorderConfigureParams'),
            'mimo_random': ('request_mimo_random_setup', 'mimoSetupParams'),
            'mimo_shock': ('request_mimo_shock_configure', 'mimoShockConfigureParams'),
            'playback': ('request_playback_configure', 'playbackConfigureParams')}

#: the golden run folders, by the module each ran
GOLDEN_RUNS = sorted(p.name for p in RUNS.iterdir() if (p / 'setup.json').exists())


def _request(module):
    name, _schema = REQUESTS[module]
    return json.loads((GOLDEN / f'{name}.json').read_text(encoding='utf-8'))


def _params(module, config):
    return {CONFIGURE[module][1]: config}


def test_every_module_is_covered_and_its_golden_is_its_configure_request():
    assert set(REQUESTS) == set(MODULES)
    for module in MODULES:
        assert _request(module)['method'] == CONFIGURE[module][0]
    assert set(GOLDEN_RUNS) <= set(MODULES) and 'random' in GOLDEN_RUNS


@pytest.mark.parametrize('module', MODULES)
def test_a_saved_setup_opens_back_as_its_configuration(module, tmp_path, validate):
    config = _request(module)['params'][CONFIGURE[module][1]]
    state = projects.new({'title': 'setups'})
    sent = {'module': module, 'protocol': 'sds/1', 'config': config,
            'table': {'revision': 2, 'rows': [{'index': 1, 'role': 'control'}]},
            'title': 'Saved', 'notes': '', 'origin': {'kind': 'typed'}}
    validate({'name': 'Saved', 'setup': sent}, 'projectSaveSetupParams')
    name = projects.save_setup(state, {'name': 'Saved', 'setup': sent})
    assert name == 'Saved' and state.modified
    path = projects.save(state, {'path': str(tmp_path / 'setups.hforge')})
    reopened = projects.open_file({'path': path})
    listed = projects.setups(reopened, {})
    validate(listed, 'projectSetupsResult')
    assert [(s['name'], s['module']) for s in listed['setups']] == [('Saved', module)]
    opened = projects.open_setup(reopened, {'name': 'Saved'})
    validate({'setup': opened}, 'projectOpenSetupResult')
    assert opened == sent
    validate(_params(module, opened['config']), REQUESTS[module][1])


@pytest.fixture(scope='module')
def converted(tmp_path_factory):
    out = {}
    for name in GOLDEN_RUNS:
        folder = str(tmp_path_factory.mktemp('runs') / name)
        shutil.copytree(RUNS / name, folder)
        out[name] = (folder, projects.open_file({'path': run_convert.convert(folder).path}))
    return out


@pytest.mark.parametrize('name', GOLDEN_RUNS)
def test_a_setup_opened_from_a_golden_run_is_what_its_configure_takes(name, converted,
                                                                       validate):
    folder, state = converted[name]
    with open(os.path.join(folder, 'setup.json'), encoding='utf-8') as f:
        accepted = json.load(f)['config']
    with open(os.path.join(folder, 'run.json'), encoding='utf-8') as f:
        manifest = json.load(f)
    listed = projects.setups(state, {})
    validate(listed, 'projectSetupsResult')
    (run,) = listed['runs']
    assert (run['module'], run['run_uid'], run['has_config']) == (name, manifest['run_uid'], True)
    opened = projects.open_setup(state, {'name': run['name']})
    validate({'setup': opened}, 'projectOpenSetupResult')
    assert opened['module'] == name
    assert opened['config'] == accepted
    assert opened['origin'] == {'kind': 'run', 'run_uid': manifest['run_uid'],
                                'run_id': manifest['run_id'], 'name': run['name']}
    with open(os.path.join(folder, 'table.json'), encoding='utf-8') as f:
        table = json.load(f)
    assert opened['table'] == {'rows': table['rows'], 'revision': table['revision'],
                               'sha256': table['sha256']}
    validate(_params(name, opened['config']), REQUESTS[name][1])


def test_what_is_not_a_setup_is_refused_and_a_name_is_not_overwritten(converted):
    _folder, state = converted['random']
    with pytest.raises(ProtocolError, match='is a channel table, not a setup'):
        projects.open_setup(state, {'name': 'Channel Table'})
    with pytest.raises(ProtocolError, match='no object named'):
        projects.open_setup(state, {'name': 'Nothing'})
    setup = {'module': 'random', 'config': {'title': 'x'}}
    with pytest.raises(ProtocolError, match='already holds an object named'):
        projects.save_setup(state, {'name': 'Channel Table', 'setup': setup, 'replace': True})
    assert projects.save_setup(state, {'name': 'Mine', 'setup': setup}) == 'Mine'
    with pytest.raises(ProtocolError, match='already holds an object named'):
        projects.save_setup(state, {'name': 'Mine', 'setup': setup})
    replaced = {'module': 'random', 'config': {'title': 'y'}}
    assert projects.save_setup(state, {'name': 'Mine', 'setup': replaced, 'replace': True}) == 'Mine'
    assert projects.open_setup(state, {'name': 'Mine'})['config'] == {'title': 'y'}
    with pytest.raises(ProtocolError, match='not \'shaker\''):
        projects.save_setup(state, {'name': 'Bad', 'setup': {'module': 'shaker', 'config': {}}})
    assert projects.setups(None, {}) == {'setups': [], 'runs': []}
    with pytest.raises(ProtocolError, match='none is open'):
        projects.open_setup(None, {'name': 'Mine'})


def test_the_verbs_answer_through_a_connection(validate):
    import asyncio

    from hertzforge.service.server import Connection, Service

    service = Service()
    connection = Connection(service, None)
    connection.project = projects.new({'title': 't'})
    config = _request('sine')['params']['config']
    try:
        saved = asyncio.run(connection.project_request('project.save_setup', {
            'name': 'Sweep', 'setup': {'module': 'sine', 'config': config}}))
        validate(saved, 'projectSaveSetupResult')
        assert saved['name'] == 'Sweep' and saved['project']['modified']
        (entry,) = [o for o in saved['project']['objects'] if o['name'] == 'Sweep']
        assert (entry['kind'], entry['holds']) == ('test_setup', 'sine setup, typed')
        listed = asyncio.run(connection.project_request('project.setups', {}))
        validate(listed, 'projectSetupsResult')
        opened = asyncio.run(connection.project_request('project.open_setup', {'name': 'Sweep'}))
        validate(opened, 'projectOpenSetupResult')
        assert opened['setup']['config'] == config
        with pytest.raises(ProtocolError, match='arm'):
            asyncio.run(connection.project_request('project.open_setup',
                                                   {'name': 'Sweep', 'arm': True}))
    finally:
        service.close()
