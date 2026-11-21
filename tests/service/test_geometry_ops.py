"""Editing a geometry and what is drawn on it, and the project's structure:
entity tables and edits, the coordinate system gizmo, DOF arrows and
sensors, Auto Compute, channel roles and DOF names, a
channel table's control loops, and the object groups."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

import hertzforge
from hertzforge.core.report import Report
from hertzforge.service import projects
from hertzforge.service.ops_geometry import OPERATIONS, id_runs_text, parse_id_runs
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in OPERATIONS}


@pytest.fixture
def project():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    state, _ = projects.import_file(state, {'path': str(PLATE / 'geometry.unv'),
                                            'units': {'length_unit': 'mm'}})
    return state.project


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()})
    return op.run(project, values)


def cells(table):
    return [dict(zip(table['columns'], row, strict=True)) for row in table['rows']]


def test_the_descriptors_validate(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')


def test_id_runs_read_both_ways():
    assert id_runs_text([1, 2, 3, 7, 9, 10]) == '1-3 7 9-10'
    assert parse_id_runs('1-3, 7 10-9') == [1, 2, 3, 7, 9, 10]
    with pytest.raises(ProtocolError, match='not an id'):
        parse_id_runs('a-3')


def test_view_entities_lists_every_node_and_draws_them(project, validate):
    view, table = run(project, 'project.view_entities', kind='nodes')
    wire_views([view, table], validate)
    geometry = project['Geometry']
    assert len(table['rows']) == geometry.num_nodes == len(view['nodes'])
    second = cells(table)[1]
    assert second['Node'] == '102'
    assert float(second['X [m]']) == pytest.approx(float(geometry.node_xyz[1, 0]))
    view, table = run(project, 'project.view_entities', kind='groups')
    assert cells(table)[0]['Elements'] == '1-144'


def test_the_node_table_and_its_view_mark_nodes_together(project, validate):
    view, table = run(project, 'project.view_entities', kind='nodes')
    wire_views([view, table], validate)
    # one link on both, the table's rows by their ids and the view's nodes by `node_ids`
    assert view['marks'] == {'link': 'nodes:Geometry'}
    assert table['marks'] == {'link': 'nodes:Geometry', 'column': 0}
    ids = [int(n) for n in project['Geometry'].node_id]
    assert list(view['node_ids']) == ids == [int(row[0]) for row in table['rows']]
    # the Nodes part picked in the tree marks with them; the element table marks elements,
    # which its view draws by `mark_sets`, not by a click on a node; the others mark nothing
    (drawn,) = _picked(project, {'kind': 'part', 'id': 'nodes'}, validate)
    assert drawn['marks'] == view['marks'] and list(drawn['node_ids']) == ids
    for kind in ('coordinate_systems', 'elements', 'groups'):
        view, table = run(project, 'project.view_entities', kind=kind)
        assert 'marks' not in view
        assert table.get('marks') == ({'link': 'elements:Geometry', 'column': 0}
                                      if kind == 'elements' else None)
        assert list(view['node_ids']) == ids, 'numbered whatever is edited'


def test_systems_and_a_groups_elements_are_marked_one_at_a_time(project, validate):
    geometry = project['Geometry']
    (drawn,) = _picked(project, {'kind': 'part', 'id': 'coordinate_systems'}, validate)
    (systems,) = drawn['mark_sets']
    # a system's row rings the origin of its frame, by its place among the frames
    assert systems == {'link': 'systems:Geometry', 'frames': True,
                       'ids': [int(cs) for cs in geometry.cs_id]}
    assert len(drawn['frames']) == len(systems['ids'])
    group = int(geometry.group_id[0])
    (drawn,) = _picked(project, {'kind': 'group', 'id': group}, validate)
    (elements,) = drawn['mark_sets']
    mine = [k for k, g in enumerate(geometry.elem_group) if int(g) == group]
    assert elements['link'] == 'elements:Geometry'
    assert elements['ids'] == [int(geometry.elem_id[k]) for k in mine]
    # each element by the rows of its own nodes in the view
    rows = {int(node): row for row, node in enumerate(geometry.node_id)}
    assert elements['rows'][0] == [rows[int(n)] for n in geometry.elem_conn[mine[0]]]
    assert np.allclose(drawn['nodes'][elements['rows'][0]],
                       np.asarray(geometry.node_xyz)[elements['rows'][0]])


def test_a_node_is_added_at_typed_coordinates(project, validate):
    view, table = run(project, 'project.add_node', x=0.1, y=0.2, z=0.3)
    wire_views([view, table], validate)
    last = cells(table)[-1]
    assert last['Node'] == '1314' and float(last['Z [m]']) == pytest.approx(0.3)
    assert view['nodes'][-1] == pytest.approx([0.1, 0.2, 0.3])
    with pytest.raises(ProtocolError, match='already exists'):
        run(project, 'project.add_node', id=101)
    with pytest.raises(ProtocolError, match='no coordinate system 7'):
        run(project, 'project.add_node', placement_cs=7)


def test_a_coordinate_system_is_added_turned_and_drawn_with_its_axes(project, validate):
    view, table = run(project, 'project.add_coordinate_system', x=0.1, rz=90.0,
                      name='turned')
    wire_views([view, table], validate)
    added = cells(table)[-1]
    assert added['Name'] == 'turned' and float(added['Rotation Z [°]']) == pytest.approx(90.0)
    geometry = project['Geometry']
    assert geometry.cs_matrix[1, 0] == pytest.approx([0.0, 1.0, 0.0], abs=1e-12)
    # two systems, each drawn as the gimbal is: its origin and three axes
    assert len(view['frames']) == 2
    assert view['frames'][1]['origin'] == pytest.approx([0.1, 0.0, 0.0])
    assert view['frames'][1]['axes'][0] == pytest.approx([0.0, 1.0, 0.0], abs=1e-12)
    assert len(view['nodes']) == geometry.num_nodes
    with pytest.raises(ProtocolError, match='already exists'):
        run(project, 'project.add_coordinate_system', id=1)


def test_elements_are_added_by_family(project, validate):
    view, table = run(project, 'project.add_element', family='triangles',
                      nodes='101 102 201')
    wire_views([view, table], validate)
    added = cells(table)[-1]
    assert added['Type'] == 'tri3' and added['Nodes'] == '101 102 201'
    assert added['Element group'] == 'element group 2'
    _view, table = run(project, 'project.add_element', family='beams', nodes='101-104')
    assert [row['Type'] for row in cells(table)[-3:]] == ['beam2'] * 3
    with pytest.raises(ProtocolError, match='joins 8 nodes; 4 are named'):
        run(project, 'project.add_element', family='quads', type='quad8',
            nodes='101 102 202 201')
    with pytest.raises(ProtocolError, match='holds quads'):
        run(project, 'project.add_element', family='triangles', nodes='101 102 201',
            group=1)
    with pytest.raises(ProtocolError, match='unknown nodes'):
        run(project, 'project.add_element', family='quads', nodes='101 102 202 9999')


def test_a_click_places_one_only_while_one_is_being_added(project, validate):
    """A node or a coordinate system is placed by a click in the views of its Add, and not
    while its table is edited (Brandon, 2026-11-17)."""
    for kind in ('nodes', 'coordinate_systems'):
        view, _table = run(project, 'project.view_entities', kind=kind)
        wire_views([view], validate)
        assert 'pick' not in view, kind
    view, table = run(project, 'project.add_node', x=0.01, y=0.02, z=0.0)
    pick = view['pick']
    assert pick['op'] == 'project.add_node' and pick['target'] == 'point'
    assert pick['in_display'] is True
    view, table = run(project, pick['op'], **pick['params'], x=0.03, y=0.02, z=0.0)
    assert cells(table)[-1]['Node'] == '1315'
    assert view['pick']['op'] == 'project.add_node', 'the next click adds another'
    view, _table = run(project, 'project.add_coordinate_system', x=0.1)
    assert view['pick']['op'] == 'project.add_coordinate_system'
    # a cell typed over and a row deleted leave the table's view as it was: no click adds
    view, _table = run(project, 'project.set_entity_cell', kind='nodes', row=1, column='x',
                       value='0.5')
    assert 'pick' not in view
    _view, table = run(project, 'project.view_entities', kind='groups')
    assert 'pick' not in _view
    assert OPS['project.add_node'].previews and OPS['project.add_coordinate_system'].previews


def test_a_systems_row_puts_the_arrows_and_rings_on_it(project, validate):
    """Edit is the one way to move a coordinate system: its row typed into, or the arrows and
    rings on it dragged, which the row picked chooses (Brandon, 2026-11-17)."""
    run(project, 'project.add_coordinate_system', x=0.1, name='gauge')
    view, table = run(project, 'project.view_entities', kind='coordinate_systems')
    wire_views([view, table], validate)
    assert 'gizmo' not in view, 'two systems and no row picked: on neither'
    select = table['select']
    assert select == {'op': 'project.view_entities',
                      'params': {'geometry': 'Geometry', 'kind': 'coordinate_systems'},
                      'param': 'cs', 'column': 0}
    view, table = run(project, select['op'], **select['params'], cs=2)
    wire_views([view, table], validate)
    assert view['gizmo']['origin'] == pytest.approx([0.1, 0.0, 0.0])
    assert view['gizmo']['slide']['params'] == {'geometry': 'Geometry', 'cs': 2}
    # typed in its row: the system moves, and the arrows and rings with it
    edit = table['edit']
    view, table = run(project, edit['op'], **edit['params'], row=2, column='origin_y',
                      value='0.25')
    assert view['gizmo']['origin'] == pytest.approx([0.1, 0.25, 0.0])
    assert view['gizmo']['slide']['params']['cs'] == 2 and 'pick' not in view
    view, table = run(project, edit['op'], **edit['params'], row=2, column='rotation_z',
                      value='90')
    assert np.asarray(view['gizmo']['axes'])[0] == pytest.approx([0.0, 1.0, 0.0], abs=1e-9)
    # dragged: the same system, and the table says where it is now
    slide = view['gizmo']['slide']
    view, table = run(project, slide['op'], **slide['params'], axis='x', distance=0.05)
    assert float(cells(table)[1]['Origin Y [m]']) == pytest.approx(0.30)
    assert 'pick' not in view and table['select'] == select
    with pytest.raises(ProtocolError, match='no coordinate system 9'):
        run(project, select['op'], **select['params'], cs=9)


def test_the_only_system_has_the_arrows_and_rings_without_a_pick(project):
    view, _table = run(project, 'project.view_entities', kind='coordinate_systems')
    assert view['gizmo']['slide']['params']['cs'] == 1


def test_clicked_nodes_make_an_element_of_the_family_in_hand(project, validate):
    view, _table = run(project, 'project.view_entities', kind='elements')
    wire_views([view], validate)
    pick = view['pick']
    geometry = project['Geometry']
    assert pick['target'] == 'nodes' and pick['param'] == 'nodes'
    assert pick['ids'] == [int(n) for n in geometry.node_id], 'one id a drawn row'
    assert pick['count'] == 4 and pick['params']['family'] == 'quads'
    view, table = run(project, 'project.add_element', family='beams', nodes='101 102')
    pick = view['pick']
    assert pick['count'] == 0 and pick['params']['family'] == 'beams', 'a chain, then Enter'
    _view, table = run(project, pick['op'], **pick['params'], nodes='102, 103, 104')
    assert [row['Type'] for row in cells(table)[-2:]] == ['beam2'] * 2


def test_an_element_group_is_added_empty(project):
    _view, table = run(project, 'project.add_element_group', name='brackets')
    added = cells(table)[-1]
    assert (added['Element group'], added['Name'], added['Elements']) == ('2', 'brackets', '')
    assert (added['Elements are'], added['Count'], added['Material']) == ('empty', '0', '')
    with pytest.raises(ProtocolError, match='already exists'):
        run(project, 'project.add_element_group', id=1)


def test_a_field_is_set_and_an_inconsistent_edit_is_put_back(project, validate):
    view, table = run(project, 'project.set_entity', kind='nodes', entity=101,
                      field='z', value='0.5')
    wire_views([view, table], validate)
    assert float(cells(table)[0]['Z [m]']) == pytest.approx(0.5)
    _view, table = run(project, 'project.set_entity', kind='nodes', entity=102, field='id',
                       value='5000')
    assert '5000' in [row['Node'] for row in cells(table)]
    assert 5000 in project['Geometry'].elem_conn[0]
    before = int(project['Geometry'].elem_type[0])
    with pytest.raises(ProtocolError, match='element 1, type'):
        run(project, 'project.set_entity', kind='elements', entity=1, field='type',
            value='tri3')
    assert int(project['Geometry'].elem_type[0]) == before
    with pytest.raises(ProtocolError, match='has the fields'):
        run(project, 'project.set_entity', kind='groups', entity=1, field='x', value='1')
    with pytest.raises(ProtocolError, match='no node 7'):
        run(project, 'project.set_entity', kind='nodes', entity=7, field='x', value='1')


def test_deleting_keeps_measured_nodes_and_the_last_system(project, validate):
    view, table = run(project, 'project.delete_entities', kind='nodes', ids='102')
    wire_views([view, table], validate)
    assert table['title'].endswith('removed 1 node, 2 elements'), 'said once, in the title'
    assert len(table['rows']) == 168
    with pytest.raises(ProtocolError, match='measured at node 101'):
        run(project, 'project.delete_entities', kind='nodes', ids='101')
    with pytest.raises(ProtocolError, match='at least one coordinate system'):
        run(project, 'project.delete_entities', kind='coordinate_systems', ids='1')
    with pytest.raises(ProtocolError, match='no element 9999'):
        run(project, 'project.delete_entities', kind='elements', ids='9999')
    _view, table = run(project, 'project.delete_entities', kind='elements', ids='3-12')
    # the rest of the first row of quads: their own nodes go, the measured stay
    assert 'removed 10 elements' in table['title']
    assert {101, 104, 110, 113} <= set(project['Geometry'].node_id.tolist())


def test_a_coordinate_system_turns_slides_and_resets(project, validate):
    run(project, 'project.add_coordinate_system', x=0.1, name='gauge')
    view, table = run(project, 'project.rotate_coordinate_system', cs=2, axis='z',
                      angle=30.0)
    assert view['gizmo']['turn']['op'] == 'project.rotate_coordinate_system'
    assert view['gizmo']['slide']['values']['y'] == 'distance'
    assert view['gizmo']['slide']['adds']['y'] == {'axis': 'y'}
    assert view['gizmo']['slide']['params']['cs'] == 2
    wire_views([view, table], validate)
    assert float(cells(table)[1]['Rotation Z [°]']) == pytest.approx(30.0)
    _view, table = run(project, 'project.slide_coordinate_system', cs=2, axis='x',
                       distance=0.02)
    row = cells(table)[1]
    assert float(row['Origin X [m]']) == pytest.approx(0.1 + 0.02 * np.cos(np.radians(30)),
                                                       abs=1e-6)
    assert float(row['Origin Y [m]']) == pytest.approx(0.02 * np.sin(np.radians(30)),
                                                       abs=1e-6)
    view, table = run(project, 'project.reset_rotation', cs=2)
    assert view['gizmo']['slide']['params']['cs'] == 2 and 'pick' not in view
    row = cells(table)[1]
    assert float(row['Rotation Z [°]']) == 0.0
    assert float(row['Origin Y [m]']) == pytest.approx(0.01)
    with pytest.raises(ProtocolError, match='no coordinate system 3'):
        run(project, 'project.reset_rotation', cs=3)


def test_dof_arrows_mark_a_tables_references_pointing_in(project, validate):
    view, table = run(project, 'project.dof_arrows', source='Channel Table',
                      role='reference')
    wire_views([view, table], validate)
    assert [row['DOF'] for row in cells(table)] == ['101Z+', '110Z+', '1304Z+', '1310Z+']
    assert all(row['Arrow'].startswith('in') for row in cells(table))
    geometry = project['Geometry']
    # the model's own nodes and lines, and a solid arrow per DOF ending on its node
    assert len(view['nodes']) == geometry.num_nodes
    drawn = view['arrows']
    assert drawn['tails'].shape == (4, 3) and drawn['directions'].dtype == np.float32
    assert drawn['axes'].tolist() == [2, 2, 2, 2], 'each colored by its own axis'
    tip = drawn['tails'][0] + drawn['length'] * drawn['directions'][0]
    assert tip == pytest.approx(geometry.node_xyz[0], abs=1e-6)
    _view, table = run(project, 'project.dof_arrows', source='Time History',
                       measured='acceleration')
    assert len(table['rows']) == 8
    assert all(row['Arrow'].startswith('out') for row in cells(table))
    with pytest.raises(ProtocolError, match='measures acceleration, force'):
        run(project, 'project.dof_arrows', source='Time History', measured='velocity')
    _view, table = run(project, 'project.dof_arrows', source='Geometry')
    assert len(table['rows']) == 3 * project['Geometry'].num_nodes


def placed_frame(project):
    """The first sensor's frame, to read its body in its own axes."""
    return project.sensors('Channel Table')[0][0].frame


