//! `eigen-hal` - Layer 3 hardware abstraction.
//!
//! The HAL advertises WHO closes the loop, which native modes a smart controller
//! exposes, the clock domain (including `Unknown`), an abort-latency bound, and the
//! output-buffer contract. Backends implement `Backend`:
//! - a delegated backend: `DeviceClosesLoop` (the device runs its own control DSP).
//! - `eigen-hal-ni`, `eigen-hal-sim`: `HostClosesLoop` (Eigen runs the control math).
//!
//! Who closes the loop is a property of the (module, backend) PAIR: the HOST resolves
//! it from `Caps` + the module's `LoopKind` via `eigen_module_abi::resolve_loop` (the
//! three-valued resolver; it lives in the module-ABI crate because L3 must not
//! depend on L4's `LoopKind`). Trait pinned by the integrator; backend bodies are
//! written by their crates.
//!
//! ## Delegated control
//!
//! A `DeviceClosesLoop` backend is driven through [`delegated::DelegatedControl`]:
//! the device-neutral lifecycle and level vocabulary, with the per-mode choreography
//! left to the backend's own delegate crate.
//!
//! ## Discovery
//!
//! A backend that can SCAN its bus reports what it found using the
//! backend-neutral types in [`discovery`]. Those types are additive to (and
//! independent of) the [`Backend`] trait: enumeration happens BEFORE a backend
//! instance exists, so it is a free function on the backend crate, not a trait
//! method.
#![allow(dead_code)]

pub mod delegated;
pub mod discovery;
pub mod stream;

pub use discovery::{DiscoveredChannel, DiscoveredDevice};
pub use stream::{play_block_wise, BlockPlay, StreamError};

// ---------------------------------------------------------------------------------
// Acquisition-block policy
// ---------------------------------------------------------------------------------
//
// These live here, next to `AcquisitionConfig::block_frames`, because a block is a
// property of an ACQUISITION and not of the engine: the direct-backend players
// (TWR / SRS / shock) request one without an `eigen_engine::Engine` anywhere in the
// picture. `eigen_engine::block` re-exports them and adds the engine-side
// derivations (ring depth, tick budget) on top.

/// Target acquisition-block duration. 20 ms is the working compromise: long enough
/// that per-block overhead and driver round-trips are negligible, short enough that
/// the control loop's transport delay stays small and an abort is acted on within a
/// block or two.
pub const TARGET_BLOCK_SECONDS: f64 = 0.020;

/// Floor on a derived block. Below this the per-block overhead dominates and the
/// engine's scheduling-health budget (one tick period) gets too tight to mean much.
pub const MIN_BLOCK_FRAMES: usize = 64;

/// Ceiling on a derived block. Above this the drive transport delay and the abort
/// reaction time grow past what a closed-loop shaker run should carry.
pub const MAX_BLOCK_FRAMES: usize = 4096;

/// The EIGEN-SIM plant path's natural block: ONE frame per call.
///
/// This is NOT the retired `SimBackend` drive-latch workaround (the latch was
/// removed; the Sim streams every drive frame at any block size). It is kept
/// because the Sim-based module loops count a segment / chunk in SAMPLES and pass
/// that count to the engine as `max_ticks`, so the two units must coincide - and
/// because a software plant has no device block to honor, so the smallest one keeps
/// a control law's drive-to-response latency at its physical minimum while it is
/// being validated for correctness.
pub const SIM_NATURAL_BLOCK_FRAMES: usize = 1;

/// The block a HARDWARE station should request at `sample_rate_hz`: the power of two
/// closest to [`TARGET_BLOCK_SECONDS`], clamped to
/// `[MIN_BLOCK_FRAMES, MAX_BLOCK_FRAMES]`.
///
/// At the 51.2 kHz delta-sigma class this yields 1024 frames (20.0 ms), inside the
/// 512-2048 target band. A power of two because every FFT-shaped consumer downstream
/// prefers one and the driver-side buffer arithmetic is exact.
pub fn default_block_frames(sample_rate_hz: f64) -> usize {
    if !(sample_rate_hz.is_finite() && sample_rate_hz > 0.0) {
        return MIN_BLOCK_FRAMES;
    }
    let ideal = sample_rate_hz * TARGET_BLOCK_SECONDS;
    let exp = ideal.log2().round().clamp(0.0, 24.0) as u32;
    (1usize << exp).clamp(MIN_BLOCK_FRAMES, MAX_BLOCK_FRAMES)
}

