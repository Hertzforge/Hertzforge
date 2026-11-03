//! The built-in multiple-output (MIMO) Random control laws.
//!
//! Each law is written once against the MIMO law boundary of `hertzforge-sdm`
//! ([`MimoControlLaw`]): the controller runs it natively, and a law file crate
//! under `laws/` builds the same code into a law file through [`law_file`]
//! (the `law-file` feature). Everything a law sees and returns is at full
//! level; the controller owns the level schedule and every protection.
//!
//! Each law declares its parameters, diagnostics, supported shapes and needed
//! inputs once, as a [`LawSpec`]; the law file's manifest and the controller's
//! catalog entry are both built from it.
//!
//! - [`ZeroLaw`]: always the zero drive. It proves the path a law takes; it
//!   controls nothing.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod spec;
mod zero;

#[cfg(feature = "law-file")]
pub mod law_file;

pub use spec::{DiagDecl, DiagKind, LawSpec, ParamDecl, ParamKindDecl, ParamVal};
pub use zero::ZeroLaw;

use hertzforge_sdm::law::{LawError, MimoControlLaw, MimoSetup};

/// A law built into the controller: its declaration, and how it is built for
/// a run from parameter values.
pub trait BuiltinMimoLaw: MimoControlLaw + Sized + Send + 'static {
    /// Everything the law declares: name, version, author, summary,
    /// parameters, diagnostics, shapes and needed inputs.
    fn spec() -> LawSpec;

    /// Build the law for a run. `params` holds every declared parameter by
    /// key, already checked against its declaration.
    fn build(setup: &MimoSetup, params: &[(String, ParamVal)]) -> Result<Self, LawError>;

    /// Build the law with its defaults, except the values in `given`, each
    /// checked against its declaration.
    fn with_params(setup: &MimoSetup, given: &[(&str, ParamVal)]) -> Result<Self, LawError> {
        let params = Self::spec().resolve(given)?;
        Self::build(setup, &params)
    }
}
