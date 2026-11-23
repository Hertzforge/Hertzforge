"""SignalCalc's MATLAB time history export: the layout read, the order
of its channels, its refusals and notes, and the round trip.

The fixtures are written here in the export's own layout (HDF5 behind
MATLAB's version 7.3 header, one `INP_k_Data` column per channel or
one chunked, compressed `TimeData` matrix, NUL-padded character
matrices), with synthetic numbers.
"""

from __future__ import annotations

import warnings

import h5py
import numpy as np
import pytest
from scipy.io import savemat

import hertzforge
from hertzforge.core.data import TimeHistory
from hertzforge.io import export_file, import_file, signalcalc_mat
from hertzforge.io.notes import ImportNote
from hertzforge.units import SYSTEMS

STEP = 1 / 20480
SAMPLES = 64


def _chars(f, name, rows, width=15):
    codes = np.zeros((len(rows), width), dtype=np.uint16)
    for k, row in enumerate(rows):
        codes[k, :len(row)] = [ord(c) for c in row]
    dataset = f.create_dataset(name, data=codes.T)
    dataset.attrs['MATLAB_class'] = np.bytes_(b'char')
    dataset.attrs['MATLAB_int_decode'] = np.int32(2)


def _doubles(f, name, values):
    dataset = f.create_dataset(name, data=np.asarray(values, dtype=float).reshape(1, -1))
    dataset.attrs['MATLAB_class'] = np.bytes_(b'double')


def _export(path, names=('A6501- Z', 'Current', 'Voltage'),
            units=('g', 'ampere', 'V'), data=None, step=STEP, inputs=None,
            sensitivity=None, skip=(), columns=None, matrix=False):
    """An export in the layout SignalCalc writes. `data` is one row of
    samples per channel (row k + 1 is `INP_<k + 1>_Data`); `skip` leaves
    variables out; `columns` replaces a channel's dataset as written.
    `matrix` writes the samples as one `TimeData` instead, chunked across
    every channel and compressed as a real export's is, and `'both'`
    writes `TimeData` and the `INP_k_Data` too."""
    if data is None:
        data = np.vstack([np.full(SAMPLES, k + 1.0) + np.sin(np.arange(SAMPLES) * (k + 1))
                          for k in range(len(names))])
    inputs = list(range(1, len(names) + 1)) if inputs is None else inputs
    sensitivity = [25.0] * len(names) if sensitivity is None else sensitivity
    with h5py.File(path, 'w', userblock_size=512) as f:
        if matrix:
            # MATLAB's samples x channels is (channels, samples) in HDF5
            stored = np.asarray(data, dtype=float)
            dataset = f.create_dataset('TimeData', data=stored, compression='gzip',
                                       chunks=(stored.shape[0], 7))
            dataset.attrs['MATLAB_class'] = np.bytes_(b'double')
        for k, row in enumerate(data if matrix in (False, 'both') else ()):
            name = f'INP_{k + 1}_Data'
            if name in skip:
                continue
            if columns and name in columns:
                f.create_dataset(name, data=columns[name])
            else:
                _doubles(f, name, row)
        if 'ChanName' not in skip:
            _chars(f, 'ChanName', list(names))
        if 'Unit' not in skip:
            _chars(f, 'Unit', list(units), width=6)
        if 'hDelta' not in skip:
            _doubles(f, 'hDelta', np.atleast_1d(step))
        _doubles(f, 'EUType', [2] * len(names))
        _doubles(f, 'InputChanNums', inputs)
        _doubles(f, 'Sensitivity', sensitivity)
    with open(path, 'r+b') as fh:
        fh.write(b'MATLAB 7.3 MAT-file, Platform: PCWIN64, HDF5 schema 1.00 .'.ljust(116, b' ')
                 + bytes(8) + b'\x00\x02IM')
    return str(path)


def _read(path, **kwargs):
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter('always')
        found = import_file(path, **kwargs)
    notes = [str(w.message) for w in caught if issubclass(w.category, ImportNote)]
    return found['time_data'], found['channel_table'], notes


