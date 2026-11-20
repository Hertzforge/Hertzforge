//! The checks every captured impact block takes before it may be averaged, each with
//! its numbers:
//!
//! * **Overload.** A sample of the reference or a response at or above
//!   [`hertzforge_hal::INPUT_OVERLOAD_FRACTION`] of its row's range. Rejects.
//! * **Double hit.** After the main pulse has fallen below [`PULSE_END_FRACTION`] of its
//!   peak, a later force sample whose magnitude exceeds the double-hit fraction of the
//!   peak. Rejects. A force cell's ringing below the fraction is not one.
//! * **Tip roll-off.** The force autospectrum's drop, dB, from its largest value in the
//!   band to its value at the band's top line. Above the setup's figure the tip is
//!   reported as too soft for the band. A warning only.
//! * **No response.** A response whose largest magnitude after the trigger is within
//!   [`NO_RESPONSE_RATIO`] times its pre-trigger RMS. A warning only; not judged without a
//!   pre-trigger.
//! * **Force target.** With a target peak set, the hit's peak against it: the percent
//!   difference and `high`, `low` or `on_target`. A signal only: it never rejects.

use crate::setup::ForceTarget;
use serde_json::{json, Value};

/// The fraction of the peak below which the main pulse has ended.
pub const PULSE_END_FRACTION: f64 = 0.05;
/// A response's post-trigger peak must exceed this many times its pre-trigger RMS.
pub const NO_RESPONSE_RATIO: f64 = 4.0;

/// What a check is handed: one block per channel, in engineering units.
pub struct HitInput<'a> {
    /// The reference (force) samples.
    pub force: &'a [f64],
    /// The reference row's range.
    pub force_range: f64,
    /// The responses, with their ranges.
    pub responses: &'a [(&'a [f64], f64)],
    /// The frames before the trigger.
    pub pretrigger: usize,
    pub fs_hz: f64,
    /// The band's top line.
    pub band_top_line: usize,
    pub double_hit_pct: f64,
    pub tip_warn_db: f64,
    pub target: Option<ForceTarget>,
}

/// A double hit's numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct DoubleHit {
    /// The frame of the later peak, from the block's start.
    pub frame: usize,
    /// The later peak over the main one.
    pub ratio: f64,
}

/// The force target's verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetCheck {
    pub target: f64,
    pub tolerance_pct: f64,
    /// `(peak - target) / target`, percent.
    pub diff_pct: f64,
    /// `high`, `low` or `on_target`.
    pub verdict: &'static str,
}

/// A hit's report.
#[derive(Debug, Clone, PartialEq)]
pub struct HitReport {
    /// The force's largest magnitude, in the hammer row's unit.
    pub peak: f64,
    pub peak_frame: usize,
    /// The overloaded channels: `0` the reference, `1 + k` response `k`.
    pub overloaded: Vec<usize>,
    pub double_hit: Option<DoubleHit>,
    pub tip_drop_db: f64,
    pub tip_warning: bool,
    /// Per response: `Some(true)` when it showed no response, `None` when not judged.
    pub no_response: Vec<Option<bool>>,
    pub target: Option<TargetCheck>,
    /// Every reason the automatic rule rejects the hit for, in order: `overload`,
    /// `double_hit`.
    pub reasons: Vec<&'static str>,
}

