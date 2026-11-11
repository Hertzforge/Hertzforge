//! The laws of a Mixed Mode run.
//!
//! A run takes two laws from the catalog: a single-output Random law for the
//! floor (default `hinv`) and a Sine law for the tones (default
//! `servo_blend`, one instance per tone). [`resolve_mixed`] resolves each
//! through its own world's resolver and refuses a law of the wrong world by
//! name. [`MixedCheckCase`] lists the cases the composed core is held to.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod check;
mod choice;

pub use check::{run_mixed_check_case, MixedCheckCase, MixedCheckError};
pub use choice::{law_choice, resolve_mixed, MixedLawError};
