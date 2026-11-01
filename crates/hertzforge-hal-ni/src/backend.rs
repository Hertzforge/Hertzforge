//! [`NiBackend`]: `hertzforge_hal::Backend` over NI-DAQmx.
//!
//! ## Lifecycle mapping
//!
//! DAQmx has no "arm" step, so the trait's five lifecycle calls land like this:
//!
//! | trait call | DAQmx work |
//! |---|---|
//! | `open()` | load the runtime, read device identity, BUILD the tasks from the stored [`NiConfig`] |
//! | `configure(table, acq)` | re-derive the assignments from the channel table AND adopt the rate/block request, then rebuild the tasks; reports the ACTUAL rate back |
//! | `arm()` | state gate + optional `DAQmxTaskControl(verify)`; no data moves |
//! | `start()` | start AO first (it waits on the AI clock), then AI |
//! | `stop()` | stop AO first (drive off), then AI |
//!
//! `configure()` is OPTIONAL: a caller that already holds a table-derived
//! [`NiConfig`] plus an [`hertzforge_hal::AcquisitionConfig`] (the selftest, and
//! anything that ran [`crate::plan_from_table`]) can go straight from `open()`
//! to `arm()`. Calling `configure()` afterwards tears the tasks down and
//! rebuilds them, so the channel table stays the authority on sensitivity and
//! interface, and the acquisition request stays the authority on rate + block.
//!
//! ## Where the rate lives
//!
//! The acquisition RATE and BLOCK SIZE are no longer fields of [`NiConfig`].
//! They ride [`hertzforge_hal::AcquisitionConfig`] into `configure()`, which answers
//! with an [`hertzforge_hal::ConfiguredActual`] carrying the rate the driver really
//! programmed. `NiConfig` is now the channel/front-end plan only. The seam
//! exists because DAQmx COERCES rates, and previously nothing in the type system
//! made a caller confront the coerced value.
//!
//! ## Prefill
//!
//! The AO buffer is non-regenerating, so it must be primed before the clock
//! starts or the first block underruns. `write_drive` works before `start()`:
//! whatever is written while the task is stopped IS the prefill. The engine's
//! top-up policy is what keeps it fed after that; this backend
//! provides the two facts that policy needs - `output_space()` and the accepted
//! frame count from `write_drive`.
//!
//! ## Threading
//!
//! One owning thread. `NiBackend` is `Send` (its task handles are moved, never
//! shared) but is not `Sync` in practice: every method takes `&mut self`, so the
//! borrow checker enforces one caller at a time. The abort path signals the
//! owning thread rather than calling into a handle from elsewhere. Enumeration
//! ([`crate::enumerate`]) opens its own short-lived driver handle and touches no
//! task, so a scan can run while a backend exists.

use crate::clock::{provisional_abort_latency, resolve_clock, ClockKey, ProvisionalAbortLatency};
use crate::config::{AiInterface, NiConfig};
use crate::error::NiError;
use crate::ffi::{self, val, Driver, Task, WarningLog};
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
};

/// Minimum read timeout. A block period alone can be sub-millisecond at small
/// block sizes, and a timeout that short would report `NoDataYet` constantly
/// even on a healthy stream.
const MIN_READ_TIMEOUT_S: f64 = 0.010;

/// Read timeout as a multiple of the block period. Long enough that a healthy
/// stream never times out, short enough that a stalled one is noticed within a
/// couple of blocks. Never `-1` (wait forever) and never "read all available" -
/// a nondeterministic frame count trips the engine's scratch guard.
const READ_TIMEOUT_BLOCKS: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Closed,
    Open,
    Configured,
    Armed,
    Running,
    Stopped,
}

/// What the driver actually programmed for one input channel's FRONT END.
///
/// The coupling and excitation-source constants this crate hard-codes are
/// UNVERIFIED against a real DAQmx header, so every one of them is read back
/// after being set. `None` means the getter is not exported by this runtime -
/// which is "not checked", never "checked and fine".
#[derive(Debug, Clone, PartialEq)]
pub struct FrontEndReadback {
    pub physical: String,
    pub requested_coupling: i32,
    pub coupling: Option<i32>,
    /// Requested excitation SOURCE, or `None` for an SE row (excitation
    /// attributes are never touched for SE - see `build_tasks`).
    pub requested_excit_src: Option<i32>,
    pub excit_src: Option<i32>,
    /// Requested excitation CURRENT in amps, or `None` for a row that asked for
    /// no excitation.
    pub requested_excit_a: Option<f64>,
    pub excit_a: Option<f64>,
}

/// What the driver said about one device in the task set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub id: String,
    pub product: String,
    pub serial: String,
    pub simulated: bool,
    pub chassis: Option<String>,
}

/// An `hertzforge_hal::Backend` over NI-DAQmx.
/// Taper length for a CONTROLLED stop (test complete, operator stop).
///
/// Long enough to be inaudible to the structure; there is no urgency here.
pub const CONTROLLED_STOP_RAMP_MS: f64 = 250.0;

/// Taper length for an ABORT.
///
/// The tension is real in both directions. A hard cut is a step, and a step
/// into an armature is a broadband impulse - it can do the damage the abort
/// exists to prevent. Too slow a taper leaves an out-of-limit condition running.
/// 25 ms is short against the measured 6.55 ms of already-queued drive (so the
/// total is dominated by neither), and long against the sample period at every
/// rate this backend runs, so the taper is a ramp and not a staircase.
pub const ABORT_RAMP_MS: f64 = 25.0;

/// Whether the `HERTZFORGE_NI_DIAG` stderr diagnostics are enabled, read ONCE.
/// `env::var_os` was being called twice per engine tick on the RT path
/// (write_drive + output_space) - a syscall-per-tick cost for a flag that
/// cannot change mid-process (found in a 2026-08-08 review).
fn ni_diag_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("HERTZFORGE_NI_DIAG").is_some())
}

pub struct NiBackend {
    cfg: NiConfig,
    /// The rate + block size ASKED FOR. Seeded at construction (the selftest
    /// path goes `open()` -> `arm()` and never calls `configure`) and REPLACED
    /// by every [`Backend::configure`] call - that call is the seam.
    acq: AcquisitionConfig,
    driver: Option<Driver>,
    ai: Option<Task>,
    ao: Option<Task>,
    state: State,

    /// The rate the DRIVER programmed (read back after timing configuration).
    /// Equals `acq.requested_rate_hz` until `open()` has run.
    actual_rate_hz: f64,
    clock: ClockDomain,
    clock_evidence: String,
    /// The provisional `[H]` abort-latency bound for the CURRENT actual rate and
    /// block size, recomputed with the tasks. `None` before the tasks are built
    /// (nothing is known yet) and on any set whose formula inputs are missing.
    abort_latency: Option<ProvisionalAbortLatency>,
    ao_routing: String,
    devices: Vec<DeviceIdentity>,
    all_simulated: bool,
    regen_mode_readback: Option<i32>,

    /// In-band counters. `frames_read` is the backend's sample counter and the
    /// engine's timebase; a plot moving while this is flat is a bug.
    frames_read: u64,
    frames_written: u64,
    epoch: u32,
    faulted: bool,

    /// Has any channel reported ADC overload since the last `start()`?
    overload_latched: bool,
    /// Set false once the overload attribute is proven absent, so a DAQmx
    /// generation without it costs one lookup, not one per block.
    overload_pollable: bool,

    read_scratch: Vec<f64>,
    write_scratch: Vec<f64>,
    /// When the device last ACCEPTED drive frames. The age of this at the
    /// moment the buffer reads zero-free is what separates "starved by a host-side
    /// gap" from "generation died".
    last_write_at: Option<std::time::Instant>,
    /// Latched so one stall reports once rather than every poll.
    starve_reported: bool,
    /// Samples-generated at the previous poll, and how many consecutive polls
    /// have seen it FROZEN. This is the generating-vs-stalled question,
    /// and it is deliberately independent of what the space query claims.
    last_gen: Option<u64>,
    gen_stuck_polls: u32,
    /// Latched so a refusing write reports once per run of refusals.
    refuse_reported: bool,
    /// Set by `ramp_drive_to_zero` when its taper could NOT complete (fewer frames
    /// accepted than the taper length, or generation short of the taper's end at
    /// the deadline), and consumed by `stop()` / `emergency_stop()` once the tasks
    /// are stopped: an incomplete taper leaves the AO HOLDING whatever it last
    /// generated, so those paths then zero the outputs explicitly
    /// (`zero_outputs_after_cut_taper`). Never set on a taper that completed.
    taper_cut_short: bool,
    /// The last drive FRAME actually accepted by the device, one value per
    /// output channel. A shutdown taper has to start from the value already on
    /// the wire; starting from anywhere else puts a step at the top of the very
    /// ramp whose job is to avoid one.
    last_drive_frame: Vec<f32>,
    /// Per-channel sensitivity in mV/EU, in task-channel order. The read path
    /// converts with these, host-side, exactly once.
    sensitivity_mv_per_eu: Vec<f64>,
    front_ends: Vec<FrontEndReadback>,
    warnings: WarningLog,
    /// WINDOWS ONLY: the 1 ms system-timer request held from `arm()` until the
    /// tasks are stopped, aborted, rebuilt, closed or dropped. Without it DAQmx's
    /// default Sleep read wait rounds up to the 15.6 ms Windows timer and the
    /// engine stops on period jitter at a 20 ms tick (measurement and pairing rule:
    /// `crate::timer_resolution`). `None` whenever nothing is armed, and whenever
    /// the OS refused the request (reported as a warning). Every release is an
    /// assignment to `None`, so the matching `timeEndPeriod` runs exactly once per
    /// successful `timeBeginPeriod`, on every path including `Drop`.
    #[cfg(windows)]
    host_loop_timer: Option<crate::timer_resolution::SystemTimerHold>,
}

impl std::fmt::Debug for NiBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NiBackend")
            .field("state", &self.state)
            .field("inputs", &self.cfg.n_input_channels())
            .field("outputs", &self.cfg.n_output_channels())
            .field("actual_rate_hz", &self.actual_rate_hz)
            .field("clock", &self.clock)
            .field("frames_read", &self.frames_read)
            .field("frames_written", &self.frames_written)
            .finish_non_exhaustive()
    }
}

/// The AO clock-routing decision, extracted pure so the negative-control
/// override is testable without a driver: a chassis routes the AO onto
/// `/<chassis>/ai/SampleClock`; no chassis (or a cross-chassis set) leaves the
/// AO on its own clock. `force_unrouted` (clock_loopback --unrouted) strips the
/// route ON PURPOSE so the arrival-day measurement can demonstrate drift
/// between genuinely independent clocks - a negative control that silently
/// still routed would make the routed run's "flat" verdict unfalsifiable. The
/// returned string flows into `ClockKey`, so the clock domain resolves exactly
/// as any unrouted set would - no special-casing downstream.
fn derive_ao_routing(chassis: Option<&str>, force_unrouted: bool) -> String {
    if force_unrouted {
        return String::new();
    }
    match chassis {
        Some(c) => format!("/{c}/ai/SampleClock"),
        None => String::new(),
    }
}

impl NiBackend {
    /// A backend for `cfg` at `acq`. Nothing touches the driver until
    /// [`Backend::open`].
    ///
    /// `acq` is the INITIAL acquisition request, needed because `open()` builds
    /// the tasks and a task needs a rate. [`Backend::configure`] replaces it and
    /// reports what the driver actually programmed; that call is the seam, and
    /// this argument is only the bootstrap for the `open() -> arm()` path the
    /// selftest uses.
    pub fn new(cfg: NiConfig, acq: AcquisitionConfig) -> Self {
        let sensitivity_mv_per_eu = cfg
            .ai_assignments
            .iter()
            .map(|a| a.sensitivity_mv_per_eu)
            .collect();
        NiBackend {
            actual_rate_hz: acq.requested_rate_hz,
            acq,
            sensitivity_mv_per_eu,
            front_ends: Vec::new(),
            cfg,
            driver: None,
            ai: None,
            ao: None,
            state: State::Closed,
            clock: ClockDomain::Unknown,
            clock_evidence: "not determined: the backend has not been opened".into(),
            abort_latency: None,
            ao_routing: String::new(),
            devices: Vec::new(),
            all_simulated: false,
            regen_mode_readback: None,
            frames_read: 0,
            frames_written: 0,
            epoch: 0,
            faulted: false,
            overload_latched: false,
            overload_pollable: true,
            read_scratch: Vec::new(),
            write_scratch: Vec::new(),
            last_write_at: None,
            starve_reported: false,
            last_gen: None,
            gen_stuck_polls: 0,
            refuse_reported: false,
            taper_cut_short: false,
            last_drive_frame: Vec::new(),
            warnings: WarningLog::new(),
            #[cfg(windows)]
            host_loop_timer: None,
        }
    }