def test_sensors_are_drawn_at_a_multiple_of_true_size(project, validate):
    with pytest.raises(ProtocolError, match='names no sensor model'):
        run(project, 'project.sensors')
    project['Channel Table'].set_cell('model', 0, '352A24')
    view, table, reading = run(project, 'project.sensors', size=10)
    wire_views([view, table, reading], validate)
    assert cells(table)[0]['Model'] == '352A24' and cells(table)[0]['Kind'] == 'uniaxial'
    bodies = view['bodies']
    assert len(view['nodes']) == project['Geometry'].num_nodes
    assert bodies['triangles'].dtype == np.uint32 and bodies['colors'].dtype == np.uint8
    assert len(bodies['nodes']) == len(bodies['colors']) == len(bodies['follow'])
    geometry = project['Geometry']
    row = int(np.flatnonzero(np.asarray(geometry.node_id) == 101)[0])
    # every point moves with node 101, the one the sensor sits on
    assert set(bodies['follow'].tolist()) == {row}
    # a 9.9 x 7.1 x 4.8 mm teardrop at ten times its size, centered on node 101, with
    # its cable out of the tail
    reach = np.ptp(bodies['nodes'] @ placed_frame(project), axis=0)
    # the round end is a polygon, so its width is a little short of 7.1 mm
    assert reach[1:] == pytest.approx([0.071, 0.048], rel=1e-3)
    assert reach[0] > 0.099


