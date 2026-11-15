//! Write a control law for Hertzforge: single-output Random ([`RandomLaw`],
//! [`export_law!`]), multiple-output Random ([`RandomMimoLaw`],
//! [`export_mimo_law!`], the [`mimo`] module), classical and SRS shock
//! ([`shock`], [`srs`]), MIMO Shock ([`ShockMimoLaw`],
//! [`export_shock_mimo_law!`], the [`shock_mimo`] module), single-output Sine ([`SineSingleLaw`],
//! [`export_sine_law!`], the [`sine`] module) or time waveform replication
//! ([`TwrSingleLaw`], [`export_twr_law!`], the [`twr`] module).
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
//!
//! # Multiple-output Random
//!
//! A multiple-output law implements [`RandomMimoLaw`] and is exported with
//! [`export_mimo_law!`]. It is written against the spectral density matrix
//! stacks of `hertzforge-sdm` ([`mimo::HermitianStack`], [`mimo::MatrixStack`])
//! and the native boundary [`mimo::MimoSetup`], [`mimo::MimoLoopInputs`] and
//! [`mimo::MimoLoopOutputs`]: the SDK unpacks what the interface carries into
//! those types and packs the drive back, so the same code runs natively inside
//! the controller and as a law file. Its manifest is built with
//! [`mimo::MimoManifestBuilder`], which adds the shapes the law supports, the
//! optional inputs it needs and the layout of each diagnostic to the
//! parameters and diagnostics every law declares.

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

/// The generated bindings of the `random-mimo` world. Not a stable API: use
/// [`mimo`] and the re-exports. The shared `types` interface is the one
/// [`__bindings`] generated, so a parameter or a manifest is one Rust type in
/// both worlds.
#[doc(hidden)]
#[allow(missing_docs)]
pub mod __mimo_bindings {
    wit_bindgen::generate!({
        path: "../../wit/control-law",
        world: "random-mimo",
        pub_export_macro: true,
        export_macro_name: "__export_random_mimo",
        default_bindings_module: "hertzforge_law_sdk::__mimo_bindings",
        additional_derives: [PartialEq],
        with: {
            "hertzforge:control-law/types@0.1.0": hertzforge_law_sdk::__bindings::hertzforge::control_law::types,
        },
    });
}

/// The generated bindings of the `shock-classical` world. Not a stable API:
/// use [`shock`]. The shared `types` interface is the one [`__bindings`]
/// generated.
#[doc(hidden)]
#[allow(missing_docs)]
pub mod __shock_bindings {
    wit_bindgen::generate!({
        path: "../../wit/control-law",
        world: "shock-classical",
        pub_export_macro: true,
        export_macro_name: "__export_shock_classical",
        default_bindings_module: "hertzforge_law_sdk::__shock_bindings",
        additional_derives: [PartialEq],
        with: {
            "hertzforge:control-law/types@0.1.0": hertzforge_law_sdk::__bindings::hertzforge::control_law::types,
        },
    });
}

/// The generated bindings of the `shock-srs` world. Not a stable API: use
/// [`srs`]. The shared `types` interface is the one [`__bindings`] generated.
#[doc(hidden)]
#[allow(missing_docs)]
pub mod __srs_bindings {
    wit_bindgen::generate!({
        path: "../../wit/control-law",
        world: "shock-srs",
        pub_export_macro: true,
        export_macro_name: "__export_shock_srs",
        default_bindings_module: "hertzforge_law_sdk::__srs_bindings",
        additional_derives: [PartialEq],
        with: {
            "hertzforge:control-law/types@0.1.0": hertzforge_law_sdk::__bindings::hertzforge::control_law::types,
        },
    });
}

/// The generated bindings of the `sine-single` world. Not a stable API: use
/// [`sine`]. The shared `types` interface is the one [`__bindings`] generated.
#[doc(hidden)]
#[allow(missing_docs)]
pub mod __sine_bindings {
    wit_bindgen::generate!({
        path: "../../wit/control-law",
        world: "sine-single",
        pub_export_macro: true,
        export_macro_name: "__export_sine_single",
        default_bindings_module: "hertzforge_law_sdk::__sine_bindings",
        additional_derives: [PartialEq],
        with: {
            "hertzforge:control-law/types@0.1.0": hertzforge_law_sdk::__bindings::hertzforge::control_law::types,
        },
    });
}

