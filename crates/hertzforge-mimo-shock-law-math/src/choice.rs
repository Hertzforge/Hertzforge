//! The choice between two followers (`Derivations.md` D21): both observe every
//! pulse; for the next pulse the law gets, in each frequency region, the model
//! of the follower whose model for the latest pulse reproduced it better there,
//! with a hysteresis ratio so the choice does not flip-flop on noise.
//!
//! Under noise both followers' residuals carry the same noise energy, which
//! dilutes a ratio of residuals; so a region moves only when the residual
//! energies differ by more than the hysteresis margin on the excess over the
//! noise and by more than `z` standard deviations of the difference's noise
//! part (`Derivations.md` D21).
//!
//! The comparison is predictive: each follower's model for a pulse was made
//! before the pulse was played, so neither has seen it, and the follower that
//! was not chosen is judged on the same pulse as the one that was (the
//! residual `||Y - H D|| / ||Y||` uses the drives actually played).

use crate::provider::{prediction_residual, ModelProvider, ModelReport, ModelSource, Observation};
use crate::MatrixStack;
use std::sync::{Arc, Mutex};

/// Options of [`ChoiceModel`].
#[derive(Clone, Debug, PartialEq)]
pub struct ChoiceOptions {
    /// A region moves to the other follower when the one in force left more
    /// residual energy on the latest pulse than the other by more than
    /// `ratio^2 - 1` times the other's excess over the noise (with no noise:
    /// a residual norm larger by this ratio), and comes back only on the same
    /// margin the other way.
    pub ratio: f64,
    /// Region edges, Hz, ascending; empty is one region for every line.
    pub edges_hz: Vec<f64>,
    /// The follower each region starts on (0: the first, 1: the second).
    pub start: usize,
    /// Standard deviations of the noise part of the residual difference a
    /// move must exceed (zero turns the test off).
    pub z: f64,
    /// Band of lines the pulse does not excite, where the noise is measured,
    /// Hz.
    pub noise_band_hz: (f64, f64),
}

impl Default for ChoiceOptions {
    fn default() -> Self {
        ChoiceOptions {
            ratio: 1.2,
            edges_hz: Vec::new(),
            start: 0,
            z: 3.0,
            noise_band_hz: (6000.0, 9500.0),
        }
    }
}

/// What the choice did on one pulse.
#[derive(Clone, Debug, PartialEq)]
pub struct ChoiceRecord {
    /// Level of the pulse, dB.
    pub level_db: f64,
    /// Per region, each follower's prediction residual on the pulse.
    pub residuals: Vec<[f64; 2]>,
    /// Per region, the follower chosen for the next pulse.
    pub chosen: Vec<usize>,
}

/// The shared log of a [`ChoiceModel`]'s decisions.
pub type ChoiceLog = Arc<Mutex<Vec<ChoiceRecord>>>;

/// The choice between two followers (module documentation).
pub struct ChoiceModel {
    followers: [Box<dyn ModelProvider>; 2],
    band: Vec<bool>,
    region_of_line: Vec<usize>,
    n_regions: usize,
    chosen: Vec<usize>,
    ratio: f64,
    z: f64,
    noise_lines: Vec<bool>,
    current: MatrixStack,
    report: ModelReport,
    log: ChoiceLog,
    level_model: Option<MatrixStack>,
}

