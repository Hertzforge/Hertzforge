//! The judge (design D6, D7): every control on its own, then every control must
//! pass.
//!
//! - **Timing.** Each control's delay against its target is found by
//!   cross-correlation over a window: `free` searches `+/- window_s` around zero
//!   and reports the delay; `hold` searches twice the window around the set
//!   delay, and the control fails unless its delay is inside the window.
//! - **Classical** (a classical pulse per control): the classical standard's
//!   tolerance (amplitude envelope, velocity change, substitution) on the
//!   control's response, normalized to full level, after its own best-fit
//!   alignment.
//! - **Envelope** (a waveform given sample by sample): every sample within the
//!   tolerance of the target's peak after the delay is removed.
//! - **SRS**: the control's SRS at its target's frequencies inside
//!   `[tol_lo_db, tol_hi_db]` of the target at the level played. The effective
//!   duration is not checked (the single-output SRS judge does not check it).
//!
//! The energy-weighted SRS error over every control is computed and reported,
//! never judged ([`global_srs_error_db`]).

use crate::pulse::ControlVerdict;
use crate::setup::{Target, Timing};
use crate::targets::Judging;
use hertzforge_dsp::shock::pulse_kinematics;
use hertzforge_dsp::srs::srs_analyze;
use hertzforge_mimo_shock_law_math::spectrum::{irfft, rfft};
use hertzforge_shock_host::tolerance::check_tolerance;
use serde::{Deserialize, Serialize};

/// One control's judgement and the numbers behind it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlJudgement {
    pub verdict: ControlVerdict,
    /// The waveform error after the delay is removed, dB:
    /// `20 log10(rms(y - s r) / rms(s r))`.
    pub waveform_error_db: f64,
    /// The measured SRS, EU, at the played level (an SRS target only).
    pub srs: Option<Vec<f64>>,
    /// The worst SRS deviation, dB, signed (an SRS target only).
    pub srs_error_db: Option<Vec<f64>>,
    /// The delay was inside its window.
    pub timing_ok: bool,
    /// The measured peak, EU.
    pub peak: f64,
    /// Peak velocity, in/s, and peak-to-peak displacement, in, of the measured
    /// record through the pure integrator.
    pub velocity_ips: f64,
    pub displacement_pk_pk_in: f64,
}

/// Peak absolute value.
pub fn peak(x: &[f64]) -> f64 {
    x.iter().fold(0.0f64, |a, v| a.max(v.abs()))
}