def test_the_layout_reads_as_one_history_in_si(tmp_path):
    data = np.vstack([np.linspace(-1, 1, SAMPLES), np.full(SAMPLES, 0.5) + np.arange(SAMPLES),
                      np.cos(np.arange(SAMPLES))])
    history, table, notes = _read(_export(tmp_path / 'run.mat', data=data))
    assert history.ordinate_dim == ['acceleration', 'current', 'voltage']
    assert history.ordinate_unit == ['g', 'ampere', 'V']
    assert np.allclose(history.ordinate[0], data[0] * 9.80665)
    assert np.allclose(history.ordinate[1:], data[1:])
    assert history.sample_rate == pytest.approx(20480.0)
    assert history.abscissa[0] == 0.0
    assert list(history.response_dof) == ['6501Z+', 'Current', 'Voltage']
    assert history.block is None
    assert [str(n) for n in table['node']] == ['6501', '', '']
    assert list(table['direction']) == ['Z+', '', '']
    assert list(table['channel_type']) == ['acceleration', 'current', 'voltage']
    assert list(table['unit']) == ['g', 'ampere', 'V']
    assert notes == []


def test_a_name_that_is_a_dof_becomes_the_records_dof(tmp_path):
    history, table, _ = _read(_export(tmp_path / 'run.mat', names=('101X+', 'Current', '7Z-')))
    assert list(history.response_dof) == ['101X+', 'Current', '7Z-']
    assert [str(n) for n in table['node']] == ['101', '', '7']
    assert list(table['direction']) == ['X+', '', 'Z-']


@pytest.mark.parametrize('name, dof', [
    ('A6002- X     ', '6002X+'), ('6002X', '6002X+'), ('A6002 Y-', '6002Y-'),
    ('acc6002-z', '6002Z+'), ('A6002-RX', '6002RX+'), ('A06002- Z', '6002Z+'),
    ('Drive Limit', 'Drive Limit'), ('Level Check', 'Level Check'),
])
def test_a_name_reads_as_a_node_and_an_axis_or_stands_as_the_channels_dof(
        tmp_path, name, dof):
    """The hyphen after the node is a separator, not a sign; a name that
    is not a node and an axis is the channel's DOF as it stands, and the
    channel table's comment keeps the name either way."""
    history, table, notes = _read(_export(tmp_path / 'run.mat', names=(name, 'Current', 'Voltage')))
    assert history.response_dof[0] == dof
    assert table['comment'][0] == f'{name.strip()}, input 1'
    assert notes == []


@pytest.mark.parametrize('name', ['6002 Accel', 'X'])
def test_a_name_that_cannot_be_a_dof_leaves_it_empty_and_says_so(tmp_path, name):
    history, _, notes = _read(_export(tmp_path / 'run.mat', names=(name, 'Current', 'Voltage')))
    assert list(history.response_dof) == ['', 'Current', 'Voltage']
    assert notes == [(f"run.mat: {name!r} is not a node and an axis, and a DOF cannot hold it "
                      'as a name (a name does not start with a digit or read as a direction); '
                      'its DOF is left empty')]


def test_two_channels_that_would_be_one_are_refused(tmp_path):
    with pytest.raises(ValueError, match=r"run\.mat: rows 1 and 2 of ChanName, 'A6002- X' and "
                                         r"'6002X\+', are both 6002X\+ in g: one channel twice"):
        import_file(_export(tmp_path / 'run.mat', names=('A6002- X', '6002X+', 'Voltage'),
                            units=('g', 'g', 'V')))
    # one DOF in two quantities is two channels, as a drive point's are
    history, _, _ = _read(_export(tmp_path / 'b.mat', names=('A6002- X', '6002X+', 'Voltage'),
                                  units=('g', 'V', 'V')))
    assert list(history.response_dof) == ['6002X+', '6002X+', 'Voltage']


