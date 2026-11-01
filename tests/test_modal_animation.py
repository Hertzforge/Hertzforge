"""Modal coordinates played on a geometry through their shape set.

Brandon, 2026-10-25: a modal response selected with a geometry should
animate without forming the physical response at every DOF, which for a
large finite element model is far larger than the modal one. Each frame
is the modes' shapes scaled by their values at that sample and summed.
Checked against the same record expanded and played the ordinary way.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import plate_geometry_and_shapes

import hertzforge
from hertzforge.core.data import TimeHistory
from hertzforge.core.shapes import ShapeSet
from hertzforge.core.transform import modal_dofs
from hertzforge.core.truncate import Truncation
from hertzforge.deform import (
    ModalTimeDeflection,
    TimeDeflection,
    modal_deflection,
)

FS = 1024.0


def _modal_record(shapes, samples=300, seed=0, extra=()):
    """Every mode's coordinate as a record, with `extra` (dof, dim)
    records ahead of them."""
    rng = np.random.default_rng(seed)
    q = rng.standard_normal((shapes.num_shapes, samples))
    rows = [rng.standard_normal(samples) for _ in extra] + list(q)
    return TimeHistory(np.arange(samples) / FS, np.asarray(rows),
                       response_dof=[d for d, _ in extra] + modal_dofs(shapes),
                       ordinate_dim=[k for _, k in extra]
                       + ['modal_acceleration'] * shapes.num_shapes), q


def _expanded(geometry, shapes, q):
    return TimeDeflection(geometry, shapes.coordinate,
                          np.real(shapes.shape_matrix).T @ q)


def test_every_frame_is_the_expanded_response():
    geometry, shapes = plate_geometry_and_shapes()
    _record, q = _modal_record(shapes)
    modal = ModalTimeDeflection(geometry, shapes.coordinate,
                                shapes.shape_matrix, q)
    physical = _expanded(geometry, shapes, q)
    assert np.array_equal(modal.rows, physical.rows)
    assert modal.num_frames == physical.num_frames == 300
    for k in (0, 17, 299):
        assert np.allclose(modal.offsets(k).copy(), physical.offsets(k),
                           rtol=0, atol=1e-12)
    assert modal.peak == pytest.approx(physical.peak, rel=1e-12)
    assert modal.peak_magnitude == pytest.approx(physical.peak_magnitude,
                                                 rel=1e-12)


def test_past_the_budget_the_loudest_frames_find_the_peak():
    """A frame where every mode is moderate but all push one node the
    same way travels further than any mode's own largest sample: the
    bound finds it, where each mode's largest sample alone would not."""
    geometry, shapes = plate_geometry_and_shapes()
    rng = np.random.default_rng(4)
    q = 0.05 * rng.standard_normal((shapes.num_shapes, 2000))
    for r in range(shapes.num_shapes):
        q[r, 100 + 50 * r] = 1.0           # each mode's own largest
    probe = ModalTimeDeflection(geometry, shapes.coordinate,
                                shapes.shape_matrix, q)
    column = probe._fields[:, 3 * 40 + 2]  # one node's Z, every mode
    q[:, 1777] = 0.6 * np.sign(column)
    modal = ModalTimeDeflection(geometry, shapes.coordinate,
                                shapes.shape_matrix, q)
    modal.EXACT_BUDGET = 0                 # the estimate, not the sweep
    physical = _expanded(geometry, shapes, q)
    travel = [np.linalg.norm(physical.offsets(k), axis=1).max()
              for k in range(q.shape[1])]
    assert int(np.argmax(travel)) == 1777, 'the test holds its premise'
    assert modal.peak_magnitude == pytest.approx(physical.peak_magnitude,
                                                 rel=1e-12)
    assert modal.peak == pytest.approx(physical.peak, rel=1e-12)