fn rms(x: &[f64]) -> f64 {
    (x.iter().map(|v| v * v).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

/// The delay (s) in `[centre - half, centre + half]` at which `y(t)` best matches
/// `r(t - d)` (largest positive cross-correlation), refined below a sample by a
/// parabola.
pub fn windowed_delay(y: &[f64], r: &[f64], fs_hz: f64, centre_s: f64, half_s: f64) -> f64 {
    let n = y.len();
    let m = 2 * n;
    let mut yp = y.to_vec();
    yp.resize(m, 0.0);
    let mut rp = r.to_vec();
    rp.resize(m, 0.0);
    let (Ok(ys), Ok(rs)) = (rfft(&yp), rfft(&rp)) else {
        return centre_s;
    };
    let cs: Vec<_> = ys.iter().zip(&rs).map(|(a, b)| a * b.conj()).collect();
    let Ok(c) = irfft(&cs, m) else {
        return centre_s;
    };
    let at = |lag: i64| c[lag.rem_euclid(m as i64) as usize];
    let lo = ((centre_s - half_s) * fs_hz).ceil() as i64;
    let hi = ((centre_s + half_s) * fs_hz).floor() as i64;
    let (lo, hi) = (lo.max(-(n as i64) + 1), hi.min(n as i64 - 1));
    if lo > hi {
        return centre_s;
    }
    let mut best = lo;
    for lag in lo..=hi {
        if at(lag) > at(best) {
            best = lag;
        }
    }
    let (ym, y0, y1) = (at(best - 1), at(best), at(best + 1));
    let den = ym - 2.0 * y0 + y1;
    let frac = if den < 0.0 && best > lo && best < hi {
        (0.5 * (ym - y1) / den).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    (best as f64 + frac) / fs_hz
}

/// `r` delayed by `tau` seconds (a linear phase; circular over the record).
pub fn delayed(r: &[f64], fs_hz: f64, tau: f64) -> Vec<f64> {
    let n = r.len();
    let Ok(mut s) = rfft(r) else {
        return r.to_vec();
    };
    for (k, v) in s.iter_mut().enumerate() {
        let w = std::f64::consts::TAU * k as f64 * fs_hz / n as f64;
        *v *= hertzforge_sdm::C64::from_polar(1.0, -w * tau);
    }
    irfft(&s, n).unwrap_or_else(|_| r.to_vec())
}

/// The waveform error of `y` against `s r` delayed by `tau`, dB.
pub fn waveform_error_db(y: &[f64], r: &[f64], s: f64, fs_hz: f64, tau: f64) -> f64 {
    let rd = delayed(r, fs_hz, tau);
    let e: Vec<f64> = y.iter().zip(&rd).map(|(a, b)| a - s * b).collect();
    let den = s * rms(r);
    if den > 0.0 {
        20.0 * (rms(&e).max(1e-300) / den).log10()
    } else {
        f64::INFINITY
    }
}

/// Judge control `c`'s measured record `y` (EU, at the played level) against
/// its target at `level_db`.
pub fn judge_control(
    c: usize,
    y: &[f64],
    level_db: f64,
    fs_hz: f64,
    target: &Target,
    judging: &Judging,
    timing: &Timing,
) -> ControlJudgement {
    let n = y.len();
    let s = 10f64.powf(level_db / 20.0);
    let r = &target.waveforms()[c * n..(c + 1) * n];
    let (centre, half, set, window) = match timing {
        Timing::Free { window_s } => (0.0, *window_s, 0.0, *window_s),
        Timing::Hold(h) => (h.delays_s[c], 2.0 * h.window_s, h.delays_s[c], h.window_s),
    };
    let tau = windowed_delay(y, r, fs_hz, centre, half);
    let timing_ok = (tau - set).abs() <= window + 0.5 / fs_hz;
    let wave_db = waveform_error_db(y, r, s, fs_hz, tau);
    let pk = peak(y);
    let r_pk = s * peak(r);
    let kin = pulse_kinematics(y, fs_hz);
    let mut out = ControlJudgement {
        verdict: ControlVerdict {
            passed: false,
            error_db: wave_db,
            delay_s: tau,
            peak_ratio: if r_pk > 0.0 { pk / r_pk } else { f64::INFINITY },
        },
        waveform_error_db: wave_db,
        srs: None,
        srs_error_db: None,
        timing_ok,
        peak: pk,
        velocity_ips: kin.peak_velocity_ips,
        displacement_pk_pk_in: kin.pk_pk_disp_in,
    };
    let shape_ok = match (judging, target) {
        (Judging::Classical { controls, standard }, _) => {
            let ctl = &controls[c];
            let sign = if ctl.inverted { -1.0 } else { 1.0 };
            let norm: Vec<f64> = y.iter().map(|v| sign * v / s).collect();
            check_tolerance(&norm, &ctl.pulse, fs_hz, *standard).overall_pass
        }
        (Judging::Envelope { tol_frac_of_peak }, _) => {
            let rd = delayed(r, fs_hz, tau);
            let worst = y
                .iter()
                .zip(&rd)
                .fold(0.0f64, |a, (m, t)| a.max((m - s * t).abs()));
            worst <= tol_frac_of_peak * r_pk
        }
        (Judging::Srs, Target::Srs(t)) => {
            let nf = t.natural_hz.len();
            let m = srs_analyze(y, fs_hz, &t.natural_hz, t.q);
            let err: Vec<f64> = m
                .iter()
                .zip(&t.points[c * nf..(c + 1) * nf])
                .map(|(mv, tv)| {
                    if *mv > 0.0 {
                        20.0 * (mv / (s * tv)).log10()
                    } else {
                        -400.0
                    }
                })
                .collect();
            let ok = err.iter().all(|e| *e >= t.tol_lo_db && *e <= t.tol_hi_db);
            out.verdict.error_db = err.iter().fold(0.0f64, |a, e| a.max(e.abs()));
            out.srs = Some(m);
            out.srs_error_db = Some(err);
            ok
        }
        (Judging::Srs, Target::Waveform(_)) => false,
    };
    out.verdict.passed = shape_ok && timing_ok;
    out
}

/// The energy-weighted SRS error over every control, dB: the root mean square
/// of every control's per-frequency error, each weighted by the target's
/// energy at that frequency (`points^2`). Shown, never judged.
pub fn global_srs_error_db(target: &Target, errors: &[Vec<f64>]) -> Option<f64> {
    let Target::Srs(t) = target else {
        return None;
    };
    let nf = t.natural_hz.len();
    let (mut num, mut den) = (0.0, 0.0);
    for (c, e) in errors.iter().enumerate() {
        for (j, v) in e.iter().enumerate() {
            let w = t.points[c * nf + j].powi(2);
            num += w * v * v;
            den += w;
        }
    }
    (den > 0.0).then(|| (num / den).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_shock_engine_windowed_delay_finds_a_delay_inside_its_window_only() {
        let fs = 1000.0;
        let n = 256;
        let r: Vec<f64> = (0..n)
            .map(|i| {
                let t = (i as f64 - 60.0) / 6.0;
                (-t * t).exp()
            })
            .collect();
        let y = delayed(&r, fs, 0.007);
        let d = windowed_delay(&y, &r, fs, 0.0, 0.010);
        assert!((d - 0.007).abs() < 2e-4, "{d}");
        // A window that does not reach the true delay reports its own edge.
        let d = windowed_delay(&y, &r, fs, 0.0, 0.003);
        assert!(d <= 0.003 + 1e-12, "{d}");
    }
}
