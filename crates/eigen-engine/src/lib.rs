//! `eigen-engine` - Layer 2: the real-time engine (skeleton).
//!
//! The engine drives a `dyn eigen_hal::Backend`, enforces safety, and streams to disk.
//! Correctness of the SAFETY behavior is the priority. The design seams are built here;
//! depth (real HDF5 capture, the control-DSP modules, an RT-thread executor) follows
//! behind them.
//!
//! # Module layout
//!
//! * [`block`] - the BLOCK-SIZE POLICY: the one place that turns a station's sample rate
//!   into an acquisition block, and a block into a raw-ring depth.
//!   `EngineConfig::with_block` is the seam every caller builds through.
//! * [`ring`] - lock-free single-producer/single-consumer ring buffer for sample
//!   frames, pure `std` (atomics + a fixed preallocated `Vec`). The raw-input tap the
//!   watchdog reads.
//! * [`timebase`] - the device-counter [`timebase::Timebase`] with EPOCH + GAP handling:
//!   distinguishes a device reset (epoch bump) from a counter wrap from dropped samples
//!   (a counter gap), and converts the counter to seconds.
//! * [`limits`] - the shared tri-field limit checker: the accel-direct-compare +
//!   HP-then-integrate velocity/displacement streaming check ([`limits::ChannelLimiter`]),
//!   used BOTH by [`watchdog::AbortWatchdog`] internally and by the delegated SMART
//!   run loop, so the limit math lives in exactly one place.
//! * [`watchdog`] - the [`watchdog::AbortWatchdog`]: reads the RAW ring (never a
//!   decimated view) and trips when any channel exceeds its tri-field
//!   [`eigen_channel_table::AbortLimits`] (via [`limits`]), calling
//!   `backend.emergency_stop()`. The per-limit-type latency budget
//!   ([`watchdog::LatencyBudget`]) is explicit.
//! * [`output`] - [`output::OutputDelivery`]: a lookahead buffer honoring the
//!   [`eigen_hal::OutputBufferContract`]; below `min_lookahead_frames` it applies the
//!   [`eigen_hal::UnderrunPolicy`] (RampDownThenAbort ramps to zero then aborts). An
//!   actual underrun is an abort event.
//! * [`health`] - [`health::SchedulingHealth`]: detects the processing step overrunning
//!   its per-tick compute budget and period jitter (starved scheduling).
//! * [`capture`] - the [`capture::CaptureSink`] seam plus [`capture::VecCaptureSink`]
//!   for tests. The real `eigen-capture` HDF5/SWMR wiring is a documented TODO.
//! * [`scheduler`] - the poll/pull [`scheduler::Engine`]: pulls input blocks from the
//!   backend, runs the watchdog on the raw ring, hands blocks to a
//!   [`scheduler::ProcessStep`], and pushes drive frames back through the output
//!   contract, all driven by the device sample counter.
//!
//! # Safety behaviors and where they are tested
//!
//! | Behavior | Where | Test |
//! |---|---|---|
//! | Over-limit -> watchdog trips + `emergency_stop` | [`watchdog`] | `watchdog::tests`, `tests/engine.rs` |
//! | Counter reset -> epoch bump detected | [`timebase`] | `timebase::tests`, `tests/engine.rs` |
//! | Counter gap -> dropped samples detected | [`timebase`] | `timebase::tests`, `tests/engine.rs` |
//! | Drive underrun -> UnderrunPolicy fires (ramp + abort) | [`output`] | `output::tests`, `tests/engine.rs` |
//! | Compute-budget overrun flagged | [`health`] | `health::tests` |
//! | Frames flow backend->ring->process->CaptureSink, drive back | [`scheduler`] | `tests/engine.rs` |

#![allow(dead_code)]

pub mod block;
pub mod capture;
pub mod health;
pub mod limits;
pub mod output;
pub mod ring;
pub mod scheduler;
pub mod timebase;
pub mod watchdog;

/// In-crate hardware-free MOCK backend. Compiled only for the crate's own unit tests.
/// The `tests/engine.rs` integration test (a separate crate that cannot see a
/// `#[cfg(test)]` module) includes the SAME source file directly via
/// `#[path = "../src/test_backend.rs"]`, so there is one mock definition shared by both.
#[cfg(test)]
pub(crate) mod test_backend;

// Re-export the primary public surface at the crate root.
pub use block::{
    default_block_frames, ring_frames_for, watchdog_sustain_frames, MAX_WATCHDOG_SUSTAIN_SECONDS,
    SIM_NATURAL_BLOCK_FRAMES, TEST_DISABLE_JITTER_GUARD, TEST_RELAX_COMPUTE_BUDGET,
};
pub use capture::{BlockMeta, CaptureError, CaptureSink, VecCaptureSink};
pub use health::{HealthBudget, HealthStatus, SchedulingHealth};
pub use limits::{ChannelIntegrator, ChannelLimiter};
pub use output::{DeliveryOutcome, OutputDelivery, UnderrunAbort};
pub use scheduler::{
    Engine, EngineConfig, EngineConfigError, EngineHealth, ProcessStep, RunReport, SilentDrive,
    StopReason, IEPE_SETTLE_PLACEHOLDER, MAX_BLANKING, STALL_DEADLINE_BLOCKS,
};
pub use timebase::{Timebase, TimebaseEvent};
// `AbortAxis` / `AbortTrip` are the shared tri-field limit types defined in
// [`limits`] and re-exported through [`watchdog`]; the crate-root path is unchanged.
pub use watchdog::{AbortAxis, AbortTrip, AbortWatchdog, LatencyBudget};
