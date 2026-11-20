//! The time windows of an impact block: the force window on the reference and the
//! exponential window on every channel.
//!
//! Definitions (to be confirmed against the cited source by the lead):
//!
//! * **Force window.** One over the pulse, then a half-cosine fall to zero, zero after.
//!   Here it is one from the block's first sample through `width` samples, falls over
//!   `taper` samples as `0.5 (1 + cos(pi k / taper))`, and is zero after.
//! * **Exponential window.** `w(t) = exp(-t / tau)`, with `t` measured from the start of
//!   the block, and end value `w(T) = exp(-T / tau)`, `T` the block's length
//!   (`N / fs`). It adds `1 / tau` to every mode's decay rate, so a damping ratio
//!   measured at natural frequency `f_n` (Hz) exceeds the true one by
//!   `1 / (2 pi f_n tau)` ([`ExpWindow::added_damping`]).

use crate::setup::{ExpWindowSet, ForceWindowSet};
use serde_json::{json, Value};

/// The force window of a block.
#[derive(Debug, Clone, PartialEq)]
pub struct ForceWindow {
    /// Samples at one.
    pub width: usize,
    /// Samples of the half-cosine fall.
    pub taper: usize,
    pub block: usize,
}

impl ForceWindow {
    pub fn new(set: &ForceWindowSet, block: usize) -> ForceWindow {
        let width = ((set.width_pct / 100.0) * block as f64).round().max(1.0) as usize;
        let width = width.min(block);
        let taper = ((set.taper_pct / 100.0) * width as f64).round() as usize;
        ForceWindow {
            width,
            taper: taper.min(block - width),
            block,
        }
    }

    /// The window's value at sample `n`.
    pub fn at(&self, n: usize) -> f64 {
        if n < self.width {
            1.0
        } else if n < self.width + self.taper {
            let k = (n - self.width) as f64;
            0.5 * (1.0 + (std::f64::consts::PI * k / self.taper as f64).cos())
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
    /// one, each when on.
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
            "force": self.force.as_ref().map(|f| json!({"width_frames": f.width, "taper_frames": f.taper})),
            "exponential": self.exp.as_ref().map(|e| json!({
                "tau_s": e.tau_s,
                "end": e.end,
                "added_damping": "1/(2*pi*f_n*tau_s)",
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_test_force_window_is_one_then_a_half_cosine_then_zero() {
        let w = ForceWindow::new(
            &ForceWindowSet {
                on: true,
                width_pct: 10.0,
                taper_pct: 50.0,
            },
            1000,
        );
        assert_eq!((w.width, w.taper), (100, 50));
        assert_eq!(w.at(0), 1.0);
        assert_eq!(w.at(99), 1.0);
        assert_eq!(w.at(100), 1.0);
        assert!((w.at(125) - 0.5).abs() < 1e-12);
        assert!(w.at(149) < 0.01 && w.at(149) > 0.0);
        assert_eq!(w.at(150), 0.0);
        assert_eq!(w.at(999), 0.0);
        // Falling only.
        let v = w.values();
        assert!(v.windows(2).all(|p| p[1] <= p[0]));
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
    }
}