def test_work_up_goes_by_the_module_it_is_run_in(validate):
    """Each kind of test's module has its own Auto Compute, and nothing has to be
    set first."""
    state, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    project = state.project
    project.project_type = None
    (reading,) = run(project, 'project.work_up_random')
    wire_views([reading], validate)
    added = [row['value'] for row in reading['rows'] if row['label'] == 'Added']
    assert 'Time History PSDs' in added and added[-1] == 'Report'
    assert set(added) <= set(project)
    assert project.project_type == 'Random Vibration'
    (reading,) = run(project, 'project.work_up_random')
    assert reading['rows'][0]['value'] == 'nothing to compute'


def test_work_up_says_what_the_report_still_lacks():
    """A report whose optional slots are empty and whose blocks are unbound
    does not have everything it needs, whatever is required."""
    state, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    (reading,) = run(state.project, 'project.work_up_sysid')
    said = [(row['label'], row['value']) for row in reading['rows']]
    assert ('Missing', 'nothing required') not in said, 'one stream leaves a required PSD'
    assert ('Optional', 'Geometry') in said and ('Optional', 'Photos') in said
    unbound = state.project['Report'].unbound(state.project,
                                               state.project.object_groups)
    assert unbound and ('Unbound', f'{len(unbound)} blocks of Report') in said


