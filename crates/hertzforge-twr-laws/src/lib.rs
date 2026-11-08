//! The TWR control laws an operator can choose, and the one door every one of
//! them goes through.
//!
//! - [`resolve_twr`] turns the operator's [`LawChoice`] into the
//!   [`TwrLawFactory`] the TWR service builds its law with
//!   (`TwrService::with_law_factory`): the built-in `twr_correct`
//!   ([`DEFAULT_TWR_LAW_ID`]) or a `twr-single` law file from the
//!   [`LawCatalog`]. A law of another world is refused
//!   ([`ChoiceError::WrongTestType`]).
//! - [`TwrCheckCase`] is the check a law file passes on the simulator before it
//!   may drive hardware: the plant set, what each case asks
//!   ([`TwrRequirement`]), what a run on it is measured by
//!   ([`TwrCaseMeasure`]) and the rules that make the measure a passed or failed
//!   case ([`judge_twr`]). [`run_twr_check_case`] runs one case under the core's
//!   guards on the backend it is handed; the controller builds the simulated
//!   station for each case and records the result.
//!
//! The law catalog, check records and the arming rule are
//! `hertzforge-random-laws`'s, shared by every world.
//!
//! [`TwrLawFactory`]: hertzforge_twr::TwrLawFactory
//! [`LawChoice`]: hertzforge_random_laws::LawChoice
//! [`LawCatalog`]: hertzforge_random_laws::LawCatalog
//! [`ChoiceError::WrongTestType`]: hertzforge_random_laws::ChoiceError::WrongTestType

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod check;
mod choice;
mod wasm;

pub use check::{
    judge_twr, run_twr_check_case, twr_check_config, twr_check_record, twr_check_reference,
    twr_check_station, CheckRunError, TwrCaseEnd, TwrCaseMeasure, TwrCheckCase, TwrCheckPlant,
    TwrCheckStation, TwrCheckStationSpec, TwrRequirement, TWR_CHECK_DRIVE_CEILING_V,
    TWR_CHECK_FEEDTHROUGH, TWR_CHECK_FS_HZ, TWR_CHECK_LEVELS_DB, TWR_CHECK_NOISE_FLOOR_G,
    TWR_CHECK_PEAK_G, TWR_CHECK_RECORD_LEN, TWR_CHECK_SYSID_DRIVE_VRMS, TWR_CHECK_TIKHONOV_FLOOR,
    TWR_CHECK_VERSION,
};
pub use choice::{
    builtin_twr_params, default_twr_choice, resolve_twr, BuiltinTwrFactory, DEFAULT_TWR_LAW_ID,
};
pub use wasm::{twr_law_fault, WasmTwrLaw, WasmTwrLawFactory};
