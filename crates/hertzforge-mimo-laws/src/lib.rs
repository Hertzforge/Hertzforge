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
//! The four built-in laws ([`BuiltinMimo`]), each from the published
//! literature and cited in its module by author, title and venue:
//!
//! - [`PseudoInverseLaw`], the default: the regularized pseudo-inverse drive,
//!   then Smallwood's closed-loop correction of a factor of the reference.
//! - [`OpenLoopLaw`]: the regularized pseudo-inverse drive, re-applied every
//!   loop.
//! - [`ShapeConstrainedLaw`]: the drive held to the plant's `m` leading
//!   singular directions, closed loop inside the constraint.
//! - [`BuzzMatchedLaw`]: the reference's autospectra with the coherence and
//!   phase of the identification response, then the default closed loop.
//!
//! And [`ZeroLaw`]: always the zero drive. It proves the path a law takes; it
//! controls nothing.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod buzz;
mod factor;
mod open_loop;
mod pinv;
mod shape;
pub mod spec;
mod support;
mod zero;

#[cfg(feature = "law-file")]
pub mod law_file;

pub use buzz::{buzz_reference, BuzzConfig, BuzzMatchedLaw};
pub use open_loop::{OpenLoopConfig, OpenLoopLaw};
pub use pinv::{PseudoInverseConfig, PseudoInverseLaw};
pub use shape::{ShapeConfig, ShapeConstrainedLaw};
pub use spec::{DiagDecl, DiagKind, LawSpec, ParamDecl, ParamKindDecl, ParamVal};
pub use zero::ZeroLaw;

use hertzforge_sdm::law::{LawError, MimoControlLaw, MimoSetup};

/// The id of the default multiple-output law.
pub const DEFAULT_MIMO_LAW_ID: &str = "pinv_closed";

/// A multiple-output law built into the controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinMimo {
    /// The pseudo-inverse with closed-loop correction: the default.
    PseudoInverse,
    /// The open-loop pseudo-inverse.
    OpenLoop,
    /// Shape-constrained (SVD) control.
    ShapeConstrained,
    /// Buzz-test matched control.
    BuzzMatched,
}

impl BuiltinMimo {
    /// Every built-in law, the default first.
    pub const ALL: [BuiltinMimo; 4] = [
        BuiltinMimo::PseudoInverse,
        BuiltinMimo::OpenLoop,
        BuiltinMimo::ShapeConstrained,
        BuiltinMimo::BuzzMatched,
    ];

    /// The stable id a choice of law names it by.
    pub fn id(self) -> &'static str {
        match self {
            BuiltinMimo::PseudoInverse => DEFAULT_MIMO_LAW_ID,
            BuiltinMimo::OpenLoop => "pinv_open",
            BuiltinMimo::ShapeConstrained => "shape_svd",
            BuiltinMimo::BuzzMatched => "buzz_match",
        }
    }

    /// The built-in law with `id`, if any.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.id() == id)
    }

    /// Everything the law declares.
    pub fn spec(self) -> LawSpec {
        match self {
            BuiltinMimo::PseudoInverse => PseudoInverseLaw::spec(),
            BuiltinMimo::OpenLoop => OpenLoopLaw::spec(),
            BuiltinMimo::ShapeConstrained => ShapeConstrainedLaw::spec(),
            BuiltinMimo::BuzzMatched => BuzzMatchedLaw::spec(),
        }
    }

    /// The law built for a run from every declared parameter by key.
    pub fn build(
        self,
        setup: &MimoSetup,
        params: &[(String, ParamVal)],
    ) -> Result<Box<dyn MimoControlLaw + Send>, LawError> {
        Ok(match self {
            BuiltinMimo::PseudoInverse => Box::new(PseudoInverseLaw::build(setup, params)?),
            BuiltinMimo::OpenLoop => Box::new(OpenLoopLaw::build(setup, params)?),
            BuiltinMimo::ShapeConstrained => Box::new(ShapeConstrainedLaw::build(setup, params)?),
            BuiltinMimo::BuzzMatched => Box::new(BuzzMatchedLaw::build(setup, params)?),
        })
    }
}

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