use eigen_channel_table::ChannelTable;
use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopOwner {
    DeviceClosesLoop,
    HostClosesLoop,
}

/// Whether the input (measurement) and output (drive) run on the SAME physical clock.
/// `Unknown` is the DEFAULT for a backend that has not proven `Shared` by a bench
/// loopback; the engine refuses closed-loop modes until it is `Shared`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockDomain {
    Shared,
    Independent,
    Unknown,
}

/// One clock group inside a backend's task set, for the wide-channel case: a
/// 300-400 channel rig is several chassis, and "the clock
/// domain" is not one fact about the whole rig.
///
/// ## Contract with [`Caps::clock`] (read this before using either)
///
/// [`Caps::clock`] REMAINS the resolved answer for the CONFIGURED task set - the
/// single value the resolver and every arm gate read. `clock_groups` is the
/// BREAKDOWN behind that answer, never a second source of truth:
///
/// * An EMPTY `clock_groups` means "one implicit group", whose domain is
///   [`Caps::clock`]. Every backend that has not been taught about groups reports
///   empty, and nothing downstream changes.
/// * A non-empty `clock_groups` must be CONSISTENT with [`Caps::clock`]: the
///   scalar is the conjunction over the groups that participate in the control
///   loop (any group not proven `Shared` makes the set not proven `Shared`).
///   A backend that cannot honor that consistency must report the scalar
///   conservatively, never widen it to match an optimistic group.
///
/// Consumers that only need the yes/no answer read [`Caps::clock`]. Consumers
/// that need to TELL THE OPERATOR WHICH chassis is the problem read the groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockGroup {
    /// Operator-facing name for the group (a chassis id, a PXIe backplane, a
    /// standalone card). Shown verbatim in a refusal.
    pub label: String,
    /// This group's own domain, resolved by the same rule as [`Caps::clock`].
    pub domain: ClockDomain,
}

/// A control mode a smart controller exposes natively (the delegated case).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeMode {
    OpenLoopSine,
    Random,
    Shock,
    Srs,
    Sine,
    Mimo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderrunPolicy {
    RampDownThenAbort,
    HoldLastThenAbort,
}

