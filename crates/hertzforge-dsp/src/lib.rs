//! `hertzforge-dsp` - Layer 1 framework-free DSP core.
//!
//! No I/O, no UI, no allocation in the hot path. Pure functions and small stateful
//! processors that the real-time engine drives. Every non-trivial primitive is
//! validated by golden vectors in `tests/golden/` and independently cross-checked by
//! `tools/dsp-xref` so Sim and the core cannot silently co-confirm.
//!
//! Contract pinned by the integrator. Bodies marked `todo!()` are filled by the
//! DSP work with golden-vector tests; the trivial primitives are implemented here
//! so downstream crates and early tests have a foothold.
#![allow(dead_code)]

// File-backed modules added by the Sine + Recorder work. Registered here at the
// top of the file (stable region).
pub mod averaging;
pub mod calculus;
pub mod db;
pub mod decimate;
pub mod filter;
pub mod frf_ext;
pub mod regrid;
pub mod resample;
pub mod resonance;
pub mod spectrum_ext;
pub mod spectrum_offline;
pub mod stats_ext;
pub mod sweep;
pub mod tracking;
pub mod window_ext;
pub mod xcorr;

/// Engineering-unit conversion. The invariant across the whole system is raw volts;
/// each channel converts with its own sensitivity (mV per EU).
pub mod eu {
    /// `eu = volts / (sensitivity_mv_per_eu / 1000)`. Sensitivity is strictly
    /// per-channel; passing a shared value is a caller error.
    #[inline]
    pub fn volts_to_eu(volts: f64, sensitivity_mv_per_eu: f64) -> f64 {
        volts * 1000.0 / sensitivity_mv_per_eu
    }
}

/// Iterative radix-2 Cooley-Tukey FFT, pure std, no allocation in the transform
/// body beyond the twiddle scratch. This is the shared spectral primitive that
/// `spectrum` (and later `frf`/`cola`) build on.
///
/// Complex numbers are `(re, im)` tuples. The forward transform uses the
/// engineering sign convention `X[k] = sum_n x[n] * exp(-j*2*pi*k*n/N)`; the
/// inverse divides by `N`. Input length MUST be a power of two; use
/// [`fft::next_pow2`] / [`fft::zero_pad_pow2`] to condition arbitrary-length
/// signals first.
pub mod fft {
    /// Complex sample as `(re, im)`.
    pub type Cplx = (f64, f64);

    /// Smallest power of two `>= n` (with `next_pow2(0) == 1`).
    #[inline]
    pub fn next_pow2(n: usize) -> usize {
        if n <= 1 {
            return 1;
        }
        // usize::next_power_of_two is std and allocation-free.
        n.next_power_of_two()
    }

    /// Zero-pad a real signal up to the next power of two, returned as complex
    /// samples with zero imaginary part. If `x.len()` is already a power of two,
    /// the length is unchanged.
    pub fn zero_pad_pow2(x: &[f64]) -> Vec<Cplx> {
        let n = next_pow2(x.len());
        let mut out = Vec::with_capacity(n);
        out.extend(x.iter().map(|&v| (v, 0.0)));
        out.resize(n, (0.0, 0.0));
        out
    }

    /// Bit-reversal permutation in place. `buf.len()` must be a power of two.
    fn bit_reverse(buf: &mut [Cplx]) {
        let n = buf.len();
        if n <= 2 {
            return;
        }
        let mut j = 0usize;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j |= bit;
            if i < j {
                buf.swap(i, j);
            }
        }
    }

    /// In-place iterative Cooley-Tukey. `inverse == false` applies the forward
    /// (negative-exponent) transform; `inverse == true` applies the positive
    /// exponent WITHOUT the 1/N scale (the scale is applied by [`ifft`]).
    fn transform_in_place(buf: &mut [Cplx], inverse: bool) {
        let n = buf.len();
        assert!(n.is_power_of_two(), "fft length must be a power of two");
        if n < 2 {
            return;
        }
        bit_reverse(buf);
        // sign of the exponent: forward = -1, inverse = +1.
        let sign = if inverse { 1.0 } else { -1.0 };
        let mut len = 2usize;
        while len <= n {
            let half = len / 2;
            // principal twiddle for this stage: exp(sign * -j * 2*pi/len)
            // == cos(theta) + j*sin(theta) with theta = sign * -2*pi/len.
            let theta = sign * -std::f64::consts::TAU / len as f64;
            let (wlen_re, wlen_im) = (theta.cos(), theta.sin());
            let mut start = 0usize;
            while start < n {
                // running twiddle w = 1 + 0j
                let mut w_re = 1.0f64;
                let mut w_im = 0.0f64;
                for k in 0..half {
                    let a = buf[start + k];
                    let b = buf[start + k + half];
                    // t = w * b
                    let t_re = w_re * b.0 - w_im * b.1;
                    let t_im = w_re * b.1 + w_im * b.0;
                    buf[start + k] = (a.0 + t_re, a.1 + t_im);
                    buf[start + k + half] = (a.0 - t_re, a.1 - t_im);
                    // advance twiddle: w *= wlen
                    let nw_re = w_re * wlen_re - w_im * wlen_im;
                    let nw_im = w_re * wlen_im + w_im * wlen_re;
                    w_re = nw_re;
                    w_im = nw_im;
                }
                start += len;
            }
            len <<= 1;
        }
    }

    /// Forward FFT of complex input. Length must be a power of two.
    pub fn fft(x: &[Cplx]) -> Vec<Cplx> {
        let mut buf = x.to_vec();
        transform_in_place(&mut buf, false);
        buf
    }

    /// Inverse FFT of complex input (includes the 1/N normalization). Length
    /// must be a power of two.
    pub fn ifft(x: &[Cplx]) -> Vec<Cplx> {
        let mut buf = x.to_vec();
        transform_in_place(&mut buf, true);
        let inv_n = 1.0 / buf.len() as f64;
        for s in buf.iter_mut() {
            s.0 *= inv_n;
            s.1 *= inv_n;
        }
        buf
    }

    /// Forward FFT of a real signal, zero-padded to the next power of two.
    /// Convenience wrapper over [`zero_pad_pow2`] + [`fft`].
    pub fn rfft(x: &[f64]) -> Vec<Cplx> {
        fft(&zero_pad_pow2(x))
    }

    /// In-place forward FFT on a caller-owned power-of-two buffer, for the hot
    /// path where the buffer is reused across frames (no allocation here).
    pub fn fft_in_place(buf: &mut [Cplx]) {
        transform_in_place(buf, false);
    }

    /// Magnitude of a complex sample.
    #[inline]
    pub fn mag(c: Cplx) -> f64 {
        (c.0 * c.0 + c.1 * c.1).sqrt()
    }
}

/// Window functions.
pub mod window {
    /// Periodic Hann window of length `n` (DFT-correct; `w[k] = 0.5 - 0.5*cos(2*pi*k/n)`).
    pub fn hann(n: usize) -> Vec<f64> {
        if n == 0 {
            return Vec::new();
        }
        let tau = std::f64::consts::TAU;
        (0..n)
            .map(|k| 0.5 - 0.5 * (tau * k as f64 / n as f64).cos())
            .collect()
    }

    /// Amplitude-correction factor (coherent gain reciprocal) for a window.
    pub fn amplitude_correction(w: &[f64]) -> f64 {
        if w.is_empty() {
            return 1.0;
        }
        let sum: f64 = w.iter().sum();
        w.len() as f64 / sum
    }
}

/// Time-domain statistics used everywhere (RMS/peak overlays, abort checks).
pub mod stats {
    pub fn mean(x: &[f64]) -> f64 {
        if x.is_empty() {
            return 0.0;
        }
        x.iter().sum::<f64>() / x.len() as f64
    }

    /// True RMS.
    pub fn rms(x: &[f64]) -> f64 {
        if x.is_empty() {
            return 0.0;
        }
        (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt()
    }

    /// Max absolute value (peak).
    pub fn peak(x: &[f64]) -> f64 {
        x.iter().fold(0.0_f64, |acc, v| acc.max(v.abs()))
    }
}

/// Peak-preserving decimation for the live scope. The display is pixel-limited, so
/// the Rust core reduces each pixel column to a (min, max) pair. This is the pattern
/// that makes the web scope fast (see graphing study).
pub mod decimator {
    /// Reduce `x` to `cols` (min, max) pairs. If `x.len() <= cols`, pairs mirror the
    /// samples. Never drops a transient because both extremes per bin are kept.
    pub fn minmax(x: &[f64], cols: usize) -> Vec<(f64, f64)> {
        if cols == 0 || x.is_empty() {
            return Vec::new();
        }
        if x.len() <= cols {
            return x.iter().map(|&v| (v, v)).collect();
        }
        let n = x.len();
        (0..cols)
            .map(|c| {
                let lo = c * n / cols;
                let hi = ((c + 1) * n / cols).max(lo + 1).min(n);
                let slice = &x[lo..hi];
                let mut mn = slice[0];
                let mut mx = slice[0];
                for &v in &slice[1..] {
                    if v < mn {
                        mn = v;
                    }
                    if v > mx {
                        mx = v;
                    }
                }
                (mn, mx)
            })
            .collect()
    }
}

/// Spectral estimation (Welch PSD, single-sided). Body: DSP work + golden vectors.
pub mod spectrum {
    use super::fft;
    use super::window;

    /// Welch PSD estimate. Returns `(freqs_hz, psd)` single-sided, in EU^2/Hz.
    ///
    /// Method: split `signal` into `nfft`-length segments with `overlap`
    /// fractional overlap (0.0..1.0), apply a periodic Hann window to each,
    /// take the FFT, form the periodogram `|X[k]|^2`, and average over segments.
    ///
    /// Normalization is the density convention: each periodogram is divided by
    /// `fs * sum(w[n]^2)`, which corrects for the window's power loss and yields
    /// a one-sided density such that `integral(psd) df == variance(signal)`.
    /// Bins between DC and Nyquist are doubled (one-sided fold); DC (`k = 0`) and
    /// the Nyquist bin (`k = nfft/2`, present only for even `nfft`) are NOT
    /// doubled because they have no negative-frequency twin.
    ///
    /// Returns `nfft/2 + 1` frequency bins from 0 to `fs/2`. If the signal is
    /// shorter than one segment or `nfft < 2`, returns empty vectors.
    pub fn psd_welch(
        signal: &[f64],
        fs_hz: f64,
        nfft: usize,
        overlap: f64,
    ) -> (Vec<f64>, Vec<f64>) {
        if nfft < 2 || signal.len() < nfft || fs_hz <= 0.0 {
            return (Vec::new(), Vec::new());
        }
        // FFT requires a power-of-two length; the periodogram theory here assumes
        // segment length == transform length, so we require nfft to be a pow2.
        assert!(
            nfft.is_power_of_two(),
            "psd_welch: nfft must be a power of two (got {nfft})"
        );
        let overlap = overlap.clamp(0.0, 0.95);
        let step = ((nfft as f64) * (1.0 - overlap)).round().max(1.0) as usize;

        let w = window::hann(nfft);
        // window power S2 = sum(w[n]^2); density norm = 1 / (fs * S2).
        let s2: f64 = w.iter().map(|v| v * v).sum();
        let norm = 1.0 / (fs_hz * s2);

        let nout = nfft / 2 + 1;
        let mut psd = vec![0.0f64; nout];
        let mut seg = vec![(0.0f64, 0.0f64); nfft];
        let mut nseg = 0usize;

        let mut start = 0usize;
        while start + nfft <= signal.len() {
            // window into the reused complex scratch, removing per-segment mean
            // so a DC offset does not smear across low bins.
            let mut segmean = 0.0f64;
            for n in 0..nfft {
                segmean += signal[start + n];
            }
            segmean /= nfft as f64;
            for n in 0..nfft {
                seg[n] = ((signal[start + n] - segmean) * w[n], 0.0);
            }
            fft::fft_in_place(&mut seg);
            for (k, psd_k) in psd.iter_mut().enumerate() {
                let (re, im) = seg[k];
                let mut p = (re * re + im * im) * norm;
                // one-sided fold: double interior bins, not DC or Nyquist.
                let is_nyquist = k == nfft / 2; // only reached for even nfft
                if k != 0 && !is_nyquist {
                    p *= 2.0;
                }
                *psd_k += p;
            }
            nseg += 1;
            start += step;
        }
        if nseg == 0 {
            return (Vec::new(), Vec::new());
        }
        let inv = 1.0 / nseg as f64;
        for p in psd.iter_mut() {
            *p *= inv;
        }
        let df = fs_hz / nfft as f64;
        let freqs: Vec<f64> = (0..nout).map(|k| k as f64 * df).collect();
        (freqs, psd)
    }
}

/// Shock Response Spectrum: analysis (of a captured transient) and synthesis
/// (wavelet / damped-sine to a target SRS). Body: DSP work.
pub mod srs {
    /// SRS analysis by the ramp-invariant digital filter (Smallwood), maxi-max
    /// absolute acceleration.
    ///
    /// For each natural frequency `f_n` in `freqs_hz`, a single-degree-of-freedom
    /// (SDOF) system with quality factor `Q` (damping `zeta = 1/(2 Q)`) is driven
    /// by the base-acceleration record `accel`. The response returned per
    /// frequency is the maxi-max: `max_n |y[n]|`, the largest absolute value of
    /// the SDOF absolute-acceleration response over the whole record (this folds
    /// the positive and negative primary/residual peaks into one maximax curve).
    ///
    /// The recursion is Smallwood's ramp-invariant (piecewise-linear-input-exact)
    /// digital filter, the industry-standard SRS synthesis filter, which stays
    /// accurate even when `f_n` approaches Nyquist where the impulse-invariant
    /// filter degrades. Units of the output match the units of `accel` (g in, g
    /// out).
    ///
    /// Frequencies at or above Nyquist (`fs/2`) yield the input peak (the filter
    /// cannot resolve them) and are returned as `peak(accel)` rather than a NaN.
    pub fn srs_analyze(accel: &[f64], fs_hz: f64, freqs_hz: &[f64], q: f64) -> Vec<f64> {
        if accel.is_empty() || fs_hz <= 0.0 {
            return vec![0.0; freqs_hz.len()];
        }
        let zeta = 1.0 / (2.0 * q);
        let dt = 1.0 / fs_hz;
        let nyquist = fs_hz / 2.0;

        // input peak, used as the fallback above Nyquist.
        let input_peak = accel.iter().fold(0.0_f64, |m, &v| m.max(v.abs()));

        freqs_hz
            .iter()
            .map(|&fn_hz| {
                if fn_hz <= 0.0 {
                    // rigid body: response == input, maxi-max == input peak.
                    return input_peak;
                }
                if fn_hz >= nyquist {
                    return input_peak;
                }
                let wn = std::f64::consts::TAU * fn_hz;
                let wd = wn * (1.0 - zeta * zeta).sqrt();
                let e = (-zeta * wn * dt).exp();
                let k = wd * dt;
                let c = e * k.cos();
                let s = e * k.sin();
                // Sp = e * sin(wd*dt) / (wd*dt); guard tiny k (won't happen for
                // fn>0 below Nyquist, but keep it defensive).
                let sp = if k.abs() < 1e-12 { e } else { s / k };
                let e2 = e * e;

                // Smallwood ramp-invariant, absolute-acceleration form.
                let b0 = 1.0 - sp;
                let b1 = 2.0 * (sp - c);
                let b2 = e2 - sp;
                let a1 = -2.0 * c;
                let a2 = e2;

                // Direct-form-I recursion.
                let mut xm1 = 0.0f64;
                let mut xm2 = 0.0f64;
                let mut ym1 = 0.0f64;
                let mut ym2 = 0.0f64;
                let mut maxabs = 0.0f64;
                for &x in accel {
                    let y = b0 * x + b1 * xm1 + b2 * xm2 - a1 * ym1 - a2 * ym2;
                    let ay = y.abs();
                    if ay > maxabs {
                        maxabs = ay;
                    }
                    xm2 = xm1;
                    xm1 = x;
                    ym2 = ym1;
                    ym1 = y;
                }
                maxabs
            })
            .collect()
    }

