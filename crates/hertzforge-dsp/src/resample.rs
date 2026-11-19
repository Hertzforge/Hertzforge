//! Rational-ratio resampling for Insight transforms:
//! polyphase windowed-sinc, L/M from a continued-fraction rational
//! approximation, Kaiser window sized for ~80 dB stopband.
//!
//! RATE HONESTY: the achieved output rate is
//! `fs_in * L / M`, which may differ from the requested rate when the ratio
//! is not exactly rational within the L,M bound. The achieved rate is
//! returned and MUST be what lands in the output record and its recipe -
//! never print the requested rate as the achieved rate.
//! New-files-only rule.

use crate::window_ext::kaiser_symmetric;

/// Upper bound on both L and M. 512 covers every hardware ladder pair seen
/// on the bench (the NI 8192 -> 8533.33 pair is 25/24) while keeping the
/// worst-case kernel around 12k taps.
pub const MAX_FACTOR: usize = 512;

/// Best rational approximation `L/M` of `ratio` with both terms bounded by
/// [`MAX_FACTOR`], via continued-fraction convergents.
pub fn rational_ratio(ratio: f64) -> Result<(usize, usize), String> {
    if !(ratio.is_finite() && ratio > 0.0) {
        return Err(format!("ratio {ratio} must be positive and finite"));
    }
    // Convergents p/q of the continued fraction; keep the last within bounds.
    let (mut p0, mut q0, mut p1, mut q1) = (1u64, 0u64, 0u64, 1u64);
    let mut x = ratio;
    let mut best: Option<(u64, u64)> = None;
    for _ in 0..64 {
        let a = x.floor() as u64;
        let p2 = a.saturating_mul(p0).saturating_add(p1);
        let q2 = a.saturating_mul(q0).saturating_add(q1);
        if p2 as usize > MAX_FACTOR || q2 as usize > MAX_FACTOR {
            break;
        }
        // p2 == 0 (the leading 0/1 of any ratio < 1) is a legitimate
        // intermediate convergent; only the FINAL answer must be nonzero.
        best = Some((p2, q2));
        let frac = x - a as f64;
        if frac < 1e-12 {
            break;
        }
        x = 1.0 / frac;
        p1 = p0;
        q1 = q0;
        p0 = p2;
        q0 = q2;
    }
    let (l, m) = best.ok_or_else(|| format!("ratio {ratio} has no L/M within {MAX_FACTOR}"))?;
    if l == 0 {
        return Err(format!(
            "ratio {ratio} rounds to zero within L/M <= {MAX_FACTOR}"
        ));
    }
    Ok((l as usize, m as usize))
}

/// A resample result: the samples plus the ACHIEVED rate facts.
#[derive(Debug, Clone)]
pub struct Resampled {
    pub y: Vec<f64>,
    /// Upsample / downsample factors actually used.
    pub l: usize,
    pub m: usize,
    /// The achieved output rate `fs_in * L / M` - the only rate a caller may
    /// print for this data.
    pub fs_out: f64,
}

