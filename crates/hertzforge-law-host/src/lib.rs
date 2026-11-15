//! The control-law host: loads a control law compiled to a WebAssembly
//! component (the `random-single`, `random-mimo`, `shock-classical`,
//! `shock-srs`, `sine-single`, `twr-single` or `shock-mimo` world of
//! `hertzforge:control-law@0.1.0`, in `wit/control-law/`) and runs it
//! contained. [`LawHost::load`] loads a single-output Random law,
//! [`LawHost::load_mimo`] a multiple-output one, [`LawHost::load_classical`] a
//! classical shock law, [`LawHost::load_srs`] an SRS shock law,
//! [`LawHost::load_sine`] a Sine law, [`LawHost::load_shock_mimo`] a MIMO Shock
//! law, and [`LawHost::load_any`] whichever the file is.
//!
//! # What the host guarantees
//!
//! - **Compile once.** [`LawHost::load`] compiles the file and reads its
//!   manifest. [`LoadedLaw::instantiate`] and every call after it compile
//!   nothing.
//! - **A deadline on every call.** Each call takes a wall-clock budget. A
//!   background ticker advances the engine's epoch while a call runs, and the
//!   law is stopped within about one tick (1 ms by default) of its budget:
//!   [`LawError::Deadline`].
//! - **A memory cap.** The law's total linear memory is capped (256 MiB by
//!   default). A law that fails past the cap gets [`LawError::Memory`].
//! - **No system access.** The law gets a WASI 0.2 context with no preopened
//!   directories, no network, no environment variables, no arguments and an
//!   empty stdin. Stdout and stderr go to a small buffer that keeps their
//!   last bytes ([`LawInstance::guest_output`]); what a law wrote to stderr
//!   during a failing call is attached to its [`LawError::Trap`].
//! - **Determinism.** The wall clock reads the Unix epoch, the monotonic clock
//!   reads zero, a sleep returns at once, and both random sources are seeded
//!   with fixed values. NaN bit patterns are canonicalized. Two instances of
//!   one law given the same inputs return the same outputs.
//! - **Checked values both ways.** The manifest, the parameter values, the
//!   setup and each loop's inputs are checked before the law sees them, and
//!   each loop's outputs before the caller does ([`validate`]).
//!
//! The host does not protect the drive: it only refuses outputs that are not
//! a well-formed drive spectrum. Every protection (drive ceiling and abort,
//! limit notching, envelope abort, divergence, change cap) belongs to the
//! controller and applies to every law.
//!
//! # Types
//!
//! [`LoopInputs`] and [`LoopOutputs`] mirror the controller's own loop
//! boundary field for field, so an adapter implements the controller's law
//! trait over a [`LawInstance`] with plain copies. This crate depends on no
//! controller crate.
//!
//! A multiple-output law is written against the MIMO law boundary of
//! `hertzforge-sdm`, and [`WasmMimoLaw`] implements that boundary's
//! [`hertzforge_sdm::law::MimoControlLaw`] over a [`MimoLawInstance`]: the
//! controller runs a law file exactly as it runs a built-in law. The host
//! checks a MIMO drive's dimensions, finiteness, Hermitian symmetry and band
//! ([`mimo`]); positive semi-definiteness and every cap belong to the
//! controller.

mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit/control-law",
        world: "random-single",
    });
}

/// The `random-mimo` world. The shared `types` interface is the one
/// [`bindings`] generated, so a manifest converts the same way for both.
mod mimo_bindings {
    wasmtime::component::bindgen!({
        path: "../../wit/control-law",
        world: "random-mimo",
        with: {
            "hertzforge:control-law/types": crate::bindings::hertzforge::control_law::types,
        },
    });
}

mod convert;
mod error;
mod host;
pub mod mimo;
mod sandbox;
pub mod shock_classical;
pub mod shock_mimo;
pub mod shock_srs;
pub mod sine;
mod ticker;
pub mod twr;
mod types;
pub mod validate;

pub use error::{
    ClassicalOutputRule, MimoOutputRule, ShockMimoOutputRule, SineOutputRule, SrsOutputRule,
    TwrOutputRule,
};
pub use error::{LawError, OutputRule};
pub use host::{law_file_sha256, AnyLaw, HostConfig, LawHost, LawInstance, LoadedLaw};
pub use mimo::{
    DiagnosticKind, DiagnosticLayout, LoadedMimoLaw, MimoLawInstance, MimoManifest, WasmMimoLaw,
};
pub use shock_classical::{
    ClassicalInputs, ClassicalLawInstance, ClassicalOutputs, ClassicalPlant, ClassicalSetup,
    ClassicalVerdict, LoadedClassicalLaw, MAX_CLASSICAL_VALUES,
};
pub use shock_mimo::{
    drive_over_ceiling, CeilingExcess, LoadedShockMimoLaw, ShockMimoLawInstance, ShockMimoManifest,
    MAX_SHOCK_MIMO_VALUES,
};
pub use shock_srs::{
    LoadedSrsLaw, SrsInputs, SrsLawInstance, SrsOutputs, SrsSetup, MAX_SRS_VALUES,
};
pub use sine::{
    LoadedSineLaw, SineHold, SineIdentification, SineInputs, SineLawInstance, SineOutputs,
    SineSetup, MAX_SINE_VALUES,
};
pub use twr::{
    LoadedTwrLaw, TwrInputs, TwrLawInstance, TwrOutputs, TwrPlayKind, TwrSetup, TwrVerdict,
    MAX_TWR_VALUES,
};
pub use types::{
    Breakpoint, ChoiceSpec, CurveSpec, DiagnosticSpec, DiagnosticValue, GuestOutput, IntegerSpec,
    LawIdentity, LoopDiagnostics, LoopInputs, LoopOutputs, Manifest, NumberSpec, Param, ParamKind,
    ParamSpec, ParamValue, Setup,
};

/// The interface package this host implements.
pub const INTERFACE: &str = "hertzforge:control-law@0.1.0";
/// The world a single-output law file implements.
pub const WORLD: &str = "random-single";
/// The world a multiple-output law file implements.
pub const MIMO_WORLD: &str = "random-mimo";
/// The world a classical shock law file implements.
pub const CLASSICAL_WORLD: &str = "shock-classical";
/// The world an SRS shock law file implements.
pub const SRS_WORLD: &str = "shock-srs";
/// The world a MIMO Shock law file implements.
pub const SHOCK_MIMO_WORLD: &str = "shock-mimo";
/// The world a single-output Sine law file implements.
pub const SINE_WORLD: &str = "sine-single";
/// The world a time waveform replication law file implements.
pub const TWR_WORLD: &str = "twr-single";