    /// Fractional-octave band spacing for [`octave_freqs`]. The `N` in the
    /// geometric ratio `f_{k+1} = f_k * 2^(1/N)`: whole octave is `N = 1`, one-
    /// third octave is `N = 3`, etc. `Arbitrary(x)` uses `N = x` directly for
    /// non-standard densities (e.g. `Arbitrary(24.0)` for 1/24-octave).
    ///
    /// Fractional-octave band centers are the standard SRS analysis frequency grid
    /// (the SRS accel envelope is checked on a log octave grid). One-sixth and
    /// one-twelfth octave are the finer spacings some specs call for; whole and
    /// third are the common defaults.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum OctaveFraction {
        /// Whole octave (`N = 1`): ratio 2.0 between adjacent centers.
        Whole,
        /// One-third octave (`N = 3`): the classic SRS/acoustics default.
        Third,
        /// One-sixth octave (`N = 6`).
        Sixth,
        /// One-twelfth octave (`N = 12`).
        Twelfth,
        /// Arbitrary fraction: `N` is the value carried here (ratio `2^(1/N)`).
        Arbitrary(f64),
    }

    impl OctaveFraction {
        /// The `N` denominator of the geometric ratio `2^(1/N)`.
        #[inline]
        pub fn denom(self) -> f64 {
            match self {
                OctaveFraction::Whole => 1.0,
                OctaveFraction::Third => 3.0,
                OctaveFraction::Sixth => 6.0,
                OctaveFraction::Twelfth => 12.0,
                OctaveFraction::Arbitrary(n) => n,
            }
        }
    }

    /// Generate fractional-octave band-center frequencies from `f_lo_hz` to
    /// `f_hi_hz` (inclusive of coverage at both ends) with geometric spacing
    /// `f_{k+1} = f_k * 2^(1/N)`, where `N` is set by `fraction`.
    ///
    /// # Endpoint convention (documented)
    ///
    /// The series STARTS exactly at `f_lo_hz` (the low endpoint is always a band
    /// center) and steps geometrically upward. Generation stops at the first
    /// center that is `>= f_hi_hz`, and that center IS included, so the returned
    /// grid always brackets `[f_lo_hz, f_hi_hz]` (the last center is at or just
    /// above `f_hi_hz` unless the interval is an exact multiple of the ratio, in
    /// which case the last center equals `f_hi_hz`). This "cover the top" rule
    /// guarantees the requested high frequency is inside the analyzed band rather
    /// than falling in a gap above the last center. A single-point interval
    /// (`f_lo_hz == f_hi_hz`) returns just that frequency.
    ///
    /// Returns an empty vector for non-positive or reversed bounds
    /// (`f_lo_hz <= 0`, `f_hi_hz < f_lo_hz`) or a non-positive `N`.
    pub fn octave_freqs(f_lo_hz: f64, f_hi_hz: f64, fraction: OctaveFraction) -> Vec<f64> {
        let n = fraction.denom();
        if f_lo_hz <= 0.0 || f_hi_hz < f_lo_hz || n <= 0.0 || !f_lo_hz.is_finite() {
            return Vec::new();
        }
        let ratio = 2.0_f64.powf(1.0 / n);
        let mut out = Vec::new();
        let mut f = f_lo_hz;
        out.push(f);
        // Step up until a center reaches or exceeds f_hi_hz; include that center
        // so the top of the requested band is always covered. A tiny relative
        // slack absorbs float rounding so an interval that is an exact power of
        // the ratio does not emit a spurious extra band just above f_hi_hz.
        let slack = 1.0 + 1e-9;
        while f < f_hi_hz / slack {
            f *= ratio;
            out.push(f);
        }
        out
    }

    /// Pseudo-velocity Shock Response Spectrum (PVSS): for each natural frequency
    /// `f_n`, `PV = wn * max|z(t)|`, where `z` is the RELATIVE displacement of the
    /// base-excited SDOF and `wn = 2*pi*f_n`. Units are in/s when `accel_g` is in
    /// g (the input g is converted to in/s^2 via [`crate::integrate::G_TO_IN_S2`]
    /// so the relative displacement comes out in inches and `wn * z` in in/s - the
    /// in/g units of the PVSS output).
    ///
    /// # Method
    ///
    /// The SDOF relative-displacement equation for base acceleration input
    /// `a(t)` is `z'' + 2*zeta*wn*z' + wn^2*z = -a(t)` (damping `zeta = 1/(2 Q)`).
    /// This is the same SDOF that [`srs_analyze`] filters, but the quantity of
    /// interest is the RELATIVE displacement `z` rather than the absolute
    /// acceleration. It is advanced by the EXACT continuous state transition
    /// (`Phi = exp(A*h)` plus the zero-order-hold input map `A^{-1}(Phi - I)B`) on
    /// a linear-input-oversampled grid - a transcription-free equivalent of the
    /// Smallwood ramp-invariant (piecewise-linear-input-exact) SRS filter
    /// (Smallwood 1981; Irvine, "An Introduction to the Shock Response Spectrum").
    /// The maximax `max_n |z[n]|` is then scaled by `wn`. See
    /// [`max_abs_relative_displacement`] for the state-space derivation.
    ///
    /// Pseudo-velocity is the shock quantity most directly tied to stress: peak
    /// modal stress in many structures is proportional to peak modal
    /// pseudo-velocity (Gaberson). The PVSS is therefore the standard third
    /// output alongside the maximax-acceleration SRS.
    ///
    /// # Relationship to [`srs_analyze`] (documented consistency)
    ///
    /// Pseudo-acceleration is `PA = wn^2 * max|z| = wn * PV`. At light damping
    /// (large `Q`) the maximax ABSOLUTE acceleration returned by [`srs_analyze`]
    /// is dominated by the spring term `wn^2 * z` (the `2*zeta*wn*z'` term is small
    /// for large `Q`), so `PA` approaches the maximax-acceleration SRS. Converting
    /// back to velocity units, at high frequency the plateau relationship is
    /// `PV ~ maximax_accel_in_in_s2 / wn`, i.e. `srs_analyze` (in g, times
    /// G_TO_IN_S2) divided by `wn`. This equivalence is asserted as a golden gate.
    ///
    /// Edge cases mirror [`srs_analyze`]: an empty record or non-positive `fs`
    /// returns zeros; `f_n <= 0` (rigid body) has zero relative displacement so
    /// `PV = 0`; `f_n >= fs/2` (at or above Nyquist) cannot be resolved and
    /// returns `0.0` (the filter has no meaningful relative-displacement response
    /// there). The `q` handling matches: `zeta = 1/(2 Q)`.
    pub fn srs_pseudo_velocity(accel_g: &[f64], fs_hz: f64, freqs_hz: &[f64], q: f64) -> Vec<f64> {
        if accel_g.is_empty() || fs_hz <= 0.0 {
            return vec![0.0; freqs_hz.len()];
        }
        // Convert the base acceleration g -> in/s^2 so relative displacement is in
        // inches and wn*z is in in/s (in/g units, matching integrate::G_TO_IN_S2).
        let accel_in_s2: Vec<f64> = accel_g
            .iter()
            .map(|&g| g * crate::integrate::G_TO_IN_S2)
            .collect();
        let zeta = 1.0 / (2.0 * q);
        let dt = 1.0 / fs_hz;
        let nyquist = fs_hz / 2.0;

        freqs_hz
            .iter()
            .map(|&fn_hz| {
                if fn_hz <= 0.0 || fn_hz >= nyquist {
                    // rigid body has no relative displacement; above Nyquist the
                    // filter cannot resolve the mode.
                    return 0.0;
                }
                let wn = std::f64::consts::TAU * fn_hz;
                let maxabs = max_abs_relative_displacement(&accel_in_s2, wn, zeta, dt);
                wn * maxabs
            })
            .collect()
    }

    /// Maximax `|z|` of the RELATIVE displacement `z` of a base-excited SDOF
    /// `z'' + 2*zeta*wn*z' + wn^2*z = -a(t)`, with `a` the base acceleration in
    /// in/s^2 (so `z` comes out in inches).
    ///
    /// # Method: exact state-space step with linear-input oversampling
    ///
    /// Rather than transcribe a closed-form biquad numerator (error-prone), the
    /// SDOF is advanced with the EXACT continuous state transition. The 2x2 state
    /// is `[z, z']` and the plant is
    /// `A = [[0, 1], [-wn^2, -2*zeta*wn]]`, `B = [0, -1]^T` (the base acceleration
    /// forces `z'' = -a`). The homogeneous transition `Phi = exp(A*h)` for the
    /// underdamped oscillator has the standard closed form in terms of
    /// `e = exp(-zeta*wn*h)`, `wd = wn*sqrt(1-zeta^2)`, `sin(wd*h)`, `cos(wd*h)`,
    /// and the zero-order-hold (constant-input) forcing vector is the matrix
    /// identity `g0 = A^{-1}(Phi - I)B` (exact, no transcription). To capture the
    /// piecewise-LINEAR (ramp-invariant) accuracy that matters for smooth pulses -
    /// the property that names Smallwood's SRS filter "ramp-invariant" - each
    /// inter-sample interval is oversampled into `SUB` zero-order-hold sub-steps
    /// with the base acceleration linearly interpolated across them. Every term is
    /// a physical matrix-exponential element, so a sign or grouping slip cannot
    /// masquerade as a plausible biquad coefficient.
    ///
    /// The static (DC) relative displacement under a constant base accel `a0` is
    /// `z = -a0/wn^2`, which this transition reproduces exactly. Verified against
    /// physics by the golden gates: SDOF resonance gain ~ Q, and the low-frequency
    /// PVSS asymptote -> the pulse velocity change.
    fn max_abs_relative_displacement(accel_in_s2: &[f64], wn: f64, zeta: f64, dt: f64) -> f64 {
        // 8 zero-order-hold sub-steps with linear input interpolation makes the
        // residual model error negligible against the golden tolerances while
        // staying cheap and transcription-free.
        const SUB: usize = 8;
        let (phi00, phi01, phi10, phi11, g0_z, g0_v) = substep_zoh(wn, zeta, dt / SUB as f64);

        let n = accel_in_s2.len();
        let mut z = 0.0f64;
        let mut v = 0.0f64;
        let mut maxabs = z.abs();
        for i in 1..n {
            let a_prev = accel_in_s2[i - 1];
            let a_cur = accel_in_s2[i];
            for j in 0..SUB {
                // linear interpolation of the base accel at the sub-step start
                let frac = j as f64 / SUB as f64;
                let a_in = a_prev + (a_cur - a_prev) * frac;
                let nz = phi00 * z + phi01 * v + g0_z * a_in;
                let nv = phi10 * z + phi11 * v + g0_v * a_in;
                z = nz;
                v = nv;
                let az = z.abs();
                if az > maxabs {
                    maxabs = az;
                }
            }
        }
        maxabs
    }

    /// Exact zero-order-hold (piecewise-constant-input) state transition for the
    /// base-excited SDOF over a single sub-step of length `h`. Returns
    /// `(Phi00, Phi01, Phi10, Phi11, g0_z, g0_v)` where
    /// `[z'; v'] = Phi*[z; v] + g0*a` for constant base acceleration `a` (in/s^2)
    /// with plant `A = [[0,1],[-wn^2,-2*zeta*wn]]`, `B = [0,-1]^T`.
    ///
    /// `Phi = exp(A*h)` is the standard underdamped closed form; `g0 =
    /// A^{-1}(Phi - I)B` is the exact ZOH input map. Both are formed from
    /// matrix-exponential elements only - no hand-transcribed biquad coefficient.
    fn substep_zoh(wn: f64, zeta: f64, h: f64) -> (f64, f64, f64, f64, f64, f64) {
        let wd = wn * (1.0 - zeta * zeta).sqrt();
        let e = (-zeta * wn * h).exp();
        let c = (wd * h).cos();
        let s = (wd * h).sin();
        let zwn = zeta * wn;
        // Phi = exp(A*h), underdamped closed form.
        let phi00 = e * (c + (zwn / wd) * s);
        let phi01 = e * (s / wd);
        let phi10 = e * (-(wn * wn / wd) * s);
        let phi11 = e * (c - (zwn / wd) * s);
        // g0 = A^{-1}(Phi - I)B, with B = [0, -1]^T so (Phi - I)B = [-Phi01, -(Phi11-1)].
        // A^{-1} = [[ -2*zeta/wn, -1/wn^2 ], [ 1, 0 ]].
        let pmib0 = -phi01;
        let pmib1 = -(phi11 - 1.0);
        let g0_z = (-2.0 * zeta / wn) * pmib0 + (-1.0 / (wn * wn)) * pmib1;
        let g0_v = pmib0; // 1*pmib0 + 0*pmib1
        (phi00, phi01, phi10, phi11, g0_z, g0_v)
    }
}

/// Constant-overlap-add continuous random synthesis (Random control).
///
/// # Method
///
/// Continuous random vibration is synthesized frame by frame and stitched
/// together with the Weighted Overlap-Add (WOLA) method (Allen & Rabiner 1977;
/// Crochiere 1980). Each frame is an independent realization of a Gaussian random
/// process whose expected power spectral density matches a caller-supplied target
/// PSD. The frames are windowed and overlap-added so that consecutive frames join
/// with no gap and no amplitude modulation ("scalloping") at the frame boundary.
///
/// ## Per-frame spectrum draw
///
/// For a one-sided target density `S(f_k)` in `EU^2/Hz` at bin `k`, the desired
/// two-sided complex spectrum magnitude for an `N`-point DFT over sample rate
/// `fs` is
///
/// ```text
///   |X[k]| = sqrt( S(f_k) * fs * N / 2 ) * G_win
/// ```
///
/// where the `fs * N / 2` factor inverts the single-sided Welch density
/// normalization `1/(fs * S2)` combined with the one-sided ×2 fold (so that the
/// synthesized frame, re-analyzed by [`crate::spectrum::psd_welch`], returns the
/// target), and `G_win` compensates the synthesis-window power loss so the
/// overlap-added stream (not the pre-window frame) carries the target power. DC
/// (`k = 0`) and Nyquist (`k = N/2`) are real-valued and are NOT given the ×0.5
/// one-sided split; interior bins are.
///
/// Each bin gets an independent uniform-random phase in `[0, 2*pi)` and the
/// Hermitian-symmetric negative-frequency half is filled by conjugation so the
/// inverse FFT is purely real. The real and imaginary parts of each interior bin
/// are therefore independent zero-mean Gaussians (drawn via Box-Muller), which is
/// exactly the circularly-symmetric complex-Gaussian model of stationary random
/// vibration.
///
/// ## COLA / overlap-add
///
/// The time frame is `w[n] * ifft(X)`, where `w` is a periodic **square-root
/// Hann** synthesis window. For *random* (rather than deterministic) overlap-add
/// synthesis, the invariant that must hold to avoid amplitude modulation is the
/// SQUARED-window overlap sum
///
/// ```text
///   W2[p] = sum_m w[p - m*H]^2   (H = N*(1 - overlap))
/// ```
///
/// being constant across `p`, because independent random frames add in *variance*
/// (their cross terms average to zero), so the output variance at position `p` is
/// `sigma_frame^2 * W2[p]`. A plain Hann synthesis window gives `W2[p] = 0.5 +
/// 0.5*cos^2(2*pi*p/N)` at 50% overlap, a 3 dB frame-locked variance ripple. Using
/// `w = sqrt(hann)` makes `w^2 = hann`, and Hann itself satisfies the linear COLA
/// condition at 50% overlap (`hann[p] + hann[p+N/2] = 1`), so `W2` is exactly the
/// flat constant `1.0` and the random stream has no periodic amplitude modulation.
/// The squared-window COLA condition is verified at construction (see
/// [`cola::ColaSynth::squared_window_cola`]); a non-COLA overlap panics.
///
/// The synthesizer keeps an internal overlap tail so that each call to
/// [`cola::ColaSynth::next_frame`] emits exactly `hop` brand-new, gap-free output
/// samples that already include the contribution of every previous frame.
pub mod cola {
    use super::fft;
    use super::window;

    /// A seeded, allocation-light PRNG stream producing standard-normal samples
    /// by the Box-Muller transform over a xorshift128+ core. Deterministic given
    /// the seed, so synthesized frames are reproducible for golden tests.
    #[derive(Clone, Debug)]
    struct GaussRng {
        s0: u64,
        s1: u64,
        spare: Option<f64>,
    }

    impl GaussRng {
        fn new(seed: u64) -> Self {
            // SplitMix64 to disperse the seed into the two xorshift128+ words so a
            // small seed (e.g. 1) still gives a well-mixed state.
            let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut next = || {
                z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut x = z;
                x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                x ^ (x >> 31)
            };
            let s0 = next();
            let s1 = next() | 1; // never all-zero state
            Self {
                s0,
                s1,
                spare: None,
            }
        }

