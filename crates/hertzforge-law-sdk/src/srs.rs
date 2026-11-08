//! SRS shock laws: the `shock-srs` world.
//!
//! An SRS shock law is called between pulses, never while one plays: once
//! after system identification ([`ShockSrsLaw::seed`]) and once after each
//! fired, measured and judged pulse ([`ShockSrsLaw::iterate`]). It returns one
//! multiplicative gain per synthesis component and up to eight values its
//! manifest declares. The controller synthesizes every pulse from its table
//! of components, so a law never writes a drive record, and a pulse's zero net
//! velocity and displacement hold whatever the gains.
//!
//! Every spectrum is a shock response spectrum on [`SrsSetup::band_hz`] at
//! quality factor [`SrsSetup::q`]. The components are on their own grid,
//! [`SrsSetup::component_hz`]; [`nearest_band`] maps a component to the band
//! nearest it. The target a law sees on each call is at the scale the last
//! pulse was commanded at, so a law aims at what was asked of that pulse.
//!
//! The controller checks what a law returns before it is used: one finite gain
//! above zero per component, the declared values, a cap on how far any gain
//! may rise in one pulse; then it caps each gain by what its limit channels
//! can take and checks the drive in volts. A law needs none of those for
//! safety.
//!
//! ```ignore
//! use hertzforge_law_sdk::srs::*;
//! use hertzforge_law_sdk::{Manifest, Params, ShockSrsLaw};
//!
//! struct Hold { n: usize }
//!
//! impl ShockSrsLaw for Hold {
//!     fn describe() -> Manifest { /* ... */ }
//!     fn new(setup: SrsSetup, _params: Params) -> Result<Self, String> {
//!         Ok(Hold { n: setup.component_hz.len() })
//!     }
//!     fn seed(&mut self, _: &SrsInputs) -> Result<SrsOutputs, String> {
//!         Ok(gains_only(vec![1.0; self.n]))
//!     }
//!     fn iterate(&mut self, _: &SrsInputs) -> Result<SrsOutputs, String> {
//!         Ok(gains_only(vec![1.0; self.n]))
//!     }
//! }
//! ```

use crate::__srs_bindings::exports::hertzforge::control_law::shock_srs_law as wit_law;
use crate::{DiagnosticValue, Manifest, Param, Params};
use core::cell::RefCell;

pub use crate::__srs_bindings::hertzforge::control_law::srs_types::{
    SrsInputs, SrsOutputs, SrsSetup,
};
pub use hertzforge_shock_law_math::srs::nearest_band;

/// An SRS shock control law.
///
/// The host builds one value per run with [`Self::new`], after system
/// identification, calls [`Self::seed`] once for the starting gains, then
/// [`Self::iterate`] after each pulse. Calls never overlap.
pub trait ShockSrsLaw: Sized + 'static {
    /// The law's name, version, parameters and values. Read once, when the
    /// file is loaded; it must not depend on anything but the code.
    fn describe() -> Manifest;

    /// Build the law for a run. `params` holds every declared parameter,
    /// already checked against the manifest. An error refuses the run.
    fn new(setup: SrsSetup, params: Params) -> Result<Self, String>;

    /// The starting gains relative to the table, before any pulse has fired.
    fn seed(&mut self, inputs: &SrsInputs) -> Result<SrsOutputs, String>;

    /// The next gains, after a pulse was fired, measured and judged.
    fn iterate(&mut self, inputs: &SrsInputs) -> Result<SrsOutputs, String>;
}

/// Gains with no values.
pub fn gains_only(gains: Vec<f64>) -> SrsOutputs {
    SrsOutputs {
        gains,
        values: Vec::new(),
    }
}

/// Gains with `values` as `(key, value)` pairs.
pub fn srs_outputs(gains: Vec<f64>, values: &[(&str, f64)]) -> SrsOutputs {
    SrsOutputs {
        gains,
        values: values
            .iter()
            .map(|(k, v)| DiagnosticValue {
                key: (*k).to_string(),
                value: *v,
            })
            .collect(),
    }
}

/// The resource the interface exports: a [`ShockSrsLaw`] behind a `RefCell`,
/// because the interface hands the law shared references.
#[doc(hidden)]
pub struct SrsLawResource<L: ShockSrsLaw> {
    law: RefCell<L>,
}

const REENTERED: &str = "the law was called while a call was in flight";

impl<L: ShockSrsLaw> wit_law::GuestLaw for SrsLawResource<L> {
    fn new(setup: SrsSetup, params: Vec<Param>) -> Result<Self, String> {
        L::new(setup, Params(params)).map(|law| SrsLawResource {
            law: RefCell::new(law),
        })
    }

    fn seed(&self, inputs: SrsInputs) -> Result<SrsOutputs, String> {
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .seed(&inputs)
    }

    fn iterate(&self, inputs: SrsInputs) -> Result<SrsOutputs, String> {
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .iterate(&inputs)
    }
}
