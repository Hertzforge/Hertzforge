//! `inverse`: waveform control through the regularized inverse of the model.
//!
//! Seed: `D0 = s G R`, with `G` the regularized inverse of the model per line
//! ([`crate::model::regularized_inverse`]) and `s` the level's scale.
//!
//! After each pulse, with every spectrum normalized to full level (drive and
//! response as played and measured, divided by the pulse's scale):
//!
//! `D+ = D + g(f) G (R - Y)`
//!
//! and the next drive is `s+ D+`. This is the design's
//! `D+ = (s+/s) D + g G (s+ R - (s+/s) Y)` written at full level.
//!
//! The watchdog keeps a gain `g(f)` per line. It smooths the error energy
//! `sum_c abs(E_c(f))^2` over a few lines; on a line whose smoothed energy grew
//! by more than `watchdog_grow` since the last pulse it halves the gain,
//! elsewhere it restores the gain by `watchdog_restore` a pulse, never above
//! the set gain (`Derivations.md` D5).
//!
//! Options: `refresh` moves the model along the pulse's drive before the
//! correction ([`crate::refresh`]); `align` removes each control's best-fit
//! delay from its response before the error is formed, so the loop never
//! fights a pure delay.

use crate::delay::estimate_delay;
use crate::model::regularized_inverse;
use crate::provider::{ModelProvider, Observation};
use crate::refresh::refresh_along_drive;
use crate::robust::weighted_inverse;
use crate::spectrum::{apply, omegas, relative_error, rms_all, rows_irfft, rows_rfft, smooth};
use crate::{
    check_inputs, check_setup, drive_mask, level_scale, LawSteps, MatrixStack, PulseInputs,
    PulseOutputs, Setup, C64,
};

/// Parameters of `inverse`.
#[derive(Clone, Debug, PartialEq)]
pub struct InverseParams {
    /// Tikhonov regularization relative to each line's largest singular value.
    pub lambda_rel: f64,
    /// The set correction gain, 0 to 1.
    pub gain: f64,
    /// Whether the per-line watchdog runs.
    pub watchdog: bool,
    /// Lines the error energy is smoothed over (odd).
    pub watchdog_lines: usize,
    /// Growth of the smoothed error energy that cuts a line's gain (1.05: 5%).
    pub watchdog_grow: f64,
    /// Factor a cut applies to the gain.
    pub watchdog_cut: f64,
    /// Factor the gain is restored by on a pulse with no cut.
    pub watchdog_restore: f64,
    /// Whether the model is refreshed along the drive after each pulse.
    pub refresh: bool,
    /// The refresh blend, `(0, 1]`.
    pub refresh_mu: f64,
    /// Lines whose drive energy is below this fraction of the largest line's
    /// are not refreshed.
    pub refresh_floor_rel: f64,
    /// Whether each control's best-fit delay is removed before the error.
    pub align: bool,
    /// The largest delay `align` removes, seconds.
    pub align_max_s: f64,
}

impl Default for InverseParams {
    fn default() -> Self {
        InverseParams {
            lambda_rel: 1.0e-3,
            gain: 1.0,
            watchdog: true,
            watchdog_lines: 9,
            watchdog_grow: 1.05,
            watchdog_cut: 0.5,
            watchdog_restore: 1.2,
            refresh: false,
            refresh_mu: 0.5,
            refresh_floor_rel: 1.0e-6,
            align: false,
            align_max_s: 2.0e-3,
        }
    }
}

/// What [`InverseLaw::with_options`] adds to the law.
#[derive(Default)]
pub struct InverseExtras {
    /// The model provider; `None` keeps the model as `params` says.
    pub provider: Option<Box<dyn ModelProvider>>,
    /// When set, the first pulse of each new level uses the
    /// uncertainty-weighted inverse with these mode-frequency drops around the
    /// provider's model.
    pub robust_first: Option<Vec<f64>>,
    /// At a level step, correct the error the next level is predicted to have,
    /// `R - Y - (Hh_next - Hh_k) D`, instead of the error measured at the level
    /// just played (`Derivations.md` D17). Needs a provider that keeps the
    /// model of the level played apart from the prediction.
    pub predicted_step: bool,
}

