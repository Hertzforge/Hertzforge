//! The guards the controller applies to every law's answer, as plain
//! arithmetic on drive vectors: the change and rise caps per drive, the
//! rotation cap, the whole-vector ceiling scale, the first-drive bound, the
//! drive abort, the backstop notch and the divergence watch.
//!
//! Every scale here acts on the whole drive vector so its direction is kept:
//! a drive over its ceiling, a limit channel over its bound and a first drive
//! over its bound each scale every drive by the same factor.

use crate::limits::MAX_LIMITS;
use crate::setup::GuardSettings;
use hertzforge_sdm::C64;

const ZERO: C64 = C64::new(0.0, 0.0);

/// A change within a billionth of a dB of a cap does not count as binding.
const EPS_DB: f64 = 1e-9;

/// What the per-drive caps did to one answer.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CapReport {
    /// A change or rise cap bound on at least one drive.
    pub magnitude_bound: bool,
    /// The rotation cap bound on at least one drive.
    pub rotation_bound: bool,
    /// The largest turn of a drive's phase the law asked for, degrees.
    pub largest_rotation_deg: f64,
}

/// Cap each drive of `proposed` against the drive in force: its magnitude
/// moves at most `max_step_db` either way and rises at most
/// `max_rise_db_per_s * dt_s`; with a rotation cap set, its phase turns at most
/// that many degrees. A drive in force of zero is not capped (the first drive
/// is bounded by [`first_drive_scale`]).
pub fn cap_drives(
    in_force: &[C64],
    proposed: &mut [C64],
    g: &GuardSettings,
    dt_s: f64,
) -> CapReport {
    let mut rep = CapReport::default();
    let up = g.max_step_db.min(g.max_rise_db_per_s * dt_s);
    let down = g.max_step_db;
    for (f, p) in in_force.iter().zip(proposed.iter_mut()) {
        let (af, ap) = (f.norm(), p.norm());
        if af.is_nan() || af <= 0.0 {
            continue;
        }
        // The turn asked for.
        let mut phase = p.arg();
        if ap > 0.0 {
            let turn = (*p * f.conj()).arg();
            rep.largest_rotation_deg = rep.largest_rotation_deg.max(turn.abs().to_degrees());
            if let Some(cap) = g.rotation_cap_deg {
                let cap = cap.to_radians();
                if turn.abs() > cap + 1e-12 {
                    phase = f.arg() + cap.copysign(turn);
                    rep.rotation_bound = true;
                }
            }
        } else {
            phase = f.arg();
        }
        let step_db = if ap > 0.0 {
            20.0 * (ap / af).log10()
        } else {
            f64::NEG_INFINITY
        };
        let (mag, bound) = if step_db > up + EPS_DB {
            (af * 10f64.powf(up / 20.0), true)
        } else if step_db < -(down + EPS_DB) {
            (af * 10f64.powf(-down / 20.0), true)
        } else {
            (ap, false)
        };
        rep.magnitude_bound |= bound;
        // A drive no cap touched is passed on exactly as asked.
        if bound || phase != p.arg() {
            *p = C64::from_polar(mag, phase);
        }
    }
    rep
}

/// The scale that puts the drive most over its ceiling at its ceiling, or
/// `None` when every drive is at or under its own.
pub fn ceiling_scale(drive: &[C64], ceilings_vpk: &[f64]) -> Option<f64> {
    let worst = drive
        .iter()
        .zip(ceilings_vpk)
        .map(|(d, c)| d.norm() / c)
        .fold(0.0, f64::max);
    (worst > 1.0).then(|| 1.0 / worst)
}

/// The first drive's bound: the scale that brings the vector's norm to
/// `bound_norm`, or `None` when it is already under it.
pub fn first_drive_scale(drive: &[C64], bound_norm: f64) -> Option<f64> {
    let n = norm(drive);
    (n > bound_norm).then(|| if n > 0.0 { bound_norm / n } else { 0.0 })
}

/// The first drive at or over its abort, if any.
pub fn drive_over_abort(drive: &[C64], aborts_vpk: &[f64]) -> Option<usize> {
    drive
        .iter()
        .zip(aborts_vpk)
        .position(|(d, a)| d.norm() >= *a)
}

/// Scale every drive by `s`.
pub fn scale(drive: &mut [C64], s: f64) {
    drive.iter_mut().for_each(|d| *d *= s);
}

/// The Euclidean norm of a complex vector.
pub fn norm(x: &[C64]) -> f64 {
    x.iter().map(|v| v.norm_sqr()).sum::<f64>().sqrt()
}

