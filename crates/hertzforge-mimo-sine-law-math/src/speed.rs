//! The loop-speed rule: the gain per update.
//!
//! An integrating correction `D += g Z E` crosses over at `g / tick` rad/s. It
//! must stay slower than everything between a drive and its measurement: the
//! tracking filter (group delay `2 / (2 pi B)` for two one-pole stages), the
//! structure's own response near the tone (the identification's group delay,
//! the largest within `f +- 2 B`, since a light mode inside the filter's band
//! limits the loop even off its resonance), and the update itself. So
//!
//! ```text
//! g = min(g_max, beta tick / (tau_f + tau_s + tick))
//! ```
//!
//! A fixed gain (the published `0.5`) is available as [`GainRule::Fixed`] for
//! tests only: on lightly damped structures it loses the loop.

use crate::setup::{GroupDelay, TrackingRule};

/// How the gain per update is set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GainRule {
    /// The loop-speed rule with factor `beta` and ceiling `g_max`.
    LoopSpeed {
        /// Ceiling on the gain.
        g_max: f64,
        /// Fraction of the inverse total lag the loop may run at.
        beta: f64,
    },
    /// A fixed gain every update. For tests: it ignores the loop's lags.
    Fixed {
        /// The gain.
        g: f64,
    },
}

impl Default for GainRule {
    fn default() -> Self {
        GainRule::LoopSpeed {
            g_max: 0.5,
            beta: 0.5,
        }
    }
}

/// The gain and the structure delay it read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gain {
    /// Gain per update.
    pub g: f64,
    /// The identification's group delay used, s (the largest near the tone).
    pub structure_delay_s: f64,
    /// The tracking filter's delay, s.
    pub filter_delay_s: f64,
}

/// The gain at `f_hz`.
pub fn gain(
    rule: GainRule,
    tracking: &TrackingRule,
    delay: &GroupDelay,
    tick_s: f64,
    f_hz: f64,
) -> Gain {
    let b = tracking.bandwidth_hz(f_hz);
    let tf = tracking.delay_s(f_hz);
    let ts = delay.at(f_hz, 2.0 * b);
    let g = match rule {
        GainRule::Fixed { g } => g,
        GainRule::LoopSpeed { g_max, beta } => g_max.min(beta * tick_s / (tf + ts + tick_s)),
    };
    Gain {
        g,
        structure_delay_s: ts,
        filter_delay_s: tf,
    }
}
