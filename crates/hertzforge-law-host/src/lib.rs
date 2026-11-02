//! The control-law host: loads a single-output Random control law compiled to
//! a WebAssembly component (the `random-single` world of
//! `hertzforge:control-law@0.1.0`, in `wit/control-law/`) and runs it
//! contained.
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

mod bindings {
    wasmtime::component::bindgen!({
        path: "../../wit/control-law",
        world: "random-single",
    });
}

mod convert;
mod error;
mod host;
mod sandbox;
mod ticker;
mod types;
pub mod validate;

pub use error::LawError;
pub use host::{law_file_sha256, HostConfig, LawHost, LawInstance, LoadedLaw};
pub use types::{
    Breakpoint, ChoiceSpec, CurveSpec, DiagnosticSpec, DiagnosticValue, GuestOutput, IntegerSpec,
    LawIdentity, LoopDiagnostics, LoopInputs, LoopOutputs, Manifest, NumberSpec, Param, ParamKind,
    ParamSpec, ParamValue, Setup,
};

/// The interface package this host implements.
pub const INTERFACE: &str = "hertzforge:control-law@0.1.0";
/// The world a law file must implement.
pub const WORLD: &str = "random-single";
