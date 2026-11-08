//! The classical shock control laws an operator can choose, and the one door
//! every one of them goes through.
//!
//! - [`resolve_classical`] turns the operator's [`LawChoice`] into the
//!   [`ClassicalLawFactory`] the classical service builds its law with
//!   (`ClassicalShockService::with_law_factory`): the built-in `fd_correct`
//!   ([`DEFAULT_CLASSICAL_LAW_ID`]) or a `shock-classical` law file from the
//!   [`LawCatalog`]. A law of another world is refused
//!   ([`ChoiceError::WrongTestType`]).
//! - [`ClassicalCheckCase`] is the check a law file passes on the simulator
//!   before it may drive hardware: the plant set, what each case asks
//!   ([`ClassicalRequirement`]), what a run on it is measured by
//!   ([`ClassicalCaseMeasure`]) and the rules that make the measure a passed or
//!   failed case ([`judge_classical`]). [`run_classical_check_case`] runs one
//!   case under the core's guards on the backend it is handed; the controller
//!   builds the simulated station for each case and records the result.
//!
//! The law catalog, check records and the arming rule are
//! `hertzforge-random-laws`'s, shared by every world.
//!
//! [`ClassicalLawFactory`]: hertzforge_shock_host::ClassicalLawFactory
//! [`LawChoice`]: hertzforge_random_laws::LawChoice
//! [`LawCatalog`]: hertzforge_random_laws::LawCatalog
//! [`ChoiceError::WrongTestType`]: hertzforge_random_laws::ChoiceError::WrongTestType

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod check;
mod choice;
mod wasm;

pub use check::{
    classical_check_config, classical_check_record, judge_classical, run_classical_check_case,
    CheckRunError, ClassicalCaseEnd, ClassicalCaseMeasure, ClassicalCheckCase, ClassicalCheckPlant,
    ClassicalRequirement, CHECK_DRIVE_ABORT_V, CHECK_DRIVE_CEILING_V, CHECK_PULSE_G,
    CLASSICAL_CHECK_VERSION,
};
pub use choice::{default_classical_choice, resolve_classical, DEFAULT_CLASSICAL_LAW_ID};
pub use wasm::{classical_law_fault, WasmClassicalLaw, WasmClassicalLawFactory};

/// The built-in law's factory: `fd_correct` natively, with its parameter values.
pub type BuiltinClassicalFactory = hertzforge_shock_host::FdCorrectFactory;