/// The Euclidean norm of a real vector.
pub fn norm_real(x: &[f64]) -> f64 {
    x.iter().map(|v| v * v).sum::<f64>().sqrt()
}

/// `y = A x` for `A` rows by cols, row-major.
pub fn mat_vec(a: &[C64], rows: usize, cols: usize, x: &[C64], y: &mut [C64]) {
    for (r, yr) in y.iter_mut().enumerate().take(rows) {
        let mut acc = ZERO;
        for c in 0..cols {
            acc += a[r * cols + c] * x[c];
        }
        *yr = acc;
    }
}

/// The backstop notch: each limit channel's amplitude is predicted for the
/// proposed drive as `kappa_j |T_j D|`, `T_j` the identification's limit row
/// at the tone and `kappa_j` the measured-over-identified ratio (seeded
/// directly, then smoothed over the setup's count of updates); where a
/// prediction passes its bound the whole vector is scaled by the smallest
/// ratio, direction kept. Once nothing binds, the scale comes back toward 1 at
/// the release ramp.
#[derive(Debug, Clone)]
pub struct BackstopNotch {
    kappa: [f64; MAX_LIMITS],
    seeded: [bool; MAX_LIMITS],
    alpha: f64,
    release_db_per_s: f64,
    scale: f64,
}

impl BackstopNotch {
    /// A notch smoothing over `smoothing_ticks` updates, releasing at
    /// `release_db_per_s`.
    pub fn new(smoothing_ticks: u32, release_db_per_s: f64) -> Self {
        BackstopNotch {
            kappa: [1.0; MAX_LIMITS],
            seeded: [false; MAX_LIMITS],
            alpha: 2.0 / (smoothing_ticks.max(1) as f64 + 1.0),
            release_db_per_s,
            scale: 1.0,
        }
    }

    /// The scale now, 0 to 1.
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// Each limit channel's measured-over-identified ratio now.
    pub fn kappa(&self, j: usize) -> f64 {
        self.kappa[j]
    }

    /// Take one update's measured limit amplitudes against what the
    /// identification predicts for the drive in force. A channel whose
    /// prediction is too small to divide by keeps its ratio.
    pub fn observe(&mut self, measured: &[C64], predicted: &[C64]) {
        for (j, (m, p)) in measured.iter().zip(predicted).enumerate() {
            let p = p.norm();
            if p.is_nan() || p <= 1e-12 || !m.norm().is_finite() {
                continue;
            }
            let r = m.norm() / p;
            if self.seeded[j] {
                self.kappa[j] += self.alpha * (r - self.kappa[j]);
            } else {
                self.kappa[j] = r;
                self.seeded[j] = true;
            }
        }
    }

    /// The scale the proposed drive is played at: `predicted` holds each
    /// limit channel's identified amplitude for the proposed drive, `bounds`
    /// each bound here (`None` where there is none). Returns the scale and
    /// whether the notch binds (a prediction at or over its bound).
    pub fn apply(&mut self, predicted: &[C64], bounds: &[Option<f64>], dt_s: f64) -> (f64, bool) {
        let mut need = 1.0f64;
        for (j, (p, b)) in predicted.iter().zip(bounds).enumerate() {
            let Some(b) = b else { continue };
            let pred = self.kappa[j] * p.norm();
            if pred > *b {
                need = need.min(b / pred);
            }
        }
        let binds = need < 1.0;
        let released = (self.scale * 10f64.powf(self.release_db_per_s * dt_s / 20.0)).min(1.0);
        self.scale = need.min(released);
        (self.scale, binds)
    }
}

/// The divergence watch: a cap binding without a break for `limit_s` stops
/// the run.
#[derive(Debug, Clone, Copy)]
pub struct DivergenceWatch {
    bound_s: f64,
    limit_s: f64,
}

impl DivergenceWatch {
    /// A watch that trips after `limit_s` seconds of binding without a break.
    pub fn new(limit_s: f64) -> Self {
        DivergenceWatch {
            bound_s: 0.0,
            limit_s,
        }
    }

    /// One update of `dt_s`: whether a cap bound. Returns true when the watch
    /// trips.
    pub fn update(&mut self, bound: bool, dt_s: f64) -> bool {
        if bound {
            self.bound_s += dt_s;
        } else {
            self.bound_s = 0.0;
        }
        self.bound_s >= self.limit_s - 1e-9
    }

