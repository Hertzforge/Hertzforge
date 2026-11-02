"""Compute Spectra: averaged spectra from multi-average time data.

Rattlesnake writes the time frames of a burst random modal test but not
their spectra; right-clicking the time history fills that gap. The
convention is sdynpy's `TimeHistoryArray.fft` with frames, on Brandon's
explicit ask: a raw single-sided rfft (norm='backward' — a sine of
amplitude A on a bin reads A*N/2) and a *complex mean* across frames,
so phase is preserved and random-phase content averages toward zero.
"""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.core.data import Spectrum, TimeHistory

N = 256          # 1 s at 256 Hz: df = 1 Hz, so test tones sit on bins


def _burst_time(frames=4, random_phase=False):
    """Two channels of exact-bin sines: 1X+ is 1 m/s^2 at 16 Hz, 2X+ is
    2 N at 24 Hz — every frame in phase, or each at a random phase."""
    t = np.arange(N) / N
    rng = np.random.default_rng(3)
    rows, dofs, blocks = [], [], []
    for dof, amplitude, f0 in (('1X+', 1.0, 16), ('2X+', 2.0, 24)):
        for k in range(frames):
            phase = rng.uniform(0, 2 * np.pi) if random_phase else 0.0
            rows.append(amplitude * np.sin(2 * np.pi * f0 * t + phase))
            dofs.append(dof)
            blocks.append(f'avg {k + 1}')
    time = TimeHistory(t, np.array(rows), response_dof=dofs, block=blocks)
    time.define_units({i: 'm/s^2' for i in range(frames)}
                      | {i + frames: 'N' for i in range(frames)})
    return time


def test_spectra_are_the_complex_mean_of_the_frames():
    time = _burst_time()
    spectra = time.compute_spectra()
    assert isinstance(spectra, Spectrum)
    assert spectra.num_records == 2, 'one record per channel, not per frame'
    assert spectra.response_dof == ['1X+', '2X+']
    assert spectra.ordinate_dim == ['acceleration', 'force']
    # in-phase frames survive the average at the raw rfft's own scale:
    # a sine of amplitude A on a bin reads A*N/2
    magnitudes = np.abs(spectra.ordinate)
    assert magnitudes[0, 16] == pytest.approx(1.0 * N / 2, rel=1e-9)
    assert magnitudes[1, 24] == pytest.approx(2.0 * N / 2, rel=1e-9)
    # and the phase rides along: rfft of a sine is purely -i at its bin
    assert spectra.ordinate[0, 16].imag == pytest.approx(-N / 2, rel=1e-9)
    off = magnitudes.copy()
    off[0, 16] = off[1, 24] = 0.0
    assert off.max() < 1e-7, 'exact-bin tones leak nowhere'
    assert float(spectra.abscissa[16]) == pytest.approx(16.0)


def test_random_phase_frames_average_toward_zero():
    """The behavior Brandon asked for by name: a complex average zeroes
    content whose phase is random frame to frame, exactly as sdynpy's
    fft documents. An RMS-magnitude average would keep it at full
    amplitude and fail this."""
    single = np.abs(_burst_time(frames=1).compute_spectra().ordinate)
    averaged = np.abs(
        _burst_time(frames=32, random_phase=True).compute_spectra().ordinate)
    assert averaged[0, 16] < 0.3 * single[0, 16]
    assert averaged[1, 24] < 0.3 * single[1, 24]


def test_the_spectrum_is_the_raw_single_sided_rfft():
    """The convention, pinned: sdynpy's. A sine with a peak-to-peak
    amplitude of 5 (2.5 peak) reads 2.5*N/2 at its own frequency —
    numpy's unscaled rfft, single-sided, rectangular window."""
    for n in (256, 501):                    # even and odd frame lengths
        t = np.arange(n) / n                # df = 1 Hz, 40 Hz on a bin
        wave = 2.5 * np.sin(2 * np.pi * 40 * t)     # 5 peak to peak
        assert wave.max() - wave.min() == pytest.approx(5.0, rel=1e-3)
        time = TimeHistory(t, wave[None, :], response_dof=['1X+'])
        spectra = time.compute_spectra()
        bin40 = int(np.argmin(np.abs(spectra.abscissa - 40.0)))
        assert float(spectra.abscissa[bin40]) == pytest.approx(40.0)
        assert np.abs(spectra.ordinate[0, bin40]) == pytest.approx(
            2.5 * n / 2, rel=1e-9)