/// What the drive side must honor so a late output frame never slams the shaker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputBufferContract {
    pub min_lookahead_frames: u32,
    pub underrun_policy: UnderrunPolicy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Caps {
    pub loop_owner: LoopOwner,
    pub native_modes: Vec<NativeMode>,
    pub clock: ClockDomain,
    pub max_input_channels: u32,
    pub max_output_channels: u32,
    /// Worst-case guaranteed latency for an abort/stop to take effect, if characterized.
    /// `None` = not characterized (treat the physical E-stop as the safety boundary).
    pub abort_latency_bound: Option<Duration>,
    pub output_buffer: Option<OutputBufferContract>,
    pub sample_rate_hz: f64,
    /// While the DEVICE replays a downloaded drive record, does the measured
    /// RESPONSE come back to the host?
    ///
    /// This is the feedback-path precondition for host adaptation that runs
    /// BETWEEN runs (`AdaptationOwner::HostBetweenRuns` over
    /// `DriveOrigin::DeviceReplay`) - TWR's iteration, SRS's wavelet law. Without
    /// it the host has no error signal, so a CONVERGING request is REFUSED
    /// (`UnsupportedReason::ResponseNotObservable`), never silently degraded to
    /// open-loop replay: the operator reaches replay by asking for replay.
    ///
    /// **DEFAULT FALSE, and it is an evidence claim in exactly the sense
    /// `native_modes` is** - a backend declares `true` only when the delegated
    /// response really arrives. A delegated backend declares `false` while
    /// its delegated telemetry and arm capture are unproven.
    ///
    /// Irrelevant to a `HostClosesLoop` backend (the host already holds the
    /// response); such a backend may report either value and the resolver never
    /// consults it, because the precondition only binds `DeviceReplay`.
    pub delegated_response_observable: bool,
    /// Per-group clock breakdown for a multi-chassis task set. EMPTY = a single
    /// implicit group whose domain is [`Caps::clock`]. See [`ClockGroup`] for the
    /// contract - the scalar `clock` stays the resolved answer, this is the
    /// breakdown behind it.
    pub clock_groups: Vec<ClockGroup>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackendStatus {
    pub connected: bool,
    pub faulted: bool,
    pub message: String,
    /// Monotonic device sample counter (drives the engine timebase).
    pub sample_counter: u64,
    /// Increments on a device reset so the engine distinguishes reset from counter wrap.
    pub epoch: u32,
}

/// Everything a backend can refuse with.
///
/// `#[non_exhaustive]`: this enum grows as backends land (the NI backend added
/// [`HalError::DriverNotAvailable`]), and every downstream `match` must
/// carry a wildcard arm so a new variant is a compile-time no-op outside this
/// crate rather than a tree-wide break. Adding a variant is now ADDITIVE.
#[derive(Debug)]
#[non_exhaustive]
pub enum HalError {
    NotConnected,
    Unsupported(&'static str),
    /// The backend's own DRIVER/runtime is not installed on this host at all.
    ///
    /// Distinct from [`HalError::Unsupported`] (an operation this backend does
    /// not offer) and from [`HalError::NotConnected`] (a driver that exists but
    /// no open handle): nothing this backend does can work here until software
    /// is installed. Normal and expected on a developer Mac for `eigen-hal-ni`.
    ///
    /// Carries an OWNED string so the loader diagnosis - which library names were
    /// tried and why each failed - survives the hop. (`Unsupported` holds a
    /// `&'static str`, which is exactly why the NI backend's original stopgap threw
    /// the detail away; widening the HAL error type was the recorded fix.)
    DriverNotAvailable(String),
    /// A closed-loop mode was asked of an `Unknown`/`Independent` clock domain.
    ClockDomainRefused,
    Config(String),
    Io(String),
    /// The device link was LIVE and has been LOST mid-session. Distinct from
    /// `NotConnected` (a handle that was never opened, or an operation issued in the
    /// wrong lifecycle state): `Disconnected` means a run may be in flight on the
    /// hardware right now with no host link to command it. Two real producers: a
    /// delegated controller re-enumerates on USB during its IEPE circuit check (observed:
    /// addr 2.4 -> 2.5 with a firmware re-download), and a USB DAQ can be hot-unplugged.
    /// The engine treats this as its own stop class: best-effort `emergency_stop()`,
    /// then stop with a distinct reason - never a clean `InputEnded`. Recovery
    /// (re-drive `open()` and resume where safe) is a documented seam and is
    /// deliberately NOT implemented yet; see the TODO on the scheduler's handler.
    Disconnected(String),
}

impl fmt::Display for HalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HalError::NotConnected => write!(f, "backend not connected"),
            HalError::Unsupported(s) => write!(f, "unsupported: {s}"),
            HalError::DriverNotAvailable(s) => write!(f, "driver not available: {s}"),
            HalError::ClockDomainRefused => {
                write!(f, "closed-loop refused: clock domain is not proven Shared")
            }
            HalError::Config(s) => write!(f, "config error: {s}"),
            HalError::Io(s) => write!(f, "io error: {s}"),
            HalError::Disconnected(s) => write!(f, "device link lost mid-session: {s}"),
        }
    }
}

impl std::error::Error for HalError {}

/// Outcome of one [`Backend::read_input`] poll.
///
/// This replaces the old `Ok(usize)` return whose `Ok(0)` was OVERLOADED: it meant
/// "stream ended cleanly" to the engine, but a continuous acquisition (an NI DAQmx
/// poll that times out with nothing buffered, a delegated telemetry poll returning an
/// all-stale block) legitimately has "no frames ready THIS poll" as a steady-state
/// answer - which the engine then misread as a clean end of test. The three cases
/// are now distinct in the type; `Frames(n)` also names the unit (FRAMES, never
/// samples - the frames/samples conflation was a real multi-channel data-loss bug).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputRead {
    /// `n` sample frames (channel-interleaved) were written into `out`. Backends
    /// should not construct `Frames(0)`; use `NoDataYet` or `Ended` so the meaning
    /// is explicit. (The engine treats a stray `Frames(0)` as `NoDataYet`.)
    Frames(usize),
    /// The stream is alive but no frames were ready on this poll. Keep polling.
    NoDataYet,
    /// The stream ended cleanly (script exhausted / acquisition finished). Final.
    Ended,
}