def test_channels_named_alike_in_one_unit_stay_apart_in_one_column(tmp_path):
    """Two channels with no node in one unit are two channels: each its own
    PSD and its own row of the tree, every record in one column, since
    the export is one recording."""
    from hertzforge.service.tree import grid

    rng = np.random.default_rng(3)
    data = rng.standard_normal((4, 4096))
    path = _export(tmp_path / 'run.mat', data=data,
                   names=('A6002- X', 'Drive Limit', 'Level Check', 'Current'),
                   units=('g', 'V', 'V', 'ampere'), inputs=[1, 2, 3, 4],
                   sensitivity=[1.0] * 4)
    history, _, _ = _read(path)
    view = grid(history)
    assert view['columns'] == ['']
    assert [row['label'] for row in view['rows']] == ['6002X+', 'Drive Limit', 'Level Check',
                                                      'Current']
    psds = history.compute_psds()
    assert list(psds.response_dof) == ['6002X+', 'Drive Limit', 'Level Check', 'Current']
    cpsds = history.compute_cpsds()
    pairs = set(zip(cpsds.response_dof, cpsds.reference_dof, strict=True))
    assert ('Drive Limit', 'Level Check') in pairs and len(pairs) == 16


def test_each_file_imported_is_a_group_named_for_it(tmp_path):
    """Several exports imported into one project are told apart in the
    tree by their groups, each named for its file, as the service lists
    them; a copy a browser sent is named as the person's file was."""
    from hertzforge.service.projects import OpenProject, summary

    project = hertzforge.Project()
    with warnings.catch_warnings():
        warnings.simplefilter('ignore', ImportNote)
        first = project.import_file(_export(tmp_path / 'run1.mat'))
        second = project.import_file(_export(tmp_path / 'run2.mat'))
        folder = tmp_path / 'hertzforge-received-1' / 'x'
        folder.mkdir(parents=True)
        third = project.import_file(_export(folder / 'shaker run.mat'), received=True)
    assert [(group['name'], group['members']) for group in project.object_groups] == [
        ('run1.mat', first), ('run2.mat', second), ('shaker run.mat', third)]
    listed = {entry['name']: entry['groups'] for entry in summary(OpenProject(project))['objects']}
    assert {name: listed[name] for name in first + second} == {
        **{name: ['run1.mat'] for name in first}, **{name: ['run2.mat'] for name in second}}


def test_inp_k_is_row_k_whatever_the_input_numbers(tmp_path):
    """The analyzer's input numbers follow its wiring: rows 1 to 30 of a
    real export were inputs 65 to 94. INP_k is still row k, and the
    input number is kept with the channel's name."""
    data = np.vstack([np.full(SAMPLES, 10.0), np.full(SAMPLES, 20.0) + np.arange(SAMPLES),
                      np.full(SAMPLES, 30.0) + np.arange(SAMPLES)])
    history, table, _ = _read(_export(tmp_path / 'run.mat', data=data, inputs=[3, 1, 2]))
    assert history.ordinate[1, 0] == pytest.approx(20.0)
    assert history.ordinate[2, 0] == pytest.approx(30.0)
    assert history.ordinate_dim[1] == 'current'
    assert list(table['comment']) == ['A6501- Z, input 3', 'Current, input 1',
                                      'Voltage, input 2']
    assert [float(s) for s in table['sensitivity']] == [25.0, 25.0, 25.0]


def test_a_window_and_a_pick_of_channels_read_only_that_part(tmp_path):
    data = np.vstack([np.arange(SAMPLES, dtype=float) * (k + 1) for k in range(3)])
    path = _export(tmp_path / 'run.mat', data=data)
    history, table, _ = _read(path, start=10 * STEP, stop=20 * STEP,
                              channels=['Voltage', 0])
    assert history.ordinate.shape == (2, 11)
    assert history.abscissa[0] == pytest.approx(10 * STEP)
    assert np.allclose(history.ordinate[0], data[2, 10:21])
    assert np.allclose(history.ordinate[1], data[0, 10:21] * 9.80665)
    assert history.comment[0].startswith(f'{10 * STEP:g} to {20 * STEP:g} s of INP_3_Data')
    assert table.num_channels == 3


