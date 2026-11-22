//! One drive budget per shaker, shared by the random part and every tone
//! (design D8, theory 4.4).
//!
//! Drive `j` as played is `lambda [x_j + sum_k rho_k Re{D_k,j exp(j phi_k)}]`;
//! tones whose frequencies are not rationally related come arbitrarily close
//! to the sum of their amplitudes, so the budget is linear per drive (4.5):
//! `sum_k rho_k |D_k,j| <= A_j`, `A_j = V_j / lambda - P_j`, with `P_j` the
//! random part's held peak at full-level scale. When it binds the ceiling
//! rule decides, by id: `common_scale` (S2, default; every tone's vector by
//! one factor) or `in_turn` (S1; each tone against what the others play, in
//! bank order: the single-output behaviour). Both are given the tones' drives
//! in force as well as their asks; `in_turn` judges each tone against the
//! other tones' drives in force (or, for a tone already decided this tick,
//! the larger of that and its decided drive), never against their asks.
//!
//! [`headroom`] and the rules' [`CeilingRule::apply`] do not allocate.
//! [`forecast`] runs at setup along the schedule and names the first time
//! and drive that cannot fit.

use crate::config::{MimoMixedConfig, MimoMixedConfigError, MimoPartBody};
use crate::mat::{model_at, Z_REGULARIZATION};
use hertzforge_mixed::{LawPick, PartClock};
use hertzforge_sdm::{MatrixStack, C64};

/// Per-drive headroom left to the tones at full-level scale:
/// `A_j = V_j / lambda - P_j`, at least zero. `level` is the amplitude
/// fraction in force (level times ramp; at or below zero gives infinite
/// room), `ceiling_v` the drive's peak ceiling, `random_peak_v` the random
/// part's held peak at full level.
pub fn headroom(level: f64, ceiling_v: &[f64], random_peak_v: &[f64], out: &mut [f64]) {
    for ((o, &v), &p) in out.iter_mut().zip(ceiling_v).zip(random_peak_v) {
        *o = if level > 0.0 {
            (v / level - p).max(0.0)
        } else {
            f64::INFINITY
        };
    }
}

/// A ceiling rule (design D8): given every tone's requested drive vector
/// (`tones` vectors of `drives` values, tone-major, peak volts at full
/// level, already times each tone's part level), every tone's drive
/// magnitudes in force (`in_force`, the same layout, as played: part level
/// times the drive the bank holds) and each drive's headroom, write a scale
/// per tone in `[0, 1]` and whether each tone is limiting. After the rule,
/// `sum_k scale_k |D_k,j| <= A_j` on every drive.
pub trait CeilingRule: Send {
    /// The rule's id.
    fn id(&self) -> &'static str;
    /// Apply the rule. Does not allocate.
    fn apply(
        &mut self,
        drives: &[C64],
        in_force: &[f64],
        tones: usize,
        headroom: &[f64],
        scales: &mut [f64],
        limiting: &mut [bool],
    );
}

/// Below this a scale is read as 1 (no tone is marked limiting for
/// rounding).
pub const SCALE_EPS: f64 = 1e-12;

/// `common_scale` (S2): every tone's vector by
/// `s = min(1, min_j A_j / sum_k |D_k,j|)`; every tone that uses a drive is
/// limiting when `s < 1`.
#[derive(Debug, Clone, Copy, Default)]
pub struct CommonScale;

impl CommonScale {
    /// The rule's id.
    pub const ID: &'static str = "common_scale";
}

impl CeilingRule for CommonScale {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn apply(
        &mut self,
        drives: &[C64],
        _in_force: &[f64],
        tones: usize,
        headroom: &[f64],
        scales: &mut [f64],
        limiting: &mut [bool],
    ) {
        let m = headroom.len();
        let mut s = 1.0f64;
        for (j, &a) in headroom.iter().enumerate() {
            let sum: f64 = (0..tones).map(|k| drives[k * m + j].norm()).sum();
            if sum > 0.0 {
                s = s.min(a / sum);
            }
        }
        let s = s.clamp(0.0, 1.0);
        let binds = s < 1.0 - SCALE_EPS;
        for k in 0..tones {
            scales[k] = s;
            let uses = (0..m).any(|j| drives[k * m + j].norm() > 0.0);
            limiting[k] = binds && uses;
        }
    }
}

