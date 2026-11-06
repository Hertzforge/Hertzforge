"""The action table beside the tree: its first column is the selection, each
other column the selection with one more object, and what a selection draws by
itself is declared so the shell can name it before anything is drawn."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

from hertzforge.core.data import Psd, Specification
from hertzforge.core.shapes import ShapeSet
from hertzforge.service import action_table, projects, show, verbs
from hertzforge.service.protocol import ProtocolError
from hertzforge.service.server import OPERATIONS

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'


@pytest.fixture(scope='module')
def plate():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    projects.import_file(state, {'path': str(PLATE / 'modal_spectra.nc4')})
    projects.import_file(state, {'path': str(PLATE / 'geometry.unv'),
                                 'units': {'length_unit': 'mm'}})
    return state


def named(state, kind):
    return next(name for name, obj in state.project.items() if projects.kind_of(obj) == kind)


def table(state, *objects, records=None, part=None, validate=None):
    out = projects.actions(state, {'objects': list(objects), 'records': records or {}, 'part': part})
    if validate:
        validate(out, 'actions')
    return out['table']['columns']


def column(columns, *kinds):
    return next(c for c in columns if c['kinds'] == list(kinds))


def row(col, verb):
    return next(r for r in col['rows'] if r['verb'] == verb)


def verbs_in(col):
    return [r['verb'] for r in col['rows']]


def test_every_operation_has_a_verb_or_is_hidden():
    ids = {op.id for op in OPERATIONS}
    assert ids == set(verbs.VERBS) | verbs.HIDDEN, 'a new operation needs a word in service/verbs.py'
    assert not set(verbs.VERBS) & verbs.HIDDEN
    assert set(verbs.MODULES) <= ids and set(verbs.PARTS) <= ids


def test_verbs_are_short():
    assert not [v for v in verbs.VERBS.values() if len(v.split()) > 2]


def test_every_core_verb_an_operation_takes_has_its_types():
    used = {p.verb for op in OPERATIONS for p in op.params if p.verb}
    assert used <= set(action_table.VERB_WHEN)


def test_modules_narrow_what_was_offered_everywhere():
    by_id = {op.id: op for op in OPERATIONS}
    averaging = verbs.modules_of(by_id['project.set_averaging'])
    assert 'modal' in averaging and 'random' in averaging
    assert 'shock' not in averaging and 'sine' not in averaging
    assert 'shock' in verbs.modules_of(by_id['project.save_as'])
    assert verbs.modules_of(by_id['project.cmif']) == ('modal',)


def test_a_geometry_alone(plate, validate):
    geometry = named(plate, 'geometry')
    first, *rest = table(plate, geometry, validate=validate)
    assert first['current'] and first['kinds'] == ['geometry']
    plot = first['rows'][0]
    assert (plot['verb'], plot['kind'], plot['ready']) == ('Plot', 'default', True)
    assert geometry in row(first, 'Export')['fill'].values()
    assert row(first, 'Unlink')['ready'] is False and row(first, 'Unlink')['reason']
    assert 'View Geometry' not in verbs_in(first), 'what it draws by itself is the default row'
    animate = row(column(rest, 'geometry', 'time_history'), 'Animate')
    assert animate['kind'] == 'default' and animate['ready']
    assert set(animate['candidates']) == {n for n, o in plate.project.items()
                                          if projects.kind_of(o) == 'time_history'}
    missing = row(column(rest, 'geometry', 'specification'), 'Animate')
    assert not missing['ready'] and missing['candidates'] == [] and 'specification' in missing['reason']
    assert 'Link' in verbs_in(column(rest, 'geometry', '*'))


def test_each_object_goes_on_the_param_that_takes_it_whichever_was_picked_first(plate):
    project = plate.project
    project.add('Measured', Psd(**_psd()))
    project.add('Spec', Specification(**_psd()))
    try:
        first = table(plate, 'Spec', 'Measured')[0]
        assert row(first, 'RMS Error')['fill'] == {'spectrum': 'Measured', 'specification': 'Spec'}
        assert first['rows'][0]['verb'] == 'Compare' and first['rows'][0]['kind'] == 'default'
    finally:
        project.remove('Measured')
        project.remove('Spec')


def test_an_act_changes_the_project_and_a_reading_does_not(plate):
    first = table(plate, named(plate, 'time_history'))[0]
    assert row(first, 'Compute FRFs')['kind'] == 'act' and row(first, 'Integrate')['kind'] == 'act'
    assert row(first, 'Kurtosis')['kind'] == 'reading'


def test_the_stage_rule_makes_no_false_combinations(plate):
    """A geometry stands by beside data as the stage, so Averaging fills from a
    time history alone; it is not something the two do together."""
    col = column(table(plate, named(plate, 'geometry')), 'geometry', 'time_history')
    assert verbs_in(col) == ['Animate']


def test_two_geometries_default_to_the_overlay(plate):
    project = plate.project
    project.add('Second', project[named(plate, 'geometry')])
    try:
        columns = table(plate, named(plate, 'geometry'), 'Second')
        first = columns[0]
        assert first['rows'][0]['verb'] == 'Overlay' and first['rows'][0]['kind'] == 'default'
        assert 'Merge' in verbs_in(first) and 'Link' in verbs_in(first)
        assert not row(first, 'Rename')['ready']
        assert not [c for c in columns if c['kinds'][-1] == '*'], 'Link is already in the first column'
    finally:
        project.remove('Second')


def test_a_merge_only_pairs_objects_of_one_type(plate):
    columns = table(plate, named(plate, 'geometry'))
    assert 'Merge' in verbs_in(column(columns, 'geometry', 'geometry'))
    assert 'Merge' not in verbs_in(column(columns, 'geometry', 'time_history'))


def test_a_verb_that_cannot_run_says_why(plate):
    first = table(plate, named(plate, 'geometry'))[0]
    solve = row(first, 'Solve')
    assert not solve['ready'] and 'properties' in solve['reason'] and solve['modules'] == ['model']


def test_an_frf_starts_a_fit_in_modal_only(plate):
    first = table(plate, named(plate, 'frf'))[0]
    assert row(first, 'Fit by Hand')['modules'] == ['modal']
    edit = row(column(table(plate, named(plate, 'frf')), 'frf', 'shape_set'), 'Edit Fit')
    assert edit['modules'] == ['modal']


def test_picks_fill_the_records(plate):
    frf = named(plate, 'frf')
    first = table(plate, frf, records={frf: [0, 1, 2, 6]})[0]
    assert frf in row(first, 'CMIF')['fill'].values()
    assert row(first, 'Export')['fill'].get('records', '1-3, 7') == '1-3, 7'


def test_a_part_of_a_geometry_brings_its_own_verbs(plate):
    geometry = named(plate, 'geometry')
    assert 'Add Node' not in verbs_in(table(plate, geometry)[0])
    with_part = table(plate, geometry, part={'name': geometry, 'kind': 'part', 'id': 'nodes'})[0]
    assert row(with_part, 'Add Node')['part'] is True
    assert 'Rotate' not in verbs_in(with_part)
    systems = table(plate, geometry, part={'name': geometry, 'kind': 'part', 'id': 'coordinate_systems'})[0]
    assert 'Rotate' in verbs_in(systems)


def test_nothing_selected_is_the_project_alone(plate, validate):
    columns = table(plate, validate=validate)
    assert len(columns) == 1 and columns[0]['kinds'] == []
    assert {'New Geometry', 'New Table'} <= set(verbs_in(columns[0]))


def _shapes():
    return ShapeSet(frequency=[10.0, 20.0], damping=[0.01, 0.01], shape_matrix=np.eye(2),
                    coordinate=['1Z+', '2Z+'])


def _psd():
    f = np.linspace(20, 2000, 50)
    return {'abscissa': f, 'ordinate': np.full((2, 50), 0.01), 'response_dof': ['1Z+', '2Z+'],
            'ordinate_unit': ['g**2/Hz'] * 2}


def test_the_declared_pairs_are_what_show_draws(plate):
    """Every pair the table names as drawn by itself has a joint reading, and a
    geometry beside a type it does not name has none."""
    project = plate.project
    project.add('Shapes', _shapes())
    project.add('Shapes 2', _shapes())
    project.add('Measured', Psd(**_psd()))
    project.add('Measured 2', Psd(**_psd()))
    project.add('Spec', Specification(**_psd()))
    project.add('Stage 2', project[named(plate, 'geometry')])
    names = {'geometry': named(plate, 'geometry'), 'time_history': named(plate, 'time_history'),
             'frf': named(plate, 'frf'), 'channel_table': named(plate, 'channel_table'),
             'shape_set': 'Shapes', 'psd': 'Measured', 'specification': 'Spec'}
    second = {'geometry': 'Stage 2', 'shape_set': 'Shapes 2', 'psd': 'Measured 2'}
    try:
        for a, b in action_table.JOINT:
            if a not in names or b not in names:
                continue
            first, other = names[a], second[b] if a == b else names[b]
            try:
                assert show._pair(project, first, other, {}, None) is not None, (a, b)
            except ProtocolError:
                pass  # the pair's own reading, refusing these two objects' DOFs
        assert show._pair(project, names['geometry'], named(plate, 'multiple_coherence'), {}, None) is None
        assert ('geometry', 'multiple_coherence') not in action_table.JOINT
    finally:
        for name in ('Shapes', 'Shapes 2', 'Measured', 'Measured 2', 'Spec', 'Stage 2'):
            project.remove(name)


def test_a_lone_shape_set_animates_only_on_a_geometry(plate):
    project = plate.project
    project.add('Shapes', _shapes())
    try:
        kinds = ['shape_set']
        assert action_table.defaults(project, kinds, []) == ['Table', 'MAC']
    finally:
        project.remove('Shapes')
    assert action_table.defaults(project, ['geometry'], []) == ['Plot']
    assert action_table.defaults(project, ['report'], []) == []


def test_export_lists_the_formats_of_the_object_it_was_reached_from(plate, validate):
    from hertzforge.io import exporters

    name = named(plate, 'geometry')
    first = table(plate, name, validate=validate)[0]
    row = next(r for r in first['rows'] if r['id'] == 'project.save_as')
    assert row['fill'] == {'object': name}
    values = [f['value'] for f in row['formats']]
    assert values == ['hforge', *(e.name for e in exporters(plate.project[name]))]
    assert 'unv' in values and 'delimited' not in values and 'sdynpy_shapes' not in values
    assert row['formats'][0]['suffix'] == '.hforge'


def test_adding_a_geometry_to_shape_sets_is_not_offered(plate):
    project = plate.project
    project.add('Shapes', _shapes())
    project.add('Shapes B', _shapes())
    try:
        for selected in (['Shapes'], ['Shapes', 'Shapes B']):
            with_geometry = [c for c in table(plate, *selected)[1:] if 'geometry' in c['kinds']]
            assert not [r for c in with_geometry for r in c['rows'] if r['kind'] == 'default']
        with_shapes = [c for c in table(plate, named(plate, 'geometry'))[1:]
                       if c['kinds'] == ['geometry', 'shape_set']]
        assert not [r for c in with_shapes for r in c['rows'] if r['kind'] == 'default']
        # selected by hand they still draw together
        assert action_table.defaults(project, ['geometry', 'shape_set'], []) == ['Animate', 'Table']
    finally:
        project.remove('Shapes')
        project.remove('Shapes B')


def test_a_mate_picked_in_the_tree_fills_the_rows_that_act_on_it(plate, validate):
    from hertzforge import mesh

    project = plate.project
    project.add('Part', mesh.assemble(
        mesh.block((0, 0, 0), (1, 0, 0), (0, 1, 0), (0, 0, 1), 1.0, 'base', unit='m'),
        mesh.block((0, 0, 1), (1, 0, 0), (0, 1, 0), (0, 0, 1), 1.0, 'lid', unit='m'),
        apart=True))
    found = project.mate_surfaces('Part')
    picked = {'name': 'Part', 'kind': 'mate', 'id': found[0]}
    (current, *_rest) = table(plate, 'Part', part=picked, validate=validate)
    assert row(current, 'Edit Sides')['fill']['mate'] == str(found[0])
    assert row(current, 'Delete Mates')['fill']['mates'] == str(found[0])
