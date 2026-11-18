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

use crate::setup::{DelayEstimate, GroupDelay, LoopDelay, TrackingRule};

/// How the gain per update is set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GainRule {
    /// The loop-speed rule with factor `beta` and ceiling `g_max`.
    LoopSpeed {
        /// Ceiling on the gain.
        g_max: f64,
        /// Fraction of the inverse total lag the loop may run at.
        beta: f64,
        /// How the identification's group delay is read.
        delay: DelayEstimate,
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
            delay: DelayEstimate::default(),
        }
    }
}

impl GainRule {
    /// How the identification's group delay is read for this rule (the
    /// present form for a fixed gain, which only reports it).
    pub fn delay_estimate(&self) -> DelayEstimate {
        match *self {
            GainRule::LoopSpeed { delay, .. } => delay,
            GainRule::Fixed { .. } => DelayEstimate::Largest,
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
    /// The transport delay the identification does not show, s.
    pub transport_delay_s: f64,
}

/// The gain at `f_hz`.
pub fn gain(
    rule: GainRule,
    tracking: &TrackingRule,
    loop_delay: LoopDelay,
    delay: &GroupDelay,
    tick_s: f64,
    f_hz: f64,
) -> Gain {
    let b = tracking.bandwidth_hz(f_hz);
    let tf = tracking.delay_s_with(f_hz, loop_delay.tracker_delay_factor);
    let ts = delay.at(f_hz, 2.0 * b);
    let tt = loop_delay.transport_delay_s;
    let g = match rule {
        GainRule::Fixed { g } => g,
        GainRule::LoopSpeed { g_max, beta, .. } => {
            g_max.min(beta * tick_s / (tf + ts + tt + tick_s))
        }
    };
    Gain {
        g,
        structure_delay_s: ts,
        filter_delay_s: tf,
        transport_delay_s: tt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::{Identification, DEFAULT_TRACKER_DELAY_FACTOR};
    use hertzforge_sdm::MatrixStack;

    /// The rule's sum holds the tracker's delay at its stated factor and the
    /// transport delay; the defaults read as before.
    #[test]
    fn mimo_sine_the_loop_speed_rule_adds_the_transport_and_the_trackers_factor() {
        let id = Identification {
            grid_hz: vec![50.0, 150.0],
            response: MatrixStack::zeros(2, 1, 1),
            limit_response: MatrixStack::zeros(2, 0, 1),
            coherence: vec![1.0; 2],
            limit_coherence: vec![],
            trusted: vec![true; 2],
        };
        let gd = GroupDelay::of(&id);
        let tr = TrackingRule::Fixed { hz: 10.0 };
        let rule = GainRule::LoopSpeed {
            g_max: 1.0,
            beta: 0.5,
            delay: DelayEstimate::Largest,
        };
        let base = gain(rule, &tr, LoopDelay::default(), &gd, 0.01, 100.0);
        assert!((base.filter_delay_s - DEFAULT_TRACKER_DELAY_FACTOR / 10.0).abs() < 1e-15);
        assert!((base.g - 0.5 * 0.01 / (base.filter_delay_s + 0.01)).abs() < 1e-15);
        let told = gain(
            rule,
            &tr,
            LoopDelay {
                tracker_delay_factor: 0.625,
                transport_delay_s: 0.05,
            },
            &gd,
            0.01,
            100.0,
        );
        assert_eq!(told.transport_delay_s, 0.05);
        assert!((told.g - 0.5 * 0.01 / (0.0625 + 0.05 + 0.01)).abs() < 1e-15);
    }
}
