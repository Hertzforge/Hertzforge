"""`hertzforge.io.drive_convert`: every form in, the native drive file out,
read back exactly; each refusal by name; fixing the ends; the golden folder."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import pathlib
import struct

import numpy as np
import pytest

from hertzforge.io import drive_convert as dc

ROOT = pathlib.Path(__file__).resolve().parents[1]
GOLDEN = ROOT / 'schema' / 'golden' / 'drive' / 'basic'
GOLDEN_FIXED = ROOT / 'schema' / 'golden' / 'drive' / 'fixed'
N = 400
RATE = 1000.0


@pytest.fixture(autouse=True)
def _data_root(tmp_path, monkeypatch):
    """New drive folders go under a data folder of the test's own."""
    monkeypatch.setenv('HERTZFORGE_DATA_ROOT', str(tmp_path / 'data'))


def _values(n: int = N, channels: int = 2) -> np.ndarray:
    """`(n, channels)` values a decimal text and a double both hold
    exactly: multiples of 1/4096."""
    t = np.arange(n) / RATE
    out = np.stack([np.sin(2 * np.pi * (11 + 7 * c) * t) * (0.9 - 0.3 * c)
                    for c in range(channels)], axis=1)
    return np.round(out * 4096) / 4096


def _csv(path, values, *, time=True, units=('V', 'V'), names=('Drive A', 'Drive B'),
         delimiter=','):
    head = (['Time (s)'] if time else []) + [f'{n} ({u})' if u else n
                                             for n, u in zip(names, units)]
    lines = [delimiter.join(head)]
    for k, row in enumerate(values):
        cells = ([repr(k / RATE)] if time else []) + [repr(float(v)) for v in row]
        lines.append(delimiter.join(cells))
    path.write_text('\n'.join(lines) + '\n', encoding='utf-8')
    return path


def _round_trip(source, selections, expected, *, rate=RATE, **kwargs):
    done = dc.convert(source, selections, **kwargs)
    back = dc.read(done.folder)
    assert back.rate_hz == rate
    assert back.values.shape == expected.shape
    np.testing.assert_array_equal(back.values, expected)
    assert done.sidecar['sha256'] == hashlib.sha256(
        (pathlib.Path(done.folder) / 'drive.bin').read_bytes()).hexdigest()
    assert [c['unit'] for c in done.sidecar['channels']] == ['V'] * expected.shape[1]
    return done, back


def _keys(source):
    return [s.key for s in dc.inspect(source).signals]


# ---- every form ----------------------------------------------------------------

def test_csv_with_a_time_column_and_volts_and_millivolts(tmp_path):
    values = _values()
    path = _csv(tmp_path / 'drive.csv', values * [1, 1000], units=('V', 'mV'))
    found = dc.inspect(path)
    assert found.format == 'delimited'
    assert [(s.key, s.name, s.samples, s.rate_hz, s.unit) for s in found.signals] == [
        ('column:2', 'Drive A', N, RATE, 'V'), ('column:3', 'Drive B', N, RATE, 'mV')]
    done = dc.convert(path, ['column:3', 'column:2'])
    back = dc.read(done.folder)
    assert back.rate_hz == RATE and back.names == ['Drive B', 'Drive A']
    np.testing.assert_array_equal(back.values[:, 1], values[:, 0])
    np.testing.assert_allclose(back.values[:, 0], values[:, 1], rtol=1e-6, atol=1e-9)
    side = done.sidecar
    assert side['format'] == 'hertzforge-drive' and side['version'] == 1
    assert side['samples'] == N and side['fixes'] == []
    assert side['source']['format'] == 'delimited'
    assert side['source']['path'] == str(path)
    assert side['source']['sha256'] == hashlib.sha256(path.read_bytes()).hexdigest()
    assert side['source']['selections'] == [{'signal': 'column:3', 'name': 'Drive B'},
                                            {'signal': 'column:2', 'name': 'Drive A'}]
    assert os.path.dirname(done.folder) == os.path.join(os.environ['HERTZFORGE_DATA_ROOT'],
                                                        'drives')
    assert os.path.basename(done.folder) == 'drive'


