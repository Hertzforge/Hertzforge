//! `matrix`: the linear law, and the default.
//!
//! Each update, with `Z(f)` the regularized inverse of the model (least
//! squares when there are more controls than drives, least norm when there are
//! more drives), `g` the loop-speed rule's gain, `f'` the frequency of the next
//! interval (`f + sweep rate x tick`) and `w` the weight on the carry:
//!
//! ```text
//! D+ = D_f + w (Z(f') - Z(f)) H(f) D_f + Z(f') (R(f') - R(f)) + g Z(f) (R - C)
//! ```
//!
//! - **Correction:** `g Z(f) (R - C)`, the published inverse correction at the
//!   loop-speed rule's gain.
//! - **Carry:** the drive moved by the model's own change between this
//!   frequency and the next, and by the reference's change through the model.
//!   A model error common to `Z` and `H` cancels. At a dwell or a step it is
//!   zero and the law is the correction alone. With `w = 0` it is the
//!   correction alone everywhere.
//! - **Rebase:** `D_f` is the drive in force as played, after every guard, so
//!   nothing the controller cut is integrated again (no wind-up).
//! - **Reset:** with more drives than controls the drive's part the controls
//!   cannot see (outside the row space of `H(f')`) is removed every update; the
//!   correction never adds to it, but the drive in force keeps it as the row
//!   space turns with frequency.
//! - **Amplitude-only reference:** each control's phase is taken from its
//!   measurement, `R_c |C_c| / C_c` scaled to `|R_c|`, so only amplitudes are
//!   corrected.
//!
//! The model is the identification's modes exact plus its smoothed remainder
//! (see [`crate::modal`]); the loop-speed rule reads the identification
//! itself.
//!
//! The law's stated limit: a resonance that has moved less than a quarter of
//! its half-power bandwidth from where it was identified is controlled as on a
//! linear structure; between a quarter and one bandwidth errors over 3 dB are
//! expected there; past about one the loop can be lost.

use crate::law::{Hold, Inputs, Outputs, SineLaw};
use crate::linalg::{mat_vec, norm, reg_pinv, row_space_part};
use crate::modal::{Model, ModelChoice};
use crate::setup::{GroupDelay, Setup, Shape, TrackingRule};
use crate::speed::{gain, GainRule};
use crate::{TestTypes, C64};

const ZERO: C64 = C64::new(0.0, 0.0);

/// The diagnostics `matrix` declares, in order.
pub const MATRIX_VALUES: [&str; 8] = [
    "gain",
    "group_delay_s",
    "condition",
    "carried_share",
    "rotation_deg",
    "unseen_share",
    "modes_kept",
    "candidates_left_out",
];

/// The test types `matrix` accepts: all three. On the research testbed's
/// linear 4 by 4 it completed 5 of 5 dwells at the lightest mode and 5 of 5
/// stepped sines (12 steps an octave, 30 to 120 Hz), and its carried loop
/// through the modal-smoothed model 16 of 20 sweeps at one octave a minute.
pub const MATRIX_TEST_TYPES: TestTypes = TestTypes {
    sweep: true,
    dwell: true,
    stepped: true,
};

/// Parameters of `matrix`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatrixParams {
    /// Weight on the carry, 0 to 1.
    pub carry: f64,
    /// The gain rule.
    pub gain_rule: GainRule,
    /// Regularization of the inverse, relative to the largest singular value.
    pub eps: f64,
    /// The model the carry and the correction read.
    pub model: ModelChoice,
    /// Integrate on the drive in force (`true`) or on the drive last asked for.
    /// Off only to show what the rebase prevents.
    pub rebase: bool,
    /// Remove the drive the controls cannot see on a setup with spare drives.
    /// Off only to show what the reset prevents.
    pub reset_unseen: bool,
}

impl Default for MatrixParams {
    fn default() -> Self {
        MatrixParams {
            carry: 0.67,
            gain_rule: GainRule::default(),
            eps: 1e-3,
            model: ModelChoice::default(),
            rebase: true,
            reset_unseen: true,
        }
    }
}

/// The `matrix` law's state.
#[derive(Clone, Debug)]
pub struct MatrixLaw {
    params: MatrixParams,
    n: usize,
    m: usize,
    shape: Shape,
    tick_s: f64,
    tracking: TrackingRule,
    grid_lo: f64,
    grid_hi: f64,
    phase_specified: bool,
    setup: Setup,
    model: Model,
    delay: GroupDelay,
    asked: Vec<C64>,
}

impl MatrixLaw {
    /// Build the law: the model is fitted here, once.
    pub fn new(setup: &Setup, params: MatrixParams) -> Result<MatrixLaw, String> {
        setup.check()?;
        let model = Model::build(
            &setup.identification,
            (setup.band_lo_hz, setup.band_hi_hz),
            params.model,
        )?;
        Self::with_model(setup, params, model)
    }

    /// Build the law on a model already made (`params.model` is then not read).
    pub fn with_model(
        setup: &Setup,
        params: MatrixParams,
        model: Model,
    ) -> Result<MatrixLaw, String> {
        setup.check()?;
        if !(0.0..=1.0).contains(&params.carry) {
            return Err("the carry weight must be 0 to 1".into());
        }
        let id = &setup.identification;
        let g = &id.grid_hz;
        Ok(MatrixLaw {
            params,
            n: setup.controls,
            m: setup.drives,
            shape: setup.shape,
            tick_s: setup.tick_s,
            tracking: setup.tracking,
            grid_lo: g[0],
            grid_hi: g[g.len() - 1],
            phase_specified: setup.phase_specified,
            delay: GroupDelay::of(id),
            setup: setup.clone(),
            model,
            asked: vec![ZERO; setup.drives],
        })
    }