def test_the_records_played_are_the_sets_modal_coordinates():
    geometry, shapes = plate_geometry_and_shapes()
    record, q = _modal_record(shapes, extra=[('101Z+', 'force'),
                                             ('M30', 'modal_acceleration')])
    played = modal_deflection(geometry, shapes, record,
                              range(record.num_records))
    assert np.allclose(played.offsets(5).copy(),
                       _expanded(geometry, shapes, q).offsets(5))
    one = modal_deflection(geometry, shapes, record, [2 + 7])
    alone = TimeDeflection(geometry, shapes.coordinate,
                           np.outer(np.real(shapes.shape_matrix[7]), q[7]))
    assert np.allclose(one.offsets(5).copy(), alone.offsets(5)), \
        "a pick of one mode is that mode's contribution"
    with pytest.raises(ValueError, match='no record is a modal coordinate'):
        modal_deflection(geometry, shapes, record, [0, 1])
    complex_set = ShapeSet(shapes.frequency, shapes.damping,
                           shapes.coordinate, shapes.shape_matrix * 1j)
    with pytest.raises(ValueError, match='complex'):
        modal_deflection(geometry, complex_set, record, [2])


def _plate_project():
    geometry, shapes = plate_geometry_and_shapes()
    project = hertzforge.Project()
    project.add('Plate', geometry)
    project.add('Modes', shapes)
    project.link('Plate', 'Modes')
    t = np.arange(2048) / FS
    force = np.random.default_rng(2).standard_normal(t.size)
    project.add('Run', TimeHistory(t, force[None, :], response_dof='101Z+',
                                   ordinate_dim='force'))
    return project, geometry, shapes


def _answered(project, shapes='Modes', modes=range(6, 22)):
    """The modal responses computed into an object of their own, merged
    back into 'Run' beside its force, and that object's name."""
    made = project.compute_modal_responses('Run', shapes, modes=modes)
    project.merge('Run', made)
    return made


def test_the_set_behind_computed_responses_is_read_from_them():
    project, _geometry, shapes = _plate_project()
    made = project.compute_modal_responses('Run', 'Modes', modes=range(6, 22))
    assert project.modal_basis(made) == 'Modes'
    project.merge('Run', made)
    assert project.modal_basis('Run') == 'Modes'
    assert project.modal_basis('Run', records=[0]) is None, 'the force'
    project.add('Other', shapes)
    # M6: a second M7 would be the same channel twice, which a merge refuses
    _answered(project, 'Other', modes=[5])
    assert project.modal_basis('Run') is None, 'two sets: none says'
    assert project.modal_basis('Run', records=[17]) == 'Other'
    del project['Other']
    assert project.modal_basis('Run', records=[17]) is None, 'set gone'


def test_the_set_behind_a_transform_is_read_from_its_provenance():
    project, _geometry, shapes = _plate_project()
    q = np.random.default_rng(5).standard_normal((shapes.num_shapes, 512))
    translational = [i for i, dof in enumerate(shapes.coordinate)
                     if 'R' not in dof]
    motion = np.real(shapes.shape_matrix).T[translational] @ q
    project.add('Motion', TimeHistory(
        np.arange(512) / FS, motion,
        response_dof=[shapes.coordinate[i] for i in translational],
        ordinate_dim='acceleration'))
    made = project.transform('Motion', 'Modes')
    assert project.modal_basis(made) == 'Modes'
    project[made].truncation = Truncation(0.1, 0.3)
    cut = project.truncate_data(made)
    assert project.modal_basis(cut) == 'Modes', \
        'a record derived from one keeps its set'
    assert project.modal_basis('Motion') is None


def test_merged_responses_add_no_capture_labels():
    """A history with no capture labels keeps none: blank ones read as a
    capture named '' and the playback said 'animating  of 1 averages'."""
    project, _geometry, _shapes = _plate_project()
    made = _answered(project, modes=[6, 7])
    assert made not in project, 'merged into Run'
    assert project['Run'].block is None