/// The generated bindings of the `twr-single` world. Not a stable API: use
/// [`twr`]. The shared `types` interface is the one [`__bindings`] generated
/// and the plant's `classical-types` the one [`__shock_bindings`] generated.
#[doc(hidden)]
#[allow(missing_docs)]
pub mod __twr_bindings {
    wit_bindgen::generate!({
        path: "../../wit/control-law",
        world: "twr-single",
        pub_export_macro: true,
        export_macro_name: "__export_twr_single",
        default_bindings_module: "hertzforge_law_sdk::__twr_bindings",
        additional_derives: [PartialEq],
        with: {
            "hertzforge:control-law/types@0.1.0": hertzforge_law_sdk::__bindings::hertzforge::control_law::types,
            "hertzforge:control-law/classical-types@0.1.0": hertzforge_law_sdk::__shock_bindings::hertzforge::control_law::classical_types,
        },
    });
}

/// The generated bindings of the `shock-mimo` world. Not a stable API: use
/// [`shock_mimo`]. The shared `types` interface is the one [`__bindings`]
/// generated.
#[doc(hidden)]
#[allow(missing_docs)]
pub mod __shock_mimo_bindings {
    wit_bindgen::generate!({
        path: "../../wit/control-law",
        world: "shock-mimo",
        pub_export_macro: true,
        export_macro_name: "__export_shock_mimo",
        default_bindings_module: "hertzforge_law_sdk::__shock_mimo_bindings",
        additional_derives: [PartialEq],
        with: {
            "hertzforge:control-law/types@0.1.0": hertzforge_law_sdk::__bindings::hertzforge::control_law::types,
        },
    });
}

pub mod mimo;
pub mod shock;
pub mod shock_mimo;
pub mod sine;
pub mod srs;
pub mod twr;

pub use mimo::{MimoLawResource, RandomMimoLaw};
pub use shock::{ClassicalLawResource, ShockClassicalLaw};
pub use shock_mimo::{ShockMimoLaw, ShockMimoLawResource};
pub use sine::{SineLawResource, SineSingleLaw};
pub use srs::{ShockSrsLaw, SrsLawResource};
pub use twr::{TwrLawResource, TwrSingleLaw};

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

/// Export a [`ShockClassicalLaw`] as this crate's law. Use it once, at the
/// crate root of a `cdylib` law crate. A law crate exports one law.
#[macro_export]
macro_rules! export_classical_law {
    ($law:ty) => {
        #[doc(hidden)]
        pub struct __HertzforgeClassicalLawExport;

        impl $crate::__shock_bindings::Guest for __HertzforgeClassicalLawExport {
            fn describe() -> $crate::Manifest {
                <$law as $crate::ShockClassicalLaw>::describe()
            }
        }

        impl $crate::__shock_bindings::exports::hertzforge::control_law::shock_classical_law::Guest
            for __HertzforgeClassicalLawExport
        {
            type Law = $crate::ClassicalLawResource<$law>;
        }

        $crate::__shock_bindings::__export_shock_classical!(
            __HertzforgeClassicalLawExport with_types_in $crate::__shock_bindings
        );
    };
}

/// Export a [`TwrSingleLaw`] as this crate's law. Use it once, at the crate
/// root of a `cdylib` law crate. A law crate exports one law.
#[macro_export]
macro_rules! export_twr_law {
    ($law:ty) => {
        #[doc(hidden)]
        pub struct __HertzforgeTwrLawExport;

        impl $crate::__twr_bindings::Guest for __HertzforgeTwrLawExport {
            fn describe() -> $crate::Manifest {
                <$law as $crate::TwrSingleLaw>::describe()
            }
        }

        impl $crate::__twr_bindings::exports::hertzforge::control_law::twr_single_law::Guest
            for __HertzforgeTwrLawExport
        {
            type Law = $crate::TwrLawResource<$law>;
        }

        $crate::__twr_bindings::__export_twr_single!(
            __HertzforgeTwrLawExport with_types_in $crate::__twr_bindings
        );
    };
}