impl HitReport {
    pub fn rejected(&self) -> Option<&'static str> {
        self.reasons.first().copied()
    }

    pub fn warnings(&self) -> Vec<&'static str> {
        let mut w = Vec::new();
        if self.tip_warning {
            w.push("tip_roll_off");
        }
        if self.no_response.contains(&Some(true)) {
            w.push("no_response");
        }
        if let Some(t) = self.target.as_ref().filter(|t| t.verdict != "on_target") {
            w.push(if t.verdict == "high" {
                "force_high"
            } else {
                "force_low"
            });
        }
        w
    }

    /// The report as the wire and the record carry it. The target's fields are absent
    /// when no target is set.
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "peak": self.peak,
            "peak_frame": self.peak_frame,
            "overloaded": self.overloaded,
            "double_hit": self.double_hit.as_ref().map(|d| json!({"frame": d.frame, "ratio": d.ratio})),
            "tip_drop_db": self.tip_drop_db,
            "tip_warning": self.tip_warning,
            "no_response": self.no_response,
            "rejected": self.rejected(),
            "reasons": self.reasons,
            "warnings": self.warnings(),
        });
        if let (Some(t), Some(o)) = (&self.target, v.as_object_mut()) {
            o.insert("target".into(), json!(t.target));
            o.insert("target_tolerance_pct".into(), json!(t.tolerance_pct));
            o.insert("target_diff_pct".into(), json!(t.diff_pct));
            o.insert("target_verdict".into(), json!(t.verdict));
        }
        v
    }
}

/// The double-hit check alone: `None` for a clean hit.
pub fn double_hit(force: &[f64], fraction: f64) -> Option<DoubleHit> {
    let (p, peak) = peak_of(force);
    if peak <= 0.0 {
        return None;
    }
    let end = (p..force.len()).find(|&n| force[n].abs() < PULSE_END_FRACTION * peak)?;
    let (q, later) = force[end..]
        .iter()
        .enumerate()
        .fold((0, 0.0f64), |(bi, bv), (i, v)| {
            if v.abs() > bv {
                (i, v.abs())
            } else {
                (bi, bv)
            }
        });
    (later > fraction * peak).then(|| DoubleHit {
        frame: end + q,
        ratio: later / peak,
    })
}

fn peak_of(x: &[f64]) -> (usize, f64) {
    x.iter().enumerate().fold((0, 0.0f64), |(bi, bv), (i, v)| {
        if v.abs() > bv {
            (i, v.abs())
        } else {
            (bi, bv)
        }
    })
}

/// The force autospectrum's drop, dB, from its largest value over lines `0..=top` to
/// its value at `top`.
pub fn tip_drop_db(force: &[f64], top: usize) -> f64 {
    let n = force.len().next_power_of_two();
    let mut buf: Vec<(f64, f64)> = force.iter().map(|v| (*v, 0.0)).collect();
    buf.resize(n, (0.0, 0.0));
    hertzforge_dsp::fft::fft_in_place(&mut buf);
    let top = top.min(n / 2);
    let p = |k: usize| buf[k].0 * buf[k].0 + buf[k].1 * buf[k].1;
    let max = (0..=top).map(p).fold(0.0, f64::max);
    let at = p(top);
    if max <= 0.0 {
        return 0.0;
    }
    if at <= 0.0 {
        return f64::INFINITY;
    }
    10.0 * (max / at).log10()
}

/// The force target's check.
pub fn target_check(peak: f64, t: &ForceTarget) -> TargetCheck {
    let diff = (peak - t.peak) / t.peak * 100.0;
    let verdict = if diff.abs() <= t.tolerance_pct {
        "on_target"
    } else if diff > 0.0 {
        "high"
    } else {
        "low"
    };
    TargetCheck {
        target: t.peak,
        tolerance_pct: t.tolerance_pct,
        diff_pct: diff,
        verdict,
    }
}

