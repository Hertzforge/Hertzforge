"""The Project: the GUI's own container, scriptable.

A Project holds what the project tree shows — named objects, link
groups, the Basis, the type, the active geometry — and gives scripts
the same verbs the GUI's buttons call, so neither side has to
hand-assemble dicts of members and roles.
"""

from __future__ import annotations

import pathlib

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge import io
from hertzforge.core.matches import MatchedModes
from hertzforge.core.report import Report
from hertzforge.core.shapes import ShapeSet


def _geometry(node=101):
    return hertzforge.Geometry(node_id=[node], node_xyz=[[0.0, 0.0, 0.0]])


def _shapes(node=101):
    return ShapeSet([10.0], [0.01], [f'{node}X+'], np.ones((1, 1)))


def test_an_empty_project_is_a_named_empty_structure():
    project = hertzforge.Project('Beam Airplane Modal Test')
    assert project.name == 'Beam Airplane Modal Test'
    assert list(project) == [] and len(project) == 0
    assert project.object_groups == [] and not project.basis
    assert project.project_type is None
    assert project.active_geometry is None
    assert isinstance(project, dict), 'a project *is* its objects'


def test_adding_numbers_a_clash_and_adopts_the_first_geometry():
    project = hertzforge.Project()
    assert project.add('Geometry', _geometry()) == 'Geometry'
    assert project.active_geometry == 'Geometry', 'the first one in'
    assert project.add('Geometry', _geometry(102)) == 'Geometry (2)', (
        'a clash is numbered, never overwritten'
    )
    assert project.active_geometry == 'Geometry', 'and it stays'
    project['FRF'] = _shapes()      # the plain mapping still works
    assert set(project) == {'Geometry', 'Geometry (2)', 'FRF'}


def test_linking_merges_groups_and_refuses_a_second_geometry():
    project = hertzforge.Project()
    project.add('Geometry', _geometry())
    project.add('Shapes', _shapes())
    project.add('More Shapes', _shapes())
    project.link('Geometry', 'Shapes')
    project.link('Shapes', 'More Shapes')        # merges into one group
    assert set(project.object_group_of('Geometry')) == {
        'Geometry', 'Shapes', 'More Shapes'}
    project.add('Other Geometry', _geometry(900))
    with pytest.raises(ValueError, match='one geometry'):
        project.link('Geometry', 'Other Geometry')
    with pytest.raises(KeyError, match="no object named 'Nothing'"):
        project.link('Geometry', 'Nothing')


def test_linking_reports_dofs_the_geometry_lacks_and_does_not_refuse():
    """Nodes the geometry lacks are reported, never refused.

    This did refuse, on the reading that a link asserts one structure
    (2026-09-19). A test names places the model does not draw as a
    matter of course, two real reports died on it, and no line could
    be drawn from here: a controller's control object and a foreign
    mode shape both name nothing the geometry has (Brandon,
    2026-09-21). The red indicator is the answer, and the person
    reading it can tell those two apart."""
    from hertzforge.compatibility import check_compatibility

    project = hertzforge.Project()
    project.add('Geometry', _geometry(101))
    project.add('Stranger Shapes', _shapes(900))
    assert project.link('Geometry', 'Stranger Shapes') == [
        'Geometry', 'Stranger Shapes']
    report = check_compatibility(dict(project), 'Geometry', project.object_groups)
    assert not report.is_compatible('Stranger Shapes')
    assert report.issue_for('Stranger Shapes').missing_dofs == ['900X+']


def test_a_few_points_the_model_lacks_are_not_another_structure():
    """A run carries virtual channels — drive points, control
    coordinates, computed places — that were never nodes, and a report
    died linking a geometry beside three of them (Brandon,
    2026-09-21). Sharing *no* node is what the refusal is for; a
    partial overlap links and keeps its indicator."""
    from hertzforge.compatibility import check_compatibility

    project = hertzforge.Project()
    project.add('Geometry', hertzforge.Geometry(
        node_id=[101, 102, 103], node_xyz=np.zeros((3, 3))))
    project.add('Shapes', ShapeSet([10.0], [0.01],
                                   ['101X+', '102X+', '1X+'],
                                   np.ones((1, 3))))
    assert project.link('Geometry', 'Shapes') == ['Geometry', 'Shapes']

    report = check_compatibility(dict(project), 'Geometry', project.object_groups)
    assert not report.is_compatible('Shapes'), 'still flagged, not refused'
    assert report.issue_for('Shapes').missing_dofs == ['1X+']