impl ChoiceModel {
    /// The choice between `first` and `second` (same model shape), judged on
    /// the lines flagged in `band`, the record at `fs_hz`.
    pub fn new(
        first: Box<dyn ModelProvider>,
        second: Box<dyn ModelProvider>,
        band: Vec<bool>,
        fs_hz: f64,
        opts: ChoiceOptions,
    ) -> Result<Self, String> {
        let dims = first.current().dims();
        if second.current().dims() != dims || band.len() != dims.0 {
            return Err("the followers and the band do not agree".into());
        }
        if opts.ratio.is_nan() || opts.ratio < 1.0 || opts.start > 1 {
            return Err("the hysteresis ratio must be at least 1 and the start 0 or 1".into());
        }
        if opts.edges_hz.windows(2).any(|w| w[1] <= w[0]) {
            return Err("the region edges must ascend".into());
        }
        let lines = dims.0;
        let df = fs_hz / (2 * (lines - 1)) as f64;
        let region_of_line: Vec<usize> = (0..lines)
            .map(|k| {
                let f = k as f64 * df;
                opts.edges_hz.iter().filter(|&&e| f >= e).count()
            })
            .collect();
        let n_regions = opts.edges_hz.len() + 1;
        let chosen = vec![opts.start; n_regions];
        let noise_lines: Vec<bool> = (0..lines)
            .map(|k| {
                let f = k as f64 * df;
                f > opts.noise_band_hz.0 && f < opts.noise_band_hz.1
            })
            .collect();
        let mut out = ChoiceModel {
            z: opts.z,
            noise_lines,
            current: first.current().clone(),
            followers: [first, second],
            band,
            region_of_line,
            n_regions,
            chosen,
            ratio: opts.ratio,
            report: ModelReport::default(),
            log: Arc::new(Mutex::new(Vec::new())),
            level_model: None,
        };
        out.compose();
        Ok(out)
    }

    /// A handle on the log of decisions, one record a pulse.
    pub fn log(&self) -> ChoiceLog {
        Arc::clone(&self.log)
    }

    /// The follower now chosen in each region.
    pub fn chosen(&self) -> &[usize] {
        &self.chosen
    }

    fn compose(&mut self) {
        let mut m = self.followers[0].current().clone();
        let other = self.followers[1].current();
        for f in 0..m.lines() {
            if self.chosen[self.region_of_line[f]] == 1 {
                m.line_mut(f).copy_from_slice(other.line(f));
            }
        }
        self.current = m;
        let lm: [Option<&MatrixStack>; 2] = [
            self.followers[0].level_model(),
            self.followers[1].level_model(),
        ];
        self.level_model = match lm {
            [Some(a), Some(b)] => {
                let mut m = a.clone();
                for f in 0..m.lines() {
                    if self.chosen[self.region_of_line[f]] == 1 {
                        m.line_mut(f).copy_from_slice(b.line(f));
                    }
                }
                Some(m)
            }
            _ => None,
        };
    }
}

impl ModelProvider for ChoiceModel {
    fn current(&self) -> &MatrixStack {
        &self.current
    }

    fn observe(&mut self, obs: &Observation<'_>) -> Result<(), String> {
        self.report.prediction_residual =
            prediction_residual(&self.current, obs.drives, obs.responses, &self.band);
        let nu2 = noise_variance(obs.responses, &self.noise_lines);
        let mut residuals = Vec::with_capacity(self.n_regions);
        for reg in 0..self.n_regions {
            let mask: Vec<bool> = self
                .band
                .iter()
                .zip(&self.region_of_line)
                .map(|(&b, &r)| b && r == reg)
                .collect();
            let st = region_stats(
                self.followers[0].current(),
                self.followers[1].current(),
                obs,
                &mask,
            );
            let now = self.chosen[reg];
            let (mine, other) = if now == 0 {
                (st.e[0], st.e[1])
            } else {
                (st.e[1], st.e[0])
            };
            let noise = nu2 * st.count as f64;
            let excess_other = (other - noise).max(0.0);
            let margin = (self.ratio * self.ratio - 1.0) * excess_other;
            let sd = (2.0 * nu2 * st.gap).sqrt();
            if mine.is_finite() && other.is_finite() && mine - other > margin.max(self.z * sd) {
                self.chosen[reg] = 1 - now;
            }
            let rel = |e: f64| {
                if st.y > 0.0 {
                    (e / st.y).sqrt()
                } else {
                    f64::NAN
                }
            };
            residuals.push([rel(st.e[0]), rel(st.e[1])]);
        }
        let second_whole = prediction_residual(
            self.followers[1].current(),
            obs.drives,
            obs.responses,
            &self.band,
        );
        self.followers[0].observe(obs)?;
        self.followers[1].observe(obs)?;
        self.report.fit_residual = self.followers[0].report().fit_residual;
        self.report.fallback_residual = second_whole;
        self.report.gate_fired = self.chosen.contains(&1);
        if let Ok(mut l) = self.log.lock() {
            l.push(ChoiceRecord {
                level_db: obs.level_db,
                residuals,
                chosen: self.chosen.clone(),
            });
        }
        Ok(())
    }