/// Every check on one block.
pub fn check(h: &HitInput<'_>) -> HitReport {
    let frac = hertzforge_hal::INPUT_OVERLOAD_FRACTION;
    let over = |x: &[f64], range: f64| range > 0.0 && x.iter().any(|v| v.abs() >= frac * range);
    let mut overloaded = Vec::new();
    if over(h.force, h.force_range) {
        overloaded.push(0);
    }
    for (k, (y, r)) in h.responses.iter().enumerate() {
        if over(y, *r) {
            overloaded.push(1 + k);
        }
    }
    let (peak_frame, peak) = peak_of(h.force);
    let dh = double_hit(h.force, h.double_hit_pct / 100.0);
    let tip = tip_drop_db(h.force, h.band_top_line);
    let no_response = h
        .responses
        .iter()
        .map(|(y, _)| {
            let pre = h.pretrigger.min(y.len());
            if pre == 0 {
                return None;
            }
            let rms = (y[..pre].iter().map(|v| v * v).sum::<f64>() / pre as f64).sqrt();
            let post = y[pre..].iter().fold(0.0f64, |m, v| m.max(v.abs()));
            Some(post <= NO_RESPONSE_RATIO * rms)
        })
        .collect();
    let target = h.target.as_ref().map(|t| target_check(peak, t));
    let mut reasons = Vec::new();
    if !overloaded.is_empty() {
        reasons.push("overload");
    }
    if dh.is_some() {
        reasons.push("double_hit");
    }
    HitReport {
        peak,
        peak_frame,
        overloaded,
        double_hit: dh,
        tip_drop_db: tip,
        tip_warning: tip > h.tip_warn_db,
        no_response,
        target,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A half-sine pulse of `width` samples and `peak` at `at`.
    fn pulse(x: &mut [f64], at: usize, width: usize, peak: f64) {
        for k in 0..width {
            if let Some(v) = x.get_mut(at + k) {
                *v += peak * (std::f64::consts::PI * (k as f64 + 0.5) / width as f64).sin();
            }
        }
    }

    fn input<'a>(
        force: &'a [f64],
        resp: &'a [(&'a [f64], f64)],
        target: Option<ForceTarget>,
    ) -> HitInput<'a> {
        HitInput {
            force,
            force_range: 100.0,
            responses: resp,
            pretrigger: 20,
            fs_hz: 1024.0,
            band_top_line: 200,
            double_hit_pct: 5.0,
            tip_warn_db: 20.0,
            target,
        }
    }

    #[test]
    fn modal_test_a_true_double_hit_is_caught_and_a_clean_hit_is_not() {
        let mut clean = vec![0.0; 1024];
        pulse(&mut clean, 20, 5, 20.0);
        assert_eq!(double_hit(&clean, 0.05), None);
        let mut twice = clean.clone();
        pulse(&mut twice, 80, 5, 4.0);
        let d = double_hit(&twice, 0.05).expect("caught");
        assert!((d.ratio - 0.2).abs() < 0.02, "{d:?}");
        assert!((80..85).contains(&d.frame));
        let r = check(&input(&twice, &[], None));
        assert_eq!(r.rejected(), Some("double_hit"));
    }

    /// A force cell's decaying ringing after the pulse, of amplitude `amp`.
    fn ring(x: &mut [f64], amp: f64) {
        for (n, v) in x.iter_mut().enumerate().take(400).skip(25) {
            let t = (n - 25) as f64 / 1024.0;
            *v += amp * (2.0 * std::f64::consts::PI * 300.0 * t).sin() * (-t * 20.0).exp();
        }
    }

    #[test]
    fn modal_test_force_cell_ringing_below_the_fraction_is_not_a_double_hit() {
        let mut f = vec![0.0; 1024];
        pulse(&mut f, 20, 5, 20.0);
        // Ringing after the pulse at 3 percent of the peak, decaying.
        ring(&mut f, 0.6);
        assert_eq!(double_hit(&f, 0.05), None);
        // The same ringing at 8 percent is one.
        let mut g = vec![0.0; 1024];
        pulse(&mut g, 20, 5, 20.0);
        ring(&mut g, 1.6);
        assert!(double_hit(&g, 0.05).is_some());
    }

    #[test]
    fn modal_test_overload_rejects_with_its_channel() {
        let mut f = vec![0.0; 1024];
        pulse(&mut f, 20, 5, 120.0);
        let y = vec![0.0; 1024];
        let resp = [(&y[..], 10.0)];
        let r = check(&input(&f, &resp, None));
        assert_eq!(r.overloaded, vec![0]);
        assert_eq!(r.rejected(), Some("overload"));
        let mut y2 = vec![0.0; 1024];
        y2[100] = 9.8;
        let mut f2 = vec![0.0; 1024];
        pulse(&mut f2, 20, 5, 20.0);
        let resp2 = [(&y2[..], 10.0)];
        assert_eq!(check(&input(&f2, &resp2, None)).overloaded, vec![1]);
    }

    #[test]
    fn modal_test_tip_roll_off_warns_for_a_soft_tip() {
        let mut hard = vec![0.0; 1024];
        pulse(&mut hard, 20, 3, 20.0);
        let mut soft = vec![0.0; 1024];
        pulse(&mut soft, 20, 40, 20.0);
        let rh = check(&input(&hard, &[], None));
        let rs = check(&input(&soft, &[], None));
        assert!(!rh.tip_warning, "{}", rh.tip_drop_db);
        assert!(rs.tip_warning, "{}", rs.tip_drop_db);
        assert!(rs.reasons.is_empty(), "a warning, never a rejection");
    }

    #[test]
    fn modal_test_no_response_is_warned_and_not_judged_without_pretrigger() {
        let mut f = vec![0.0; 1024];
        pulse(&mut f, 20, 5, 20.0);
        let noise: Vec<f64> = (0..1024)
            .map(|n| 0.01 * ((n * 7919 % 13) as f64 - 6.0))
            .collect();
        let mut live = noise.clone();
        for (n, v) in live.iter_mut().enumerate().skip(20) {
            *v += 2.0 * (-(n as f64 - 20.0) / 100.0).exp();
        }
        let resp = [(&noise[..], 10.0), (&live[..], 10.0)];
        let r = check(&input(&f, &resp, None));
        assert_eq!(r.no_response, vec![Some(true), Some(false)]);
        assert!(r.warnings().contains(&"no_response"));
        let mut h = input(&f, &resp, None);
        h.pretrigger = 0;
        assert_eq!(check(&h).no_response, vec![None, None]);
    }

    #[test]
    fn modal_test_force_target_high_low_on_target_and_never_rejects() {
        let t = ForceTarget {
            peak: 20.0,
            tolerance_pct: 20.0,
        };
        let at = |peak: f64, t: ForceTarget| {
            let mut f = vec![0.0; 1024];
            f[30] = peak;
            check(&input(&f, &[], Some(t)))
        };
        let on = at(22.0, t);
        let c = on.target.as_ref().unwrap();
        assert_eq!(c.verdict, "on_target");
        assert!((c.diff_pct - 10.0).abs() < 1e-9);
        assert!(on.reasons.is_empty());
        let high = at(25.0, t);
        assert_eq!(high.target.as_ref().unwrap().verdict, "high");
        assert!(high.reasons.is_empty(), "a signal only by default");
        assert!(high.warnings().contains(&"force_high"));
        let low = at(15.0, t);
        assert_eq!(low.target.as_ref().unwrap().verdict, "low");
        assert!((low.target.as_ref().unwrap().diff_pct + 25.0).abs() < 1e-9);
        assert!(low.warnings().contains(&"force_low"));
        // Far off its target, a hit is still never rejected for it.
        assert_eq!(at(60.0, t).rejected(), None);
        assert_eq!(at(2.0, t).rejected(), None);
        let j = high.to_json();
        assert_eq!(j["target_verdict"], "high");
        assert_eq!(j["target"], 20.0);
    }

    #[test]
    fn modal_test_no_target_means_no_target_fields() {
        let mut f = vec![0.0; 1024];
        f[30] = 20.0;
        let r = check(&input(&f, &[], None));
        assert!(r.target.is_none());
        let j = r.to_json();
        for k in [
            "target",
            "target_tolerance_pct",
            "target_diff_pct",
            "target_verdict",
        ] {
            assert!(j.get(k).is_none(), "{k}");
        }
    }
}
