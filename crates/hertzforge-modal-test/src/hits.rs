//! The checks every captured impact block takes before it may be averaged, each with
//! its numbers:
//!
//! * **Overload.** A sample of the reference or a response at or above
//!   [`hertzforge_hal::INPUT_OVERLOAD_FRACTION`] of its row's range. Rejects.
//! * **Double hit.** A second impact: after the main pulse has fallen below
//!   [`PULSE_END_FRACTION`] of its peak, a later excursion of the hammer's own polarity
//!   that rises above the larger of the double-hit fraction of the main peak and
//!   [`DOUBLE_HIT_NOISE_RMS`] times the pre-trigger force's RMS. Anywhere in the block,
//!   after the force window too. Rejects. An undershoot of the opposite polarity never
//!   counts, and nor does ringing: a lobe of the hammer's polarity no larger than the
//!   opposite lobe just before it is the force cell ringing down, not an impact.
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
/// A second impact must also rise above this many times the pre-trigger force's RMS.
pub const DOUBLE_HIT_NOISE_RMS: f64 = 6.0;
/// A response's post-trigger peak must exceed this many times its pre-trigger RMS.
pub const NO_RESPONSE_RATIO: f64 = 4.0;

/// What a check is handed: one block per channel, in engineering units.
pub struct HitInput<'a> {
    /// The reference (force) samples, the block from its first sample.
    pub force: &'a [f64],
    /// The hammer's polarity: `1` or `-1`.
    pub sign: f64,
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
    /// The force window's flat end, `pre + width`, when the window is on.
    pub force_window_end: Option<usize>,
}

/// A force window that cuts into the pulse: the pulse had not fallen below
/// [`PULSE_END_FRACTION`] of its peak before the window's flat end.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowCut {
    /// The first sample after the peak below the fraction, `None` if none in the block.
    pub pulse_end_frame: Option<usize>,
    /// `pre + width`.
    pub window_end_frame: usize,
}

/// A double hit's numbers: the second impact's.
#[derive(Debug, Clone, PartialEq)]
pub struct DoubleHit {
    /// The sample of the second impact's peak, from the block's first sample.
    pub frame: usize,
    /// The second impact's peak, in the hammer row's unit, as a magnitude in the
    /// hammer's polarity.
    pub peak: f64,
    /// The second impact's peak over the main one.
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

/// The decay check's window: this fraction of the block, at least one sample.
pub const DECAY_WINDOW_FRACTION: f64 = 0.05;

/// How far a response has decayed by the frame's end, as the estimate sees it (after the
/// windows): the RMS over the frame's last [`DECAY_WINDOW_FRACTION`] of samples divided by
/// the largest RMS over any window of that length in the frame. `0` for a silent frame.
pub fn decay_ratio(x: &[f64]) -> f64 {
    let n = x.len();
    if n == 0 {
        return 0.0;
    }
    let w = ((n as f64 * DECAY_WINDOW_FRACTION).round() as usize).clamp(1, n);
    let sq: Vec<f64> = x.iter().map(|v| v * v).collect();
    let mut sum: f64 = sq[..w].iter().sum();
    let mut loud = sum;
    for k in w..n {
        sum += sq[k] - sq[k - w];
        loud = loud.max(sum);
    }
    let last: f64 = sq[n - w..].iter().sum();
    if loud <= 0.0 {
        0.0
    } else {
        (last / loud).sqrt()
    }
}

/// The decay check of one hit or frame: the response row (its table index) with the
/// largest ratio, the ratio, and whether it is over the setup's threshold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decay {
    pub row: u32,
    pub ratio: f64,
    pub warning: bool,
}

impl Decay {
    pub fn to_json(&self) -> Value {
        json!({"row": self.row, "ratio": self.ratio, "warning": self.warning})
    }
}