def test_a_pick_of_channels_names_what_it_cannot_find(tmp_path):
    path = _export(tmp_path / 'run.mat')
    with pytest.raises(ValueError, match=r"run\.mat: channel 'Force' is not in the table"):
        import_file(path, channels=['Force'])
    with pytest.raises(ValueError, match='an import needs at least one channel'):
        import_file(path, channels=[])


def test_a_row_vector_reads_like_a_column(tmp_path):
    column = np.arange(SAMPLES, dtype=float).reshape(-1, 1)
    history, _, _ = _read(_export(tmp_path / 'run.mat',
                                  columns={'INP_2_Data': column}))
    assert np.allclose(history.ordinate[1], column.ravel())


@pytest.mark.parametrize('step, said', [
    (0.0, 'hDelta is 0.0'), (-STEP, 'hDelta is -'), (np.nan, 'hDelta is nan'),
    ([STEP, STEP], 'hDelta holds 2 values'),
], ids=['zero', 'negative', 'nan', 'two'])
def test_a_time_step_that_is_not_one_positive_number_is_refused(tmp_path, step, said):
    with pytest.raises(ValueError, match=f'run.mat: {said}'):
        import_file(_export(tmp_path / 'run.mat', step=step))


def test_no_time_step_is_refused(tmp_path):
    path = _export(tmp_path / 'run.mat', skip=('hDelta',))
    with pytest.raises(ValueError, match='no hDelta, the time step'):
        signalcalc_mat.load(path)


def test_channels_of_different_lengths_are_refused(tmp_path):
    path = _export(tmp_path / 'run.mat', columns={'INP_3_Data': np.zeros((1, SAMPLES - 1))})
    with pytest.raises(ValueError, match=f'INP_3_Data holds {SAMPLES - 1} samples where '
                                         f'INP_1_Data holds {SAMPLES}'):
        import_file(path)


def test_a_channel_that_is_a_matrix_is_refused(tmp_path):
    path = _export(tmp_path / 'run.mat', columns={'INP_2_Data': np.zeros((2, SAMPLES))})
    with pytest.raises(ValueError, match=r'INP_2_Data is a \(2, 64\) matrix'):
        import_file(path)


def test_a_missing_channel_and_an_extra_one_are_refused(tmp_path):
    with pytest.raises(ValueError, match='INP_2_Data is missing; ChanName names 3'):
        import_file(_export(tmp_path / 'a.mat', skip=('INP_2_Data',)))
    four = np.vstack([np.arange(SAMPLES, dtype=float)] * 4)
    with pytest.raises(ValueError, match='INP_4_Data has no row in ChanName'):
        import_file(_export(tmp_path / 'b.mat', data=four, inputs=[1, 2, 3],
                            sensitivity=[1.0] * 3, names=('a', 'b', 'c'),
                            units=('g', 'g', 'g')))


def test_counts_that_differ_from_the_names_are_refused(tmp_path):
    with pytest.raises(ValueError, match='Unit holds 2 units for 3 channels'):
        import_file(_export(tmp_path / 'a.mat', units=('g', 'V')))
    with pytest.raises(ValueError, match='Sensitivity holds 2 values for 3 channels'):
        import_file(_export(tmp_path / 'b.mat', sensitivity=[25.0, 25.0]))


def test_no_names_is_refused(tmp_path):
    path = _export(tmp_path / 'run.mat', names=(), units=(), data=np.zeros((1, SAMPLES)),
                   inputs=[], sensitivity=[])
    assert signalcalc_mat.sniff(path)
    with pytest.raises(ValueError, match='ChanName names no channels'):
        import_file(path)


def test_a_window_with_under_two_samples_is_refused(tmp_path):
    path = _export(tmp_path / 'run.mat')
    with pytest.raises(ValueError, match='the window holds fewer than two of its 64'):
        import_file(path, start=1.0, stop=2.0)


def test_unreadable_and_missing_units_are_left_undeclared_and_noted(tmp_path):
    history, _, notes = _read(_export(tmp_path / 'run.mat', units=('EU', '', 'V')))
    assert history.ordinate_unit[:2] == [None, None]
    assert history.ordinate_dim[2] == 'voltage'
    assert any("6501Z+: the unit 'EU'" in note and 'undeclared' in note for note in notes)
    assert any('Current: the file states no unit' in note for note in notes)


