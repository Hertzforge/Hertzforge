//! The Sine control laws an operator can choose, and the one door every one of
//! them goes through.
//!
//! - [`resolve_sine`] turns the operator's [`LawChoice`] into the
//!   [`SineLawFactory`] the Sine service builds its law with
//!   (`SineService::with_law_factory`): the built-in `servo_blend`
//!   ([`DEFAULT_SINE_LAW_ID`]) or a `sine-single` law file from the
//!   [`LawCatalog`]. A law of another world is refused
//!   ([`ChoiceError::WrongTestType`]).
//! - [`SineCheckCase`] is the check a law file passes on the simulator before
//!   it may drive hardware: the plant set, what each case asks
//!   ([`SineRequirement`]), what a run on it is measured by
//!   ([`SineCaseMeasure`]) and the rules that make the measure a passed or
//!   failed case ([`judge_sine`]). [`run_check_case`] runs one case under the
//!   core's guards on the backend it is handed; the controller builds the
//!   simulated station for each case and records the result.
//!
//! The law catalog, check records and the arming rule are
//! `hertzforge-random-laws`'s, shared by every world.
//!
//! A law file runs as the core's law through [`WasmSineLaw`] (the host's
//! deadline on every call is the core's own call budget); the built-in is the
//! native `servo_blend` ([`BuiltinSineFactory`]).
//!
//! [`SineLawFactory`]: hertzforge_sine::SineLawFactory
//! [`LawChoice`]: hertzforge_random_laws::LawChoice
//! [`LawCatalog`]: hertzforge_random_laws::LawCatalog
//! [`ChoiceError::WrongTestType`]: hertzforge_random_laws::ChoiceError::WrongTestType

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod check;
mod choice;
mod wasm;

pub use check::{
    judge_sine, run_check_case, sine_check_config, sine_check_record, sine_check_station,
    sine_check_table, trip_name, CheckRunError, SineCaseEnd, SineCaseMeasure, SineCheckCase,
    SineCheckLimit, SineCheckMode, SineCheckPlant, SineCheckStation, SineCheckStationSpec,
    SineRequirement, SINE_CHECK_ABORT_DB, SINE_CHECK_ANTI_RESONANCE_NOISE_G,
    SINE_CHECK_DRIVE_ABORT_V, SINE_CHECK_DRIVE_CEILING_V, SINE_CHECK_FEEDTHROUGH, SINE_CHECK_FS_HZ,
    SINE_CHECK_NOISE_FLOOR_G, SINE_CHECK_RELEASE_WATCH_S, SINE_CHECK_SEED, SINE_CHECK_SYSID_DB,
    SINE_CHECK_TARGET_G, SINE_CHECK_TICK_S, SINE_CHECK_VERSION,
};
pub use choice::{
    builtin_sine_params, default_sine_choice, resolve_sine, BuiltinSineFactory, DEFAULT_SINE_LAW_ID,
};
pub use wasm::{sine_law_fault, WasmSineLaw, WasmSineLawFactory};
