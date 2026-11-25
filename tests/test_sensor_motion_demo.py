"""Infer Geometry on the BARC: fourteen sensors of one, two and three axes
placed from responses the package computes itself (`demo.sensor_motion`)."""

import numpy as np
import pytest

from hertzforge.core import motion_geometry as mg
from hertzforge.demo import sensor_motion

#: four sensors of three axes that are not in a plane
KNOWN = (1, 2, 3, 5)


def _truth(project):
    truth = project[sensor_motion.TRUE]
    return {int(node): xyz for node, xyz in zip(truth.node_id, truth.node_xyz, strict=True)}


def _some_nodes(project):
    from hertzforge import Geometry

    truth = _truth(project)
    return project.add('Four Known', Geometry(node_id=list(KNOWN),
                                              node_xyz=[truth[node] for node in KNOWN],
                                              length_unit='in'))


def _worst(project, recovered):
    """The worst position error, m: of a one-axis sensor, its line's
    distance from the true place."""
    truth = _truth(project)
    worst = 0.0
    for one in recovered.sensors:
        gap = one.position - truth[one.node]
        if one.slides:
            along = one.frame[one.axes[0]]
            gap = gap - (gap @ along) * along
        worst = max(worst, float(np.linalg.norm(gap)))
    return worst


def _axes_off(project, recovered):
    """The worst angle, degrees, between a measured axis and the true one."""
    truth = project[sensor_motion.TRUE]
    worst = 0.0
    for one in recovered.sensors:
        for axis in one.axes:
            true = truth.dof_direction(f'{one.node}{"XYZ"[axis]}+')
            worst = max(worst, float(np.degrees(np.arccos(np.clip(one.frame[axis] @ true, -1, 1)))))
    return worst


@pytest.fixture(scope='module')
def random_test():
    return sensor_motion.project()


@pytest.fixture(scope='module')
def axis_tests():
    return sensor_motion.project(sensor_motion.AXIS_TESTS)


def test_one_random_test_places_every_sensor(random_test):
    """All twelve drives running: the fourteen sensors come back within a
    quarter of a millimeter on a structure 0.2 m tall, and their axes within
    a quarter of a degree, from four known nodes and the responses."""
    project = random_test
    known = _some_nodes(project)
    history = project['BARC Random']
    assert set(history.ordinate_dim) == {'acceleration'} and len(history.response_dof) == 28
    found = project.motion_sensors('BARC Random', known=known)
    assert [len(one.axes) for one in found.sensors] == [3] * 5 + [2] * 4 + [1] * 5
    low, high = found.motion.band
    # below the first elastic mode, 194 Hz
    modes = np.asarray(project['BARC Modes'].frequency)
    assert modes[modes > 1.0][0] == pytest.approx(194.4, abs=0.5)
    assert low < 20 and 60 < high < 150
    assert not found.image_open and found.length_unit == 'in' and not found.notes
    assert _worst(project, found) < 0.25e-3 and _axes_off(project, found) < 0.25
    assert found.misfit < mg.GOOD_MISFIT


def test_the_geometry_made_draws_the_data_where_the_sensors_are(random_test):
    project = random_test
    made = project.geometry_from_motion('BARC Random', known=sensor_motion.TRUE, name='Found')
    geometry, truth = project[made], project[sensor_motion.TRUE]
    assert geometry.node_id.tolist() == list(sensor_motion.SENSORS)
    rows = truth.node_index(geometry.node_id)
    # every node is known here, the one-axis sensors' too, so each is slid to its place
    np.testing.assert_allclose(geometry.node_xyz, truth.node_xyz[rows], atol=0.25e-3)
    assert not geometry.missing_dofs(project['BARC Random'].response_dof)
    project.remove(made)


def test_single_axis_tests_place_the_sensors_only_together(axis_tests):
    """Three tests, each driving along one global axis, move the free BARC in
    three rigid motions apiece. One is refused, two are refused, and the
    three read together place the sensors as well as one test of all six."""
    project = axis_tests
    names = list(sensor_motion.AXIS_TESTS)
    known = _some_nodes(project)
    for picked in ([names[0]], [names[1]], names[:2]):
        with pytest.raises(ValueError, match='six rigid motions do not stand clear'):
            project.motion_sensors(*picked, known=known)
    found = project.motion_sensors(*names, known=known)
    assert _worst(project, found) < 0.25e-3 and _axes_off(project, found) < 0.25


def test_four_triaxial_sensors_fit_once_released_and_nothing_is_said(random_test):
    """Four tri-axial sensors give just enough linear conditions to start
    from, and the start is poor. Looked into, the worst sensor's channels
    turn out square, meeting and right-handed: the sensor is put back, the
    fit is good from where that left it, and no DOF is said to be wrong."""
    from hertzforge.core.data import parse_dof

    history = random_test['BARC Random']
    rows = [row for row, dof in enumerate(history.response_dof) if parse_dof(dof)[0] in (1, 2, 3, 4)]
    motion = mg.rigid_motion([history], records=[rows])
    start = mg._started(motion.basis, mg._sensors_of(motion.channels), None)
    assert start is not None and start[1] > mg.POOR_MISFIT
    found = mg.fit_poses(motion)
    assert found.misfit < mg.GOOD_MISFIT and found.starts == 2
    assert not found.notes and not found.left_out
    assert [(one.node, one.axes, one.reversed, one.note) for one in found.sensors] == [
        (node, (0, 1, 2), (), '') for node in (1, 2, 3, 4)]