def test_a_dead_channel_and_a_missing_sample_are_noted(tmp_path):
    data = np.vstack([np.zeros(SAMPLES), np.arange(SAMPLES, dtype=float),
                      np.full(SAMPLES, 3.0)])
    data[1, 5] = np.nan
    history, _, notes = _read(_export(tmp_path / 'run.mat', data=data))
    assert np.isnan(history.ordinate[1, 5])
    said = ' '.join(notes)
    assert '6501Z+ is constant at 0 g' in said
    assert 'Voltage is constant at 3 V' in said
    assert 'Current' in said and 'missing' in said


def test_the_sniff_takes_only_this_layout_and_never_raises(tmp_path):
    assert signalcalc_mat.sniff(_export(tmp_path / 'run.mat'))
    assert not signalcalc_mat.sniff(_export(tmp_path / 'run.h5'))
    own = tmp_path / 'own.mat'
    savemat(own, {'ChanName': 'x', 'hDelta': 1.0})
    assert not signalcalc_mat.sniff(own)
    other = tmp_path / 'other.mat'
    with h5py.File(other, 'w') as f:
        f.create_dataset('signal', data=np.ones(4))
    assert not signalcalc_mat.sniff(other)
    broken = tmp_path / 'broken.mat'
    broken.write_bytes(b'\x89HDF\r\n\x1a\n' + bytes(40))
    assert not signalcalc_mat.sniff(broken)
    assert hertzforge.io.known_format(_export(tmp_path / 'b.mat')) == \
        'SignalCalc time history export (.mat)'


def test_a_hertzforge_mat_is_still_the_project_layouts(tmp_path):
    history = TimeHistory(np.arange(8) * 0.5, np.ones((1, 8)), response_dof=['1X+'])
    path = tmp_path / 'own.mat'
    export_file(history, path)
    assert not signalcalc_mat.sniff(path)
    assert isinstance(import_file(path), TimeHistory)


def _history():
    t = np.arange(SAMPLES) * STEP
    history = TimeHistory(
        t, np.vstack([np.sin(t * 3000), 0.2 * np.cos(t * 900), np.linspace(0, 1, SAMPLES)]),
        response_dof=['Drive Limit', '101X+', 'gauge'])
    history.define_units(['g', 'mA', None])
    return history


def test_the_round_trip_in_a_display_system_keeps_values_units_and_names(tmp_path):
    history = _history()
    path = tmp_path / 'out.mat'
    export_file(history, path, format='signalcalc_mat',
                unit_system=SYSTEMS['in-slinch-lbf-s (g)'])
    with h5py.File(path, 'r') as f:
        units = [''.join(map(chr, row)).strip('\x00') for row in f['Unit'][()].T]
    assert units == ['g', 'A', '']
    back, _, notes = _read(path)
    assert np.allclose(back.ordinate, history.ordinate)
    assert back.ordinate_dim == ['acceleration', 'current', history.ordinate_dim[2]]
    assert back.ordinate_unit[2] is None
    assert list(back.response_dof) == ['Drive Limit', '101X+', 'gauge']
    assert back.sample_rate == pytest.approx(1 / STEP)
    assert notes == [('out.mat: gauge: the file states no unit; left undeclared, '
                      "holding the file's numbers")]


def test_the_round_trip_without_a_system_is_in_si(tmp_path):
    history = _history()
    path = tmp_path / 'out.mat'
    signalcalc_mat.save(history, path)
    with h5py.File(path, 'r') as f:
        units = [''.join(map(chr, row)).strip('\x00') for row in f['Unit'][()].T]
        first = f['INP_1_Data'][0, :]
    assert units == ['m/s**2', 'A', '']
    assert np.allclose(first, history.ordinate[0])
    back, _, _ = _read(path)
    assert np.allclose(back.ordinate, history.ordinate)


