"""A geometry's default view (Brandon, 2026-09-27).

Every 3-D view of a geometry opened from +X+Y+Z with Z up — the app by
PyVista's default camera, the report by an isometric of its own — and
nothing let a geometry say otherwise, so a model built Y-up drew lying
on its side and its pictures turned its nodes to stand it up. A geometry
carries a `View` now: the app opens it and everything drawn on it on
that view, Reset View returns to it, and the report's scenes and the
exported figures are drawn from it.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge import View


def _camera_basis(plotter):
    """(right, up) on screen, from a PyVista camera."""
    position, focus, up = (np.asarray(v, dtype=float)
                           for v in plotter.camera_position)
    sight = focus - position
    sight /= np.linalg.norm(sight)
    right = np.cross(sight, up)
    right /= np.linalg.norm(right)
    return right, np.cross(right, sight)


def _plate():
    from hertzforge import mesh

    return mesh.plane((0, 0, 0), (1, 0, 0), (0, 1, 0), 0.25)


def test_a_view_is_a_direction_and_an_up():
    view = View(eye=(2, 2, -2), up=(0, 3, 0))
    assert view == View(eye=(1, 1, -1), up=(0, 1, 0)), 'lengths do not count'
    assert view.eye == (2.0, 2.0, -2.0), 'kept as written'
    right, up = view.basis()
    # a camera's own basis: right-handed, the eye on the near side
    assert np.allclose(np.cross(right, up), np.array([1, 1, -1]) / np.sqrt(3))
    assert abs(np.dot(up, [1, 1, -1])) < 1e-12
    assert eval(repr(view), {'View': View}) == view
    with pytest.raises(ValueError, match='line of sight'):
        View(eye=(0, 0, 1), up=(0, 0, 2))
    with pytest.raises(ValueError, match='direction'):
        View(eye=(0, 0, 0))


def test_a_geometry_without_one_opens_on_the_old_isometric():
    geometry = _plate()
    assert geometry.view is None
    assert geometry.opening_view == View(eye=(1, 1, 1), up=(0, 0, 1))


def test_a_view_survives_the_project_file(tmp_path):
    geometry = _plate()
    geometry.view = View(eye=(0, -1, 0.5), up=(0, 0, 1))
    bare = _plate()
    project = hertzforge.Project('views')
    project.add('Turned', geometry)
    project.add('Bare', bare)
    project.save(tmp_path / 'views.vdyn')
    back = hertzforge.load(tmp_path / 'views.vdyn')
    assert back['Turned'].view == geometry.view
    assert back['Bare'].view is None, 'absent stays absent'


def test_a_merged_geometry_keeps_the_view():
    from hertzforge import mesh

    first = _plate()
    second = mesh.plane((0, 0, 1), (1, 0, 0), (0, 1, 0), 0.25)
    second.node_id = second.node_id + 1000
    second.elem_conn = [conn + 1000 for conn in second.elem_conn]
    second.view = View(eye=(1, 0, 0), up=(0, 0, 1))
    project = hertzforge.Project('merge')
    project.add('A', first)
    project.add('B', second)
    merged = project[project.merge('A', 'B')]
    assert merged.view == second.view


def test_set_view_is_a_journaled_verb():
    project = hertzforge.Project('journal')
    project.add('Plate', _plate())
    view = View(eye=(1, 0, 1), up=(0, 0, 1))
    project.set_view('Plate', view)
    assert project['Plate'].view == view
    script = project.session_script()
    assert 'from hertzforge import View' in script
    assert ("project.set_view('Plate', View(eye=(1.0, 0.0, 1.0), "
            "up=(0.0, 0.0, 1.0)))") in script
    project.set_view('Plate', None)
    assert project['Plate'].view is None
    assert 'set_view' in dict(project.selection_verbs('Plate'))
    with pytest.raises(TypeError, match='view must be a View'):
        project.set_view('Plate', ((1, 0, 0), (0, 0, 1)))


def test_a_report_scene_opens_on_the_geometry_view():
    from hertzforge.report import _scene_block

    geometry = _plate()
    geometry.view = View(eye=(1, 1, -1), up=(0, 1, 0))
    built = _scene_block({'kind': 'scene'}, geometry, None,
                         hertzforge.SI, {})
    assert np.allclose(built['home'], geometry.view.basis(), atol=1e-6)
    geometry.view = None
    built = _scene_block({'kind': 'scene'}, geometry, None,
                         hertzforge.SI, {})
    assert np.allclose(built['home'], View().basis(), atol=1e-6)