        /// xorshift128+ raw 64-bit output.
        #[inline]
        fn next_u64(&mut self) -> u64 {
            let mut x = self.s0;
            let y = self.s1;
            self.s0 = y;
            x ^= x << 23;
            x ^= x >> 17;
            x ^= y ^ (y >> 26);
            self.s1 = x;
            x.wrapping_add(y)
        }

        /// Uniform in the open interval `(0, 1)` (never exactly 0, so `ln` in
        /// Box-Muller is safe).
        #[inline]
        fn next_uniform(&mut self) -> f64 {
            // Take 53 bits for a double; add half an ULP so the result is in (0,1].
            let bits = self.next_u64() >> 11;
            (bits as f64 + 0.5) * (1.0 / (1u64 << 53) as f64)
        }

        /// Standard normal (mean 0, variance 1) via Box-Muller, caching the spare.
        #[inline]
        fn next_normal(&mut self) -> f64 {
            if let Some(s) = self.spare.take() {
                return s;
            }
            let u1 = self.next_uniform();
            let u2 = self.next_uniform();
            let r = (-2.0 * u1.ln()).sqrt();
            let theta = std::f64::consts::TAU * u2;
            self.spare = Some(r * theta.sin());
            r * theta.cos()
        }
    }

    /// A COLA frame synthesizer that produces gap-free output from a target PSD.
    ///
    /// Construct with [`ColaSynth::new`], then call [`ColaSynth::next_frame`]
    /// repeatedly, each call returning the next `hop = nfft*(1-overlap)` output
    /// samples for the supplied one-sided target PSD (`EU^2/Hz`, length
    /// `nfft/2 + 1`).
    pub struct ColaSynth {
        nfft: usize,
        hop: usize,
        fs_hz: f64,
        win: Vec<f64>,
        /// Overlap tail carried between frames (length `nfft`); the first `hop`
        /// samples are emitted next call, the rest is the running overlap.
        tail: Vec<f64>,
        rng: GaussRng,
    }

    impl ColaSynth {
        /// Build a synthesizer for `nfft`-point frames at fractional `overlap`
        /// (e.g. `0.5` for 50%) and sample rate `fs_hz`.
        ///
        /// `nfft` must be a power of two (>= 4). `overlap` must be one of the
        /// Hann-COLA-valid fractions; 0.5 and 0.75 are the standard choices and
        /// are verified at construction. Panics if the COLA condition is not met
        /// for the chosen window/overlap so a misconfiguration cannot silently
        /// produce amplitude ripple.
        pub fn new(nfft: usize, overlap: f64, fs_hz: f64) -> Self {
            Self::with_seed(nfft, overlap, fs_hz, 0xE1_6E_00_C0_1A_5E_ED_01)
        }

        /// As [`ColaSynth::new`] but with an explicit PRNG seed (used by tests to
        /// pin a reproducible stream, or by the caller to decorrelate parallel
        /// synthesizers).
        pub fn with_seed(nfft: usize, overlap: f64, fs_hz: f64, seed: u64) -> Self {
            assert!(
                nfft >= 4 && nfft.is_power_of_two(),
                "ColaSynth: nfft must be a power of two >= 4 (got {nfft})"
            );
            assert!(
                fs_hz > 0.0,
                "ColaSynth: fs_hz must be positive (got {fs_hz})"
            );
            assert!(
                (0.0..1.0).contains(&overlap),
                "ColaSynth: overlap must be in [0, 1) (got {overlap})"
            );
            let hop = ((nfft as f64) * (1.0 - overlap)).round() as usize;
            assert!(
                hop >= 1 && hop <= nfft,
                "ColaSynth: derived hop {hop} invalid"
            );

            // Synthesis window = SQUARE-ROOT Hann. For continuous *random* WOLA
            // synthesis the quantity that must be constant to avoid amplitude
            // modulation is the SQUARED-window overlap sum W2 = sum_m w[p-m*hop]^2
            // (the per-position variance-transfer factor), not the linear COLA
            // sum. With w = sqrt(hann), w^2 = hann, and hann satisfies the linear
            // COLA condition at 50% overlap (hann[p] + hann[p+N/2] = 1), so W2 is
            // exactly constant and the overlap-added random stream has NO
            // frame-locked variance ripple. (A plain Hann synthesis window would
            // give W2 = 0.5 + 0.5*cos^2(2*pi*p/N), a 3 dB variance ripple.)
            let win: Vec<f64> = window::hann(nfft).iter().map(|w| w.sqrt()).collect();

            // Verify the (squared-window) COLA condition: W2 must be near-constant.
            // If not, the overlap-added random stream would carry a periodic
            // amplitude (variance) ripple.
            let (w2_const, w2_ripple) = Self::cola_stats_sq(&win, hop);
            assert!(
                w2_ripple < 1e-6 * w2_const.max(1e-12),
                "ColaSynth: window/overlap fails squared-window COLA \
                 (W2 {w2_const:.6}, ripple {w2_ripple:.3e}); use 50% or 75% overlap"
            );

            Self {
                nfft,
                hop,
                fs_hz,
                win,
                tail: vec![0.0; nfft],
                rng: GaussRng::new(seed),
            }
        }

        /// Mean and peak-to-peak ripple, across one hop, of the shifted-overlap sum
        /// `sum_m v[p - m*hop]` for an arbitrary per-sample value array `v`. With
        /// `v = w` this is the linear COLA sum; with `v = w^2` it is the
        /// squared-window (variance-transfer) COLA sum.
        fn overlap_sum_stats(v: &[f64], hop: usize) -> (f64, f64) {
            let n = v.len();
            let mut acc = vec![0.0f64; hop];
            let nshift = n.div_ceil(hop) + 1;
            for (i, a) in acc.iter_mut().enumerate() {
                let mut s = 0.0;
                for m in 0..(2 * nshift) {
                    let idx = i as isize + (m as isize - nshift as isize) * hop as isize;
                    if idx >= 0 && (idx as usize) < n {
                        s += v[idx as usize];
                    }
                }
                *a = s;
            }
            let mean = acc.iter().sum::<f64>() / hop as f64;
            let mut lo = acc[0];
            let mut hi = acc[0];
            for &x in &acc {
                lo = lo.min(x);
                hi = hi.max(x);
            }
            (mean, hi - lo)
        }

        /// Mean and ripple of the SQUARED-window overlap sum `W2 = sum_m
        /// w[p-m*hop]^2` (the variance-transfer factor). For sqrt-Hann at 50%
        /// overlap this is the flat constant `1.0`.
        fn cola_stats_sq(w: &[f64], hop: usize) -> (f64, f64) {
            let sq: Vec<f64> = w.iter().map(|v| v * v).collect();
            Self::overlap_sum_stats(&sq, hop)
        }

        /// The linear COLA sum `sum_m w[p-m*hop]` for this synth's window/hop.
        /// Exposed so callers/tests can inspect the overlap structure. For the
        /// sqrt-Hann synthesis window this is NOT 1.0; the meaningful invariant
        /// for random synthesis is the squared-window sum
        /// [`Self::squared_window_cola`].
        pub fn cola_sum(&self) -> f64 {
            Self::overlap_sum_stats(&self.win, self.hop).0
        }

        /// The squared-window overlap-add constant `W2 = sum_m w[p - m*hop]^2`,
        /// the (constant, for a COLA window/hop) variance-transfer factor from an
        /// independent-per-frame random signal to the overlap-added stream. This
        /// is what actually sets the output power and must be flat to avoid a
        /// frame-locked variance ripple. For sqrt-Hann at 50% overlap it is `1.0`.
        pub fn squared_window_cola(&self) -> f64 {
            Self::cola_stats_sq(&self.win, self.hop).0
        }

        /// Hop size (number of new output samples returned per
        /// [`Self::next_frame`]).
        pub fn hop(&self) -> usize {
            self.hop
        }

        /// Frame size.
        pub fn nfft(&self) -> usize {
            self.nfft
        }

        /// Produce the next `hop` gap-free output samples for the given one-sided
        /// target PSD (`EU^2/Hz`, length `nfft/2 + 1`, indexed by bin `k` at
        /// frequency `k*fs/nfft`).
        ///
        /// The returned slice length is exactly [`ColaSynth::hop`]. Consecutive
        /// calls tile seamlessly: appending the outputs of successive calls yields
        /// a continuous stationary random stream whose Welch PSD matches `target`.
        ///
        /// # Edge-bin accuracy (DC and Nyquist) - documented limitation
        ///
        /// The recovered PSD matches `target` to within a fraction of a percent in
        /// the INTERIOR band. The two real band-edge bins (`k = 0` DC and, for even
        /// `nfft`, `k = nfft/2` Nyquist) are NOT power-accurate and callers must not
        /// place control energy there:
        ///
        /// - **DC (`k = 0`) is unobservable** through [`crate::spectrum::psd_welch`], which
        ///   removes the per-segment mean before transforming. Whatever DC power is
        ///   synthesized is discarded by the estimator, so DC density cannot be
        ///   verified or controlled through the Welch path by construction.
        /// - **Nyquist (`k = nfft/2`) carries a window-dependent bias.** The
        ///   sqrt-Hann synthesis window redistributes a single real edge bin
        ///   differently from an interior sinusoid, so there is no closed-form draw
        ///   that makes the re-analyzed Nyquist density equal `target[nfft/2]` for an
        ///   arbitrary analysis window. The edge bins are drawn at the
        ///   density-matched `s*C` (a real bin has no Hermitian twin, so it needs the
        ///   full interior power, not half), which is the principled value; the
        ///   residual is the irreducible window-leakage term.
        ///
        /// This is benign for real drive profiles, which roll off well before DC and
        /// Nyquist. It is a hard limitation only if a profile demands controlled
        /// energy in the first or last bin; do not rely on those two bins.
        pub fn next_frame(&mut self, target_psd: &[f64]) -> Vec<f64> {
            let n = self.nfft;
            let half = n / 2;
            assert_eq!(
                target_psd.len(),
                half + 1,
                "ColaSynth::next_frame: target_psd must have nfft/2+1 bins"
            );

            // Build the two-sided complex spectrum with the target magnitude and
            // random phase, drawing each bin's power so that the OVERLAP-ADDED
            // OUTPUT STREAM (not the pre-window frame) carries the target PSD.
            let mut spec = vec![(0.0f64, 0.0f64); n];

            // Exact normalization (derivation):
            //
            // Draw E|X[k]|^2 = target_psd[k] * C for interior bins (half power to
            // Re, half to Im). The real IFFT gives a frame whose per-sample
            // variance, for a flat target S0, is
            //     sigma_x^2 = (1/N^2) * sum_all_k E|X[k]|^2 = C * S0 / N
            // (interior bins counted twice via the Hermitian mirror -> sum ~ C*S0*N).
            // Independent windowed frames overlap-added give stream variance
            //     sigma_y^2 = sigma_x^2 * W2
            // where W2 = sum_m w[p - m*hop]^2 is the squared-window COLA constant.
            // The target flat one-sided PSD integrates to sigma_y^2 = S0 * (fs/2).
            // Solving sigma_x^2 * W2 = S0*fs/2 gives
            //     C = N * fs / (2 * W2).
            // So E|X[k]|^2 = target_psd[k] * N * fs / (2 * W2). This is exact for a
            // COLA window/hop (W2 constant) and is verified by the golden test that
            // re-analyzes the stream with Welch.
            let w2 = self.squared_window_cola();
            let c = n as f64 * self.fs_hz / (2.0 * w2);

            for k in 0..=half {
                let s = target_psd[k].max(0.0);
                let is_edge = k == 0 || (n.is_multiple_of(2) && k == half);
                // Interior bins: E|X|^2 = s*C, split evenly across Re/Im. An
                // interior bin k has a Hermitian twin at n-k, so the conjugate PAIR
                // contributes 2*s*C to the two-sided energy sum.
                //
                // Edge (DC/Nyquist) bins are real and have NO Hermitian twin: to
                // carry the same one-sided PSD DENSITY as an interior pair, a real
                // edge bin needs the full s*C (not s*C/2). The previous s*C*0.5 was
                // an aggregate-power argument, not a per-bin-density one, and left
                // the edge bins synthesized ~2x low in density; s*C is the
                // density-matched draw. See EDGE-BIN ACCURACY below and the
                // documented `next_frame` limitation: exact per-bin edge density is
                // not achievable in closed form because the sqrt-Hann synthesis
                // window redistributes a single real edge bin differently from an
                // interior sinusoid, so a residual, window/overlap-dependent edge
                // bias remains. Shaker random control profiles roll off at DC and
                // near Nyquist, so callers must not place control energy at the
                // band edges; edge fidelity is documented as not power-accurate.
                let e_mag2 = s * c;
                if is_edge {
                    let re = e_mag2.sqrt() * self.rng.next_normal();
                    spec[k] = (re, 0.0);
                } else {
                    // circularly-symmetric complex Gaussian: each component var =
                    // e_mag2/2 -> std = sqrt(e_mag2)/sqrt(2).
                    let sd = e_mag2.sqrt() * std::f64::consts::FRAC_1_SQRT_2;
                    let re = sd * self.rng.next_normal();
                    let im = sd * self.rng.next_normal();
                    spec[k] = (re, im);
                    spec[n - k] = (re, -im);
                }
            }

            // Inverse FFT -> real time frame, then apply the synthesis window.
            let time = fft::ifft(&spec);
            let mut frame: Vec<f64> = time.iter().map(|c| c.0).collect();
            for (f, &wn) in frame.iter_mut().zip(self.win.iter()) {
                *f *= wn;
            }

            // Overlap-add into the running tail.
            for (t, &f) in self.tail.iter_mut().zip(frame.iter()) {
                *t += f;
            }
            // Emit the first `hop` samples; shift the tail down by `hop`.
            let out: Vec<f64> = self.tail[..self.hop].to_vec();
            self.tail.copy_within(self.hop.., 0);
            for v in self.tail[n - self.hop..].iter_mut() {
                *v = 0.0;
            }
            out
        }
    }
}

/// Frequency-response estimation and drive estimation for closed-loop control.
///
/// The frequency response function (FRF) `H(f)` relates a drive input `x` to a
/// measured response `y` as `Y(f) = H(f) * X(f)`. Estimating `H` from noisy,
/// finite records is the core of adaptive random control: the controller inverts
/// the estimated plant to compute the drive that will realize a target response.
pub mod frf {
    use super::fft;
    use super::window;

    // NOTE: a general complex-division helper was deliberately removed. It was
    // unused and was a trap next to the cross-spectrum code: the Hv phase path uses
    // `<X*conj(Y)>`, NOT an `H1 = Gxy/Gxx` division, so a stray `cdiv` primitive
    // invited wiring in the wrong estimator. If an H1 = Gxy/Gxx path is added later,
    // introduce the divide at that call site with its own convention note.