def test_an_unevenly_sampled_history_is_not_written(tmp_path):
    history = TimeHistory(np.array([0.0, 0.1, 0.3, 0.4]), np.ones((1, 4)), response_dof=['1X+'])
    with pytest.raises(ValueError, match='not evenly spaced|evenly'):
        signalcalc_mat.save(history, tmp_path / 'out.mat')


def test_the_matrix_layout_reads_as_the_vector_layout_does(tmp_path):
    """Column k of TimeData is row k of ChanName, as INP_k_Data is,
    whatever the input numbers."""
    data = np.vstack([np.linspace(-1, 1, SAMPLES), np.full(SAMPLES, 0.5) + np.arange(SAMPLES),
                      np.cos(np.arange(SAMPLES))])
    path = _export(tmp_path / 'run.mat', data=data, inputs=[3, 1, 2], matrix=True)
    assert signalcalc_mat.sniff(path)
    history, table, notes = _read(path)
    assert history.ordinate_dim == ['acceleration', 'current', 'voltage']
    assert np.allclose(history.ordinate[0], data[0] * 9.80665)
    assert np.allclose(history.ordinate[1:], data[1:])
    assert history.sample_rate == pytest.approx(20480.0)
    assert list(history.response_dof) == ['6501Z+', 'Current', 'Voltage']
    assert history.comment[1] == 'column 2 of TimeData of run.mat'
    assert list(table['comment']) == ['A6501- Z, input 3', 'Current, input 1',
                                      'Voltage, input 2']
    assert notes == []


def test_a_window_and_a_pick_from_the_matrix_span_its_blocks(tmp_path, monkeypatch):
    """TimeData is read a block of samples at a time; a block of 5
    samples puts the window's ends inside blocks and the pick out of row
    order."""
    from hertzforge.io import rattlesnake

    monkeypatch.setattr(rattlesnake, 'READ_SAMPLES', 5)
    data = np.vstack([np.arange(SAMPLES, dtype=float) * (k + 1) for k in range(3)])
    path = _export(tmp_path / 'run.mat', data=data, matrix=True)
    done = []
    found = signalcalc_mat.load(path, start=10 * STEP, stop=31 * STEP,
                                channels=['Voltage', 0],
                                progress=lambda n, total: done.append((n, total)))
    history = found['time_data']
    assert history.ordinate.shape == (2, 22)
    assert history.abscissa[0] == pytest.approx(10 * STEP)
    assert np.array_equal(history.ordinate[0], data[2, 10:32])
    assert np.allclose(history.ordinate[1], data[0, 10:32] * 9.80665)
    assert history.comment[0].startswith(f'{10 * STEP:g} to {31 * STEP:g} s of column 3')
    assert done[-1] == (22, 22)


def test_a_matrix_with_other_than_one_column_per_name_is_refused(tmp_path):
    four = np.vstack([np.arange(SAMPLES, dtype=float)] * 4)
    with pytest.raises(ValueError, match=r'run\.mat: TimeData is 64 x 4 \(samples x channels '
                                         r'in MATLAB\); ChanName names 3 channels'):
        import_file(_export(tmp_path / 'run.mat', data=four, matrix=True))
    # a matrix stored channels x samples in MATLAB is the other way round
    path = tmp_path / 'flip.mat'
    _export(path, matrix=True)
    with h5py.File(path, 'r+') as f:
        del f['TimeData']
        f.create_dataset('TimeData', data=np.zeros((SAMPLES, 3)))
    with pytest.raises(ValueError, match='TimeData is 3 x 64'):
        import_file(path)


def test_a_complex_matrix_is_refused(tmp_path):
    path = tmp_path / 'run.mat'
    _export(path, matrix=True)
    with h5py.File(path, 'r+') as f:
        del f['TimeData']
        f.create_dataset('TimeData', data=np.zeros((3, SAMPLES), dtype=complex))
    with pytest.raises(ValueError, match='TimeData holds complex128, not real numbers'):
        import_file(path)


def test_a_file_with_both_layouts_is_refused(tmp_path):
    path = _export(tmp_path / 'run.mat', matrix='both')
    with pytest.raises(ValueError, match='holds both TimeData and INP_1_Data'):
        import_file(path)
