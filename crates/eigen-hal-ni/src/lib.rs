//! `eigen-hal-ni` - the NI-DAQmx backend (Layer 3).
//!
//! A `HostClosesLoop` backend: a DAQ card measures and generates, Eigen runs the
//! control math. One AI task for every enabled input row, one AO task for every
//! drive row, both riding the same sample clock when a chassis route exists.
//!
//! ## What this crate refuses to do
//!
//! Three refusals are the point of the design, not limitations of it:
//!
//! 1. **It never turns an absent driver into data.** No NI-DAQmx runtime means
//!    [`NiError::DriverNotAvailable`], surfaced honestly all the way to the UI.
//!    There is no fallback, no synthesized signal, no "demo mode".
//! 2. **It never claims a shared clock it has not been shown.** See [`clock`]:
//!    driver-SIMULATED device sets are `Shared` (there is no physical clock to
//!    drift), real hardware is `Unknown` until a bench loopback capture adds its
//!    exact configuration to a proven set that ships EMPTY.
//! 3. **It never regenerates the output buffer.** A non-regenerating AO that
//!    runs dry is an underrun the engine must handle; a regenerating one would
//!    loop stale drive into a shaker while every plot stayed alive.
//!
//! ## Layout
//!
//! | module | what lives there |
//! |---|---|
//! | [`ffi`] | the runtime binding, the status discipline, the constants |
//! | [`error`] | [`NiError`] and its mapping onto `eigen_hal::HalError` |
//! | [`enumerate`] | device scan -> `eigen_hal::discovery` types |
//! | [`config`] | channel table + physical channels -> a task plan |
//! | [`clock`] | the two-tier clock rule and the proven set |
//! | [`timing`] | timing families: which devices can share one DAQmx task |
//! | [`backend`] | [`NiBackend`], the `eigen_hal::Backend` implementation |
//! | `timer_resolution` | Windows only: the 1 ms timer hold the host loop needs there |
//!
//! ## Where the driver is actually required
//!
//! Everything except the true FFI boundary is unit-tested on a host with no NI
//! driver at all: the status split, the config mapping and its scaling math, the
//! enumeration parsing and its degradation paths, the clock rule, and the
//! backend's caps/lifecycle/counters. What needs real DAQmx is the `examples/`
//! selftest, meant to be run on the Linux node:
//!
//! ```text
//! cargo run -p eigen-hal-ni --example ni_selftest -- --help
//! ```
//!
//! ## Threading
//!
//! A task handle is owned by exactly one thread. [`NiBackend`] is `Send` and
//! every method takes `&mut self`, so the run thread owns it outright;
//! [`enumerate()`] opens its own short-lived driver handle and touches no task,
//! so a scan is safe while a backend exists.

#![allow(dead_code)]

pub mod backend;
pub mod clock;
pub mod config;
pub mod enumerate;
pub mod error;
pub mod ffi;
pub mod timing;
// Windows only (and tests, which prove its pairing rule on every host): the 1 ms
// system timer the backend holds while its tasks are armed or running.
#[cfg(any(windows, test))]
mod timer_resolution;

pub use backend::{DeviceIdentity, NiBackend};
pub use clock::{resolve_clock, ClockKey, ProvenClockConfig, PROVEN_SHARED_CLOCK};
pub use config::{plan_from_table, plan_from_table_on, AiAssign, AiInterface, AoAssign, NiConfig};
pub use enumerate::{enumerate, enumerate_with, SysQuery};
pub use error::NiError;
pub use timing::{FamilyKind, TimingFamily, TimingFamilyMap};