/// The `inverse` law's state.
pub struct InverseLaw {
    setup: Setup,
    params: InverseParams,
    mask: Vec<bool>,
    omegas: Vec<f64>,
    model: MatrixStack,
    inverse: MatrixStack,
    reference: Vec<Vec<C64>>,
    gains: Vec<f64>,
    previous_energy: Option<Vec<f64>>,
    extras: Option<InverseExtras>,
}

impl InverseLaw {
    /// The law for a setup.
    pub fn new(setup: Setup, params: InverseParams) -> Result<Self, String> {
        let lines = check_setup(&setup)?;
        if !(params.gain > 0.0 && params.gain <= 1.0) {
            return Err(format!("the gain {} must be in (0, 1]", params.gain));
        }
        if params.watchdog
            && !(params.watchdog_cut > 0.0
                && params.watchdog_cut < 1.0
                && params.watchdog_restore >= 1.0
                && params.watchdog_grow >= 1.0)
        {
            return Err(
                "the watchdog needs a cut below one and growth and restore of one or more".into(),
            );
        }
        let mask = drive_mask(&setup);
        let model = setup.plant.frf.clone();
        let inverse = regularized_inverse(&model, &mask, params.lambda_rel)?;
        let reference = rows_rfft(&setup.reference, 1.0)?;
        Ok(InverseLaw {
            omegas: omegas(setup.fs_hz, setup.record_len),
            gains: vec![params.gain; lines],
            previous_energy: None,
            extras: None,
            setup,
            params,
            mask,
            model,
            inverse,
            reference,
        })
    }

    /// The law with a model provider (and the weighted first pulse of a
    /// level). With `InverseExtras::default()` it is the law of
    /// [`InverseLaw::new`].
    pub fn with_options(
        setup: Setup,
        params: InverseParams,
        extras: InverseExtras,
    ) -> Result<Self, String> {
        if extras.provider.is_some() && params.refresh {
            return Err("a model provider replaces the refresh; turn the refresh off".into());
        }
        if extras.robust_first.is_some() && extras.provider.is_none() {
            return Err("the weighted first pulse needs a model provider".into());
        }
        let mut law = InverseLaw::new(setup, params)?;
        if let Some(p) = &extras.provider {
            if p.current().dims() != law.model.dims() {
                return Err("the provider's model does not match the setup".into());
            }
            law.model = p.current().clone();
            law.inverse = regularized_inverse(&law.model, &law.mask, law.params.lambda_rel)?;
        }
        law.extras = Some(extras);
        Ok(law)
    }

    /// The gain per line now in force.
    pub fn line_gains(&self) -> &[f64] {
        &self.gains
    }

    /// The model the law holds now.
    pub fn model(&self) -> &MatrixStack {
        &self.model
    }
}