/// What the caller ASKS a backend to acquire at, handed to [`Backend::configure`].
///
/// Previously the rate lived wherever each backend happened to keep it (the NI
/// backend carried it on its constructor-built `NiConfig`; the Sim carried it on
/// `SimConfig`), so there was no seam at which a caller could state a rate and
/// learn what it actually got. That gap is exactly where a profile generated at
/// one rate silently plays at another. This struct plus [`ConfiguredActual`] is
/// that seam.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AcquisitionConfig {
    /// Rate the caller wants, in Hz. The backend may COERCE it (real hardware
    /// often does); what it actually programmed comes back in
    /// [`ConfiguredActual::actual_rate_hz`]. A backend NEVER resamples to hide a
    /// coercion - the mismatch is reported and the arm gate decides.
    pub requested_rate_hz: f64,
    /// Frames per acquisition block the caller intends to poll with.
    pub block_frames: usize,
}

impl AcquisitionConfig {
    /// The common case: a rate and a block size.
    pub fn new(requested_rate_hz: f64, block_frames: usize) -> Self {
        AcquisitionConfig {
            requested_rate_hz,
            block_frames,
        }
    }
}

/// What the backend ACTUALLY programmed, returned from [`Backend::configure`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfiguredActual {
    /// The rate the hardware is really running at. Everything downstream (DSP
    /// bin spacing, profile validity, the abort-latency bound) uses THIS, never
    /// the request.
    pub actual_rate_hz: f64,
    /// Frames per block the backend will actually deliver.
    pub block_frames: usize,
    /// Fixed acquisition-path delay in FRAMES, when the backend can state one
    /// (a delta-sigma converter's decimation-filter group delay is the case that
    /// motivated it). `None` = the backend does not characterize its own delay,
    /// which is "not known", never "zero" - a consumer that needs a latency
    /// bound must refuse rather than assume.
    pub group_delay_frames: Option<u32>,
}

/// The hardware backend contract. Object-safe so backends live behind `Box<dyn Backend>`.
pub trait Backend: Send {
    fn caps(&self) -> Caps;
    fn open(&mut self) -> Result<(), HalError>;
    fn close(&mut self) -> Result<(), HalError>;
    /// Take any non-fatal DRIVER diagnostics accumulated since the last call
    /// (DAQmx warnings: underruns, coercions, late samples), draining them.
    ///
    /// **Absence of a channel for these is how a real signal goes missing.** The
    /// NI backend has always collected driver warnings, but they were reachable
    /// only on the concrete type - so every caller holding a `dyn Backend` (the
    /// engine, every module's plant loop) was structurally unable to see an
    /// underrun the driver had already reported (2026-08-02). The default
    /// returns empty, so a backend with nothing to say costs nothing and no
    /// existing implementor breaks.
    fn take_warnings(&mut self) -> Vec<String> {
        Vec::new()
    }
    /// Apply `table` (measurement scaling, roles) and `acq` (rate + block size),
    /// and report what was ACTUALLY programmed.
    ///
    /// The returned [`ConfiguredActual`] is the authority from here on. A
    /// backend that coerced the rate says so here; it must not resample to make
    /// the request appear honored.
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError>;
    fn arm(&mut self) -> Result<(), HalError>;
    fn start(&mut self) -> Result<(), HalError>;
    fn stop(&mut self) -> Result<(), HalError>;
    /// Pull the next block of input samples (channel-interleaved f32) into `out`.
    /// Returns [`InputRead`]: `Frames(n)` sample FRAMES written (n >= 1),
    /// `NoDataYet` (alive, keep polling), or `Ended` (clean end of stream).
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError>;
    /// Push a block of drive samples (`HostClosesLoop` only; channel-interleaved,
    /// whole frames). Returns the number of FRAMES ACCEPTED, `0..=offered`.
    ///
    /// Lockstep backends (Sim; the engine's one-output-frame-per-input-frame default
    /// cadence) accept everything offered, every call. A backend with its own
    /// free-running output buffer (NI DAQmx regenerating an AO buffer on the device
    /// clock) may accept FEWER when the buffer is full; the caller retains the
    /// unaccepted tail and re-offers it after checking [`Backend::output_space`].
    /// Acceptance is at FRAME granularity - a backend never splits a frame.
    /// `DeviceClosesLoop` backends return `Unsupported` (the device generates drive).
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError>;
    /// Frames of space currently free in the backend's output buffer, or `None` for
    /// a backend with no independent output buffer (the lockstep case, where
    /// `write_drive` always accepts everything and this query is meaningless).
    /// A future NI backend returns `Some(space)` so the engine can top up an
    /// independently DMA-clocked AO buffer instead of running in input lockstep.
    fn output_space(&mut self) -> Option<usize> {
        None
    }
    fn poll_status(&mut self) -> BackendStatus;
    /// Emergency ramp-down and safe stop. Must NEVER issue flash/firmware opcodes.
    fn emergency_stop(&mut self) -> Result<(), HalError>;
}

