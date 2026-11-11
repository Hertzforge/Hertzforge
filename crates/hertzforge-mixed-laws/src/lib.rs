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

pub use check::{
    mixed_check_config, mixed_check_run_s, mixed_check_station, mixed_check_table,
    run_mixed_check_case, MixedCheckCase, MixedCheckError, MixedCheckStation,
    MIXED_CHECK_DELAY_FRAMES, MIXED_CHECK_FS_HZ, MIXED_CHECK_MODE,
};
pub use choice::{law_choice, resolve_mixed, MixedLawError};