def test_a_reference_nowhere_on_the_model_flags_every_record_and_still_links():
    """Counted over DOFs, never over records: a set shaken at one
    virtual drive flags all of its records while every response sits
    on the model exactly."""
    from hertzforge.core.data import Frf

    project = hertzforge.Project()
    project.add('Geometry', hertzforge.Geometry(
        node_id=[101, 102, 103], node_xyz=np.zeros((3, 3))))
    project.add('FRF', Frf(np.array([1.0, 2.0]), np.ones((3, 2), complex),
                           response_dof=['101X+', '102X+', '103X+'],
                           reference_dof=['1X+', '1X+', '1X+']))
    assert project.link('Geometry', 'FRF') == ['Geometry', 'FRF']


def test_an_object_that_is_all_virtual_points_links_too():
    """The case an overlap rule could not reach: a controller writes a
    control object whose every channel is a virtual coordinate, so it
    shares no node with the model at all and still belongs in the
    group (Brandon, 2026-09-21, the second report to die on it)."""
    from hertzforge.compatibility import check_compatibility
    from hertzforge.core.data import Psd

    project = hertzforge.Project()
    project.add('Geometry', hertzforge.Geometry(
        node_id=[101, 102, 103], node_xyz=np.zeros((3, 3))))
    project.add('Control', Psd(np.array([1.0, 2.0]), np.ones((3, 2)),
                               response_dof=['1', '2', '3']))
    assert project.link('Geometry', 'Control') == ['Geometry', 'Control']
    report = check_compatibility(dict(project), 'Geometry', project.object_groups)
    assert report.issue_for('Control').missing_dofs == ['1', '2', '3']


def test_a_modal_record_with_no_shape_set_still_refuses():
    """The other reason a link is refused is untouched: modal
    coordinates answer to a shape set, and without one there is
    nothing to draw them against at all."""
    from hertzforge.core.data import Frf

    project = hertzforge.Project()
    project.add('Geometry', _geometry())
    project.add('Modal', Frf(np.array([1.0, 2.0]), np.ones((2, 2), complex),
                             response_dof=['M1', 'M2'],
                             reference_dof=['M1', 'M2']))
    with pytest.raises(ValueError, match='cannot link'):
        project.link('Geometry', 'Modal')


def test_the_basis_is_declared_once_and_moves():
    project = hertzforge.Project()
    project.add('Geometry', _geometry())
    project.add('Shapes', _shapes())
    project.add('FEM Geometry', _geometry(900))
    project.add('FEM Shapes', _shapes(900))
    assert project.set_basis('Geometry', 'Shapes') == ['Geometry', 'Shapes']
    assert project.role_of('Shapes') == 'Basis'
    project.link('FEM Geometry', 'FEM Shapes')
    assert project.role_of('FEM Shapes') is None
    project.set_basis('FEM Shapes')
    assert project.basis.names == ['FEM Geometry', 'FEM Shapes']
    assert project.role_of('Shapes') is None, 'the role is unique'


def test_the_geometry_an_object_answers_to():
    project = hertzforge.Project()
    project.add('Geometry', _geometry())
    project.add('FEM Geometry', _geometry(900))
    project.add('FEM Shapes', _shapes(900))
    project.link('FEM Geometry', 'FEM Shapes')
    assert project.geometry_for('FEM Shapes')[0] == 'FEM Geometry', (
        'its object group, not the active geometry')
    project.add('Stranger', _shapes())
    assert project.geometry_for('Stranger')[0] == 'Geometry', (
        'unlinked: the active one')


def test_renaming_carries_every_reference_with_it():
    project = hertzforge.Project()
    project.add('Geometry', _geometry())
    project.add('Shapes', _shapes())
    project.link('Geometry', 'Shapes')
    project.add('Matched', MatchedModes('Shapes', 'Other', [[0, 0]], [1.0]))
    project.add('Report', Report('r', [{'kind': 'plot', 'source': 'Shapes',
                                        'mode': 'mac', 'caption': ''}]))
    project.rename('Shapes', 'Experimental Modes')
    assert 'Shapes' not in project and 'Experimental Modes' in project
    assert project.object_group_of('Geometry') == ['Geometry',
                                            'Experimental Modes']
    assert project['Matched'].first == 'Experimental Modes'
    assert project['Report'].blocks[0]['source'] == 'Experimental Modes'
    assert list(project).index('Experimental Modes') == 1, 'order held'
    with pytest.raises(ValueError, match='already in use'):
        project.rename('Geometry', 'Experimental Modes')