/// `in_turn` (S1): in bank order, tone `k`'s vector by
/// `s_k = min(1, min_j (A_j - sum_{l != k} P_l,j) / |D_k,j|)`, `P_l,j` the
/// other tone's drive in force on drive `j`; for a tone `l` before `k`,
/// already decided this tick, the larger of its drive in force and its
/// decided drive `s_l |D_l,j|`. So a tone is judged against what the others
/// play, never against what they ask, and the last tone on each drive sees
/// every decided drive before it: `sum_k s_k |D_k,j| <= A_j` holds after the
/// pass. The outcome depends on the order, which is the rule's known
/// weakness.
#[derive(Debug, Clone, Copy, Default)]
pub struct InTurn;

impl InTurn {
    /// The rule's id.
    pub const ID: &'static str = "in_turn";
}

impl CeilingRule for InTurn {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn apply(
        &mut self,
        drives: &[C64],
        in_force: &[f64],
        tones: usize,
        headroom: &[f64],
        scales: &mut [f64],
        limiting: &mut [bool],
    ) {
        let m = headroom.len();
        for k in 0..tones {
            let mut s = 1.0f64;
            for (j, &a) in headroom.iter().enumerate() {
                let mine = drives[k * m + j].norm();
                if mine <= 0.0 {
                    continue;
                }
                let others: f64 = (0..tones)
                    .filter(|&l| l != k)
                    .map(|l| {
                        let now = in_force.get(l * m + j).copied().unwrap_or(0.0);
                        if l < k {
                            now.max(scales[l] * drives[l * m + j].norm())
                        } else {
                            now
                        }
                    })
                    .sum();
                s = s.min((a - others) / mine);
            }
            scales[k] = s.clamp(0.0, 1.0);
            limiting[k] = scales[k] < 1.0 - SCALE_EPS;
        }
    }
}

/// Build a ceiling rule by id. An unknown id is refused
/// `unknown_ceiling_rule`; neither rule takes a parameter.
pub fn ceiling_rule(pick: &LawPick) -> Result<Box<dyn CeilingRule>, MimoMixedConfigError> {
    let rule: Box<dyn CeilingRule> = match pick.id.as_str() {
        CommonScale::ID => Box::new(CommonScale),
        InTurn::ID => Box::new(InTurn),
        _ => {
            return Err(MimoMixedConfigError::UnknownCeilingRule {
                id: pick.id.clone(),
            })
        }
    };
    if let Some(k) = pick.params.keys().next() {
        return Err(MimoMixedConfigError::Parts(
            hertzforge_mixed::ConfigError::BadConfig {
                field: "law.ceiling_rule",
                why: format!("{} takes no parameter ({k})", pick.id),
            },
        ));
    }
    Ok(rule)
}

/// The tone drive the model asks for a reference `r` at the controls:
/// `D = Z(f) r`, `Z` the regularized inverse of the model at `f_hz`.
/// `None` when the inverse fails or the drive is not finite.
pub fn model_drive(model: &MatrixStack, df_hz: f64, f_hz: f64, r: &[C64]) -> Option<Vec<C64>> {
    let z = model_at(model, df_hz, f_hz).pinv(Z_REGULARIZATION).ok()?;
    let d = z.mul_vec(r);
    d.iter()
        .all(|v| v.re.is_finite() && v.im.is_finite())
        .then_some(d)
}

/// The worst moment of a forecast.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BudgetForecast {
    /// Test time, seconds.
    pub time_s: f64,
    /// Drive index.
    pub drive: usize,
    /// The peak needed there, volts: `lambda (P_j + sum_k |D_k,j|)`.
    pub need_v: f64,
    /// The ceiling, volts.
    pub ceiling_v: f64,
}

