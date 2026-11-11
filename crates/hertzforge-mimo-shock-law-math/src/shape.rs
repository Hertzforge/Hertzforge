//! `shape`: SRS control by shaping each control's target, with no waveform
//! error formed anywhere.
//!
//! The law keeps a real, positive gain `C_c(f)` per control and a delay
//! `tau_c` per control. After a pulse, with the measured SRS `S_c` and the
//! target `T_c` at the SRS's natural frequencies:
//!
//! `C_c(f) <- C_c(f) * interp_f( clamp((T_c / S_c)^w) )`
//!
//! where the ratio is taken in dB, limited to `+/- max_step_db` a pulse, and
//! carried to the lines by linear interpolation in log frequency; `w` is the
//! weight for the level just played, from a table (`Derivations.md` D6). The
//! next drive is
//!
//! `D = s+ G (C o R(tau))`,   `R_c(f; tau) = R_c(f) exp(-j w tau_c)`
//!
//! with `G` the model's regularized inverse. The delays are re-estimated from
//! the measured response by cross-correlation against the shaped target and
//! relaxed: `tau_c <- tau_c + relax * d_c`, referred to control 0 and bounded.
//!
//! Options:
//! - `anchor = Measured` (requires `refresh`): the wanted response of the next
//!   pulse is the measured response with its own phase and its magnitude
//!   scaled by this pulse's ratio, and the drive moves by the model's inverse
//!   of the change: `D+ = D + G ((r - 1) o Y)` (`Derivations.md` D2, ledger
//!   MS-C1 with MS-C3).
//! - `refresh`: the model is moved along the drive after each pulse.
//! - `seed_timing = LeastEffort`: the delays before the first pulse are chosen
//!   for least unreachable energy plus drive ([`crate::delay`]).

use crate::delay::{apply_delay, estimate_delay, least_effort_delays, EffortCost, LeastEffort};
use crate::model::regularized_inverse;
use crate::refresh::refresh_along_drive;
use crate::spectrum::{apply, omegas, rms_all, rows_irfft, rows_rfft};
use crate::srs::{srs_rows, to_lines_log, weight_at, worst_error_db};
use crate::{
    check_inputs, check_setup, drive_mask, level_scale, LawSteps, MatrixStack, PulseInputs,
    PulseOutputs, Setup, C64,
};

/// What the SRS correction is applied to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// The target waveform, shaped by the accumulated gain (the default).
    Target,
    /// The measured response, keeping its phase (requires the refresh).
    Measured,
}

/// How the delays are set before the first pulse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SeedTiming {
    /// Every control's target at zero delay.
    Zero,
    /// The least-effort delays.
    LeastEffort(LeastEffort),
}

/// Parameters of `shape`.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeParams {
    /// Tikhonov regularization relative to each line's largest singular value.
    pub lambda_rel: f64,
    /// The weight by level: `(level_db, weight)` rows, sorted by level.
    pub weights: Vec<(f64, f64)>,
    /// The largest change of a control's gain in one pulse, dB.
    pub max_step_db: f64,
    /// Relaxation of the delay re-estimation; zero turns it off.
    pub delay_relax: f64,
    /// Each delay stays inside `+/- delay_bound_s` of control 0's, seconds.
    pub delay_bound_s: f64,
    /// What the correction is applied to.
    pub anchor: Anchor,
    /// Whether the model is refreshed along the drive after each pulse.
    pub refresh: bool,
    /// The refresh blend, `(0, 1]`.
    pub refresh_mu: f64,
    /// The refresh's drive-energy floor relative to the largest line's.
    pub refresh_floor_rel: f64,
    /// How the delays are set before the first pulse.
    pub seed_timing: SeedTiming,
}

/// The default weight table: 0.5 at -12 dB rising to 0.85 at -3 dB and above
/// (the published single-drive schedule, `Derivations.md` D6).
pub const DEFAULT_WEIGHTS: [(f64, f64); 5] = [
    (-12.0, 0.5),
    (-9.0, 0.72),
    (-6.0, 0.82),
    (-3.0, 0.85),
    (0.0, 0.85),
];