def test_removing_prunes_the_links():
    project = hertzforge.Project()
    project.add('Geometry', _geometry())
    project.add('Shapes', _shapes())
    project.add('More Shapes', _shapes())
    project.link('Geometry', 'Shapes', 'More Shapes')
    project.remove('Shapes')
    assert project.object_group_of('Geometry') == ['Geometry', 'More Shapes']
    project.remove('More Shapes')
    assert project.object_groups == [], 'a group of one dissolves'
    project.remove('Geometry')
    assert project.active_geometry is None


def test_a_project_round_trips_through_the_vibe_file(tmp_path):
    project = hertzforge.Project('Scripted')
    project.project_type = 'Modal Test'
    project.add('Geometry', _geometry())
    project.add('Shapes', _shapes())
    project.set_basis('Geometry', 'Shapes')
    project.save(tmp_path / 'p.vdyn')

    back = hertzforge.Project.open(tmp_path / 'p.vdyn')
    assert isinstance(back, hertzforge.Project)
    assert back.name == 'Scripted'
    assert back.project_type == 'Modal Test'
    assert back.basis.names == ['Geometry', 'Shapes']
    assert back.active_geometry == 'Geometry'
    assert list(back) == ['Geometry', 'Shapes']
    # import_file and io.load give the same thing — a project is a
    # project however it is opened
    assert isinstance(hertzforge.import_file(tmp_path / 'p.vdyn'), hertzforge.Project)
    assert isinstance(io.load(str(tmp_path / 'p.vdyn')), hertzforge.Project)


def test_a_project_prints_as_its_tree():
    """Typing a project's name at a prompt answers what the tree
    answers: what is in here, how it groups, how big each piece is."""
    project = hertzforge.Project('Beam Airplane Modal Test')
    project.project_type = 'Modal Test'
    project.add('Shapes', _shapes())            # out of order on purpose
    project.add('Geometry', _geometry())
    project.add('FEM Geometry', _geometry(900))
    project.add('FEM Shapes', _shapes(900))
    project.set_basis('Geometry', 'Shapes')
    project.link('FEM Geometry', 'FEM Shapes')

    text = repr(project)
    lines = [line.rstrip() for line in text.splitlines()]
    assert lines[0] == 'Project: Beam Airplane Modal Test  [Modal Test]'
    assert lines[1].strip() == 'Basis', 'the Basis group reads first'
    assert 'Geometry' in lines[2] and 'ShapeSet' not in lines[2], (
        'the canonical type order, whatever order they arrived in')
    assert '1 node' in lines[2], 'and what it holds'
    assert '*' in lines[2], 'the active geometry is marked'
    assert lines[4].strip() == 'linked', 'then the unroled groups'
    assert project.ordered_names() == ['Geometry', 'Shapes',
                                       'FEM Geometry', 'FEM Shapes']
    assert repr(hertzforge.Project('Empty')).endswith('(empty)')


def test_importing_data_files_names_them_as_the_tree_would():
    project = hertzforge.Project()
    added = project.import_file(fixture_path('plate',
                                             'geometry.npz'))
    assert added == ['Geometry']
    # a reader's keys are for code; the names are what the objects are
    added = project.import_file(fixture_path('plate',
                                             'modal_spectra.nc4'))
    assert 'FRF' in added and 'Time History' in added
    assert 'Multiple Coherence' in added and 'Channel Table' in added


def test_importing_a_project_brings_its_structure(tmp_path):
    """A saved project imported into an empty one arrives whole: its
    links, Basis, type and name."""
    source = hertzforge.Project('Saved Test')
    source.project_type = 'Modal Test'
    source.add('Geometry', _geometry())
    source.add('Shapes', _shapes())
    source.set_basis('Geometry', 'Shapes')
    source.save(tmp_path / 'saved.vdyn')

    project = hertzforge.Project()
    added = project.import_file(tmp_path / 'saved.vdyn')
    assert added == ['Geometry', 'Shapes']
    assert project.basis.names == ['Geometry', 'Shapes']
    assert project.name == 'Saved Test'
    assert project.project_type == 'Modal Test'
    assert project.active_geometry == 'Geometry'


