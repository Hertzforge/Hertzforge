//! The single-output Random control laws an operator can choose from, and the
//! one door every law goes through.
//!
//! - [`LawCatalog`] lists every law: the built-in laws first ([`BuiltinLaw`]:
//!   `hinv`, the default, and `ratio`), then the law files imported into the
//!   local data root. Every entry carries the same [`Manifest`] type, so a form
//!   or a list treats a built-in law and an imported one alike.
//! - [`LawChoice`] names a law and its parameter values;
//!   [`LawCatalog::resolve`] checks the values against the law's manifest and
//!   returns the [`LawFactory`] the run service builds the law with.
//! - [`WasmControlLaw`] runs a law file as the run core's
//!   [`hertzforge_random::ControlLaw`]: the law host contains it (a deadline on
//!   every call from the run's own call budget, a memory cap, no system
//!   access), and the run core guards its output exactly as it guards a native
//!   law's.
//! - [`CheckRecord`] is the result of checking a law file on the simulator, kept
//!   beside it; [`may_arm_on_hardware`] is the rule the arming gate applies.
//!
//! The built-in laws run natively; each also exists as a law file built from
//! the same code (`laws/hinv`, `laws/ratio`), which the tests hold to the
//! native law's behavior over a whole run.
//!
//! [`Manifest`]: hertzforge_law_host::Manifest
//! [`LawFactory`]: hertzforge_random::LawFactory

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod adapter;
mod builtin;
mod catalog;
mod check;
mod choice;
mod ratio;

pub use adapter::{law_fault, WasmControlLaw, WasmLawFactory};
pub use builtin::{BuiltinLaw, DEFAULT_LAW_ID};
pub use catalog::{
    CatalogEntry, CatalogError, CatalogProblem, CheckStatus, LawCatalog, LAWS_DIR,
    MAX_LAW_FILE_BYTES,
};
pub use check::{may_arm_on_hardware, ArmRefusal, CheckCase, CheckRecord, CHECK_FORMAT};
pub use choice::{ChoiceError, LawChoice};
pub use ratio::{NativeRatioFactory, NativeRatioLaw};