impl BudgetForecast {
    /// Need over ceiling.
    pub fn ratio(&self) -> f64 {
        self.need_v / self.ceiling_v
    }
}

/// Forecast the budget along the schedule at setup (theory 4.4 remark 5):
/// every `step_s` of test time up to `horizon_s` (or until every part's
/// schedule ends), each tone that plays asks `D_k = Z(f_k) R_k(f_k)` (its
/// stated amplitudes, its stated phases or zero), and drive `j` needs
/// `lambda (P_j + sum_k |D_k,j|)` against its peak ceiling, with `lambda`
/// the highest level of the schedule and `P_j` the random part's peak at
/// full level (`random_peak_v`, from [`crate::predict`]). Returns the worst
/// moment, or `budget_cannot_fit` naming the first moment and drive whose
/// need is over its ceiling.
pub fn forecast(
    cfg: &MimoMixedConfig,
    model: &MatrixStack,
    random_peak_v: &[f64],
    step_s: f64,
    horizon_s: f64,
) -> Result<BudgetForecast, MimoMixedConfigError> {
    let n = cfg.n();
    let m = cfg.m();
    let df = cfg.df_hz();
    let proj = cfg.projection(0);
    let mut clock = PartClock::new(&proj)?;
    let lambda = max_level(cfg);
    let tones: Vec<(usize, &crate::config::MimoTonePart)> =
        cfg.tones().map(|(i, _, t)| (i, t)).collect();
    let mut worst = BudgetForecast {
        time_s: 0.0,
        drive: 0,
        need_v: 0.0,
        ceiling_v: cfg.drives.first().map_or(1.0, |d| d.peak_v),
    };
    let mut t = 0.0;
    let mut first = true;
    let mut sums = vec![0.0; m];
    loop {
        let now = if first {
            first = false;
            clock.tick(0.0).to_vec()
        } else {
            clock.tick(step_s).to_vec()
        };
        sums.iter_mut().for_each(|s| *s = 0.0);
        for &(i, tone) in &tones {
            let p = now[i];
            let Some(f) = p.freq_hz.filter(|_| p.on && !p.done) else {
                continue;
            };
            let Some(r) = tone.reference_at(n, f) else {
                continue;
            };
            let Some(d) = model_drive(model, df, f, &r) else {
                return Err(MimoMixedConfigError::ToneModelNotFinite {
                    part: cfg.parts[i].id.clone(),
                    f_hz: f,
                });
            };
            for (s, v) in sums.iter_mut().zip(&d) {
                *s += v.norm();
            }
        }
        for (j, drive) in cfg.drives.iter().enumerate() {
            let need = lambda * (random_peak_v.get(j).copied().unwrap_or(0.0) + sums[j]);
            let f = BudgetForecast {
                time_s: t,
                drive: j,
                need_v: need,
                ceiling_v: drive.peak_v,
            };
            if need.is_nan() || need > drive.peak_v {
                return Err(MimoMixedConfigError::BudgetCannotFit {
                    time_s: t,
                    drive: j,
                    need_v: need,
                    ceiling_v: drive.peak_v,
                });
            }
            if f.ratio() > worst.ratio() {
                worst = f;
            }
        }
        if t >= horizon_s || clock.all_parts_done() {
            break;
        }
        t += step_s;
    }
    Ok(worst)
}

/// The highest level of the schedule as an amplitude fraction of full
/// level (at least the start level).
pub fn max_level(cfg: &MimoMixedConfig) -> f64 {
    let s = &cfg.schedule;
    let db = s.table_db.iter().copied().fold(s.start_db, f64::max);
    10f64.powf(db / 20.0)
}

/// Whether a part is a tone (for callers that index parts).
pub fn is_tone(cfg: &MimoMixedConfig, part: usize) -> bool {
    matches!(
        cfg.parts.get(part).map(|p| &p.body),
        Some(MimoPartBody::Tone(_))
    )
}
