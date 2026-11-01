"""The chi-square band as a reading: each record's band stated with the
confidence and the count it was worked out at, and the legend of the
plot that shades it saying the same in words.

The counts are the ones the PSD was stamped with when it was averaged
(`Psd.statistical_dof`), never worked out again here. Hand values are
the printed chi-square table: at 120 degrees of freedom the 90 percent
band runs from 0.9 dB below to 1.0 dB above the estimate, at 16 from
2.2 below to 3.0 above, and at 2 from 4.8 below to 12.9 above.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge.core.averaging import (
    Averaging,
    ConfidenceBand,
    chi_square_factors,
    confidence_bands,
    db_text,
    dof_text,
)
from hertzforge.core.data import Psd, TimeHistory


def _averaged(frames=10, overlap=0.5, channels=3, length=256,
              rate=1024.0, seed=1503):
    """The PSD of white noise averaged over Hann frames, carrying the
    count its averaging gives."""
    averaging = Averaging(frame_length=length, overlap=overlap,
                          window='hann', frames=frames)
    rng = np.random.default_rng(seed)
    history = TimeHistory(np.arange(averaging.span) / rate,
                          rng.standard_normal((channels, averaging.span)),
                          response_dof=[f'{i + 1}X+' for i in
                                        range(channels)],
                          ordinate_dim='acceleration')
    history.averaging = averaging
    return history.compute_psds(), averaging


@pytest.mark.parametrize('dof, level, words', [
    (120.0, 0.90, '90 % band at 120 DOF: -0.9 / +1.0 dB'),
    (120.0, 0.95, '95 % band at 120 DOF: -1.0 / +1.2 dB'),
    (16.0, 0.90, '90 % band at 16 DOF: -2.2 / +3.0 dB'),
    (2.0, 0.90, '90 % band at 2 DOF: -4.8 / +12.9 dB'),
])
def test_a_band_says_its_confidence_its_count_and_its_edges(dof, level,
                                                            words):
    band, = confidence_bands(1.0, dof, levels=[level])
    assert band.describe() == words


@pytest.mark.parametrize('dof, shown', [
    (120.0, '120'), (225.9437, '225.9'), (2.0, '2'), (17.1718, '17.17'),
    (1504.6, '1505'), (9999.4, '9999'), (9999.7, '10000'),
    (12500.3, '12500'),
    (1.3e6, '1300000'),
])
def test_a_count_is_shown_to_four_figures(dof, shown):
    assert dof_text(dof) == shown


def test_each_record_is_read_at_the_count_it_carries():
    psd, averaging = _averaged()
    readings = psd.confidence_reading(0.9)
    assert len(readings) == psd.num_records
    assert all(isinstance(band, ConfidenceBand) for band in readings)
    expected = averaging.statistical_dof().equivalent
    for i, band in enumerate(readings):
        assert band.level == 0.9
        assert band.dof == pytest.approx(expected)
        low, high = chi_square_factors(expected, 0.9)
        assert band.lower_factor == pytest.approx(low)
        assert band.upper_factor == pytest.approx(high)
        assert np.allclose(band.lower, np.real(psd.ordinate[i]) * low)
        assert np.allclose(band.upper, np.real(psd.ordinate[i]) * high)


def test_the_reading_is_the_band_the_plot_draws():
    """The two views of one band: the reading's edges are the PSDs
    `confidence_band` returns, record by record."""
    psd, _averaging = _averaged()
    for level in (0.9, 0.95):
        lower, upper = psd.confidence_band(level)
        for i, band in enumerate(psd.confidence_reading(level)):
            assert np.allclose(band.lower, lower.ordinate[i])
            assert np.allclose(band.upper, upper.ordinate[i])


def test_ninety_five_is_wider_than_ninety_both_ways():
    psd, _averaging = _averaged()
    ninety = psd.confidence_reading(0.90)[0]
    ninety_five = psd.confidence_reading(0.95)[0]
    assert ninety_five.lower_db < ninety.lower_db < 0.0
    assert ninety_five.upper_db > ninety.upper_db > 0.0


def test_a_typed_count_overrides_one_for_all_or_one_per_record():
    psd, _averaging = _averaged()
    every = psd.confidence_reading(0.9, dof=120.0)
    assert [band.dof for band in every] == [120.0] * psd.num_records
    assert every[0].describe() == '90 % band at 120 DOF: -0.9 / +1.0 dB'
    each = psd.confidence_reading(0.9, dof=[2.0, 16.0, 120.0])
    assert [band.dof for band in each] == [2.0, 16.0, 120.0]
    assert each[1].lower_factor == pytest.approx(chi_square_factors(16.0)[0])


def test_an_imported_spectrum_is_read_only_at_a_count_given():
    psd = hertzforge.import_file(fixture_path('plate', 'psd.npz'))
    assert psd.statistical_dof is None
    with pytest.raises(ValueError, match='does not say what it was '
                                         'averaged over'):
        psd.confidence_reading()
    band = psd.confidence_reading(0.95, dof=40.0)[0]
    assert band.dof == 40.0
    assert band.lower_factor == pytest.approx(
        chi_square_factors(40.0, 0.95)[0])


def test_a_cross_term_is_not_an_autospectrum():
    lines = np.linspace(10.0, 100.0, 4)
    auto = Psd(lines, np.full((1, 4), 2.0), response_dof=['1X+'],
               ordinate_dim='acceleration**2/frequency')
    assert auto.is_autospectrum()
    cross = Psd(lines, np.ones((1, 4)), response_dof=['1X+'],
                reference_dof=['2X+'],
                ordinate_dim='acceleration*acceleration/frequency')
    assert not cross.is_autospectrum()
    # one DOF named twice, two quantities: still a cross term
    drive = Psd(lines, np.full((1, 4), 2.0 + 1.0j), response_dof=['1X+'],
                reference_dof=['1X+'],
                ordinate_dim='acceleration*force/frequency')
    assert not drive.is_autospectrum()
    for refused in (cross, drive):
        with pytest.raises(ValueError, match='cross terms'):
            refused.confidence_reading(dof=12.0)


def _target_spectra():
    """A breakpoint specification, and the PSD of a transient
    specification, which is a `Specification` carrying the count its
    averaging gives."""
    from hertzforge.core.data import (
        Specification,
        TransientSpecification,
    )

    breakpoints = Specification([20.0, 80.0, 350.0, 2000.0],
                                [[0.01, 0.04, 0.04, 0.007]],
                                response_dof=['1Z+'],
                                ordinate_dim='acceleration**2/frequency')
    averaging = Averaging(frame_length=256, overlap=0.5, window='hann',
                          frames=10)
    rng = np.random.default_rng(1509)
    waveform = TransientSpecification(
        np.arange(averaging.span) / 1024.0,
        rng.standard_normal((1, averaging.span)), response_dof=['1Z+'],
        ordinate_dim='acceleration')
    waveform.averaging = averaging
    averaged = waveform.compute_psds()
    assert isinstance(averaged, Specification)
    assert averaged.statistical_dof is not None, 'a count is stamped'
    return breakpoints, averaged


def test_a_specification_has_no_band_even_given_a_count():
    """The refusal is the spectrum's, not the window's: a script asking
    for the band on a target is told no, as the app never offers it."""
    for target in _target_spectra():
        assert target.confidence_refusal() is not None
        with pytest.raises(ValueError, match='specification'):
            target.confidence_band(0.9, dof=16.0)
        with pytest.raises(ValueError, match='specification'):
            target.confidence_reading(0.9, dof=16.0)
        with pytest.raises(ValueError, match='specification'):
            target.confidence_reading(0.9)


def test_an_average_and_a_typed_count_have_no_refusal():
    psd, _averaging = _averaged()
    assert psd.confidence_refusal() is None
    imported = hertzforge.import_file(fixture_path('plate', 'psd.npz'))
    assert imported.confidence_refusal() is None, \
        'a missing count is typed in, not refused'


def test_every_view_shows_an_edge_to_a_tenth_of_a_decibel():
    """The Averaging view's band row (`edges_text`), the confidence
    pane's Below and Above rows (`db_text`) and the status line
    (`describe`) give one figure for one edge."""
    band, = confidence_bands(1.0, 16.0, levels=[0.9])
    assert band.edges_text() == '-2.2 / +3.0 dB'
    assert db_text(band.lower_db) == '-2.2 dB'
    assert db_text(band.upper_db) == '+3.0 dB'
    assert band.describe().endswith(band.edges_text())


def test_a_drive_point_cross_term_reads_no_tone():
    """Force against acceleration at one DOF names 1X+ twice and has no
    imaginary part when real, yet is not an autospectrum: the tone
    readout refuses it by the same test the band does."""
    lines = np.arange(0.0, 64.0)
    drive = Psd(lines, np.full((1, 64), 2.0), response_dof=['1X+'],
                reference_dof=['1X+'],
                ordinate_dim='acceleration*force/frequency')
    assert not drive.is_autospectrum(0)
    assert 'cross spectrum' in drive.tone_refusal(0)
    auto = Psd(lines, np.full((1, 64), 2.0), response_dof=['1X+'],
               ordinate_dim='acceleration**2/frequency')
    assert auto.is_autospectrum(0)
    assert auto.tone_refusal(0) is None