def test_importing_a_project_into_a_full_one_keeps_both(tmp_path):
    """The clash is numbered, the imported group follows its objects
    to their new names, and the Basis already spoken for stays put."""
    source = hertzforge.Project('Second Run')
    source.add('Geometry', _geometry())
    source.add('Shapes', _shapes())
    source.add('Matched', MatchedModes('Shapes', 'Shapes', [[0, 0]], [1.0]))
    source.set_basis('Geometry', 'Shapes')
    source.save(tmp_path / 'second.vdyn')

    project = hertzforge.Project('First Run')
    project.add('Geometry', _geometry())
    project.add('Shapes', _shapes())
    project.set_basis('Geometry', 'Shapes')

    added = project.import_file(tmp_path / 'second.vdyn')
    assert added == ['Geometry (2)', 'Shapes (2)', 'Matched']
    assert project.object_group_of('Geometry (2)') == ['Geometry (2)',
                                                'Shapes (2)'], (
        'the imported links came with it, renamed')
    assert project.role_of('Geometry (2)') is None, (
        'the Basis is the project\'s, not the file\'s')
    assert project.basis.names == ['Geometry', 'Shapes']
    assert project['Matched'].first == 'Shapes (2)', (
        'references inside the imported objects followed too')
    assert project.name == 'First Run', 'identity is not overwritten'


def test_a_single_object_file_is_not_a_project(tmp_path):
    io.save(_geometry(), str(tmp_path / 'g.vdyn'))
    with pytest.raises(ValueError, match='not a project'):
        hertzforge.Project.open(tmp_path / 'g.vdyn')


def test_the_workflow_verbs_derive_and_link(survey, tmp_path):
    """Each verb adds its result and links it to what it came from,
    the way the GUI's buttons do."""
    _shape_set, frfs = survey
    project = hertzforge.Project('Scripted')
    project.add('Geometry', hertzforge.import_file(
        fixture_path('plate', 'geometry.npz')))
    project.add('FRF', frfs)
    project.set_basis('Geometry', 'FRF')

    modes = project.fit_modes('FRF', bounds=(5.0, 60.0), limit=3)
    assert modes == 'FRF Modes'
    assert project[modes].num_shapes >= 1
    assert modes in project.object_group_of('FRF'), 'linked to what it came from'

    # a report, bound symbolically, exported as one file
    report = project.generate_report('modal')
    block = next(b for b in project[report].blocks
                 if b.get('mode') == 'cmif')
    assert block['source'] == '@basis:Frf', 'no name in the binding'
    path = project.export_report(report, tmp_path / 'r.html')
    assert pathlib.Path(path).read_text(
        encoding='utf-8').lstrip().startswith('<')


def test_matching_uses_the_displayed_comparison(survey):
    """match_modes projects across geometries exactly as the
    comparison screen does, and keeps the MAC values it showed."""
    fem_shapes, _frfs = survey
    test_geometry = hertzforge.import_file(
        fixture_path('plate', 'test_geometry.npz'))
    fem_geometry = hertzforge.import_file(
        fixture_path('plate', 'geometry.npz'))
    # a stand-in measured set: the model's own motion at the measured
    # nodes, which is what a good test would have found
    from hertzforge.core.correlate import project_shapes

    dofs = [f'{int(node)}Z+' for node in test_geometry.node_id]
    stub = ShapeSet(fem_shapes.frequency[:6], fem_shapes.damping[:6],
                    dofs, np.ones((6, len(dofs))))
    measured, _report = project_shapes(fem_shapes, fem_geometry, stub,
                                       test_geometry)

    project = hertzforge.Project()
    project.add('Test Geometry', test_geometry)
    project.add('Test Modes', measured)
    project.add('FEM Geometry', fem_geometry)
    project.add('FEM Modes', fem_shapes)
    project.link('Test Geometry', 'Test Modes')
    project.link('FEM Geometry', 'FEM Modes')
    project.set_basis('Test Modes')

    matrix = project.comparison_mac('Test Modes', 'FEM Modes')
    assert matrix.shape[0] == measured.num_shapes, (
        'the basis set makes the rows')
    name = project.match_modes('Test Modes', 'FEM Modes', threshold=0.9)
    matched = project[name]
    assert matched.num_matches, 'a set projected from the model matches it'
    assert all(mac >= 0.9 for mac in matched.macs)
    assert matched.first_geometry == 'Test Geometry'
    assert matched.second_geometry == 'FEM Geometry'
    assert project.object_group_of(name) is None, (
        'a comparison names a set on each side, so it joins neither')
    assert matched.first_geometry == 'Test Geometry', (
        'and needs no group to find its geometries — it holds them')


