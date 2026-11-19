//! `steepest_descent`: the published steepest-descent correction,
//! `D+ = D_f + (g / s_max^2) H^H (R - C)`, with `H` the identification at the
//! tone and `s_max` its largest singular value. The first drive is the
//! regularized inverse of the identification times the reference. The gain is
//! the loop-speed rule's by default; the published fixed gain is a parameter.
//! Slow in the ill-conditioned directions, which is why it trails `matrix`.

use crate::law::{aimed_reference, worst_error_db, Hold, Inputs, Outputs, SineLaw};
use crate::linalg::{mat_vec, reg_pinv, singular_values};
use crate::setup::{GroupDelay, Setup};
use crate::speed::{gain, GainRule};
use crate::{TestTypes, C64};

/// The diagnostics `steepest_descent` declares, in order.
pub const STEEPEST_VALUES: [&str; 2] = ["gain", "error_db"];

/// The test types `steepest_descent` accepts: a dwell only. On the research
/// testbed's linear 4 by 4 it completed 5 of 5 dwells at the lightest mode, 2
/// of 5 stepped sines (12 steps an octave, 30 to 120 Hz) and 0 of 20 sweeps at
/// one octave a minute.
pub const STEEPEST_TEST_TYPES: TestTypes = TestTypes {
    sweep: false,
    dwell: true,
    stepped: false,
};

/// Parameters of `steepest_descent`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SteepestParams {
    /// The gain rule.
    pub gain_rule: GainRule,
    /// Regularization of the first drive's inverse.
    pub eps: f64,
}

impl Default for SteepestParams {
    fn default() -> Self {
        SteepestParams {
            gain_rule: GainRule::default(),
            eps: 1e-3,
        }
    }
}

/// The `steepest_descent` law.
#[derive(Clone, Debug)]
pub struct SteepestLaw {
    params: SteepestParams,
    setup: Setup,
    delay: GroupDelay,
}

impl SteepestLaw {
    /// Build the law.
    pub fn new(setup: &Setup, params: SteepestParams) -> Result<SteepestLaw, String> {
        setup.check()?;
        Ok(SteepestLaw {
            params,
            delay: GroupDelay::of_with(&setup.identification, params.gain_rule.delay_estimate()),
            setup: setup.clone(),
        })
    }
}

impl SineLaw for SteepestLaw {
    fn engage(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        let (n, m) = (self.setup.controls, self.setup.drives);
        let mut h = vec![C64::new(0.0, 0.0); n * m];
        self.setup.response_at(x.f_hz, &mut h);
        let (z, _) = reg_pinv(&h, n, m, self.params.eps)?;
        mat_vec(&z, m, n, &aimed_reference(x), &mut out.drive);
        out.values.clear();
        out.values.extend_from_slice(&[0.0, 0.0]);
        Ok(())
    }

    fn tick(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        let (n, m) = (self.setup.controls, self.setup.drives);
        let mut h = vec![C64::new(0.0, 0.0); n * m];
        self.setup.response_at(x.f_hz, &mut h);
        let smax = singular_values(&h, n, m)?[0];
        let g = if x.hold == Hold::Engage {
            0.0
        } else {
            gain(
                self.params.gain_rule,
                &self.setup.tracking,
                self.setup.loop_delay,
                &self.delay,
                self.setup.tick_s,
                x.f_hz,
            )
            .g
        };
        let k = g / (smax * smax);
        let r = aimed_reference(x);
        for (j, d) in out.drive.iter_mut().enumerate() {
            let s = (0..n).fold(C64::new(0.0, 0.0), |s, i| {
                s + h[i * m + j].conj() * (r[i] - x.controls[i])
            });
            *d = x.drive_in_force[j] + s * k;
        }
        out.values.clear();
        out.values
            .extend_from_slice(&[g, worst_error_db(x.controls, x.reference)]);
        Ok(())
    }

    fn value_names(&self) -> &'static [&'static str] {
        &STEEPEST_VALUES
    }
}
