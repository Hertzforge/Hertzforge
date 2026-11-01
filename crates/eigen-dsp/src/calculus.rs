//! Time-domain integration and differentiation for Insight transforms.
//! New-files-only rule.
//!
//! Integration drifts on any DC or low-frequency error, so `integrate` ships
//! with a drift guard: mean removal BEFORE the cumulative trapezoid and a
//! first-order high-pass AFTER it (cutoff caller-set, recipe-recorded).
//! Unit algebra is the CALLER's job (`g -> g·s`, no gravitational conversion
//! here) - kernels stay unit-blind.

/// Cumulative trapezoidal integration with the drift guard.
///
/// `hp_hz = None` disables the post-integration high-pass (mean removal still
/// applies - integrating a nonzero mean is a ramp, never what a shaker-data
/// reviewer wants; a caller who truly wants the ramp can add the mean back).
pub fn integrate(x: &[f64], dt: f64, hp_hz: Option<f64>) -> Result<Vec<f64>, String> {
    // `<= 0.0 || is_nan()` so a NaN step is still refused (the
    // eigen-breakpoints convention for clippy neg_cmp_op_on_partial_ord).
    if dt <= 0.0 || dt.is_nan() {
        return Err("sample step must be positive".into());
    }
    if x.len() < 2 {
        return Err("integration needs at least 2 samples".into());
    }
    if let Some(f) = hp_hz {
        if !(f > 0.0 && f < 0.5 / dt) {
            return Err(format!("drift high-pass {f} Hz outside (0, Nyquist)"));
        }
    }
    let mean = x.iter().sum::<f64>() / x.len() as f64;
    let mut y = Vec::with_capacity(x.len());
    let mut acc = 0.0f64;
    y.push(0.0);
    for i in 1..x.len() {
        acc += dt * ((x[i] - mean) + (x[i - 1] - mean)) / 2.0;
        y.push(acc);
    }
    if let Some(f) = hp_hz {
        highpass1_zero_phase(&mut y, dt, f);
    }
    Ok(y)
}

/// First-order high-pass run forward then backward (zero-phase, doubled
/// attenuation at the corner). One-pole RC form: `a = 1/(1 + tau*dt)`.
fn highpass1_zero_phase(y: &mut [f64], dt: f64, f: f64) {
    let a = 1.0 / (1.0 + std::f64::consts::TAU * f * dt);
    let pass = |v: &mut [f64]| {
        let mut prev_in = v[0];
        let mut prev_out = 0.0f64;
        for s in v.iter_mut() {
            let out = a * (prev_out + *s - prev_in);
            prev_in = *s;
            prev_out = out;
            *s = out;
        }
    };
    pass(y);
    y.reverse();
    pass(y);
    y.reverse();
}

/// Central-difference differentiation; the two ends are one-sided.
pub fn differentiate(x: &[f64], dt: f64) -> Result<Vec<f64>, String> {
    // NaN-refusing positive check; see `integrate`.
    if dt <= 0.0 || dt.is_nan() {
        return Err("sample step must be positive".into());
    }
    let n = x.len();
    if n < 3 {
        return Err("differentiation needs at least 3 samples".into());
    }
    let mut y = Vec::with_capacity(n);
    y.push((x[1] - x[0]) / dt);
    for i in 1..n - 1 {
        y.push((x[i + 1] - x[i - 1]) / (2.0 * dt));
    }
    y.push((x[n - 1] - x[n - 2]) / dt);
    Ok(y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    const FS: f64 = 4096.0;

    fn sine(f: f64, n: usize) -> Vec<f64> {
        (0..n).map(|i| (TAU * f * i as f64 / FS).sin()).collect()
    }

    fn interior_rms(v: &[f64]) -> f64 {
        let n = v.len();
        let s = &v[n / 8..n - n / 8];
        (s.iter().map(|a| a * a).sum::<f64>() / s.len() as f64).sqrt()
    }

    #[test]
    fn integral_of_a_sine_has_amplitude_over_omega() {
        let f = 100.0;
        let x = sine(f, 16384);
        let y = integrate(&x, 1.0 / FS, Some(2.0)).unwrap();
        // integral of sin(wt) = -cos(wt)/w: same RMS scaled by 1/w, and the
        // 2 Hz guard is far below 100 Hz so it does not touch the tone.
        let expect = interior_rms(&x) / (TAU * f);
        let got = interior_rms(&y);
        assert!(
            (got - expect).abs() / expect < 0.01,
            "rms {got} vs {expect}"
        );
    }

    #[test]
    fn drift_guard_kills_the_offset_ramp() {
        let mut x = sine(100.0, 16384);
        for v in x.iter_mut() {
            *v += 0.5; // a DC offset that would integrate into a ramp
        }
        let y = integrate(&x, 1.0 / FS, Some(2.0)).unwrap();
        // Late-record mean stays near zero instead of ramping to 0.5*T ~ 2.0.
        let tail = &y[y.len() - 1024..];
        let tail_mean = tail.iter().sum::<f64>() / tail.len() as f64;
        assert!(tail_mean.abs() < 0.01, "tail mean {tail_mean}");
    }

    #[test]
    fn derivative_of_a_sine_has_amplitude_times_omega() {
        let f = 100.0;
        let x = sine(f, 8192);
        let y = differentiate(&x, 1.0 / FS).unwrap();
        let expect = interior_rms(&x) * TAU * f;
        let got = interior_rms(&y);
        // Central difference under-reads by sinc(f/fs): ~0.1% at 100/4096.
        assert!(
            (got - expect).abs() / expect < 0.005,
            "rms {got} vs {expect}"
        );
    }

    #[test]
    fn integrate_then_differentiate_round_trips_the_passband() {
        let x = sine(150.0, 16384);
        let y = integrate(&x, 1.0 / FS, Some(2.0)).unwrap();
        let z = differentiate(&y, 1.0 / FS).unwrap();
        let num: f64 = x
            .iter()
            .zip(&z)
            .skip(2048)
            .take(12288)
            .map(|(a, b)| (a - b) * (a - b))
            .sum();
        let den: f64 = x.iter().skip(2048).take(12288).map(|a| a * a).sum();
        assert!(
            (num / den).sqrt() < 0.02,
            "round-trip error {}",
            (num / den).sqrt()
        );
    }

    #[test]
    fn bad_parameters_are_refused() {
        assert!(integrate(&[1.0], 1.0 / FS, None).is_err());
        assert!(integrate(&[1.0, 2.0, 3.0], 0.0, None).is_err());
        assert!(integrate(&[1.0, 2.0, 3.0], 1.0 / FS, Some(3000.0)).is_err());
        assert!(differentiate(&[1.0, 2.0], 1.0 / FS).is_err());
    }

    #[test]
    fn a_nan_sample_step_is_refused() {
        let x = [1.0, 2.0, 3.0];
        assert!(integrate(&x, f64::NAN, None).is_err());
        assert!(differentiate(&x, f64::NAN).is_err());
    }
}
