"""The MIMO Random run converter: a run folder in the control service's
layout becomes a `.hforge` run file and reads back exactly; a cross-spectral
phase survives; a run that did not finish converts what it wrote; a folder
that is not a run is refused. With `HERTZFORGE_MIMO_RUN_DIR` naming a folder
the control service wrote, that folder is converted and checked as well."""

from __future__ import annotations

import json
import os

import numpy as np
import pytest

from hertzforge.io import mimo_run_convert as mrc

LINES, N, ND = 9, 2, 2
BAND = (2, 8)


def _payload(arrays):
    """The controller's payload: little-endian float64, complex as (re, im)."""
    out, infos = bytearray(), []
    for name, kind, shape, packed, values in arrays:
        start = len(out)
        flat = np.asarray(values).ravel()
        if kind == 'c64':
            pairs = np.empty(flat.size * 2)
            pairs[0::2], pairs[1::2] = flat.real, flat.imag
            out += pairs.astype('<f8').tobytes()
        else:
            out += flat.astype('<f8').tobytes()
        infos.append({'name': name, 'kind': kind, 'shape': list(shape), 'packed': packed,
                      'offset_bytes': start, 'len_bytes': len(out) - start})
    return bytes(out), infos


def _record(folder, name, side, arrays):
    payload, infos = _payload(arrays)
    with open(os.path.join(folder, f'{name}.bin'), 'wb') as f:
        f.write(payload)
    with open(os.path.join(folder, f'{name}.json'), 'w', encoding='utf-8') as f:
        json.dump({**side, 'arrays': infos}, f)


def _hermitian(rng, phase_deg=None):
    """A packed positive definite stack, with a known phase on the cross term."""
    full = np.zeros((LINES, N, N), dtype=complex)
    for k in range(*BAND):
        a = rng.normal(size=(N, N)) + 1j * rng.normal(size=(N, N))
        full[k] = a @ a.conj().T + np.eye(N)
        if phase_deg is not None:
            mag = 0.5 * np.sqrt(full[k, 0, 0].real * full[k, 1, 1].real)
            full[k, 0, 1] = mag * np.exp(1j * np.deg2rad(phase_deg))
            full[k, 1, 0] = np.conj(full[k, 0, 1])
    iu = np.triu_indices(N)
    return full[:, iu[0], iu[1]]


