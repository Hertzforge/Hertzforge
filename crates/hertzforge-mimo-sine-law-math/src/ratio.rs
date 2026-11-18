//! `ratio`: the published amplitude-only ratio law per control on a matrix
//! feedforward. Each factor `sigma_i` carries the ratio updates along the
//! sweep,
//!
//! ```text
//! sigma_i <- sigma_i (K + 1/E_i) / (K + E_i)   if E_i <= 1
//! sigma_i <- sigma_i (K + 1) / (K + E_i)       if E_i > 1,     E_i = |C_i| / |R_i|
//! ```
//!
//! applied once every `stride` updates (by default about three group delays
//! of the tracking filter, so each ratio is read settled). Pairing
//! `Reference` (default) applies control `i`'s ratio to its reference,
//! `D = Z(f) diag(sigma) R`, which needs no collocation; pairing `Drive` is the
//! source's form, `D = diag(sigma) Z(f) R`, on square setups only. `Z` is the
//! regularized inverse of the identification. The law has no gain: its pace is
//! the stride and `K`.

use crate::law::{worst_error_db, Hold, Inputs, Outputs, SineLaw};
use crate::linalg::{mat_vec, reg_pinv};
use crate::setup::{Setup, Shape};
use crate::{TestTypes, C64};

/// The diagnostics `ratio` declares, in order.
pub const RATIO_VALUES: [&str; 3] = ["sigma_min", "sigma_max", "error_db"];

/// The test types `ratio` accepts with a pairing. With the reference pairing,
/// a dwell and a stepped sine: on the research testbed's linear 4 by 4 it
/// completed 5 of 5 of each and 3 of 20 sweeps at one octave a minute. With the
/// drive pairing, none: 1 of 5 dwells and 0 of 5 stepped sines.
pub fn ratio_test_types(pairing: Pairing) -> TestTypes {
    match pairing {
        Pairing::Reference => TestTypes {
            sweep: false,
            dwell: true,
            stepped: true,
        },
        Pairing::Drive => TestTypes {
            sweep: false,
            dwell: false,
            stepped: false,
        },
    }
}

/// Where each ratio is applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pairing {
    /// To the control's own reference (any shape).
    Reference,
    /// To drive `i`, paired with control `i` (square setups).
    Drive,
}

/// Parameters of `ratio`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RatioParams {
    /// The factor `K` of the update.
    pub k: f64,
    /// Regularization of the inverse.
    pub eps: f64,
    /// Updates between ratio updates; `None` sets it from the tracking filter.
    pub stride: Option<usize>,
    /// Where the ratio is applied.
    pub pairing: Pairing,
}

impl Default for RatioParams {
    fn default() -> Self {
        RatioParams {
            k: 1.0,
            eps: 1e-3,
            stride: None,
            pairing: Pairing::Reference,
        }
    }
}

/// The `ratio` law.
#[derive(Clone, Debug)]
pub struct RatioLaw {
    params: RatioParams,
    setup: Setup,
    sigma: Vec<f64>,
    count: usize,
}

impl RatioLaw {
    /// Build the law.
    pub fn new(setup: &Setup, params: RatioParams) -> Result<RatioLaw, String> {
        setup.check()?;
        if params.pairing == Pairing::Drive && setup.shape != Shape::Square {
            return Err("drive pairing (drive i with control i) needs a square setup".into());
        }
        let len = match params.pairing {
            Pairing::Reference => setup.controls,
            Pairing::Drive => setup.drives,
        };
        Ok(RatioLaw {
            params,
            setup: setup.clone(),
            sigma: vec![1.0; len],
            count: 0,
        })
    }

    fn drive(&self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        let (n, m) = (self.setup.controls, self.setup.drives);
        let mut h = vec![C64::new(0.0, 0.0); n * m];
        self.setup.response_at(x.f_hz, &mut h);
        let (z, _) = reg_pinv(&h, n, m, self.params.eps)?;
        match self.params.pairing {
            Pairing::Reference => {
                let r: Vec<C64> = x
                    .reference
                    .iter()
                    .zip(&self.sigma)
                    .map(|(r, s)| r * s)
                    .collect();
                mat_vec(&z, m, n, &r, &mut out.drive);
            }
            Pairing::Drive => {
                mat_vec(&z, m, n, x.reference, &mut out.drive);
                out.drive
                    .iter_mut()
                    .zip(&self.sigma)
                    .for_each(|(d, s)| *d *= s);
            }
        }
        let lo = self.sigma.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = self.sigma.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        out.values.clear();
        out.values
            .extend_from_slice(&[lo, hi, worst_error_db(x.controls, x.reference)]);
        Ok(())
    }

    fn stride(&self, f: f64) -> usize {
        if let Some(s) = self.params.stride {
            return s;
        }
        let tau = 1.0 / (2.0 * std::f64::consts::PI * self.setup.tracking.bandwidth_hz(f));
        ((3.0 * 2.0 * tau / self.setup.tick_s).ceil() as usize).max(1)
    }
}

impl SineLaw for RatioLaw {
    fn engage(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        self.drive(x, out)
    }

    fn tick(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        self.count += 1;
        if x.hold == Hold::Engage || self.count < self.stride(x.f_hz) {
            return self.drive(x, out);
        }
        self.count = 0;
        let k = self.params.k;
        for (i, s) in self.sigma.iter_mut().enumerate() {
            let eq = (x.controls[i].norm() / x.reference[i].norm().max(1e-300)).max(1e-6);
            *s *= if eq <= 1.0 {
                (k + 1.0 / eq) / (k + eq)
            } else {
                (k + 1.0) / (k + eq)
            };
        }
        self.drive(x, out)
    }

    fn value_names(&self) -> &'static [&'static str] {
        &RATIO_VALUES
    }
}