def test_merge_and_export_are_verbs_too(tmp_path):
    """The last two things the app's menus do: combining compatible
    objects, and writing one out to a foreign format."""
    project = hertzforge.Project()
    project.add('Front', hertzforge.Geometry(node_id=[1, 2],
                                       node_xyz=[[0, 0, 0], [1, 0, 0]]))
    project.add('Back', hertzforge.Geometry(node_id=[3, 4],
                                      node_xyz=[[2, 0, 0], [3, 0, 0]]))
    whole = project.merge('Front', 'Back', name='Airframe')
    assert list(project) == ['Airframe']
    assert project['Airframe'].num_nodes == 4

    path = project.export('Airframe', tmp_path / 'airframe.unv')
    assert pathlib.Path(path).stat().st_size > 100
    assert hertzforge.import_file(path).num_nodes == 4, 'and it reads back'
    assert whole == 'Airframe'


def test_objects_are_reached_by_type_not_by_name(survey):
    """A name is arbitrary and goes stale; a type does not."""
    shapes, frfs = survey
    project = hertzforge.Project()
    project.add('Geometry', hertzforge.import_file(
        fixture_path('plate', 'geometry.npz')))
    project.add('Measured FRFs', frfs)          # any name at all
    project.add('Identified Modes', shapes)
    project.set_basis('Geometry', 'Measured FRFs', 'Identified Modes')

    assert project.frf is frfs, 'across the project'
    assert project.basis.frf is frfs, 'and within a group'
    assert project.basis.shapes is shapes
    assert project.basis.geometry.num_nodes == 169
    assert project.frfs == [frfs], 'the plural is always a list'
    assert project.specifications == [], 'even when there are none'


def test_the_singular_says_which_ones_it_found(survey):
    """Sometimes-an-object-sometimes-a-list would break code the day a
    second object arrives; this refuses and names them instead."""
    shapes, _frfs = survey
    project = hertzforge.Project()
    project.add('First', shapes)
    assert project.shapes is shapes
    project.add('Second', shapes)
    with pytest.raises(AttributeError) as raised:
        _ = project.shapes
    message = str(raised.value)
    assert "'First'" in message and "'Second'" in message
    assert '.shape_sets[i]' in message, 'and how to pick one'
    assert len(project.shape_sets) == 2, 'the plural still answers'
    with pytest.raises(AttributeError, match='no frf in'):
        _ = project.frf


def test_completion_offers_what_the_project_holds(survey):
    shapes, frfs = survey
    project = hertzforge.Project()
    project.add('Shapes', shapes)
    offered = set(dir(project))
    assert {'shapes', 'shape_sets'} <= offered
    assert 'frf' not in offered, 'nothing here is an FRF yet'
    project.add('FRF', frfs)
    assert {'frf', 'frfs'} <= set(dir(project)), 'and now there is'
    # the scoped view offers only what is in scope
    project.set_basis('Shapes', 'FRF')
    assert {'shapes', 'frf', 'names'} <= set(dir(project.basis))


def test_the_view_is_never_a_snapshot(survey):
    """Attributes are computed on access, so adds, renames and deletes
    show up without anything to invalidate."""
    shapes, frfs = survey
    project = hertzforge.Project()
    project.add('Shapes', shapes)
    assert project.shapes is shapes
    project.rename('Shapes', 'Truth')
    assert project.shapes is shapes, 'renaming changes the name, not the kind'
    project.add('FRF', frfs)
    assert project.frf is frfs
    project.remove('Truth')
    with pytest.raises(AttributeError, match='no shapes in project'):
        _ = project.shapes