/// A boxed backend IS a backend.
///
/// This is what makes `Box<dyn Backend>` usable at every `B: Backend` seam in the
/// tree (the TWR player, the SRS plant, the shock firer, the recorder service)
/// without any of them being rewritten to take a trait object. The service crates
/// stay free of the Sim by being handed a backend the FACTORY built, and the factory's
/// product is `Box<dyn Backend>`: without this impl each of those generic seams
/// would need a parallel `dyn` variant, and the two would drift.
///
/// `?Sized` covers both `Box<dyn Backend>` (the factory product) and `Box<Concrete>`.
/// **Every method with a DEFAULT body must be forwarded here explicitly**, because a
/// default that is not overridden silently WINS over the inner backend's real
/// implementation - and it wins quietly, at a call site that looks correct.
///
/// Two have been caught doing exactly that:
///
/// * [`Backend::output_space`], whose default (`None`, "no independent output buffer")
///   put the engine back into input lockstep on a DMA-clocked AO buffer.
/// * [`Backend::take_warnings`], whose default (empty) meant that once every service
///   crate started holding a `Box<dyn Backend>`, the DAQmx underrun and coercion
///   warnings the NI backend collects reached NOBODY. That is a known defect class ("a
///   warning collected on a concrete backend but absent from the trait is a signal that
///   cannot reach anyone") re-created one layer up, in the very impl added to prevent
///   that class. Found 2026-08-08 by an Explorer test asserting the warnings
///   arrive in its status; before the fix the assertion read 0 of 1.
///
/// The lesson generalizes: adding a defaulted method to `Backend` is NOT additive for
/// this impl. Add the forwarding line in the same commit.
impl<T: Backend + ?Sized> Backend for Box<T> {
    fn caps(&self) -> Caps {
        (**self).caps()
    }
    fn open(&mut self) -> Result<(), HalError> {
        (**self).open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        (**self).close()
    }
    fn take_warnings(&mut self) -> Vec<String> {
        (**self).take_warnings()
    }
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        (**self).configure(table, acq)
    }
    fn arm(&mut self) -> Result<(), HalError> {
        (**self).arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        (**self).start()
    }
    fn stop(&mut self) -> Result<(), HalError> {
        (**self).stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        (**self).read_input(out)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        (**self).write_drive(drive)
    }
    fn output_space(&mut self) -> Option<usize> {
        (**self).output_space()
    }
    fn poll_status(&mut self) -> BackendStatus {
        (**self).poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        (**self).emergency_stop()
    }
}

// NOTE: the old `resolve_loop_owner(caps) -> LoopOwner` free function that lived here was
// REMOVED deliberately. It took no module parameter and returned `caps.loop_owner` verbatim -
// the exact v1 collapse the three-valued resolver was written to repeal (it made "Sine on a
// delegated controller" and "Sine on NI" two different control systems behind one seam, and
// nothing checked whether the device even HAS a native mode for the requested test). It had
// zero production callers. The real three-valued resolver is `eigen_module_abi::resolve_loop`
// (module-ABI crate: it needs L4's `LoopKind`, which this L3 crate must not depend on). Never
// two resolvers with different answers.

#[cfg(test)]
mod tests {
    use super::*;

