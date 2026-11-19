//! `srs_ratio`: the built-in SRS shock law.
//!
//! - **Seed.** One gain per synthesis component: `1 / |H(fc)|`, the
//!   identified gain at the component's frequency as the core floored it, so
//!   the first pulse's response lands near the target. Where the run
//!   identified nothing, every gain is 1.
//! - **Each pulse.** Per component, the target over the measured SRS at the
//!   band nearest the component's frequency, clamped to `max_db_per_pulse`
//!   either way. A band with no measured response, or no target, holds
//!   (gain 1): there is no ratio to trust there.
//! - **What the law returns** is a multiplicative update to each component of
//!   the table in force. The core applies it, after capping it by what its
//!   prediction says each limit channel can take; a falling gain always wins
//!   over a rising one.
//!
//! The update is the classic per-band SRS iteration: the same arithmetic as
//! the SRS run's own per-pulse update, without the limit cap, which the core
//! applies.
//!
//! This module is the law's declaration and its arithmetic and nothing else.
//! The controller's native law and the law file `laws/shock-srs-ratio` both
//! wrap it.

use crate::{DiagnosticDecl, NumberParam};

/// The law's catalog id.
pub const ID: &str = "srs_ratio";
/// The law's name in its manifest.
pub const NAME: &str = "SRS ratio";
/// The law's version in its manifest.
pub const VERSION: &str = "0.1.0";
/// What the law does, for its manifest.
pub const SUMMARY: &str = "Starts each component from the inverse of the identified gain at \
    its frequency, then after each pulse scales it by the target over the measured response \
    at the nearest band, limited per pulse. The controller caps every change by what the limit \
    channels can take.";

/// The parameters, in form order: `max_db_per_pulse`.
pub const KNOBS: &[NumberParam] = &[NumberParam {
    key: "max_db_per_pulse",
    label: "Largest change per pulse",
    unit: "dB",
    help: "The most any one component of the drive may rise or fall from one pulse to the \
           next. Smaller values settle more gently when the measured response is noisy.",
    min: 0.5,
    max: 5.0,
    step: 0.5,
    default: 3.0,
}];

/// The values the law reports on each call.
pub const DIAGNOSTICS: &[DiagnosticDecl] = &[
    DiagnosticDecl {
        key: "worst_band_error_db",
        label: "Worst band error",
        unit: "dB",
        help: "The last pulse's measured response against the target at the band furthest from \
               it, in dB (0 at the seed).",
    },
    DiagnosticDecl {
        key: "worst_band_hz",
        label: "Worst band",
        unit: "Hz",
        help: "The frequency of that band (0 at the seed).",
    },
    DiagnosticDecl {
        key: "clamped_components",
        label: "Components at the change limit",
        unit: "components",
        help: "Components whose change the law held to its largest change per pulse on this \
               call.",
    },
];

/// The parameter values a law is built with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SrsRatioParams {
    /// The most a component's gain may move in one pulse, either way, dB.
    pub max_db_per_pulse: f64,
}

impl Default for SrsRatioParams {
    /// The declared defaults.
    fn default() -> Self {
        SrsRatioParams {
            max_db_per_pulse: KNOBS[0].default,
        }
    }
}

impl SrsRatioParams {
    /// Set the parameter `key` to `value`, checked against its declaration.
    /// Leaves the values unchanged on an unknown key or an out-of-range value.
    pub fn set(&mut self, key: &str, value: f64) -> Result<(), String> {
        let Some(p) = KNOBS.iter().find(|p| p.key == key) else {
            return Err(format!("`{key}` is not a parameter of this law"));
        };
        if !value.is_finite() || value < p.min || value > p.max {
            return Err(format!(
                "{key} is {value}; it runs from {} to {}",
                p.min, p.max
            ));
        }
        self.max_db_per_pulse = value;
        Ok(())
    }
}

/// What one call did besides producing the gains: the values behind
/// [`DIAGNOSTICS`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StepReport {
    /// The worst band's measured response against its target, dB (0 at the
    /// seed, or when no band was comparable).
    pub worst_band_error_db: f64,
    /// That band's frequency, Hz (0 at the seed, or when no band was
    /// comparable).
    pub worst_band_hz: f64,
    /// Components whose change was held to `max_db_per_pulse`.
    pub clamped_components: usize,
}

impl StepReport {
    /// The report as `(key, value)` pairs, in [`DIAGNOSTICS`] order.
    pub fn values(&self) -> [(&'static str, f64); 3] {
        [
            (DIAGNOSTICS[0].key, self.worst_band_error_db),
            (DIAGNOSTICS[1].key, self.worst_band_hz),
            (DIAGNOSTICS[2].key, self.clamped_components as f64),
        ]
    }
}

/// One call's result: a gain per component, in table order, and the report.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// One multiplicative update per component, finite and above zero.
    pub gains: Vec<f64>,
    /// What the call did.
    pub report: StepReport,
}