    /// The `Hv` frequency-response estimate (Smallwood / SAND98-2044C lineage).
    ///
    /// # Method
    ///
    /// `H1 = Gxy / Gxx` is optimal when only the *output* `y` is noisy; `H2 =
    /// Gyy / Gyx` is optimal when only the *input* `x` is noisy. Both are biased
    /// when noise is on both channels. The `Hv` (total-least-squares / maximum-
    /// likelihood) estimator splits the difference and is the estimator Smallwood
    /// recommends for vibration control, where drive and response both carry
    /// measurement noise.
    ///
    /// For a single-input/single-output channel the `Hv` estimator reduces to the
    /// scalar total-least-squares solution whose magnitude is the geometric mean
    /// `|Hv| = sqrt(Gyy / Gxx)` and whose phase is the cross-spectrum phase
    /// `angle(Gxy)`. That is exactly `|H2*H1|^(1/2)` with `H1`'s phase, and it is
    /// what this function returns:
    ///
    /// ```text
    ///   Hv[k] = sqrt( Gyy[k] / Gxx[k] ) * exp( j * angle(Gxy[k]) )
    /// ```
    ///
    /// `Gxx = <|X|^2>`, `Gyy = <|Y|^2>`, and `Gxy = <X*conj(Y)>` are averaged
    /// (Welch) over overlapping Hann-windowed `nfft`-length blocks so the estimate
    /// has usable variance. Averaging is what makes the cross-spectrum phase
    /// meaningful; a single block gives `|Gxy| == sqrt(Gxx*Gyy)` identically
    /// (coherence 1) and no noise rejection.
    ///
    /// # Phase convention (load-bearing - do not "fix" to match intuition)
    ///
    /// This crate's forward FFT uses the negative exponent
    /// `X[k] = sum_n x[n] e^{-j2pi kn/N}`. Under that sign, the cross-spectrum that
    /// yields the correct z-domain / continuous FRF phase `angle(H(e^{jw}))` is
    /// `Gxy = <X*conj(Y)>`, so `angle(Hv) = angle(Gxy)`. This is the CONJUGATE of
    /// the `<conj(X)*Y>` form; the two differ only in the SIGN of the phase, and the
    /// sign that makes a lagging response come out with negative (lagging) phase is
    /// the `<X*conj(Y)>` one implemented below. Verified against the analytic biquad
    /// plant AND a pure integer-sample-delay plant in the test suite
    /// (`h_estimate_hv_phase_sign_pinned`). A future FFT-sign change or a "fix" to
    /// `<conj(X)*Y>` would negate every FRF phase and drive the closed loop the wrong
    /// way around resonances; the sign-pinning test exists to trip on exactly that.
    /// The magnitude `sqrt(Gyy/Gxx)` is convention-independent.
    ///
    /// # Magnitude bias at low SNR
    ///
    /// `|Hv| = sqrt(Gyy/Gxx)` is the geometric mean `sqrt(|H1|*|H2|)`. It is biased
    /// HIGH by output noise: with additive output noise of spectral density `Gnn`,
    /// `E|Hv|` scales roughly as `sqrt(1 + Gnn/Gxx)` (an H2-flavored magnitude, so
    /// output noise inflates it). The adaptive-random loop should budget this bias at
    /// low output SNR; it is within the in-band recover-test tolerance (15%).
    ///
    /// Returns complex `H` as `(re, im)` for bins `0 ..= nfft/2` (length
    /// `nfft/2 + 1`). Bins where `Gxx` is zero (no drive energy) return `(0, 0)`.
    pub fn h_estimate_hv(
        drive: &[f64],
        response: &[f64],
        _fs_hz: f64,
        nfft: usize,
    ) -> Vec<(f64, f64)> {
        assert!(
            nfft >= 2 && nfft.is_power_of_two(),
            "h_estimate_hv: nfft must be a power of two >= 2 (got {nfft})"
        );
        let nout = nfft / 2 + 1;
        if drive.len() < nfft || response.len() < nfft {
            return vec![(0.0, 0.0); nout];
        }
        let n = drive.len().min(response.len());
        let w = window::hann(nfft);
        // 50% overlap Welch averaging.
        let hop = (nfft / 2).max(1);

        let mut gxx = vec![0.0f64; nout]; // <|X|^2>
        let mut gyy = vec![0.0f64; nout]; // <|Y|^2>
        let mut gxy = vec![(0.0f64, 0.0f64); nout]; // <conj(X)*Y>
        let mut nseg = 0usize;

        let mut xbuf = vec![(0.0f64, 0.0f64); nfft];
        let mut ybuf = vec![(0.0f64, 0.0f64); nfft];

        let mut start = 0usize;
        while start + nfft <= n {
            for i in 0..nfft {
                xbuf[i] = (drive[start + i] * w[i], 0.0);
                ybuf[i] = (response[start + i] * w[i], 0.0);
            }
            fft::fft_in_place(&mut xbuf);
            fft::fft_in_place(&mut ybuf);
            for k in 0..nout {
                let (xr, xi) = xbuf[k];
                let (yr, yi) = ybuf[k];
                gxx[k] += xr * xr + xi * xi;
                gyy[k] += yr * yr + yi * yi;
                // Cross spectrum Gxy = <X * conj(Y)> so that angle(Gxy) equals the
                // z-domain FRF phase angle(H(e^{jw})) under this crate's
                // negative-exponent forward FFT (X[k] = sum x[n] e^{-j2pi kn/N}).
                // With that FFT sign, a positive-frequency component of y that
                // lags x shows up with the sign that <X*conj(Y)> (not <conj(X)*Y>)
                // maps onto the continuous/z-domain H phase; verified against the
                // analytic biquad transfer function in tests. The magnitude is
                // sqrt(Gyy/Gxx) regardless of this phase convention.
                // X * conj(Y) = (xr + j xi)(yr - j yi):
                gxy[k].0 += xr * yr + xi * yi;
                gxy[k].1 += xi * yr - xr * yi;
            }
            nseg += 1;
            start += hop;
        }
        if nseg == 0 {
            return vec![(0.0, 0.0); nout];
        }

        (0..nout)
            .map(|k| {
                if gxx[k] <= 0.0 {
                    return (0.0, 0.0);
                }
                let mag = (gyy[k] / gxx[k]).sqrt();
                // phase of the (averaged) cross-spectrum.
                let phase = gxy[k].1.atan2(gxy[k].0);
                (mag * phase.cos(), mag * phase.sin())
            })
            .collect()
    }

    /// As [`h_estimate_hv`], but ADDITIONALLY returns the per-bin ordinary
    /// coherence `gamma^2 = |Gxy|^2 / (Gxx * Gyy)`.
    ///
    /// This is an additive coherence API: `hertzforge-dsp` is a pinned-interface crate,
    /// so a new coherence output is added as a NEW function alongside
    /// [`h_estimate_hv`] rather than by changing the existing estimator's
    /// signature. The FRF vector returned here is bit-identical to
    /// [`h_estimate_hv`] for the same inputs (same Welch accumulators, same
    /// `Gxy = <X*conj(Y)>` phase convention - see that function's phase-convention
    /// note, which is load-bearing).
    ///
    /// # Coherence
    ///
    /// Ordinary coherence measures the fraction of output power at each bin that is
    /// linearly explained by the input:
    ///
    /// ```text
    ///   gamma^2[k] = |Gxy[k]|^2 / (Gxx[k] * Gyy[k])   in [0, 1]
    /// ```
    ///
    /// with `Gxx = <|X|^2>`, `Gyy = <|Y|^2>`, `Gxy = <X*conj(Y)>` averaged over the
    /// same overlapping Hann blocks as the FRF. Interpretation:
    ///
    /// - `gamma^2 -> 1`: the response is fully explained by the drive (clean,
    ///   linear, low-noise) - the FRF estimate at that bin is trustworthy.
    /// - `gamma^2 -> 0`: the response is uncorrelated with the drive (extraneous
    ///   noise, an unmeasured input, or nonlinearity) - the FRF is unreliable there.
    ///
    /// A closed-loop random controller uses coherence as the per-bin measurement
    /// TRUST gate: a bin whose coherence is below a floor is held (its
    /// shape-trim update is skipped) rather than chased into noise.
    ///
    /// # Averaging caveat (mirror of the FRF magnitude-bias note)
    ///
    /// Coherence is only meaningful WITH averaging. A single block gives
    /// `|Gxy|^2 == Gxx*Gyy` identically, so `gamma^2 == 1` at every bin regardless
    /// of the actual SNR (the same no-averaging degeneracy that makes the
    /// single-block cross-spectrum phase meaningless). With `>= 2` overlapping
    /// blocks the estimate becomes informative; more blocks reduce its variance.
    ///
    /// # Zero-denominator guard
    ///
    /// Bins with no drive energy (`Gxx == 0`) or no response energy (`Gyy == 0`)
    /// have an undefined coherence ratio; they return coherence `0.0` (untrusted)
    /// AND FRF `(0, 0)`, matching [`h_estimate_hv`]'s zero-drive behavior. The
    /// returned coherence is clamped to `[0, 1]` to absorb floating-point rounding
    /// at the `gamma^2 == 1` degenerate case.
    ///
    /// Returns `(h, coherence)`, both of length `nfft/2 + 1`.
    pub fn h_estimate_hv_with_coherence(
        drive: &[f64],
        response: &[f64],
        _fs_hz: f64,
        nfft: usize,
    ) -> (Vec<(f64, f64)>, Vec<f64>) {
        assert!(
            nfft >= 2 && nfft.is_power_of_two(),
            "h_estimate_hv_with_coherence: nfft must be a power of two >= 2 (got {nfft})"
        );
        let nout = nfft / 2 + 1;
        if drive.len() < nfft || response.len() < nfft {
            return (vec![(0.0, 0.0); nout], vec![0.0; nout]);
        }
        let n = drive.len().min(response.len());
        let w = window::hann(nfft);
        // 50% overlap Welch averaging - identical to `h_estimate_hv`.
        let hop = (nfft / 2).max(1);

        let mut gxx = vec![0.0f64; nout]; // <|X|^2>
        let mut gyy = vec![0.0f64; nout]; // <|Y|^2>
        let mut gxy = vec![(0.0f64, 0.0f64); nout]; // <X*conj(Y)>
        let mut nseg = 0usize;

        let mut xbuf = vec![(0.0f64, 0.0f64); nfft];
        let mut ybuf = vec![(0.0f64, 0.0f64); nfft];

        let mut start = 0usize;
        while start + nfft <= n {
            for i in 0..nfft {
                xbuf[i] = (drive[start + i] * w[i], 0.0);
                ybuf[i] = (response[start + i] * w[i], 0.0);
            }
            fft::fft_in_place(&mut xbuf);
            fft::fft_in_place(&mut ybuf);
            for k in 0..nout {
                let (xr, xi) = xbuf[k];
                let (yr, yi) = ybuf[k];
                gxx[k] += xr * xr + xi * xi;
                gyy[k] += yr * yr + yi * yi;
                // Same cross-spectrum convention as `h_estimate_hv`:
                // Gxy = <X*conj(Y)> = <(xr + j xi)(yr - j yi)>.
                gxy[k].0 += xr * yr + xi * yi;
                gxy[k].1 += xi * yr - xr * yi;
            }
            nseg += 1;
            start += hop;
        }
        if nseg == 0 {
            return (vec![(0.0, 0.0); nout], vec![0.0; nout]);
        }

        let mut h = vec![(0.0f64, 0.0f64); nout];
        let mut coherence = vec![0.0f64; nout];
        for k in 0..nout {
            // Guard BOTH denominators: a zero Gxx (no drive) or zero Gyy (no
            // response) leaves coherence 0 and H (0,0) - matching h_estimate_hv's
            // zero-drive contract and giving the trust gate an "untrusted" bin.
            if gxx[k] <= 0.0 || gyy[k] <= 0.0 {
                continue;
            }
            let mag = (gyy[k] / gxx[k]).sqrt();
            let phase = gxy[k].1.atan2(gxy[k].0);
            h[k] = (mag * phase.cos(), mag * phase.sin());
            let cross_mag2 = gxy[k].0 * gxy[k].0 + gxy[k].1 * gxy[k].1;
            coherence[k] = (cross_mag2 / (gxx[k] * gyy[k])).clamp(0.0, 1.0);
        }
        (h, coherence)
    }

    /// Tikhonov-regularized inverse of an FRF: `Hinv = conj(H) / (|H|^2 + eps)`.
    ///
    /// # Method
    ///
    /// The naive inverse `1/H` blows up at anti-resonances where `|H| -> 0`,
    /// which would command an enormous drive and destabilize the loop. The
    /// Tikhonov (ridge) regularized inverse
    ///
    /// ```text
    ///   Hinv = conj(H) / (|H|^2 + epsilon)
    /// ```
    ///
    /// is the minimum-norm solution of `min |H*Hinv - 1|^2 + epsilon*|Hinv|^2`.
    /// Where `|H|^2 >> epsilon` (in-band, near resonances) it is `~1/H` and
    /// `H*Hinv ~= 1`. Where `|H|^2 << epsilon` (anti-resonances) the denominator
    /// is floored by `epsilon`, so `|Hinv|` peaks at `1/(2*sqrt(epsilon))` instead
    /// of diverging, and `H*Hinv -> 0` (the controller simply does not fight a
    /// notch it cannot drive). The transition happens at `|H| ~ sqrt(epsilon)`.
    ///
    /// ## Choosing epsilon
    ///
    /// `epsilon` has units of `|H|^2`. Pick it as `(beta * Hmax)^2` where `Hmax`
    /// is the peak plant gain in band and `beta` is the fraction of that peak
    /// below which the plant is treated as un-driveable (a typical `beta` is
    /// `1e-2 .. 1e-1`, i.e. attenuate 40..20 dB below the peak). This bounds the
    /// inverse gain to `1/(2*beta*Hmax)` regardless of how deep the notch goes.
    /// This function takes `epsilon` directly so the caller owns that policy;
    /// [`suggest_epsilon`] computes the `(beta*Hmax)^2` heuristic.
    ///
    /// Returns `Hinv` as `(re, im)`, same length as `h`.
    pub fn h_inverse_regularized(h: &[(f64, f64)], epsilon: f64) -> Vec<(f64, f64)> {
        let eps = epsilon.max(0.0);
        h.iter()
            .map(|&(re, im)| {
                let den = re * re + im * im + eps;
                if den == 0.0 {
                    return (0.0, 0.0);
                }
                // conj(H)/den = (re - j im)/den
                (re / den, -im / den)
            })
            .collect()
    }

    /// Suggest a Tikhonov `epsilon = (beta * Hmax)^2` from an FRF and a floor
    /// fraction `beta` (e.g. `0.03` = treat plant gain 30 dB below peak as a
    /// notch). Returns `0` for an empty FRF.
    pub fn suggest_epsilon(h: &[(f64, f64)], beta: f64) -> f64 {
        let hmax2 = h
            .iter()
            .map(|&(re, im)| re * re + im * im)
            .fold(0.0f64, f64::max);
        beta * beta * hmax2
    }
}

/// Sine control: real-time narrowband tracking filter + amplitude servo.
pub mod sine {
    /// Heterodyne / lock-in tracking filter that extracts response amplitude and
    /// phase at the instantaneous drive frequency as it sweeps.
    ///
    /// # Method
    ///
    /// This is a digital lock-in amplifier. The input `x[n]` is multiplied by a
    /// reference quadrature pair at the instantaneous drive frequency:
    ///
    /// ```text
    ///   I[n] = x[n] * cos(phi[n])
    ///   Q[n] = x[n] * sin(phi[n])
    /// ```
    ///
    /// where `phi[n]` is the running reference phase advanced by
    /// `2*pi*f_inst/fs` each sample. For a response `A*sin(phi + theta)`, the
    /// products are
    ///
    /// ```text
    ///   I = (A/2)[ sin(theta) + sin(2 phi + theta) ]
    ///   Q = (A/2)[ cos(theta) - cos(2 phi + theta) ] ... (sign per convention)
    /// ```
    ///
    /// The wanted terms are the DC (baseband) components `(A/2) sin(theta)` and
    /// `(A/2) cos(theta)`; the `2*phi` terms are at twice the drive frequency. A
    /// low-pass filter removes the `2f` image and any other tones, leaving a
    /// complex baseband phasor `I_lp + j Q_lp` whose magnitude is `A/2` and whose
    /// angle is the response phase relative to the drive. Amplitude is recovered
    /// as `2*sqrt(I_lp^2 + Q_lp^2)`.
    ///
    /// The low-pass is a matched pair of one-pole IIR filters with corner
    /// `bandwidth_hz`. The corner sets the classic sine-control trade: a narrow
    /// band rejects noise and the `2f` image well but responds slowly to sweep
    /// (long settling), a wide band tracks a fast sweep but is noisier. Choose
    /// `bandwidth_hz` a few times larger than the sweep rate seen at the tracking
    /// filter but well below `2 * f_min` so the image is still rejected.
    ///
    /// Because the reference phase is advanced by the *instantaneous* frequency
    /// passed to each [`TrackingFilter::update`] call, the demodulation stays
    /// locked as the drive sweeps; the baseband phasor simply follows the
    /// amplitude/phase envelope.
    ///
    /// # Phase accuracy during a sweep (readout caveat)
    ///
    /// The returned PHASE is accurate only in the quasi-static limit (sweep rate
    /// well below the low-pass bandwidth). The two-pole baseband low-pass has group
    /// delay of about two pole time-constants (`~2/(2*pi*bw)`), and on a swept tone
    /// that group delay maps to a systematic phase-readout lag proportional to
    /// sweep-rate / bandwidth. In practice a fast log sweep can bias the phase output
    /// by ~10-14 degrees. The AMPLITUDE is essentially unaffected (magnitude is
    /// insensitive to the small phase lag), so amplitude control and the
    /// [`AmplitudeServo`] are safe; only downstream consumers of the phase (e.g. a
    /// resonance-dwell or MIMO cross-channel-phase routine) inherit the lag. If phase
    /// accuracy matters during a sweep, size `bandwidth_hz` for the required phase
    /// tolerance (not just amplitude settling) or compensate the known group delay
    /// using the sweep rate and bandwidth available at [`TrackingFilter::update`].
    #[derive(Clone, Debug)]
    pub struct TrackingFilter {
        fs_hz: f64,
        /// One-pole low-pass smoothing coefficient `alpha = 1 - exp(-2*pi*bw/fs)`.
        alpha: f64,
        /// Running reference phase (radians), advanced each sample.
        ref_phase: f64,
        /// Two cascaded one-pole low-pass stages per baseband channel (I and Q).
        /// A single pole leaves ~1/(2f/bw) of the demodulation image; cascading
        /// two poles squares that rejection, which removes the amplitude bias the
        /// 2f image otherwise beats into the magnitude estimate.
        i_lp1: f64,
        i_lp2: f64,
        q_lp1: f64,
        q_lp2: f64,
    }