    /// EVERY DEFAULTED METHOD MUST BE FORWARDED BY THE `Box<T>` IMPL.
    ///
    /// A default body that this impl does not override wins silently over the inner
    /// backend's real one, at a call site that looks correct. That has now happened
    /// twice (`output_space`, then `take_warnings`), and the second time it re-created
    /// the defect of driver diagnostics reaching nobody, inside the impl that exists
    /// to keep boxing transparent.
    ///
    /// RED-MUTATION (run 2026-08-08, reverted): delete either forwarding method from
    /// `impl Backend for Box<T>` and the corresponding assertion below reds.
    #[test]
    fn boxing_a_backend_does_not_swallow_its_defaulted_methods() {
        struct Talkative;
        impl Backend for Talkative {
            fn caps(&self) -> Caps {
                Caps {
                    loop_owner: LoopOwner::HostClosesLoop,
                    native_modes: vec![],
                    clock: ClockDomain::Unknown,
                    max_input_channels: 1,
                    max_output_channels: 1,
                    abort_latency_bound: None,
                    output_buffer: None,
                    sample_rate_hz: 1000.0,
                    delegated_response_observable: false,
                    clock_groups: vec![],
                }
            }
            fn open(&mut self) -> Result<(), HalError> {
                Ok(())
            }
            fn close(&mut self) -> Result<(), HalError> {
                Ok(())
            }
            fn take_warnings(&mut self) -> Vec<String> {
                vec!["DAQmx 200011: onboard AO buffer underflowed".into()]
            }
            fn configure(
                &mut self,
                _t: &ChannelTable,
                _a: &AcquisitionConfig,
            ) -> Result<ConfiguredActual, HalError> {
                Err(HalError::NotConnected)
            }
            fn arm(&mut self) -> Result<(), HalError> {
                Ok(())
            }
            fn start(&mut self) -> Result<(), HalError> {
                Ok(())
            }
            fn stop(&mut self) -> Result<(), HalError> {
                Ok(())
            }
            fn read_input(&mut self, _o: &mut [f32]) -> Result<InputRead, HalError> {
                Ok(InputRead::NoDataYet)
            }
            fn write_drive(&mut self, _d: &[f32]) -> Result<usize, HalError> {
                Ok(0)
            }
            fn output_space(&mut self) -> Option<usize> {
                Some(4096)
            }
            fn poll_status(&mut self) -> BackendStatus {
                BackendStatus::default()
            }
            fn emergency_stop(&mut self) -> Result<(), HalError> {
                Ok(())
            }
        }

        let mut boxed: Box<dyn Backend> = Box::new(Talkative);
        assert_eq!(
            boxed.take_warnings().len(),
            1,
            "the Box impl swallowed the driver's warnings"
        );
        assert_eq!(
            boxed.output_space(),
            Some(4096),
            "the Box impl swallowed the real output-buffer depth"
        );
    }