def test_work_up_refuses_an_empty_project():
    with pytest.raises(ProtocolError, match='Nothing is loaded'):
        run(hertzforge.Project(), 'project.work_up_modal')


def test_each_kind_of_test_makes_its_own_report():
    """A report from Random does not count as Transient's, so working up a
    mixed run in both modules leaves one of each, and once more adds nothing."""
    state, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    project = state.project
    run(project, 'project.work_up_random')
    run(project, 'project.work_up_transient')
    titles = sorted(obj.title for _name, obj in project.items() if isinstance(obj, Report))
    assert len(titles) == 2 and len(set(titles)) == 2
    (reading,) = run(project, 'project.work_up_transient')
    assert reading['rows'][0]['value'] == 'nothing to compute'


def test_a_channel_role_moves_on_the_history_and_its_table(project, validate):
    (table,) = run(project, 'project.set_channel_role', source='Time History', record=2,
                   role='monitor')
    wire_views([table], validate)
    assert cells(table)[1]['Role'] == 'monitor'
    assert table['title'].endswith('4 references, 7 responses, 1 monitor')
    assert project['Channel Table'].roles()[1] == 'monitor'
    (table,) = run(project, 'project.set_channel_role', source='Channel Table', record=2,
                   role='response')
    assert project['Time History'].channel_roles()[('104Z+', 'acceleration')] == 'response'
    with pytest.raises(ProtocolError, match='no channel 13'):
        run(project, 'project.set_channel_role', source='Time History', record=13)


def test_a_dof_is_renamed_for_one_quantity(project, validate):
    table, reading = run(project, 'project.rename_dof', source='Time History', old='101Z+',
                         measured='force', new='102Z+')
    wire_views([table, reading], validate)
    dofs = project['Time History'].response_dof
    assert list(dofs).count('101Z+') == 1 and '102Z+' in list(dofs)
    assert reading['rows'][0]['value'] == 'Time History'
    with pytest.raises(ProtocolError, match='Time History'):
        run(project, 'project.rename_dof', source='Time History', old='999Z+', new='998Z+')
    with pytest.raises(ProtocolError, match='name the DOF'):
        run(project, 'project.rename_dof', source='Time History', old='101Z+')


def test_a_tables_control_loop_and_its_transformation(project, validate):
    from hertzforge.core.channel_table import Environment

    (table,) = run(project, 'project.channel_table_view', table='Channel Table')
    wire_views([table], validate)
    assert [row['Control'] for row in cells(table)] == ['True'] * 8 + ['False'] * 4
    (table,) = run(project, 'project.set_control', table='Channel Table', record=1,
                   control=False)
    assert cells(table)[0]['Control'] == 'False'
    with pytest.raises(ProtocolError, match='holds no response transformation'):
        run(project, 'project.channel_table_view', table='Channel Table', form='response')
    channels = project['Channel Table']
    channels.environments['Random'] = Environment(
        controls=[1, 2], drives=[9], response_transformation=[[0.5, 0.5], [1.0, -1.0]])
    (table,) = run(project, 'project.channel_table_view', table='Channel Table',
                   environment='Random', form='response')
    assert table['columns'] == ['Row', '101Z+', '104Z+']
    assert table['rows'][1][1:] == ['1', '-1']
    with pytest.raises(ProtocolError, match='not \'Sine\''):
        run(project, 'project.channel_table_view', table='Channel Table',
            environment='Sine')


def test_object_groups_are_linked_named_placed_and_given_the_basis(project, validate):
    (table,) = run(project, 'project.link', first='Geometry', second='Time History',
                   name='Run 1')
    wire_views([table], validate)
    group = cells(table)[0]
    assert group['Name'] == 'Run 1' and group['Members'].endswith('Geometry')
    (table,) = run(project, 'project.set_basis', object='Time History')
    assert cells(table)[0]['Basis'] == 'yes' and cells(table)[0]['Role'] == 'Basis'
    (table,) = run(project, 'project.unlink', object='Specification')
    assert 'Specification' not in cells(table)[0]['Members']
    (table,) = run(project, 'project.place', object='Specification', side='other')
    assert cells(table)[1]['Members'] == 'Specification'
    (table,) = run(project, 'project.set_role', object='Time History', role='none')
    assert cells(table)[0]['Role'] == ''
    (table,) = run(project, 'project.name_object_group', object='Time History', name='')
    assert cells(table)[0]['Name'] == ''
    with pytest.raises(ProtocolError, match='two or more different objects'):
        run(project, 'project.link', first='Geometry', second='Geometry')
    run(project, 'project.unlink', object='Specification')
    with pytest.raises(ProtocolError, match='in no object group'):
        run(project, 'project.unlink', object='Specification')
    with pytest.raises(ProtocolError, match='not in an object group'):
        run(project, 'project.set_basis', object='Specification')


