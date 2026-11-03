//! Write a single-output Random control law for Hertzforge.
//!
//! A law is a Rust library crate built to a WebAssembly component:
//!
//! ```toml
//! [lib]
//! crate-type = ["cdylib"]
//!
//! [dependencies]
//! hertzforge-law-sdk = { path = "..." }
//! ```
//!
//! It implements [`RandomLaw`] and exports it with [`export_law!`]:
//!
//! ```ignore
//! use hertzforge_law_sdk::*;
//!
//! struct Hold { drive: Vec<f64> }
//!
//! impl RandomLaw for Hold {
//!     fn describe() -> Manifest { /* name, version, parameters, diagnostics */ }
//!     fn new(setup: Setup, params: Params) -> Result<Self, String> { /* ... */ }
//!     fn seed(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, String> { /* ... */ }
//!     fn iterate(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, String> { /* ... */ }
//!     fn retarget_level(&mut self, level_db: f64, target_rms: f64) -> Result<LoopOutputs, String> { /* ... */ }
//! }
//!
//! export_law!(Hold);
//! ```
//!
//! and is built with `cargo build --target wasm32-wasip2 --release`. The file
//! `target/wasm32-wasip2/release/<crate>.wasm` is the law.
//!
//! The types are the interface's own (`wit/control-law/types.wit`),
//! re-exported here. The law sees only what it is handed: no files, no
//! network, a clock that does not move and fixed random seeds. The
//! controller applies every protection to the drive a law returns.
//!
//! Because [`RandomLaw`] is a plain trait, a law's math can also be unit
//! tested natively with `cargo test`, calling the trait methods directly.

#![warn(missing_docs)]

extern crate self as hertzforge_law_sdk;

use core::cell::RefCell;

/// The generated bindings. Not a stable API: use the re-exports.
#[doc(hidden)]
#[allow(missing_docs)]
pub mod __bindings {
    wit_bindgen::generate!({
        path: "../../wit/control-law",
        world: "random-single",
        pub_export_macro: true,
        export_macro_name: "__export_random_single",
        default_bindings_module: "hertzforge_law_sdk::__bindings",
        additional_derives: [PartialEq],
    });
}

use __bindings::exports::hertzforge::control_law::random_single_law as wit_law;

pub use __bindings::hertzforge::control_law::types::{
    Band, Breakpoint, ChoiceSpec, CurveSpec, DiagnosticSpec, DiagnosticValue, IntegerSpec,
    LoopDiagnostics, LoopInputs, LoopOutputs, Manifest, NumberSpec, Param, ParamKind, ParamSpec,
    ParamValue, Setup,
};