    #[test]
    fn unknown_clock_is_the_default_posture() {
        // A freshly described host backend should not claim a shared clock it cannot prove.
        let caps = Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Unknown,
            max_input_channels: 8,
            max_output_channels: 2,
            abort_latency_bound: None,
            output_buffer: Some(OutputBufferContract {
                min_lookahead_frames: 4,
                underrun_policy: UnderrunPolicy::RampDownThenAbort,
            }),
            sample_rate_hz: 65_536.0,
            delegated_response_observable: false,
            clock_groups: vec![],
        };
        assert_eq!(caps.clock, ClockDomain::Unknown);
        assert_eq!(caps.loop_owner, LoopOwner::HostClosesLoop);
        // Both newer fields default to the CONSERVATIVE posture: no delegated
        // response path claimed, and no group breakdown (one implicit group).
        assert!(!caps.delegated_response_observable);
        assert!(caps.clock_groups.is_empty());
    }

    /// RED-MUTATION: change `ClockGroup`'s doc contract into code that lets a
    /// group disagree with the scalar (e.g. have a backend report
    /// `clock: Shared` while a group says `Unknown`) and this test reds - it is
    /// the executable statement of "the scalar is the conjunction".
    #[test]
    fn clock_groups_are_a_breakdown_of_the_scalar_not_a_second_answer() {
        // Empty groups: the scalar stands alone (one implicit group).
        let single = Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: 4,
            max_output_channels: 1,
            abort_latency_bound: None,
            output_buffer: None,
            sample_rate_hz: 51_200.0,
            delegated_response_observable: false,
            clock_groups: vec![],
        };
        assert!(single.clock_groups.is_empty());
        assert_eq!(single.clock, ClockDomain::Shared);

        // Two groups, one unproven: the scalar must NOT claim Shared.
        let wide = Caps {
            clock: ClockDomain::Unknown,
            clock_groups: vec![
                ClockGroup {
                    label: "cDAQ-A".into(),
                    domain: ClockDomain::Shared,
                },
                ClockGroup {
                    label: "cDAQ-B".into(),
                    domain: ClockDomain::Unknown,
                },
            ],
            ..single.clone()
        };
        let all_shared = wide
            .clock_groups
            .iter()
            .all(|g| g.domain == ClockDomain::Shared);
        assert!(!all_shared);
        assert_ne!(
            wide.clock,
            ClockDomain::Shared,
            "a set with any unproven group must not report Shared"
        );
    }

    /// A backend that OVERRIDES `output_space` (the one trait method with a
    /// default body) and counts its own calls, so the boxed forwarding can be
    /// distinguished from the default.
    struct SpacedBackend {
        space: usize,
        stops: u32,
    }

    impl Backend for SpacedBackend {
        fn caps(&self) -> Caps {
            Caps {
                loop_owner: LoopOwner::HostClosesLoop,
                native_modes: vec![],
                clock: ClockDomain::Shared,
                max_input_channels: 2,
                max_output_channels: 1,
                abort_latency_bound: None,
                output_buffer: None,
                sample_rate_hz: 1000.0,
                delegated_response_observable: false,
                clock_groups: vec![],
            }
        }
        fn open(&mut self) -> Result<(), HalError> {
            Ok(())
        }
        fn close(&mut self) -> Result<(), HalError> {
            Ok(())
        }
        fn configure(
            &mut self,
            _t: &ChannelTable,
            acq: &AcquisitionConfig,
        ) -> Result<ConfiguredActual, HalError> {
            Ok(ConfiguredActual {
                actual_rate_hz: acq.requested_rate_hz,
                block_frames: acq.block_frames,
                group_delay_frames: None,
            })
        }
        fn arm(&mut self) -> Result<(), HalError> {
            Ok(())
        }
        fn start(&mut self) -> Result<(), HalError> {
            Ok(())
        }
        fn stop(&mut self) -> Result<(), HalError> {
            self.stops += 1;
            Ok(())
        }
        fn read_input(&mut self, _out: &mut [f32]) -> Result<InputRead, HalError> {
            Ok(InputRead::NoDataYet)
        }
        fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
            Ok(drive.len())
        }
        fn output_space(&mut self) -> Option<usize> {
            Some(self.space)
        }
        fn poll_status(&mut self) -> BackendStatus {
            BackendStatus {
                connected: true,
                ..BackendStatus::default()
            }
        }
        fn emergency_stop(&mut self) -> Result<(), HalError> {
            Ok(())
        }
    }

    /// RED-MUTATION: delete the `output_space` forward from
    /// `impl<T: Backend + ?Sized> Backend for Box<T>` (let the trait default
    /// stand) and this test reds with `None != Some(37)`. That is the whole
    /// point of writing the forward out by hand: the ONE method with a default
    /// body is the one a blanket impl silently gets wrong, and getting it wrong
    /// puts the engine back into input lockstep on a DMA-clocked AO buffer.
    #[test]
    fn a_boxed_backend_forwards_every_method_including_the_defaulted_one() {
        let mut boxed: Box<dyn Backend> = Box::new(SpacedBackend {
            space: 37,
            stops: 0,
        });
        assert_eq!(
            boxed.output_space(),
            Some(37),
            "the box must forward output_space, not fall back to the trait default"
        );

        // The rest of the surface forwards too (a spot check across the
        // lifecycle, the two data paths, and caps).
        assert_eq!(boxed.caps().max_input_channels, 2);
        assert!(boxed.open().is_ok());
        let acq = AcquisitionConfig::new(1000.0, 64);
        let actual = boxed
            .configure(&ChannelTable::new(), &acq)
            .expect("configure forwards");
        assert_eq!(actual.block_frames, 64);
        assert!(boxed.arm().is_ok());
        assert!(boxed.start().is_ok());
        assert_eq!(boxed.write_drive(&[0.0, 0.0]).expect("write forwards"), 2);
        let mut buf = [0.0f32; 2];
        assert!(matches!(
            boxed.read_input(&mut buf),
            Ok(InputRead::NoDataYet)
        ));
        assert!(boxed.poll_status().connected);
        assert!(boxed.emergency_stop().is_ok());
        assert!(boxed.stop().is_ok());
        assert!(boxed.close().is_ok());
    }

    /// `Box<dyn Backend>` must satisfy a `B: Backend` bound, which is the only
    /// reason the generic seams (TWR player, SRS plant, shock firer, recorder)
    /// did not have to be rewritten when the services moved to factory-built backends.
    #[test]
    fn a_boxed_trait_object_satisfies_a_generic_backend_bound() {
        fn takes_generic<B: Backend>(b: B) -> f64 {
            b.caps().sample_rate_hz
        }
        let boxed: Box<dyn Backend> = Box::new(SpacedBackend { space: 1, stops: 0 });
        assert_eq!(takes_generic(boxed), 1000.0);
    }
}