    /// The task plan this backend is running.
    pub fn config(&self) -> &NiConfig {
        &self.cfg
    }

    /// The acquisition request currently in force (rate + block size).
    pub fn acquisition(&self) -> &AcquisitionConfig {
        &self.acq
    }

    /// The provisional `[H]` abort-latency bound in force, with its derivation.
    /// `None` until the tasks are built, and on any set whose formula inputs are
    /// unavailable - in which case the arm gate REFUSES rather than assuming.
    pub fn abort_latency(&self) -> Option<&ProvisionalAbortLatency> {
        self.abort_latency.as_ref()
    }

    /// The rate the DRIVER programmed. Differs from the request whenever the
    /// hardware coerced it; the coercion itself arrives as a logged warning.
    pub fn actual_rate_hz(&self) -> f64 {
        self.actual_rate_hz
    }

    /// Why [`Caps::clock`] says what it says.
    ///
    /// `Caps` carries no evidence field (adding one belongs to the resolver
    /// phase, not here), so this inherent accessor is how the basis for a
    /// closed-loop refusal reaches an operator.
    pub fn clock_evidence(&self) -> &str {
        &self.clock_evidence
    }

    /// The AO sample-clock routing string actually used (`""` = the AO task's
    /// own onboard clock).
    pub fn ao_routing(&self) -> &str {
        &self.ao_routing
    }

    /// Identity of every device in the task set, as the driver reported it.
    pub fn devices(&self) -> &[DeviceIdentity] {
        &self.devices
    }

    /// What the driver actually programmed for each input channel's front end
    /// (coupling + excitation), read back after being set. Empty before
    /// [`Backend::open`].
    pub fn front_ends(&self) -> &[FrontEndReadback] {
        &self.front_ends
    }

    /// True when the DRIVER reports every device in the task set as simulated.
    pub fn all_simulated(&self) -> bool {
        self.all_simulated
    }

    /// The regeneration mode read BACK from the AO task after it was set.
    /// `Some(`[`val::DO_NOT_ALLOW_REGEN`]`)` is the only acceptable value; a
    /// regenerating AO looping a stale buffer while plots stay alive is the
    /// sim-leak shape in hardware clothes.
    pub fn regen_mode_readback(&self) -> Option<i32> {
        self.regen_mode_readback
    }

    /// Cumulative frames read from the device since construction.
    pub fn frames_read(&self) -> u64 {
        self.frames_read
    }