/// The law's state for one run.
#[derive(Debug, Clone)]
pub struct SrsRatio {
    params: SrsRatioParams,
    band_hz: Vec<f64>,
    component_hz: Vec<f64>,
    /// For each component, the band nearest its frequency.
    nearest: Vec<usize>,
    component_gain: Option<Vec<f64>>,
}

/// The index of the band in `band_hz` nearest `f_hz` (the first on a tie; 0
/// for an empty grid).
pub fn nearest_band(band_hz: &[f64], f_hz: f64) -> usize {
    let mut best = 0usize;
    let mut best_d = f64::INFINITY;
    for (i, &fb) in band_hz.iter().enumerate() {
        let d = (fb - f_hz).abs();
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

impl SrsRatio {
    /// Build the law for a run: the analysis band centres `band_hz`, the
    /// synthesis components' frequencies `component_hz` (table order), and the
    /// core's floored `|H|` at each component (`None` when the run identified
    /// nothing).
    pub fn new(
        params: SrsRatioParams,
        band_hz: &[f64],
        component_hz: &[f64],
        component_gain: Option<&[f64]>,
    ) -> Result<Self, String> {
        if band_hz.is_empty() {
            return Err("the band grid is empty".into());
        }
        if component_hz.is_empty() {
            return Err("the component table is empty".into());
        }
        if let Some(h) = component_gain {
            if h.len() != component_hz.len() {
                return Err(format!(
                    "{} identified gains for {} components",
                    h.len(),
                    component_hz.len()
                ));
            }
            if let Some(j) = h.iter().position(|&g| !(g.is_finite() && g > 0.0)) {
                return Err(format!(
                    "the identified gain at component {j} is {}; it must be above zero",
                    h[j]
                ));
            }
        }
        if !(params.max_db_per_pulse.is_finite() && params.max_db_per_pulse > 0.0) {
            return Err(format!(
                "max_db_per_pulse is {}; it must be above zero",
                params.max_db_per_pulse
            ));
        }
        Ok(SrsRatio {
            params,
            band_hz: band_hz.to_vec(),
            component_hz: component_hz.to_vec(),
            nearest: component_hz
                .iter()
                .map(|&f| nearest_band(band_hz, f))
                .collect(),
            component_gain: component_gain.map(<[f64]>::to_vec),
        })
    }

    /// The parameters the law was built with.
    pub fn params(&self) -> &SrsRatioParams {
        &self.params
    }

    /// Components the law returns a gain for.
    pub fn components(&self) -> usize {
        self.component_hz.len()
    }

    /// The starting gains, before any pulse has fired: `1 / |H(fc)|` per
    /// component, or 1 where the run identified nothing.
    pub fn seed(&mut self) -> Result<Step, String> {
        let gains = match &self.component_gain {
            Some(h) => h.iter().map(|&g| 1.0 / g).collect(),
            None => vec![1.0; self.component_hz.len()],
        };
        Ok(Step {
            gains,
            report: StepReport::default(),
        })
    }

    /// The next gains, from the `target` at the scale the last pulse was
    /// commanded at and the last pulse's `measured` SRS, both on the band grid:
    /// per component, the target over the measured response at its nearest
    /// band, held at 1 where either is not above zero, clamped to
    /// `max_db_per_pulse` either way.
    pub fn iterate(&mut self, target: &[f64], measured: &[f64]) -> Result<Step, String> {
        let bands = self.band_hz.len();
        if target.len() != bands || measured.len() != bands {
            return Err(format!(
                "the target has {} bands and the measured response {}; the grid has {bands}",
                target.len(),
                measured.len()
            ));
        }
        let max_lin = 10.0_f64.powf(self.params.max_db_per_pulse / 20.0);
        let min_lin = 1.0 / max_lin;
        let mut clamped = 0usize;
        let gains = self
            .nearest
            .iter()
            .map(|&b| {
                let (tgt, meas) = (target[b], measured[b]);
                // No trusted ratio on a dead band: hold.
                let ratio = if meas > 0.0 && tgt > 0.0 {
                    tgt / meas
                } else {
                    1.0
                };
                let g = ratio.clamp(min_lin, max_lin);
                if (g - ratio).abs() > 1e-12 {
                    clamped += 1;
                }
                g
            })
            .collect();
        // The band furthest from its target.
        let mut worst: Option<(f64, usize)> = None;
        for b in 0..bands {
            let (tgt, meas) = (target[b], measured[b]);
            if tgt > 0.0 && meas > 0.0 {
                let db = 20.0 * (meas / tgt).log10();
                if worst.is_none_or(|(w, _)| db.abs() > w.abs()) {
                    worst = Some((db, b));
                }
            }
        }
        Ok(Step {
            gains,
            report: StepReport {
                worst_band_error_db: worst.map_or(0.0, |(db, _)| db),
                worst_band_hz: worst.map_or(0.0, |(_, b)| self.band_hz[b]),
                clamped_components: clamped,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_declared_ones_and_inside_their_ranges() {
        assert_eq!(SrsRatioParams::default().max_db_per_pulse, 3.0);
        for k in KNOBS {
            assert!(k.min <= k.default && k.default <= k.max, "{}", k.key);
        }
    }

    #[test]
    fn set_checks_the_key_and_the_range() {
        let mut p = SrsRatioParams::default();
        p.set("max_db_per_pulse", 1.5).unwrap();
        assert_eq!(p.max_db_per_pulse, 1.5);
        assert!(p.set("max_db_per_pulse", 6.0).is_err());
        assert!(p.set("beta", 0.5).is_err());
        assert!(p.set("max_db_per_pulse", f64::NAN).is_err());
        assert_eq!(p.max_db_per_pulse, 1.5, "a refused value changes nothing");
    }

    #[test]
    fn new_refuses_mismatched_grids_and_unusable_gains() {
        let bands = [100.0, 125.0, 160.0];
        let comps = [100.0, 160.0];
        let p = SrsRatioParams::default();
        assert!(SrsRatio::new(p, &[], &comps, None).is_err());
        assert!(SrsRatio::new(p, &bands, &[], None).is_err());
        assert!(SrsRatio::new(p, &bands, &comps, Some(&[1.0])).is_err());
        assert!(SrsRatio::new(p, &bands, &comps, Some(&[1.0, 0.0])).is_err());
        assert!(SrsRatio::new(p, &bands, &comps, Some(&[1.0, f64::NAN])).is_err());
        let zero = SrsRatioParams {
            max_db_per_pulse: 0.0,
        };
        assert!(SrsRatio::new(zero, &bands, &comps, None).is_err());
        let mut law = SrsRatio::new(p, &bands, &comps, Some(&[1.0, 2.0])).unwrap();
        assert_eq!(law.components(), 2);
        assert!(law
            .iterate(&[1.0; 2], &[1.0; 3])
            .unwrap_err()
            .contains("bands"));
    }

    #[test]
    fn the_seed_inverts_the_identified_gain_or_holds_without_one() {
        let bands = [100.0, 125.0, 160.0];
        let comps = [100.0, 130.0, 160.0];
        let p = SrsRatioParams::default();
        let mut law = SrsRatio::new(p, &bands, &comps, Some(&[0.5, 2.0, 4.0])).unwrap();
        let s = law.seed().unwrap();
        assert_eq!(s.gains, vec![2.0, 0.5, 0.25]);
        assert_eq!(s.report, StepReport::default());
        let mut blind = SrsRatio::new(p, &bands, &comps, None).unwrap();
        assert_eq!(blind.seed().unwrap().gains, vec![1.0; 3]);
    }

    #[test]
    fn each_component_follows_its_nearest_band_clamped_and_a_dead_band_holds() {
        let bands = [100.0, 200.0, 400.0];
        // 130 Hz is nearest 100; 310 nearest 400; 205 nearest 200.
        let comps = [130.0, 205.0, 310.0];
        let p = SrsRatioParams::default(); // 3 dB
        let mut law = SrsRatio::new(p, &bands, &comps, None).unwrap();
        let target = [10.0, 10.0, 10.0];
        // Band 0 reads 1 dB low; band 1 reads 10 dB high (clamped to -3 dB);
        // band 2 is dead.
        let measured = [10.0 * 10f64.powf(-1.0 / 20.0), 10.0 * 10f64.powf(0.5), 0.0];
        let s = law.iterate(&target, &measured).unwrap();
        assert!((s.gains[0] - 10f64.powf(1.0 / 20.0)).abs() < 1e-12);
        assert!((s.gains[1] - 10f64.powf(-3.0 / 20.0)).abs() < 1e-12);
        assert_eq!(s.gains[2], 1.0, "a dead band holds");
        assert_eq!(s.report.clamped_components, 1);
        assert!((s.report.worst_band_error_db - 10.0).abs() < 1e-9);
        assert_eq!(s.report.worst_band_hz, 200.0);
        assert_eq!(nearest_band(&bands, 150.0), 0, "a tie goes to the first");
    }

    #[test]
    fn the_report_values_follow_the_declarations() {
        let v = StepReport {
            worst_band_error_db: -1.5,
            worst_band_hz: 400.0,
            clamped_components: 2,
        }
        .values();
        let keys: Vec<&str> = v.iter().map(|(k, _)| *k).collect();
        let declared: Vec<&str> = DIAGNOSTICS.iter().map(|d| d.key).collect();
        assert_eq!(keys, declared);
        assert_eq!(v[2].1, 2.0);
    }
}
