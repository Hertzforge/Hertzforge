"""The converter from specification files to the controller's native
reference file: round trips through files the tests write, the sign of a
known phase, and the refusals."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import pathlib

import h5py
import numpy as np
import pytest
from scipy.io import savemat

from hertzforge.core.data import Specification
from hertzforge.io import native, reference_convert

ROOT = pathlib.Path(__file__).resolve().parents[1]


def _generator():
    spec = importlib.util.spec_from_file_location(
        'generate_mimo_reference', ROOT / 'testdata' / 'generate_mimo_reference.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GEN = _generator()


def _v5(tmp_path, variables=None, name='spec.mat'):
    path = tmp_path / name
    savemat(path, GEN.specification() if variables is None else variables)
    return path


def _v73(tmp_path, variables):
    """A MATLAB v7.3 file: HDF5 with every array's axes reversed and complex
    values as a real and imaginary compound."""
    path = tmp_path / 'spec73.mat'
    with h5py.File(path, 'w') as f:
        for key, value in variables.items():
            if key == 'dof':
                continue
            value = np.asarray(value).T
            if np.iscomplexobj(value):
                compound = np.dtype([('real', '<f8'), ('imag', '<f8')])
                data = np.empty(value.shape, dtype=compound)
                data['real'], data['imag'] = value.real, value.imag
                value = data
            f.create_dataset(key, data=value).attrs['MATLAB_class'] = b'double'
    return path


def test_a_v5_file_converts_with_the_documented_sign_and_limits(tmp_path):
    folder = tmp_path / 'ref'
    digest = reference_convert.from_mat(_v5(tmp_path), folder, df_hz=GEN.DF_HZ,
                                        lines=GEN.LINES)
    ref = reference_convert.read(folder)
    assert ref.labels == ['front', 'rear', 'top']
    assert ref.band == (4, 21)
    assert ref.sdm.shape == (41, 3, 3)
    # rear lags front by 30 degrees: -30 in the file's Bendat form, +30 held
    assert np.degrees(np.angle(ref.sdm[10, 0, 1])) == pytest.approx(30.0)
    assert np.degrees(np.angle(ref.sdm[10, 1, 0])) == pytest.approx(-30.0)
    assert abs(ref.sdm[10, 0, 1]) ** 2 / (0.01 * 0.02) == pytest.approx(0.5)
    assert not ref.sdm[3].any()
    assert np.isnan(ref.tolerances['alarm_upper'][4, 2])
    assert ref.tolerances['alarm_upper'][5, 2] == pytest.approx(0.08)
    assert np.isnan(ref.tolerances['abort_upper'][30]).all()
    assert ref.provenance['method'] == 'specification_mat'
    assert ref.provenance['conjugated_on_import'] is True
    binary = (folder / reference_convert.BINARY_FILE).read_bytes()
    assert digest == 'sha256:' + hashlib.sha256(binary).hexdigest()

    kept = tmp_path / 'kept'
    reference_convert.from_mat(_v5(tmp_path), kept, df_hz=GEN.DF_HZ, lines=GEN.LINES,
                               conjugate_on_import=False)
    ref = reference_convert.read(kept)
    assert np.degrees(np.angle(ref.sdm[10, 0, 1])) == pytest.approx(-30.0)
    assert ref.provenance['conjugated_on_import'] is False


def test_a_v73_file_converts_to_the_same_content(tmp_path):
    variables = GEN.specification()
    a = reference_convert.from_mat(_v5(tmp_path), tmp_path / 'a', df_hz=GEN.DF_HZ,
                                   lines=GEN.LINES)
    b = reference_convert.from_mat(_v73(tmp_path, variables), tmp_path / 'b',
                                   df_hz=GEN.DF_HZ, lines=GEN.LINES,
                                   labels=['front', 'rear', 'top'])
    assert a == b


def test_the_committed_fixture_is_what_the_generator_writes(tmp_path):
    digest = GEN.write(tmp_path / 'fresh')
    side = json.loads((GEN.OUT / reference_convert.SIDECAR_FILE).read_text())
    assert side['payload']['content_hash'] == digest
    assert reference_convert.read(GEN.OUT).labels == ['front', 'rear', 'top']


def test_a_hforge_specification_converts(tmp_path):
    f = np.arange(10.0, 60.0, 10.0)
    cross = 0.5 * np.exp(-1j * np.radians(40.0)) * np.ones(len(f))
    rows = np.array([np.ones(len(f)), cross, 4 * np.ones(len(f))], dtype=complex)
    nan = np.full(len(f), np.nan)
    spec = Specification(f, rows, response_dof=['101X+', '101X+', '102Y+'],
                         reference_dof=['101X+', '102Y+', '102Y+'],
                         abort_upper=np.array([4 * np.ones(len(f)), nan,
                                               16 * np.ones(len(f))]))
    path = tmp_path / 'spec.hforge'
    native.save(spec, path)
    folder = tmp_path / 'ref'
    reference_convert.from_hforge(path, folder, df_hz=10.0, lines=8, unit='g')
    ref = reference_convert.read(folder)
    assert ref.labels == ['101X+', '102Y+']
    assert ref.band == (1, 6)
    assert np.degrees(np.angle(ref.sdm[2, 0, 1])) == pytest.approx(40.0)
    assert ref.tolerances['abort_upper'][3, 1] == pytest.approx(16.0)
    assert ref.provenance['method'] == 'hforge'


def test_encode_and_decode_round_trip_with_transformations(tmp_path):
    lines, n = 6, 2
    sdm = np.zeros((lines, n, n), dtype=complex)
    sdm[1:5] = [[2.0, 0.5 + 0.5j], [0.5 - 0.5j, 3.0]]
    tol = {name: np.full((lines, n), np.nan) for name in reference_convert.TOLERANCES}
    response = reference_convert.Transformation(
        np.array([[[0.5, 0.5, 0, 0], [0, 0, 0.5, 0.5]]], dtype=complex), ['u', 'v'],
        ['a', 'b', 'c', 'd'])
    drive = reference_convert.Transformation(
        np.array([[[1, 0], [1, 0], [0, 1]]], dtype=complex), ['d1', 'd2', 'd3'],
        ['x', 'y'])
    ref = reference_convert.NativeReference(sdm, 2.5, (1, 5), ['u', 'v'], 'm/s^2', tol,
                                            {'method': 'direct'}, response, drive)
    reference_convert.write(ref, tmp_path / 'r')
    back = reference_convert.read(tmp_path / 'r')
    np.testing.assert_array_equal(back.sdm, sdm)
    np.testing.assert_array_equal(back.response_transformation.matrix, response.matrix)
    assert back.drive_transformation.row_labels == ['d1', 'd2', 'd3']
    side = json.loads((tmp_path / 'r' / reference_convert.SIDECAR_FILE).read_text())
    assert side['payload']['space'] == 'virtual'
    assert side['payload']['drive_transformation'] == [1, 3, 2]


def test_a_changed_binary_is_refused(tmp_path):
    folder = tmp_path / 'ref'
    reference_convert.from_mat(_v5(tmp_path), folder, df_hz=GEN.DF_HZ, lines=GEN.LINES)
    binary = folder / reference_convert.BINARY_FILE
    data = bytearray(binary.read_bytes())
    data[-3] ^= 0x40
    binary.write_bytes(bytes(data))
    with pytest.raises(ValueError, match='content hash'):
        reference_convert.read(folder)


def test_refusals_name_the_problem(tmp_path):
    variables = GEN.specification()
    off_grid = dict(variables, f=variables['f'] + 2.5)
    with pytest.raises(ValueError, match=r'line at 22.5 Hz \(entry 0\)'):
        reference_convert.from_mat(_v5(tmp_path, off_grid), tmp_path / 'a',
                                   df_hz=GEN.DF_HZ, lines=GEN.LINES)

    with pytest.raises(ValueError, match="'extra'"):
        reference_convert.from_mat(_v5(tmp_path, dict(variables, extra=np.ones(3))),
                                   tmp_path / 'b', df_hz=GEN.DF_HZ, lines=GEN.LINES)

    with pytest.raises(ValueError, match="no 'f'"):
        reference_convert.from_mat(_v5(tmp_path, {'cpsd': variables['cpsd']}),
                                   tmp_path / 'c', df_hz=GEN.DF_HZ, lines=GEN.LINES)

    # unit coherence with phases 0, 0 and 180 degrees cannot hold together
    bad = variables['cpsd'].copy()
    psd = np.array([0.01, 0.02, 0.04])
    one = np.sqrt(np.outer(psd, psd)) * np.array([[1, 1, -1], [1, 1, 1], [-1, 1, 1]])
    bad[:] = one[:, :, np.newaxis]
    with pytest.raises(ValueError, match='not positive semi-definite'):
        reference_convert.from_mat(_v5(tmp_path, dict(variables, cpsd=bad)),
                                   tmp_path / 'd', df_hz=GEN.DF_HZ, lines=GEN.LINES)

    # the same file converts when the repair is asked for, keeping the PSDs
    reference_convert.from_mat(_v5(tmp_path, dict(variables, cpsd=bad)), tmp_path / 'r',
                               df_hz=GEN.DF_HZ, lines=GEN.LINES, repair=True)
    ref = reference_convert.read(tmp_path / 'r')
    np.testing.assert_allclose(np.real(np.diagonal(ref.sdm[10])), psd, rtol=1e-12)
    values = np.linalg.eigvalsh(ref.sdm[10])
    assert values[0] >= -1e-12 * values[-1]
    assert ref.provenance['repair']['lines_repaired'] == 17
    assert ref.provenance['repair']['max_coherence_change'] > 0.1

    # 5 Hz lines on a 2.5 Hz grid leave zero lines inside the band
    with pytest.raises(ValueError, match='no level at 22.5 Hz'):
        reference_convert.from_mat(_v5(tmp_path), tmp_path / 'e', df_hz=2.5, lines=81)

    with pytest.raises(TypeError, match='not a random specification'):
        path = tmp_path / 'other.hforge'
        native.save(Specification.PLAIN(np.arange(1.0, 4.0), np.ones((1, 3)),
                                        response_dof=['1X+']), path)
        reference_convert.from_hforge(path, tmp_path / 'f', df_hz=1.0, lines=8)