def test_an_entity_table_is_edited_cell_by_cell(validate):
    """The entity tables carry their edit; a cell typed over goes through
    Set Entity for the entity on that row, and a bad value changes nothing."""
    state, _ = projects.import_file(None, {'path': str(PLATE / 'geometry.unv'),
                                           'units': {'length_unit': 'mm'}})
    project = state.project
    ops = {op.id: op for op in OPERATIONS}
    _drawn, table = ops['project.view_entities'].run(project, {'geometry': 'Geometry',
                                                               'kind': 'nodes', 'family': '',
                                                               'groups': ''})
    validate(table, 'tableView')
    assert table['edit']['op'] == 'project.set_entity_cell'
    assert table['edit']['columns'][1] == 'x'
    _drawn, after = ops['project.set_entity_cell'].run(project, {
        'geometry': 'Geometry', 'kind': 'nodes', 'row': 1, 'column': 'x', 'value': '0.5'})
    assert float(after['rows'][0][1]) == pytest.approx(0.5)
    with pytest.raises(ProtocolError, match="'wide' is not a number"):
        ops['project.set_entity_cell'].run(project, {
            'geometry': 'Geometry', 'kind': 'nodes', 'row': 1, 'column': 'x', 'value': 'wide'})
    with pytest.raises(ProtocolError, match='there is no row 99999'):
        ops['project.set_entity_cell'].run(project, {
            'geometry': 'Geometry', 'kind': 'nodes', 'row': 99999, 'column': 'x', 'value': '1'})


def _brick():
    import hertzforge
    from hertzforge import mesh

    project = hertzforge.Project('p')
    project.add('G', mesh.block((0, 0, 0), (1, 0, 0), (0, 1, 0), (0, 0, 1), 1.0, 'b'))
    return project


def test_a_node_with_data_keeps_its_id_as_it_cannot_be_deleted():
    project = _brick()
    project.generate_rigid_body_modes('G')
    geometry = project['G']
    node = int(geometry.node_id[0])
    with pytest.raises(ProtocolError, match='data in the project is measured at node'):
        run(project, 'project.set_entity', kind='nodes', entity=node, field='id', value='999')
    assert node in geometry.node_id and 999 not in geometry.node_id


@pytest.mark.parametrize(('kind', 'entity', 'field', 'value'), [
    ('nodes', 1, 'x', 'nan'), ('nodes', 1, 'y', 'inf'),
    ('coordinate_systems', 1, 'origin_x', 'nan'), ('coordinate_systems', 1, 'rotation_z', 'inf'),
    ('elements', 1, 'nodes', '1 2 3'), ('elements', 1, 'nodes', '1 1 1 1 1 1 1 1'),
    ('elements', 1, 'type', 'quad4')])
def test_a_cell_that_would_break_the_model_is_refused_and_left(kind, entity, field, value):
    project = _brick()
    geometry = project['G']
    xyz = geometry.node_xyz.copy()
    matrix = geometry.cs_matrix.copy()
    conn = [c.copy() for c in geometry.elem_conn]
    types = geometry.elem_type.copy()
    with pytest.raises(ProtocolError) as error:
        run(project, 'project.set_entity', kind=kind, entity=entity, field=field, value=value)
    assert error.value.code == 'refused'
    assert (geometry.node_xyz == xyz).all() and (geometry.cs_matrix == matrix).all()
    assert all((a == b).all() for a, b in zip(geometry.elem_conn, conn, strict=True))
    assert (geometry.elem_type == types).all()


def test_a_cell_typed_on_a_row_of_a_repeated_id_changes_that_row():
    import hertzforge
    from hertzforge.core.geometry import Geometry

    geometry = Geometry(node_id=[1, 2, 3, 4], node_xyz=np.eye(4)[:, :3].astype(float),
                        elem_id=[7, 7], elem_type=[21, 21],
                        elem_conn=[np.array([1, 2]), np.array([3, 4])], elem_group=[1, 1],
                        group_id=[1], group_name=['b'])
    project = hertzforge.Project('p')
    project.add('Q', geometry)
    run(project, 'project.set_entity_cell', geometry='Q', kind='elements', row=2, column='nodes',
        value='1 4')
    assert [c.tolist() for c in geometry.elem_conn] == [[1, 2], [1, 4]]


def _picked(project, part, validate):
    from hertzforge.service import show

    views = show.show(project, ['Geometry'], {}, part)['views']
    wire_views(views, validate)
    return views


def _edited(project, part, validate):
    """The part's Edit, as its row in the action table runs it."""
    from hertzforge.service.action_table import action_table

    rows = action_table(project, ['Geometry'], {}, part)['columns'][0]['rows']
    row = next(row for row in rows if row['verb'] == 'Edit')
    assert row['id'] == 'project.view_entities' and row['ready']
    assert {'kind', 'family', 'groups'} <= set(row['omit']), 'the pick says them all'
    assert {'geometry', 'kind', 'family', 'groups'} <= set(row['fill']), 'so it opens at once'
    views = run(project, row['id'], **row['fill'])
    wire_views(views, validate)
    return views


def test_a_part_picked_is_drawn_and_its_edit_holds_its_table(project, validate):
    """Brandon, 2026-11-13: the table is editing's, not the plot's."""
    for part in ('nodes', 'coordinate_systems'):
        (drawn,) = _picked(project, {'kind': 'part', 'id': part}, validate)
        assert drawn['view'] == 'shape3d' and 'pick' not in drawn
        assert ('frames' in drawn) is (part == 'coordinate_systems')
    drawn, nodes = _edited(project, {'kind': 'part', 'id': 'nodes'}, validate)
    assert drawn['view'] == 'shape3d' and 'pick' not in drawn
    assert len(nodes['rows']) == len(project['Geometry'].node_id)
    # Delete takes a row by its id, the first column
    assert nodes['edit']['remove'] == {
        'op': 'project.delete_entities', 'params': {'geometry': 'Geometry', 'kind': 'nodes'},
        'column': 0}
    drawn, systems = _edited(project, {'kind': 'part', 'id': 'coordinate_systems'}, validate)
    assert systems['title'] == 'Coordinate systems of Geometry' and 'frames' in drawn