def test_groups_are_reached_the_same_way(survey):
    shapes, frfs = survey
    project = hertzforge.Project()
    project.add('Test Geometry', hertzforge.import_file(
        fixture_path('plate', 'geometry.npz')))
    project.add('FRF', frfs)
    project.add('FEM Geometry', hertzforge.import_file(
        fixture_path('plate', 'geometry.npz')))
    project.add('FEM Modes', shapes)
    project.set_basis('Test Geometry', 'FRF')
    project.link('FEM Geometry', 'FEM Modes')

    assert project.basis.frf is frfs
    assert project.other.shapes is shapes, 'the one group that is not it'
    assert [group.names for group in project.object_group_selections] == [
        ['Test Geometry', 'FRF'], ['FEM Geometry', 'FEM Modes']]
    assert not hertzforge.Project().basis, 'no Basis declared: empty, not everything'


def test_verbs_take_objects_as_well_as_names():
    """So the two halves compose: reach an object by type, hand it
    straight to the verb that stores and links the result."""
    time = _time_history()
    project = hertzforge.Project()
    project.add('Time History', time)
    added = project.compute_psds(project.time_history)
    assert added == 'Time History PSDs'
    assert project.psd is project[added]
    assert added in project.object_group_of(project.time_history)
    assert project.name_of(project.psd) == added
    with pytest.raises(ValueError, match='not in this project'):
        project.name_of(_time_history())


def _time_history():
    from hertzforge.core.data import TimeHistory

    t = np.arange(256) / 256.0
    rows = [np.sin(2 * np.pi * 16 * t)] * 2
    time = TimeHistory(t, np.array(rows), response_dof=['1X+', '1X+'],
                       block=['avg 1', 'avg 2'])
    time.define_units('m/s^2')
    return time


def test_a_refusing_property_keeps_its_own_message():
    """Python routes an AttributeError raised *inside* a property to
    __getattr__, which would answer 'no such kind of object' — the
    wrong reason entirely."""
    project = hertzforge.Project()
    for tag, node in (('Test', 101), ('FEM', 200), ('Updated', 300)):
        project.add(f'{tag} Geometry', _geometry(node))
        project.add(f'{tag} Modes', _shapes(node))
        project.link(f'{tag} Geometry', f'{tag} Modes')
    project.set_basis('Test Modes')
    with pytest.raises(AttributeError, match='besides the Basis'):
        _ = project.other
    assert len(project.object_group_selections) == 3, 'and every one is reachable'
    assert project.object_group_selections[2].shapes is project['Updated Modes']


def test_no_object_type_sorts_after_the_report():
    """Every kind of object reads above the report that reports on it.

    `type_rank` walks TYPE_ORDER and takes the first `isinstance` match,
    so a class that subclasses nothing listed falls through to
    `len(TYPE_ORDER)` — past the end, and past Report with it. That is
    not a wrong position, it is *no* position, and it is silent: the
    type appears, sorts last, and nothing says why.

    It happened. `Srs` subclasses no listed class, so a shock project
    listed Report above both its SRS and its shock specification — the
    report over the two things the report is about. Both project types
    that run a shock had it.

    Written against `NAMED_CLASSES` — the native format's own list of
    every persistable data class — plus the non-data types, so adding a
    class anywhere makes this fail rather than making a tree quietly
    wrong.
    """
    from hertzforge.core.channel_table import ChannelTable
    from hertzforge.core.data import NAMED_CLASSES
    from hertzforge.core.geometry import Geometry
    from hertzforge.core.matches import MatchedModes
    from hertzforge.core.photos import Photos
    from hertzforge.core.report import Report
    from hertzforge.core.shapes import ShapeSet
    from hertzforge.project import TYPE_ORDER, type_rank

    everything = [*NAMED_CLASSES.values(), Geometry, Photos, ChannelTable,
                  ShapeSet, MatchedModes, Report]
    homeless = [cls.__name__ for cls in everything
                if type_rank(cls.__new__(cls)) >= len(TYPE_ORDER)]
    assert not homeless, (
        f'{homeless} match nothing in TYPE_ORDER, so they sort after '
        'Report. Give the class an entry, or place it under one it '
        'subclasses.')

    report = type_rank(Report.__new__(Report))
    below = [cls.__name__ for cls in everything
             if cls is not Report and type_rank(cls.__new__(cls)) >= report]
    assert not below, f'{below} sort at or after the Report'


