//! Time waveform replication laws: the `twr-single` world.
//!
//! A TWR law is called between plays, never while one plays: once at proceed,
//! after the identification ([`TwrSingleLaw::seed`]), and after each judged
//! play the controller wants corrected ([`TwrSingleLaw::iterate`]). It returns
//! the next full-level drive record in volts and up to eight values its
//! manifest declares. It works at full level: the controller scales the drive
//! to the commanded level and hands back the response referred to full level,
//! so a law never sees the level and cannot change it.
//!
//! Every record is [`TwrSetup::record_len`] samples at [`TwrSetup::fs_hz`] (a
//! power of two): a quiet lead-in, the replicated section, a quiet lead-out.
//! A spectrum is the one-sided transform under the negative-exponent
//! convention; [`rfft_half`] and [`irfft_half`] are that pair, so a law works
//! in the same convention as the plant the controller hands it. The plant has
//! the classical shock world's shape.
//!
//! The controller checks what a law returns before it plays: the length, that
//! every number is finite, the declared values, the rise of the drive's peak
//! and of each bin over the last drive, the drive's predicted response peak,
//! the first drive's bound and the drive ceiling. It watches every play's
//! response as it arrives and checks every play's alignment, and it brings
//! every record to rest at both ends. A law needs none of those for safety.
//!
//! When the controller clipped the last drive, [`PlayInputs::limited`] is set
//! and [`PlayInputs::drive`] is what played: a correction that starts from
//! that record rather than from the one it asked for measures what really
//! played.

use crate::__twr_bindings::exports::hertzforge::control_law::twr_single_law as wit_law;
use crate::{DiagnosticValue, Manifest, Param, Params};
use core::cell::RefCell;

pub use crate::__twr_bindings::hertzforge::control_law::twr_types::{
    PlayInputs, PlayKind, PlayOutputs, TwrSetup, Verdict,
};
pub use crate::shock::{cplx, frf, inverse, Complex, Plant};
pub use hertzforge_shock_law_math::spectrum::{abs, conj, irfft_half, mul, rfft_half};
pub use hertzforge_shock_law_math::Cplx;

/// A time waveform replication control law.
///
/// The host builds one value per run with [`Self::new`], at proceed, calls
/// [`Self::seed`] once for the first drive, then [`Self::iterate`] after each
/// judged play the controller wants corrected. Calls never overlap.
pub trait TwrSingleLaw: Sized + 'static {
    /// The law's name, version, parameters and values. Read once, when the
    /// file is loaded; it must not depend on anything but the code.
    fn describe() -> Manifest;

    /// Build the law for a run. `params` holds every declared parameter,
    /// already checked against the manifest. An error refuses the run.
    fn new(setup: TwrSetup, params: Params) -> Result<Self, String>;

    /// The first full-level drive, before any play.
    fn seed(&mut self, inputs: &PlayInputs) -> Result<PlayOutputs, String>;

    /// The next full-level drive, after a play was played, measured and
    /// judged.
    fn iterate(&mut self, inputs: &PlayInputs) -> Result<PlayOutputs, String>;
}

/// A drive with no values.
pub fn drive_only(drive: Vec<f64>) -> PlayOutputs {
    PlayOutputs {
        drive,
        values: Vec::new(),
    }
}

/// A drive with `values` as `(key, value)` pairs.
pub fn outputs(drive: Vec<f64>, values: &[(&str, f64)]) -> PlayOutputs {
    PlayOutputs {
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

/// The resource the interface exports: a [`TwrSingleLaw`] behind a
/// `RefCell`, because the interface hands the law shared references.
#[doc(hidden)]
pub struct TwrLawResource<L: TwrSingleLaw> {
    law: RefCell<L>,
}

const REENTERED: &str = "the law was called while a call was in flight";

impl<L: TwrSingleLaw> wit_law::GuestLaw for TwrLawResource<L> {
    fn new(setup: TwrSetup, params: Vec<Param>) -> Result<Self, String> {
        L::new(setup, Params(params)).map(|law| TwrLawResource {
            law: RefCell::new(law),
        })
    }

    fn seed(&self, inputs: PlayInputs) -> Result<PlayOutputs, String> {
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .seed(&inputs)
    }

    fn iterate(&self, inputs: PlayInputs) -> Result<PlayOutputs, String> {
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .iterate(&inputs)
    }
}
