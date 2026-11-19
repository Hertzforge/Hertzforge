import numpy as np
import pytest
from conftest import fixture_path, plate_geometry_and_shapes

import hertzforge
from hertzforge.deform import ShapeDeflection


@pytest.fixture(scope='module')
def model():
    return plate_geometry_and_shapes()


def _mode(model, index=3):
    geometry, shapes = model
    return geometry, ShapeDeflection(geometry, shapes.coordinate,
                                     shapes.shape_matrix[index])


@pytest.fixture(scope='module')
def measured():
    geometry = hertzforge.import_file(
        fixture_path('plate', 'geometry.npz'), length_unit='m')
    frf = hertzforge.import_file(
        fixture_path('plate', 'frfs.npz'))
    psd = hertzforge.import_file(
        fixture_path('plate', 'psd.npz'))
    return geometry, frf, psd


def test_envelope_records_is_the_one_filtering_rule():
    """The app's envelope view and the headless call share this rule
    (they each carried a copy once): cross records go first, then one
    quantity — both before the one-record-per-DOF dedupe, or a drive
    point's cross row or force PSD listed first would shadow its own
    DOF's genuine accelerometer auto."""

    from hertzforge.core.data import Psd
    from hertzforge.deform import envelope_records

    f = np.arange(8.0)
    rows = np.ones((4, 8))
    data = Psd(f, rows,
               response_dof=['101Z+', '101Z+', '101Z+', '102Z+'],
               reference_dof=['9001X+', '', '101Z+', ''],
               ordinate_dim=['acceleration**2/frequency',
                             'force**2/frequency',
                             'acceleration**2/frequency',
                             'acceleration**2/frequency'])
    indices, quantity, crossed, left_out = envelope_records(data)
    assert crossed == 1, 'the true cross record (its reference elsewhere)'
    assert quantity == 'acceleration', 'the commonest kind answers'
    assert indices == [2, 3], \
        'the drive point auto survives its own cross row and force row'
    assert left_out == 1, 'the force record waits for the quantity box'

    indices, quantity, _crossed, left_out = envelope_records(
        data, quantity='force')
    assert quantity == 'force' and indices == [1]
    assert left_out == 2, 'now the accelerations wait'

    asked, _q, _c, _l = envelope_records(data, records=[0, 3])
    assert asked == [3], 'an explicit record list is filtered, not obeyed'