def test_a_family_or_group_shows_its_own_elements_only(project, validate):
    geometry = project['Geometry']
    run(project, 'project.add_element', family='beams', nodes='101 102')
    (plot,) = _picked(project, {'kind': 'family', 'id': 'beams'}, validate)
    assert plot['view'] == 'shape3d'
    drawn, beams = _edited(project, {'kind': 'family', 'id': 'beams'}, validate)
    assert len(beams['rows']) == 1
    assert drawn['pick']['params']['family'] == 'beams'
    # its edit says the family, so its row 1 is the beam, not the geometry's first element
    assert beams['edit']['params'] == {'geometry': 'Geometry', 'kind': 'elements', 'family': 'beams'}
    edited = run(project, 'project.set_entity_cell', **beams['edit']['params'], row=1,
                 column='nodes', value='102 103')
    beam = int(np.flatnonzero(np.asarray(geometry.elem_type) == 21)[0])
    assert [int(n) for n in geometry.elem_conn[beam]] == [102, 103]
    assert len(edited[-1]['rows']) == 1, 'the family\'s table again'
    group = int(geometry.group_id[0])
    (plot,) = _picked(project, {'kind': 'group', 'id': group}, validate)
    assert plot['view'] == 'shape3d'
    _drawn, groups, elements = _edited(project, {'kind': 'group', 'id': group}, validate)
    assert [row[0] for row in groups['rows']] == [str(group)]
    assert len(elements['rows']) == int(np.sum(np.asarray(geometry.elem_group) == group))


def test_delete_in_a_parts_table_takes_its_rows(project, validate):
    _drawn, nodes = _edited(project, {'kind': 'part', 'id': 'nodes'}, validate)
    remove = nodes['edit']['remove']
    before = len(project['Geometry'].node_id)
    run(project, 'project.add_node', x=1.0, y=1.0, z=1.0)
    assert len(project['Geometry'].node_id) == before + 1
    added = str(int(project['Geometry'].node_id[-1]))
    run(project, remove['op'], **remove['params'], ids=added)
    assert len(project['Geometry'].node_id) == before


def test_the_solids_row_or_a_solid_shows_the_solids(validate):
    from hertzforge.service import show
    from hertzforge.service.ops_solids import OPERATIONS as SOLIDS

    state = projects.new({'title': 'x'})
    project = state.project
    project.new_geometry('Geometry', unit='m')
    block = next(op for op in SOLIDS if op.id == 'project.add_solid_block')
    block.run(project, block.resolve({'geometry': 'Geometry'}, objects={'Geometry': 'geometry'}))
    for part in ({'kind': 'solids', 'id': 0}, {'kind': 'solid', 'id': 1}):
        views = show.show(project, ['Geometry'], {}, part)['views']
        wire_views(views, validate)
        (drawn,) = views
        # the block drawn before it is meshed: lit in the bodies when it is the one picked
        lit = part['kind'] == 'solid'
        assert len(drawn['bodies']['triangles'] if lit else drawn['triangles']) == 12
        # a solid picked carries the gimbal, whose drag moves it; the row as a whole does not
        assert ('gizmo' in drawn) is (part['kind'] == 'solid')
    assert drawn['gizmo']['slide']['op'] == 'project.move_solids'
    assert drawn['gizmo']['slide']['params'] == {'geometry': 'Geometry', 'solids': '1'}


def test_a_long_entity_table_sends_a_page_and_the_rest_on_asking(validate):
    """A million elements whole took 2 s to write and 80 MB to send: past a thousand
    rows a table sends its first and its count, and `project.entity_rows` the rest."""
    import asyncio

    from hertzforge.service.ops_geometry import PAGE_ROWS

    from .test_projects import _exchange

    out = asyncio.run(_exchange([
        ('op.run', {'op': 'project.new_geometry', 'params': {}}),
        ('op.run', {'op': 'project.add_solid_plate', 'params': {'geometry': 'Geometry'}}),
        ('op.run', {'op': 'project.mesh_solids', 'params': {
            'geometry': 'Geometry', 'elements': 'quads', 'size': 0.02}}),
        ('op.run', {'op': 'project.view_entities', 'params': {
            'geometry': 'Geometry', 'kind': 'elements', 'family': 'quads'}}),
        ('project.entity_rows', {'geometry': 'Geometry', 'kind': 'elements', 'family': 'quads',
                                 'start': PAGE_ROWS, 'count': 5}),
        ('project.entity_rows', {'geometry': 'Geometry', 'kind': 'elements', 'start': 0,
                                 'count': 99999}),
    ]))
    table = out[3][-1]['result']['views'][-1]
    validate(table, 'tableView')
    assert len(table['rows']) == PAGE_ROWS and table['total'] == 50 * 50
    assert table['more'] == {'method': 'project.entity_rows', 'params': {
        'geometry': 'Geometry', 'kind': 'elements', 'family': 'quads'}}
    page = out[4][-1]['result']['rows']
    assert [row[0] for row in page] == [str(PAGE_ROWS + k + 1) for k in range(5)]
    assert out[5][-1]['error']['code'] == 'bad_request'


def test_a_geometry_alone_draws_its_solids_as_its_parts_do():
    """The geometry picked and a part of it picked draw the same model, so the camera stays."""
    from hertzforge.service import show
    from hertzforge.service.ops_solids import OPERATIONS as SOLIDS

    state = projects.new({'title': 'x'})
    project = state.project
    project.new_geometry('Geometry', unit='m')
    block = next(op for op in SOLIDS if op.id == 'project.add_solid_block')
    block.run(project, block.resolve({'geometry': 'Geometry'}, objects={'Geometry': 'geometry'}))
    alone = show.show(project, ['Geometry'], {}, None)['views'][0]
    row = show.show(project, ['Geometry'], {}, {'kind': 'solids', 'id': 0})['views'][0]
    assert len(alone['triangles']) == len(row['triangles']) == 12


@pytest.mark.parametrize('family, nodes', [('quads', '101 102 202 201'), ('beams', '101 102'),
                                           ('triangles', '101 102 201')])