impl Default for ShapeParams {
    fn default() -> Self {
        ShapeParams {
            lambda_rel: 1.0e-3,
            weights: DEFAULT_WEIGHTS.to_vec(),
            max_step_db: 6.0,
            delay_relax: 0.5,
            delay_bound_s: 2.0e-3,
            anchor: Anchor::Target,
            refresh: false,
            refresh_mu: 1.0,
            refresh_floor_rel: 2.5e-3,
            seed_timing: SeedTiming::Zero,
        }
    }
}

/// The `shape` law's state.
pub struct ShapeLaw {
    setup: Setup,
    params: ShapeParams,
    mask: Vec<bool>,
    omegas: Vec<f64>,
    model: MatrixStack,
    inverse: MatrixStack,
    reference: Vec<Vec<C64>>,
    gains: Vec<Vec<f64>>,
    delays: Vec<f64>,
}

impl ShapeLaw {
    /// The law for an SRS setup.
    pub fn new(setup: Setup, params: ShapeParams) -> Result<Self, String> {
        let lines = check_setup(&setup)?;
        if setup.srs.is_none() {
            return Err("shape controls an SRS test; the setup has no SRS target".into());
        }
        if params.anchor == Anchor::Measured && !params.refresh {
            return Err("the measured anchor needs the model refresh".into());
        }
        if params.max_step_db.is_nan()
            || params.max_step_db <= 0.0
            || params.delay_bound_s.is_nan()
            || params.delay_bound_s < 0.0
        {
            return Err("the step limit must be positive and the delay bound not negative".into());
        }
        if !(0.0..=1.0).contains(&params.delay_relax) {
            return Err("the delay relaxation must be in [0, 1]".into());
        }
        let mask = drive_mask(&setup);
        let model = setup.plant.frf.clone();
        let inverse = regularized_inverse(&model, &mask, params.lambda_rel)?;
        let reference = rows_rfft(&setup.reference, 1.0)?;
        Ok(ShapeLaw {
            omegas: omegas(setup.fs_hz, setup.record_len),
            gains: vec![vec![1.0; lines]; setup.n_controls],
            delays: vec![0.0; setup.n_controls],
            setup,
            params,
            mask,
            model,
            inverse,
            reference,
        })
    }

    /// The shaped targets at the current gains and delays, full level.
    fn shaped(&self) -> Vec<Vec<C64>> {
        self.reference
            .iter()
            .zip(&self.gains)
            .zip(&self.delays)
            .map(|((r, g), &tau)| {
                apply_delay(r, &self.omegas, tau)
                    .into_iter()
                    .zip(g)
                    .map(|(v, gv)| v * *gv)
                    .collect()
            })
            .collect()
    }

    /// The delays the law builds for now, seconds.
    pub fn delays(&self) -> &[f64] {
        &self.delays
    }
}