def _write_run(folder, rng):
    ref = _hermitian(rng, phase_deg=60.0)
    tol = rng.uniform(size=(4, LINES, N))
    _record(folder, 'reference',
            {'df_hz': 2.0, 'band': list(BAND), 'controls': [1, 2], 'space': 'physical'},
            [('sdm', 'c64', (LINES, N, N), True, ref)]
            + [(k, 'f64', (LINES, N), False, tol[i]) for i, k in
               enumerate(('alarm_upper', 'alarm_lower', 'abort_upper', 'abort_lower'))])
    frf = rng.normal(size=(LINES, N, ND)) + 1j * rng.normal(size=(LINES, N, ND))
    _record(folder, 'identification',
            {'format': mrc.IDENT_FORMAT, 'setup_hash': '0123456789abcdef',
             'drive_vrms': [0.05, 0.06], 'level_db': -20.2, 'frames': 64, 'steps': 3,
             'lag_frames': 0},
            [('frf', 'c64', frf.shape, False, frf),
             ('g_cc', 'c64', (LINES, N, N), True, _hermitian(rng)),
             ('g_rd', 'c64', frf.shape, False, frf * 2),
             ('g_dd', 'c64', (LINES, ND, ND), True, _hermitian(rng)),
             ('noise', 'c64', (LINES, N, N), True, _hermitian(rng)),
             ('coherence', 'f64', (LINES, N), False, rng.uniform(size=(LINES, N))),
             ('condition', 'f64', (LINES,), False, rng.uniform(1, 9, size=LINES))])
    loops = rng.normal(size=(3, LINES * (2 * N + ND)))
    loops.astype('<f8').tofile(os.path.join(folder, 'loops_diagonal.bin'))
    with open(os.path.join(folder, 'loops_diagonal.json'), 'w', encoding='utf-8') as f:
        json.dump({'lines': LINES, 'controls': N, 'drives': ND, 'loops': 3,
                   'loop_index': [1, 2, 3]}, f)
    with open(os.path.join(folder, 'loops_summary.jsonl'), 'w', encoding='utf-8') as f:
        f.writelines(json.dumps({'loop_index': k, 'level_db': -6.0 + 3 * (k - 1),
                                 'state': 'engaged', 'error_db': [0.3, 0.2],
                                 'drive_rms': [0.4, 0.5], 'guards': {}, 'timing': {},
                                 'law_misses': 0}) + '\n' for k in (1, 2, 3))
    snap = (_hermitian(rng, phase_deg=-30.0), _hermitian(rng))
    _record(folder, 'sdm_000003', {'loop_index': 3, 'why': 'first loop at full level'},
            [('response_full_level', 'c64', (LINES, N, N), True, snap[0]),
             ('drive_full_level', 'c64', (LINES, ND, ND), True, snap[1])])
    with open(os.path.join(folder, 'law.json'), 'w', encoding='utf-8') as f:
        json.dump({'id': 'pinv_closed', 'name': 'pseudo-inverse, closed loop',
                   'version': '1.0.0', 'sha256': '', 'builtin': True,
                   'params': [{'key': 'convergence', 'kind': 'number', 'value': 0.5}],
                   'file': None, 'catalog_path': None,
                   'parameters': ['convergence=Number(0.5)']}, f)
    calls = []
    with open(os.path.join(folder, 'law_calls.bin'), 'wb') as b, \
            open(os.path.join(folder, 'law_calls.jsonl'), 'w', encoding='utf-8') as j:
        for k in range(2):
            payload, infos = _payload([('response', 'c64', (LINES, N, N), True,
                                        _hermitian(rng)),
                                       ('out_drive', 'c64', (LINES, ND, ND), True,
                                        _hermitian(rng))])
            entry = {'format': mrc.LAW_CALLS_FORMAT, 'seq': k, 'call': 'seed' if k == 0
                     else 'iterate', 'loop_index': k, 'arrays': infos, 'carried': [],
                     'present': ['response'], 'outcome': 'ok',
                     'offset_bytes': b.tell(), 'len_bytes': len(payload)}
            b.write(payload)
            j.write(json.dumps(entry) + '\n')
            calls.append((entry, payload))
    with open(os.path.join(folder, 'report.json'), 'w', encoding='utf-8') as f:
        json.dump({'final_state': 'stopped', 'loops': 3}, f)
    with open(os.path.join(folder, 'capture.json'), 'w', encoding='utf-8') as f:
        json.dump({'run_config_json': json.dumps({'source': 'SIM', 'setup_hash':
                                                  '0123456789abcdef'}),
                   'start_unix_secs': 1793750000, 'complete': True}, f)
    return {'ref': ref, 'tol': tol, 'frf': frf, 'loops': loops, 'snap': snap,
            'calls': calls}


def test_a_run_folder_converts_and_reads_back_exactly(tmp_path):
    rng = np.random.default_rng(7)
    folder = tmp_path / 'mimo_random_1793750000_00000001'
    folder.mkdir()
    want = _write_run(str(folder), rng)
    out = mrc.convert(str(folder), str(tmp_path / 'run.hforge'))
    run = mrc.read(out)
    assert run['attrs']['test'] == 'mimo_random'
    assert bool(run['attrs']['complete'])
    assert run['attrs']['source'] == 'SIM'
    assert run['attrs']['setup_hash'] == '0123456789abcdef'
    ref = run['reference']
    np.testing.assert_array_equal(ref['sdm'], want['ref'])
    assert ref['sdm'].dtype == np.complex128
    for i, k in enumerate(('alarm_upper', 'alarm_lower', 'abort_upper', 'abort_lower')):
        np.testing.assert_array_equal(ref[k], want['tol'][i])
    assert list(ref['attrs']['band']) == list(BAND)
    ident = run['identification']
    np.testing.assert_array_equal(ident['frf'], want['frf'])
    np.testing.assert_array_equal(ident['g_rd'], want['frf'] * 2)
    assert ident['attrs']['setup_hash'] == '0123456789abcdef'
    diag = run['loops']['diagonal']
    np.testing.assert_array_equal(diag['loop_index'], [1, 2, 3])
    a = LINES * N
    np.testing.assert_array_equal(diag['control_psd'],
                                  want['loops'][:, :a].reshape(3, LINES, N))
    np.testing.assert_array_equal(diag['drive_psd'],
                                  want['loops'][:, 2 * a:].reshape(3, LINES, ND))
    summary = run['loops']['summary']
    np.testing.assert_array_equal(summary['level_db'], [-6.0, -3.0, 0.0])
    sdm = run['loops']['sdm']['000003']
    np.testing.assert_array_equal(sdm['response_full_level'], want['snap'][0])
    assert sdm['attrs']['why'] == 'first loop at full level'
    law = run['law']
    assert law['attrs']['id'] == 'pinv_closed' and bool(law['attrs']['builtin'])
    assert json.loads(law['params'][0])['value'] == 0.5
    assert len(law['calls']['index']) == 2
    for k, (_, payload) in enumerate(want['calls']):
        arrays = mrc.law_call_arrays(run, k)
        side_payload, infos = payload, json.loads(law['calls']['index'][k])['arrays']
        got = mrc._array(side_payload, infos[1])
        np.testing.assert_array_equal(arrays['out_drive'], got)
    assert json.loads(run['report'][0])['loops'] == 3