    /// Cumulative frames accepted by the device's output buffer.
    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }

    /// Has any channel reported ADC overload since the last `start()`?
    pub fn overload_latched(&self) -> bool {
        self.overload_latched
    }

    /// Whether the ADC-overload attribute exists on this driver at all. When
    /// false, overload is NOT being checked - which is a gap to report, not a
    /// clean bill of health.
    pub fn overload_pollable(&self) -> bool {
        self.overload_pollable
    }

    /// Every warning recorded so far (driver warnings plus this backend's own),
    /// leaving the log intact.
    pub fn warnings(&self) -> Vec<String> {
        self.absorb_driver_warnings();
        self.warnings.snapshot()
    }

    /// Take every warning recorded so far, emptying the log.
    pub fn drain_warnings(&self) -> Vec<String> {
        self.absorb_driver_warnings();
        self.warnings.drain()
    }

    fn absorb_driver_warnings(&self) {
        if let Some(d) = &self.driver {
            for w in d.warnings().drain() {
                self.warnings.push(w);
            }
        }
    }

    fn drv(&self) -> Result<&Driver, NiError> {
        self.driver.as_ref().ok_or_else(|| {
            NiError::config("the NI driver is not loaded; call open() before this operation")
        })
    }

    fn require_ai(&self) -> Result<Task, NiError> {
        self.ai
            .ok_or_else(|| NiError::config("no input task exists; call open() first"))
    }

    fn read_timeout_s(&self, frames: usize) -> f64 {
        let rate = if self.actual_rate_hz > 0.0 {
            self.actual_rate_hz
        } else {
            self.acq.requested_rate_hz.max(1.0)
        };
        (READ_TIMEOUT_BLOCKS * frames as f64 / rate).max(MIN_READ_TIMEOUT_S)
    }

    // --- task construction ---------------------------------------------------

    /// Clear both tasks, ignoring failures. Safe from any state.
    fn clear_tasks(&mut self) {
        if let Some(d) = &self.driver {
            if let Some(t) = self.ao.take() {
                let _ = d.stop_task(t);
                let _ = d.clear_task(t);
            }
            if let Some(t) = self.ai.take() {
                let _ = d.stop_task(t);
                let _ = d.clear_task(t);
            }
        } else {
            self.ao = None;
            self.ai = None;
        }
        // No task, no host loop: release the 1 ms timer (close, Drop, a rebuild,
        // and the abort path all come through here).
        #[cfg(windows)]
        {
            self.host_loop_timer = None;
        }
    }

    /// WINDOWS ONLY: hold the 1 ms system timer for the armed/running life of the
    /// tasks (why: `crate::timer_resolution`). Idempotent while held. A refusal is
    /// a warning, not an arm failure - the run proceeds exactly as it did before
    /// this existed, and the warning says what to expect.
    #[cfg(windows)]
    fn hold_host_loop_timer(&mut self) {
        use crate::timer_resolution::{
            honor_timer_resolution_when_occluded, SystemTimerHold, Winmm, HOST_LOOP_TIMER_MS,
        };
        if self.host_loop_timer.is_some() {
            return;
        }
        if let Err(e) = honor_timer_resolution_when_occluded() {
            self.warnings.push(format!(
                "could not opt this process out of Windows timer-resolution throttling \
                 (SetProcessInformation(ProcessPowerThrottling): {e}); the 1 ms timer may be \
                 ignored while the window is minimized or hidden, and a run at a 20 ms tick \
                 can then stop on period jitter. Keep the window visible during a run."
            ));
        }
        match SystemTimerHold::acquire(Winmm, HOST_LOOP_TIMER_MS) {
            Ok(hold) => self.host_loop_timer = Some(hold),
            Err(code) => self.warnings.push(format!(
                "timeBeginPeriod({HOST_LOOP_TIMER_MS}) refused (MMRESULT {code}); input reads \
                 wait on the 15.6 ms default Windows timer, and a run at a block period under \
                 about 30 ms can stop on period jitter"
            )),
        }
    }

    /// Read the identity of every device the assignments name.
    ///
    /// `simulated` is a HARD requirement, not a best effort: the clock rule and
    /// the SIMULATED badge both hang off it, and a guessed value would either
    /// hide a simulated run or slander a real one.
    fn read_device_identity(&mut self) -> Result<(), NiError> {
        let d = self.drv()?;
        let (devices, all_sim) = read_identity_with(d, &self.cfg.devices())?;
        self.all_simulated = all_sim;
        self.devices = devices;
        Ok(())
    }

    fn identity(&self, dev: &str) -> Option<&DeviceIdentity> {
        self.devices.iter().find(|d| d.id == dev)
    }

    /// The chassis every device in the task set shares, if there is exactly one.
    ///
    /// A cross-chassis set has no single `ai/SampleClock` terminal to route to,
    /// so it is treated the same as no chassis at all: the AO task falls back to
    /// its own clock and the domain stays `Unknown`.
    fn common_chassis(&self) -> Option<String> {
        let mut seen: Option<String> = None;
        for dev in self.cfg.devices() {
            let ch = self.identity(&dev).and_then(|i| i.chassis.clone())?;
            match &seen {
                None => seen = Some(ch),
                Some(prev) if *prev == ch => {}
                Some(_) => return None,
            }
        }
        seen
    }

    /// `+`-joined product strings of the devices in `list`, deduped, in
    /// first-seen order. A mixed-module set gets its own clock key, because it
    /// needs its own evidence.
    fn products_of(&self, list: &[String]) -> String {
        let mut parts: Vec<String> = Vec::new();
        for dev in list {
            let p = self
                .identity(dev)
                .map(|i| i.product.clone())
                .unwrap_or_default();
            if !parts.contains(&p) {
                parts.push(p);
            }
        }
        parts.join("+")
    }

    /// Build (or rebuild) both DAQmx tasks from `self.cfg`.
    fn build_tasks(&mut self) -> Result<(), NiError> {
        self.cfg.validate()?;
        self.clear_tasks();
        self.read_device_identity()?;

        self.sensitivity_mv_per_eu = self
            .cfg
            .ai_assignments
            .iter()
            .map(|a| a.sensitivity_mv_per_eu)
            .collect();
        self.regen_mode_readback = None;
        self.overload_latched = false;
        self.overload_pollable = true;

        // --- AI task ---------------------------------------------------------
        let ai_name = format!("hertzforge_ai_{}", task_name_seq());
        let ai = self.drv()?.create_task(&ai_name)?;
        self.ai = Some(ai);
        let mut front_ends: Vec<FrontEndReadback> = Vec::new();
        let actual_rate = {
            let d = self.driver.as_ref().expect("driver loaded above");
            for a in &self.cfg.ai_assignments {
                // EVERY input row is a plain VOLTAGE channel. The driver never
                // applies a sensor sensitivity - the host does, exactly once,
                // in `read_input` via `AiAssign::to_eu`.
                let range = a.range_volts();
                d.create_ai_voltage_chan(ai, &a.physical, val::CFG_DEFAULT, -range, range)?;
                // Front-end attributes are applied ONLY where the row demands
                // non-default behavior. IEPE rows: AC coupling + internal
                // excitation, hard failures (a DC-coupled IEPE input rails at
                // the sensor's bias; an unexcited one reads as a dead channel).
                // SE rows: request DC coupling where the device supports the
                // property; a device without selectable coupling (9215/6363
                // class) is natively DC and the refusal (-200452) downgrades to
                // a warning. Excitation attributes are NEVER touched for SE -
                // even writing "none" errors on devices without excitation
                // hardware (proven on the simulated 9215/6363, 2026-07-25).
                match a.interface {
                    AiInterface::Iepe => {
                        d.set_ai_coupling(ai, &a.physical, a.coupling())?;
                        d.set_ai_excit_src(ai, &a.physical, a.excitation_source())?;
                        d.set_ai_excit_val(ai, &a.physical, self.cfg.excitation_a)?;
                    }
                    AiInterface::Voltage => {
                        match d.set_ai_coupling(ai, &a.physical, a.coupling()) {
                            Ok(()) => {}
                            Err(ref e) if e.daqmx_code() == Some(-200452) => {
                                self.warnings.push(format!(
                                    "{}: coupling not selectable on this device (property \
                                     unsupported); SE row uses the device-native coupling",
                                    a.physical
                                ));
                            }
                            Err(e) => return Err(e),
                        }
                    }
                }
                // Read back what the driver actually programmed. The coupling
                // and excitation-source values are UNVERIFIED constants; a
                // readback turns them from a doc claim into a measurement.
                // Excitation readbacks are attempted only where excitation was
                // requested, so an SE row on a non-excitation device produces
                // no readback noise.
                let is_iepe = a.interface == AiInterface::Iepe;
                front_ends.push(FrontEndReadback {
                    physical: a.physical.clone(),
                    requested_coupling: a.coupling(),
                    coupling: d.get_ai_coupling(ai, &a.physical).ok(),
                    requested_excit_src: if is_iepe {
                        Some(a.excitation_source())
                    } else {
                        None
                    },
                    excit_src: if is_iepe {
                        d.get_ai_excit_src(ai, &a.physical).ok()
                    } else {
                        None
                    },
                    requested_excit_a: if is_iepe {
                        Some(self.cfg.excitation_a)
                    } else {
                        None
                    },
                    excit_a: if is_iepe {
                        d.get_ai_excit_val(ai, &a.physical).ok()
                    } else {
                        None
                    },
                });
            }
            // Continuous timing at the REQUESTED rate. A coercion arrives here
            // as a positive status and is logged as a warning, not an error.
            d.cfg_samp_clk_timing(
                ai,
                "",
                self.acq.requested_rate_hz,
                self.cfg.ai_buffer_frames(self.acq.block_frames) as u64,
            )?;
            // ...and the ACTUAL rate is what propagates from here on.
            let actual = d.samp_clk_rate(ai)?;
            // Size the input buffer explicitly rather than trusting the driver's
            // default: a short buffer overflows before the engine's next poll.
            // Seconds-floored at the ACTUAL rate - the blocks-only ring was 160 ms
            // at 51.2 kHz/1024 and startup latency alone overflowed it (-200279).
            d.cfg_input_buffer(
                ai,
                self.cfg.ai_ring_frames(self.acq.block_frames, actual) as u32,
            )?;
            // COMMIT so that everything the driver validates lazily (routing
            // strings, buffer allocation, rate program) is validated NOW, at
            // configure time - proven necessary 2026-07-25: CfgSampClkTiming
            // ACCEPTS a clock route on a nonexistent device; only commit/start
            // refuses it. A config error must never wait for start().
            match d.task_control(ai, val::TASK_COMMIT) {
                Ok(()) => {}
                Err(e @ NiError::MissingSymbol { .. }) => self.warnings.push(format!(
                    "AI task commit unavailable ({e}); lazy driver validation is deferred \
                     to start()"
                )),
                Err(e) => return Err(e),
            }
            actual
        };
        self.actual_rate_hz = actual_rate;
        self.front_ends = front_ends;
        for fe in &self.front_ends {
            if let Some(got) = fe.coupling {
                if got != fe.requested_coupling {
                    self.warnings.push(format!(
                        "{}: coupling read back as {got}, requested {}",
                        fe.physical, fe.requested_coupling
                    ));
                }
            }
        }
        if (self.actual_rate_hz - self.acq.requested_rate_hz).abs() > 1e-9 {
            self.warnings.push(format!(
                "sample rate coerced: requested {} Hz, device programmed {} Hz; the ACTUAL rate \
                 is what caps/engine/DSP use from here",
                self.acq.requested_rate_hz, self.actual_rate_hz
            ));
        }

        // --- AO task ---------------------------------------------------------
        self.ao_routing = String::new();
        if !self.cfg.ao_assignments.is_empty() {
            let chassis = self.common_chassis();
            // A chassis exposes its AI sample clock as a routable terminal, so
            // the AO task can ride the same clock. Without one, the AO task uses
            // its own clock and the domain stays Unknown (never asserted).
            self.ao_routing = derive_ao_routing(chassis.as_deref(), self.cfg.force_unrouted_ao);
            let ao_name = format!("hertzforge_ao_{}", task_name_seq());
            let ao = self.drv()?.create_task(&ao_name)?;
            self.ao = Some(ao);
            let (regen_back, commit_err) = {
                let d = self.driver.as_ref().expect("driver loaded above");
                let range = self.cfg.ao_range_v;
                for a in &self.cfg.ao_assignments {
                    d.create_ao_voltage_chan(ao, &a.physical, -range, range)?;
                    // The BACKSTOP for every path that never reaches
                    // `emergency_stop`'s taper. Not the device default: the
                    // default holds the last commanded sample on the wire after
                    // the task stops (measured, gate 5, 2026-08-01). Optional
                    // because not every AO device implements the attribute -
                    // a device that refuses it is recorded, not fatal, since the
                    // taper still runs on the normal paths.
                    match d.set_ao_idle_output_behavior(ao, &a.physical, val::ZERO_VOLTS) {
                        Ok(()) => match d.get_ao_idle_output_behavior(ao, &a.physical) {
                            Ok(v) if v == val::ZERO_VOLTS => {}
                            Ok(v) => self.warnings.push(format!(
                                "{}: AO idle-output-behavior set to ZeroVolts but reads back {v};                                  the output may HOLD its last sample after a stop",
                                a.physical
                            )),
                            Err(e) => self.warnings.push(format!(
                                "{}: AO idle-output-behavior set but not readable ({e})",
                                a.physical
                            )),
                        },
                        Err(e) => self.warnings.push(format!(
                            "{}: AO idle-output-behavior NOT settable ({e}); after a stop this                              output HOLDS its last commanded sample - the shutdown taper is the                              only thing bringing it to zero",
                            a.physical
                        )),
                    }
                }
                d.cfg_samp_clk_timing(
                    ao,
                    &self.ao_routing,
                    self.actual_rate_hz,
                    self.cfg.ao_buffer_frames(self.acq.block_frames) as u64,
                )?;
                // Non-regenerating: running dry is an UNDERRUN the engine must
                // see, never a silent replay of stale drive.
                d.set_write_regen_mode(ao, val::DO_NOT_ALLOW_REGEN)?;
                let back = d.get_write_regen_mode(ao)?;
                d.cfg_output_buffer(ao, self.cfg.ao_buffer_frames(self.acq.block_frames) as u32)?;
                // Commit so (a) the buffer is allocated and WriteSpaceAvail is
                // meaningful before start, and (b) the clock ROUTE is validated
                // NOW - the driver accepts any routing string at
                // CfgSampClkTiming and only refuses at commit/start (proven
                // 2026-07-25). Only a missing DAQmxTaskControl symbol degrades;
                // a driver refusal is a hard config error.
                (back, d.task_control(ao, val::TASK_COMMIT).err())
            };
            self.regen_mode_readback = Some(regen_back);
            if regen_back != val::DO_NOT_ALLOW_REGEN {
                return Err(NiError::config(format!(
                    "AO regeneration mode read back as {regen_back}, expected {} \
                     (DoNotAllowRegen). A regenerating output would loop stale drive on the \
                     shaker while the plots stayed alive.",
                    val::DO_NOT_ALLOW_REGEN
                )));
            }
            match commit_err {
                None => {}
                Some(e @ NiError::MissingSymbol { .. }) => self.warnings.push(format!(
                    "AO task commit unavailable ({e}); write-space queries before start() may \
                     be unavailable and lazy route validation is deferred to start()"
                )),
                Some(e) => return Err(e),
            }
        }

        // --- clock domain ----------------------------------------------------
        // The key's routing component is NAME-NORMALIZED: device names are
        // operator-renameable in NI MAX, and a proven-set entry keyed on
        // "/cDAQ1/..." would silently stop matching after a rename. The task
        // keeps the real routing string; only the KEY abstracts the name.
        let key_routing = match self.common_chassis() {
            Some(c) if !self.ao_routing.is_empty() => {
                self.ao_routing.replace(&format!("/{c}/"), "/<chassis>/")
            }
            _ => self.ao_routing.clone(),
        };
        let key = ClockKey::new(
            self.common_chassis()
                .and_then(|c| self.identity(&c).map(|i| i.product.clone()))
                .unwrap_or_default(),
            self.products_of(&self.cfg.input_devices()),
            self.products_of(&self.cfg.output_devices()),
            key_routing,
        );
        let (clock, evidence) = resolve_clock(self.all_simulated, &key);
        self.clock = clock;
        self.clock_evidence = evidence;

        // --- provisional [H] abort-latency bound -----------------------------
        // Derived from the ACTUAL rate (the delta-sigma delay is fixed in
        // SAMPLES, so a coerced rate changes the duration) and the block policy.
        // Seeded for BOTH simulated and real sets: without it the arm gate has
        // no bound to permit and nothing to produce one, which is the bootstrap
        // deadlock round 3 found. A set whose inputs are unavailable gets `None`
        // and the gate refuses.
        self.abort_latency = provisional_abort_latency(
            self.actual_rate_hz,
            self.acq.block_frames,
            self.cfg.lookahead_blocks,
        );

        self.absorb_driver_warnings();
        Ok(())
    }

    /// One [`hertzforge_hal::ClockGroup`] per distinct chassis (or per standalone
    /// device) in the task set.
    ///
    /// EMPTY while nothing has been read from the driver - "one implicit group",
    /// per the `ClockGroup` contract. Every group carries the SET's resolved
    /// domain rather than a per-group re-resolution, because that is exactly
    /// what has been established: `resolve_clock` answers for the configuration
    /// as a whole, and inventing a rosier per-group answer would break the
    /// conjunction rule the contract states.
    fn clock_group_breakdown(&self) -> Vec<hertzforge_hal::ClockGroup> {
        let mut labels: Vec<String> = Vec::new();
        for d in &self.devices {
            let label = d.chassis.clone().unwrap_or_else(|| d.id.clone());
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
        labels
            .into_iter()
            .map(|label| hertzforge_hal::ClockGroup {
                label,
                domain: self.clock,
            })
            .collect()
    }

    /// Validate an acquisition request before it is programmed.
    fn check_acq(acq: &AcquisitionConfig) -> Result<(), NiError> {
        if !(acq.requested_rate_hz.is_finite() && acq.requested_rate_hz > 0.0) {
            return Err(NiError::config(format!(
                "requested rate must be finite and positive, got {}",
                acq.requested_rate_hz
            )));
        }
        if acq.block_frames == 0 {
            return Err(NiError::config("block_frames must be at least 1"));
        }
        Ok(())
    }

    /// Query the device's free output space in FRAMES (samples per channel).
    /// Taper the drive from wherever it is to zero over `ms`, then leave it
    /// there. Best-effort by construction: every failure path falls through to
    /// the caller's task stop, which with `IdleOutputBehavior = ZeroVolts` is
    /// itself safe.
    ///
    /// Shape is a raised cosine (half-Hann): unity at the first taper frame,
    /// zero at the last, with zero slope at BOTH ends. A linear ramp would have
    /// a slope discontinuity at the top and bottom - smaller than a hard step,
    /// but still a transient, and the whole point here is not to hit the
    /// structure on the way out.
    ///
    /// The taper is APPENDED, and on this hardware it has to be.
    ///
    /// **Preemption was tried and is not available** (measured 2026-08-01). The
    /// documented DAQmx idiom for overwriting queued-but-not-yet-generated
    /// samples - `WriteRelativeTo = FirstSample` plus an absolute `WriteOffset`
    /// ahead of `TotalSampPerChanGenerated` - is refused by the cDAQ-9178 at
    /// EVERY guard from 2 ms to 120 ms with `-200289: attempted to write samples
    /// that have already been sent to the device for generation`. A USB chassis
    /// streams the whole AO buffer into onboard memory well ahead of the pin, so
    /// once drive is queued it is committed.
    ///
    /// The consequence is a real constraint, not an implementation detail:
    /// **abort latency is bounded below by AO queue depth**
    /// (`lookahead_blocks + 1` blocks), and the only lever on it is queueing
    /// less. Time-to-zero is that queue plus `ms`.
    fn ramp_drive_to_zero(&mut self, ms: f64) {
        let n_ch = self.cfg.n_output_channels();
        if n_ch == 0 || self.ao.is_none() || !matches!(self.state, State::Running) {
            return;
        }
        // Start from the last frame the device actually accepted. If nothing has
        // been written yet there is no drive to taper.
        if self.last_drive_frame.len() < n_ch || self.last_drive_frame.iter().all(|v| *v == 0.0) {
            return;
        }
        let start: Vec<f32> = self.last_drive_frame[..n_ch].to_vec();
        let rate = if self.actual_rate_hz > 0.0 {
            self.actual_rate_hz
        } else {
            return;
        };
        let n_frames = ((ms / 1000.0) * rate).round() as usize;
        if n_frames < 2 {
            return;
        }
        let mut ramp = vec![0.0f32; n_frames * n_ch];
        for k in 0..n_frames {
            // w[0] = 1, w[n-1] = 0, zero slope at both ends.
            let w = 0.5 * (1.0 + (std::f64::consts::PI * k as f64 / (n_frames - 1) as f64).cos());
            for c in 0..n_ch {
                ramp[k * n_ch + c] = (start[c] as f64 * w) as f32;
            }
        }
        // Push it through, honoring partial acceptance. Bounded by wall clock:
        // if the buffer will not drain we stop trying and let the task stop
        // (plus the ZeroVolts idle behavior) finish the job.
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_millis((ms * 4.0).max(50.0) as u64);
        let mut sent = 0usize;
        while sent < n_frames && std::time::Instant::now() < deadline {
            match self.write_drive(&ramp[sent * n_ch..]) {
                Ok(0) => std::thread::sleep(std::time::Duration::from_micros(200)),
                Ok(f) => sent += f,
                Err(_) => break,
            }
        }
        if sent < n_frames {
            self.warnings.push(format!(
                "shutdown taper incomplete: {sent}/{n_frames} frames accepted before the \
                 deadline; the drive will be cut where the taper stopped"
            ));
            self.taper_cut_short = true;
        }

        // WAIT for the taper to actually reach the pin.
        //
        // This wait IS the mechanism, not politeness. Stopping the task here
        // freezes the output at whatever the taper had reached (the device holds
        // its last sample), so a stop before generation catches up is the same
        // bug as the zero-block it replaced - only with a nicer-looking buffer.
        let target = self.frames_written;
        let ao = self.ao.expect("checked above");
        let budget = ((self.cfg.ao_buffer_frames(self.acq.block_frames) as f64 + n_frames as f64)
            / rate)
            * 2.0;
        let wait_until =
            std::time::Instant::now() + std::time::Duration::from_secs_f64(budget.min(2.0));
        // Device stop path with no hardware-free test, so it keeps its reviewed form
        // rather than a `while let` rewrite for a style lint.
        #[allow(clippy::while_let_loop)]
        loop {
            let generated = match self.drv() {
                Ok(d) => d
                    .write_total_samp_per_chan_generated(ao)
                    .unwrap_or(u64::MAX),
                Err(_) => break,
            };
            if generated >= target {
                break;
            }
            if std::time::Instant::now() >= wait_until {
                self.warnings.push(format!(
                    "shutdown taper did not finish generating within {:.0} ms ({generated}/{target} \
                     frames); the drive is being cut mid-taper",
                    budget.min(2.0) * 1e3
                ));
                self.taper_cut_short = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
    }

    /// After a shutdown taper that did NOT complete, and only then: put every AO
    /// channel at 0 V. Called with the tasks already stopped and UNRESERVED.
    ///
    /// The 9263 holds its last generated sample after its task stops (and
    /// `IdleOutputBehavior = ZeroVolts` is not supported on it - see
    /// `emergency_stop`), so a taper that was cut short leaves DC on the wire, and
    /// it survives the process. Measured on the Windows bench 2026-11-01: a play
    /// whose AO had stalled accepted none of its taper and left -30 mV held after
    /// the run. A COMPLETED taper ends on a generated zero and never comes here,
    /// so that path's device sequence is unchanged.
    ///
    /// Mechanism: the stopped, unreserved task has released the channels, so a
    /// short-lived ON-DEMAND task (no sample clock) can take them, write one frame
    /// of zeros, and let them go - [`zero_ao_on_demand`]. That is one step from
    /// wherever the cut taper left the output down to 0 V: a single transient,
    /// where the alternative is holding that level indefinitely. Best effort:
    /// every outcome is a warning, because this runs on the stop and abort paths,
    /// which must not fail.
    fn zero_outputs_after_cut_taper(&mut self) {
        let physical: Vec<String> = self
            .cfg
            .ao_assignments
            .iter()
            .map(|a| a.physical.clone())
            .collect();
        if physical.is_empty() {
            return;
        }
        let range = self.cfg.ao_range_v;
        let outcome = match self.drv() {
            Ok(d) => zero_ao_on_demand(
                d,
                &format!("hertzforge_ao_zero_{}", task_name_seq()),
                &physical,
                range,
            ),
            Err(e) => Err(e),
        };
        match outcome {
            Ok(()) => self.warnings.push(format!(
                "shutdown taper was cut short; {} set to 0 V by an on-demand write after the \
                 task stopped (one step from where the taper stopped)",
                physical.join(", ")
            )),
            Err(e) => self.warnings.push(format!(
                "shutdown taper was cut short and {} could NOT be zeroed afterwards ({e}); the \
                 output may be HOLDING its last generated sample - check it before touching \
                 the setup",
                physical.join(", ")
            )),
        }
    }

    fn query_write_space(&self) -> Result<u32, NiError> {
        let ao = self
            .ao
            .ok_or_else(|| NiError::config("no output task exists"))?;
        self.drv()?.write_space_avail(ao)
    }

    /// Poll ADC overload once. Absence of the attribute degrades to "not
    /// checked" - recorded, never treated as "no overload".
    fn poll_overload(&mut self) {
        if !self.overload_pollable {
            return;
        }
        let Ok(ai) = self.require_ai() else { return };
        let Ok(d) = self.drv() else { return };
        match d.read_overloaded_chans_exist(ai) {
            Ok(true) => {
                if !self.overload_latched {
                    self.warnings
                        .push("ADC OVERLOAD reported on at least one input channel".to_string());
                }
                self.overload_latched = true;
            }
            Ok(false) => {}
            Err(NiError::MissingSymbol { symbol, .. }) => {
                self.overload_pollable = false;
                self.warnings.push(format!(
                    "ADC overload is NOT being checked: this DAQmx runtime does not export \
                     {symbol}. Overrange on a delta-sigma input will not be reported."
                ));
            }
            Err(e) => {
                self.overload_pollable = false;
                self.warnings
                    .push(format!("ADC overload is NOT being checked: {e}"));
            }
        }
    }
}

/// Read every device's identity from an attribute source, and REFUSE a task set
/// that mixes real and driver-simulated devices.
///
/// Free function over [`crate::enumerate::SysQuery`] rather than a method on
/// `NiBackend`, so the refusal is provable on a host with no NI driver: the
/// canned attribute source drives it directly. The real `Driver` implements
/// `SysQuery`, so production and test go through the same code.
///
/// ## Why a mixed set is refused rather than degraded
///
/// The whole simulated-device tier of the clock rule ([`resolve_clock`] tier 1)
/// rests on "a simulated device has no physical clock, so the physical-drift
/// failure mode cannot occur AND no armature can be attached". Neither half
/// survives mixing: a set containing one real module CAN be wired to an armature,
/// and the real module's clock CAN drift against everything else. The SIMULATED
/// badge has the same problem - it is a property of the RUN, and a run that is
/// half real cannot honestly carry it or honestly drop it. So the set is refused
/// at configure time, which is the last moment a human can still fix the
/// selection.
pub(crate) fn read_identity_with<Q: crate::enumerate::SysQuery + ?Sized>(
    q: &Q,
    device_ids: &[String],
) -> Result<(Vec<DeviceIdentity>, bool), NiError> {
    let mut out: Vec<DeviceIdentity> = Vec::new();
    for id in device_ids {
        let id = id.clone();
        let simulated = q.dev_is_simulated(&id).map_err(|e| {
            NiError::config(format!(
                "cannot read the simulated flag for device {id} ({e}); refusing to build a \
                 task whose SIMULATED badge and clock domain would both be guesses"
            ))
        })?;
        out.push(DeviceIdentity {
            product: q.dev_product_type(&id).unwrap_or_default(),
            serial: q
                .dev_serial_num(&id)
                .map(crate::enumerate::format_serial)
                .unwrap_or_default(),
            chassis: q
                .dev_chassis_name(&id)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            simulated,
            id,
        });
    }

    // The PARENT CHASSIS of every module joins the identity map: the clock
    // key needs the chassis PRODUCT (an entry keyed on a blank chassis would
    // silently stop matching the day the lookup improved - found on the first
    // real-hardware gate 3, where the evidence string printed chassis="").
    let parents: Vec<String> = out
        .iter()
        .filter_map(|d| d.chassis.clone())
        .filter(|c| !out.iter().any(|d| &d.id == c))
        .collect();
    for chassis in parents {
        if out.iter().any(|d| d.id == chassis) {
            continue;
        }
        // A chassis whose simulated flag cannot be read inherits its RESIDENT
        // modules' flag - the modules are physically inside it, and defaulting
        // to "real" would trip the mixed-set refusal on an all-sim rig.
        let children_sim = out
            .iter()
            .filter(|d| d.chassis.as_deref() == Some(chassis.as_str()))
            .all(|d| d.simulated);
        let simulated = q.dev_is_simulated(&chassis).unwrap_or(children_sim);
        out.push(DeviceIdentity {
            product: q.dev_product_type(&chassis).unwrap_or_default(),
            serial: q
                .dev_serial_num(&chassis)
                .map(crate::enumerate::format_serial)
                .unwrap_or_default(),
            chassis: None,
            simulated,
            id: chassis,
        });
    }

    let any_sim = out.iter().any(|d| d.simulated);
    let all_sim = !out.is_empty() && out.iter().all(|d| d.simulated);
    if any_sim && !all_sim {
        let sim: Vec<&str> = out
            .iter()
            .filter(|d| d.simulated)
            .map(|d| d.id.as_str())
            .collect();
        let real: Vec<&str> = out
            .iter()
            .filter(|d| !d.simulated)
            .map(|d| d.id.as_str())
            .collect();
        return Err(NiError::config(format!(
            "this task set MIXES driver-simulated and real devices - simulated: {sim:?}, \
             real: {real:?}. Refused: the simulated-device clock tier is only honest for a set \
             where NO physical clock and NO armature exist, and the SIMULATED data badge is a \
             property of the whole run, so a half-real set can neither carry it nor drop it. \
             Select an ALL-simulated set or an ALL-real set."
        )));
    }
    Ok((out, all_sim))
}

/// The DAQmx calls an on-demand zero write is made of. A trait so the sequence in
/// [`zero_ao_on_demand`] is provable on a host with no driver; [`Driver`]
/// implements it by delegating to the same entry points the backend uses
/// everywhere else, so no new binding was needed.
pub(crate) trait AoZeroOps {
    fn create_task(&self, name: &str) -> Result<Task, NiError>;
    fn create_ao_voltage_chan(
        &self,
        t: Task,
        physical: &str,
        min_v: f64,
        max_v: f64,
    ) -> Result<(), NiError>;
    fn start_task(&self, t: Task) -> Result<(), NiError>;
    fn write_analog_f64_raw(
        &self,
        t: Task,
        frames: i32,
        timeout_s: f64,
        data: &[f64],
    ) -> Result<(ffi::Status, i32), NiError>;
    fn check_raw(&self, status: ffi::Status, context: &str) -> Result<(), NiError>;
    fn stop_task(&self, t: Task) -> Result<(), NiError>;
    fn clear_task(&self, t: Task) -> Result<(), NiError>;
}

impl AoZeroOps for Driver {
    fn create_task(&self, name: &str) -> Result<Task, NiError> {
        Driver::create_task(self, name)
    }
    fn create_ao_voltage_chan(
        &self,
        t: Task,
        physical: &str,
        min_v: f64,
        max_v: f64,
    ) -> Result<(), NiError> {
        Driver::create_ao_voltage_chan(self, t, physical, min_v, max_v)
    }
    fn start_task(&self, t: Task) -> Result<(), NiError> {
        Driver::start_task(self, t)
    }
    fn write_analog_f64_raw(
        &self,
        t: Task,
        frames: i32,
        timeout_s: f64,
        data: &[f64],
    ) -> Result<(ffi::Status, i32), NiError> {
        Driver::write_analog_f64_raw(self, t, frames, timeout_s, data)
    }
    fn check_raw(&self, status: ffi::Status, context: &str) -> Result<(), NiError> {
        Driver::check_raw(self, status, context)
    }
    fn stop_task(&self, t: Task) -> Result<(), NiError> {
        Driver::stop_task(self, t)
    }
    fn clear_task(&self, t: Task) -> Result<(), NiError> {
        Driver::clear_task(self, t)
    }
}

/// Timeout for the one-frame on-demand write. An on-demand write completes as soon
/// as the driver has updated the outputs; the bound only stops a wedged driver from
/// holding the stop path.
const ZERO_WRITE_TIMEOUT_S: f64 = 1.0;

/// Drive `physical` to 0 V through a short-lived ON-DEMAND task named `name`:
/// create it, add each AO channel at `+/-range_v` (the main task's range), START
/// it, write ONE frame of zeros, then stop and clear it.
///
/// No sample clock is configured, so the task is on-demand: a write to it while it
/// RUNS updates the outputs immediately. It is started before the write because the
/// shared write binding passes `autoStart = false`, and DAQmx refuses a write to an
/// on-demand task that is neither running nor auto-started (-200473). The task is
/// stopped and cleared on EVERY path once it exists, so a refusal part-way leaves
/// no task (and no reservation) behind for the next arm to trip on.
///
/// The caller must have stopped and unreserved any task holding these channels.
pub(crate) fn zero_ao_on_demand<Z: AoZeroOps + ?Sized>(
    z: &Z,
    name: &str,
    physical: &[String],
    range_v: f64,
) -> Result<(), NiError> {
    let t = z.create_task(name)?;
    let written = (|| {
        for p in physical {
            z.create_ao_voltage_chan(t, p, -range_v, range_v)?;
        }
        z.start_task(t)?;
        let zeros = vec![0.0f64; physical.len()];
        let (status, written) = z.write_analog_f64_raw(t, 1, ZERO_WRITE_TIMEOUT_S, &zeros)?;
        // Negative is an error (returned); positive is a logged warning and the
        // write succeeded - the same status discipline as every other call.
        if status != 0 {
            z.check_raw(status, "DAQmxWriteAnalogF64 (on-demand zero)")?;
        }
        if written != 1 {
            return Err(NiError::config(format!(
                "the on-demand zero write accepted {written} of 1 frame"
            )));
        }
        Ok(())
    })();
    let _ = z.stop_task(t);
    let _ = z.clear_task(t);
    written
}

impl Drop for NiBackend {
    fn drop(&mut self) {
        // Best effort and panic-free: every call is `let _ =`, and `clear_tasks`
        // touches nothing that can panic.
        self.clear_tasks();
    }
}

/// Process-unique task-name suffix. DAQmx task names are PROCESS-GLOBAL, and this
/// daemon hosts SIX modules in one process: a parked backend held by any host keeps
/// its names alive, and a fixed "hertzforge_ai" collides with every other host's open
/// (-200089, measured on the chassis 2026-08-08 - the Sine acceptance's parked
/// backend blocked the shock probe's first NI-REAL arm). Nothing anywhere depends
/// on the literal name; instance-unique names delete the whole cross-host
/// collision class. Announced to the peer sessions as a shared-surface change.
fn task_name_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

impl Backend for NiBackend {
    /// Surface the driver's own diagnostics through the trait. These
    /// were previously reachable only on the concrete type, so an underrun
    /// DAQmx had already reported was invisible to every `dyn Backend` caller.
    fn take_warnings(&mut self) -> Vec<String> {
        self.drain_warnings()
    }

    fn caps(&self) -> Caps {
        Caps {
            // Hertzforge runs the control math; a DAQ card has no control DSP.
            loop_owner: LoopOwner::HostClosesLoop,
            // A DAQ card exposes no native test modes. Empty is the truth, and
            // `native_modes` is an evidence claim, not an aspiration.
            native_modes: vec![],
            clock: self.clock,
            max_input_channels: self.cfg.n_input_channels() as u32,
            max_output_channels: self.cfg.n_output_channels() as u32,
            // The PROVISIONAL `[H]` paper-derived bound once the tasks exist
            // (see `crate::clock::provisional_abort_latency` and
            // `abort_latency()` for the derivation and its evidence string);
            // `None` before that, and on any set whose formula inputs are
            // unavailable - in which case the arm gate refuses a drive path
            // rather than assuming. Arrival-day gate 5 REPLACES it with the
            // measured value; until then the physical E-stop is the boundary.
            abort_latency_bound: self.abort_latency.as_ref().map(|p| p.bound),
            output_buffer: if self.cfg.ao_assignments.is_empty() {
                None
            } else {
                Some(OutputBufferContract {
                    min_lookahead_frames: self.cfg.lookahead_frames(self.acq.block_frames) as u32,
                    underrun_policy: UnderrunPolicy::RampDownThenAbort,
                })
            },
            // The ACTUAL rate once opened; the request before that.
            sample_rate_hz: self.actual_rate_hz,
            // A DAQ card is HostClosesLoop: nothing is ever delegated to it, so
            // there is no delegated response path to claim. False is the
            // "claims nothing" posture and the resolver never reads it here.
            delegated_response_observable: false,
            // One group per CHASSIS in the task set, so a wide multi-chassis rig
            // can tell the operator WHICH chassis is unproven. The scalar
            // `clock` above stays the resolved answer for the whole set: it is
            // `resolve_clock`'s verdict over the single common chassis, and a
            // cross-chassis set has no common `ai/SampleClock` terminal at all,
            // so it never routes and never claims Shared. That makes the scalar
            // the conjunction, as `ClockGroup`'s contract requires.
            clock_groups: self.clock_group_breakdown(),
        }
    }

    fn open(&mut self) -> Result<(), HalError> {
        if self.state != State::Closed {
            return Err(HalError::Config(format!(
                "open() requires Closed state, was {:?}",
                self.state
            )));
        }
        if self.driver.is_none() {
            self.driver = Some(Driver::load().map_err(HalError::from)?);
        }
        self.build_tasks().map_err(HalError::from)?;
        self.state = State::Open;
        self.faulted = false;
        Ok(())
    }

    fn close(&mut self) -> Result<(), HalError> {
        self.clear_tasks();
        self.state = State::Closed;
        Ok(())
    }

    /// Re-derive the task plan from `table` + `acq` and rebuild the tasks.
    ///
    /// The physical-channel lists come from the CURRENT config (they were chosen
    /// by the operator or the scan), and everything else - which rows are
    /// enabled, their interface, sensitivity, and range - comes from the table.
    /// That keeps the channel table the single authority on measurement scaling
    /// while leaving the physical assignment where the operator put it.
    ///
    /// **This is the rate seam.** `acq` replaces whatever request was in force;
    /// the tasks are rebuilt; and the returned [`ConfiguredActual`] carries the
    /// rate the DRIVER programmed, which may differ from the request (DAQmx
    /// coerces to what the timebase can divide to). Nothing here resamples to
    /// hide that - the coercion is logged as a warning and reported here, and
    /// the arm gate's rate policy decides whether it is tolerable.
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        if self.state == State::Closed {
            return Err(HalError::NotConnected);
        }
        Self::check_acq(acq).map_err(HalError::from)?;
        table
            .validate()
            .map_err(|e| HalError::Config(format!("channel table invalid: {e}")))?;

        let inputs: Vec<String> = self
            .cfg
            .ai_assignments
            .iter()
            .map(|a| a.physical.clone())
            .collect();
        let outputs: Vec<String> = self
            .cfg
            .ao_assignments
            .iter()
            .map(|a| a.physical.clone())
            .collect();
        let fresh =
            crate::config::plan_from_table(table, &inputs, &outputs).map_err(HalError::from)?;

        self.cfg = NiConfig {
            ai_buffer_blocks: self.cfg.ai_buffer_blocks,
            lookahead_blocks: self.cfg.lookahead_blocks,
            ao_range_v: self.cfg.ao_range_v,
            ..fresh
        };
        self.acq = *acq;
        self.build_tasks().map_err(HalError::from)?;
        self.state = State::Configured;
        Ok(ConfiguredActual {
            actual_rate_hz: self.actual_rate_hz,
            block_frames: self.acq.block_frames,
            // The `[H]` provisional group delay, in frames, when the formula's
            // inputs exist. `None` is "not characterized", never "zero".
            group_delay_frames: self.abort_latency.as_ref().map(|p| p.group_delay_frames),
        })
    }

    fn arm(&mut self) -> Result<(), HalError> {
        match self.state {
            State::Open | State::Configured | State::Stopped => {
                self.require_ai().map_err(HalError::from)?;
                // Verify is advisory: it asks the driver to check the task's
                // configuration without reserving hardware. Absence is fine.
                if let (Ok(d), Some(ai)) = (self.drv(), self.ai) {
                    if let Err(e) = d.task_control(ai, val::TASK_VERIFY) {
                        self.warnings.push(format!("AI task verify skipped: {e}"));
                    }
                }
                // RE-COMMIT THE AO TASK - the symmetric partner of `stop()`'s
                // TASK_UNRESERVE, and the invariant every pre-start prime
                // depends on.
                //
                // `configure()` commits the AO for a stated reason: "so the
                // buffer is allocated and WriteSpaceAvail is meaningful before
                // start". `stop()` then UNRESERVES it (the zombie-AO fix), which
                // deallocates the buffer and makes `DAQmxGetWriteSpaceAvail`
                // fail - and nothing put the commit back, so from the first
                // `stop()` onward every ARMED task answered "I cannot tell you"
                // to the space query. `output_space()` renders that as `Some(0)`
                // (its deliberate conservative answer for a RUNNING task), which
                // a pre-start primer reads as FULL and so writes NOTHING; then
                // `start()` finds an empty buffer and tops it up with a full
                // lookahead of SILENCE. Measured 2026-08-08 on the cDAQ-9178:
                // every classical shock record played 2048 frames (40 ms) behind
                // a silence pad it did not ask for, bit-stable, on a firer whose
                // whole design is "the record is in front of the clock".
                //
                // Committing here is the same known-good sequence as first
                // bring-up (configure commits, then the caller writes, then
                // start), and it is what the driver would do implicitly at the
                // first write anyway - the difference is that the space query
                // works BEFORE that write instead of after it.
                if let (Ok(d), Some(ao)) = (self.drv(), self.ao) {
                    if let Err(e) = d.task_control(ao, val::TASK_COMMIT) {
                        self.warnings.push(format!(
                            "AO task commit at arm() failed ({e}); write-space queries will \
                             report UNKNOWN until the first write lands, and a caller that \
                             primes the buffer from the space query will prime nothing"
                        ));
                    }
                }
                // Last, after every check that can refuse: an arm that fails holds
                // nothing. Released by stop(), the abort, a rebuild, close or Drop.
                #[cfg(windows)]
                self.hold_host_loop_timer();
                self.state = State::Armed;
                Ok(())
            }
            other => Err(HalError::Config(format!(
                "arm() requires Open/Configured/Stopped, was {other:?}"
            ))),
        }
    }

    /// Start AO first, then AI.
    ///
    /// The AO task is clocked by the AI sample clock (when a chassis route
    /// exists), so starting it first parks it waiting on a clock that has not
    /// ticked yet; starting AI last releases both together. Whatever was written
    /// before this call is the prefill.
    fn start(&mut self) -> Result<(), HalError> {
        if self.state != State::Armed {
            return Err(HalError::Config(format!(
                "start() requires Armed, was {:?}",
                self.state
            )));
        }
        let ai = self.require_ai().map_err(HalError::from)?;
        // THE START PRECONDITION IS THE BACKEND'S TO SATISFY. A non-regenerating AO
        // refuses to start on an empty buffer (-200462), and the design that put
        // the prefill on callers (via OutputBufferContract) produced FIVE
        // independent copies of the same bug: recorder, random, then TWR, SRS,
        // classical shock and sine all fell to the identical refusal in the gate-6
        // sweep on the real chassis (2026-07-30). If fewer than the lookahead's
        // frames are queued at start, the backend tops the buffer up with SILENCE
        // itself - zeros are the one drive that is always safe, and a caller that
        // already prefilled real drive is left untouched (its queue already meets
        // the lookahead, or the remainder is silence behind its real samples).
        if self.ao.is_some() {
            let want = self.cfg.lookahead_frames(self.acq.block_frames);
            let queued = {
                let total = self.cfg.ao_buffer_frames(self.acq.block_frames);
                match self.query_write_space() {
                    Ok(space) => total.saturating_sub(space as usize),
                    // Unknown state: assume EMPTY and prefill fully. The query
                    // only fails on a task that is not committed (fresh, or
                    // unreserved by `stop()`), and both of those have an empty
                    // buffer - skipping the prefill there is the -200462
                    // empty-buffer start refusal. A committed task with real
                    // caller prefill answers the query and never lands here.
                    Err(_) => 0,
                }
            };
            if queued < want {
                let n_out = self.cfg.n_output_channels().max(1);
                let zeros = vec![0.0f32; (want - queued) * n_out];
                let mut offset = 0usize;
                while offset < zeros.len() {
                    match self.write_drive(&zeros[offset..]) {
                        Ok(0) | Err(_) => break,
                        Ok(frames) => offset += frames * n_out,
                    }
                }
                if ni_diag_enabled() {
                    eprintln!(
                        "[ni-diag] start(): AO buffer {} frames, queued {queued}, lookahead want \
                         {want} -> SILENCE PREFILLED {} frames AHEAD OF NOTHING (behind whatever \
                         is queued)",
                        self.cfg.ao_buffer_frames(self.acq.block_frames),
                        offset / n_out
                    );
                }
            } else if ni_diag_enabled() {
                eprintln!(
                    "[ni-diag] start(): AO buffer {} frames, queued {queued}, lookahead want \
                     {want} -> no silence prefill",
                    self.cfg.ao_buffer_frames(self.acq.block_frames)
                );
            }
        }
        {
            let d = self.drv().map_err(HalError::from)?;
            if let Some(ao) = self.ao {
                d.start_task(ao).map_err(HalError::from)?;
            }
            d.start_task(ai).map_err(HalError::from)?;
        }
        self.overload_latched = false;
        self.state = State::Running;
        self.absorb_driver_warnings();
        Ok(())
    }

    /// Stop AO first (drive off), then AI.
    fn stop(&mut self) -> Result<(), HalError> {
        if matches!(self.state, State::Running | State::Armed) {
            // Controlled stop: taper the drive out before killing the task. The
            // hardware holds its last sample when a task stops (see
            // `ramp_drive_to_zero`), so a bare stop would leave whatever the
            // drive happened to be at frozen on the wire.
            self.ramp_drive_to_zero(CONTROLLED_STOP_RAMP_MS);
            if let Ok(d) = self.drv() {
                if let Some(ao) = self.ao {
                    let _ = d.stop_task(ao);
                    // UNRESERVE, or the next start is a zombie: a stopped but
                    // still-reserved non-regenerating AO keeps its transfer
                    // position, and restarting it "succeeds" while consuming
                    // nothing - output_space() stays 0 and the engine dies on
                    // its staging deadman (measured 2026-08-03; first seen
                    // 2026-07-30 and dodged by never stopping). Unreserved,
                    // the restart's prefill + start is the same known-good
                    // path as first bring-up.
                    let _ = d.task_control(ao, val::TASK_UNRESERVE);
                }
                if let Some(ai) = self.ai {
                    let _ = d.stop_task(ai);
                    let _ = d.task_control(ai, val::TASK_UNRESERVE);
                }
            }
            // Only when the taper above was cut short; a completed taper skips it.
            if std::mem::take(&mut self.taper_cut_short) {
                self.zero_outputs_after_cut_taper();
            }
            self.state = State::Stopped;
        }
        // AFTER the taper and the task stops: the taper's generation wait polls
        // on a 200 us sleep, which is only 200 us while the hold is live.
        #[cfg(windows)]
        {
            self.host_loop_timer = None;
        }
        self.absorb_driver_warnings();
        Ok(())
    }

    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let n_ch = self.cfg.n_input_channels();
        if n_ch == 0 {
            return Err(HalError::Config("no input channels configured".into()));
        }
        let ai = self.require_ai().map_err(HalError::from)?;
        if !matches!(self.state, State::Running | State::Stopped) {
            return Err(HalError::NotConnected);
        }
        // Fixed-N. Never "read all available": a nondeterministic frame count
        // trips the engine's scratch guard.
        let frames = (out.len() / n_ch).min(self.acq.block_frames);
        if frames == 0 {
            return Err(HalError::Config(format!(
                "read_input buffer holds {} samples, smaller than one {n_ch}-channel frame",
                out.len()
            )));
        }
        let want = frames * n_ch;
        if self.read_scratch.len() < want {
            self.read_scratch.resize(want, 0.0);
        }
        let timeout = self.read_timeout_s(frames);

        let (status, got) = {
            // Field-level borrows: `driver` immutably, `read_scratch` mutably.
            let d = self.driver.as_ref().ok_or(HalError::NotConnected)?;
            d.read_analog_f64_raw(ai, frames as i32, timeout, &mut self.read_scratch[..want])
                .map_err(HalError::from)?
        };
        self.absorb_driver_warnings();

        if status < 0 {
            if ffi::is_timeout(status) {
                // The DOCUMENTED steady state of a timed fixed-N read: the block
                // was not ready. Not a fault.
                if got <= 0 {
                    return Ok(InputRead::NoDataYet);
                }
                // A partial block IS data. Fall through and deliver what arrived.
            } else {
                let d = self.drv().map_err(HalError::from)?;
                let err = d
                    .check_raw(status, "DAQmxReadAnalogF64")
                    .expect_err("a negative status is always an error");
                self.faulted = true;
                return Err(HalError::from(err));
            }
        }
        // status > 0 is a warning; `check_raw` logs it and the read succeeded.
        if status > 0 {
            let d = self.drv().map_err(HalError::from)?;
            let _ = d.check_raw(status, "DAQmxReadAnalogF64");
            self.absorb_driver_warnings();
        }

        let got = got.max(0) as usize;
        if got == 0 {
            return Ok(InputRead::NoDataYet);
        }
        // VOLTS -> engineering units, host-side, exactly once. Written as
        // `v * 1000 / sens` so it is bit-identical to `hertzforge_dsp::eu::volts_to_eu`
        // (the system's one EU conversion); the driver applied no sensitivity of
        // its own, which is why this is the only place it happens.
        for f in 0..got {
            let base = f * n_ch;
            for (c, sens) in self.sensitivity_mv_per_eu.iter().enumerate() {
                out[base + c] = (self.read_scratch[base + c] * 1000.0 / sens) as f32;
            }
        }
        self.frames_read += got as u64;
        self.poll_overload();
        Ok(InputRead::Frames(got))
    }

    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let n_ch = self.cfg.n_output_channels();
        if n_ch == 0 || self.ao.is_none() {
            return Err(HalError::Unsupported("no drive channels configured"));
        }
        let ao = self.ao.expect("checked above");
        let offered = drive.len() / n_ch;
        if offered == 0 {
            return Ok(0);
        }
        // Ask the device how much room it has. If the QUERY fails (some DAQmx
        // states refuse it before the buffer is committed), fall back to
        // offering everything and letting the write itself be the authority - a
        // zero-timeout write reports exactly what it accepted.
        let n = match self.query_write_space() {
            Ok(space) => offered.min(space as usize),
            Err(e) => {
                self.warnings.push(format!(
                    "write-space query failed ({e}); offering the whole block and letting the \
                     write report what it accepted"
                ));
                offered
            }
        };
        if n == 0 {
            return Ok(0);
        }
        let want = n * n_ch;
        if self.write_scratch.len() < want {
            self.write_scratch.resize(want, 0.0);
        }
        for (i, v) in drive[..want].iter().enumerate() {
            self.write_scratch[i] = *v as f64;
        }

        // Remember the last frame heading to the device: the shutdown taper has
        // to start from the value that is actually on the wire.
        if self.last_drive_frame.len() != n_ch {
            self.last_drive_frame = vec![0.0; n_ch];
        }
        self.last_drive_frame
            .copy_from_slice(&drive[(n - 1) * n_ch..n * n_ch]);

        let (status, written) = {
            let d = self.drv().map_err(HalError::from)?;
            // Timeout 0.0: try once, return immediately. Never block the run
            // thread on a full buffer.
            {
                // TEMP-DIAG 2026-07-30b (remove with the sweep closure)
                let r = d.write_analog_f64_raw(ao, n as i32, 0.0, &self.write_scratch[..want]);
                if ni_diag_enabled() {
                    match &r {
                        Ok((st, acc)) => eprintln!("[ni-diag] ao write offered={offered} space-limited-to={n} status={st} accepted={acc}"),
                        Err(e) => eprintln!("[ni-diag] ao write offered={offered} ERR: {e}"),
                    }
                }
                r
            }
                .map_err(HalError::from)?
        };
        self.absorb_driver_warnings();

        if written == 0 && n > 0 && !self.refuse_reported {
            self.refuse_reported = true;
            eprintln!(
                "AO REFUSING: write of {n} frames accepted 0 (status {status}) \
                 after the space query said at least that many would fit"
            );
        }
        if written > 0 {
            self.refuse_reported = false;
            // The age of THIS stamp when the buffer next reads zero-free
            // is what separates a host-side gap (underrun) from a generation
            // that died. Stamped on acceptance, not on the call.
            self.last_write_at = Some(std::time::Instant::now());
        }
        if status < 0 {
            if ffi::is_timeout(status) {
                // Buffer full: honest backpressure, not a fault.
                return Ok(written.max(0) as usize);
            }
            let d = self.drv().map_err(HalError::from)?;
            let err = d
                .check_raw(status, "DAQmxWriteAnalogF64")
                .expect_err("a negative status is always an error");
            self.faulted = true;
            return Err(HalError::from(err));
        }
        if status > 0 {
            let d = self.drv().map_err(HalError::from)?;
            let _ = d.check_raw(status, "DAQmxWriteAnalogF64");
            self.absorb_driver_warnings();
        }
        let written = written.max(0) as usize;
        self.frames_written += written as u64;
        Ok(written)
    }

    /// Free space in the device's output buffer, in FRAMES.
    ///
    /// Advisory. On a query failure this reports `Some(0)` (do not top up) and
    /// logs a warning, while [`Backend::write_drive`] falls back to offering the
    /// whole block. The asymmetry is deliberate: the conservative answer belongs
    /// in the planner, and the driver's own accept count belongs in the writer.
    fn output_space(&mut self) -> Option<usize> {
        // No output task means no independent output buffer, which is exactly
        // what `None` says in this trait.
        self.ao?;
        // The AO-generation diagnostics below are questions about a task that is SUPPOSED
        // to be generating. A PARKED task legitimately generates nothing and
        // legitimately fills to zero-free (that is what a pre-start prime IS), so
        // running them off a parked task produces "the output clock has stopped"
        // on a healthy shot - once per fire, in the middle of a passing gate.
        // An instrument that cries wolf is an instrument nobody reads, so it only
        // asks while the clock is actually running.
        let running = self.state == State::Running;
        match self.query_write_space() {
            Ok(v) if !running => {
                if ni_diag_enabled() {
                    eprintln!("[ni-diag] output_space -> {v} (task parked)");
                }
                Some(v as usize)
            }
            Ok(v) => {
                // A buffer reporting ZERO free is the symptom; the
                // question is whether it stopped draining because nobody FED it
                // (a host-side gap longer than the queue - underrun) or because
                // the generation itself died. Report the age of the last
                // accepted write against what the buffer can cover, ONCE per
                // stall, so the answer is in the log instead of being inferred.
                // GATE ON GENERATION, NOT ON SPACE. The first version of this
                // asked its question only when the buffer read zero-free, and it
                // never fired - because in the failing run the space query
                // reports ROOM while the write accepts nothing. Gating a
                // diagnostic on a symptom you assumed rather than measured is how
                // an instrument comes back silent and gets misread as "nothing
                // happened". The question is simply: is the device generating?
                let gen_now = self.ao.and_then(|t| {
                    self.drv()
                        .ok()
                        .and_then(|d| d.write_total_samp_per_chan_generated(t).ok())
                });
                match (self.last_gen, gen_now) {
                    (Some(prev), Some(now)) if now == prev => {
                        self.gen_stuck_polls = self.gen_stuck_polls.saturating_add(1);
                        if self.gen_stuck_polls == 8 {
                            eprintln!(
                                "AO NOT GENERATING: samples-generated frozen at \
                                 {now} across 8 consecutive polls while output_space \
                                 reports {v} frames free. The output clock has stopped."
                            );
                        }
                    }
                    (Some(prev), Some(now)) => {
                        if self.gen_stuck_polls >= 8 {
                            eprintln!(
                                "AO resumed generating (+{} frames)",
                                now.saturating_sub(prev)
                            );
                        }
                        self.gen_stuck_polls = 0;
                    }
                    _ => {}
                }
                if gen_now.is_some() {
                    self.last_gen = gen_now;
                }
                if v == 0 && !self.starve_reported {
                    self.starve_reported = true;
                    let since_ms = self
                        .last_write_at
                        .map(|t| t.elapsed().as_secs_f64() * 1e3)
                        .unwrap_or(f64::NAN);
                    let depth = self.cfg.ao_buffer_frames(self.acq.block_frames);
                    // The ACTUAL programmed rate, never the requested one: the 9234
                    // coerces, and a coverage figure computed at the request would
                    // misstate the very window this is measuring.
                    let covers_ms = depth as f64 / self.actual_rate_hz.max(1.0) * 1e3;
                    eprintln!(
                        "AO buffer hit ZERO free {since_ms:.0} ms after the last \
                         accepted write; the {depth}-frame queue covers {covers_ms:.0} ms \
                         at this rate"
                    );
                }
                if v > 0 {
                    self.starve_reported = false;
                }
                if ni_diag_enabled() {
                    eprintln!("[ni-diag] output_space -> {v}");
                }
                Some(v as usize)
            }
            Err(e) => {
                // `Some(0)` is the conservative answer for a RUNNING task ("do
                // not top up"), and it is INDISTINGUISHABLE from a full device.
                // Is what that ambiguity cost: a pre-start primer read it
                // as full, committed nothing, and every record played a whole
                // lookahead of `start()`-prefilled silence late. Callers priming
                // a PARKED buffer must treat this as the unknown and let
                // `write_drive`'s accept count answer - the warning says so.
                self.warnings.push(format!(
                    "write-space query failed ({e}); reporting NO SPACE, which on a parked \
                     task is the UNKNOWN and not a full device - prime by offering \
                     and reading the accept count"
                ));
                Some(0)
            }
        }
    }

    fn poll_status(&mut self) -> BackendStatus {
        self.absorb_driver_warnings();
        let warn_n = self.warnings.len();
        let recent = self.warnings.snapshot().last().cloned().unwrap_or_default();
        let overload = if !self.overload_pollable {
            "; ADC overload NOT CHECKED"
        } else if self.overload_latched {
            "; ADC OVERLOAD"
        } else {
            ""
        };
        let sim = if self.all_simulated {
            "; DRIVER-SIMULATED"
        } else {
            ""
        };
        let mut message = format!(
            "{:?}; {} Hz actual; clock {:?}; in {} / out {} ch; frames r/w {}/{}{}{}",
            self.state,
            self.actual_rate_hz,
            self.clock,
            self.cfg.n_input_channels(),
            self.cfg.n_output_channels(),
            self.frames_read,
            self.frames_written,
            sim,
            overload,
        );
        if warn_n > 0 {
            message.push_str(&format!("; {warn_n} warning(s), latest: {recent}"));
        }
        BackendStatus {
            connected: self.driver.is_some() && self.state != State::Closed,
            faulted: self.faulted,
            message,
            // In-band counter: the engine's timebase. A live plot with a flat
            // counter is a bug, and that is exactly what it is here for.
            sample_counter: self.frames_read,
            epoch: self.epoch,
        }
    }

    /// Emergency ramp-down and safe stop.
    ///
    /// Tapers the drive to zero over [`ABORT_RAMP_MS`], then stops AO and AI.
    /// Never issues any firmware/flash operation; DAQmx has none to issue.
    ///
    /// **Corrected 2026-08-01 by measurement.** This used to write ONE block of
    /// zeros and then stop the task, documenting the task stop as "the
    /// authoritative drive kill". The bench refuted both halves:
    ///
    ///  - Stopping the AO task does NOT kill the drive. The device holds its
    ///    last commanded sample. Gate 5 tripped at 0.500 V and left 0.5135 V
    ///    driving the output for the remaining 3 s of the capture - and it was
    ///    still there when the next run opened the device, across process exit.
    ///  - The zero block could not have helped anyway: it is APPENDED, so it
    ///    queues behind the drive already buffered, and the task was stopped
    ///    before those zeros were ever clocked out. It was unreachable code
    ///    wearing the label of the mechanism.
    ///
    /// The taper below is therefore the ONLY thing that brings the drive to zero
    /// on this hardware. `AO.IdleOutputBehavior = ZeroVolts` is still requested
    /// at configure time as a backstop for devices that implement it, but **the
    /// NI 9263 does not**: DAQmx answers `-200452, property not supported by the
    /// device` for ZeroVolts and HighImpedance alike (measured 2026-08-01,
    /// directly against the driver). Do not rely on it here.
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.ramp_drive_to_zero(ABORT_RAMP_MS);
        if let Ok(d) = self.drv() {
            if let Some(ao) = self.ao {
                let _ = d.stop_task(ao);
                // UNRESERVE, for exactly the reason `stop()` above does
                // (measured 2026-08-03): a stopped but still-RESERVED
                // non-regenerating AO keeps its transfer position, and the next
                // start "succeeds" while consuming nothing - output_space()
                // stays 0 and the run dies on its staging deadman. An abort
                // that leaves that zombie behind makes abort a ONE-SHOT: the
                // re-arm after it can never fire. Same two lines, same order,
                // same rationale as the controlled stop; the only difference is
                // the ramp length above it.
                //
                // CHASSIS VERIFICATION: the `abort -> re-arm -> fire` bench check
                // (PASS twice, 2026-08-08) is what proves this on real iron - it is
                // not proven by any test in this crate, which has no driver.
                let _ = d.task_control(ao, val::TASK_UNRESERVE);
            }
            if let Some(ai) = self.ai {
                let _ = d.stop_task(ai);
                let _ = d.task_control(ai, val::TASK_UNRESERVE);
            }
        }
        // Only when the taper above was cut short (same rule as `stop()`).
        if std::mem::take(&mut self.taper_cut_short) {
            self.zero_outputs_after_cut_taper();
        }
        // CLEAR both tasks as well: unreserve frees the HARDWARE but not the
        // task NAMES, and DAQmx task names are process-global - a fresh station
        // built while an aborted backend is still alive dies at
        // DAQmxCreateTask(hertzforge_ai) with -200089 (measured on the chassis at
        // an abort->re-arm->fire check, 2026-08-08). An abort is terminal for
        // this backend (the service rebuilds through the factory), so releasing
        // the names here is the abort SEMANTICS, not a convenience. Same call
        // close() and Drop use; idempotent with the later Drop.
        self.clear_tasks();
        if matches!(self.state, State::Running | State::Armed) {
            self.state = State::Stopped;
        }
        self.absorb_driver_warnings();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AiAssign, AoAssign};
    use crate::ffi::Task;

    fn ai(n: usize) -> Vec<AiAssign> {
        (0..n)
            .map(|i| AiAssign {
                physical: format!("Mod1/ai{i}"),
                interface: AiInterface::Iepe,
                sensitivity_mv_per_eu: 100.0,
                max_eu: 10.0,
                table_index: Some(i as u32 + 1),
            })
            .collect()
    }

    fn ao(n: usize) -> Vec<AoAssign> {
        (0..n)
            .map(|i| AoAssign {
                physical: format!("Mod2/ao{i}"),
                table_index: None,
            })
            .collect()
    }

    fn cfg(n_in: usize, n_out: usize) -> NiConfig {
        NiConfig::new(ai(n_in), ao(n_out))
    }

    /// The standard test acquisition request: 51.2 kHz, 2048-frame blocks.
    fn acq() -> AcquisitionConfig {
        AcquisitionConfig::new(51_200.0, 2048)
    }

    fn backend(n_in: usize, n_out: usize) -> NiBackend {
        NiBackend::new(cfg(n_in, n_out), acq())
    }

    #[test]
    fn caps_before_open_are_the_honest_unopened_posture() {
        let b = backend(4, 2);
        let c = b.caps();
        assert_eq!(c.loop_owner, LoopOwner::HostClosesLoop);
        assert!(
            c.native_modes.is_empty(),
            "a DAQ card exposes no native test modes"
        );
        // Nothing has been proven, so nothing is claimed.
        assert_eq!(c.clock, ClockDomain::Unknown);
        assert_eq!(c.abort_latency_bound, None);
        assert_eq!(c.max_input_channels, 4);
        assert_eq!(c.max_output_channels, 2);
        // Before open the reported rate is the REQUEST; after open it is the
        // driver's coerced answer.
        assert_eq!(c.sample_rate_hz, 51_200.0);
        assert!(b.clock_evidence().contains("not determined"));
    }

    #[test]
    fn output_buffer_contract_is_the_configured_lookahead() {
        let b = backend(1, 1);
        let contract = b.caps().output_buffer.expect("drive configured");
        assert_eq!(contract.min_lookahead_frames, 2048 * 2);
        assert_eq!(contract.underrun_policy, UnderrunPolicy::RampDownThenAbort);
    }

    #[test]
    fn a_capture_only_config_advertises_no_output_buffer() {
        let b = backend(2, 0);
        assert_eq!(b.caps().output_buffer, None);
        assert_eq!(b.caps().max_output_channels, 0);
    }

    #[test]
    fn read_timeout_is_two_block_periods_with_a_floor() {
        let b = backend(1, 0);
        // 2048 frames at 51200 Hz is 40 ms; two of them is 80 ms.
        assert!((b.read_timeout_s(2048) - 0.08).abs() < 1e-9);
        // A one-frame block would be 39 us, which the floor lifts to 10 ms.
        assert_eq!(b.read_timeout_s(1), MIN_READ_TIMEOUT_S);
    }

    #[test]
    fn counters_start_at_zero_and_status_is_disconnected_before_open() {
        let mut b = backend(2, 1);
        let s = b.poll_status();
        assert!(!s.connected);
        assert!(!s.faulted);
        assert_eq!(s.sample_counter, 0);
        assert_eq!(b.frames_read(), 0);
        assert_eq!(b.frames_written(), 0);
        // The message carries the in-band facts even before a driver exists.
        assert!(s.message.contains("Closed"), "{}", s.message);
        assert!(s.message.contains("frames r/w 0/0"), "{}", s.message);
    }

    #[test]
    fn the_read_path_carries_per_channel_sensitivity_not_a_precomputed_factor() {
        // Two rows with different front ends and different sensitivities: the
        // read path converts with the SENSITIVITY, host-side, so the front-end
        // choice cannot change the scaling.
        let mut assigns = ai(2);
        assigns[0].sensitivity_mv_per_eu = 10.25;
        assigns[1].interface = AiInterface::Voltage;
        assigns[1].sensitivity_mv_per_eu = 1000.0;
        let b = NiBackend::new(
            NiConfig::new(assigns, vec![]),
            AcquisitionConfig::new(1000.0, 64),
        );
        assert_eq!(b.sensitivity_mv_per_eu, vec![10.25, 1000.0]);
        // And it is the same arithmetic the DSP layer uses.
        for (i, sens) in b.sensitivity_mv_per_eu.iter().enumerate() {
            assert_eq!(
                b.config().ai_assignments[i].to_eu(0.5),
                hertzforge_dsp::eu::volts_to_eu(0.5, *sens)
            );
        }
    }

    #[test]
    fn front_end_readbacks_are_empty_until_the_tasks_are_built() {
        let b = backend(2, 0);
        assert!(
            b.front_ends().is_empty(),
            "nothing has been programmed, so nothing may be reported as programmed"
        );
    }

    // --- driver-absent behavior (the macOS dev-host path) --------------------

    #[cfg(target_os = "macos")]
    #[test]
    fn open_on_a_host_with_no_driver_fails_honestly() {
        let mut b = backend(4, 2);
        let e = b.open().expect_err("no NI driver exists on macOS");
        match e {
            // The HAL gained a variant that says exactly this, and it owns
            // a String, so the loader diagnosis reaches the caller intact
            // (the first version mapped it to `Unsupported(&'static str)` and lost
            // the detail).
            HalError::DriverNotAvailable(s) => {
                assert!(s.contains("not available"), "{s}");
                assert!(s.contains("macOS"), "the loader detail must survive: {s}");
            }
            other => panic!("expected DriverNotAvailable, got {other:?}"),
        }
        // The full loader diagnosis (which library names were tried, and why
        // macOS was refused outright) is still reachable from the NiError that
        // `Driver::load()` returns; only the HAL-level hop narrows it.
        let raw = crate::ffi::Driver::load().expect_err("macOS has no DAQmx");
        assert!(raw.to_string().contains("macOS"), "{raw}");
        // And it did NOT quietly come up anyway.
        assert!(!b.poll_status().connected);
        assert_eq!(b.caps().clock, ClockDomain::Unknown);
    }

    #[test]
    fn read_before_open_is_not_connected() {
        let mut b = backend(2, 0);
        let mut buf = vec![0.0f32; 2 * 64];
        let e = b.read_input(&mut buf).expect_err("not open");
        assert!(
            matches!(e, HalError::NotConnected | HalError::Config(_)),
            "{e:?}"
        );
    }

    #[test]
    fn write_drive_on_a_capture_only_config_is_unsupported() {
        let mut b = backend(2, 0);
        let e = b.write_drive(&[0.0]).expect_err("no drive channels");
        assert!(matches!(e, HalError::Unsupported(_)), "{e:?}");
        assert_eq!(b.output_space(), None);
    }

    #[test]
    fn lifecycle_order_is_enforced() {
        let mut b = backend(1, 0);
        // start() before arm() is refused regardless of driver state.
        assert!(b.start().is_err());
        // arm() before open() is refused: there is no task.
        assert!(b.arm().is_err());
        // configure() before open() is NotConnected.
        let t = hertzforge_channel_table::ChannelTable::default_with_channels(1);
        assert!(matches!(
            b.configure(&t, &acq()),
            Err(HalError::NotConnected)
        ));
    }

    #[test]
    fn close_is_safe_from_any_state_and_drops_the_tasks() {
        let mut b = backend(1, 1);
        b.close().unwrap();
        b.close().unwrap();
        assert!(b.ai.is_none() && b.ao.is_none());
    }

    #[test]
    fn emergency_stop_is_safe_with_no_driver_and_never_panics() {
        let mut b = backend(2, 1);
        b.emergency_stop().unwrap();
        assert_eq!(
            b.state,
            State::Closed,
            "e-stop must not invent a live state"
        );
    }

    #[test]
    fn a_backend_is_send_so_the_engine_can_own_it_on_its_run_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<NiBackend>();
        // ...and it satisfies the object-safe trait the engine holds.
        let b: Box<dyn Backend> = Box::new(backend(1, 0));
        assert_eq!(b.caps().max_input_channels, 1);
    }

    #[test]
    fn warnings_accumulate_and_drain() {
        let b = backend(1, 0);
        assert!(b.warnings().is_empty());
        b.warnings.push("first");
        b.warnings.push("second");
        assert_eq!(b.warnings().len(), 2);
        assert_eq!(b.drain_warnings().len(), 2);
        assert!(b.warnings().is_empty());
    }

    #[test]
    fn poll_status_surfaces_the_warning_count_and_the_latest_text() {
        let mut b = backend(1, 0);
        b.warnings.push("sample rate coerced to 51200");
        let s = b.poll_status();
        assert!(s.message.contains("1 warning(s)"), "{}", s.message);
        assert!(s.message.contains("coerced"), "{}", s.message);
    }

    #[test]
    fn the_rate_and_block_ride_the_acquisition_request_not_the_channel_plan() {
        // NiConfig is the channel/front-end plan; it has no rate at all now.
        let b = NiBackend::new(cfg(2, 1), AcquisitionConfig::new(25_600.0, 512));
        assert_eq!(b.acquisition().requested_rate_hz, 25_600.0);
        assert_eq!(b.acquisition().block_frames, 512);
        // ...and the lookahead contract is derived from THAT block size.
        let contract = b.caps().output_buffer.expect("drive configured");
        assert_eq!(contract.min_lookahead_frames, 512 * 2);
    }

    // --- the cut-taper zero write -------------------------------------------

    /// A backend that LOOKS mid-run with drive on the wire and no driver: the shape
    /// of a stalled play, where every taper write fails. `ao` is a placeholder
    /// handle that is never dereferenced (every driver call fails first).
    fn mid_run_with_drive(n_out: usize) -> NiBackend {
        let mut b = backend(1, n_out);
        b.ao = Some(Task(std::ptr::null_mut()));
        b.state = State::Running;
        b.last_drive_frame = vec![-0.03; n_out];
        b
    }

    /// The stalled-play shape reaches the zero step on BOTH stop paths, after the
    /// taper reports itself cut short, and the flag is consumed. With no driver the
    /// zero write cannot happen, and the warning says the output may be holding.
    ///
    /// RED-MUTATION: drop either `taper_cut_short = true` in `ramp_drive_to_zero`'s
    /// incomplete branches, or the `mem::take` check in `stop()` /
    /// `emergency_stop()`, and the "could NOT be zeroed" assertion reds.
    #[test]
    fn a_cut_taper_is_followed_by_a_zero_attempt_on_stop_and_on_abort() {
        for abort in [false, true] {
            let mut b = mid_run_with_drive(2);
            if abort {
                b.emergency_stop().unwrap();
            } else {
                b.stop().unwrap();
            }
            let w = b.warnings().join(" | ");
            assert!(
                w.contains("shutdown taper incomplete"),
                "abort={abort}: {w}"
            );
            assert!(
                w.contains("could NOT be zeroed") && w.contains("Mod2/ao0, Mod2/ao1"),
                "abort={abort}: {w}"
            );
            assert!(!b.taper_cut_short, "the flag is consumed by the stop path");
            assert_eq!(b.state, State::Stopped);
        }
    }

    /// No drive was ever written, so there is no taper and nothing is zeroed: the
    /// zero step belongs to a CUT taper only, never to every stop.
    #[test]
    fn a_stop_with_no_drive_to_taper_never_attempts_the_zero_write() {
        let mut b = mid_run_with_drive(1);
        b.last_drive_frame = vec![0.0];
        b.stop().unwrap();
        let w = b.warnings().join(" | ");
        assert!(!w.contains("taper"), "{w}");
        assert!(!w.contains("zero"), "{w}");
    }

    /// Records the DAQmx calls `zero_ao_on_demand` makes, failing the step named in
    /// `fail` (and answering the write with `write`).
    struct ZeroOps {
        calls: std::cell::RefCell<Vec<String>>,
        fail: Option<&'static str>,
        write: (ffi::Status, i32),
    }

    impl ZeroOps {
        fn new(fail: Option<&'static str>, write: (ffi::Status, i32)) -> Self {
            ZeroOps {
                calls: std::cell::RefCell::new(Vec::new()),
                fail,
                write,
            }
        }
        fn step(&self, what: &'static str, detail: String) -> Result<(), NiError> {
            self.calls.borrow_mut().push(format!("{what}{detail}"));
            if self.fail == Some(what) {
                return Err(NiError::Daqmx {
                    code: -50103,
                    text: "the specified resource is reserved".into(),
                    context: what.into(),
                });
            }
            Ok(())
        }
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl AoZeroOps for ZeroOps {
        fn create_task(&self, name: &str) -> Result<Task, NiError> {
            self.step("create", format!(" {name}"))?;
            Ok(Task(std::ptr::null_mut()))
        }
        fn create_ao_voltage_chan(
            &self,
            _t: Task,
            physical: &str,
            min_v: f64,
            max_v: f64,
        ) -> Result<(), NiError> {
            self.step("chan", format!(" {physical} {min_v}..{max_v}"))
        }
        fn start_task(&self, _t: Task) -> Result<(), NiError> {
            self.step("start", String::new())
        }
        fn write_analog_f64_raw(
            &self,
            _t: Task,
            frames: i32,
            timeout_s: f64,
            data: &[f64],
        ) -> Result<(ffi::Status, i32), NiError> {
            self.step("write", format!(" {frames} {timeout_s} {data:?}"))?;
            Ok(self.write)
        }
        fn check_raw(&self, status: ffi::Status, context: &str) -> Result<(), NiError> {
            if status < 0 {
                Err(NiError::Daqmx {
                    code: status,
                    text: String::new(),
                    context: context.into(),
                })
            } else {
                Ok(())
            }
        }
        fn stop_task(&self, _t: Task) -> Result<(), NiError> {
            self.step("stop", String::new())
        }
        fn clear_task(&self, _t: Task) -> Result<(), NiError> {
            self.step("clear", String::new())
        }
    }

    fn two_ao() -> Vec<String> {
        vec!["cDAQ1Mod2/ao0".into(), "cDAQ1Mod2/ao1".into()]
    }

    /// The whole sequence: an on-demand task on every AO channel at the main
    /// task's range, STARTED before its one-frame zero write (the shared binding
    /// passes autoStart = false), then stopped and cleared.
    ///
    /// RED-MUTATION: move the start after the write, write a non-zero frame, or
    /// drop the clear, and the sequence assertion reds.
    #[test]
    fn the_zero_write_is_one_frame_of_zeros_on_a_started_on_demand_task() {
        let z = ZeroOps::new(None, (0, 1));
        zero_ao_on_demand(&z, "hertzforge_ao_zero_7", &two_ao(), 10.0).expect("zeroed");
        assert_eq!(
            z.calls(),
            vec![
                "create hertzforge_ao_zero_7",
                "chan cDAQ1Mod2/ao0 -10..10",
                "chan cDAQ1Mod2/ao1 -10..10",
                "start",
                "write 1 1 [0.0, 0.0]",
                "stop",
                "clear",
            ]
        );
    }

    /// Every refusal after the task exists still stops and clears it, so a failed
    /// zero leaves no reservation for the next arm to trip on; a refusal to create
    /// the task leaves nothing to clear. A write that accepted nothing, or answered
    /// a negative status, is an error, never a silent "zeroed".
    #[test]
    fn a_refused_zero_write_reports_and_never_leaves_its_task_behind() {
        for fail in ["chan", "start", "write"] {
            let z = ZeroOps::new(Some(fail), (0, 1));
            let e = zero_ao_on_demand(&z, "t", &two_ao(), 10.0).expect_err(fail);
            assert_eq!(e.daqmx_code(), Some(-50103), "{fail}");
            let calls = z.calls();
            assert_eq!(&calls[calls.len() - 2..], ["stop", "clear"], "{fail}");
        }
        let z = ZeroOps::new(Some("create"), (0, 1));
        assert!(zero_ao_on_demand(&z, "t", &two_ao(), 10.0).is_err());
        assert_eq!(z.calls(), vec!["create t"]);

        for write in [(-200473, 0), (0, 0)] {
            let z = ZeroOps::new(None, write);
            assert!(
                zero_ao_on_demand(&z, "t", &two_ao(), 10.0).is_err(),
                "{write:?}"
            );
            assert_eq!(z.calls().last().map(String::as_str), Some("clear"));
        }
        // A positive status is a driver WARNING and the write succeeded.
        let z = ZeroOps::new(None, (200_000, 1));
        assert!(zero_ao_on_demand(&z, "t", &two_ao(), 10.0).is_ok());
    }

    #[test]
    fn caps_declare_no_delegated_response_and_no_groups_before_open() {
        let c = backend(2, 1).caps();
        assert!(
            !c.delegated_response_observable,
            "a host-loop DAQ card delegates nothing and must claim nothing"
        );
        assert!(
            c.clock_groups.is_empty(),
            "no devices have been read, so there is no breakdown to report"
        );
    }
}