def test_tsv_without_a_time_column_takes_the_rate_given(tmp_path):
    values = _values()
    path = _csv(tmp_path / 'drive.tsv', values, time=False, units=(None, None),
                delimiter='\t')
    found = dc.inspect(path)
    assert [s.rate_hz for s in found.signals] == [None, None]
    with pytest.raises(ValueError, match='states no rate'):
        dc.convert(path, ['column:1'])
    done, _back = _round_trip(path, ['column:1', 'column:2'], values, rate_hz=RATE)
    assert any('states no unit; taken as volts' in note for note in done.notes)


def test_mat_version_5(tmp_path):
    from scipy.io import savemat

    values = _values()
    savemat(tmp_path / 'drive.mat', {'drive': values, 'fs': RATE, 'label': 'test'})
    assert _keys(tmp_path / 'drive.mat') == ['variable:drive:column:1',
                                             'variable:drive:column:2']
    done, _b = _round_trip(tmp_path / 'drive.mat', _keys(tmp_path / 'drive.mat'), values)
    assert done.sidecar['source']['format'] == 'mat5'


def test_mat_version_7_3_in_matlab_orientation(tmp_path):
    import h5py

    values = _values()
    with h5py.File(tmp_path / 'drive.mat', 'w') as f:
        # MATLAB writes an N-by-2 array as a 2-by-N dataset
        f.create_dataset('drive', data=values.T).attrs['MATLAB_class'] = b'double'
        f.create_dataset('fs', data=np.array([[RATE]])).attrs['MATLAB_class'] = b'double'
    keys = _keys(tmp_path / 'drive.mat')
    assert keys == ['variable:drive:column:1', 'variable:drive:column:2']
    done, _b = _round_trip(tmp_path / 'drive.mat', keys, values)
    assert done.sidecar['source']['format'] == 'mat73'


def test_npy_and_npz(tmp_path):
    values = _values()
    np.save(tmp_path / 'drive.npy', values)
    _round_trip(tmp_path / 'drive.npy', ['array:column:1', 'array:column:2'], values,
                rate_hz=RATE)
    np.savez(tmp_path / 'drive.npz', x=values[:, 0], y=values[:, 1], fs=RATE)
    assert _keys(tmp_path / 'drive.npz') == ['array:x', 'array:y']
    _round_trip(tmp_path / 'drive.npz', ['array:y', 'array:x'], values[:, ::-1])


def test_hdf5_dataset_columns_units_and_rate(tmp_path):
    import h5py

    values = _values()
    with h5py.File(tmp_path / 'drive.h5', 'w') as f:
        f.attrs['sample_rate'] = RATE
        ds = f.create_group('test').create_dataset('drives', data=values * [1, 1000])
        ds.attrs['units'] = np.array([b'V', b'mV'])
        f.create_dataset('notes', data=np.array([b'a', b'b']))
    found = dc.inspect(tmp_path / 'drive.h5').signals
    assert [(s.key, s.unit, s.rate_hz) for s in found] == [
        ('dataset:/test/drives:column:1', 'V', RATE),
        ('dataset:/test/drives:column:2', 'mV', RATE)]
    done = dc.convert(tmp_path / 'drive.h5', [s.key for s in found])
    back = dc.read(done.folder)
    np.testing.assert_allclose(back.values, values, rtol=1e-6, atol=1e-9)
    np.testing.assert_array_equal(back.values[:, 0], values[:, 0])


def _history(values, units='V'):
    from hertzforge.core.data import TimeHistory

    history = TimeHistory(np.arange(values.shape[0]) / RATE, values.T.copy(),
                          response_dof=['101X+', '102X+'][:values.shape[1]])
    if units:
        history.define_units([units] * values.shape[1])
    return history