    impl TrackingFilter {
        /// Build a tracking filter at sample rate `fs_hz` with low-pass corner
        /// `bandwidth_hz`. The baseband low-pass is a two-pole cascade at that
        /// corner (see field docs) for strong rejection of the `2*f` demodulation
        /// image.
        pub fn new(fs_hz: f64, bandwidth_hz: f64) -> Self {
            assert!(fs_hz > 0.0, "TrackingFilter: fs_hz must be positive");
            assert!(
                bandwidth_hz > 0.0 && bandwidth_hz < fs_hz / 2.0,
                "TrackingFilter: bandwidth must be in (0, fs/2)"
            );
            // One-pole low-pass: y += alpha*(x - y), with
            //   alpha = 1 - exp(-2*pi*fc/fs)  (matched-z / impulse-invariant pole).
            let alpha = 1.0 - (-std::f64::consts::TAU * bandwidth_hz / fs_hz).exp();
            Self {
                fs_hz,
                alpha,
                ref_phase: 0.0,
                i_lp1: 0.0,
                i_lp2: 0.0,
                q_lp1: 0.0,
                q_lp2: 0.0,
            }
        }

        /// Reset the filter state (phasor and reference phase) to zero.
        pub fn reset(&mut self) {
            self.ref_phase = 0.0;
            self.i_lp1 = 0.0;
            self.i_lp2 = 0.0;
            self.q_lp1 = 0.0;
            self.q_lp2 = 0.0;
        }

        /// The per-pole low-pass smoothing coefficient (exposed for settling-time
        /// budgets). The cascade uses two of these poles.
        pub fn alpha(&self) -> f64 {
            self.alpha
        }

        /// Change the low-pass corner to `bandwidth_hz` WITHOUT touching the
        /// baseband state (`i_lp*`/`q_lp*`) or the reference phase.
        ///
        /// This is the state-preserving retune used by an adaptive/proportional
        /// bandwidth wrapper (see `crate::tracking::AdaptiveTrackingFilter`): as
        /// the drive sweeps, the wrapper re-derives the corner from the
        /// instantaneous frequency and calls this per move. Because only `alpha`
        /// is recomputed and the low-pass accumulators are preserved, the
        /// amplitude/phase readout has no dropout or glitch at the retune - the
        /// settled baseband phasor is a fixed point of the one-pole update for
        /// ANY alpha, so a retune on a settled tone leaves the output unchanged
        /// and only the response speed to future changes moves.
        ///
        /// Panics on the same domain rule as [`TrackingFilter::new`]:
        /// `bandwidth_hz` must lie in `(0, fs/2)`.
        pub fn set_bandwidth(&mut self, bandwidth_hz: f64) {
            assert!(
                bandwidth_hz > 0.0 && bandwidth_hz < self.fs_hz / 2.0,
                "TrackingFilter: bandwidth must be in (0, fs/2)"
            );
            self.alpha = 1.0 - (-std::f64::consts::TAU * bandwidth_hz / self.fs_hz).exp();
        }

        /// Feed one sample at instantaneous drive frequency `inst_freq_hz`.
        /// Returns `(amplitude, phase_rad)` of the response at that frequency,
        /// where `amplitude` is the peak amplitude `A` (not RMS) and `phase_rad`
        /// is the response phase relative to a sine reference, in `(-pi, pi]`.
        pub fn update(&mut self, sample: f64, inst_freq_hz: f64) -> (f64, f64) {
            // Demodulate against the current reference phase.
            let c = self.ref_phase.cos();
            let s = self.ref_phase.sin();
            // Project onto sine (I) and cosine (Q). Using sin as the phase
            // reference means phase 0 => response in phase with sin(phi).
            let i = sample * s;
            let q = sample * c;
            // Two-pole (cascaded one-pole) low-pass on each baseband channel.
            self.i_lp1 += self.alpha * (i - self.i_lp1);
            self.i_lp2 += self.alpha * (self.i_lp1 - self.i_lp2);
            self.q_lp1 += self.alpha * (q - self.q_lp1);
            self.q_lp2 += self.alpha * (self.q_lp1 - self.q_lp2);

            // Advance the reference phase by the instantaneous frequency AFTER
            // demodulating this sample (so the first sample uses phase 0).
            self.ref_phase += std::f64::consts::TAU * inst_freq_hz / self.fs_hz;
            if self.ref_phase > std::f64::consts::TAU {
                self.ref_phase -= std::f64::consts::TAU;
            }

            // Baseband phasor magnitude is A/2 for a sine of amplitude A.
            let (ib, qb) = (self.i_lp2, self.q_lp2);
            let amp = 2.0 * (ib * ib + qb * qb).sqrt();
            let phase = qb.atan2(ib);
            (amp, phase)
        }
    }

    /// Amplitude servo (compression) that holds the control channel at target
    /// using proportional/integral feedback with dB-domain rate limiting, plus an
    /// optional `H^-1` feedforward for startup.
    ///
    /// # Method
    ///
    /// Sine "compression" control holds a control-accelerometer amplitude at a
    /// target by adjusting a drive scale. The loop works in the dB (log) domain
    /// because the plant gain multiplies the drive: in dB the plant is an additive
    /// offset, so a fixed-gain controller behaves uniformly across a wide dynamic
    /// range and the "rate of change" limit is naturally expressed in dB/s (the
    /// standard sweep-rate safety limit in sine testing).
    ///
    /// Let `e = 20*log10(target / measured)` be the error in dB. The controller
    /// integrates the error with an integral gain and adds a proportional term:
    ///
    /// ```text
    ///   step_db      = kp * e + ki * integral(e) dt      (PI in dB)
    ///   step_db      = clamp(step_db, -rate*dt, +rate*dt)   (dB-rate limit)
    ///   level_db    += step_db
    ///   drive_scale  = 10^(level_db / 20)
    /// ```
    ///
    /// The dB-rate clamp `max_rate_db_per_s * dt` bounds how fast the drive can move
    /// per second, so a bad measurement or a large initial error produces a
    /// controlled ramp rather than a runaway step.
    ///
    /// # Stability and safety - what the guards do and do NOT guarantee
    ///
    /// A rate limit bounds the DERIVATIVE of the drive, not its VALUE. On its own it
    /// does NOT prevent divergence: a sustained low/zero measurement (e.g. a
    /// control-accelerometer cable disconnect, the most common field fault) would
    /// ramp the drive up at the rate limit indefinitely. Two additional guards make
    /// the loop safe:
    ///
    /// - **Absolute drive ceiling** (`max_level_db`, set via
    ///   [`AmplitudeServo::with_drive_ceiling`]). `level_db` is clamped to this
    ///   ceiling after every update, so the commanded `drive_scale` can never exceed
    ///   `10^(max_level_db/20)` no matter how long a fault persists.
    /// - **Fault hold on implausible measurement.** When `measured` is below
    ///   `target * fault_fraction` (default `1e-3`), the update treats it as a sensor
    ///   fault and FREEZES `level_db` (returns the current drive unchanged) instead
    ///   of ramping up into the disconnected channel. [`AmplitudeServo::faulted`]
    ///   reports the latched state so the engine's abort watchdog can trip on it.
    ///
    /// Convergence is monotonic in dB ONLY against an instantaneous-gain plant.
    /// Against any real (dynamic, lagged) plant the loop overshoots and rings; the
    /// overshoot is bounded by the rate limit but is NOT zero. Callers driving a
    /// physical shaker must budget overshoot on the way up as an over-test risk.
    ///
    /// ## Feedforward (startup)
    ///
    /// If the plant gain `g = measured/drive_scale` at the current frequency is
    /// known (from an `H^-1` estimate), the servo can be *seeded* with the drive
    /// that would put the response on target in one shot, via
    /// [`AmplitudeServo::set_feedforward_from_gain`] (or
    /// [`AmplitudeServo::set_feedforward_scale`]), so the loop starts near the answer
    /// instead of ramping up from zero. These setters write `level_db` directly and
    /// bypass the rate limit; they are startup-only and must not be called mid-sweep.
    /// The feedback then only trims the residual plant-estimate error.
    #[derive(Clone, Debug)]
    pub struct AmplitudeServo {
        target: f64,
        max_rate_db_per_s: f64,
        kp: f64,
        ki: f64,
        /// Current commanded drive level, in dB (`drive_scale = 10^(level/20)`).
        level_db: f64,
        /// Previous dB error, for the velocity-form proportional increment.
        prev_e_db: f64,
        /// Absolute ceiling on `level_db` (safety bound on drive_scale). Defaults to
        /// a generous but finite value so an un-configured servo still cannot run
        /// away; set explicitly via [`AmplitudeServo::with_drive_ceiling`].
        max_level_db: f64,
        /// Measurement below `target * fault_fraction` is treated as a sensor fault
        /// (drive frozen), not a demand for more drive.
        fault_fraction: f64,
        /// Latched fault state: set when a faulted (implausible) measurement was seen
        /// on the most recent update. Cleared when a plausible measurement returns.
        faulted: bool,
    }

    impl AmplitudeServo {
        /// Build a servo holding the control amplitude at `target` (same EU as the
        /// measured amplitude) with a maximum drive slew of `max_rate_db_per_s`
        /// dB per second. Uses sensible default PI gains; tune with
        /// [`AmplitudeServo::with_gains`].
        pub fn new(target: f64, max_rate_db_per_s: f64) -> Self {
            assert!(target > 0.0, "AmplitudeServo: target must be positive");
            assert!(
                max_rate_db_per_s > 0.0,
                "AmplitudeServo: max_rate_db_per_s must be positive"
            );
            Self {
                target,
                max_rate_db_per_s,
                // Default: mostly-integral loop. kp gives a little lead; ki drives
                // the steady-state error to zero. Both are well within the range
                // where the rate limiter dominates transient behavior.
                kp: 0.3,
                ki: 2.0,
                level_db: 0.0, // drive_scale starts at 1.0 (0 dB) unless seeded.
                prev_e_db: 0.0,
                // Default ceiling: +60 dB = 1000x the 0 dB start. Generous headroom
                // for legitimate low-gain plants, but still a hard finite bound so an
                // un-configured servo cannot run away on a stuck measurement. Callers
                // driving hardware should set this from the amplifier/shaker limits.
                max_level_db: 60.0,
                fault_fraction: 1e-3,
                faulted: false,
            }
        }

        /// Override the PI gains (`kp`, `ki` in dB-domain, `ki` in per-second).
        pub fn with_gains(mut self, kp: f64, ki: f64) -> Self {
            self.kp = kp;
            self.ki = ki;
            self
        }

        /// Set the absolute drive ceiling as a maximum `drive_scale` (`> 0`). The
        /// commanded drive can never exceed this value regardless of the measurement
        /// or how long a fault persists; this is the value bound that the rate limit
        /// alone does not provide. Panics if `max_scale <= 0`.
        pub fn with_drive_ceiling(mut self, max_scale: f64) -> Self {
            assert!(
                max_scale > 0.0,
                "AmplitudeServo: drive ceiling must be positive"
            );
            self.max_level_db = 20.0 * max_scale.log10();
            self
        }

        /// Set the fault threshold as a fraction of `target`: a measured amplitude
        /// below `target * fraction` is treated as a sensor fault (drive frozen).
        /// `fraction` in `(0, 1)`. Panics otherwise.
        pub fn with_fault_fraction(mut self, fraction: f64) -> Self {
            assert!(
                fraction > 0.0 && fraction < 1.0,
                "AmplitudeServo: fault_fraction must be in (0, 1)"
            );
            self.fault_fraction = fraction;
            self
        }

        /// Maximum commanded drive scale (the absolute ceiling).
        pub fn max_drive_scale(&self) -> f64 {
            10.0f64.powf(self.max_level_db / 20.0)
        }

        /// Whether the most recent [`AmplitudeServo::update`] saw an implausible
        /// (faulted) measurement and froze the drive. The engine abort watchdog can
        /// trip on this: a persistent fault means the control channel is dead.
        pub fn faulted(&self) -> bool {
            self.faulted
        }

        /// Seed the starting drive level directly (feedforward). `scale` is the
        /// initial `drive_scale` (>0). Call once before the first
        /// [`AmplitudeServo::update`].
        pub fn set_feedforward_scale(&mut self, scale: f64) {
            if scale > 0.0 {
                self.level_db = 20.0 * scale.log10();
            }
        }

        /// Seed the starting level from a known plant gain `g` (`measured =
        /// g * drive_scale`): the one-shot drive to hit target is
        /// `target/g`, i.e. level `20*log10(target/g)`. This is the `H^-1`
        /// feedforward at the current frequency.
        pub fn set_feedforward_from_gain(&mut self, plant_gain: f64) {
            if plant_gain > 0.0 {
                self.set_feedforward_scale(self.target / plant_gain);
            }
        }

        /// Current commanded drive scale.
        pub fn drive_scale(&self) -> f64 {
            10.0f64.powf(self.level_db / 20.0)
        }

        /// Tick the servo WITHOUT moving the drive: freeze integration while an
        /// external governor is binding, and return the (unchanged) drive scale.
        ///
        /// # Why this exists (sine notch limiting)
        ///
        /// In sine notch limiting a per-channel response ceiling caps the drive
        /// below what the compression loop is asking for (limit-down beats
        /// control-up). While that ceiling binds, the servo's error is
        /// UNREALIZABLE: calling [`AmplitudeServo::update`] would integrate an
        /// error the plant can never close and wind `level_db` up against the
        /// clamp, producing a large overshoot when the notch releases. The
        /// engine instead calls `hold_tick` on every control tick where the
        /// notch governor is binding:
        ///
        /// - `level_db` is NOT changed (no windup - the drive stays put), and
        /// - `prev_e_db` IS refreshed to the current error, so the first
        ///   [`AmplitudeServo::update`] after release sees `e - prev_e ~ 0` and
        ///   its step is bounded by the integral term alone (`~ ki*e*dt`), i.e.
        ///   no proportional/derivative kick from the error history accumulated
        ///   during the hold.
        ///
        /// Implausible (fault-level) measurements are ignored for the
        /// `prev_e_db` refresh so a dead channel during a hold cannot poison the
        /// resume; the latched [`AmplitudeServo::faulted`] state is not touched.
        pub fn hold_tick(&mut self, measured_amplitude: f64) -> f64 {
            if measured_amplitude <= 0.0 || measured_amplitude.is_nan() {
                return self.drive_scale();
            }
            if measured_amplitude >= self.target * self.fault_fraction {
                self.prev_e_db = 20.0 * (self.target / measured_amplitude).log10();
            }
            self.drive_scale()
        }

