"""What the service does to a project replays from its journal: the session
script, run, rebuilds what the session built."""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge as hz
from hertzforge.core.shocks import Shock
from hertzforge.service import projects
from hertzforge.service.server import OPERATIONS
from hertzforge.units import SYSTEMS

OPS = {op.id: op for op in OPERATIONS}


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj) for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return op.run(project, values)


def replayed(project):
    script = project.session_script()
    assert 'not replayable' not in script
    space: dict = {}
    exec(compile(script, 'session', 'exec'), space)
    return space['project']


def opened(*paths, **options):
    state = None
    for path in paths:
        state, _added = projects.import_file(state, {'path': path, **options})
    return state


def test_units_declared_replay(tmp_path):
    state = opened(fixture_path('plate', 'frfs.npz'))
    project = state.project
    name = next(iter(project))
    tables = projects.units(state, {'name': name})['tables']
    rows = [{'side': table['side'], 'key': row['key'],
             'unit': 'g' if table['side'] == 'response' else 'lbf'}
            for table in tables for row in table['rows']]
    projects.define_units(state, {'name': name, 'units': rows})
    back = replayed(project)
    assert back[name].ordinate_unit == project[name].ordinate_unit
    assert back[name].reference_unit == project[name].reference_unit
    assert np.allclose(back[name].ordinate, project[name].ordinate)


def test_a_geometry_and_a_shape_set_declared_replay():
    state = opened(fixture_path('plate', 'geometry.npz'))
    state, _ = projects.import_file(state, {'path': fixture_path('plate', 'shapes.npy')})
    project = state.project
    projects.define_units(state, {'name': 'Geometry', 'units': [
        {'side': 'object', 'key': 'length', 'unit': 'in'}]})
    shapes = next(n for n in project if n != 'Geometry')
    projects.define_units(state, {'name': shapes, 'units': [
        {'side': 'object', 'key': 'mass', 'unit': 'slinch'}]})
    back = replayed(project)
    assert back['Geometry'].length_unit == 'in'
    assert np.allclose(back['Geometry'].node_xyz, project['Geometry'].node_xyz)
    assert back[shapes].mass_unit == 'slinch'


def test_the_settings_replay():
    state = opened(fixture_path('plate', 'random.nc4'))
    project = state.project
    run(project, 'project.set_averaging', source='Time History', frame_length=1024,
        overlap=0.0, window='boxcar', frames=4, start=0.0, detrend='none')
    psd = project.compute_psds('Time History')
    run(project, 'project.set_filter', source='Time History', kind='low-pass', high=200.0,
        order=4)
    run(project, 'project.set_truncation', source='Time History',
        start=float(project['Time History'].abscissa[1]), stop=float(project['Time History'].abscissa[2000]))
    back = replayed(project)
    for attribute in ('averaging', 'filtering', 'truncation'):
        assert getattr(back['Time History'], attribute) == getattr(project['Time History'], attribute)
    assert back[psd].ordinate.shape == project[psd].ordinate.shape
    assert np.allclose(back[psd].ordinate, project[psd].ordinate)


def test_detected_averaging_replays():
    state = opened(fixture_path('plate', 'random.nc4'))
    run(state.project, 'project.detect_averaging', source='Time History')
    assert replayed(state.project)['Time History'].averaging == state.project['Time History'].averaging


def test_shock_windows_replay():
    state = opened(fixture_path('plate', 'shock.nc4'))
    project = state.project
    history = next(n for n, o in project.items() if isinstance(o, hz.TimeHistory))
    origin = float(project[history].abscissa[0])
    run(project, 'project.set_shock', history=history, shock=1, start=origin + 0.01,
        stop=origin + 0.02)
    assert isinstance(project[history].shocks[0], Shock)
    assert replayed(project)[history].shocks == project[history].shocks


def geometry_session():
    state = opened(fixture_path('plate', 'geometry.npz'))
    return state.project


def test_geometry_edits_replay():
    project = geometry_session()
    run(project, 'project.add_node', geometry='Geometry', x=1.0, y=2.0, z=3.0)
    run(project, 'project.add_coordinate_system', geometry='Geometry', x=0.5, rz=30.0, name='turned')
    cs = int(project['Geometry'].cs_id[-1])
    run(project, 'project.rotate_coordinate_system', geometry='Geometry', cs=cs, axis='x', angle=10.0)
    run(project, 'project.slide_coordinate_system', geometry='Geometry', cs=cs, axis='y', distance=2.0)
    run(project, 'project.add_element_group', geometry='Geometry', name='added')
    run(project, 'project.add_element', geometry='Geometry', family='triangles',
        nodes=' '.join(str(int(n)) for n in project['Geometry'].node_id[:3]))
    back = replayed(project)['Geometry']
    here = project['Geometry']
    assert np.allclose(back.node_xyz, here.node_xyz)
    assert np.array_equal(back.node_id, here.node_id)
    assert np.allclose(back.cs_matrix, here.cs_matrix)
    assert list(back.cs_name) == list(here.cs_name)
    assert list(back.group_name) == list(here.group_name)
    assert len(back.elem_conn) == len(here.elem_conn)


def test_the_active_geometry_replays():
    state = opened(fixture_path('plate', 'geometry.npz'))
    state, _ = projects.import_file(state, {'path': fixture_path('plate', 'geometry.npz')})
    project = state.project
    other = next(n for n in project if n != project.active_geometry)
    projects.activate(state, {'geometry': other})
    assert replayed(project).active_geometry == other


def test_a_channel_cell_replays():
    state = opened(fixture_path('plate', 'random.nc4'))
    project = state.project
    run(project, 'project.set_channel_cell', table='Channel Table', row=2,
        column='comment', value='on the bracket')
    assert replayed(project)['Channel Table'].frame.loc[1, 'comment'] == 'on the bracket'


@pytest.mark.parametrize('system', sorted(SYSTEMS))
def test_a_foreign_save_as_keeps_the_script_running(tmp_path, system):
    state = opened(fixture_path('plate', 'geometry.npz'), units={'length_unit': 'mm'})
    project = state.project
    op = OPS['project.save_as']
    values = op.resolve({'object': 'Geometry', 'format': 'unv', 'path': str(tmp_path / 'g.unv')},
                        objects={name: projects.kind_of(obj) for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    # the server puts the connection's display system in place of 'display'
    op.run(project, {**values, 'units': system})
    assert f'hertzforge.units.SYSTEMS[{system!r}]' in project.journal[-1]
    replayed(project)