def test_psds_are_the_welch_average_of_the_frames():
    """One-sided auto-power density, rectangular window, powers
    averaged — scipy's Welch with each average as its own frame, and
    Parseval holds: the PSD integrates to the signal's mean square."""
    import pytest

    # a dev extra, so CI has it; skipped rather than errored for anyone
    # running the suite without them
    welch = pytest.importorskip('scipy.signal').welch

    from hertzforge.core.data import Psd

    time = _burst_time(frames=8, random_phase=True)
    psds = time.compute_psds()
    assert isinstance(psds, Psd)
    assert psds.num_records == 2
    assert psds.ordinate_dim == ['acceleration**2/frequency',
                                 'force**2/frequency']
    # random phase costs nothing: power is phase-insensitive, so the
    # tone keeps its full A^2/(2 df) in its bin
    assert np.abs(psds.ordinate[0, 16]) == pytest.approx(0.5, rel=1e-9)
    assert np.abs(psds.ordinate[1, 24]) == pytest.approx(2.0, rel=1e-9)
    # scipy agrees, frame for frame
    rows = [i for i, d in enumerate(time.response_dof) if d == '1X+']
    signal = np.concatenate([time.ordinate[i] for i in rows])
    _freqs, reference = welch(signal, fs=N, nperseg=N, noverlap=0,
                             window='boxcar', detrend=False)
    assert np.allclose(np.abs(psds.ordinate[0]), reference, rtol=1e-9)
    # Parseval: integrated density equals the mean square of the frames
    mean_square = np.mean([time.ordinate[i] ** 2 for i in rows])
    df = float(psds.abscissa[1] - psds.abscissa[0])
    assert np.sum(np.abs(psds.ordinate[0])) * df == pytest.approx(
        mean_square, rel=1e-9)


def test_uneven_sampling_is_refused():
    time = TimeHistory(np.array([0.0, 0.1, 0.3]), np.ones((1, 3)),
                       response_dof=['1X+'])
    with pytest.raises(ValueError, match='evenly spaced'):
        time.compute_spectra()


def _two_channel_time(frames=6):
    """The same signal on two channels, several frames of each."""
    import numpy as np

    from hertzforge.core.data import TimeHistory

    fs, n = 1000.0, 512
    t = np.arange(n) / fs
    rng = np.random.default_rng(0)
    rows, dofs = [], []
    for dof in ('101X+', '102X+'):
        for _ in range(frames):
            rows.append(np.sin(2 * np.pi * 50.0 * t)
                        + 0.1 * rng.standard_normal(n))
            dofs.append(dof)
    return TimeHistory(abscissa=t, ordinate=np.array(rows),
                       response_dof=dofs,
                       ordinate_dim=['acceleration'] * len(rows))


def test_cpsds_are_the_whole_matrix_not_the_diagonal():
    history = _two_channel_time()
    cpsds = history.compute_cpsds()
    assert cpsds.num_records == 4, 'two channels make a 2x2'
    assert list(zip(cpsds.response_dof, cpsds.reference_dof)) == [
        ('101X+', '101X+'), ('101X+', '102X+'),
        ('102X+', '101X+'), ('102X+', '102X+')]


def test_the_diagonal_is_exactly_compute_psds():
    """Two ways of saying the same thing must say it identically, or
    which one you asked for changes the answer."""
    import numpy as np

    history = _two_channel_time()
    psds, cpsds = history.compute_psds(), history.compute_cpsds()
    for k, (response, reference) in enumerate(zip(cpsds.response_dof,
                                                  cpsds.reference_dof)):
        if response != reference:
            continue
        row = list(psds.response_dof).index(response)
        assert np.allclose(cpsds.ordinate[k], psds.ordinate[row])


def test_an_autospectrum_is_stored_real():
    """A channel against itself is a magnitude squared. Held complex it
    carried an imaginary part of exactly nothing at twice the size, and
    every caller asking "is this complex?" — the component selector
    above all — got the answer of the type rather than of the data."""
    psds = _two_channel_time().compute_psds()
    assert psds.ordinate.dtype == np.float64


def test_a_cross_spectrum_keeps_its_phase():
    """The other half of the same rule: demoting a CPSD would discard
    the phase between channels, which is most of what it is for."""
    cpsds = _two_channel_time().compute_cpsds()
    assert cpsds.ordinate.dtype == np.complex128
    cross = [k for k, (r, f) in enumerate(zip(cpsds.response_dof,
                                              cpsds.reference_dof)) if r != f]
    assert np.abs(cpsds.ordinate[cross].imag).max() > 0.0


def test_a_cpsd_of_one_channel_is_real():
    """The rule reads the values, not the class: a 1x1 CPSD is its own
    diagonal, so there is no cross term and nothing to hold complex."""
    history = _two_channel_time()
    keep = [k for k, dof in enumerate(history.response_dof) if dof == '101X+']
    one = TimeHistory(history.abscissa, history.ordinate[keep],
                      response_dof=[history.response_dof[k] for k in keep])
    assert one.compute_cpsds().ordinate.dtype == np.float64


def test_the_matrix_is_hermitian():
    """Gxy is the conjugate of Gyx — the property that says the cross
    terms are a cross-spectrum and not two unrelated numbers."""
    import numpy as np

    cpsds = _two_channel_time().compute_cpsds()
    assert np.allclose(cpsds.ordinate[1], np.conj(cpsds.ordinate[2]))


def test_a_cross_term_carries_the_product_dimension():
    from hertzforge.core.data import TimeHistory

    history = _two_channel_time()
    history.ordinate_dim = ['acceleration'] * 6 + ['force'] * 6
    cpsds = TimeHistory(
        abscissa=history.abscissa, ordinate=history.ordinate,
        response_dof=history.response_dof,
        ordinate_dim=history.ordinate_dim).compute_cpsds()
    dims = dict(zip(zip(cpsds.response_dof, cpsds.reference_dof),
                    cpsds.ordinate_dim))
    assert dims[('101X+', '101X+')] == 'acceleration**2/frequency'
    assert dims[('101X+', '102X+')] == 'acceleration*force/frequency'
