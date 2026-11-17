"""A geometry's bare and measured nodes drawn as point layers."""

import numpy as np

from hertzforge import Project
from hertzforge.core.geometry import Geometry
from hertzforge.io.point_files import points_geometry
from hertzforge.service import scans, show

from .conftest import wire_views


def _project():
    project = Project()
    rng = np.random.default_rng(3)
    project.add('Cloud', points_geometry(rng.uniform(0, 100, (9000, 3)) * [1, 1, 0.05]))
    project.add('Probed', points_geometry(
        [[10, 0, 5], [10, 4, 5], [10, 2, 9], [0, 12, 5], [50, 50, 2]],
        length_unit='mm', features=['31_1', '31_2'], feature_of=[0, 0, 0, 1, -1]))
    return project


def test_measured_points_are_drawn_in_their_features_colors(validate):
    project = _project()
    (view,) = show.show(project, ['Probed'], {}, None)['views']
    wire, arrays = wire_views([view], validate)
    assert wire['views'][0]['points'][0]['xyz']['$array'] in arrays
    assert view['view'] == 'shape3d' and view['unit'] == 'm' and len(view['nodes']) == 5
    (layer,) = view['points']
    assert layer['xyz'].dtype == np.float32 and layer['xyz'].shape == (5, 3)
    assert layer['colors'].dtype == np.uint8 and layer['size'] == scans.PROBED_SIZE
    np.testing.assert_allclose(layer['xyz'][2], [0.010, 0.002, 0.009], rtol=1e-6)
    # a feature's points share a color, another feature's differ, a point in none is gray
    colors = layer['colors']
    assert (colors[0] == colors[1]).all() and (colors[0] != colors[3]).any()
    assert tuple(colors[4]) == scans.BARE_COLOR


def test_a_cloud_is_colored_by_height_along_its_thin_axis():
    project = _project()
    (view,) = show.show(project, ['Cloud'], {}, None)['views']
    (layer,) = view['points']
    assert view['unit'] == 'undefined' and layer['size'] == scans.CLOUD_SIZE
    height = project['Cloud'].node_xyz[:, 2]
    low, high = layer['colors'][np.argmin(height)], layer['colors'][np.argmax(height)]
    assert tuple(low) == (68, 1, 84) and tuple(high) == (253, 231, 37)


def test_two_geometries_shown_together_keep_their_points():
    project = _project()
    (view,) = show.show(project, ['Cloud', 'Probed'], {}, None)['views']
    assert [len(layer['xyz']) for layer in view['points']] == [9000, 5]


def test_only_nodes_no_line_face_or_solid_uses_are_points():
    """A node on a beam is drawn by the beam; a brick's inside node is
    used by the brick though only its skin is drawn; a node alone is a
    point."""
    corners = [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0], [0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]]
    geometry = Geometry(node_id=np.arange(1, 12),
                        node_xyz=[*corners, [3, 0, 0], [4, 0, 0], [5, 5, 5]], length_unit='m')
    geometry.add_beams([9, 10])
    geometry.add_element(list(range(1, 9)), elem_type=115)
    assert scans.undrawn_rows(geometry).tolist() == [10]
    project = Project()
    project.add('Model', geometry)
    (view,) = show.show(project, ['Model'], {}, None)['views']
    (layer,) = view['points']
    np.testing.assert_allclose(layer['xyz'], [[5, 5, 5]])
    assert tuple(layer['colors'][0]) == scans.BARE_COLOR
    # every node on an element: no layer at all
    plain = Geometry(node_id=[1, 2], node_xyz=[[0, 0, 0], [1, 0, 0]], length_unit='m')
    plain.add_beams([1, 2])
    assert scans.layers(plain) == ([], '')


def test_a_large_cloud_is_drawn_every_nth_point(monkeypatch):
    monkeypatch.setattr(scans, 'DRAWN_POINTS', 4000)
    project = _project()
    shown = show.show(project, ['Cloud'], {}, None)
    assert len(shown['views'][0]['points'][0]['xyz']) == 3000
    assert shown['views'][0]['note'] == 'every 3th point of 9,000 is drawn'


def test_points_are_in_the_display_unit_with_the_nodes():
    """The points travel in the unit the nodes travel in: both in inches
    under an inch system, not the nodes in inches and the points in meters."""
    from hertzforge.service import display_units

    project = _project()
    (view,) = show.show(project, ['Probed'], {}, None)['views']
    inch = next(name for name in display_units.systems() if name.startswith('in'))
    display_units.convert([view], inch)
    assert view['unit'] == 'in'
    np.testing.assert_allclose(view['points'][0]['xyz'], view['nodes'], rtol=1e-6)
    np.testing.assert_allclose(view['nodes'][2], np.array([10, 2, 9]) / 25.4, rtol=1e-6)