    /// The model the law reads (its modes and its smoothed grid).
    pub fn model(&self) -> &Model {
        &self.model
    }

    fn kept(&self) -> f64 {
        self.model.modes.modes() as f64
    }

    fn left_out(&self) -> f64 {
        self.model.fit.candidates_left_out as f64
    }

    fn model_at(&self, f: f64) -> Vec<C64> {
        let mut h = vec![ZERO; self.n * self.m];
        self.model.at(f, &mut h);
        h
    }

    /// The reference the law controls to: as given, or with each control's
    /// phase from its measurement when the reference is amplitude only.
    fn effective(&self, r: &[C64], c: &[C64]) -> Vec<C64> {
        if self.phase_specified {
            return r.to_vec();
        }
        r.iter()
            .zip(c)
            .map(|(r, c)| {
                let a = c.norm();
                let ph = if a > 1e-300 {
                    c / a
                } else {
                    C64::new(1.0, 0.0)
                };
                ph * r.norm()
            })
            .collect()
    }

    fn reset(&self, d: Vec<C64>, f: f64) -> Result<(Vec<C64>, f64), String> {
        if self.shape != Shape::Wide || !self.params.reset_unseen {
            return Ok((d, 0.0));
        }
        let h = self.model_at(f);
        let kept = row_space_part(&h, self.n, self.m, &d)?;
        let removed: Vec<C64> = d.iter().zip(&kept).map(|(a, b)| a - b).collect();
        let share = norm(&removed) / norm(&d).max(1e-300);
        Ok((kept, share))
    }

    /// The first drive: the inverse at the start frequency and level.
    fn engage_impl(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        let h = self.model_at(x.f_hz);
        let (z, cond) = reg_pinv(&h, self.n, self.m, self.params.eps)?;
        let r = self.effective(x.reference, x.controls);
        let mut d = vec![ZERO; self.m];
        mat_vec(&z, self.m, self.n, &r, &mut d);
        let (d, unseen) = self.reset(d, x.f_hz)?;
        self.asked.copy_from_slice(&d);
        out.drive.copy_from_slice(&d);
        out.values.clear();
        out.values
            .extend_from_slice(&[0.0, 0.0, cond, 0.0, 0.0, 0.0, unseen]);
        Ok(())
    }

    /// Every update after engage.
    fn tick_impl(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        let (n, m) = (self.n, self.m);
        let gn = gain(
            self.params.gain_rule,
            &self.tracking,
            &self.delay,
            self.tick_s,
            x.f_hz,
        );
        let g = if x.hold == Hold::Engage { 0.0 } else { gn.g };
        let rk = self.effective(x.reference, x.controls);
        let e: Vec<C64> = rk.iter().zip(x.controls).map(|(r, c)| r - c).collect();
        let f_next = (x.f_hz + x.sweep_hz_s * self.tick_s).clamp(self.grid_lo, self.grid_hi);
        let mut rn = vec![ZERO; n];
        self.setup.reference_at(f_next, &mut rn);
        rn.iter_mut().for_each(|v| *v *= x.level);
        let rn = self.effective(&rn, x.controls);
        let hk = self.model_at(x.f_hz);
        let (zk, cond) = reg_pinv(&hk, n, m, self.params.eps)?;
        let (zn, _) = reg_pinv(&self.model_at(f_next), n, m, self.params.eps)?;
        let d0: Vec<C64> = if self.params.rebase {
            x.drive_in_force.to_vec()
        } else {
            self.asked.clone()
        };
        // Carried: w (Zn - Zk) Hk D0 + Zn (Rn - Rk).
        let mut c0 = vec![ZERO; n];
        mat_vec(&hk, n, m, &d0, &mut c0);
        let mut a = vec![ZERO; m];
        let mut b = vec![ZERO; m];
        mat_vec(&zn, m, n, &c0, &mut a);
        mat_vec(&zk, m, n, &c0, &mut b);
        let dr: Vec<C64> = rn.iter().zip(&rk).map(|(p, q)| p - q).collect();
        let mut c = vec![ZERO; m];
        mat_vec(&zn, m, n, &dr, &mut c);
        let w = self.params.carry;
        let carried: Vec<C64> = (0..m).map(|i| (a[i] - b[i]) * w + c[i]).collect();
        let mut corr = vec![ZERO; m];
        mat_vec(&zk, m, n, &e, &mut corr);
        corr.iter_mut().for_each(|v| *v *= g);
        let d: Vec<C64> = (0..m).map(|i| d0[i] + carried[i] + corr[i]).collect();
        let (d, unseen) = self.reset(d, f_next)?;
        let (nc, nk) = (norm(&carried), norm(&corr));
        let share = if nc + nk > 0.0 { nc / (nc + nk) } else { 0.0 };
        let rotation = d
            .iter()
            .zip(x.drive_in_force)
            .filter(|(a, b)| a.norm() > 0.0 && b.norm() > 0.0)
            .map(|(a, b)| (a * b.conj()).arg().abs().to_degrees())
            .fold(0.0, f64::max);
        self.asked.copy_from_slice(&d);
        out.drive.copy_from_slice(&d);
        out.fault = false;
        out.values.clear();
        out.values.extend_from_slice(&[
            g,
            gn.structure_delay_s,
            cond,
            share,
            rotation,
            unseen,
            self.kept(),
            self.left_out(),
        ]);
        Ok(())
    }
}

impl SineLaw for MatrixLaw {
    fn engage(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        self.engage_impl(x, out)
    }
    fn tick(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        self.tick_impl(x, out)
    }
    fn value_names(&self) -> &'static [&'static str] {
        &MATRIX_VALUES
    }
}