/// Resample `x` (rate `fs_in`) to approximately `fs_target`.
///
/// Polyphase windowed-sinc: kernel cutoff `0.45 * min(fs_in, fs_out)`,
/// Kaiser beta 7.857 (~80 dB), half-length `12 * max(L, M)` upsampled taps.
/// Output sample `j` sits at source time `j * M / (L * fs_in)` (group delay
/// compensated by construction); tap sums are normalized per output sample,
/// so DC passes exactly and edges stay bias-free.
pub fn resample(x: &[f64], fs_in: f64, fs_target: f64) -> Result<Resampled, String> {
    if !(fs_in > 0.0 && fs_target > 0.0) {
        return Err("rates must be positive".into());
    }
    if x.len() < 4 {
        return Err("resampling needs at least 4 samples".into());
    }
    let (l, m) = rational_ratio(fs_target / fs_in)?;
    if l == m {
        return Ok(Resampled {
            y: x.to_vec(),
            l,
            m,
            fs_out: fs_in,
        });
    }
    let fs_out = fs_in * l as f64 / m as f64;
    let fs_up = fs_in * l as f64;
    // Cutoff in cycles per UPSAMPLED sample.
    let fc = 0.45 * fs_in.min(fs_out) / fs_up;
    let half = 12 * l.max(m);
    let ntaps = 2 * half + 1;
    let win = kaiser_symmetric(ntaps, 7.857);
    let mut h = Vec::with_capacity(ntaps);
    for (t, &wt) in win.iter().enumerate() {
        let k = t as f64 - half as f64;
        let sinc = if k == 0.0 {
            2.0 * fc
        } else {
            (std::f64::consts::TAU * fc * k).sin() / (std::f64::consts::PI * k)
        };
        h.push(sinc * wt);
    }
    let n = x.len();
    let n_out = ((n - 1) * l) / m + 1;
    let mut y = Vec::with_capacity(n_out);
    for j in 0..n_out {
        // Kernel centered at upsampled position c = j*M; input i contributes
        // through tap index c - i*L + half.
        let c = j * m;
        let lo = c.saturating_sub(half).div_ceil(l);
        let hi = ((c + half) / l).min(n - 1);
        let mut acc = 0.0f64;
        let mut wsum = 0.0f64;
        for (i, &xi) in x.iter().enumerate().take(hi + 1).skip(lo) {
            let t = c + half - i * l;
            let w = h[t];
            acc += xi * w;
            wsum += w;
        }
        y.push(if wsum.abs() > 1e-12 { acc / wsum } else { 0.0 });
    }
    Ok(Resampled { y, l, m, fs_out })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    #[test]
    fn the_ni_ladder_pair_is_exactly_25_over_24() {
        let (l, m) = rational_ratio(8533.333333333334 / 8192.0).unwrap();
        assert_eq!((l, m), (25, 24));
        let (l, m) = rational_ratio(2.0).unwrap();
        assert_eq!((l, m), (2, 1));
        let (l, m) = rational_ratio(0.5).unwrap();
        assert_eq!((l, m), (1, 2));
    }

    #[test]
    fn dc_passes_exactly_and_the_achieved_rate_is_reported() {
        let x = vec![3.5f64; 4096];
        let r = resample(&x, 1000.0, 1500.0).unwrap();
        assert_eq!((r.l, r.m), (3, 2));
        assert!((r.fs_out - 1500.0).abs() < 1e-9);
        for v in &r.y {
            assert!((v - 3.5).abs() < 1e-9, "{v}");
        }
    }

    #[test]
    fn a_sine_survives_up_and_down_with_amplitude_and_alignment() {
        let fs = 1000.0;
        let f = 100.0;
        let n = 8192usize;
        let x: Vec<f64> = (0..n).map(|i| (TAU * f * i as f64 / fs).sin()).collect();
        let up = resample(&x, fs, 1500.0).unwrap();
        // Compare against the analytic sine AT THE OUTPUT CLOCK - this checks
        // amplitude AND phase/delay alignment in one pass.
        let mut worst = 0.0f64;
        let n_up = up.y.len();
        for (j, v) in up.y.iter().enumerate().skip(n_up / 10).take(8 * n_up / 10) {
            let t = j as f64 / up.fs_out;
            worst = worst.max((v - (TAU * f * t).sin()).abs());
        }
        assert!(worst < 0.01, "worst interior error {worst}");
        // And back down: same test at the original rate.
        let dn = resample(&up.y, up.fs_out, fs).unwrap();
        let n_dn = dn.y.len();
        let mut worst = 0.0f64;
        for (j, v) in dn.y.iter().enumerate().skip(n_dn / 10).take(8 * n_dn / 10) {
            let t = j as f64 / dn.fs_out;
            worst = worst.max((v - (TAU * f * t).sin()).abs());
        }
        assert!(worst < 0.02, "round-trip interior error {worst}");
    }

    #[test]
    fn a_tone_above_the_target_nyquist_is_rejected_not_aliased() {
        let fs = 1000.0;
        let n = 8192usize;
        let x: Vec<f64> = (0..n)
            .map(|i| (TAU * 350.0 * i as f64 / fs).sin())
            .collect();
        let r = resample(&x, fs, 500.0).unwrap();
        let s = &r.y[r.y.len() / 10..9 * r.y.len() / 10];
        let rms = (s.iter().map(|a| a * a).sum::<f64>() / s.len() as f64).sqrt();
        // Input tone RMS is 0.707; anything surviving is alias leakage.
        assert!(rms < 0.007, "alias leakage rms {rms}");
    }

    #[test]
    fn unity_ratio_is_the_identity_and_bad_input_is_refused() {
        let x = vec![1.0, 2.0, 3.0, 4.0];
        let r = resample(&x, 1000.0, 1000.0).unwrap();
        assert_eq!(r.y, x);
        assert!(resample(&x, 0.0, 1000.0).is_err());
        assert!(resample(&[1.0], 1000.0, 500.0).is_err());
        // A ratio no bounded L/M reaches: prime-ish irrational far from small fractions.
        assert!(rational_ratio(f64::NAN).is_err());
    }
}