def test_a_familys_table_stays_the_familys_through_its_edits(project, validate, family, nodes):
    """Adding, typing over and deleting in a family's table shows that family's elements
    again, and a click in the view still adds one of that family."""
    from hertzforge.core.geometry import element_family

    geometry = project['Geometry']
    other = ('beams', '301 302') if family != 'beams' else ('triangles', '301 302 401')
    run(project, 'project.add_element', family=other[0], nodes=other[1])
    others = {element_family(int(c)) for c in geometry.elem_type} - {family}
    assert others, 'the geometry holds elements of other families'

    def only_family(views):
        drawn, table = views
        wire_views(views, validate)
        kept = [int(row[0]) for row in table['rows']]
        rows = [int(np.flatnonzero(np.asarray(geometry.elem_id) == e)[0]) for e in kept]
        assert {element_family(int(geometry.elem_type[r])) for r in rows} <= {family}
        assert drawn['pick']['params']['family'] == family
        return table

    added = only_family(run(project, 'project.add_element', family=family, nodes=nodes))
    count = len(added['rows'])
    assert count == sum(element_family(int(c)) == family for c in geometry.elem_type)
    edit = added['edit']
    assert edit['params'] == {'geometry': 'Geometry', 'kind': 'elements', 'family': family}
    only_family(run(project, 'project.set_entity_cell', **edit['params'], row=1,
                    column='nodes', value=added['rows'][0][3]))
    remove = edit['remove']
    assert remove['params'] == edit['params']
    left = only_family(run(project, remove['op'], **remove['params'], ids=added['rows'][-1][0]))
    assert len(left['rows']) == count - 1


def test_an_element_tables_rows_mark_their_elements_in_the_view(project, validate):
    """A row selected in an element table highlights its element in the view beside it:
    the table marks by element id, and the view holds each row's element, its nodes,
    its two triangles (a quad) and its four sides, by node row."""
    geometry = project['Geometry']
    run(project, 'project.add_element', family='beams', nodes='101 102')
    drawn, quads = _edited(project, {'kind': 'family', 'id': 'quads'}, validate)
    assert quads['marks'] == {'link': 'elements:Geometry', 'column': 0}
    (marks,) = drawn['mark_sets']
    assert marks['link'] == quads['marks']['link']
    assert marks['ids'] == [int(row[0]) for row in quads['rows']], 'a set entry per row, in order'
    first = int(np.flatnonzero(np.asarray(geometry.elem_id) == marks['ids'][0])[0])
    index = {int(n): r for r, n in enumerate(geometry.node_id)}
    corners = [index[int(n)] for n in geometry.elem_conn[first]]
    assert marks['rows'][0] == corners
    a, b, c, d = corners
    assert marks['faces'][0] == [a, b, c, a, c, d]
    assert sorted(map(sorted, np.reshape(marks['edges'][0], (-1, 2)).tolist())) == sorted(
        sorted(pair) for pair in ([a, b], [b, c], [c, d], [d, a]))
    beam_view, _beams = _edited(project, {'kind': 'family', 'id': 'beams'}, validate)
    (beam,) = beam_view['mark_sets']
    assert beam['faces'] == [[]] and beam['edges'] == [[index[101], index[102]]]
    assert 'note' not in drawn


def test_a_solid_element_is_highlighted_by_each_side_once():
    from hertzforge.core.geometry import Geometry
    from hertzforge.service.mesh import element_highlights

    cube = np.array([[x, y, z] for z in (0, 1) for y in (0, 1) for x in (0, 1)], dtype=float)
    geometry = Geometry(node_id=np.arange(1, 9), node_xyz=cube, elem_id=[1], elem_type=[115],
                        elem_conn=[np.array([1, 2, 4, 3, 5, 6, 8, 7])], elem_group=[1],
                        group_id=[1], group_name=['brick'])
    (faces,), (edges,) = element_highlights(geometry, [0])
    assert len(faces) == 6 * 2 * 3, 'six faces, two triangles each'
    assert len(edges) == 12 * 2, 'a hexahedron has twelve sides'


def test_a_long_element_table_says_which_rows_the_view_highlights(project, validate, monkeypatch):
    from hertzforge.service import ops_geometry

    monkeypatch.setattr(ops_geometry, 'MARKED_ELEMENTS', 10)
    drawn, quads = _edited(project, {'kind': 'family', 'id': 'quads'}, validate)
    assert len(drawn['mark_sets'][0]['ids']) == 10 < len(quads['rows'])
    assert drawn['note'] == 'Rows after the first 10 are not highlighted in the view.'


def test_a_family_offers_no_add_element_row_its_edit_adds_them(project, validate):
    from hertzforge.service.action_table import action_table

    rows = action_table(project, ['Geometry'], {}, {'kind': 'family', 'id': 'quads'})['columns'][0]['rows']
    assert 'project.add_element' not in {row['id'] for row in rows}
    drawn, _quads = _edited(project, {'kind': 'family', 'id': 'quads'}, validate)
    assert drawn['pick']['op'] == 'project.add_element'


def test_add_mode_offers_the_familys_types_and_its_groups(project, validate):
    geometry = project['Geometry']
    run(project, 'project.add_element_group', name='Struts')
    struts = int(geometry.group_id[-1])
    run(project, 'project.add_element', family='beams', nodes='101 102', group=struts)
    run(project, 'project.add_element_group', name='Empty')
    drawn, _quads = _edited(project, {'kind': 'family', 'id': 'quads'}, validate)
    pick = drawn['pick']
    types, groups = pick['settings']
    assert types['param'] == 'type' and pick['params']['type'] == 'quad4'
    words = [o['value'] for o in types['options']]
    assert {'quad4', 'quad8', 'quadshell4'} <= set(words) and 'tri3' not in words
    counts = {o['value']: o['count'] for o in types['options']}
    assert counts['quad4'] == 4 and counts['quad8'] == 8
    # the quads' group and the empty one, not the beams' group
    labels = [o['label'] for o in groups['options']]
    assert labels == ['First or new', 'element group 1', 'Empty']
    beams_view, _beams = _edited(project, {'kind': 'family', 'id': 'beams'}, validate)
    (beam_types, _groups) = beams_view['pick']['settings']
    assert {o['count'] for o in beam_types['options']} == {0}, 'a line runs through any count'
    # a type picked in Add mode comes back as the one the next click makes
    view, _table = run(project, 'project.add_element', family='quads', type='quad8',
                       nodes='101 102 103 203 303 302 301 201')
    assert view['pick']['params']['type'] == 'quad8' and view['pick']['count'] == 8