/// A single-output Random control law.
///
/// The host builds one value per run with [`Self::new`], calls
/// [`Self::seed`] once with the system-identification measurement, then
/// [`Self::iterate`] once per control loop, and [`Self::retarget_level`]
/// whenever the scheduled level moves. Calls never overlap.
pub trait RandomLaw: Sized + 'static {
    /// The law's name, version, parameters and diagnostics. Read once, when
    /// the file is loaded; it must not depend on anything but the code.
    fn describe() -> Manifest;

    /// Build the law for a run. `params` holds every declared parameter,
    /// already checked against the manifest. An error refuses the run.
    fn new(setup: Setup, params: Params) -> Result<Self, String>;

    /// Return the starting drive from the system-identification
    /// measurement.
    fn seed(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, String>;

    /// Return the next drive from the latest measurement.
    fn iterate(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, String>;

    /// The scheduled level moved: return the drive at the new level.
    /// `level_db` is the new level in dB relative to the full target (0 dB
    /// is full level), `target_rms` the target's in-band RMS at that level.
    fn retarget_level(&mut self, level_db: f64, target_rms: f64) -> Result<LoopOutputs, String>;
}

/// The parameter values a law is built with: one per declared parameter, in
/// manifest order, each checked by the host. The getters return an error
/// naming the key when it is missing or of another kind, which for a
/// declared key means the manifest and the code disagree.
#[derive(Debug, Clone, PartialEq)]
pub struct Params(pub Vec<Param>);

impl Params {
    /// The value under `key`.
    pub fn get(&self, key: &str) -> Result<&ParamValue, String> {
        self.0
            .iter()
            .find(|p| p.key == key)
            .map(|p| &p.value)
            .ok_or_else(|| format!("parameter `{key}` was not supplied"))
    }

    fn wrong(key: &str, want: &str) -> String {
        format!("parameter `{key}` is not {want}")
    }

    /// A number parameter.
    pub fn number(&self, key: &str) -> Result<f64, String> {
        match self.get(key)? {
            ParamValue::Number(v) => Ok(*v),
            _ => Err(Self::wrong(key, "a number")),
        }
    }

    /// An integer parameter.
    pub fn integer(&self, key: &str) -> Result<i64, String> {
        match self.get(key)? {
            ParamValue::Integer(v) => Ok(*v),
            _ => Err(Self::wrong(key, "an integer")),
        }
    }

    /// A toggle parameter.
    pub fn toggle(&self, key: &str) -> Result<bool, String> {
        match self.get(key)? {
            ParamValue::Toggle(v) => Ok(*v),
            _ => Err(Self::wrong(key, "a toggle")),
        }
    }

    /// A choice parameter: the chosen option's name.
    pub fn choice(&self, key: &str) -> Result<&str, String> {
        match self.get(key)? {
            ParamValue::Choice(v) => Ok(v),
            _ => Err(Self::wrong(key, "a choice")),
        }
    }

    /// A curve parameter.
    pub fn curve(&self, key: &str) -> Result<&[Breakpoint], String> {
        match self.get(key)? {
            ParamValue::Curve(v) => Ok(v),
            _ => Err(Self::wrong(key, "a curve")),
        }
    }

    /// A per-channel parameter: one value per control channel.
    pub fn per_channel(&self, key: &str) -> Result<&[f64], String> {
        match self.get(key)? {
            ParamValue::PerChannel(v) => Ok(v),
            _ => Err(Self::wrong(key, "a per-channel list")),
        }
    }
}

/// The band as a range of line indices.
pub fn band_range(band: Band) -> core::ops::Range<usize> {
    band.lo as usize..band.hi as usize
}

/// The in-band root-mean-square of a spectral density:
/// `sqrt(sum over the band of psd * df_hz)`. Lines past the end of `psd` are
/// ignored.
pub fn inband_rms(psd: &[f64], band: Band, df_hz: f64) -> f64 {
    let r = band_range(band);
    let hi = r.end.min(psd.len());
    let lo = r.start.min(hi);
    (psd[lo..hi].iter().sum::<f64>() * df_hz).max(0.0).sqrt()
}

/// Diagnostics with nothing to report.
pub fn no_diagnostics() -> LoopDiagnostics {
    LoopDiagnostics {
        untrusted_bins: Vec::new(),
        clamped_bins: Vec::new(),
        divergent_bins: Vec::new(),
        inband_err_db: None,
        inband_rms_g: None,
        servo_faulted: false,
        values: Vec::new(),
    }
}

/// The resource the interface exports: a [`RandomLaw`] behind a `RefCell`,
/// because the interface hands the law shared references. Calls never
/// overlap, so the borrow never fails; if it did, the call returns an error
/// rather than panicking.
#[doc(hidden)]
pub struct LawResource<L: RandomLaw> {
    law: RefCell<L>,
}

const REENTERED: &str = "the law was called while a call was in flight";

impl<L: RandomLaw> wit_law::GuestLaw for LawResource<L> {
    fn new(setup: Setup, params: Vec<Param>) -> Result<Self, String> {
        L::new(setup, Params(params)).map(|law| LawResource {
            law: RefCell::new(law),
        })
    }

    fn seed(&self, inputs: LoopInputs) -> Result<LoopOutputs, String> {
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .seed(&inputs)
    }

    fn iterate(&self, inputs: LoopInputs) -> Result<LoopOutputs, String> {
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .iterate(&inputs)
    }

    fn retarget_level(&self, level_db: f64, target_rms: f64) -> Result<LoopOutputs, String> {
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .retarget_level(level_db, target_rms)
    }
}

/// Export a [`RandomLaw`] as this crate's law. Use it once, at the crate
/// root of a `cdylib` law crate.
#[macro_export]
macro_rules! export_law {
    ($law:ty) => {
        #[doc(hidden)]
        pub struct __HertzforgeLawExport;

        impl $crate::__bindings::Guest for __HertzforgeLawExport {
            fn describe() -> $crate::Manifest {
                <$law as $crate::RandomLaw>::describe()
            }
        }

        impl $crate::__bindings::exports::hertzforge::control_law::random_single_law::Guest
            for __HertzforgeLawExport
        {
            type Law = $crate::LawResource<$law>;
        }

        $crate::__bindings::__export_random_single!(
            __HertzforgeLawExport with_types_in $crate::__bindings
        );
    };
}