/// Export a [`ShockSrsLaw`] as this crate's law. Use it once, at the crate
/// root of a `cdylib` law crate. A law crate exports one law.
#[macro_export]
macro_rules! export_srs_law {
    ($law:ty) => {
        #[doc(hidden)]
        pub struct __HertzforgeSrsLawExport;

        impl $crate::__srs_bindings::Guest for __HertzforgeSrsLawExport {
            fn describe() -> $crate::Manifest {
                <$law as $crate::ShockSrsLaw>::describe()
            }
        }

        impl $crate::__srs_bindings::exports::hertzforge::control_law::shock_srs_law::Guest
            for __HertzforgeSrsLawExport
        {
            type Law = $crate::SrsLawResource<$law>;
        }

        $crate::__srs_bindings::__export_shock_srs!(
            __HertzforgeSrsLawExport with_types_in $crate::__srs_bindings
        );
    };
}

/// Export a [`SineSingleLaw`] as this crate's law. Use it once, at the crate
/// root of a `cdylib` law crate. A law crate exports one law.
#[macro_export]
macro_rules! export_sine_law {
    ($law:ty) => {
        #[doc(hidden)]
        pub struct __HertzforgeSineLawExport;

        impl $crate::__sine_bindings::Guest for __HertzforgeSineLawExport {
            fn describe() -> $crate::sine::SineManifest {
                <$law as $crate::SineSingleLaw>::describe()
            }
        }

        impl $crate::__sine_bindings::exports::hertzforge::control_law::sine_single_law::Guest
            for __HertzforgeSineLawExport
        {
            type Law = $crate::SineLawResource<$law>;
        }

        $crate::__sine_bindings::__export_sine_single!(
            __HertzforgeSineLawExport with_types_in $crate::__sine_bindings
        );
    };
}

/// Export a [`RandomMimoLaw`] as this crate's law. Use it once, at the crate
/// root of a `cdylib` law crate. A law crate exports one law: either this or
/// [`export_law!`].
#[macro_export]
macro_rules! export_mimo_law {
    ($law:ty) => {
        #[doc(hidden)]
        pub struct __HertzforgeMimoLawExport;

        impl $crate::__mimo_bindings::Guest for __HertzforgeMimoLawExport {
            fn describe() -> $crate::mimo::MimoManifest {
                <$law as $crate::RandomMimoLaw>::describe()
            }
        }

        impl $crate::__mimo_bindings::exports::hertzforge::control_law::random_mimo_law::Guest
            for __HertzforgeMimoLawExport
        {
            type Law = $crate::MimoLawResource<$law>;
        }

        $crate::__mimo_bindings::__export_random_mimo!(
            __HertzforgeMimoLawExport with_types_in $crate::__mimo_bindings
        );
    };
}

/// Export a [`ShockMimoLaw`] as this crate's law. Use it once, at the crate
/// root of a `cdylib` law crate. A law crate exports one law.
#[macro_export]
macro_rules! export_shock_mimo_law {
    ($law:ty) => {
        #[doc(hidden)]
        pub struct __HertzforgeShockMimoLawExport;

        impl $crate::__shock_mimo_bindings::Guest for __HertzforgeShockMimoLawExport {
            fn describe() -> $crate::shock_mimo::ShockMimoManifest {
                <$law as $crate::ShockMimoLaw>::describe()
            }
        }

        impl $crate::__shock_mimo_bindings::exports::hertzforge::control_law::shock_mimo_law::Guest
            for __HertzforgeShockMimoLawExport
        {
            type Law = $crate::ShockMimoLawResource<$law>;
        }

        $crate::__shock_mimo_bindings::__export_shock_mimo!(
            __HertzforgeShockMimoLawExport with_types_in $crate::__shock_mimo_bindings
        );
    };
}
