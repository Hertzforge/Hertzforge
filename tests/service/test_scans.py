"""A scan in the service: listed, drawn as point layers, its unit declared."""

import numpy as np

from hertzforge import Project
from hertzforge.core.geometry import Geometry
from hertzforge.core.scan import Scan
from hertzforge.service import projects, scans, show, units

from .conftest import wire_views


def _project():
    project = Project()
    rng = np.random.default_rng(3)
    project.add('Cloud', Scan(rng.uniform(0, 100, (9000, 3)) * [1, 1, 0.05]))
    project.add('Probed', Scan([[10, 0, 5], [10, 4, 5], [10, 2, 9], [0, 12, 5], [50, 50, 2]],
                               length_unit='mm', features=['31_1', '31_2'],
                               feature_of=[0, 0, 0, 1, -1]))
    return project


def test_a_scan_is_drawn_as_points(validate):
    project = _project()
    shown = show.show(project, ['Probed'], {}, None)
    (view,) = shown['views']
    wire, arrays = wire_views([view], validate)
    assert wire['views'][0]['points'][0]['xyz']['$array'] in arrays
    assert view['view'] == 'shape3d' and view['unit'] == 'm' and len(view['nodes']) == 0
    (layer,) = view['points']
    assert layer['xyz'].dtype == np.float32 and layer['xyz'].shape == (5, 3)
    assert layer['colors'].dtype == np.uint8 and layer['size'] == scans.PROBED_SIZE
    np.testing.assert_allclose(layer['xyz'][2], [0.010, 0.002, 0.009], rtol=1e-6)
    # a feature's points share a color, another feature's differ, a point in none is gray
    colors = layer['colors']
    assert (colors[0] == colors[1]).all() and (colors[0] != colors[3]).any()
    assert tuple(colors[4]) == scans.UNASSIGNED_COLOR


def test_a_cloud_is_colored_by_height_along_its_thin_axis():
    project = _project()
    (view,) = show.show(project, ['Cloud'], {}, None)['views']
    (layer,) = view['points']
    assert view['unit'] == 'undefined' and layer['size'] == scans.CLOUD_SIZE
    height = project['Cloud'].points[:, 2]
    low, high = layer['colors'][np.argmin(height)], layer['colors'][np.argmax(height)]
    assert tuple(low) == (68, 1, 84) and tuple(high) == (253, 231, 37)


def test_scans_and_a_geometry_share_one_scene():
    project = _project()
    project.add('Frame', Geometry(node_id=[1, 2], node_xyz=[[0, 0, 0], [0.1, 0, 0]],
                                  length_unit='m'))
    shown = show.show(project, ['Cloud', 'Probed'], {}, None)
    (view,) = shown['views']
    assert [len(layer['xyz']) for layer in view['points']] == [9000, 5]
    assert 'not drawn to one scale' in shown['note']
    shown = show.show(project, ['Frame', 'Probed'], {}, None)
    (view,) = shown['views']
    assert len(view['nodes']) == 2 and len(view['points']) == 1 and shown['note'] == ''


def test_a_large_scan_is_drawn_every_nth_point(monkeypatch):
    monkeypatch.setattr(scans, 'DRAWN_POINTS', 4000)
    project = _project()
    shown = show.show(project, ['Cloud'], {}, None)
    assert len(shown['views'][0]['points'][0]['xyz']) == 3000
    assert shown['note'] == 'Cloud: every 3th point of 9,000 is drawn'


def test_a_scan_is_listed_and_takes_a_length_unit():
    project = _project()
    assert projects.kind_of(project['Cloud']) == 'scan'
    (table,) = units.tables(project, 'Cloud')['tables']
    assert [(row['key'], row['unit']) for row in table['rows']] == [('length', None)]
    was = project['Cloud'].points.copy()
    units.define(project, 'Cloud', [{'side': 'object', 'key': 'length', 'unit': 'mm'}])
    np.testing.assert_allclose(project['Cloud'].points, was / 1000)
    assert project.journal[-1] == "project['Cloud'].define_units('mm')"