def test_hforge_time_history(tmp_path):
    values = _values()
    _history(values).save(tmp_path / 'drive.hforge')
    keys = _keys(tmp_path / 'drive.hforge')
    assert keys == ['object:drive:record:1', 'object:drive:record:2']
    _round_trip(tmp_path / 'drive.hforge', keys, values)


def test_universal_file_dataset_58(tmp_path):
    from hertzforge.io import unv

    values = _values()
    unv.save(_history(values, units=None), tmp_path / 'drive.unv')
    found = dc.inspect(tmp_path / 'drive.unv').signals
    assert [s.samples for s in found] == [N, N] and {s.rate_hz for s in found} == {RATE}
    done = dc.convert(tmp_path / 'drive.unv', [s.key for s in found])
    back = dc.read(done.folder)
    np.testing.assert_allclose(back.values, values, rtol=1e-6, atol=1e-7)


def test_excel(tmp_path):
    from openpyxl import Workbook

    values = _values()
    book = Workbook()
    sheet = book.active
    sheet.title = 'Drive'
    sheet.append(['Time (s)', 'Drive A (V)', 'Drive B'])
    for k, row in enumerate(values):
        sheet.append([k / RATE, float(row[0]), float(row[1])])
    book.save(tmp_path / 'drive.xlsx')
    found = dc.inspect(tmp_path / 'drive.xlsx').signals
    assert [(s.key, s.name, s.unit, s.rate_hz) for s in found] == [
        ('sheet:Drive:column:B', 'Drive A', 'V', RATE),
        ('sheet:Drive:column:C', 'Drive B', None, RATE)]
    _round_trip(tmp_path / 'drive.xlsx', [s.key for s in found], values)


def test_wav_integers_scale_to_one_volt_full_scale(tmp_path):
    from scipy.io import wavfile

    values = _values()
    wavfile.write(tmp_path / 'drive.wav', int(RATE), (values * 32768).astype('<i2'))
    found = dc.inspect(tmp_path / 'drive.wav')
    assert [s.key for s in found.signals] == ['channel:1', 'channel:2']
    assert any('16-bit integer samples, scaled to [-1, 1)' in n for n in found.notes)
    _round_trip(tmp_path / 'drive.wav', ['channel:1', 'channel:2'], values)
    wavfile.write(tmp_path / 'float.wav', int(RATE), values.astype('<f4'))
    _round_trip(tmp_path / 'float.wav', ['channel:2'], values[:, 1:])


def test_a_projects_time_history(tmp_path):
    values = _values()
    objects = {'Drive': _history(values), 'Note': 'not a history'}
    found = dc.inspect(objects)
    assert [s.key for s in found.signals] == ['object:Drive:record:1', 'object:Drive:record:2']
    assert any("'Note' is a str" in n for n in found.notes)
    done, _b = _round_trip(objects, [{'signal': 'object:Drive:record:2', 'name': 'Shaker'},
                                     'object:Drive:record:1'], values[:, ::-1],
                           project_file='/somewhere/test.hforge')
    source = done.sidecar['source']
    assert source['format'] == 'project' and source['path'] == '/somewhere/test.hforge'
    expected = hashlib.sha256(values[:, 1].astype('<f8').tobytes()
                              + values[:, 0].astype('<f8').tobytes()).hexdigest()
    assert source['sha256'] == expected
    assert [c['name'] for c in done.sidecar['channels']] == ['Shaker', 'Drive 101X+']


def test_large_inputs_stream_in_blocks(tmp_path, monkeypatch):
    monkeypatch.setattr(dc, 'CHUNK_SAMPLES', 64)
    values = _values(1000, 3)
    np.save(tmp_path / 'long.npy', values)
    _round_trip(tmp_path / 'long.npy', ['array:column:3', 'array:column:1'],
                values[:, [2, 0]], rate_hz=RATE)


