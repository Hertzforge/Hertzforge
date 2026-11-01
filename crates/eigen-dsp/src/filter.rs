//! IIR filtering for Insight transforms: Butterworth
//! low-pass / high-pass / band-pass as cascaded RBJ-cookbook biquads, with an
//! optional zero-phase (forward-backward) pass.
//!
//! Scope is deliberately narrow: Butterworth only, EVEN orders 2..=8. A
//! band-pass is an HP(f1) x LP(f2) cascade - the recipe line names that
//! topology exactly so nobody mistakes it for a true band-pass transform.
//! New-files-only rule: nothing in the existing kernels is touched.

/// One biquad section, Direct Form II Transposed, coefficients normalized
/// so `a0 == 1`.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Biquad {
    /// RBJ-cookbook low-pass section at `fc` (Hz) with quality `q`.
    pub fn lowpass(fc: f64, fs: f64, q: f64) -> Self {
        let w0 = std::f64::consts::TAU * fc / fs;
        let (sw, cw) = (w0.sin(), w0.cos());
        let alpha = sw / (2.0 * q);
        let a0 = 1.0 + alpha;
        Biquad {
            b0: ((1.0 - cw) / 2.0) / a0,
            b1: (1.0 - cw) / a0,
            b2: ((1.0 - cw) / 2.0) / a0,
            a1: (-2.0 * cw) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// RBJ-cookbook high-pass section at `fc` (Hz) with quality `q`.
    pub fn highpass(fc: f64, fs: f64, q: f64) -> Self {
        let w0 = std::f64::consts::TAU * fc / fs;
        let (sw, cw) = (w0.sin(), w0.cos());
        let alpha = sw / (2.0 * q);
        let a0 = 1.0 + alpha;
        Biquad {
            b0: ((1.0 + cw) / 2.0) / a0,
            b1: -(1.0 + cw) / a0,
            b2: ((1.0 + cw) / 2.0) / a0,
            a1: (-2.0 * cw) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }
}

/// The filter kinds Insight's Transform sheet offers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilterKind {
    Lowpass,
    Highpass,
    /// HP(f1) x LP(f2) cascade - NOT a true band-pass transform.
    Bandpass,
}

/// Butterworth pole-pair quality factors for an even order `n`:
/// `Q_k = 1 / (2 cos(theta_k))`, `theta_k = pi (2k+1) / (2n)`, k < n/2.
fn butterworth_qs(order: usize) -> Vec<f64> {
    let n = order as f64;
    (0..order / 2)
        .map(|k| {
            let theta = std::f64::consts::PI * (2.0 * k as f64 + 1.0) / (2.0 * n);
            1.0 / (2.0 * theta.cos())
        })
        .collect()
}

/// Build the Butterworth cascade. `order` must be even and in 2..=8; cutoffs
/// must sit inside (0, fs/2). Band-pass takes `fc = f1` (high-pass edge) and
/// `fc2 = Some(f2)` (low-pass edge) with `f1 < f2`.
pub fn design_butterworth(
    kind: FilterKind,
    order: usize,
    fc: f64,
    fc2: Option<f64>,
    fs: f64,
) -> Result<Vec<Biquad>, String> {
    if !((2..=8).contains(&order) && order.is_multiple_of(2)) {
        return Err(format!("order {order} unsupported: even orders 2..=8 only"));
    }
    // `<= 0.0 || is_nan()` so a NaN rate is still refused (the
    // eigen-breakpoints convention for clippy neg_cmp_op_on_partial_ord).
    if fs <= 0.0 || fs.is_nan() {
        return Err("sample rate must be positive".into());
    }
    let nyq = fs / 2.0;
    let check = |f: f64, name: &str| -> Result<(), String> {
        if !(f > 0.0 && f < nyq) {
            return Err(format!("{name} {f} Hz outside (0, Nyquist {nyq} Hz)"));
        }
        Ok(())
    };
    let qs = butterworth_qs(order);
    match kind {
        FilterKind::Lowpass => {
            check(fc, "cutoff")?;
            Ok(qs.iter().map(|&q| Biquad::lowpass(fc, fs, q)).collect())
        }
        FilterKind::Highpass => {
            check(fc, "cutoff")?;
            Ok(qs.iter().map(|&q| Biquad::highpass(fc, fs, q)).collect())
        }
        FilterKind::Bandpass => {
            let f2 = fc2.ok_or("band-pass needs an upper cutoff")?;
            check(fc, "lower cutoff")?;
            check(f2, "upper cutoff")?;
            if fc >= f2 {
                return Err(format!("lower cutoff {fc} Hz must sit below upper {f2} Hz"));
            }
            let mut sec: Vec<Biquad> = qs.iter().map(|&q| Biquad::highpass(fc, fs, q)).collect();
            sec.extend(qs.iter().map(|&q| Biquad::lowpass(f2, fs, q)));
            Ok(sec)
        }
    }
}

/// Run the cascade causally (single pass, DF2T per section).
pub fn filter(sections: &[Biquad], x: &[f64]) -> Vec<f64> {
    let mut y = x.to_vec();
    for s in sections {
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for v in y.iter_mut() {
            let xin = *v;
            let out = s.b0 * xin + s1;
            s1 = s.b1 * xin - s.a1 * out + s2;
            s2 = s.b2 * xin - s.a2 * out;
            *v = out;
        }
    }
    y
}

/// Zero-phase pass: reflect-pad both ends, filter forward, reverse, filter,
/// reverse, trim. Effective magnitude response is |H|^2 (order doubles);
/// phase is zero. Pad length is `min(len-1, 12*order_hint)` samples.
pub fn filtfilt(sections: &[Biquad], x: &[f64]) -> Vec<f64> {
    let n = x.len();
    if n < 3 {
        return x.to_vec();
    }
    let pad = (12 * (2 * sections.len() + 1)).min(n - 1);
    // Odd (point-symmetric) reflection about the end samples avoids a step
    // at the boundary: p[i] = 2*x[0] - x[pad-i].
    let mut ext = Vec::with_capacity(n + 2 * pad);
    for i in 0..pad {
        ext.push(2.0 * x[0] - x[pad - i]);
    }
    ext.extend_from_slice(x);
    for i in 0..pad {
        ext.push(2.0 * x[n - 1] - x[n - 2 - i]);
    }
    let mut y = filter(sections, &ext);
    y.reverse();
    let mut y = filter(sections, &y);
    y.reverse();
    y[pad..pad + n].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fft;

    /// |H(f)| in dB measured from the impulse response (long enough that the
    /// truncation tail is negligible for these orders).
    fn mag_db_at(sections: &[Biquad], fs: f64, f: f64) -> f64 {
        let n = 16384usize;
        let mut imp = vec![0.0; n];
        imp[0] = 1.0;
        let h = filter(sections, &imp);
        let spec = fft::rfft(&h);
        let bin = f / (fs / n as f64);
        let i = bin.round() as usize;
        let (re, im) = spec[i];
        10.0 * (re * re + im * im).log10()
    }

    #[test]
    fn butterworth_qs_match_the_classic_tables() {
        let q2 = butterworth_qs(2);
        assert!((q2[0] - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
        let q4 = butterworth_qs(4);
        assert!((q4[0] - 0.541196).abs() < 1e-5, "{}", q4[0]);
        assert!((q4[1] - 1.306563).abs() < 1e-5, "{}", q4[1]);
    }

    #[test]
    fn lowpass_hits_minus_3db_at_cutoff_and_rolls_off_by_order() {
        let fs = 8192.0;
        for order in [2usize, 4, 6, 8] {
            let sec = design_butterworth(FilterKind::Lowpass, order, 500.0, None, fs).unwrap();
            let at_fc = mag_db_at(&sec, fs, 500.0);
            assert!(
                (at_fc + 3.0103).abs() < 0.15,
                "order {order}: {at_fc} dB at fc"
            );
            // Two octaves into the stopband: -6.02 dB/oct/order * 2 oct.
            let at_4fc = mag_db_at(&sec, fs, 2000.0);
            let expect = -12.041 * order as f64;
            assert!(
                at_4fc < expect + 3.0,
                "order {order}: {at_4fc} dB at 4fc, expected <= ~{expect}"
            );
            // Passband is flat: 1 dB down at most an octave below fc.
            let at_half = mag_db_at(&sec, fs, 250.0);
            assert!(at_half > -1.0, "order {order}: {at_half} dB at fc/2");
        }
    }

    #[test]
    fn highpass_mirrors_and_bandpass_is_the_cascade() {
        let fs = 8192.0;
        let hp = design_butterworth(FilterKind::Highpass, 4, 100.0, None, fs).unwrap();
        assert!((mag_db_at(&hp, fs, 100.0) + 3.0103).abs() < 0.15);
        assert!(mag_db_at(&hp, fs, 25.0) < -45.0);
        assert!(mag_db_at(&hp, fs, 1000.0) > -0.1);

        let bp = design_butterworth(FilterKind::Bandpass, 4, 100.0, Some(1000.0), fs).unwrap();
        assert_eq!(bp.len(), 4, "HP pair + LP pair");
        assert!(
            mag_db_at(&bp, fs, 316.0) > -0.5,
            "geometric center near flat"
        );
        assert!(mag_db_at(&bp, fs, 25.0) < -45.0);
        assert!(mag_db_at(&bp, fs, 4000.0) < -45.0);
    }

    #[test]
    fn filtfilt_is_zero_phase_and_squares_the_magnitude() {
        let fs = 2048.0;
        let sec = design_butterworth(FilterKind::Lowpass, 4, 200.0, None, fs).unwrap();
        // A passband sine passes with no delay: peak cross-correlation at lag 0.
        let n = 4096usize;
        let x: Vec<f64> = (0..n)
            .map(|i| (std::f64::consts::TAU * 50.0 * i as f64 / fs).sin())
            .collect();
        let y = filtfilt(&sec, &x);
        let mut best = (0i64, f64::MIN);
        for lag in -8i64..=8 {
            let mut acc = 0.0;
            for (i, &xi) in x.iter().enumerate().take(n - 200).skip(200) {
                let j = i as i64 + lag;
                acc += xi * y[j as usize];
            }
            if acc > best.1 {
                best = (lag, acc);
            }
        }
        assert_eq!(best.0, 0, "zero-phase peak must sit at lag 0");
        // At fc the single pass is -3.01 dB, so the double pass is -6.02 dB:
        // measure via steady-state RMS ratio of a cutoff-frequency sine.
        let xc: Vec<f64> = (0..n)
            .map(|i| (std::f64::consts::TAU * 200.0 * i as f64 / fs).sin())
            .collect();
        let yc = filtfilt(&sec, &xc);
        let rms = |v: &[f64]| (v[512..n - 512].iter().map(|a| a * a).sum::<f64>()).sqrt();
        let db = 20.0 * (rms(&yc) / rms(&xc)).log10();
        assert!((db + 6.0206).abs() < 0.3, "double-pass at fc: {db} dB");
    }

    #[test]
    fn design_rejects_bad_parameters() {
        let fs = 1000.0;
        assert!(design_butterworth(FilterKind::Lowpass, 3, 100.0, None, fs).is_err());
        assert!(design_butterworth(FilterKind::Lowpass, 4, 600.0, None, fs).is_err());
        assert!(design_butterworth(FilterKind::Bandpass, 4, 200.0, Some(100.0), fs).is_err());
        assert!(design_butterworth(FilterKind::Bandpass, 4, 100.0, None, fs).is_err());
    }

    #[test]
    fn order_bounds_are_two_and_eight_inclusive() {
        let fs = 1000.0;
        for order in [0, 1, 9, 10] {
            assert!(design_butterworth(FilterKind::Lowpass, order, 100.0, None, fs).is_err());
        }
        for order in [2, 8] {
            assert!(design_butterworth(FilterKind::Lowpass, order, 100.0, None, fs).is_ok());
        }
    }

    #[test]
    fn a_nan_sample_rate_is_refused_by_the_rate_guard() {
        // The rate guard itself, not the cutoff check a NaN Nyquist would also fail.
        let err = design_butterworth(FilterKind::Lowpass, 4, 100.0, None, f64::NAN).unwrap_err();
        assert_eq!(err, "sample rate must be positive");
    }
}