/// A hit's report.
#[derive(Debug, Clone, PartialEq)]
pub struct HitReport {
    /// The force's largest value in the hammer's polarity, as a magnitude, in the hammer
    /// row's unit.
    pub peak: f64,
    /// Its sample, from the block's first sample.
    pub peak_frame: usize,
    /// The overloaded channels: `0` the reference, `1 + k` response `k`.
    pub overloaded: Vec<usize>,
    pub double_hit: Option<DoubleHit>,
    pub tip_drop_db: f64,
    pub tip_warning: bool,
    /// Per response: `Some(true)` when it showed no response, `None` when not judged.
    pub no_response: Vec<Option<bool>>,
    pub target: Option<TargetCheck>,
    /// A warning only: the force window cuts into the pulse.
    pub window_cut: Option<WindowCut>,
    /// Every reason the automatic rule rejects the hit for, in order: `overload`,
    /// `double_hit`.
    pub reasons: Vec<&'static str>,
    /// A warning only: the decay check (set by the session, after the windows).
    pub decay: Option<Decay>,
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
        if self.window_cut.is_some() {
            w.push("force_window_cuts_pulse");
        }
        if self.decay.is_some_and(|d| d.warning) {
            w.push("response_not_decayed");
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
            "double_hit": self.double_hit.as_ref().map(|d| json!({"frame": d.frame, "peak": d.peak, "ratio": d.ratio})),
            "tip_drop_db": self.tip_drop_db,
            "tip_warning": self.tip_warning,
            "no_response": self.no_response,
            "force_window_cut": self.window_cut.as_ref().map(|c| json!({"pulse_end_frame": c.pulse_end_frame, "window_end_frame": c.window_end_frame})),
            "decay": self.decay.as_ref().map(Decay::to_json),
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

/// The double-hit check alone: `None` for a clean hit. `sign` is the hammer's
/// polarity, `fraction` the double-hit fraction and `pre` the pre-trigger's samples.
pub fn double_hit(force: &[f64], sign: f64, fraction: f64, pre: usize) -> Option<DoubleHit> {
    let y: Vec<f64> = force.iter().map(|v| sign * v).collect();
    let (p, peak) = peak_of(&y);
    if peak <= 0.0 {
        return None;
    }
    let end = pulse_end(&y, p, peak)?;
    let pre = pre.min(force.len());
    let rms = if pre == 0 {
        0.0
    } else {
        (force[..pre].iter().map(|v| v * v).sum::<f64>() / pre as f64).sqrt()
    };
    let threshold = (fraction * peak).max(DOUBLE_HIT_NOISE_RMS * rms);
    // Lobe by lobe from the pulse's end: a lobe of the hammer's polarity over the
    // threshold, and larger than the opposite lobe just before it, is a second impact.
    let mut n = end;
    let mut opposite = 0.0f64;
    while n < y.len() {
        if y[n] > 0.0 {
            let (mut at, mut top) = (n, y[n]);
            while n < y.len() && y[n] > 0.0 {
                if y[n] > top {
                    (at, top) = (n, y[n]);
                }
                n += 1;
            }
            if top > threshold && top > opposite {
                return Some(DoubleHit {
                    frame: at,
                    peak: top,
                    ratio: top / peak,
                });
            }
            opposite = 0.0;
        } else {
            let mut m = 0.0f64;
            while n < y.len() && y[n] <= 0.0 {
                m = m.max(-y[n]);
                n += 1;
            }
            opposite = m;
        }
    }
    None
}

/// The largest value and its sample.
fn peak_of(y: &[f64]) -> (usize, f64) {
    y.iter().enumerate().fold(
        (0, 0.0f64),
        |(bi, bv), (i, v)| {
            if *v > bv {
                (i, *v)
            } else {
                (bi, bv)
            }
        },
    )
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
    let y: Vec<f64> = h.force.iter().map(|v| h.sign * v).collect();
    let (peak_frame, peak) = peak_of(&y);
    let dh = double_hit(h.force, h.sign, h.double_hit_pct / 100.0, h.pretrigger);
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
    let window_cut = h.force_window_end.and_then(|end| {
        let pulse_end = pulse_end(&y, peak_frame, peak);
        (peak > 0.0 && pulse_end.is_none_or(|e| e >= end)).then_some(WindowCut {
            pulse_end_frame: pulse_end,
            window_end_frame: end,
        })
    });
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
        window_cut,
        reasons,
        decay: None,
    }
}

/// The first sample after the peak at `p` where the force in the hammer's polarity,
/// `y`, is below [`PULSE_END_FRACTION`] of `peak`.
pub fn pulse_end(y: &[f64], p: usize, peak: f64) -> Option<usize> {
    (p..y.len()).find(|&n| y[n] < PULSE_END_FRACTION * peak)
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
            sign: 1.0,
            force_range: 100.0,
            responses: resp,
            pretrigger: 20,
            fs_hz: 1024.0,
            band_top_line: 200,
            double_hit_pct: 5.0,
            tip_warn_db: 20.0,
            target,
            force_window_end: None,
        }
    }

    #[test]
    fn modal_test_a_force_window_cutting_the_pulse_is_a_warning_with_its_numbers() {
        // A pulse of 10 samples from the trigger at 20: it falls below 5 % at 30.
        let mut f = vec![0.0; 1024];
        pulse(&mut f, 20, 10, 20.0);
        let mut h = input(&f, &[], None);
        h.force_window_end = Some(40);
        let r = check(&h);
        assert_eq!(r.window_cut, None);
        // A window whose flat part ends at 25 cuts into it.
        h.force_window_end = Some(25);
        let r = check(&h);
        let c = r.window_cut.clone().expect("warned");
        assert_eq!(c.window_end_frame, 25);
        let end = c.pulse_end_frame.unwrap();
        assert!((28..=30).contains(&end), "{c:?}");
        assert!(r.warnings().contains(&"force_window_cuts_pulse"));
        assert_eq!(r.rejected(), None);
        let j = r.to_json();
        assert_eq!(j["force_window_cut"]["window_end_frame"], 25);
        assert_eq!(j["force_window_cut"]["pulse_end_frame"], end);
    }

    /// The built traces' rate: a 1 ms hammer pulse is 20 samples.
    const DH_FS: f64 = 20480.0;
    const DH_PRE: usize = 200;

    /// A block with a 1 ms half-sine of `peak` at the end of the pre-trigger.
    fn hit(peak: f64) -> Vec<f64> {
        let mut f = vec![0.0; 4096];
        pulse(&mut f, DH_PRE, 20, peak);
        f
    }

    fn dh(f: &[f64]) -> Option<DoubleHit> {
        double_hit(f, 1.0, 0.05, DH_PRE)
    }

    #[test]
    fn modal_test_decay_ratio_is_the_last_window_over_the_loudest() {
        // 100 samples, windows of 5: loud 2.0 at the start, 0.1 at the end.
        let mut x = vec![0.5; 100];
        x[10..15].iter_mut().for_each(|v| *v = 2.0);
        x[95..].iter_mut().for_each(|v| *v = 0.1);
        assert!(
            (decay_ratio(&x) - 0.05).abs() < 1e-12,
            "{}",
            decay_ratio(&x)
        );
        assert_eq!(decay_ratio(&[0.0; 40]), 0.0);
        assert_eq!(decay_ratio(&[1.0; 40]), 1.0);
    }

    #[test]
    fn modal_test_a_clean_half_sine_is_not_a_double_hit() {
        assert_eq!(dh(&hit(20.0)), None);
    }

    #[test]
    fn modal_test_a_second_pulse_5_ms_on_is_a_double_hit_down_to_the_fraction() {
        let gap = (0.005 * DH_FS) as usize;
        for (pct, caught) in [(50.0, true), (10.0, true), (4.0, false)] {
            let mut f = hit(20.0);
            pulse(&mut f, DH_PRE + gap, 20, 20.0 * pct / 100.0);
            let d = dh(&f);
            assert_eq!(d.is_some(), caught, "{pct} %: {d:?}");
            if let Some(d) = d {
                // The second impact's own numbers: its sample, its peak, its fraction.
                assert!(
                    (DH_PRE + gap..DH_PRE + gap + 20).contains(&d.frame),
                    "{d:?}"
                );
                assert!((d.peak - 20.0 * pct / 100.0).abs() < 0.01 * 20.0, "{d:?}");
                assert!((d.ratio - pct / 100.0).abs() < 0.01, "{d:?}");
                let r = check(&input(&f, &[], None));
                assert_eq!(r.rejected(), Some("double_hit"));
                let j = r.to_json();
                assert_eq!(j["double_hit"]["frame"], d.frame);
                assert_eq!(j["double_hit"]["peak"], d.peak);
                assert_eq!(j["double_hit"]["ratio"], d.ratio);
            }
        }
    }

    #[test]
    fn modal_test_an_opposite_undershoot_and_its_ringing_are_not_a_double_hit() {
        // After the pulse, a 20 % undershoot of the opposite sign ringing down at the
        // force cell's 2 kHz: its lobes of the hammer's sign reach 15 %, then less.
        let mut f = hit(20.0);
        let from = DH_PRE + 20;
        for (n, v) in f.iter_mut().enumerate().skip(from) {
            let t = (n - from) as f64 / DH_FS;
            *v -= 4.0 * (2.0 * std::f64::consts::PI * 2000.0 * t).sin() * (-t * 600.0).exp();
        }
        let lobe = f[from..].iter().fold(0.0f64, |m, v| m.max(*v));
        assert!(
            lobe > 0.05 * 20.0,
            "the ringing rises above the fraction: {lobe}"
        );
        assert_eq!(dh(&f), None);
    }

    #[test]
    fn modal_test_noise_at_1_25_pct_rms_of_the_peak_is_not_a_double_hit() {
        // Gaussian noise at 1.25 % RMS of the peak over the whole block.
        let mut f = hit(20.0);
        let mut z = 0x9E37_79B9_7F4A_7C15u64;
        let mut u = || {
            z ^= z << 13;
            z ^= z >> 7;
            z ^= z << 17;
            (z >> 11) as f64 / (1u64 << 53) as f64
        };
        for v in f.iter_mut() {
            let g = (-2.0 * u().max(1e-300).ln()).sqrt() * (2.0 * std::f64::consts::PI * u()).cos();
            *v += 0.0125 * 20.0 * g;
        }
        assert_eq!(dh(&f), None);
        // At the setup's lowest fraction, 1 %, under the noise itself: six times the
        // pre-trigger's RMS keeps the noise from counting.
        assert_eq!(double_hit(&f, 1.0, 0.01, DH_PRE), None);
    }

    #[test]
    fn modal_test_a_second_pulse_after_the_force_window_is_a_double_hit() {
        // The force window's flat part ends 2 ms after the trigger; a 20 % second
        // impact 50 ms on is still one.
        let mut f = hit(20.0);
        let late = DH_PRE + (0.050 * DH_FS) as usize;
        pulse(&mut f, late, 20, 4.0);
        let mut h = input(&f, &[], None);
        h.pretrigger = DH_PRE;
        h.force_window_end = Some(DH_PRE + 40);
        let r = check(&h);
        let d = r.double_hit.clone().expect("caught after the window");
        assert!(
            d.frame > DH_PRE + 40 && (late..late + 20).contains(&d.frame),
            "{d:?}"
        );
        assert_eq!(r.rejected(), Some("double_hit"));
    }

    #[test]
    fn modal_test_a_negative_hammer_is_judged_in_its_own_polarity() {
        let mut f: Vec<f64> = hit(20.0).iter().map(|v| -v).collect();
        let gap = (0.005 * DH_FS) as usize;
        let mut second = vec![0.0; f.len()];
        pulse(&mut second, DH_PRE + gap, 20, 2.0);
        for (a, b) in f.iter_mut().zip(&second) {
            *a -= b;
        }
        let mut h = input(&f, &[], None);
        h.sign = -1.0;
        let r = check(&h);
        assert!((r.peak - 20.0).abs() < 0.1, "{}", r.peak);
        assert!((DH_PRE..DH_PRE + 20).contains(&r.peak_frame));
        assert!(r.double_hit.is_some());
        // Judged as a positive hammer, the same block has no pulse of its sign.
        assert_eq!(double_hit(&f, 1.0, 0.05, DH_PRE), None);
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
