//! The Modal Test: impact (roving hammer) and shaker FRF measurement, on the
//! `hertzforge-modal-test` crate. The service's side: the setup as the wire gives it,
//! the run and its thread, the record, and the simulator's structure.

pub mod config;
pub mod records;
pub mod run;
pub mod sim_source;
