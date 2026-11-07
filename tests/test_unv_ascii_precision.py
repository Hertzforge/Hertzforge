"""The ASCII form of dataset 58 writes what its record declares: double
precision values at E20.12, an even abscissa as its start and step, and a
NaN or an infinity that the reader can read back. Types the format calls
unknown stay on the record instead of refusing the file."""

import sys

import numpy as np
import pytest

import hertzforge as hz
from hertzforge.core.data import Frf, Psd, Spectrum
from hertzforge.io import import_file, unv
from hertzforge.io.exporters import data_values
from hertzforge.units import DEFAULT_SYSTEM


def back(obj, tmp_path, name='f.unv', **kw):
    path = tmp_path / name
    unv.save(obj, path, **kw)
    return import_file(str(path))


def test_a_long_fast_record_keeps_its_rate(tmp_path):
    rate = 51200.0
    t = np.arange(int(20 * rate)) / rate
    history = hz.TimeHistory(t, np.sin(t)[None, :], ['1X+'])
    read = back(history, tmp_path)
    assert read.sample_rate == pytest.approx(rate, rel=1e-12)
    assert read.ordinate[0, 500000] == pytest.approx(history.ordinate[0, 500000], rel=1e-11)


def test_a_step_with_no_short_decimal_keeps_seven_figures(tmp_path):
    rate = 48000.0
    history = hz.TimeHistory(np.arange(4096) / rate, np.ones((1, 4096)), ['1X+'])
    assert back(history, tmp_path).sample_rate == pytest.approx(rate, rel=1e-6)


def test_fine_lines_above_a_thousand_hertz_stay_on_their_step(tmp_path):
    lines = 1000.0 + 0.125 * np.arange(64)
    frf = Frf(lines, np.ones((1, 64)) * (1 + 2j), ['1X+'], ['2X+'])
    read = back(frf, tmp_path)
    assert np.allclose(read.abscissa, lines, rtol=1e-12)
    assert np.allclose(np.diff(read.abscissa), 0.125, rtol=1e-9)


@pytest.mark.parametrize('make', ['real', 'complex'])
def test_ordinates_come_back_to_twelve_decimals(tmp_path, make):
    rng = np.random.default_rng(0)
    values = rng.normal(size=(2, 33)) * 10.0 ** rng.integers(-9, 9, size=(2, 33))
    if make == 'complex':
        values = values + 1j * rng.normal(size=(2, 33))
        data = Spectrum(np.arange(33) * 2.0, values, ['1X+', '2X+'])
    else:
        data = hz.TimeHistory(np.arange(33) / 8.0, values, ['1X+', '2X+'])
    assert np.allclose(back(data, tmp_path).ordinate, values, rtol=1e-11, atol=0)


@pytest.mark.parametrize('make', ['real', 'complex'])
def test_an_uneven_abscissa_is_written_with_every_value(tmp_path, make):
    lines = np.array([1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0])
    values = np.linspace(-1.0, 5.0, 7)[None, :] + (0.5j if make == 'complex' else 0.0)
    data = (Spectrum if make == 'complex' else hz.Srs)(lines, values, ['1X+'])
    read = back(data, tmp_path)
    assert np.allclose(read.abscissa, lines)
    assert np.allclose(read.ordinate, values, rtol=1e-11)


def test_the_record_lays_out_the_declared_double_types(tmp_path):
    history = hz.TimeHistory(np.arange(9) / 8.0, np.arange(9.0)[None, :], ['1X+'])
    unv.save(history, tmp_path / 'f.unv')
    lines = (tmp_path / 'f.unv').read_text().splitlines()
    at = next(i for i, line in enumerate(lines) if line.strip() == '58')
    form = lines[at + 7]
    assert form[:30] == f'{4:10d}{9:10d}{1:10d}'
    assert len(form) == 69, 'three fields of thirteen columns'
    assert float(form[30:43]) == 0.0 and float(form[43:56]) == 0.125
    assert len(lines[at + 12]) == 80, 'four values of twenty columns'


@pytest.mark.parametrize('bad', [np.nan, np.inf, -np.inf])
def test_a_nan_or_an_infinity_reads_back(tmp_path, bad):
    values = np.arange(8.0)[None, :]
    values[0, 3] = bad
    read = back(hz.TimeHistory(np.arange(8) / 8.0, values, ['1X+']), tmp_path)
    assert read.ordinate.shape == (1, 8)
    assert np.array_equal(read.ordinate, values, equal_nan=True)


# ---- type 0 is unknown ---------------------------------------------------------

def frf_file(tmp_path):
    frf = Frf(np.arange(1, 9) * 1.0, np.ones((1, 8)) * (1 + 1j), ['1X+'], ['2X+'])
    frf.define_units('m/s**2', 'N')
    unv.save(frf, tmp_path / 'f.unv')
    return (tmp_path / 'f.unv').read_text()


def test_an_unknown_numerator_keeps_the_file(tmp_path):
    text = frf_file(tmp_path).replace('        12    0    0    0', '         0    0    0    0')
    (tmp_path / 'g.unv').write_text(text)
    read = import_file(str(tmp_path / 'g.unv'))
    assert read.ordinate_dim == ['unknown']
    assert read.reference_dof == ['2X+']


def test_an_unknown_denominator_is_a_ratio_to_nothing(tmp_path):
    text = frf_file(tmp_path).replace('        13    0    0    0', '         0    0    0    0')
    text = text.replace('         9    0    0    0', '         0    0    0    0')
    (tmp_path / 'g.unv').write_text(text)
    read = import_file(str(tmp_path / 'g.unv'))
    assert read.ordinate_dim == ['acceleration/dimensionless']


def test_a_psd_written_to_an_afu_reads_back(tmp_path):
    from hertzforge.io import export_file
    psd = Psd(np.arange(1, 9) * 1.0, np.full((1, 8), 2.0 + 0j), ['1X+'], ['1X+'])
    psd.define_units('g')
    path = tmp_path / 'p.afu'
    export_file(psd, str(path))
    read = import_file(str(path))
    assert read.ordinate_dim == ['unknown']
    assert np.allclose(read.ordinate.real, psd.ordinate.real, rtol=1e-5)


# ---- the cost ------------------------------------------------------------------

@pytest.mark.parametrize('save', [unv.save, unv.save_binary])
def test_the_object_is_converted_once(tmp_path, save, monkeypatch):
    frf = Frf(np.arange(1, 9) * 1.0, np.ones((40, 8)) + 0j, [f'{i + 1}X+' for i in range(40)],
              ['99X+'] * 40)
    frf.define_units('g', 'lbf')
    exporters = sys.modules[data_values.__module__]
    calls = []
    monkeypatch.setattr(exporters, 'data_values',
                        lambda *a, **k: calls.append(1) or data_values(*a, **k))
    save(frf, tmp_path / 'f.unv', DEFAULT_SYSTEM)
    assert len(calls) == 1