def test_a_cross_spectral_phase_survives_the_conversion(tmp_path):
    rng = np.random.default_rng(11)
    folder = tmp_path / 'run'
    folder.mkdir()
    _write_run(str(folder), rng)
    run = mrc.read(mrc.convert(str(folder), str(tmp_path / 'run.hforge')))
    full = mrc.unpack(run['reference']['sdm'])
    k = BAND[0]
    assert np.rad2deg(np.angle(full[k, 0, 1])) == pytest.approx(60.0, abs=1e-9)
    assert np.rad2deg(np.angle(full[k, 1, 0])) == pytest.approx(-60.0, abs=1e-9)
    np.testing.assert_allclose(full, np.conj(np.swapaxes(full, 1, 2)))
    snap = mrc.unpack(run['loops']['sdm']['000003']['response_full_level'])
    assert np.rad2deg(np.angle(snap[k, 0, 1])) == pytest.approx(-30.0, abs=1e-9)
    assert run['reference']['sdm'].dtype == np.complex128


def test_a_run_that_did_not_finish_converts_what_it_wrote(tmp_path):
    rng = np.random.default_rng(3)
    folder = tmp_path / 'run'
    folder.mkdir()
    _write_run(str(folder), rng)
    os.remove(folder / 'report.json')
    # Two and a half loops of diagonals on disk; a summary line cut short.
    per = LINES * (2 * N + ND) * 8
    with open(folder / 'loops_diagonal.bin', 'r+b') as f:
        f.truncate(per * 2 + per // 2)
    with open(folder / 'loops_summary.jsonl', 'a', encoding='utf-8') as f:
        f.write('{"loop_index": 4, "lev')
    run = mrc.read(mrc.convert(str(folder), str(tmp_path / 'cut.hforge')))
    assert not bool(run['attrs']['complete'])
    assert run['loops']['diagonal']['control_psd'].shape == (2, LINES, N)
    assert len(run['loops']['summary']['loop_index']) == 3
    assert 'report' not in run


def test_a_folder_that_is_not_a_run_is_refused_with_what_to_do(tmp_path):
    with pytest.raises(mrc.RunFolderError, match='choose the run folder'):
        mrc.convert(str(tmp_path), str(tmp_path / 'x.hforge'))
    assert not os.path.exists(tmp_path / 'x.hforge')


def test_a_file_from_a_newer_schema_is_refused(tmp_path):
    import h5py
    p = tmp_path / 'new.hforge'
    with h5py.File(p, 'w') as f:
        f.attrs['hertzforge_schema'] = mrc.HFORGE_SCHEMA + 1
        f.create_group('run')
    with pytest.raises(mrc.RunFolderError, match='newer Hertzforge'):
        mrc.read(str(p))


@pytest.mark.skipif('HERTZFORGE_MIMO_RUN_DIR' not in os.environ,
                    reason='set HERTZFORGE_MIMO_RUN_DIR to a run folder the control service wrote')
def test_a_run_folder_the_control_service_wrote_converts(tmp_path):
    folder = os.environ['HERTZFORGE_MIMO_RUN_DIR']
    run = mrc.read(mrc.convert(folder, str(tmp_path / 'service.hforge')))
    ref = mrc.unpack(run['reference']['sdm'])
    np.testing.assert_allclose(ref, np.conj(np.swapaxes(ref, 1, 2)))
    lo, hi = run['reference']['attrs']['band']
    assert np.all(np.real(np.diagonal(ref[lo:hi], axis1=1, axis2=2)) > 0)
    diag = run['loops']['diagonal']
    lines, n = ref.shape[0], ref.shape[1]
    assert diag['control_psd'].shape[1:] == (lines, n)
    # Every loop has a summary; a loop not judged (the settle loop) has no diagonal.
    assert set(diag['loop_index']) <= set(run['loops']['summary']['loop_index'])
    assert len(diag['loop_index']) >= 1
    ident = run['identification']
    assert ident['frf'].shape[0] == lines
    with open(os.path.join(folder, 'law_calls.jsonl'), encoding='utf-8') as f:
        assert len(run['law']['calls']['index']) == sum(1 for line in f if line.strip())
    first = mrc.law_call_arrays(run, 0)
    np.testing.assert_array_equal(first['reference'], run['reference']['sdm'])