    fn model_for(&mut self, next_level_db: f64) -> Result<ModelSource, String> {
        let s0 = self.followers[0].model_for(next_level_db)?;
        let s1 = self.followers[1].model_for(next_level_db)?;
        self.compose();
        let source = if self.chosen.iter().all(|&c| c == 0) {
            s0
        } else if self.chosen.iter().all(|&c| c == 1) {
            s1
        } else if s0 == s1 {
            s0
        } else {
            ModelSource::Predicted
        };
        self.report.source = source;
        Ok(source)
    }

    fn report(&self) -> ModelReport {
        self.report
    }

    fn level_model(&self) -> Option<&MatrixStack> {
        self.level_model.as_ref()
    }
}

struct RegionStats {
    /// Residual energy of each follower's model.
    e: [f64; 2],
    /// `||(H_0 - H_1) D||^2`.
    gap: f64,
    /// Response energy.
    y: f64,
    /// Number of complex values summed.
    count: usize,
}

fn region_stats(
    h0: &MatrixStack,
    h1: &MatrixStack,
    obs: &Observation<'_>,
    mask: &[bool],
) -> RegionStats {
    let (_, n_c, n_d) = h0.dims();
    let mut st = RegionStats {
        e: [0.0; 2],
        gap: 0.0,
        y: 0.0,
        count: 0,
    };
    for (f, &on) in mask.iter().enumerate() {
        if !on {
            continue;
        }
        let (l0, l1) = (h0.line(f), h1.line(f));
        for r in 0..n_c {
            let (mut a, mut b) = (crate::C64::new(0.0, 0.0), crate::C64::new(0.0, 0.0));
            for c in 0..n_d {
                a += l0[r * n_d + c] * obs.drives[c][f];
                b += l1[r * n_d + c] * obs.drives[c][f];
            }
            let y = obs.responses[r][f];
            st.e[0] += (y - a).norm_sqr();
            st.e[1] += (y - b).norm_sqr();
            st.gap += (a - b).norm_sqr();
            st.y += y.norm_sqr();
            st.count += 1;
        }
    }
    st
}

/// The noise variance per line of a response, from lines the pulse does not
/// excite: the median power there over every control, divided by `ln 2`.
fn noise_variance(responses: &[Vec<crate::C64>], lines: &[bool]) -> f64 {
    let mut p: Vec<f64> = Vec::new();
    for row in responses {
        for (v, &on) in row.iter().zip(lines) {
            if on {
                p.push(v.norm_sqr());
            }
        }
    }
    if p.is_empty() {
        return 0.0;
    }
    p.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = p.len();
    let med = if n % 2 == 1 {
        p[n / 2]
    } else {
        0.5 * (p[n / 2 - 1] + p[n / 2])
    };
    med / std::f64::consts::LN_2
}

/// The relative difference `||a - b|| / ||b||` over flagged lines, for
/// diagnostics.
pub fn model_distance(a: &MatrixStack, b: &MatrixStack, band: &[bool]) -> f64 {
    let (mut num, mut den) = (0.0, 0.0);
    for (f, &on) in band.iter().enumerate() {
        if !on {
            continue;
        }
        for (x, y) in a.line(f).iter().zip(b.line(f)) {
            num += (x - y).norm_sqr();
            den += y.norm_sqr();
        }
    }
    if den > 0.0 {
        (num / den).sqrt()
    } else {
        f64::NAN
    }
}