impl LawSteps for ShapeLaw {
    fn seed(&mut self, level_db: f64) -> Result<PulseOutputs, String> {
        let mut values = Vec::new();
        if let SeedTiming::LeastEffort(le) = self.params.seed_timing {
            let cost = EffortCost::new(
                &self.model,
                &self.inverse,
                &self.mask,
                &self.reference,
                &self.omegas,
                le.lambda,
            );
            let before = cost.cost(&vec![0.0; self.delays.len()]);
            self.delays = least_effort_delays(&cost, le);
            values.push(("effort_cost_zero_delay", before));
            values.push(("effort_cost_seed", cost.cost(&self.delays)));
        }
        let d = apply(&self.inverse, &self.shaped());
        let s = level_scale(level_db);
        let drives = rows_irfft(&d, self.setup.record_len, s)?;
        values.push(("drive_rms_full_level", rms_all(&drives) / s));
        Ok(PulseOutputs {
            drives,
            delays_s: Some(self.delays.clone()),
            values,
        })
    }

    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, String> {
        check_inputs(&self.setup, inputs)?;
        let target = self.setup.srs.as_ref().expect("checked at construction");
        let n = self.setup.record_len;
        let lines = n / 2 + 1;
        let df = self.setup.fs_hz / n as f64;
        let s = level_scale(inputs.level_db);
        let s_next = level_scale(inputs.next_level_db);

        let measured = match &inputs.srs {
            Some(m) => {
                if m.len() != self.setup.n_controls
                    || m.iter().any(|r| r.len() != target.natural_hz.len())
                {
                    return Err("the measured SRS must have one row per control".into());
                }
                m.iter()
                    .map(|r| r.iter().map(|v| v / s).collect())
                    .collect()
            }
            None => srs_rows(
                &inputs.responses,
                self.setup.fs_hz,
                &target.natural_hz,
                target.q,
                s,
            ),
        };
        let w = weight_at(&self.params.weights, inputs.level_db);
        let lim = self.params.max_step_db;
        // This pulse's ratio per control, on the lines.
        let ratios: Vec<Vec<f64>> = measured
            .iter()
            .zip(&target.points)
            .map(|(m, t)| {
                let db: Vec<f64> = m
                    .iter()
                    .zip(t)
                    .map(|(&mv, &tv)| {
                        let e = if mv > 0.0 && tv > 0.0 {
                            20.0 * (tv / mv).log10()
                        } else {
                            0.0
                        };
                        (w * e).clamp(-lim, lim)
                    })
                    .collect();
                let at = to_lines_log(&target.natural_hz, &db, lines, df);
                at.iter().map(|v| 10f64.powf(v / 20.0)).collect()
            })
            .collect();

        let d = rows_rfft(&inputs.drives, s)?;
        let y = rows_rfft(&inputs.responses, s)?;
        let mut refreshed = 0usize;
        if self.params.refresh {
            refreshed = refresh_along_drive(
                &mut self.model,
                &d,
                &y,
                self.params.refresh_mu,
                self.params.refresh_floor_rel,
            )?;
            self.inverse = regularized_inverse(&self.model, &self.mask, self.params.lambda_rel)?;
        }

        let next = match self.params.anchor {
            Anchor::Target => {
                for (g, r) in self.gains.iter_mut().zip(&ratios) {
                    for (gv, rv) in g.iter_mut().zip(r) {
                        *gv *= rv;
                    }
                }
                if self.params.delay_relax > 0.0 {
                    let shaped = self.shaped();
                    let shaped_t = rows_irfft(&shaped, n, 1.0)?;
                    for ((resp, shaped_c), tau) in inputs
                        .responses
                        .iter()
                        .zip(&shaped_t)
                        .zip(self.delays.iter_mut())
                    {
                        let yt: Vec<f64> = resp.iter().map(|v| v / s).collect();
                        let dc = estimate_delay(&yt, shaped_c, self.setup.fs_hz)?;
                        *tau += self.params.delay_relax * dc;
                    }
                    let d0 = self.delays[0];
                    let b = self.params.delay_bound_s;
                    for tau in self.delays.iter_mut() {
                        *tau = (*tau - d0).clamp(-b, b);
                    }
                }
                apply(&self.inverse, &self.shaped())
            }
            Anchor::Measured => {
                let change: Vec<Vec<C64>> = y
                    .iter()
                    .zip(&ratios)
                    .map(|(yc, r)| yc.iter().zip(r).map(|(v, rv)| v * (rv - 1.0)).collect())
                    .collect();
                let step = apply(&self.inverse, &change);
                d.iter()
                    .zip(&step)
                    .map(|(dc, sc)| dc.iter().zip(sc).map(|(a, b)| a + b).collect())
                    .collect()
            }
        };
        let drives = rows_irfft(&next, n, s_next)?;
        Ok(PulseOutputs {
            values: vec![
                (
                    "worst_srs_error_db",
                    worst_error_db(&measured, &target.points),
                ),
                ("weight", w),
                ("drive_rms_full_level", rms_all(&drives) / s_next),
                ("lines_refreshed", refreshed as f64),
            ],
            drives,
            delays_s: Some(self.delays.clone()),
        })
    }
}
