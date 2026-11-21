//! The time windows of an impact block: the force window on the reference and the
//! exponential window on every channel.
//!
//! * **Force window.** Defined from the trigger sample, in samples. With `pre` the
//!   block's trigger sample (the pre-trigger length), `width` the pulse allowance after
//!   the trigger and `fall` the fall's length:
//!
//!   ```text
//!   w[n] = 1                                         n < pre + width
//!   w[n] = 0.5 (1 + cos(pi k / fall)), k = n - pre - width,   0 <= k < fall
//!   w[n] = 0                                         n >= pre + width + fall
//!   ```
//!
//!   `width` is the setup's fraction of the block, counted after the trigger; `fall`
//!   is the setup's fraction of `width` (10 % unless set), at least one sample. A
//!   window longer than the block is refused at configure, never clamped.
//! * **Exponential window.** `w[n] = exp(-(n / fs) / tau)` from the start of the block,
//!   on every channel, force and responses alike, with `tau` set by the end value
//!   `w(T) = exp(-T / tau)`, `T = N / fs`. It adds `1 / tau` to every mode's decay
//!   rate, so a damping ratio measured at natural frequency `f_n` (Hz) exceeds the true
//!   one by `1 / (2 pi f_n tau)` ([`ExpWindow::added_damping`]). `tau` and `1 / tau` are
//!   stored with every FRF.

use crate::setup::{ExpWindowSet, ForceWindowSet};
use serde_json::{json, Value};

/// The force window of a block.
#[derive(Debug, Clone, PartialEq)]
pub struct ForceWindow {
    /// The trigger sample: the pre-trigger length.
    pub pre: usize,
    /// Samples at one after the trigger.
    pub width: usize,
    /// Samples of the half-cosine fall.
    pub fall: usize,
    pub block: usize,
}

/// The force window's `width` and `fall` in samples for a block of `block`, or why it
/// cannot be made: `None` when the width is not finite or not above zero.
pub fn force_window_samples(set: &ForceWindowSet, block: usize) -> Option<(usize, usize)> {
    if !(set.width_pct.is_finite() && set.width_pct > 0.0) {
        return None;
    }
    let width = ((set.width_pct / 100.0) * block as f64).round() as usize;
    if width == 0 {
        return None;
    }
    let fall = if set.taper_pct.is_finite() {
        ((set.taper_pct / 100.0) * width as f64).round().max(1.0) as usize
    } else {
        1
    };
    Some((width, fall))
}

impl ForceWindow {
    /// The window of a block whose trigger is at sample `pre`. The setup's refusals
    /// have already refused a window that does not fit ([`force_window_samples`]).
    pub fn new(set: &ForceWindowSet, block: usize, pre: usize) -> ForceWindow {
        let (width, fall) = force_window_samples(set, block).unwrap_or((block, 1));
        ForceWindow {
            pre,
            width,
            fall,
            block,
        }
    }

    /// The first sample after the window's flat part: `pre + width`.
    pub fn flat_end(&self) -> usize {
        self.pre + self.width
    }

    /// The window's value at sample `n`.
    pub fn at(&self, n: usize) -> f64 {
        let flat = self.flat_end();
        if n < flat {
            1.0
        } else if n < flat + self.fall {
            let k = (n - flat) as f64;
            0.5 * (1.0 + (std::f64::consts::PI * k / self.fall as f64).cos())
        } else {
            0.0
        }
    }

    pub fn values(&self) -> Vec<f64> {
        (0..self.block).map(|n| self.at(n)).collect()
    }
}

/// The exponential window of a block.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpWindow {
    /// The time constant, seconds.
    pub tau_s: f64,
    /// The value at the block's end, `exp(-T / tau)`.
    pub end: f64,
    pub block: usize,
    pub fs_hz: f64,
}

impl ExpWindow {
    /// The window whose value at the block's end, `T = block / fs`, is `end_pct`
    /// percent.
    pub fn new(set: &ExpWindowSet, block: usize, fs_hz: f64) -> ExpWindow {
        let end = (set.end_pct / 100.0).clamp(1e-9, 0.999_999);
        let t = block as f64 / fs_hz;
        ExpWindow {
            tau_s: -t / end.ln(),
            end,
            block,
            fs_hz,
        }
    }

    /// The value at sample `n` (`t = n / fs`).
    pub fn at(&self, n: usize) -> f64 {
        (-(n as f64 / self.fs_hz) / self.tau_s).exp()
    }

    pub fn values(&self) -> Vec<f64> {
        (0..self.block).map(|n| self.at(n)).collect()
    }

    /// The damping ratio the window adds to a mode at `f_n` Hz: `1 / (2 pi f_n tau)`.
    pub fn added_damping(&self, f_n_hz: f64) -> f64 {
        1.0 / (2.0 * std::f64::consts::PI * f_n_hz * self.tau_s)
    }
}

/// The windows one measurement's blocks are taken with: the per-channel data windows
/// (applied to the samples) and the analysis window (applied with the cross spectra).
#[derive(Debug, Clone, PartialEq)]
pub struct Windows {
    pub force: Option<ForceWindow>,
    pub exp: Option<ExpWindow>,
    /// `uniform` (frame-synchronous: impact, burst, periodic) or `hann` (continuous
    /// random).
    pub analysis: &'static str,
}