def test_a_shock_project_reads_in_derivation_order():
    """The bug the rank guard is about, from the outside: the tree."""
    import numpy as np

    from hertzforge.core.data import ShockSpecification, Srs, TimeHistory
    from hertzforge.core.report import Report

    f = np.linspace(10.0, 2000.0, 40)
    units = {'response_dof': ['1Z+'], 'ordinate_dim': 'acceleration',
             'ordinate_unit': 'm/s**2'}
    t = np.linspace(0.0, 1.0, 64)
    project = hertzforge.Project('Shock')
    project.add('Report', Report('R'))
    project.add('SRS', Srs(f, np.ones((1, 40)), **units))
    project.add('Shock Specification',
                ShockSpecification(f, np.ones((1, 40)) * 2.0, **units))
    project.add('Time Data', TimeHistory(t, np.sin(t)[None], **units))
    assert project.ordered_names() == [
        'Time Data', 'SRS', 'Shock Specification', 'Report'], (
        'the report reads last, under the two things it is about')


def test_a_kind_never_answers_with_what_a_narrower_kind_claims():
    """The same rule the report's symbolic bindings live by, arrived at
    the same way. A TransientSpecification *is* a TimeHistory, so in a
    transient project `project.time_history` found two and refused,
    though there is exactly one thing anyone asking means; and
    `project.psds` handed back the specification alongside the measured
    PSDs, so every caller filtered it back out by hand.
    """
    import numpy as np

    from hertzforge.core.data import (
        Psd,
        Specification,
        TimeHistory,
        TransientSpecification,
    )

    project = hertzforge.Project()
    project.add('History', TimeHistory(
        np.arange(8) / 256.0, np.ones((1, 8)), response_dof=['101Z+']))
    project.add('Target', TransientSpecification(
        np.arange(8) / 256.0, np.ones((1, 8)), response_dof=['101Z+']))
    assert project.time_history is project['History'], (
        'the broad kind means the thing that is only that kind')
    assert project.transient_specification is project['Target']
    assert [project.name_of(h) for h in project.time_histories] == \
        ['History'], 'the plural excludes it too'

    project.add('PSD', Psd(np.arange(4.0), np.ones((1, 4)),
                           response_dof=['101Z+']))
    project.add('Spec', Specification(np.arange(4.0), np.ones((1, 4)),
                                      response_dof=['101Z+']))
    assert project.psd is project['PSD']
    assert project.specification is project['Spec']


def test_the_singular_accessor_is_the_plain_object_beside_its_banded_one():
    """A worked-up random project holds the requirement on lines and on
    octave bands: `.specification` is the one on lines, as
    '@basis:Specification' is in a report; two on lines still refuse."""
    import numpy as np

    freq = np.linspace(10.0, 2000.0, 200)
    level = np.full((1, 200), 1e-3)
    spec = hertzforge.Specification(
        freq, level, response_dof=['101Z+'],
        ordinate_dim=['acceleration**2/frequency'], ordinate_unit=['m/s**2'])
    psd = hertzforge.Psd(freq, level, response_dof=['101Z+'],
                             ordinate_dim=['acceleration**2/frequency'],
                             ordinate_unit=['m/s**2'])
    project = hertzforge.Project('t')
    project.add('Spec', spec)
    project.add('Octave Spec', spec.to_octave(6))
    project.add('PSD', psd)
    project.add('Octave PSD', psd.to_octave(6))
    assert project.specification is project['Spec']
    assert project.psd is project['PSD']
    assert len(project.specifications) == 2 and len(project.psds) == 2
    project.add('Spec 2', spec)
    with pytest.raises(AttributeError, match='3 in project'):
        _ = project.specification


def test_a_groups_name_survives_unlinking_and_removing_from_other_groups():
    """Unlinking and pruning rebuilt every group from its members and
    role, so a named group lost its name when anything was unlinked or
    removed anywhere in the project (2026-10-13)."""
    project = hertzforge.Project('p')
    for name, node in (('A', 101), ('C', 103)):
        project.add(name, _geometry(node))
    for name, node in (('B', 101), ('D', 103), ('E', 103)):
        project.add(name, _shapes(node))
    project.link('A', 'B', name='first')
    project.link('C', 'D', 'E', name='second')
    project.unlink('E')
    assert [g.get('name') for g in project.object_groups] == ['first', 'second']
    project.remove('D')
    assert [g.get('name') for g in project.object_groups] == ['first']