# ---- refusals ------------------------------------------------------------------

def test_unequal_lengths_are_refused_by_name(tmp_path):
    values = _values()
    np.savez(tmp_path / 'd.npz', a=values[:, 0], b=values[:-3, 1])
    with pytest.raises(ValueError, match=r'a has 400, b has 397'):
        dc.convert(tmp_path / 'd.npz', ['array:a', 'array:b'], rate_hz=RATE)


def test_a_unit_that_is_not_a_voltage_is_refused_by_name(tmp_path):
    path = _csv(tmp_path / 'd.csv', _values(), units=('V', 'g'))
    assert [s.problem is not None for s in dc.inspect(path).signals] == [False, True]
    dc.convert(path, ['column:2'])
    with pytest.raises(ValueError, match=r"Drive B is in 'g', which is a acceleration unit"):
        dc.convert(path, ['column:2', 'column:3'])


def test_a_time_column_that_is_not_even_is_refused(tmp_path):
    lines = ['Time (s),Drive (V)'] + [f'{t:.6f},0.5' for t in
                                      (0.0, 0.001, 0.002, 0.0035, 0.004, 0.005, 0.006)]
    (tmp_path / 'd.csv').write_text('\n'.join(lines) + '\n', encoding='utf-8')
    with pytest.raises(ValueError, match='(?i)even|uneven|time'):
        dc.convert(tmp_path / 'd.csv', ['column:2'])
    np.savez(tmp_path / 'd.npz', t=np.array([0, 1, 2, 4, 5.0]), x=np.zeros(5))
    with pytest.raises(ValueError, match='does not step evenly'):
        dc.convert(tmp_path / 'd.npz', ['array:x'])


def test_a_rate_that_disagrees_with_the_file_is_refused(tmp_path):
    path = _csv(tmp_path / 'd.csv', _values())
    with pytest.raises(ValueError, match=r'the file states 1000 samples per second for Drive A '
                                         r'and 2000 was given'):
        dc.convert(path, ['column:2'], rate_hz=2000)
    done = dc.convert(path, ['column:2'], rate_hz=1000.00001)
    assert done.sidecar['rate_hz'] == 1000.00001


def test_a_value_that_is_not_finite_is_refused_and_nothing_is_left(tmp_path):
    values = _values()
    values[7, 1] = np.nan
    np.savez(tmp_path / 'd.npz', a=values[:, 0], b=values[:, 1], fs=RATE)
    with pytest.raises(ValueError, match=r'b: sample 8 \(at 0.007 s\) is nan'):
        dc.convert(tmp_path / 'd.npz', ['array:a', 'array:b'])
    assert os.listdir(dc.default_folder()) == []
    values[7, 1] = 1e39
    np.savez(tmp_path / 'e.npz', b=values[:, 1], fs=RATE)
    with pytest.raises(ValueError, match='too large'):
        dc.convert(tmp_path / 'e.npz', ['array:b'])


class _Planted:
    """Unpickling this would make a file: the test fails if the file is there."""

    marker = ''

    def __reduce__(self):
        return (open, (self.marker, 'w'))


def test_a_pickled_npz_is_refused_without_unpickling(tmp_path):
    _Planted.marker = str(tmp_path / 'ran')
    np.savez(tmp_path / 'd.npz', x=np.zeros(4), evil=np.array([_Planted()], dtype=object))
    with pytest.raises(ValueError, match='holds Python objects'):
        dc.inspect(tmp_path / 'd.npz')
    with pytest.raises(ValueError, match='holds Python objects'):
        dc.convert(tmp_path / 'd.npz', ['array:x'], rate_hz=RATE)
    np.save(tmp_path / 'd.npy', np.array([_Planted()], dtype=object))
    with pytest.raises(ValueError, match='holds Python objects'):
        dc.inspect(tmp_path / 'd.npy')
    assert not (tmp_path / 'ran').exists()