def test_an_element_is_colored_in_its_table_and_its_view(project, validate):
    geometry = project['Geometry']
    drawn, quads = _edited(project, {'kind': 'family', 'id': 'quads'}, validate)
    assert quads['columns'][-1] == 'Color' and quads['rows'][0][-1] == '1 blue'
    assert quads['edit']['kinds'] == {'type': 'choice', 'color': 'choice'}
    assert quads['edit']['choices']['color'][4] == '4 red'
    assert quads['edit']['swatches']['color']['4 red'] == '#d62629'
    assert 'tri3' not in quads['edit']['choices']['type'], 'the types of the families shown'
    colors = drawn['element_colors']
    assert len(colors['triangles']) == 2 * len(geometry.elem_id)
    assert {tuple(c) for c in colors['triangle_colors']} == {(31, 120, 181)}
    for typed in ('4', 'red', '4 red'):
        geometry.elem_color[0] = 1
        view, table = run(project, 'project.set_entity_cell', **quads['edit']['params'], row=1,
                          column='color', value=typed)
        assert int(geometry.elem_color[0]) == 4 and table['rows'][0][-1] == '4 red'
    assert (214, 38, 41) in {tuple(c) for c in view['element_colors']['triangle_colors']}
    with pytest.raises(ProtocolError, match='not a color'):
        run(project, 'project.set_entity_cell', **quads['edit']['params'], row=1,
            column='color', value='chartreuse')


def test_select_mode_clicks_and_deletes_the_tables_elements(project, validate):
    drawn, quads = _edited(project, {'kind': 'family', 'id': 'quads'}, validate)
    (marks,) = drawn['mark_sets']
    assert marks['clicks'] is True
    assert marks['remove'] == {'op': 'project.delete_entities', 'params': quads['edit']['remove']['params']}


def test_an_element_added_in_a_groups_edit_shows_the_groups_again(project, validate):
    geometry = project['Geometry']
    group = int(geometry.group_id[0])
    drawn, _groups, _elements = _edited(project, {'kind': 'group', 'id': group}, validate)
    pick = drawn['pick']
    assert pick['params']['family'] == 'quads' and pick['params']['group'] == group
    assert pick['params']['groups'] == str(group)
    views = run(project, pick['op'], **pick['params'], nodes='101 102 202 201')
    wire_views(views, validate)
    assert [v['title'].split(':')[0] for v in views[1:]] == [
        'Element groups of Geometry', 'Elements of Geometry']
    assert len(views[2]['rows']) == len(geometry.elem_id)


def test_the_geometrys_own_plot_draws_each_element_in_its_color(validate):
    """Picking the geometry itself plots it with its elements' colors, by the rows of the
    nodes it draws: a node no element uses is not drawn, so the rows after it move up."""
    from hertzforge.core.geometry import Geometry
    from hertzforge.service import show

    xyz = np.array([[9, 9, 9], [0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0], [2, 0, 0]], float)
    geometry = Geometry(node_id=[1, 2, 3, 4, 5, 6], node_xyz=xyz, elem_id=[10, 11],
                        elem_type=[44, 21], elem_conn=[np.array([2, 3, 4, 5]), np.array([3, 6])],
                        elem_color=[4, 3], elem_group=[1, 2], group_id=[1, 2],
                        group_name=['plate', 'strut'])
    project = hertzforge.Project('p')
    project.add('G', geometry)
    (view,) = show.show(project, ['G'], {}, None)['views']
    wire_views([view], validate)
    colors = view['element_colors']
    assert list(view['node_ids']) == [2, 3, 4, 5, 6], 'node 1 is used by nothing'
    assert colors['triangles'].tolist() == [[0, 1, 2], [0, 2, 3]]
    assert colors['triangle_colors'].tolist() == [[214, 38, 41]] * 2
    assert colors['lines'].tolist() == [[1, 4]] and colors['line_colors'].tolist() == [[43, 161, 43]]
def test_add_group_on_a_family_makes_an_empty_group_of_it_at_once(project, validate):
    """Add Group on a family adds a group straight away, numbered next and unnamed, and
    the tree lists it under that family while it holds nothing; it is renamed there."""
    from hertzforge.service.action_table import action_table
    from hertzforge.service.tree import children

    geometry = project['Geometry']
    rows = action_table(project, ['Geometry'], {}, {'kind': 'family', 'id': 'beams'})['columns'][0]['rows']
    add = next(row for row in rows if row['id'] == 'project.add_element_group')
    assert add['instant'] is True and add['fill']['family'] == 'beams'
    assert {'name', 'id', 'family'} <= set(add['omit'])
    views = run(project, add['id'], **add['fill'])
    wire_views(views, validate)
    group = int(geometry.group_id[-1])
    assert geometry.group_family == {group: 'beams'} and geometry.group_name[-1] == ''
    assert views[-1]['title'] == f'Element groups of Geometry: added element group {group} for the beams'

    def listed():
        return {f['id']: [(g['id'], g['count']) for g in f['groups']]
                for f in children(project, 'Geometry')['families']}

    assert listed()['beams'] == [(group, 0)]
    assert (group, 0) not in listed()['quads']
    run(project, 'project.add_element', family='beams', nodes='101 102', group=group)
    assert listed()['beams'] == [(group, 1)], 'with an element, by what it holds'


def test_a_groups_family_follows_it_and_is_saved(tmp_path):
    from hertzforge.core.geometry import Geometry
    from hertzforge.io import native

    geometry = Geometry(node_id=[1, 2], node_xyz=np.eye(2, 3), elem_id=[1], elem_type=[21],
                        elem_conn=[np.array([1, 2])], elem_group=[1], group_id=[1],
                        group_name=['b'])
    with pytest.raises(ValueError, match='not an element family'):
        geometry.add_group('x', family='squares')
    two = geometry.add_group('strut', family='beams')
    three = geometry.add_group('', family='quads')
    geometry.renumber_group(int(np.flatnonzero(geometry.group_id == two)[0]), 20)
    assert geometry.group_family == {20: 'beams', three: 'quads'}
    path = tmp_path / 'g.hforge'
    native.save(geometry, path)
    assert native.load(path).group_family == {20: 'beams', three: 'quads'}
    four = geometry.add_group('', family='beams')
    geometry.merge_groups([20, four])
    assert geometry.group_family == {20: 'beams', three: 'quads'}, 'the merged-away one goes'
    geometry.delete_groups([three, 20])
    assert geometry.group_family == {}
    native.save(geometry, path)
    assert native.load(path).group_family == {}, 'a file with none reads back with none'