        /// Update the servo with the latest `measured_amplitude` (same EU as
        /// target) over elapsed time `dt` seconds. Returns the new `drive_scale`
        /// to apply.
        ///
        /// Safety behaviour (see the type-level docs for the full contract):
        ///
        /// - **Fault hold.** If `measured_amplitude < target * fault_fraction` the
        ///   measurement is treated as a dead-channel fault: the drive is FROZEN
        ///   (level unchanged), [`AmplitudeServo::faulted`] latches true, and the
        ///   current drive is returned. This is what stops a cable-disconnect from
        ///   ramping the drive into the amplifier limit.
        /// - **Absolute ceiling.** After the rate-limited increment, `level_db` is
        ///   clamped to `max_level_db`, so `drive_scale <= max_drive_scale()` always.
        /// - **Anti-windup.** When the PI step is rate-clamped, `prev_e_db` is
        ///   back-calculated from the APPLIED step so the velocity-form proportional
        ///   term is not computed against an error history the output never followed.
        ///
        /// Against an instantaneous-gain plant the loop converges monotonically in
        /// dB. Against a real lagged plant it overshoots and rings within the rate
        /// limit; that is expected, not a divergence.
        pub fn update(&mut self, measured_amplitude: f64, dt: f64) -> f64 {
            if dt <= 0.0 {
                return self.drive_scale();
            }

            // Fault detection: a measurement below target*fault_fraction is not a
            // real "way below target, drive up" condition - it is an implausible
            // reading (dead sensor / disconnected cable). Ramping up into it is the
            // classic runaway. Freeze the drive and latch the fault instead.
            if measured_amplitude < self.target * self.fault_fraction {
                self.faulted = true;
                // Hold prev_e_db as-is so a spurious single dropout does not inject a
                // huge proportional kick when a plausible measurement returns.
                return self.drive_scale();
            }
            self.faulted = false;

            // dB error: positive when we are BELOW target (need more drive).
            let e_db = 20.0 * (self.target / measured_amplitude).log10();

            // Velocity-form (incremental) PI in dB. The output INCREMENT this step
            // is
            //   d_level = kp*(e - e_prev) + ki*e*dt
            // This is the discrete derivative of the positional PI law
            // `kp*e + ki*integral(e)`, so it is a true PI controller but expressed
            // as a per-step move. Its fixed point is e == 0 AND e == e_prev, i.e.
            // measured == target and holding.
            let prop_inc = self.kp * (e_db - self.prev_e_db);
            let integ_inc = self.ki * e_db * dt;
            let step_db = prop_inc + integ_inc;

            // Rate limit the applied step so the drive can never jump.
            let max_step = self.max_rate_db_per_s * dt;
            let clamped = step_db.clamp(-max_step, max_step);

            // Anti-windup: when the step is clamped, update prev_e_db as if the error
            // had moved only by the amount the OUTPUT actually followed, rather than
            // the full (unclamped) error. Without this, during a rate-limited climb
            // the proportional term kp*(e - e_prev) is computed against an error
            // history the drive never realized, adding to overshoot. When unclamped,
            // prev_e_db = e_db exactly (the original behaviour).
            if clamped == step_db {
                self.prev_e_db = e_db;
            } else {
                // Back-calculate the effective prev error from the applied step:
                // solve kp*(e - prev) + ki*e*dt = clamped for prev.
                if self.kp != 0.0 {
                    self.prev_e_db = e_db - (clamped - self.ki * e_db * dt) / self.kp;
                } else {
                    self.prev_e_db = e_db;
                }
            }

            // Apply the increment and clamp to the absolute ceiling (the value bound
            // the rate limit alone does not provide).
            self.level_db = (self.level_db + clamped).min(self.max_level_db);
            self.drive_scale()
        }
    }
}

/// Banded high-pass integration (accel -> velocity -> displacement) with anti-drift.
pub mod integrate {
    /// Standard gravity in inches per second squared. `1 g = 386.0886 in/s^2`.
    pub const G_TO_IN_S2: f64 = 386.0886;

    /// Integrate `accel` (in g) to velocity (in in/s) with a banded high-pass to
    /// kill the DC drift that raw numerical integration accumulates.
    ///
    /// Pipeline:
    /// 1. Convert g to in/s^2 (`* G_TO_IN_S2`).
    /// 2. First-order Butterworth high-pass (bilinear transform) at corner
    ///    `hp_hz`, applied to the ACCELERATION. High-passing before the integral
    ///    is what actually bands the drift: a constant (DC) acceleration is
    ///    removed at the source, so it never becomes an unbounded velocity ramp.
    ///    (High-passing after the integral leaves a residual `c/wc` for a
    ///    constant-acceleration input `c`; high-passing before does not.)
    /// 3. Trapezoidal cumulative integration at sample rate `fs_hz`.
    ///
    /// The high-pass passband gain approaches unity for input frequencies well
    /// above `hp_hz`, so a sine `A*sin(wt)` in g returns approximately
    /// `-(G_TO_IN_S2 * A / w) * cos(wt)` in in/s once the filter transient has
    /// settled (amplitude `G_TO_IN_S2 * A / w`). Choose `hp_hz` at least a
    /// decade below the lowest frequency of interest.
    ///
    /// A single high-pass gives phase lead near the corner; it is chosen over a
    /// zero-phase pass so the function stays causal and usable in the real-time
    /// watchdog path (group delay is budgeted at the `arm()` gate).
    pub fn accel_to_velocity(accel_g: &[f64], fs_hz: f64, hp_hz: f64) -> Vec<f64> {
        let n = accel_g.len();
        if n == 0 || fs_hz <= 0.0 {
            return vec![0.0; n];
        }
        let dt = 1.0 / fs_hz;

        // 1: convert g -> in/s^2.
        // 2: high-pass the acceleration (drift removed before it can integrate).
        // First-order Butterworth via bilinear transform:
        //   H(z) = a * (1 - z^-1) / (1 - b*z^-1),  K = tan(pi*fc/fs),
        //   a = 1/(1+K), b = (1-K)/(1+K).
        let mut acc: Vec<f64> = accel_g.iter().map(|&g| g * G_TO_IN_S2).collect();
        if hp_hz > 0.0 {
            let k = (std::f64::consts::PI * hp_hz / fs_hz).tan();
            let a = 1.0 / (1.0 + k);
            let b = (1.0 - k) / (1.0 + k);
            let mut x_prev = acc[0];
            let mut y_prev = 0.0f64;
            acc[0] = 0.0;
            for slot in acc.iter_mut().skip(1) {
                let x = *slot;
                let y = a * (x - x_prev) + b * y_prev;
                *slot = y;
                x_prev = x;
                y_prev = y;
            }
        }

        // 3: trapezoidal cumulative integration of the (high-passed) in/s^2 signal.
        let mut vel = vec![0.0f64; n];
        let mut acc_prev = acc[0];
        let mut running = 0.0f64;
        for i in 1..n {
            let acc_cur = acc[i];
            running += 0.5 * (acc_prev + acc_cur) * dt;
            vel[i] = running;
            acc_prev = acc_cur;
        }
        vel
    }

    /// Integrate `vel_ips` (in in/s) to displacement (in inches) with a banded
    /// high-pass to kill the DC drift that raw numerical integration accumulates.
    /// This is the SECOND integration stage, mirroring [`accel_to_velocity`]'s
    /// conventions exactly: same first-order Butterworth high-pass design, same
    /// trapezoidal integrator, same settle caveats.
    ///
    /// This is the STREAMING integration policy for live measured data: a high-pass
    /// removes the constant and low-frequency drift that would otherwise ramp a
    /// measured-velocity record into an unbounded displacement. For the PURE (no
    /// high-pass) double-integral used to VERIFY finite-support pulse kinematics,
    /// use [`crate::shock::pulse_kinematics`] instead - an HP filter there would
    /// mask exactly the residual displacement being verified.
    ///
    /// Pipeline:
    /// 1. First-order Butterworth high-pass (bilinear transform) at corner
    ///    `hp_hz`, applied to the VELOCITY. As in [`accel_to_velocity`],
    ///    high-passing before the integral is what bands the drift: a constant
    ///    (DC) velocity is removed at the source, so it never becomes an unbounded
    ///    displacement ramp. (High-passing after the integral leaves a residual
    ///    `c/wc` for a constant-velocity input `c`; high-passing before does not.)
    /// 2. Trapezoidal cumulative integration at sample rate `fs_hz`.
    ///
    /// The high-pass passband gain approaches unity for input frequencies well
    /// above `hp_hz`, so a sine `V*sin(wt)` in in/s returns approximately
    /// `-(V / w) * cos(wt)` in inches once the filter transient has settled
    /// (amplitude `V / w`; no unit conversion here because the input is already
    /// in/s and the output is inches). Choose `hp_hz` at least a decade below the
    /// lowest frequency of interest.
    ///
    /// A single high-pass gives phase lead near the corner; it is chosen over a
    /// zero-phase pass so the function stays causal and usable in the real-time
    /// watchdog path (group delay is budgeted at the `arm()` gate). Chaining
    /// `accel_to_velocity` then `velocity_to_displacement` applies the high-pass
    /// TWICE (once per stage), which is the intended banded double-integration for
    /// streaming displacement estimation.
    pub fn velocity_to_displacement(vel_ips: &[f64], fs_hz: f64, hp_hz: f64) -> Vec<f64> {
        let n = vel_ips.len();
        if n == 0 || fs_hz <= 0.0 {
            return vec![0.0; n];
        }
        let dt = 1.0 / fs_hz;

        // 1: high-pass the velocity (drift removed before it can integrate).
        // First-order Butterworth via bilinear transform (identical design to
        // `accel_to_velocity`):
        //   H(z) = a * (1 - z^-1) / (1 - b*z^-1),  K = tan(pi*fc/fs),
        //   a = 1/(1+K), b = (1-K)/(1+K).
        let mut v: Vec<f64> = vel_ips.to_vec();
        if hp_hz > 0.0 {
            let k = (std::f64::consts::PI * hp_hz / fs_hz).tan();
            let a = 1.0 / (1.0 + k);
            let b = (1.0 - k) / (1.0 + k);
            let mut x_prev = v[0];
            let mut y_prev = 0.0f64;
            v[0] = 0.0;
            for slot in v.iter_mut().skip(1) {
                let x = *slot;
                let y = a * (x - x_prev) + b * y_prev;
                *slot = y;
                x_prev = x;
                y_prev = y;
            }
        }

        // 2: trapezoidal cumulative integration of the (high-passed) in/s signal.
        let mut disp = vec![0.0f64; n];
        let mut v_prev = v[0];
        let mut running = 0.0f64;
        for i in 1..n {
            let v_cur = v[i];
            running += 0.5 * (v_prev + v_cur) * dt;
            disp[i] = running;
            v_prev = v_cur;
        }
        disp
    }
}

/// Classical-shock pulse kinematics and pulse generators.
///
/// This module is the PURE integration policy for finite-support transients: a
/// plain trapezoidal double-integral with NO high-pass. For a finite pulse (a burst
/// bracketed by quiet on both sides) the net velocity and displacement changes ARE
/// the physical quantities being verified - a high-pass filter would mask exactly
/// the residual it is supposed to reveal. That is why this path deliberately does
/// not reuse the banded high-pass integrator in [`crate::integrate`] (which is the
/// STREAMING policy for live measured data). Units are the in/g system: input
/// acceleration in g, velocity in in/s, displacement in inches, converted with
/// [`crate::integrate::G_TO_IN_S2`].
pub mod shock {
    use crate::integrate::G_TO_IN_S2;