def test_a_folder_is_never_written_over(tmp_path):
    path = _csv(tmp_path / 'd.csv', _values())
    first = dc.convert(path, ['column:2']).folder
    second = dc.convert(path, ['column:2']).folder
    assert os.path.basename(second) == 'd-2' and first != second
    with pytest.raises(ValueError, match='never written over'):
        dc.convert(path, ['column:2'], out=first)


def test_selections_are_checked(tmp_path):
    path = _csv(tmp_path / 'd.csv', _values())
    with pytest.raises(ValueError, match="'column:9' is not a signal"):
        dc.convert(path, ['column:9'])
    with pytest.raises(ValueError, match='chosen twice'):
        dc.convert(path, ['column:2', 'column:2'])
    with pytest.raises(ValueError, match='two drives are named'):
        dc.convert(path, ['column:2', {'signal': 'column:3', 'name': 'Drive A'}])
    with pytest.raises(ValueError, match='at least one'):
        dc.convert(path, [])
    with pytest.raises(FileNotFoundError):
        dc.inspect(tmp_path / 'missing.csv')
    with pytest.raises(ValueError, match='is not one'):
        dc.inspect(_csv(tmp_path / 'd.json', _values()))


def test_read_refuses_a_changed_or_cut_file(tmp_path):
    folder = pathlib.Path(dc.convert(_csv(tmp_path / 'd.csv', _values()), ['column:2']).folder)
    data = bytearray((folder / 'drive.bin').read_bytes())
    data[20] ^= 1
    (folder / 'drive.bin').write_bytes(bytes(data))
    with pytest.raises(ValueError, match='does not match the SHA-256'):
        dc.read(folder)
    (folder / 'drive.bin').write_bytes(bytes(data[:-4]))
    with pytest.raises(ValueError, match='bytes;'):
        dc.read(folder)


# ---- fix ends ------------------------------------------------------------------

def _drifting(n=4000):
    t = np.arange(n) / RATE
    return np.stack([0.5 * np.sin(2 * np.pi * 7 * t) + 0.2 + 0.1 * t,
                     0.3 * np.cos(2 * np.pi * 23 * t) - 0.05], axis=1)


def test_fix_ends_puts_the_ends_at_rest_in_a_new_folder(tmp_path):
    values = _drifting()
    folder = dc.write(values, RATE, ['A', 'B'], source={'path': None, 'sha256': None,
                                                        'format': 'test', 'selections': []})
    before = (pathlib.Path(folder) / 'drive.bin').read_bytes()
    done = dc.fix_ends(folder)
    assert (pathlib.Path(folder) / 'drive.bin').read_bytes() == before
    assert done['folder'] != folder and os.path.basename(done['folder']) == 'A-fixed'
    fixed = dc.read(done['folder'])
    dt = 1.0 / RATE
    for c in range(2):
        x = fixed.values[:, c].astype(np.float64)
        assert x[0] == 0.0 and x[-1] == 0.0
        v, d = dc._integrals(x, dt)
        assert abs(v[-1]) < 1e-6 * np.max(np.abs(v))
        assert abs(d[-1]) < 1e-6 * np.max(np.abs(d))
        raw = dc._ends(values[:, c], dt)
        assert abs(raw['velocity_end']) > 1e-2 * raw['velocity_peak']
    # 1 % of a 4 s record is 40 ms, 40 samples
    assert done['taper_samples'] == 40 and done['taper_s'] == 0.04
    record = fixed.sidecar['fixes'][-1]
    assert record['kind'] == 'fix_ends' and record['taper_samples'] == 40
    assert record['from']['sha256'] == hashlib.sha256(before).hexdigest()
    assert [c['name'] for c in record['channels']] == ['A', 'B']
    assert all(np.all(np.isfinite([c['a'], c['b'], c['rms_before'], c['rms_after']]))
               for c in record['channels'])
    summary = done['channels'][0]
    assert summary['after']['velocity_end'] == pytest.approx(0, abs=1e-6 * summary['after'][
        'velocity_peak'])
    assert summary['before']['start'] != 0
    with pytest.raises(ValueError, match='never written over'):
        dc.fix_ends(folder, out=done['folder'])


