//! Classical shock laws: the `shock-classical` world.
//!
//! A classical shock law is called between pulses, never while one plays:
//! once after system identification ([`ShockClassicalLaw::seed`]) and once after
//! each fired, measured and judged pulse ([`ShockClassicalLaw::iterate`]). It
//! returns the next full-level drive record in volts and up to eight values its
//! manifest declares. It works at full level: the controller scales the drive to
//! the scheduled level and hands back the response referred to full level, so a
//! law never sees the level and cannot change it.
//!
//! Every record is [`ClassicalSetup::record_len`] samples at
//! [`ClassicalSetup::fs_hz`] (a power of two), starting with the quiet lead-in.
//! A spectrum is the one-sided transform under the negative-exponent
//! convention; [`rfft_half`] and [`irfft_half`] are that pair, so a law works
//! in the same convention as the plant the controller hands it.
//!
//! The controller checks what a law returns before it plays: the length, that
//! every number is finite, the declared values, the rise of the drive's peak
//! and of each bin over the last drive, the first drive's predicted response,
//! the drive ceiling and abort, and it brings every record to rest at both
//! ends. A law needs none of those for safety.
//!
//! ```ignore
//! use hertzforge_law_sdk::shock::*;
//! use hertzforge_law_sdk::{Manifest, Params, ShockClassicalLaw};
//!
//! struct Feedforward { drive: Vec<f64> }
//!
//! impl ShockClassicalLaw for Feedforward {
//!     fn describe() -> Manifest { /* ... */ }
//!     fn new(setup: ClassicalSetup, _params: Params) -> Result<Self, String> {
//!         // D = G R on the trusted bins.
//!         let r = rfft_half(&setup.reference)?;
//!         let d: Vec<Cplx> = r.iter().zip(inverse(&setup.plant)).zip(&setup.plant.trusted)
//!             .map(|((&r, g), &t)| if t { mul(r, g) } else { (0.0, 0.0) })
//!             .collect();
//!         Ok(Feedforward { drive: irfft_half(&d, setup.record_len as usize)? })
//!     }
//!     fn seed(&mut self, _: &PulseInputs) -> Result<PulseOutputs, String> {
//!         Ok(drive_only(self.drive.clone()))
//!     }
//!     fn iterate(&mut self, _: &PulseInputs) -> Result<PulseOutputs, String> {
//!         Ok(drive_only(self.drive.clone()))
//!     }
//! }
//! ```

use crate::__shock_bindings::exports::hertzforge::control_law::shock_classical_law as wit_law;
use crate::{DiagnosticValue, Manifest, Param, Params};
use core::cell::RefCell;

pub use crate::__shock_bindings::hertzforge::control_law::classical_types::{
    ClassicalSetup, Complex, Plant, PulseInputs, PulseOutputs, Verdict,
};
pub use hertzforge_shock_law_math::spectrum::{abs, conj, irfft_half, mul, rfft_half};
pub use hertzforge_shock_law_math::Cplx;

/// A classical shock control law.
///
/// The host builds one value per run with [`Self::new`], after system
/// identification, calls [`Self::seed`] once for the first drive, then
/// [`Self::iterate`] after each pulse. Calls never overlap.
pub trait ShockClassicalLaw: Sized + 'static {
    /// The law's name, version, parameters and values. Read once, when the
    /// file is loaded; it must not depend on anything but the code.
    fn describe() -> Manifest;

    /// Build the law for a run. `params` holds every declared parameter,
    /// already checked against the manifest. An error refuses the run.
    fn new(setup: ClassicalSetup, params: Params) -> Result<Self, String>;

    /// The first full-level drive, before any pulse has fired.
    fn seed(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, String>;

    /// The next full-level drive, after a pulse was fired, measured and
    /// judged.
    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, String>;
}

/// A list of the world's complex values as `(re, im)` pairs.
pub fn cplx(v: &[Complex]) -> Vec<Cplx> {
    v.iter().map(|c| (c.re, c.im)).collect()
}

/// The plant's regularized inverse as `(re, im)` pairs.
pub fn inverse(plant: &Plant) -> Vec<Cplx> {
    cplx(&plant.inverse)
}

/// The plant's FRF as `(re, im)` pairs.
pub fn frf(plant: &Plant) -> Vec<Cplx> {
    cplx(&plant.frf)
}

/// A drive with no values.
pub fn drive_only(drive: Vec<f64>) -> PulseOutputs {
    PulseOutputs {
        drive,
        values: Vec::new(),
    }
}

/// A drive with `values` as `(key, value)` pairs.
pub fn outputs(drive: Vec<f64>, values: &[(&str, f64)]) -> PulseOutputs {
    PulseOutputs {
        drive,
        values: values
            .iter()
            .map(|(k, v)| DiagnosticValue {
                key: (*k).to_string(),
                value: *v,
            })
            .collect(),
    }
}

/// The resource the interface exports: a [`ShockClassicalLaw`] behind a
/// `RefCell`, because the interface hands the law shared references.
#[doc(hidden)]
pub struct ClassicalLawResource<L: ShockClassicalLaw> {
    law: RefCell<L>,
}

const REENTERED: &str = "the law was called while a call was in flight";

impl<L: ShockClassicalLaw> wit_law::GuestLaw for ClassicalLawResource<L> {
    fn new(setup: ClassicalSetup, params: Vec<Param>) -> Result<Self, String> {
        L::new(setup, Params(params)).map(|law| ClassicalLawResource {
            law: RefCell::new(law),
        })
    }

    fn seed(&self, inputs: PulseInputs) -> Result<PulseOutputs, String> {
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .seed(&inputs)
    }

    fn iterate(&self, inputs: PulseInputs) -> Result<PulseOutputs, String> {
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .iterate(&inputs)
    }
}