impl Windows {
    /// The data window of a reference row: the force window times the exponential
    /// one, each when on (the exponential window is on every channel alike).
    pub fn reference(&self, block: usize) -> Vec<f64> {
        (0..block)
            .map(|n| {
                self.force.as_ref().map_or(1.0, |w| w.at(n))
                    * self.exp.as_ref().map_or(1.0, |w| w.at(n))
            })
            .collect()
    }

    /// The data window of a response row: the exponential one, when on.
    pub fn response(&self, block: usize) -> Vec<f64> {
        (0..block)
            .map(|n| self.exp.as_ref().map_or(1.0, |w| w.at(n)))
            .collect()
    }

    /// The analysis window.
    pub fn analysis_values(&self, block: usize) -> Vec<f64> {
        if self.analysis == "hann" {
            hertzforge_dsp::window::hann(block)
        } else {
            vec![1.0; block]
        }
    }

    /// The windows as the results record them, so a reader can undo the exponential
    /// window's damping.
    pub fn to_json(&self) -> Value {
        json!({
            "analysis": self.analysis,
            "force": self.force.as_ref().map(|f| json!({"pre_frames": f.pre, "width_frames": f.width, "fall_frames": f.fall})),
            "exponential": self.exp.as_ref().map(|e| json!({
                "tau_s": e.tau_s,
                "inv_tau_per_s": 1.0 / e.tau_s,
                "end": e.end,
                "added_damping": "1/(2*pi*f_n*tau_s)",
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(width_pct: f64, taper_pct: f64) -> ForceWindowSet {
        ForceWindowSet {
            on: true,
            width_pct,
            taper_pct,
        }
    }

    #[test]
    fn modal_test_force_window_is_one_to_the_trigger_plus_width_then_a_half_cosine() {
        // block 1000, trigger at 50, width 10 % (100), fall 50 % of it (50).
        let w = ForceWindow::new(&set(10.0, 50.0), 1000, 50);
        assert_eq!((w.pre, w.width, w.fall), (50, 100, 50));
        assert_eq!(w.at(0), 1.0);
        assert_eq!(w.at(149), 1.0);
        // The fall begins at pre + width with 1, k = n - pre - width.
        assert_eq!(w.at(150), 1.0);
        assert!((w.at(175) - 0.5).abs() < 1e-12);
        let k = 49.0;
        assert!((w.at(199) - 0.5 * (1.0 + (std::f64::consts::PI * k / 50.0).cos())).abs() < 1e-15);
        assert!(w.at(199) > 0.0 && w.at(199) < 0.01);
        assert_eq!(w.at(200), 0.0);
        assert_eq!(w.at(999), 0.0);
        let v = w.values();
        assert_eq!(v.len(), 1000);
        assert!(v.windows(2).all(|p| p[1] <= p[0]));
    }

    #[test]
    fn modal_test_force_window_fall_defaults_to_a_tenth_and_is_at_least_one_sample() {
        assert_eq!(
            force_window_samples(&set(10.0, 10.0), 1024),
            Some((102, 10))
        );
        // A fall that rounds to nothing is one sample.
        assert_eq!(force_window_samples(&set(0.4, 10.0), 1024), Some((4, 1)));
        let w = ForceWindow::new(&set(0.4, 10.0), 1024, 10);
        assert_eq!((w.at(13), w.at(14), w.at(15)), (1.0, 1.0, 0.0));
        // No width: refused, not clamped.
        assert_eq!(force_window_samples(&set(0.0, 10.0), 1024), None);
        assert_eq!(force_window_samples(&set(-5.0, 10.0), 1024), None);
        assert_eq!(force_window_samples(&set(f64::NAN, 10.0), 1024), None);
        assert_eq!(force_window_samples(&set(0.01, 10.0), 1024), None);
    }

    #[test]
    fn modal_test_exponential_window_meets_its_end_value_and_added_damping() {
        let fs = 1024.0;
        let e = ExpWindow::new(
            &ExpWindowSet {
                on: true,
                end_pct: 5.0,
            },
            1024,
            fs,
        );
        // w(T) = exp(-T/tau) = 0.05 at T = 1 s.
        assert!((e.at(1024) - 0.05).abs() < 1e-12);
        assert!((e.tau_s - 1.0 / (20f64).ln()).abs() < 1e-12);
        assert_eq!(e.at(0), 1.0);
        // The added decay rate is 1/tau: a mode's envelope exp(-zeta w t) times the
        // window is exp(-(zeta w + 1/tau) t), so the added ratio is 1/(w tau).
        let fn_hz = 50.0;
        let w = 2.0 * std::f64::consts::PI * fn_hz;
        assert!((e.added_damping(fn_hz) - 1.0 / (w * e.tau_s)).abs() < 1e-15);
        let zeta = 0.02;
        let t = 0.25;
        let lhs = (-zeta * w * t).exp() * e.at((t * fs) as usize);
        let rhs = (-(zeta + e.added_damping(fn_hz)) * w * t).exp();
        assert!((lhs - rhs).abs() < 1e-12);
        // w[n] = exp(-(n / rate) / tau), sample by sample.
        for n in [0usize, 1, 100, 1023] {
            assert_eq!(e.at(n), (-(n as f64 / fs) / e.tau_s).exp());
        }
        let ws = Windows {
            force: None,
            exp: Some(e.clone()),
            analysis: "uniform",
        };
        assert_eq!(ws.reference(1024), ws.response(1024));
        let j = ws.to_json();
        assert_eq!(j["exponential"]["inv_tau_per_s"], 1.0 / e.tau_s);
    }
}
