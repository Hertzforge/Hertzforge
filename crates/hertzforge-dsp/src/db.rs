//! Decibel conversions and non-finite-safe reductions.
//!
//! Kernel-layer only: WHICH form applies (power vs amplitude) is policy and
//! lives in the module crate - `hertzforge-dsp` offers both and the caller picks.

/// Power-quantity dB: `10*log10(x/ref)`. PSDs, autopowers, power spectra.
/// Non-positive `x` returns `f64::NEG_INFINITY` (the honest value; display
/// flooring is a UI policy, not a math policy).
pub fn to_db_power(x: f64, reference: f64) -> f64 {
    if x <= 0.0 || reference <= 0.0 || !x.is_finite() {
        return f64::NEG_INFINITY;
    }
    10.0 * (x / reference).log10()
}

/// Amplitude-quantity dB: `20*log10(x/ref)`. Linear spectra, FRF magnitude,
/// time peaks.
pub fn to_db_amplitude(x: f64, reference: f64) -> f64 {
    if x <= 0.0 || reference <= 0.0 || !x.is_finite() {
        return f64::NEG_INFINITY;
    }
    20.0 * (x / reference).log10()
}

pub fn from_db_power(db: f64, reference: f64) -> f64 {
    reference * 10f64.powf(db / 10.0)
}

pub fn from_db_amplitude(db: f64, reference: f64) -> f64 {
    reference * 10f64.powf(db / 20.0)
}

/// Element-wise power dB over a slice (allocates; NEG_INFINITY where x <= 0).
pub fn slice_to_db_power(xs: &[f64], reference: f64) -> Vec<f64> {
    xs.iter().map(|&x| to_db_power(x, reference)).collect()
}

pub fn slice_to_db_amplitude(xs: &[f64], reference: f64) -> Vec<f64> {
    xs.iter().map(|&x| to_db_amplitude(x, reference)).collect()
}

/// Minimum over finite values only. `None` when nothing finite.
pub fn finite_min(xs: &[f64]) -> Option<f64> {
    xs.iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(None, |acc, v| {
            Some(match acc {
                None => v,
                Some(a) => a.min(v),
            })
        })
}

/// Maximum over finite values only. `None` when nothing finite.
pub fn finite_max(xs: &[f64]) -> Option<f64> {
    xs.iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(None, |acc, v| {
            Some(match acc {
                None => v,
                Some(a) => a.max(v),
            })
        })
}

/// Mean over finite values only. `None` when nothing finite.
pub fn finite_mean(xs: &[f64]) -> Option<f64> {
    let mut n = 0usize;
    let mut s = 0.0;
    for &v in xs {
        if v.is_finite() {
            n += 1;
            s += v;
        }
    }
    (n > 0).then(|| s / n as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_and_amplitude_forms_differ_by_factor_two() {
        let p = to_db_power(100.0, 1.0);
        let a = to_db_amplitude(100.0, 1.0);
        assert!((p - 20.0).abs() < 1e-12);
        assert!((a - 40.0).abs() < 1e-12);
    }

    #[test]
    fn round_trips() {
        for db in [-60.0, -3.0103, 0.0, 12.5] {
            assert!((to_db_power(from_db_power(db, 2.0), 2.0) - db).abs() < 1e-9);
            assert!((to_db_amplitude(from_db_amplitude(db, 0.5), 0.5) - db).abs() < 1e-9);
        }
    }

    #[test]
    fn zero_and_negative_are_neg_infinity_not_nan() {
        assert_eq!(to_db_power(0.0, 1.0), f64::NEG_INFINITY);
        assert_eq!(to_db_power(-4.0, 1.0), f64::NEG_INFINITY);
        assert_eq!(to_db_amplitude(f64::NAN, 1.0), f64::NEG_INFINITY);
    }

    #[test]
    fn finite_reductions_skip_non_finite() {
        let xs = [f64::NAN, 3.0, f64::NEG_INFINITY, -1.0, 7.0];
        assert_eq!(finite_min(&xs), Some(-1.0));
        assert_eq!(finite_max(&xs), Some(7.0));
        assert!((finite_mean(&xs).unwrap() - 3.0).abs() < 1e-12);
        assert_eq!(finite_min(&[f64::NAN]), None);
    }
}