def test_fix_ends_leaves_a_long_tone_at_rest_as_stored(tmp_path):
    # 20 s of a 710 Hz tone at 12 800 Hz: its displacement drifts as made. What
    # fix_ends writes, read back and integrated as the control service does, ends
    # at rest to far inside the service's 1 %.
    rate = 12_800.0
    t = np.arange(int(20 * rate)) / rate
    values = 0.5 * np.sin(2 * np.pi * 710.0 * t)[:, None]
    folder = dc.write(values, rate, ['Tone'], source={'path': None, 'sha256': None,
                                                      'format': 'test', 'selections': []})
    raw = dc._ends(values[:, 0], 1.0 / rate)
    assert abs(raw['displacement_end']) > 1e-2 * raw['displacement_peak']
    done = dc.fix_ends(folder)
    stored = dc.read(done['folder']).values[:, 0].astype(np.float64)
    v, d = dc._integrals(stored, 1.0 / rate)
    assert abs(v[-1]) <= 1e-6 * np.max(np.abs(v))
    assert abs(d[-1]) <= 1e-6 * np.max(np.abs(d))


def test_fix_ends_bounds_its_taper_and_leaves_a_zero_channel(tmp_path):
    assert dc.taper_samples(1001, 1000.0) == 10          # 1 % of 1 s is under 10 ms
    assert dc.taper_samples(1_000_001, 1000.0) == 1000   # 1 % of 1000 s is over 1 s
    assert dc.taper_samples(11, 1000.0) == 5             # never more than half
    values = _drifting(2000)
    values[:, 1] = 0.0
    folder = dc.write(values, RATE, ['A', 'Z'], source={'path': None, 'sha256': None,
                                                        'format': 'test', 'selections': []})
    done = dc.fix_ends(folder, taper_s=0.05)
    assert done['taper_samples'] == 50
    zero = done['channels'][1]
    assert (zero['a'], zero['b'], zero['rms_after']) == (0.0, 0.0, 0.0)


def test_convert_can_fix_ends_too(tmp_path):
    np.save(tmp_path / 'd.npy', _drifting(1000))
    done = dc.convert(tmp_path / 'd.npy', ['array:column:1'], rate_hz=RATE, fix_ends=True)
    assert done.fixed is not None and done.fixed['from']['folder'] == done.folder
    assert dc.read(done.folder).sidecar['fixes'] == []
    assert dc.read(done.fixed['folder']).sidecar['fixes'][0]['kind'] == 'fix_ends'


# ---- the golden folder ---------------------------------------------------------