    /// Deterministic kinematics of a finite acceleration pulse, computed by pure
    /// trapezoidal double-integration with NO high-pass.
    ///
    /// All fields are in the in/g system:
    /// - accelerations in g,
    /// - velocities in in/s,
    /// - displacements in inches.
    ///
    /// "residual" fields are the value at the END of the record (the net change
    /// left after the pulse), which is the quantity a compensated pulse must drive
    /// to ~0 and an uncompensated pulse does not. "peak"/"min" fields are the
    /// extrema over the whole record.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct PulseKinematics {
        /// Peak absolute acceleration over the record, in g.
        pub peak_accel_g: f64,
        /// Peak absolute velocity over the record, in in/s.
        pub peak_velocity_ips: f64,
        /// Velocity at the last sample (net velocity change), in in/s. For an
        /// UNCOMPENSATED classical pulse this is nonzero (e.g. `2*A*T/pi` for a
        /// half-sine); for a self-nulling waveform (odd-half-cycle wavelet) it is
        /// ~0.
        pub residual_velocity_ips: f64,
        /// Maximum (most positive) displacement over the record, in inches.
        pub peak_disp_in: f64,
        /// Minimum (most negative) displacement over the record, in inches.
        pub min_disp_in: f64,
        /// Peak-to-peak displacement span (`peak_disp_in - min_disp_in`), in
        /// inches. This is the stroke the pulse demands and the quantity checked
        /// against shaker stroke limits in feasibility.
        pub pk_pk_disp_in: f64,
        /// Displacement at the last sample (net displacement change), in inches.
        /// ~0 for a self-nulling waveform; nonzero (and growing with the quiet
        /// tail, since residual velocity ramps it) for an uncompensated pulse.
        pub residual_disp_in: f64,
    }

    /// The velocity and displacement TRACES behind [`PulseKinematics`]: the same
    /// pure trapezoidal double-integration with NO high-pass, kept as full
    /// per-sample series instead of collapsed to extrema.
    ///
    /// Units are the in/g system: `velocity_ips` in in/s, `disp_in` in
    /// inches, both the same length as the input acceleration record.
    #[derive(Debug, Clone, PartialEq, Default)]
    pub struct PulseSeries {
        /// Velocity at each sample, in/s (starts at 0 by construction).
        pub velocity_ips: Vec<f64>,
        /// Displacement at each sample, in (starts at 0 by construction).
        pub disp_in: Vec<f64>,
    }

    /// Integrate a finite acceleration pulse `accel_g` (g) at `fs_hz` to the
    /// velocity and displacement SERIES, by pure trapezoidal double-integration
    /// with NO high-pass (the PURE policy).
    ///
    /// This is the arithmetic [`pulse_kinematics`] performs internally, exposed so
    /// a DISPLAY can plot the velocity and displacement a pulse actually produced
    /// (the operator's accel / velocity / displacement toggle) using the same
    /// integrator that computed the scalars beside it. It is additive: it changes
    /// nothing about the control path, and `pulse_kinematics` now calls it, so the
    /// two can never drift into two answers for one pulse.
    ///
    /// Do NOT reach for [`crate::integrate`]'s banded high-pass pair here. That is
    /// the STREAMING policy for live measured data; on a finite pulse it would
    /// attenuate exactly the low-frequency content the stroke demand lives in, and
    /// the plotted displacement would be smaller than the one the feasibility gate
    /// checks against the shaker's rating.
    ///
    /// An empty record or non-positive `fs_hz` returns empty/zero-length series.
    pub fn pulse_kinematic_series(accel_g: &[f64], fs_hz: f64) -> PulseSeries {
        let n = accel_g.len();
        if n == 0 || fs_hz <= 0.0 {
            return PulseSeries::default();
        }
        let dt = 1.0 / fs_hz;

        // Convert to in/s^2 and trapezoidally integrate to velocity (in/s), NO HP.
        let acc_in_s2: Vec<f64> = accel_g.iter().map(|&g| g * G_TO_IN_S2).collect();
        let mut vel = vec![0.0f64; n];
        let mut running_v = 0.0f64;
        for i in 1..n {
            running_v += 0.5 * (acc_in_s2[i - 1] + acc_in_s2[i]) * dt;
            vel[i] = running_v;
        }

        // Trapezoidally integrate velocity to displacement (in), NO HP.
        let mut disp = vec![0.0f64; n];
        let mut running_d = 0.0f64;
        for i in 1..n {
            running_d += 0.5 * (vel[i - 1] + vel[i]) * dt;
            disp[i] = running_d;
        }

        PulseSeries {
            velocity_ips: vel,
            disp_in: disp,
        }
    }

    /// Compute the [`PulseKinematics`] of a finite acceleration pulse `accel_g`
    /// (g) sampled at `fs_hz`, using pure trapezoidal double-integration with NO
    /// high-pass (the PURE policy).
    ///
    /// # Method and units
    ///
    /// 1. Convert g to in/s^2 (`* G_TO_IN_S2`).
    /// 2. Trapezoidal cumulative integration to velocity (in/s), starting from 0.
    /// 3. Trapezoidal cumulative integration of velocity to displacement (in),
    ///    starting from 0.
    ///
    /// No high-pass is applied at EITHER stage. For a finite-support pulse the net
    /// velocity change and net displacement are the physical residuals that
    /// compensation exists to zero; a high-pass filter (as used in the streaming
    /// [`crate::integrate`] path for live drift rejection) would attenuate exactly
    /// those low-frequency residuals and MASK what this function is meant to
    /// measure. This is the two-integration-policy split.
    ///
    /// # Analytic cross-check (half-sine)
    ///
    /// For an uncompensated half-sine `a(t) = A*sin(pi*t/T)` (peak `A` g, main-lobe
    /// duration `T`), the velocity change is
    /// `dV = integral_0^T a dt = A * (2T/pi)` in the acceleration's units times
    /// seconds; in this function's in/s output that is
    /// `G_TO_IN_S2 * A * 2T/pi`. Because the pulse is one-signed, the velocity
    /// never returns toward zero, so `residual_velocity_ips == peak_velocity_ips`
    /// (both equal `dV`). The golden gate asserts both to < 0.1%.
    ///
    /// An empty record or non-positive `fs_hz` returns an all-zero
    /// [`PulseKinematics`].
    pub fn pulse_kinematics(accel_g: &[f64], fs_hz: f64) -> PulseKinematics {
        let zero = PulseKinematics {
            peak_accel_g: 0.0,
            peak_velocity_ips: 0.0,
            residual_velocity_ips: 0.0,
            peak_disp_in: 0.0,
            min_disp_in: 0.0,
            pk_pk_disp_in: 0.0,
            residual_disp_in: 0.0,
        };
        let n = accel_g.len();
        if n == 0 || fs_hz <= 0.0 {
            return zero;
        }

        // Peak acceleration (g), no conversion needed.
        let peak_accel_g = accel_g.iter().fold(0.0_f64, |m, &v| m.max(v.abs()));

        // The SAME pure double-integration this function has always done, now
        // reachable on its own (see `pulse_kinematic_series`) so a display can draw
        // the velocity and displacement TRACES the scalars are extracted from
        // without a second integrator existing anywhere.
        let PulseSeries {
            velocity_ips: vel,
            disp_in: disp,
        } = pulse_kinematic_series(accel_g, fs_hz);

        let peak_velocity_ips = vel.iter().fold(0.0_f64, |m, &v| m.max(v.abs()));
        let residual_velocity_ips = vel[n - 1];
        let peak_disp_in = disp.iter().fold(f64::NEG_INFINITY, |m, &v| m.max(v));
        let min_disp_in = disp.iter().fold(f64::INFINITY, |m, &v| m.min(v));
        let residual_disp_in = disp[n - 1];

        PulseKinematics {
            peak_accel_g,
            peak_velocity_ips,
            residual_velocity_ips,
            peak_disp_in,
            min_disp_in,
            pk_pk_disp_in: peak_disp_in - min_disp_in,
            residual_disp_in,
        }
    }

    /// Classical-shock pulse generators (MIL-STD-810 / IEC 60068-2-27 pulse
    /// library). Each generator returns an acceleration waveform in g sampled at
    /// `fs_hz`, with `duration_ms` of main-pulse support and no pre/post
    /// compensation (compensation is a Path-B host-side step handled downstream).
    ///
    /// # Sample-count and endpoint conventions (consistent with
    /// the delegated backend's half-sine reference builder)
    ///
    /// The main-pulse sample count is `N = round(fs_hz * duration_ms / 1000)`
    /// (clamped to >= 1), matching the decoded-geometry rule of the delegated
    /// half-sine reference. Shape functions are sampled on `n in 0..N` with the
    /// phase normalized by `N - 1`, so:
    /// - sample 0 and sample `N-1` sit at the pulse endpoints,
    /// - the PEAK equals `amplitude_g` EXACTLY when the peak location falls on a
    ///   sample (always true for the sawtooths and the trapezoid/rectangle
    ///   plateaus; true for the half-sine/haversine only when `N` is odd, else the
    ///   nearest sample is within one bin, as documented on the individual
    ///   generators).
    ///
    /// These functions return the MAIN PULSE ONLY (no quiet tail). Callers that
    /// need residual response to develop (e.g. SRS analysis, kinematics
    /// verification) append their own quiet samples, exactly as the existing
    /// `srs` golden tests and [`super::pulse_kinematics`] callers do.
    pub mod pulse {
        /// Main-pulse sample count `round(fs_hz * duration_ms / 1000)`, clamped to
        /// at least 1. Shared by every generator so lengths are consistent.
        #[inline]
        pub fn sample_count(duration_ms: f64, fs_hz: f64) -> usize {
            (fs_hz * duration_ms / 1000.0).round().max(1.0) as usize
        }

        // Common guard: an empty/degenerate request yields an empty or single
        // sample without panicking.
        #[inline]
        fn guard_len(duration_ms: f64, fs_hz: f64) -> usize {
            if fs_hz <= 0.0 || duration_ms <= 0.0 {
                return 0;
            }
            sample_count(duration_ms, fs_hz)
        }

        /// Half-sine acceleration pulse `A*sin(pi*n/(N-1))`, `n in 0..N`.
        ///
        /// Endpoints are exactly zero; the peak equals `amplitude_g` exactly when
        /// `N` is odd (the midpoint sample lands on `sin(pi/2) = 1`) and within one
        /// sample of it when `N` is even. This is the same formula and endpoint
        /// behavior as the delegated backend's half-sine reference builder, kept
        /// bit-for-bit consistent so the host-side library and the delegated
        /// reference agree.
        pub fn half_sine(amplitude_g: f64, duration_ms: f64, fs_hz: f64) -> Vec<f64> {
            let n = guard_len(duration_ms, fs_hz);
            if n == 0 {
                return Vec::new();
            }
            if n == 1 {
                return vec![amplitude_g];
            }
            let denom = (n - 1) as f64;
            (0..n)
                .map(|i| amplitude_g * (std::f64::consts::PI * i as f64 / denom).sin())
                .collect()
        }

        /// Haversine (versed-sine) acceleration pulse
        /// `A * 0.5*(1 - cos(2*pi*n/(N-1)))`, `n in 0..N`.
        ///
        /// This is the "raised-cosine" shock pulse: endpoints and their slopes are
        /// zero (smoother onset than the half-sine, which starts with nonzero
        /// slope), and the peak equals `amplitude_g` exactly at the midpoint when
        /// `N` is odd (within one sample when even). Preferred where a
        /// jerk-limited onset matters.
        pub fn haversine(amplitude_g: f64, duration_ms: f64, fs_hz: f64) -> Vec<f64> {
            let n = guard_len(duration_ms, fs_hz);
            if n == 0 {
                return Vec::new();
            }
            if n == 1 {
                return vec![amplitude_g];
            }
            let denom = (n - 1) as f64;
            (0..n)
                .map(|i| {
                    let phase = std::f64::consts::TAU * i as f64 / denom;
                    amplitude_g * 0.5 * (1.0 - phase.cos())
                })
                .collect()
        }

        /// Terminal-peak sawtooth: a linear ramp from 0 up to `amplitude_g` at the
        /// LAST sample, `A * n/(N-1)`, `n in 0..N`.
        ///
        /// The peak equals `amplitude_g` exactly at the final sample; the ramp is
        /// monotonically increasing. This is the MIL-STD-810 terminal-peak sawtooth
        /// (used for its broadband, gently-sloping SRS).
        pub fn terminal_peak_sawtooth(amplitude_g: f64, duration_ms: f64, fs_hz: f64) -> Vec<f64> {
            let n = guard_len(duration_ms, fs_hz);
            if n == 0 {
                return Vec::new();
            }
            if n == 1 {
                return vec![amplitude_g];
            }
            let denom = (n - 1) as f64;
            (0..n).map(|i| amplitude_g * i as f64 / denom).collect()
        }

        /// Initial-peak sawtooth: a linear ramp from `amplitude_g` at the FIRST
        /// sample down to 0, `A * (1 - n/(N-1))`, `n in 0..N`.
        ///
        /// The peak equals `amplitude_g` exactly at the first sample; the ramp is
        /// monotonically decreasing. This is the time-reversed terminal-peak
        /// sawtooth.
        pub fn initial_peak_sawtooth(amplitude_g: f64, duration_ms: f64, fs_hz: f64) -> Vec<f64> {
            let n = guard_len(duration_ms, fs_hz);
            if n == 0 {
                return Vec::new();
            }
            if n == 1 {
                return vec![amplitude_g];
            }
            let denom = (n - 1) as f64;
            (0..n)
                .map(|i| amplitude_g * (1.0 - i as f64 / denom))
                .collect()
        }

        /// Trapezoidal pulse: linear rise to `amplitude_g`, flat plateau at
        /// `amplitude_g`, linear fall back to 0. `rise_frac` and `fall_frac` are
        /// the fractions of the total duration spent rising and falling
        /// respectively; the plateau occupies the remaining
        /// `1 - rise_frac - fall_frac`.
        ///
        /// The two fractions are each clamped to `[0, 1]` and, if their sum
        /// exceeds 1, scaled down proportionally so a plateau of zero width is the
        /// worst case (the pulse degenerates to a triangle, never inverts). Every
        /// plateau sample equals `amplitude_g` exactly; the rise and fall are
        /// monotonic. Endpoints are 0 unless a fraction is 0 (a 0 rise_frac starts
        /// at the plateau value, i.e. a rectangle-like leading edge - use
        /// [`rectangle`] if that is the intent).
        pub fn trapezoid(
            amplitude_g: f64,
            duration_ms: f64,
            fs_hz: f64,
            rise_frac: f64,
            fall_frac: f64,
        ) -> Vec<f64> {
            let n = guard_len(duration_ms, fs_hz);
            if n == 0 {
                return Vec::new();
            }
            if n == 1 {
                return vec![amplitude_g];
            }
            let mut rf = rise_frac.clamp(0.0, 1.0);
            let mut ff = fall_frac.clamp(0.0, 1.0);
            if rf + ff > 1.0 {
                let scale = 1.0 / (rf + ff);
                rf *= scale;
                ff *= scale;
            }
            let denom = (n - 1) as f64;
            let rise_end = rf; // normalized position where the plateau begins
            let fall_start = 1.0 - ff; // normalized position where the fall begins
            (0..n)
                .map(|i| {
                    let x = i as f64 / denom; // in [0, 1]
                    if x < rise_end {
                        // linear rise 0 -> A
                        amplitude_g * (x / rise_end)
                    } else if x <= fall_start {
                        // plateau
                        amplitude_g
                    } else {
                        // linear fall A -> 0
                        let ffrac = (1.0 - x) / (1.0 - fall_start);
                        amplitude_g * ffrac
                    }
                })
                .collect()
        }

        /// Rectangular pulse with finite rise/fall edges. A pure rectangle has
        /// infinite slew (an instantaneous jump to `amplitude_g`), which no shaker
        /// can realize and which produces a numerically ill-posed derivative; this
        /// generator therefore requires short rise and fall edges (as fractions of
        /// the total duration) to bound the slew. It is [`trapezoid`] with small
        /// default-caller-chosen edge fractions, exposed separately because the
        /// INTENT (a nominally flat-top rectangle) differs from a general
        /// trapezoid.
        ///
        /// `rise_frac`/`fall_frac` are clamped to `[0, 1]` and proportionally
        /// scaled if they sum above 1. Passing very small fractions approximates an
        /// ideal rectangle while keeping the edges finite; passing 0 for both is
        /// allowed and yields a flat `amplitude_g` block (documented sharp-edge
        /// case - callers wanting bounded slew must pass nonzero fractions).
        pub fn rectangle(
            amplitude_g: f64,
            duration_ms: f64,
            fs_hz: f64,
            rise_frac: f64,
            fall_frac: f64,
        ) -> Vec<f64> {
            let n = guard_len(duration_ms, fs_hz);
            if n == 0 {
                return Vec::new();
            }
            if n == 1 {
                return vec![amplitude_g];
            }
            // Zero-edge case: a flat block at amplitude (sharp edges via the
            // sample grid). Documented above as the ill-posed-slew case.
            if rise_frac <= 0.0 && fall_frac <= 0.0 {
                return vec![amplitude_g; n];
            }
            // Otherwise reuse the trapezoid with the requested finite edges.
            trapezoid(amplitude_g, duration_ms, fs_hz, rise_frac, fall_frac)
        }
    }

    // =========================================================================
    // Host-side (Path B) pulse compensation
    // =========================================================================

    /// The pre/post compensation shoulder shape for [`compensate_pulse`]
    /// (double-sided compensation with selectable shapes - half-sine / rectangular
    /// / rounded-rectangular; the "displacement optimum" family is a later
    /// addition).
    ///
    /// The trade the research pinned: a LONGER shoulder needs LESS amplitude (the
    /// shoulder's velocity area is what cancels the main pulse's, so amplitude
    /// scales inversely with shoulder duration), at the cost of a longer total
    /// waveform and more table stroke used slowly. Shape choice moves the peak
    /// shoulder amplitude for a fixed area: a rectangle is the lowest-amplitude
    /// shape for a given duration; a half-sine of the same duration needs
    /// `pi/2 ~ 1.57x` the amplitude but has zero-slope-free smooth entry/exit.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum CompensationShape {
        /// Half-sine shoulder `sin(pi*n/(N-1))`: smooth, zero at both ends.
        HalfSine,
        /// Rectangular (constant) shoulder: the lowest peak amplitude for a given
        /// duration, but with step edges at the boundaries.
        Rectangular,
        /// Rectangle with raised-cosine tapers over the first and last
        /// [`ROUNDED_RECT_TAPER_FRAC`] of the shoulder: near-rectangular area
        /// efficiency with bounded slew at the edges.
        RoundedRectangular,
    }

    /// Fraction of a [`CompensationShape::RoundedRectangular`] shoulder's length
    /// spent in each raised-cosine taper (Tukey `alpha = 0.5`: 25% ramp-up, 50%
    /// flat, 25% ramp-down).
    pub const ROUNDED_RECT_TAPER_FRAC: f64 = 0.25;

    /// Configuration for [`compensate_pulse`].
    ///
    /// `pre_frac` / `post_frac` are the shoulder DURATIONS as fractions of the
    /// main-pulse duration (e.g. `1.5` = each shoulder lasts 1.5x the main pulse).
    /// The shoulder AMPLITUDES are not configured - they are SOLVED by
    /// [`compensate_pulse`] so the composite has zero net velocity and zero net
    /// displacement. Longer shoulders solve to lower amplitudes (the
    /// research trade above).
    ///
    /// `symmetric == true` (the default; "double-sided" in the research notes)
    /// mirrors the PRE geometry onto the POST side: the post shoulder gets the
    /// same shape and the same length as the pre shoulder (`post_frac` is
    /// ignored). The two amplitudes are still solved independently, so an
    /// asymmetric MAIN pulse (a sawtooth) still nulls both residuals exactly; for
    /// a symmetric main (half-sine) the two solved amplitudes come out equal.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct CompensationConfig {
        /// Shoulder shape (both sides use the same shape).
        pub shape: CompensationShape,
        /// Pre-shoulder duration as a fraction of the main-pulse duration. Must be
        /// finite and `> 0`.
        pub pre_frac: f64,
        /// Post-shoulder duration as a fraction of the main-pulse duration.
        /// Ignored when `symmetric` (the pre geometry is mirrored). Must be finite
        /// and `> 0` when `!symmetric`.
        pub post_frac: f64,
        /// Mirror the pre-shoulder geometry onto the post side (double-sided
        /// symmetric compensation, the default).
        pub symmetric: bool,
    }

    impl Default for CompensationConfig {
        /// Half-sine shoulders, each 1.5x the main-pulse duration, symmetric.
        ///
        /// At this geometry a half-sine main solves to shoulder amplitudes of
        /// `-A/(2*1.5) ~ -33%` of the main peak - the same order as the decoded OEM
        /// 20 g / 11 ms compensation (-24% trough / +12% shoulders; see
        /// [`compensate_pulse`]'s OEM cross-check note).
        fn default() -> Self {
            CompensationConfig {
                shape: CompensationShape::HalfSine,
                pre_frac: 1.5,
                post_frac: 1.5,
                symmetric: true,
            }
        }
    }

    /// A host-compensated (Path B) pulse: the composite waveform plus the
    /// residuals and peak stroke it was verified to, all through
    /// [`pulse_kinematics`] (the PURE no-high-pass integrator).
    /// Units are the in/g system.
    #[derive(Debug, Clone, PartialEq)]
    pub struct CompensatedPulse {
        /// The composite `[pre-shoulder | main pulse | post-shoulder]` waveform, g.
        pub waveform_g: Vec<f64>,
        /// Pre-shoulder length, samples.
        pub pre_len: usize,
        /// Main-pulse length, samples.
        pub main_len: usize,
        /// Post-shoulder length, samples.
        pub post_len: usize,
        /// Net velocity left at the end of the composite, in/s (should be ~0).
        pub residual_velocity_ips: f64,
        /// Net displacement left at the end of the composite, in (should be ~0).
        pub residual_displacement_in: f64,
        /// Peak absolute displacement over the composite, in (the stroke demand
        /// fed to feasibility).
        pub peak_displacement_in: f64,
    }

    /// A unit-amplitude compensation-shoulder shape of `len` samples (peak 1.0).
    /// The shape helper [`compensate_pulse`] scales by the solved amplitudes;
    /// exposed so callers (UI preview, tests) can inspect the shoulder geometry.
    ///
    /// `len == 0` returns empty; `len == 1` returns `[1.0]` for every shape.
    pub fn compensation_shape(shape: CompensationShape, len: usize) -> Vec<f64> {
        if len == 0 {
            return Vec::new();
        }
        if len == 1 {
            return vec![1.0];
        }
        let denom = (len - 1) as f64;
        match shape {
            CompensationShape::HalfSine => (0..len)
                .map(|i| (std::f64::consts::PI * i as f64 / denom).sin())
                .collect(),
            CompensationShape::Rectangular => vec![1.0; len],
            CompensationShape::RoundedRectangular => {
                let taper = ROUNDED_RECT_TAPER_FRAC;
                (0..len)
                    .map(|i| {
                        let x = i as f64 / denom;
                        if x < taper {
                            0.5 * (1.0 - (std::f64::consts::PI * x / taper).cos())
                        } else if x > 1.0 - taper {
                            0.5 * (1.0 - (std::f64::consts::PI * (1.0 - x) / taper).cos())
                        } else {
                            1.0
                        }
                    })
                    .collect()
            }
        }
    }

    /// Double-sided host compensation of a classical main pulse: bracket
    /// `main_pulse_g` with a pre and a post shoulder of the configured
    /// shape/duration and SOLVE the two shoulder amplitudes so the COMPOSITE has
    /// ZERO net velocity AND ZERO net displacement, verified through
    /// [`pulse_kinematics`] - the pure trapezoidal double-integrator with NO
    /// high-pass, because a high-pass would mask exactly the residuals being
    /// nulled (the two-policy split).
    ///
    /// # Path honesty - HOST (Path B) ONLY
    ///
    /// This is host-side compensation for the HostDac/Sim drive path. The
    /// DELEGATED (Path A) path downloads the UNCOMPENSATED main pulse and the
    /// device compensates on-device ("Recalculate Compensation Continuously ON");
    /// feeding this function's output to the delegated path would DOUBLE-COMPENSATE
    /// and over-stroke the shaker. The `hertzforge-shock-common`
    /// `ReferenceForPath::delegated` type seam rejects compensated waveforms (the
    /// negative shoulders trip its one-signed check); use it.
    ///
    /// # Method (Smallwood-lineage optimal-compensation constraints, linear solve)
    ///
    /// The compensated-pulse constraint set is the classical one (Smallwood's
    /// shaker shock synthesis work, e.g. Smallwood & Nord 1974 and the optimal
    /// compensation-pulse literature; also standard practice in commercial
    /// classical-shock controllers - Vibration Research's classical-shock
    /// double-sided compensation exposes the same shape/duration knobs): zero net
    /// velocity and zero net displacement at the end of the composite, so the
    /// shaker table returns to rest at center.
    ///
    /// Both residuals are LINEAR in the two shoulder amplitudes `(A_pre, A_post)`.
    /// For half-sine and rectangular shoulders the pulse-integral relations have
    /// textbook closed forms (a rectangle of amplitude `a` and duration `T_c`
    /// carries velocity area `a*T_c`; a half-sine carries `(2/pi)*a*T_c`), and the
    /// rounded rectangle's integrals are equally well-defined; rather than
    /// hand-transcribe each shape's velocity area and displacement moment (a
    /// transcription-error class), the 2x2 linear system is assembled by PROBING
    /// the same trapezoidal double-integrator the verification uses at
    /// `(A_pre, A_post) = (0,0), (1,0), (0,1)` and solved exactly. The solution is
    /// therefore exact for the SAMPLED geometry (closed-form linear algebra, no
    /// iteration) and bit-consistent with [`pulse_kinematics`]: the golden
    /// gate holds residual velocity `< 1e-6 in/s` and residual displacement
    /// `< 1e-8 in` for the 20 g / 11 ms half-sine, and lands orders of magnitude
    /// below even that.
    ///
    /// The probe/solve runs in SAMPLE units (`dt = 1`): both residual conditions
    /// scale by the same `dt` powers on both sides, so the solved amplitudes are
    /// `dt`-independent (same argument as hertzforge-shock-common's placeholder helper,
    /// which this function supersedes as the real selectable-shape compensator).
    ///
    /// # OEM cross-check (documented, not byte-matched)
    ///
    /// The decoded OEM 20 g / 11 ms half-sine compensation shows a -24% trough with
    /// +12% shoulders - the device's own multi-lobe optimum. This function's
    /// single-lobe double-sided solution at the default geometry (half-sine
    /// shoulders, 1.5x duration each) solves to ~-33% shoulders; rectangular
    /// shoulders at the same duration solve to ~-21%. Same order of magnitude,
    /// different (simpler) shape family - the device recomputes its own
    /// compensation on Path A, so byte-matching the OEM shape is explicitly a
    /// non-goal (the OEM figure is a cross-check of what the DEVICE produces, never
    /// what Hertzforge downloads).
    ///
    /// # Units
    ///
    /// Input and output waveforms in g; residual velocity in in/s; displacements
    /// in inches (via [`crate::integrate::G_TO_IN_S2`]).
    ///
    /// # Edge cases / panics
    ///
    /// An empty `main_pulse_g` or non-positive `fs_hz` returns an all-zero
    /// [`CompensatedPulse`]. Panics (contract violation, matching this crate's
    /// assert convention) if a used shoulder fraction is non-finite or `<= 0`, or
    /// if the shoulder geometry is singular (cannot null both residuals - both
    /// shoulders must have nonzero length, which the fraction check guarantees).
    pub fn compensate_pulse(
        main_pulse_g: &[f64],
        fs_hz: f64,
        cfg: &CompensationConfig,
    ) -> CompensatedPulse {
        if main_pulse_g.is_empty() || fs_hz <= 0.0 {
            return CompensatedPulse {
                waveform_g: Vec::new(),
                pre_len: 0,
                main_len: 0,
                post_len: 0,
                residual_velocity_ips: 0.0,
                residual_displacement_in: 0.0,
                peak_displacement_in: 0.0,
            };
        }
        assert!(
            cfg.pre_frac.is_finite() && cfg.pre_frac > 0.0,
            "compensate_pulse: pre_frac must be finite and > 0 (got {}); double-sided \
             compensation needs both shoulders",
            cfg.pre_frac
        );
        let post_frac = if cfg.symmetric {
            cfg.pre_frac
        } else {
            assert!(
                cfg.post_frac.is_finite() && cfg.post_frac > 0.0,
                "compensate_pulse: post_frac must be finite and > 0 (got {}); double-sided \
                 compensation needs both shoulders",
                cfg.post_frac
            );
            cfg.post_frac
        };
        let m = main_pulse_g.len();
        let n_pre = ((m as f64 * cfg.pre_frac).round() as usize).max(1);
        let n_post = ((m as f64 * post_frac).round() as usize).max(1);
        let pre_shape = compensation_shape(cfg.shape, n_pre);
        let post_shape = compensation_shape(cfg.shape, n_post);

        let assemble = |a_pre: f64, a_post: f64| -> Vec<f64> {
            let mut w = Vec::with_capacity(n_pre + m + n_post);
            w.extend(pre_shape.iter().map(|&s| a_pre * s));
            w.extend_from_slice(main_pulse_g);
            w.extend(post_shape.iter().map(|&s| a_post * s));
            w
        };

        // Probe the pure double-integral residuals at three amplitude points and
        // solve the (linear) 2x2 system exactly. Sample units (dt = 1): the solved
        // amplitudes are dt-independent (see the method note above).
        let residuals = |a_pre: f64, a_post: f64| -> (f64, f64) {
            sample_unit_residuals(&assemble(a_pre, a_post))
        };
        let (v00, d00) = residuals(0.0, 0.0);
        let (v10, d10) = residuals(1.0, 0.0);
        let (v01, d01) = residuals(0.0, 1.0);
        let a11 = v10 - v00;
        let a12 = v01 - v00;
        let a21 = d10 - d00;
        let a22 = d01 - d00;
        let det = a11 * a22 - a12 * a21;
        assert!(
            det.abs() > f64::EPSILON * (a11.abs() + a12.abs() + a21.abs() + a22.abs()).max(1.0),
            "compensate_pulse: shoulder geometry is singular (pre {n_pre} / post {n_post} \
             samples); increase the shoulder fractions"
        );
        let b1 = -v00;
        let b2 = -d00;
        let a_pre = (b1 * a22 - a12 * b2) / det;
        let a_post = (a11 * b2 - b1 * a21) / det;

        let waveform_g = assemble(a_pre, a_post);
        // Verify through the SAME pure integrator the fixtures use.
        let kin = pulse_kinematics(&waveform_g, fs_hz);
        CompensatedPulse {
            waveform_g,
            pre_len: n_pre,
            main_len: m,
            post_len: n_post,
            residual_velocity_ips: kin.residual_velocity_ips,
            residual_displacement_in: kin.residual_disp_in,
            peak_displacement_in: kin.peak_disp_in.abs().max(kin.min_disp_in.abs()),
        }
    }

    /// Net velocity and net displacement (end-of-record values) of `accel` under
    /// the pure trapezoidal double-integral with `dt = 1` (sample units). The same
    /// integrator shape as [`pulse_kinematics`], no high-pass, used by
    /// the [`compensate_pulse`] linear solve so the solved waveform is consistent
    /// with the integrator that verifies it.
    fn sample_unit_residuals(accel: &[f64]) -> (f64, f64) {
        let n = accel.len();
        if n == 0 {
            return (0.0, 0.0);
        }
        let mut vel = vec![0.0f64; n];
        let mut running_v = 0.0f64;
        for i in 1..n {
            running_v += 0.5 * (accel[i - 1] + accel[i]);
            vel[i] = running_v;
        }
        let mut running_d = 0.0f64;
        for i in 1..n {
            running_d += 0.5 * (vel[i - 1] + vel[i]);
        }
        (vel[n - 1], running_d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eu_conversion_is_per_channel() {
        // 1 V on a 1000 mV/g channel is 1 g; on a 10 mV/g channel it is 100 g.
        assert!((eu::volts_to_eu(1.0, 1000.0) - 1.0).abs() < 1e-12);
        assert!((eu::volts_to_eu(1.0, 10.0) - 100.0).abs() < 1e-12);
    }

    #[test]
    fn hann_is_periodic_and_zero_at_edges() {
        let w = window::hann(8);
        assert_eq!(w.len(), 8);
        assert!(w[0].abs() < 1e-12);
    }

    #[test]
    fn rms_and_peak_of_unit_sine() {
        let n = 1024;
        let x: Vec<f64> = (0..n)
            .map(|k| (std::f64::consts::TAU * 4.0 * k as f64 / n as f64).sin())
            .collect();
        assert!((stats::rms(&x) - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        assert!((stats::peak(&x) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn minmax_decimator_preserves_extremes() {
        let mut x = vec![0.0; 1000];
        x[500] = 9.0; // a spike that must survive decimation
        x[501] = -9.0;
        let d = decimator::minmax(&x, 50);
        assert_eq!(d.len(), 50);
        let (gmin, gmax) = d.iter().fold((0.0_f64, 0.0_f64), |(mn, mx), &(a, b)| {
            (mn.min(a), mx.max(b))
        });
        assert_eq!(gmax, 9.0);
        assert_eq!(gmin, -9.0);
    }

    // ---- fft primitive: fast in-file sanity (deeper golden vectors in tests/) ----

    #[test]
    fn next_pow2_boundaries() {
        assert_eq!(fft::next_pow2(0), 1);
        assert_eq!(fft::next_pow2(1), 1);
        assert_eq!(fft::next_pow2(2), 2);
        assert_eq!(fft::next_pow2(3), 4);
        assert_eq!(fft::next_pow2(1000), 1024);
        assert_eq!(fft::next_pow2(1024), 1024);
    }

    #[test]
    fn zero_pad_pow2_pads_and_preserves() {
        let x = [1.0, 2.0, 3.0];
        let p = fft::zero_pad_pow2(&x);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0], (1.0, 0.0));
        assert_eq!(p[2], (3.0, 0.0));
        assert_eq!(p[3], (0.0, 0.0));
    }

    #[test]
    fn ifft_inverts_fft_roundtrip() {
        // ifft(fft(x)) == x within float tolerance, for a nontrivial signal.
        let n = 64;
        let x: Vec<fft::Cplx> = (0..n)
            .map(|k| {
                let t = k as f64;
                (0.3 * t.sin() + (0.13 * t).cos(), 0.05 * (0.7 * t).sin())
            })
            .collect();
        let back = fft::ifft(&fft::fft(&x));
        for (a, b) in x.iter().zip(back.iter()) {
            assert!((a.0 - b.0).abs() < 1e-12, "re {} vs {}", a.0, b.0);
            assert!((a.1 - b.1).abs() < 1e-12, "im {} vs {}", a.1, b.1);
        }
    }

    #[test]
    fn fft_pure_tone_lands_in_one_bin() {
        // A cosine at exactly bin m puts all energy in bins m and N-m.
        let n = 32usize;
        let m = 5usize;
        let x: Vec<f64> = (0..n)
            .map(|k| (std::f64::consts::TAU * m as f64 * k as f64 / n as f64).cos())
            .collect();
        let spec = fft::rfft(&x);
        for (k, &c) in spec.iter().enumerate() {
            let mag = fft::mag(c);
            if k == m || k == n - m {
                assert!((mag - (n as f64 / 2.0)).abs() < 1e-9, "bin {k} mag {mag}");
            } else {
                assert!(mag < 1e-9, "leak into bin {k}: {mag}");
            }
        }
    }

    // ---- control-DSP fast in-file checks (deep coverage in tests/control.rs) ----

    #[test]
    fn cola_next_frame_emits_hop_samples() {
        let nfft = 128;
        let mut s = cola::ColaSynth::new(nfft, 0.5, 4096.0);
        let target = vec![1.0f64; nfft / 2 + 1];
        let out = s.next_frame(&target);
        assert_eq!(out.len(), s.hop());
        assert_eq!(s.hop(), nfft / 2);
        assert_eq!(s.nfft(), nfft);
        // sqrt-Hann synthesis window -> squared-window COLA constant is 1.0.
        assert!((s.squared_window_cola() - 1.0).abs() < 1e-9);
    }

    #[test]
    #[should_panic(expected = "nfft/2+1")]
    fn cola_next_frame_rejects_wrong_target_len() {
        let mut s = cola::ColaSynth::new(128, 0.5, 4096.0);
        let bad = vec![1.0f64; 10];
        let _ = s.next_frame(&bad);
    }

    #[test]
    fn h_estimate_hv_handles_short_input() {
        // shorter than one block -> zeros, correct length.
        let h = frf::h_estimate_hv(&[1.0, 2.0], &[1.0, 2.0], 1000.0, 64);
        assert_eq!(h.len(), 64 / 2 + 1);
        assert!(h.iter().all(|&(re, im)| re == 0.0 && im == 0.0));
    }

    #[test]
    fn h_inverse_regularized_bounds_and_unity() {
        // |H| = 2 (well above sqrt(eps)) -> Hinv ~ 1/H; product ~ 1.
        let h = [(2.0, 0.0)];
        let hinv = frf::h_inverse_regularized(&h, 1e-6);
        // H*Hinv real part ~ 1.
        let prod = h[0].0 * hinv[0].0 - h[0].1 * hinv[0].1;
        assert!((prod - 1.0).abs() < 1e-3);
        // at |H| = 0 the inverse is 0, not infinite.
        let hz = [(0.0, 0.0)];
        let hiz = frf::h_inverse_regularized(&hz, 1e-4);
        assert_eq!(hiz[0], (0.0, 0.0));
    }

    #[test]
    fn tracking_filter_reset_clears_state() {
        let mut tf = sine::TrackingFilter::new(10_000.0, 20.0);
        for i in 0..500 {
            let t = i as f64 / 10_000.0;
            tf.update((std::f64::consts::TAU * 300.0 * t).sin(), 300.0);
        }
        tf.reset();
        // immediately after reset the first output is ~0.
        let (a, _) = tf.update(1.0, 300.0);
        assert!(a < 0.5, "reset did not clear state: amp {a}");
    }

    #[test]
    fn servo_zero_dt_is_noop() {
        let mut s = sine::AmplitudeServo::new(5.0, 6.0);
        let before = s.drive_scale();
        let after = s.update(0.01, 0.0);
        assert_eq!(before, after);
    }

    #[test]
    fn servo_feedforward_scale_sets_level() {
        let mut s = sine::AmplitudeServo::new(5.0, 6.0);
        s.set_feedforward_scale(4.0);
        assert!((s.drive_scale() - 4.0).abs() < 1e-9);
    }
}