    /// Seconds the caps have bound without a break.
    pub fn bound_s(&self) -> f64 {
        self.bound_s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(re: f64, im: f64) -> C64 {
        C64::new(re, im)
    }

    #[test]
    fn mimo_sine_guard_caps_each_drive_on_its_own_and_keeps_phase() {
        let g = GuardSettings::default();
        let f = [c(1.0, 0.0), c(0.0, 1.0), c(0.5, 0.0)];
        // +20 dB on drive 0, -20 dB on drive 1, drive 2 unchanged.
        let mut p = [c(10.0, 0.0), c(0.0, 0.1), c(0.5, 0.0)];
        let r = cap_drives(&f, &mut p, &g, 0.01);
        assert!(r.magnitude_bound);
        assert!((p[0].norm() - 10f64.powf(1.0 / 20.0)).abs() < 1e-12);
        assert!((p[1].norm() - 10f64.powf(-1.0 / 20.0)).abs() < 1e-12);
        assert!((p[1].arg() - f[1].arg()).abs() < 1e-12);
        assert_eq!(p[2], c(0.5, 0.0));
    }

    #[test]
    fn mimo_sine_guard_rise_cap_binds_below_the_step_cap() {
        let g = GuardSettings {
            max_rise_db_per_s: 10.0,
            ..GuardSettings::default()
        };
        let f = [c(1.0, 0.0)];
        let mut p = [c(1.1, 0.0)];
        let r = cap_drives(&f, &mut p, &g, 0.01);
        assert!(r.magnitude_bound);
        assert!((20.0 * p[0].norm().log10() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn mimo_sine_guard_rotation_is_recorded_and_capped_only_when_set() {
        let f = [c(1.0, 0.0)];
        let turned = C64::from_polar(1.0, 60f64.to_radians());
        let mut p = [turned];
        let off = cap_drives(&f, &mut p, &GuardSettings::default(), 0.01);
        assert!(!off.rotation_bound);
        assert!((off.largest_rotation_deg - 60.0).abs() < 1e-9);
        assert_eq!(p[0], turned);
        let g = GuardSettings {
            rotation_cap_deg: Some(10.0),
            ..GuardSettings::default()
        };
        let mut p = [turned];
        let on = cap_drives(&f, &mut p, &g, 0.01);
        assert!(on.rotation_bound);
        assert!((p[0].arg().to_degrees() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn mimo_sine_guard_ceiling_scales_the_whole_vector_keeping_direction() {
        let mut d = [c(2.0, 0.0), c(0.0, 1.0)];
        let s = ceiling_scale(&d, &[1.0, 5.0]).expect("over");
        scale(&mut d, s);
        assert!((d[0].norm() - 1.0).abs() < 1e-12);
        assert!((d[1].norm() - 0.5).abs() < 1e-12);
        assert!(ceiling_scale(&d, &[1.0, 5.0]).is_none());
    }

    #[test]
    fn mimo_sine_guard_drive_abort_names_the_drive() {
        assert_eq!(
            drive_over_abort(&[c(1.0, 0.0), c(3.0, 0.0)], &[2.0, 2.0]),
            Some(1)
        );
        assert_eq!(drive_over_abort(&[c(1.0, 0.0)], &[2.0]), None);
    }

    #[test]
    fn mimo_sine_guard_notch_scales_to_the_bound_and_releases_on_its_ramp() {
        let mut n = BackstopNotch::new(3, 20.0);
        // Measured twice the identified: kappa seeds to 2.
        n.observe(&[c(2.0, 0.0)], &[c(1.0, 0.0)]);
        assert!((n.kappa(0) - 2.0).abs() < 1e-12);
        // Identified 1 for the proposed drive, predicted 2, bound 1: half.
        let (s, binds) = n.apply(&[c(1.0, 0.0)], &[Some(1.0)], 0.01);
        assert!(binds);
        assert!((s - 0.5).abs() < 1e-12);
        // Nothing binds now: back up by 0.2 dB a 10 ms update, not at once.
        let (s2, binds) = n.apply(&[c(0.1, 0.0)], &[Some(1.0)], 0.01);
        assert!(!binds);
        assert!((20.0 * (s2 / 0.5).log10() - 0.2).abs() < 1e-9);
    }

    #[test]
    fn mimo_sine_guard_divergence_trips_after_its_time_and_resets_on_a_break() {
        let mut w = DivergenceWatch::new(1.0);
        for _ in 0..99 {
            assert!(!w.update(true, 0.01));
        }
        assert!(!w.update(false, 0.01));
        for _ in 0..99 {
            assert!(!w.update(true, 0.01));
        }
        assert!(w.update(true, 0.01));
    }
}