def _make_golden():
    spec = importlib.util.spec_from_file_location('make_golden_drive',
                                                  ROOT / 'schema' / 'make_golden_drive.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_the_golden_drive_folder_holds_its_known_values():
    side = json.loads((GOLDEN / 'drive.json').read_text(encoding='utf-8'))
    data = (GOLDEN / 'drive.bin').read_bytes()
    samples, width = side['samples'], len(side['channels'])
    # the layout, read without the reader: little-endian float64
    assert data[:8] == b'HFDRV01\x00'
    assert len(data) == 8 + 8 * samples * width
    assert hashlib.sha256(data).hexdigest() == side['sha256']
    values = np.array(struct.unpack(f'<{samples * width}d', data[8:])).reshape(samples, width)
    expected = _make_golden().known_values()
    np.testing.assert_array_equal(values, expected)
    back = dc.read(GOLDEN)
    np.testing.assert_array_equal(back.values, values)
    assert (back.rate_hz, back.names) == (1000.0, ['Drive 1', 'Drive 2'])
    # it starts and ends at rest, so it passes the control service's check
    for c in range(width):
        ends = dc._ends(values[:, c], 1.0 / back.rate_hz)
        assert ends['start'] == 0.0 and ends['end'] == 0.0
        # within the values' float32 rounding: far inside the service's 1 %
        assert abs(ends['velocity_end']) < 1e-5 * ends['velocity_peak']
        assert abs(ends['displacement_end']) < 1e-5 * ends['displacement_peak']


def test_the_script_writes_the_golden_drive_folder_as_committed(tmp_path):
    out = tmp_path / 'basic'
    _make_golden().main(out)
    assert sorted(p.name for p in out.iterdir()) == sorted(p.name for p in GOLDEN.iterdir())
    for name in ('drive.bin', 'drive.json'):
        assert (out / name).read_bytes() == (GOLDEN / name).read_bytes(), name


def test_the_fixed_golden_folder_round_trips():
    # written by fix_ends from a drive whose ends are not at rest; the control
    # service's tests check that it passes the service's check
    side = json.loads((GOLDEN_FIXED / 'drive.json').read_text(encoding='utf-8'))
    data = (GOLDEN_FIXED / 'drive.bin').read_bytes()
    samples, width = side['samples'], len(side['channels'])
    assert len(data) == 8 + 8 * samples * width
    values = np.array(struct.unpack(f'<{samples * width}d', data[8:])).reshape(samples, width)
    back = dc.read(GOLDEN_FIXED)
    np.testing.assert_array_equal(back.values, values)
    assert side['fixes'][-1]['kind'] == 'fix_ends'
    made = _make_golden()
    rate = back.rate_hz
    for c in range(width):
        before = dc._ends(made.ends_values()[:, c], 1.0 / rate)
        assert abs(before['start']) > 0.005 * before['peak']
        after = dc._ends(values[:, c], 1.0 / rate)
        assert after['start'] == 0.0 and after['end'] == 0.0
        assert abs(after['velocity_end']) <= 1e-6 * after['velocity_peak']
        assert abs(after['displacement_end']) <= 1e-6 * after['displacement_peak']
    # written again from what was read, the samples are the same bytes
    again = dc.write(back.values, rate, back.names, source=side['source'],
                     fixes=side['fixes'], channel_sources=[c['source'] for c in side['channels']],
                     created_utc=side['created_utc'], tool=side['tool'])
    assert (pathlib.Path(again) / 'drive.bin').read_bytes() == data
    assert json.loads((pathlib.Path(again) / 'drive.json').read_text(encoding='utf-8')) == side


def test_the_script_writes_the_fixed_golden_folder(tmp_path):
    out = tmp_path / 'fixed'
    _make_golden().fixed(out)
    side = json.loads((GOLDEN_FIXED / 'drive.json').read_text(encoding='utf-8'))
    made = json.loads((out / 'drive.json').read_text(encoding='utf-8'))
    # the correction is solved in float64 arithmetic, which may differ in its last
    # place between platforms; on this one it is the committed folder exactly
    for key in ('format', 'version', 'rate_hz', 'samples', 'channels', 'source', 'tool',
                'created_utc'):
        assert made[key] == side[key], key
    ours, theirs = made['fixes'][-1], side['fixes'][-1]
    assert ours['from'] == theirs['from']
    assert (ours['taper_s'], ours['taper_samples']) == (theirs['taper_s'], theirs['taper_samples'])
    for a, b in zip(ours['channels'], theirs['channels'], strict=True):
        for key in ('a', 'b', 'rms_before', 'rms_after'):
            assert a[key] == pytest.approx(b[key], rel=1e-9, abs=1e-15)
    np.testing.assert_allclose(dc.read(out).values, dc.read(GOLDEN_FIXED).values,
                               rtol=0, atol=1e-12)
