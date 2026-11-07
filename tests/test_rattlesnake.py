"""The Rattlesnake importer against a real controller run, the plate's."""

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge


@pytest.fixture(scope='module')
def run():
    return fixture_path('plate', 'random.nc4')


@pytest.fixture(scope='module')
def imported(run):
    return hertzforge.import_file(run)


def test_channel_table(imported, run):
    """Every channel's DOF is the file's own node and direction, read
    here straight from its `channels` group."""
    import h5py

    table = imported['channel_table']
    assert isinstance(table, hertzforge.ChannelTable)
    assert table.num_channels == 12
    assert set(table['unit']) == {'m/s^2', 'N'}
    with h5py.File(run, 'r') as source:
        nodes = [n.decode() for n in source['channels/node_number'][()]]
        directions = [d.decode()
                      for d in source['channels/node_direction'][()]]
    assert table.dof_strings() == [n + d for n, d in zip(nodes, directions,
                                                          strict=True)]


def test_time_data_units_automatic(imported):
    time = imported['time_data']
    assert isinstance(time, hertzforge.TimeHistory)
    assert time.num_records == 12
    # m/s^2 and N are already SI: ordinate must equal the file contents
    dims = set(time.ordinate_dim)
    assert dims == {'acceleration', 'force'}
    dt = time.abscissa[1] - time.abscissa[0]
    assert dt == pytest.approx(1 / 4096)


def test_specification(imported):
    """This run wrote its off-diagonal terms as placeholders, so it imports
    as autospectra alone: one record for each of its eight control channels."""
    spec = imported['Random_specification']
    assert isinstance(spec, hertzforge.Specification)
    assert spec.num_records == 8
    assert list(spec.response_dof) == list(spec.reference_dof)
    assert all(d == 'acceleration**2/frequency' for d in spec.ordinate_dim)
    assert not np.iscomplexobj(spec.ordinate)


def test_mixed_dimension_display(imported):
    time = imported['time_data']
    disp = time.display_ordinate(hertzforge.IN_LBF_S)
    accel_row = time.ordinate_dim.index('acceleration')
    force_row = time.ordinate_dim.index('force')
    assert np.allclose(disp[accel_row], time.ordinate[accel_row] / 0.0254)
    assert np.allclose(disp[force_row],
                       time.ordinate[force_row] / 4.4482216152605)