#[cfg(test)]
mod identity_tests {
    //! Device-identity + mixed-set tests, driven through the canned attribute
    //! source (`enumerate::SysQuery`). No driver, no hardware: the refusal is a
    //! pure function of what the attribute layer reports.
    use super::*;
    use crate::enumerate::SysQuery;
    use std::collections::HashMap;

    /// A canned attribute source. Only the fields identity reads are modeled;
    /// everything else degrades exactly as the real path does.
    #[derive(Default)]
    struct Attrs {
        simulated: HashMap<&'static str, bool>,
        product: HashMap<&'static str, &'static str>,
        chassis: HashMap<&'static str, &'static str>,
    }

    fn absent(what: &str, dev: &str) -> NiError {
        NiError::Daqmx {
            code: -200220,
            text: format!("attribute {what} is not supported by device {dev}"),
            context: what.into(),
        }
    }

    impl SysQuery for Attrs {
        fn sys_dev_names(&self) -> Result<String, NiError> {
            Err(absent("SysDevNames", "system"))
        }
        fn dev_product_type(&self, dev: &str) -> Result<String, NiError> {
            self.product
                .get(dev)
                .map(|s| s.to_string())
                .ok_or_else(|| absent("ProductType", dev))
        }
        fn dev_serial_num(&self, dev: &str) -> Result<u32, NiError> {
            Err(absent("SerialNum", dev))
        }
        fn dev_is_simulated(&self, dev: &str) -> Result<bool, NiError> {
            self.simulated
                .get(dev)
                .copied()
                .ok_or_else(|| absent("IsSimulated", dev))
        }
        fn dev_chassis_name(&self, dev: &str) -> Result<String, NiError> {
            self.chassis
                .get(dev)
                .map(|s| s.to_string())
                .ok_or_else(|| absent("CompactDAQChassisDevName", dev))
        }
        fn dev_slot_num(&self, dev: &str) -> Result<u32, NiError> {
            Err(absent("CompactDAQSlotNum", dev))
        }
        fn dev_ai_physical_chans(&self, dev: &str) -> Result<String, NiError> {
            Err(absent("AIPhysicalChans", dev))
        }
        fn dev_ao_physical_chans(&self, dev: &str) -> Result<String, NiError> {
            Err(absent("AOPhysicalChans", dev))
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// RED-MUTATION: delete the `any_sim && !all_sim` refusal in
    /// `read_identity_with` (or soften it to a warning) and this test reds - a
    /// half-simulated set would build tasks, and its clock domain would then be
    /// decided by tier 1's "no physical clock exists, no armature can be
    /// attached" reasoning while a real module sat in the chassis.
    #[test]
    fn a_mixed_real_and_simulated_task_set_is_refused_at_configure() {
        let attrs = Attrs {
            simulated: [("Mod1", true), ("Mod2", false)].into_iter().collect(),
            product: [("Mod1", "NI-9234"), ("Mod2", "NI-9263")]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let e = read_identity_with(&attrs, &ids(&["Mod1", "Mod2"]))
            .expect_err("a mixed set must be refused");
        let text = e.to_string();
        assert!(text.contains("MIXES"), "{text}");
        // The refusal must NAME both sides so the operator can fix the selection.
        assert!(text.contains("Mod1"), "{text}");
        assert!(text.contains("Mod2"), "{text}");
        assert!(matches!(e, NiError::Config(_)), "{e:?}");
    }

    #[test]
    fn an_all_simulated_set_passes_identity_and_reports_all_simulated() {
        let attrs = Attrs {
            simulated: [("Mod1", true), ("Mod2", true)].into_iter().collect(),
            chassis: [("Mod1", "cDAQ9178Sim"), ("Mod2", "cDAQ9178Sim")]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let (devices, all_sim) =
            read_identity_with(&attrs, &ids(&["Mod1", "Mod2"])).expect("an all-sim set is legal");
        assert!(all_sim);
        // 2 modules + their PARENT CHASSIS row (added 2026-07-30: the clock key
        // needs the chassis PRODUCT; the first real gate 3 printed chassis="").
        // The chassis inherits its residents' simulated flag when the query
        // cannot answer for it, so all_sim holds.
        assert_eq!(devices.len(), 3);
        assert!(devices.iter().all(|d| d.simulated));
        assert_eq!(devices[0].chassis.as_deref(), Some("cDAQ9178Sim"));
        let chassis_row = devices
            .iter()
            .find(|d| d.id == "cDAQ9178Sim")
            .expect("the parent chassis joins the identity map");
        assert!(chassis_row.simulated, "inherited from its resident modules");
    }

    #[test]
    fn an_all_real_set_passes_identity_and_reports_not_all_simulated() {
        let attrs = Attrs {
            simulated: [("Mod1", false), ("Mod2", false)].into_iter().collect(),
            ..Default::default()
        };
        let (devices, all_sim) =
            read_identity_with(&attrs, &ids(&["Mod1", "Mod2"])).expect("an all-real set is legal");
        assert!(!all_sim, "a real set must never claim the simulated tier");
        assert!(devices.iter().all(|d| !d.simulated));
    }

    #[test]
    fn an_unreadable_simulated_flag_refuses_rather_than_guessing() {
        // `IsSimulated` absent for Mod2: the badge and the clock domain would
        // both be guesses, so nothing is built.
        let attrs = Attrs {
            simulated: [("Mod1", true)].into_iter().collect(),
            ..Default::default()
        };
        let e = read_identity_with(&attrs, &ids(&["Mod1", "Mod2"])).expect_err("must refuse");
        assert!(e.to_string().contains("simulated flag"), "{e}");
    }

    #[test]
    fn ao_routing_rides_the_chassis_ai_sample_clock() {
        assert_eq!(
            derive_ao_routing(Some("cDAQ1"), false),
            "/cDAQ1/ai/SampleClock"
        );
        assert_eq!(derive_ao_routing(None, false), "");
    }

    #[test]
    fn force_unrouted_strips_the_route_even_with_a_chassis() {
        // The clock_loopback negative control: if this ever silently kept the
        // route, the unrouted run would be measuring the SAME clock as the
        // routed run and the arrival-day drift check would prove nothing.
        assert_eq!(derive_ao_routing(Some("cDAQ1"), true), "");
    }
}