impl LawSteps for InverseLaw {
    fn seed(&mut self, level_db: f64) -> Result<PulseOutputs, String> {
        let d = apply(&self.inverse, &self.reference);
        let drives = rows_irfft(&d, self.setup.record_len, level_scale(level_db))?;
        let rms = rms_all(&drives) / level_scale(level_db);
        Ok(PulseOutputs {
            drives,
            delays_s: None,
            values: vec![("drive_rms_full_level", rms)],
        })
    }

    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, String> {
        check_inputs(&self.setup, inputs)?;
        let n = self.setup.record_len;
        let s = level_scale(inputs.level_db);
        let s_next = level_scale(inputs.next_level_db);
        let d = rows_rfft(&inputs.drives, s)?;
        let y = rows_rfft(&inputs.responses, s)?;
        let lines = n / 2 + 1;

        // The error, optionally after removing each control's best-fit delay.
        let mut delays = None;
        let y_err: Vec<Vec<C64>> = if self.params.align {
            let mut taus = Vec::with_capacity(y.len());
            let mut out = Vec::with_capacity(y.len());
            for (c, yc) in y.iter().enumerate() {
                let yt: Vec<f64> = inputs.responses[c].iter().map(|v| v / s).collect();
                let tau = estimate_delay(&yt, &self.setup.reference[c], self.setup.fs_hz)?
                    .clamp(-self.params.align_max_s, self.params.align_max_s);
                taus.push(tau);
                out.push(crate::delay::apply_delay(yc, &self.omegas, -tau));
            }
            delays = Some(taus);
            out
        } else {
            y.clone()
        };
        let e: Vec<Vec<C64>> = self
            .reference
            .iter()
            .zip(&y_err)
            .map(|(r, yc)| r.iter().zip(yc).map(|(a, b)| a - b).collect())
            .collect();

        // The watchdog.
        let mut cut = 0usize;
        if self.params.watchdog {
            let energy: Vec<f64> = (0..lines)
                .map(|f| e.iter().map(|ec| ec[f].norm_sqr()).sum())
                .collect();
            let sm = smooth(&energy, self.params.watchdog_lines);
            if let Some(prev) = &self.previous_energy {
                for f in 0..lines {
                    if sm[f] > self.params.watchdog_grow * prev[f] {
                        self.gains[f] *= self.params.watchdog_cut;
                        if self.mask[f] {
                            cut += 1;
                        }
                    } else {
                        self.gains[f] =
                            (self.gains[f] * self.params.watchdog_restore).min(self.params.gain);
                    }
                }
            }
            self.previous_energy = Some(sm);
        }

        // The refresh, along the drive as played.
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

        // The provider's model for the next pulse.
        let mut extra_values: Vec<(&'static str, f64)> = Vec::new();
        let mut first_inverse: Option<MatrixStack> = None;
        if let Some(ex) = self.extras.as_mut() {
            if let Some(p) = ex.provider.as_mut() {
                p.observe(&Observation {
                    level_db: inputs.level_db,
                    drives: &d,
                    responses: &y,
                })?;
                p.model_for(inputs.next_level_db)?;
                self.model = p.current().clone();
                self.inverse =
                    regularized_inverse(&self.model, &self.mask, self.params.lambda_rel)?;
                if let Some(drops) = &ex.robust_first {
                    if inputs.next_level_db != inputs.level_db {
                        if let Some(rad) = p.radius(inputs.next_level_db, drops) {
                            first_inverse = Some(weighted_inverse(
                                &self.model,
                                &self.mask,
                                self.params.lambda_rel,
                                &rad,
                            )?);
                        }
                    }
                }
                let rep = p.report();
                extra_values.push(("model_prediction_residual", rep.prediction_residual));
                extra_values.push(("model_fit_residual", rep.fit_residual));
                extra_values.push(("model_fallback_residual", rep.fallback_residual));
                extra_values.push(("model_gate_fired", f64::from(u8::from(rep.gate_fired))));
                extra_values.push(("model_source", rep.source.code()));
            }
        }

        // At a level step, the error the next level is predicted to have.
        let mut e = e;
        if let Some(ex) = self.extras.as_ref() {
            if let (true, Some(p)) = (ex.predicted_step, ex.provider.as_ref()) {
                if inputs.next_level_db != inputs.level_db {
                    if let Some(hk) = p.level_model() {
                        let shift = apply(p.current(), &d);
                        let here = apply(hk, &d);
                        for (ec, (sc, hc)) in e.iter_mut().zip(shift.iter().zip(&here)) {
                            for (f, v) in ec.iter_mut().enumerate() {
                                if self.mask[f] {
                                    *v -= sc[f] - hc[f];
                                }
                            }
                        }
                        extra_values.push(("predicted_step", 1.0));
                    }
                }
            }
        }

        // The correction.
        let step = apply(first_inverse.as_ref().unwrap_or(&self.inverse), &e);
        let next: Vec<Vec<C64>> = d
            .iter()
            .zip(&step)
            .map(|(dc, sc)| {
                dc.iter()
                    .zip(sc)
                    .zip(&self.gains)
                    .map(|((a, b), g)| a + b * *g)
                    .collect()
            })
            .collect();
        let drives = rows_irfft(&next, n, s_next)?;

        let y_t: Vec<Vec<f64>> = inputs
            .responses
            .iter()
            .map(|r| r.iter().map(|v| v / s).collect())
            .collect();
        let min_gain = self
            .gains
            .iter()
            .zip(&self.mask)
            .filter(|(_, &m)| m)
            .fold(f64::INFINITY, |a, (g, _)| a.min(*g));
        let mut values = vec![
            (
                "waveform_error",
                relative_error(&y_t, &self.setup.reference),
            ),
            ("drive_rms_full_level", rms_all(&drives) / s_next),
            (
                "least_line_gain",
                if min_gain.is_finite() { min_gain } else { 0.0 },
            ),
            ("lines_cut", cut as f64),
            ("lines_refreshed", refreshed as f64),
        ];
        values.extend(extra_values);
        Ok(PulseOutputs {
            values,
            drives,
            delays_s: delays,
        })
    }
}
