//! The persistent closed-loop sine engine seam (the load-bearing decision).
//!
//! [`SineProcessStep`] implements [`hertzforge_engine::ProcessStep`] and is PERSISTENT
//! across engine chunks. It owns everything the per-frame + per-control-tick loop
//! needs and is the ONLY object the service hands to `Engine::run`. The service
//! runs the engine in short chunks, re-arming the Sim backend per chunk (the Sim
//! plant state persists across chunks), draining the status snapshot and the
//! swept trace between chunks, and posting operator commands into the lock-free
//! mailbox that `process()` drains at each control tick.
//!
//! # The loop shape, exactly
//!
//! Per FRAME inside `process()`:
//! 1. `drive = nco.next()` gives `(sin, inst_freq, phase)`; the control drive
//!    sample is `sin * current_drive_scale`.
//! 2. Write the drive frame to `drive_out`: ch0 = control drive; ch1 = the COLA
//!    aux (constant amplitude at the swept frequency) when COLA is enabled - the
//!    width matches `EngineConfig.out_channels`.
//! 3. Demodulate EVERY input channel through its [`AdaptiveTrackingFilter`] at
//!    `inst_freq`; feed the control channel's sample into the [`ThdBank`].
//! 4. Accumulate a broadband sum-of-squares per input channel.
//! 5. Every `control_tick_frames` frames run one CONTROL TICK.
//!
//! Per CONTROL TICK:
//! - Advance the scheduler by the tick's `dt` (test seconds), getting the new
//!   frequency command; retune the NCO and every tracking filter's center.
//! - Composite the control channels' tracked amplitudes.
//! - Normalize: `feed = measured_composite / (profile_target(f) * rung_scale)`.
//!   Run `servo.update` (or `servo.hold_tick` while the notch binds) to get a new
//!   drive scale; derive `servo_step_db` from the before/after scale.
//! - `BlendLaw::step` adds the `w`-scaled feedforward slope and clamps
//!   the total per-tick move; apply it to the working drive scale.
//! - Observe the limit channels and apply the [`NotchGovernor`] ceiling.
//! - Guard the drive Vpk; run the scalar alarm/abort at `f`; run
//!   the analytic kinematic monitor; run the debounced servo-fault
//!   check. Any abort sets the [`AbortLatch`] and starts a drive
//!   ramp-down (never a cliff).
//! - Push a [`ControlSnapshot`] into the shared status cell and a trace point per
//!   channel into the trace log.
//!
//! The engine's ACCEL-axis watchdog stays live (its raw-ring accel abort is
//! correct); this module's ANALYTIC kinematic monitor replaces the watchdog's
//! vel/disp axes, which reset per chunk and under-read at low `f`.

use crate::control::{
    composite_amplitude, tracked_displacement_in_pp, tracked_velocity_ips_pk, BlendLaw,
    CompositeMode, DriveGuard, DriveVerdict, FeedforwardTable, LimitChannelTick, NotchGovernor,
};
use crate::law::servo_blend::{self, ServoBlendLaw};
use crate::law::{
    Hold, LawClock, LawDescriptor, LawFault, LawGuards, LawValues, SineControlLaw, SineOutputRule,
    TickInputs, TickOutputs, FIRST_DRIVE_MARGIN_DB, FIRST_DRIVE_UNTRUSTED_FRACTION,
};
use crate::profile::SineProfile;
use crate::scheduler::{
    BidirSweep, DwellRun, LadderPlan, RstdEvent, RstdPlan, StepSine, SweepDirection,
};
use crate::service::config::ColaWaveform;
use crate::service::display::{DisplayCell, DisplayLog, DisplayRow, DisplaySlots};
use crate::service::law_records::{miss_code, LawRecord, LawRecordRing, LAW_RECORD_CAPACITY};
use crate::service::report::{TripKind, TripRecord};
use crate::service::ring::SampleRing;
use hertzforge_dsp::sine::AmplitudeServo;
use hertzforge_dsp::sweep::SweepOscillator;
use hertzforge_dsp::tracking::{AdaptiveTrackingFilter, BandwidthMode, ThdBank};
use hertzforge_engine::{ProcessStep, TimebaseEvent};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// An operator command posted into the lock-free mailbox. `process()`
/// drains the mailbox at every control tick, so a STOP is honored within one
/// engine chunk.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// End the run (the outer service loop stops after this chunk).
    Stop,
    /// Hold the frequency scheduler (output continues; progress pauses).
    Hold,
    /// Resume from a hold.
    Resume,
    /// Reverse the sweep direction (sweep mode).
    Reverse,
    /// Nudge the frequency by `delta_hz` (sweep mode), clamped to the band.
    Jog(f64),
    /// Command an ABSOLUTE frequency (sweep mode), clamped to the band.
    ///
    /// Not expressible as a `Jog`: a relative nudge needs the caller to subtract
    /// a frequency it read at some earlier moment, and a sweep moves in between.
    /// The service refuses this outside Sweep by name rather than posting a
    /// command the other schedulers would silently drop.
    GoTo(f64),
    /// Step the ladder level by `delta_db`.
    StepLevel(f64),
    /// Jump to ladder rung index `i`.
    JumpLevel(usize),
    /// Jump to the rung NEAREST `db` (the SetLevel{Db} resolution rule;
    /// [`crate::scheduler::LadderPlan::nearest_rung`] is the one expression of
    /// it). Out-of-table requests land on the end rung they are nearest.
    SetLevelDb(f64),
    /// AUTO / MANUAL. MANUAL suppresses the ladder's own advance; the operator
    /// owns every level move.
    SetRunMode(RunMode),
    /// Change the law's live parameter number `index` (in its declared live
    /// list) to `value`, already checked against its range. A law that
    /// refuses keeps the value in force.
    Adjust { index: usize, value: f64 },
}

/// Who advances the level ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RunMode {
    /// The ladder advances itself when a rung's rule is met.
    Auto,
    /// The ladder holds; the operator steps or jumps it.
    Manual,
}

/// A lock-free-drain command mailbox: the service pushes, `process()` drains at
/// control ticks. A `Mutex<Vec>` is used (not a raw SPSC) because the push side
/// is the service thread and the drain side is the engine thread within the same
/// process; the lock is held only for the O(1) push and the drain-swap, never
/// across `Engine::run`, so it never blocks the RT tick meaningfully. (A true
/// SPSC ring is the hardware-thread upgrade.)
#[derive(Debug, Default)]
pub struct CommandMailbox {
    inner: Mutex<Vec<Command>>,
}

impl CommandMailbox {
    /// Post a command (service side).
    pub fn post(&self, cmd: Command) {
        if let Ok(mut q) = self.inner.lock() {
            q.push(cmd);
        }
    }

    /// Drain all pending commands (engine side, at a control tick).
    fn drain(&self) -> Vec<Command> {
        match self.inner.lock() {
            Ok(mut q) => std::mem::take(&mut *q),
            Err(_) => Vec::new(),
        }
    }
}

/// The abort latch: set by `process()` when any protection trips; read by the
/// service between chunks to end the run with the recorded trip. Carries the
/// typed [`TripRecord`]. The `AtomicBool` lets the service poll cheaply without
/// locking; the record is behind the mutex.
#[derive(Debug, Default)]
pub struct AbortLatch {
    tripped: AtomicBool,
    record: Mutex<Option<TripRecord>>,
    /// A run the step ended with the CONTROLLED stop (a law fault or a
    /// divergence): the ramp runs to zero first, and this says why.
    controlled: Mutex<Option<TripRecord>>,
}

impl AbortLatch {
    /// Whether an abort has been latched.
    pub fn is_tripped(&self) -> bool {
        self.tripped.load(Ordering::Acquire)
    }

    /// The trip record, if any.
    pub fn take_record(&self) -> Option<TripRecord> {
        self.record.lock().ok().and_then(|mut r| r.take())
    }

    /// Latch a trip record from the SERVICE side (the engine's raw-ring accel
    /// watchdog fired between chunks; the service records it here so `run_chunk`
    /// ends the run through the same abort path as the in-`process()` trips).
    /// Idempotent: a record already latched (e.g. an in-process trip) wins.
    pub fn set_record(&self, record: TripRecord) {
        self.set(record);
    }

    /// Record why the step took the controlled stop (the first one wins). Does
    /// not set the abort: the run ramps down like a stop.
    pub fn set_controlled(&self, record: TripRecord) {
        if let Ok(mut r) = self.controlled.lock() {
            if r.is_none() {
                *r = Some(record);
            }
        }
    }

    /// Why the step took the controlled stop, if it did.
    pub fn take_controlled(&self) -> Option<TripRecord> {
        self.controlled.lock().ok().and_then(|mut r| r.take())
    }

    /// Whether the step took the controlled stop.
    pub fn controlled_pending(&self) -> bool {
        self.controlled.lock().map(|r| r.is_some()).unwrap_or(false)
    }

    fn set(&self, record: TripRecord) {
        if let Ok(mut r) = self.record.lock() {
            if r.is_none() {
                *r = Some(record);
            }
        }
        self.tripped.store(true, Ordering::Release);
    }
}

/// The live control snapshot the service drains between chunks for the status
/// surface. Lock-free from the service's view: the engine stores the latest via
/// the `Mutex` (held only for the O(1) store/clone), so a slow reader never
/// stalls the tick.
#[derive(Debug, Clone, Default)]
pub struct ControlSnapshot {
    pub freq_hz: f64,
    pub level_db: f64,
    pub target_g: f64,
    pub measured_g: f64,
    pub per_channel_tracked: Vec<f64>,
    pub per_channel_broadband: Vec<f64>,
    pub thd: f64,
    pub notch_binding: bool,
    pub alarm_active: bool,
    pub sweeps_done: u32,
    pub elapsed_test_s: f64,
    /// Overload latches per control channel, in control-set order.
    pub overload_latched: Vec<bool>,
    /// Station time of this tick, seconds since the loop engaged, holds
    /// included: frames the step has processed over the adopted rate.
    pub t_s: f64,
    /// Frames the step has processed at this tick.
    pub frames: u64,
    /// The drive as played, drive units peak (volts peak on hardware).
    pub drive_vpk: f64,
    /// The reference output's amplitude as played, volts peak; `None` without
    /// one.
    pub cola_vpk: Option<f64>,
    /// The limit channels' ceiling on the drive, drive units peak; `None`
    /// when there is none or it is unbounded.
    pub notch_ceiling: Option<f64>,
    /// Whether test progress is held (the operator's hold).
    pub held: bool,
    /// The sweep's direction; `None` outside a sweep.
    pub direction: Option<SweepDirection>,
    /// The sweep rate now, Hz per second, signed; 0 outside a sweep or while
    /// held.
    pub sweep_rate_hz_per_s: f64,
    /// The same in octaves per minute for a logarithmic sweep; `None`
    /// otherwise.
    pub sweep_rate_oct_per_min: Option<f64>,
    /// The ladder's rung index and the time at this level, seconds of test
    /// time.
    pub rung_index: usize,
    pub at_level_s: f64,
    /// Every input slot (every enabled input, in table order): tracked
    /// amplitude, phase against the drive in degrees, broadband rms over the
    /// tick.
    pub input_tracked: Vec<f64>,
    pub input_phase_deg: Vec<f64>,
    pub input_rms: Vec<f64>,
    /// Per limit channel, in configuration order: its bound at the tick's
    /// frequency (the limit profile's abort edge).
    pub limit_bound: Vec<f64>,
    /// The control law's counters and its values on the last call.
    pub law_calls: u64,
    pub law_misses: u32,
    pub law_capped_ticks: u64,
    pub law_values: Option<LawValues>,
}

/// What one control tick decided, for an observer ([`TickObserver`]).
///
/// The numbers the drive decision rests on: the drive committed this tick, the
/// amplitude servo's own step and level, and what it measured against. Taken
/// at the end of a tick that ran control (not on a ramp-down or a tripped
/// tick).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickObservation {
    /// The control-tick index.
    pub tick: u64,
    /// The frequency the tick's amplitudes were measured at, Hz.
    pub freq_hz: f64,
    /// The scheduled level after the tick, dB.
    pub level_db: f64,
    /// The control composite the tick acted on.
    pub measured: f64,
    /// The drive committed this tick, drive units peak.
    pub drive: f64,
    /// The amplitude servo's step this tick, dB.
    pub servo_step_db: f64,
    /// The amplitude servo's own level after the tick, dB re one drive unit.
    pub servo_level_db: f64,
    /// The amplitude servo's fault latch after the tick.
    pub servo_faulted: bool,
    /// Whether the limit channels held the drive this tick.
    pub notch_binding: bool,
}

/// A callback that sees every control tick. It runs on the control thread,
/// once per tick: it must not block, and should not allocate.
pub type TickObserver = Box<dyn FnMut(&TickObservation) + Send>;

/// The shared status cell.
#[derive(Debug, Default)]
pub struct StatusCell {
    inner: Mutex<ControlSnapshot>,
}

/// A snapshot whose per-channel slices are BORROWED from the step's preallocated
/// buffers. Exists so the publish path allocates nothing: the cell copies into
/// the `Vec`s it already owns.
pub(crate) struct ControlSnapshotRef<'a> {
    pub freq_hz: f64,
    pub level_db: f64,
    pub target_g: f64,
    pub measured_g: f64,
    pub per_channel_tracked: &'a [f64],
    pub per_channel_broadband: &'a [f64],
    pub thd: f64,
    pub notch_binding: bool,
    pub alarm_active: bool,
    pub sweeps_done: u32,
    pub elapsed_test_s: f64,
    pub overload_latched: &'a [bool],
    pub t_s: f64,
    pub frames: u64,
    pub drive_vpk: f64,
    pub cola_vpk: Option<f64>,
    pub notch_ceiling: Option<f64>,
    pub held: bool,
    pub direction: Option<SweepDirection>,
    pub sweep_rate_hz_per_s: f64,
    pub sweep_rate_oct_per_min: Option<f64>,
    pub rung_index: usize,
    pub at_level_s: f64,
    pub input_tracked: &'a [f64],
    pub input_phase_deg: &'a [f64],
    pub input_rms: &'a [f64],
    pub limit_bound: &'a [f64],
    pub law_calls: u64,
    pub law_misses: u32,
    pub law_capped_ticks: u64,
    pub law_values: Option<LawValues>,
}

impl StatusCell {
    /// Read the latest snapshot (service side).
    pub fn read(&self) -> ControlSnapshot {
        self.inner.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Store by COPYING into the cell's existing allocations.
    ///
    /// `clear` + `extend_from_slice` rather than assignment: the cell's `Vec`s
    /// keep their capacity across the whole run, so a publish on the control tick
    /// never reaches the allocator. The lock is held for three memcpys of a
    /// handful of f64s - never across anything else.
    pub(crate) fn store_from(&self, r: ControlSnapshotRef<'_>) {
        if let Ok(mut s) = self.inner.lock() {
            s.freq_hz = r.freq_hz;
            s.level_db = r.level_db;
            s.target_g = r.target_g;
            s.measured_g = r.measured_g;
            s.thd = r.thd;
            s.notch_binding = r.notch_binding;
            s.alarm_active = r.alarm_active;
            s.sweeps_done = r.sweeps_done;
            s.elapsed_test_s = r.elapsed_test_s;
            s.per_channel_tracked.clear();
            s.per_channel_tracked
                .extend_from_slice(r.per_channel_tracked);
            s.per_channel_broadband.clear();
            s.per_channel_broadband
                .extend_from_slice(r.per_channel_broadband);
            s.overload_latched.clear();
            s.overload_latched.extend_from_slice(r.overload_latched);
            s.t_s = r.t_s;
            s.frames = r.frames;
            s.drive_vpk = r.drive_vpk;
            s.cola_vpk = r.cola_vpk;
            s.notch_ceiling = r.notch_ceiling;
            s.held = r.held;
            s.direction = r.direction;
            s.sweep_rate_hz_per_s = r.sweep_rate_hz_per_s;
            s.sweep_rate_oct_per_min = r.sweep_rate_oct_per_min;
            s.rung_index = r.rung_index;
            s.at_level_s = r.at_level_s;
            s.input_tracked.clear();
            s.input_tracked.extend_from_slice(r.input_tracked);
            s.input_phase_deg.clear();
            s.input_phase_deg.extend_from_slice(r.input_phase_deg);
            s.input_rms.clear();
            s.input_rms.extend_from_slice(r.input_rms);
            s.limit_bound.clear();
            s.limit_bound.extend_from_slice(r.limit_bound);
            s.law_calls = r.law_calls;
            s.law_misses = r.law_misses;
            s.law_capped_ticks = r.law_capped_ticks;
            s.law_values = r.law_values;
        }
    }
}

/// The stop / abort drive ramp, in dB PER SECOND.
///
/// This used to be `scale *= 0.999` per FRAME, which is a rate-dependent law
/// wearing a rate-independent constant: it is -0.00869 dB per frame, so the same
/// code decays at -142 dB/s at the 16 384 Hz Sim rate and -445 dB/s at the
/// chassis's 51 200 Hz. The safe-stop duration - the number a host has to check
/// against `controller_timeout_s` before it accepts a profile - was therefore
/// unstateable without knowing the rate, and it silently changed by 3x the day
/// Sine first armed on hardware.
///
/// 120 dB/s is chosen to keep the SIM behaviour it replaces (~142 dB/s, i.e. the
/// full span in ~0.85 s) while being a round number in the unit that survives a
/// rate change. Smooth, never stepwise: it is a geometric decay
/// evaluated per frame, so at any rate the drive travels the same dB in the same
/// TIME. See [`SineProcessStep::safe_stop_seconds`] for the budget this makes
/// computable.
pub const RAMP_DB_PER_S: f64 = 120.0;

/// The drive scale a ramp snaps to zero at: **-120 dB re unit drive scale**.
///
/// A geometric decay never reaches zero, so the law needs a floor, and the floor
/// is what makes "the drive is at zero" a decidable predicate rather than an
/// asymptote. -120 dB is below any DAC's LSB at any usable range, so a value
/// under it is zero on the wire whatever the code says.
pub const RAMP_FLOOR_SCALE: f64 = 1e-6;

/// The per-frame decay factor for [`RAMP_DB_PER_S`] at `fs_hz`.
pub fn ramp_factor_per_frame(fs_hz: f64) -> f64 {
    if !fs_hz.is_finite() || fs_hz <= 0.0 {
        return 0.0; // a run with no rate cannot ramp; cut rather than hang
    }
    10f64.powf(-RAMP_DB_PER_S / (20.0 * fs_hz))
}

/// The per-channel trace point cap. When a channel's trace reaches
/// `2 * TRACE_CAP` it is downsampled by 2 (every other point kept) so the trace
/// stays bounded between `TRACE_CAP` and `2 * TRACE_CAP` points, and the push
/// stride doubles so subsequent points are decimated at the coarser rate. This
/// preserves the swept-vs-frequency SHAPE (uniform decimation over the whole
/// sweep) while bounding memory on an arbitrarily long dwell, and it means
/// `process()` does not grow a Vec without bound over a long run.
pub const TRACE_CAP: usize = 4096;

/// A swept-trace log the service drains for the report. One
/// `(freq, tracked, broadband)` per input channel per control tick, BOUNDED to
/// `~TRACE_CAP` points per channel by progressive downsampling.
#[derive(Debug, Default)]
pub struct TraceLog {
    /// Per input channel: the swept `(freq, tracked_amp)` points.
    pub tracked: Mutex<Vec<Vec<(f64, f64)>>>,
    /// Per input channel: the swept `(freq, broadband_rms)` points.
    pub broadband: Mutex<Vec<Vec<(f64, f64)>>>,
    /// Per input channel: the current keep stride (1, 2, 4, ...) and the count of
    /// pushes since the last kept point. Doubling the stride as the trace grows is
    /// what bounds it without losing the sweep shape.
    stride: Mutex<Vec<(u64, u64)>>,
}

impl TraceLog {
    fn ensure(&self, n_ch: usize) {
        // Sized once to the bound the decimation keeps (`2 * TRACE_CAP`), so a
        // push on the control tick never reaches the allocator.
        let full = || -> Vec<Vec<(f64, f64)>> {
            (0..n_ch)
                .map(|_| Vec::with_capacity(2 * TRACE_CAP + 1))
                .collect()
        };
        if let Ok(mut t) = self.tracked.lock() {
            if t.len() != n_ch {
                *t = full();
            }
        }
        if let Ok(mut b) = self.broadband.lock() {
            if b.len() != n_ch {
                *b = full();
            }
        }
        if let Ok(mut s) = self.stride.lock() {
            if s.len() != n_ch {
                *s = vec![(1, 0); n_ch];
            }
        }
    }

    /// Push a point per channel, keeping the trace bounded. Uniform
    /// decimation: only every `stride`-th push is kept; when a channel exceeds
    /// `2 * TRACE_CAP` points its stored points are halved (keep every other) and
    /// its stride doubles, so the whole-sweep shape is preserved at a coarser but
    /// still-uniform resolution.
    fn push(&self, f: f64, tracked: &[f64], broadband: &[f64]) {
        let (Ok(mut t), Ok(mut b), Ok(mut strides)) = (
            self.tracked.lock(),
            self.broadband.lock(),
            self.stride.lock(),
        ) else {
            return;
        };
        for ch in 0..strides.len() {
            let (stride, since) = strides[ch];
            // Only keep every `stride`-th push (uniform decimation).
            if since + 1 < stride {
                strides[ch] = (stride, since + 1);
                continue;
            }
            // This push is kept. Reset the since-counter; the stride only doubles
            // when a decimate ACTUALLY fires below (not every tick above the cap -
            // that would runaway-double the stride and starve the trace).
            let mut new_stride = stride;
            if let (Some(tv), Some(&a)) = (t.get_mut(ch), tracked.get(ch)) {
                tv.push((f, a));
                if tv.len() >= 2 * TRACE_CAP {
                    decimate_in_place(tv);
                    // A decimate halved the stored resolution: double the keep
                    // stride so future pushes match the coarser stored rate. The
                    // trace now oscillates between TRACE_CAP and 2*TRACE_CAP.
                    new_stride = stride.saturating_mul(2).max(2);
                }
            }
            if let (Some(bv), Some(&a)) = (b.get_mut(ch), broadband.get(ch)) {
                bv.push((f, a));
                if bv.len() >= 2 * TRACE_CAP {
                    decimate_in_place(bv);
                }
            }
            strides[ch] = (new_stride, 0);
        }
    }
}

/// Keep every other point of a trace, halving its length in place (progressive
/// downsampling). Preserves the endpoints' spread across the sweep.
fn decimate_in_place(v: &mut Vec<(f64, f64)>) {
    let mut w = 0usize;
    let mut r = 0usize;
    while r < v.len() {
        v[w] = v[r];
        w += 1;
        r += 2;
    }
    v.truncate(w);
}

/// The scheduler state (one of the four mode families). The engine advances
/// whichever variant is active per control tick.
enum SchedulerState {
    Sweep(BidirSweep),
    Dwell(DwellRun),
    StepSine(StepSine),
    Rstd(RstdPlan),
}

/// A resolved limit channel: its input-channel slot (offset into the interleaved
/// input frame) and its limit profile.
pub struct LimitChannelSpec {
    /// Offset of this channel in the interleaved input frame.
    pub input_slot: usize,
    /// The channel table index (for the trip record).
    pub channel_index: u32,
    /// The limit profile (evaluated at `f` each tick as a bound).
    pub profile: SineProfile,
    /// The sysID `|H|` fallback for the notch (response EU per unit drive), keyed
    /// by frequency via a small grid; `None` until sysID runs.
    pub sysid_h: Option<(Vec<f64>, Vec<f64>)>,
}

/// The COLA aux-output settings resolved into the engine.
#[derive(Debug, Clone, Copy)]
pub struct ColaState {
    pub enabled: bool,
    pub amp: f64,
    pub waveform: ColaWaveform,
}

/// How many per-control-tick snapshots [`SineProcessStep`] keeps for
/// [`SineProcessStep::context_at_frame`].
///
/// The consumer is the raw-ring watchdog trip path, which asks about a frame the
/// engine has ALREADY published to the ring - i.e. at most one block back from
/// the step's own position, never more. 256 ticks is orders of magnitude more
/// than that (about 2.5 s of history at a 10 ms tick) and costs ~10 KB, so the
/// depth is not a thing anyone has to size against a block.
const FRAME_CONTEXT_DEPTH: usize = 256;

/// What the drive was actually doing at a given RUN FRAME - the state a trip
/// record must quote.
///
/// # Why a frame-indexed snapshot instead of re-reading the step
///
/// The engine's raw-ring [`hertzforge_engine::AbortWatchdog`] reports the FRAME its
/// sustained breach completed on (`AbortTrip.at_frame`). The old trip path
/// threw that away and re-read `step.freq_hz()` / `step.level_db()` after the
/// batch unwound, which answers a different question: "what is the scheduler
/// commanding NOW", not "what was being commanded when the breach happened".
/// The two differ by up to a block's worth of control ticks, and on the chassis
/// tracking-lag trips that was enough to put the recorded LEVEL on the wrong
/// ladder rung entirely - the journal showed 0 dB drive where the record said
/// -12 dB, which is what made "level-independent" look like an observation
/// instead of an artifact.
///
/// One of these is pushed at the end of every control tick, tagged with the first
/// frame it applies to. The lookup is then exact for any frame the step has
/// processed, and honestly flagged (see [`FrameContext::extrapolated`]) for a
/// frame that is still in flight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameContext {
    /// First run frame (1-based, matching `AbortTrip.at_frame`) this state
    /// applied to.
    pub since_frame: u64,
    /// The NCO frequency command in effect (Hz) - what the tracking filters were
    /// demodulating against.
    pub freq_hz: f64,
    /// The ladder rung in effect (dB).
    pub level_db: f64,
    /// The committed drive scale applied to the NCO output over this window.
    pub drive_scale: f64,
    /// The tracked COMPOSITE (EU pk) the servo last acted on. Measured over the
    /// control-tick window that ENDED at `since_frame - 1`: it is the freshest
    /// demodulated number that existed while these frames were being driven, and
    /// stating that is the point - it is precisely the number the raw ring
    /// disagreed with.
    pub measured: f64,
    /// The control-tick index this snapshot was taken at.
    pub tick: u64,
    /// TRUE when the requested frame is AHEAD of everything the step has
    /// processed, so the answer is the last committed command extrapolated
    /// forward rather than a record of what ran.
    ///
    /// This is the normal case for the raw-ring watchdog: the engine polls it on
    /// a block BEFORE handing that block to the step, so a trip's frame is one
    /// block ahead of the step's own position. The value is still the right one
    /// (the NCO does not move between control ticks), but a reader deserves to
    /// know which of the two it is holding.
    pub extrapolated: bool,
}

impl Default for FrameContext {
    fn default() -> Self {
        Self {
            since_frame: 0,
            freq_hz: 0.0,
            level_db: 0.0,
            drive_scale: 0.0,
            measured: 0.0,
            tick: 0,
            extrapolated: false,
        }
    }
}

/// The persistent sine control step. Owns the NCO, the tracking-filter
/// bank, the THD bank, the servo, the blend law + feedforward, the notch
/// governor, the scheduler + ladder, and the per-tick accumulators. Publishes
/// into the shared [`StatusCell`] + [`TraceLog`]; latches aborts into the shared
/// [`AbortLatch`]; drains the shared [`CommandMailbox`].
pub struct SineProcessStep {
    // --- config-derived immutables --------------------------------------------
    fs_hz: f64,
    control_tick_frames: usize,
    out_channels: usize,
    profile: SineProfile,
    /// Input-frame offsets of the control channels, in control-set order.
    control_slots: Vec<usize>,
    control_weights: Vec<f64>,
    composite_mode: CompositeMode,
    cola: ColaState,
    drive_guard: DriveGuard,
    kinematic_monitor: bool,
    /// Shaker kinematic ceilings (max velocity in/s, max pk-pk displacement in);
    /// `None` disables that axis.
    max_velocity_ips: Option<f64>,
    max_displacement_in_pp: Option<f64>,
    fault_debounce_ticks: u32,
    /// SERVICE-SIDE collapse fraction: the composite is "collapsed" (feeds the
    /// debounced servo-fault abort) when `measured / target` is below this.
    /// Set from [`StepBuild::fault_fraction`]; deliberately WIDER than the servo's
    /// own internal freeze fraction (`fault_fraction_effective()`, ~1e-3) so the
    /// service-side debounced abort - not the servo's silent freeze - owns the
    /// fault decision. Threaded through, not hardcoded.
    fault_fraction: f64,
    /// Control ticks after engage / a rung change during which the scalar
    /// alarm/abort, the debounced servo-fault, and the kinematic monitor are
    /// SUPPRESSED so the startup ramp can bring the response onto the band before
    /// the protections arm (the level-settle debounce, as in hertzforge-random). The
    /// scalar-abort is ALSO debounced over `scalar_abort_debounce` ticks after
    /// the settle window, so a single noisy read does not trip.
    settle_ticks: u32,
    /// Consecutive out-of-band ticks required to trip the scalar abort.
    scalar_abort_debounce: u32,

    /// The drive-to-response transport lag, in frames, that the
    /// demodulation reference is retarded by. `0` disables the compensation
    /// exactly (see [`SineProcessStep::reference_freq`]).
    tracking_lag_frames: usize,
    /// Ring of the NCO's per-frame instantaneous frequency, `tracking_lag_frames`
    /// deep, seeded with the start frequency. Empty when the lag is zero.
    freq_delay: Vec<f64>,
    /// Write cursor into `freq_delay`.
    freq_delay_head: usize,

    // --- live DSP objects -----------------------------------------------------
    nco: SweepOscillator,
    filters: Vec<AdaptiveTrackingFilter>,
    thd_bank: ThdBank,
    /// The control law: decides the drive each tick (see [`crate::law`]).
    law: Box<dyn SineControlLaw + Send>,
    /// The values the law reports, by declared index; cleared before each call.
    law_buf: LawValues,
    /// The core's guards on the law; `None` runs it with none but the law's
    /// own (the shipped law's 1 dB clamp).
    law_guards: Option<LawGuards>,
    /// The clock calls are timed on, under the guards.
    law_clock: Option<Arc<dyn LawClock>>,
    /// The time one call may take, seconds.
    law_budget_s: f64,
    /// Calls since the law was engaged (the `tick_index` it sees).
    law_tick_index: u64,
    /// The law was stopped (a law fault or a divergence): it is not called
    /// again.
    law_stopped: bool,
    /// The last unusable call's fault.
    law_last_fault: Option<LawFault>,
    /// The law's own fault judgement on its last usable call.
    law_faulted: bool,
    /// Seconds the change cap has bound without a break.
    capped_streak_s: f64,
    /// Where the observer finds the servo's step and level among the law's
    /// values.
    obs_ix: (Option<usize>, Option<usize>),
    /// Per control channel: phase against the drive this tick, degrees.
    /// Preallocated, refilled in place.
    control_phase_buf: Vec<f64>,
    /// Every call, for the records (drained between batches).
    law_records: Arc<LawRecordRing>,
    /// The law's live parameters, in its declared order, and their values in
    /// force.
    live_keys: Vec<String>,
    live_values: Vec<f64>,
    feedforward: Option<FeedforwardTable>,
    notch: Option<NotchGovernor>,
    limit_channels: Vec<LimitChannelSpec>,

    // --- scheduler + ladder ---------------------------------------------------
    scheduler: SchedulerState,
    ladder: LadderPlan,

    // --- per-tick accumulators ------------------------------------------------
    /// Frames elapsed in the current control-tick window.
    tick_frames: usize,
    /// Per input channel: latest tracked amplitude (g pk) from its filter.
    tracked: Vec<f64>,
    /// Per input channel: latest tracked phase (rad) from its filter (for RSTD).
    last_phase: Vec<f64>,
    /// Per input channel: broadband sum-of-squares this tick window.
    broadband_ss: Vec<f64>,
    /// Reusable per-tick broadband-RMS scratch (the per-tick RMS derived from
    /// `broadband_ss`). Preallocated once and refilled (clear + extend) each
    /// control tick so `control_tick` does not allocate a fresh Vec every tick
    /// (ProcessStep's "must not allocate in steady state", scheduler.rs).
    broadband_buf: Vec<f64>,
    /// The remaining PER-TICK allocations, given the same preallocation treatment.
    ///
    /// Four `Vec`s were being built from scratch on every control tick: the
    /// control-channel composite input, the notch's per-limit-channel observation
    /// slice, and the two per-channel status slices. At a 10 ms tick that is 400
    /// allocations a second on the thread that owns the drive clock, and the
    /// engine's compute budget on hardware IS the tick period - an allocator
    /// stall inside one is a `SchedulingFault` and an `emergency_stop`. Each is
    /// now a preallocated buffer, moved out with `mem::take`, refilled in place
    /// and moved back, exactly as `broadband_buf` above already was.
    control_amps_buf: Vec<f64>,
    notch_ticks_buf: Vec<LimitChannelTick>,
    tracked_buf: Vec<f64>,
    bb_pub_buf: Vec<f64>,
    /// Overload latches, published by CLONE every tick. Kept as a preallocated
    /// mirror so the snapshot copies into an existing allocation.
    overload_pub_buf: Vec<bool>,
    /// The drive scale actually applied this tick window (for the notch
    /// sensitivity ratio and the status).
    current_drive_scale: f64,
    /// Consecutive faulted control ticks (the servo-fault debounce).
    fault_streak: u32,
    /// Consecutive out-of-abort-band control ticks (the scalar-abort debounce).
    scalar_abort_streak: u32,
    /// Remaining settle ticks (protections suppressed while > 0). Reset on a
    /// rung change so each new level re-settles before the abort re-arms.
    settle_remaining: u32,
    /// The ladder level (dB) at the last tick, to detect a rung change and
    /// re-open the settle window.
    last_level_db: f64,
    /// Whether the PREVIOUS tick was in the RSTD probe (Search/Settle/
    /// Capture). Used to detect the Capture -> Dwell transition and re-open the
    /// settle window: the probe freezes the drive off-resonance and bypasses
    /// feedforward, so at Dwell entry the response rings at ~Q*target and the
    /// high-side scalar abort / limit abort / kinematic monitor would arm with no
    /// settle window; re-opening lets the servo ramp the frozen-hot drive down to
    /// the compressed level first.
    was_rstd_probe: bool,
    /// The control-tick counter (for trip records + level history).
    tick_count: u64,
    /// Cumulative RUN FRAMES this step has processed, in the SAME 1-based
    /// numbering the engine's raw-ring watchdog uses for `AbortTrip.at_frame`
    /// (the engine pushes each input frame to the ring and hands the same block
    /// to `process`, in order, so the two counters name the same frames).
    frames_processed: u64,
    /// Fixed-capacity ring of per-control-tick [`FrameContext`] snapshots, oldest
    /// to newest by `since_frame`. Preallocated at construction and written in
    /// place, so a control tick pushing one allocates nothing (the same rule the
    /// per-tick scratch buffers above follow).
    frame_ctx: Vec<FrameContext>,
    /// Write cursor into `frame_ctx`.
    frame_ctx_head: usize,
    /// Overload latch per control channel (proxy: |tracked| beyond a fraction of
    /// its abort-band high edge). One per control channel, in control-set order.
    overload_latched: Vec<bool>,
    /// Whether a ramp-down is in progress (after an abort): the drive scale
    /// decays geometrically each frame so the drive never cliffs.
    ramping_down: bool,
    /// Notch-active hysteresis. The observe-time / apply-time binding signals
    /// flicker on a knife-edge while the servo holds the drive right at the notch
    /// ceiling, which would otherwise open a false-abort gap for the low-side
    /// scalar abort + servo-fault (both legitimately suppressed while the notch
    /// holds the control point down). This latch stays > 0 for a short window
    /// after ANY binding tick, so a sustained hold reads as continuously notched.
    notch_hold_remaining: u32,
    /// The per-frame decay factor of the stop/abort ramp, derived ONCE from the
    /// ADOPTED rate ([`ramp_factor_per_frame`]). A per-frame constant is what the
    /// inner loop can afford; deriving it from the rate is what makes the ramp a
    /// dB/s law instead of a rate-dependent accident.
    ramp_per_frame: f64,
    /// The RAW acquired blocks, for the display / time-stream tap. Pushed by
    /// `process` (a memcpy, nothing more) and drained by the service between tick
    /// batches. See [`crate::service::ring`] for why this is not a callback.
    ring: Arc<SampleRing>,
    /// The COLA aux-output ramp factor, in `[0, 1]`. Held at 1.0 during
    /// normal running; decays in lockstep with the control drive during a
    /// post-abort / stop ramp-down so the COLA ch1 output DECAYS to zero with ch0
    /// rather than cliffing at full amplitude on a Drive-role DAC (never a cliff).
    cola_ramp_scale: f64,
    /// The tracking filters' reference phase, mirrored: every filter is fed
    /// the same reference frequency every frame, so one accumulator stepped
    /// by the same arithmetic is the phase each of them demodulated against.
    /// With the NCO's phase it turns a filter's phase (against its own
    /// reference) into the phase against the drive.
    ref_phase_mirror: f64,
    /// The filters' reference phase less the NCO's, on the last frame,
    /// radians.
    phase_offset: f64,
    /// Per input slot: the phase against the drive this tick, degrees.
    /// Preallocated, refilled in place.
    input_phase_buf: Vec<f64>,
    /// Per limit channel: its bound at the tick's frequency. Preallocated.
    limit_bound_buf: Vec<f64>,
    /// The display record (see [`crate::service::display`]).
    display: Arc<DisplayCell>,
    /// The frequency this tick's amplitudes were measured at (before the
    /// scheduler advances), Hz.
    tick_freq_hz: f64,
    /// The operator moved the tone (a jog or a go to): the display must not
    /// fill the points between.
    display_jump: bool,
    /// The control law's counters and its values on the last call.
    law_calls: u64,
    law_misses: u32,
    law_capped_ticks: u64,
    law_values: Option<LawValues>,
    /// A test's or a diagnostic's view of every control tick
    /// ([`SineProcessStep::observe_ticks`]).
    observer: Option<TickObserver>,

    // --- shared surfaces ------------------------------------------------------
    status: Arc<StatusCell>,
    trace: Arc<TraceLog>,
    mailbox: Arc<CommandMailbox>,
    abort: Arc<AbortLatch>,
    /// Set when the operator posted Stop (the service reads it to end the loop).
    stop_requested: Arc<AtomicBool>,
}

/// Everything the service needs to build a [`SineProcessStep`]. Keeps the
/// constructor argument list from ballooning and documents the seam.
pub struct StepBuild {
    pub fs_hz: f64,
    pub control_tick_frames: usize,
    pub out_channels: usize,
    pub profile: SineProfile,
    pub n_input_channels: usize,
    pub control_slots: Vec<usize>,
    pub control_weights: Vec<f64>,
    pub composite_mode: CompositeMode,
    pub tracking_bw: BandwidthMode,
    /// The built-in law's servo and blend: the step runs them as its law
    /// until [`SineProcessStep::install_law`] gives it another (the service
    /// always installs the law its factory builds).
    pub servo: AmplitudeServo,
    pub blend: BlendLaw,
    pub feedforward: Option<FeedforwardTable>,
    pub notch: Option<NotchGovernor>,
    pub limit_channels: Vec<LimitChannelSpec>,
    pub scheduler: SchedulerBuild,
    pub ladder: LadderPlan,
    pub cola: ColaState,
    pub drive_guard: DriveGuard,
    pub kinematic_monitor: bool,
    pub max_velocity_ips: Option<f64>,
    pub max_displacement_in_pp: Option<f64>,
    pub fault_debounce_ticks: u32,
    /// The SERVICE-SIDE collapse fraction for the debounced servo-fault abort: the
    /// composite is "collapsed" when `measured / target` is below this. Deliberately
    /// WIDER than the servo's OWN internal freeze fraction (the servo uses
    /// `fault_fraction_effective()`, a tighter floor, so the servo's own detection
    /// stays quiet while THIS debounced service-side check owns the abort decision).
    /// Threaded through explicitly (not hardcoded) so the intended service-abort >=
    /// servo-freeze relationship is visible and single-sourced at the call site.
    pub fault_fraction: f64,
    /// Ticks to suppress the protections after engage / a rung change (the
    /// level-settle debounce). The service sizes this to a few settling times.
    pub settle_ticks: u32,
    /// Consecutive out-of-band ticks to trip the scalar abort.
    pub scalar_abort_debounce: u32,
    pub start_f_hz: f64,
    pub start_level_scale: f64,
    /// The measured drive-to-response transport lag in frames, used
    /// to retard the tracking filters' demodulation reference. `0` = no
    /// compensation, which is what HERTZFORGE-SIM measures and therefore what every
    /// Sim fixture in the tree runs with.
    pub tracking_lag_frames: usize,
    /// Blocks of acquisition the raw ring holds before it drops the oldest.
    /// Sized in TIME by the service, not as a bare count.
    pub ring_blocks: usize,
    /// Frames per acquisition block, for sizing the ring's recycled buffers so
    /// the steady state never reaches the allocator.
    pub block_frames: usize,
    /// AUTO / MANUAL at engage. The service carries the operator's choice from
    /// before the step existed (a MANUAL run is selected at Start, and the step
    /// is not built until the coherence gate), so the mode has to arrive with the
    /// build rather than as a command that would race the first rung rule.
    pub run_mode: RunMode,
}

/// The scheduler variant to build (the service resolves the mode into this).
pub enum SchedulerBuild {
    Sweep(BidirSweep),
    Dwell(DwellRun),
    StepSine(StepSine),
    Rstd(RstdPlan),
}

impl SchedulerBuild {
    /// The band the scheduler runs over (a dwell's one frequency, twice).
    fn band(&self) -> (f64, f64) {
        match self {
            SchedulerBuild::Sweep(s) => s.band(),
            SchedulerBuild::Dwell(d) => (d.freq_hz(), d.freq_hz()),
            SchedulerBuild::StepSine(s) => s.band(),
            SchedulerBuild::Rstd(r) => r.band(),
        }
    }

    fn into_state(self) -> SchedulerState {
        match self {
            SchedulerBuild::Sweep(s) => SchedulerState::Sweep(s),
            SchedulerBuild::Dwell(d) => SchedulerState::Dwell(d),
            SchedulerBuild::StepSine(s) => SchedulerState::StepSine(s),
            SchedulerBuild::Rstd(r) => SchedulerState::Rstd(r),
        }
    }
}

/// The shared handles the service keeps to drive + observe a step.
#[derive(Clone)]
pub struct StepHandles {
    pub status: Arc<StatusCell>,
    pub trace: Arc<TraceLog>,
    pub mailbox: Arc<CommandMailbox>,
    pub abort: Arc<AbortLatch>,
    pub stop_requested: Arc<AtomicBool>,
    /// The raw acquisition ring the service drains between batches.
    pub ring: Arc<SampleRing>,
    /// The display record the service copies out between batches.
    pub display: Arc<DisplayCell>,
    /// The law's calls, drained between batches.
    pub law_records: Arc<LawRecordRing>,
}

impl SineProcessStep {
    /// Build a persistent step from the resolved [`StepBuild`], returning the
    /// step and the shared handles the service uses to observe / command it.
    pub fn new(b: StepBuild) -> (Self, StepHandles) {
        // `SweepOscillator::new` already parks at `start_f_hz`; the redundant
        // `set_frequency` that used to follow was a second panicking call on the
        // same value. The service clamps `start_f_hz` into the ADOPTED band
        // (<= 0.45 x fs) before it gets here, which is what keeps `new`'s own
        // assert unreachable.
        let nco = SweepOscillator::new(b.fs_hz, b.start_f_hz);
        let filters: Vec<AdaptiveTrackingFilter> = (0..b.n_input_channels)
            .map(|_| AdaptiveTrackingFilter::new(b.fs_hz, b.tracking_bw))
            .collect();
        // THD bandwidth: the control filter's effective bw at the start freq.
        let thd_bw = b.tracking_bw.effective_bw(b.start_f_hz).max(1.0);
        let thd_bank = ThdBank::new(b.fs_hz, thd_bw);

        let status = Arc::new(StatusCell::default());
        let trace = Arc::new(TraceLog::default());
        trace.ensure(b.n_input_channels);
        let mailbox = Arc::new(CommandMailbox::default());
        let abort = Arc::new(AbortLatch::default());
        let stop_requested = Arc::new(AtomicBool::new(false));
        let ring = Arc::new(SampleRing::new(
            b.ring_blocks,
            b.block_frames,
            b.n_input_channels.max(1),
        ));

        let n_control = b.control_slots.len();
        let n_limit = b.limit_channels.len();
        // A LAG LONGER THAN A SECOND IS NOT A LAG, it is a bad measurement, and
        // compensating against one would beat the reference off the signal by as
        // much as leaving it uncompensated. Clamp rather than trust; the sysID
        // cross-correlation searches a bounded window anyway, so reaching this is
        // a signal that something upstream is wrong, not a normal case.
        let lag = b.tracking_lag_frames.min(b.fs_hz.max(1.0) as usize);
        let mut b = b;
        b.ladder.set_manual(matches!(b.run_mode, RunMode::Manual));
        let start_level_db = b.ladder.level_db();
        let tick_s = b.control_tick_frames.max(1) as f64 / b.fs_hz;
        let display = Arc::new(DisplayCell {
            log: Mutex::new(DisplayLog::new(
                b.n_input_channels,
                n_limit,
                b.scheduler.band(),
                tick_s,
            )),
            slots: Mutex::new(DisplaySlots {
                input_index: (0..b.n_input_channels as u32).collect(),
                control_slots: b.control_slots.clone(),
                limit_slots: b.limit_channels.iter().map(|l| l.input_slot).collect(),
                cola: b.cola.enabled,
            }),
        });
        let law_records = Arc::new(LawRecordRing::new(LAW_RECORD_CAPACITY));
        let blend_w = b.blend.w();
        let handles = StepHandles {
            law_records: law_records.clone(),
            status: status.clone(),
            trace: trace.clone(),
            mailbox: mailbox.clone(),
            abort: abort.clone(),
            stop_requested: stop_requested.clone(),
            ring: ring.clone(),
            display: display.clone(),
        };

        let step = SineProcessStep {
            fs_hz: b.fs_hz,
            control_tick_frames: b.control_tick_frames.max(1),
            out_channels: b.out_channels.max(1),
            profile: b.profile,
            control_slots: b.control_slots,
            control_weights: b.control_weights,
            composite_mode: b.composite_mode,
            cola: b.cola,
            drive_guard: b.drive_guard,
            kinematic_monitor: b.kinematic_monitor,
            max_velocity_ips: b.max_velocity_ips,
            max_displacement_in_pp: b.max_displacement_in_pp,
            fault_debounce_ticks: b.fault_debounce_ticks,
            fault_fraction: b.fault_fraction,
            settle_ticks: b.settle_ticks,
            scalar_abort_debounce: b.scalar_abort_debounce.max(1),
            tracking_lag_frames: lag,
            // SEEDED WITH THE START FREQUENCY, which is not an arbitrary filler:
            // the frames whose echo arrives during the first `lag` frames of the
            // run are the ENGAGE LEAD-IN, and the lead-in is synthesized at the
            // start frequency before the scheduler has advanced. The seed is
            // therefore the frequency that was actually on the wire.
            freq_delay: if lag > 0 {
                vec![b.start_f_hz; lag]
            } else {
                Vec::new()
            },
            freq_delay_head: 0,
            nco,
            filters,
            thd_bank,
            law: Box::new(ServoBlendLaw::from_parts(b.servo, b.blend)),
            law_buf: LawValues::new(servo_blend::DIAGNOSTICS.len()),
            law_guards: None,
            law_clock: None,
            law_budget_s: f64::INFINITY,
            law_tick_index: 0,
            law_stopped: false,
            law_last_fault: None,
            law_faulted: false,
            capped_streak_s: 0.0,
            obs_ix: (
                Some(servo_blend::V_SERVO_STEP),
                Some(servo_blend::V_SERVO_LEVEL),
            ),
            control_phase_buf: vec![0.0; n_control],
            law_records: law_records.clone(),
            live_keys: servo_blend::live_params()
                .into_iter()
                .map(|l| l.key)
                .collect(),
            live_values: vec![blend_w],
            feedforward: b.feedforward,
            notch: b.notch,
            limit_channels: b.limit_channels,
            scheduler: b.scheduler.into_state(),
            ladder: b.ladder,
            tick_frames: 0,
            tracked: vec![0.0; b.n_input_channels],
            last_phase: vec![0.0; b.n_input_channels],
            broadband_ss: vec![0.0; b.n_input_channels],
            broadband_buf: vec![0.0; b.n_input_channels],
            control_amps_buf: Vec::with_capacity(n_control),
            notch_ticks_buf: Vec::with_capacity(n_limit),
            tracked_buf: Vec::with_capacity(n_control),
            bb_pub_buf: Vec::with_capacity(n_control),
            overload_pub_buf: Vec::with_capacity(n_control),
            current_drive_scale: b.start_level_scale,
            fault_streak: 0,
            scalar_abort_streak: 0,
            settle_remaining: b.settle_ticks,
            last_level_db: start_level_db,
            was_rstd_probe: false,
            tick_count: 0,
            frames_processed: 0,
            // Seeded with the state the run STARTS in, applying from frame 1, so
            // a trip inside the very first control tick still resolves to a real
            // frequency instead of a zero.
            frame_ctx: {
                let mut v = vec![FrameContext::default(); FRAME_CONTEXT_DEPTH];
                v[0] = FrameContext {
                    since_frame: 1,
                    freq_hz: b.start_f_hz,
                    level_db: start_level_db,
                    drive_scale: b.start_level_scale,
                    measured: 0.0,
                    tick: 0,
                    extrapolated: false,
                };
                v
            },
            frame_ctx_head: 1,
            overload_latched: vec![false; n_control],
            ramping_down: false,
            notch_hold_remaining: 0,
            ramp_per_frame: ramp_factor_per_frame(b.fs_hz),
            ring,
            cola_ramp_scale: 1.0,
            ref_phase_mirror: 0.0,
            phase_offset: 0.0,
            input_phase_buf: vec![0.0; b.n_input_channels],
            limit_bound_buf: vec![0.0; n_limit],
            display,
            tick_freq_hz: b.start_f_hz,
            display_jump: false,
            law_calls: 0,
            law_misses: 0,
            law_capped_ticks: 0,
            law_values: None,
            observer: None,
            status,
            trace,
            mailbox,
            abort,
            stop_requested,
        };
        (step, handles)
    }

    /// Whether the scheduler's completion target has been met (the service uses
    /// this to move Engaged -> FullLevel -> Stopped between chunks).
    pub fn scheduler_done(&self) -> bool {
        match &self.scheduler {
            SchedulerState::Sweep(s) => s.done(),
            SchedulerState::Dwell(d) => d.done(),
            SchedulerState::StepSine(s) => s.done(),
            SchedulerState::Rstd(r) => matches!(
                r.state(),
                crate::scheduler::RstdState::Complete | crate::scheduler::RstdState::Failed
            ),
        }
    }

    /// Whether test PROGRESS is held - the operator's Hold, which pauses the
    /// sweep / dwell / step clocks while the output CONTINUES at the held
    /// frequency and level.
    ///
    /// Asked of whichever scheduler is active, because "held" is that scheduler's
    /// own flag: a second copy on the step would be a value that disagrees with
    /// the one the tick actually reads the moment a mailbox command is dropped or
    /// re-ordered. RSTD reports held the same way, even though a phase-tracked
    /// dwell holds its own frequency - what is paused is the dwell target's clock.
    pub fn is_held(&self) -> bool {
        match &self.scheduler {
            SchedulerState::Sweep(s) => s.held(),
            SchedulerState::Dwell(d) => d.held(),
            SchedulerState::StepSine(s) => {
                matches!(s.progress().state, crate::scheduler::SweepState::Held)
            }
            SchedulerState::Rstd(r) => r.held(),
        }
    }

    /// Whether the level ladder has completed (all rungs done).
    pub fn ladder_done(&self) -> bool {
        self.ladder.done()
    }

    /// The current ladder level in dB (for the service's level history).
    pub fn level_db(&self) -> f64 {
        self.ladder.level_db()
    }

    /// Cumulative RUN FRAMES processed, in `AbortTrip.at_frame`'s numbering.
    pub fn frames_processed(&self) -> u64 {
        self.frames_processed
    }

    /// The transport lag the demodulation reference is retarded by, in frames.
    /// `0` = no compensation. This is the CLAMPED value actually in force, not
    /// the number the builder asked for.
    pub fn tracking_lag_frames(&self) -> usize {
        self.tracking_lag_frames
    }

    /// What the drive was doing at run `frame`.
    ///
    /// Answers from the per-control-tick history rather than from live state, so
    /// a trip record can quote the breaching frame instead of whatever the
    /// scheduler had advanced to by the time the batch unwound. See
    /// [`FrameContext`] for why that distinction was not cosmetic.
    ///
    /// A frame ahead of [`SineProcessStep::frames_processed`] gets the newest
    /// snapshot with `extrapolated = true`; a frame older than the retained
    /// history gets the oldest retained snapshot, also flagged.
    pub fn context_at_frame(&self, frame: u64) -> FrameContext {
        let mut best: Option<FrameContext> = None;
        let mut oldest: Option<FrameContext> = None;
        for c in &self.frame_ctx {
            if c.since_frame == 0 {
                continue; // unwritten slot
            }
            if oldest.is_none_or(|o| c.since_frame < o.since_frame) {
                oldest = Some(*c);
            }
            if c.since_frame <= frame && best.is_none_or(|b| c.since_frame > b.since_frame) {
                best = Some(*c);
            }
        }
        let mut out = match (best, oldest) {
            (Some(b), _) => b,
            (None, Some(o)) => FrameContext {
                extrapolated: true,
                ..o
            },
            (None, None) => FrameContext::default(),
        };
        if frame > self.frames_processed {
            out.extrapolated = true;
        }
        out
    }

    /// Record the state committed by the control tick that just finished. It
    /// applies from the NEXT frame until the following control tick.
    fn push_frame_context(&mut self, measured: f64) {
        let slot = FrameContext {
            since_frame: self.frames_processed + 1,
            freq_hz: self.freq_hz(),
            level_db: self.ladder.level_db(),
            drive_scale: self.current_drive_scale,
            measured,
            tick: self.tick_count,
            extrapolated: false,
        };
        let head = self.frame_ctx_head % self.frame_ctx.len();
        self.frame_ctx[head] = slot;
        self.frame_ctx_head = head + 1;
    }

    /// The current frequency command (Hz).
    pub fn freq_hz(&self) -> f64 {
        match &self.scheduler {
            SchedulerState::Sweep(s) => s.freq_hz(),
            SchedulerState::Dwell(d) => d.freq_hz(),
            SchedulerState::StepSine(s) => s.freq_hz(),
            SchedulerState::Rstd(r) => r.freq_hz(),
        }
    }

    /// The number of completed one-direction sweeps (sweep mode; 0 otherwise).
    pub fn sweeps_done(&self) -> u32 {
        match &self.scheduler {
            SchedulerState::Sweep(s) => s.sweeps_done(),
            _ => 0,
        }
    }

    /// The RSTD estimate if in RSTD mode and a search has completed.
    pub fn rstd_estimate(&self) -> Option<hertzforge_dsp::resonance::ResonanceEstimate> {
        match &self.scheduler {
            SchedulerState::Rstd(r) => r.estimate(),
            _ => None,
        }
    }

    /// The RSTD captured phase setpoint (radians), if available.
    pub fn rstd_setpoint_rad(&self) -> Option<f64> {
        match &self.scheduler {
            SchedulerState::Rstd(r) => r.setpoint_rad(),
            _ => None,
        }
    }

    /// The last-committed drive scale (for tests / status).
    pub fn drive_scale(&self) -> f64 {
        self.current_drive_scale
    }

    /// The COLA aux ramp factor in `[0, 1]` (1.0 while running, decaying to 0
    /// through a stop/abort ramp).
    pub fn cola_scale(&self) -> f64 {
        self.cola_ramp_scale
    }

    /// **EVERY drive output is at zero** - the predicate the two-phase stop gates
    /// the backend stop on.
    ///
    /// `ch0 == 0 AND (COLA off OR its aux is also at 0)`. The COLA half is the
    /// part that is easy to leave out and expensive to leave out: at
    /// `out_channels = 2` the aux rides a SECOND physical DAC, and a stop that
    /// only checked ch0 would stop the tasks while that DAC still held its last
    /// full-amplitude sample - which the device then holds forever. The two
    /// scales are independent state and do NOT arrive at zero together: the
    /// servo's drive scale at the end of a run is typically orders of magnitude
    /// below the COLA reference amplitude, so it reaches the -120 dB floor first.
    pub fn drive_is_zero(&self) -> bool {
        self.current_drive_scale == 0.0 && (!self.cola.enabled || self.cola_ramp_scale == 0.0)
    }

    /// Worst-case time for the stop ramp to reach the [`RAMP_FLOOR_SCALE`] floor
    /// from `from_scale`, in seconds.
    ///
    /// This is what makes the safe-stop budget CHECKABLE at set_profile rather
    /// than discovered on a shaker: a host refuses a profile whose ramp cannot
    /// beat its controller timeout, and it can only do that if the ramp states a
    /// duration. The old per-frame law could not - its duration was a function of
    /// a sample rate nobody passed in.
    pub fn safe_stop_seconds(from_scale: f64) -> f64 {
        if !from_scale.is_finite() || from_scale <= RAMP_FLOOR_SCALE {
            return 0.0;
        }
        20.0 * (from_scale / RAMP_FLOOR_SCALE).log10() / RAMP_DB_PER_S
    }

    /// See every control tick that runs control ([`TickObservation`]). For tests
    /// and diagnostics; production runs install none.
    pub fn observe_ticks(&mut self, observer: TickObserver) {
        self.observer = Some(observer);
    }

    /// Take every output to zero at once. Only for a run that ends before
    /// anything was played (a law that could not be engaged).
    pub fn zero_drive(&mut self) {
        self.current_drive_scale = 0.0;
        self.cola_ramp_scale = 0.0;
    }

    /// Whether a stop/abort ramp is in progress.
    pub fn is_ramping_down(&self) -> bool {
        self.ramping_down
    }

    /// Begin the stop ramp WITHOUT going through the mailbox.
    ///
    /// The mailbox is drained at a control TICK, so a stop posted into it is not
    /// acted on until the next one. That is the right latency for an operator
    /// intent and the wrong one for the service's own terminal paths (a fault
    /// arm in `run_one_batch`, an abort), which must start the drive moving
    /// toward zero before they return.
    pub fn begin_ramp_down(&mut self) {
        self.ramping_down = true;
    }

    // --- the control law -------------------------------------------------------

    /// Give the step the law it runs. `descriptor` sizes the values buffer and
    /// names the servo's values for an observer; `guards` are the core's
    /// guards (`None`: none but the law's own); `clock` times the calls under
    /// them. Before the loop engages.
    pub fn install_law(
        &mut self,
        law: Box<dyn SineControlLaw + Send>,
        descriptor: &LawDescriptor,
        guards: Option<LawGuards>,
        clock: Arc<dyn LawClock>,
    ) {
        let tick_s = self.control_tick_frames as f64 / self.fs_hz;
        self.law_budget_s = guards
            .as_ref()
            .map(|g| g.call_budget_s(tick_s))
            .unwrap_or(f64::INFINITY);
        self.law = law;
        self.law_buf = LawValues::new(descriptor.diagnostics.len());
        let ix = |key: &str| descriptor.diagnostics.iter().position(|d| d == key);
        self.obs_ix = (ix("servo_step_db"), ix("servo_level_db"));
        self.law_guards = guards;
        self.law_clock = Some(clock);
        self.live_keys = descriptor.live.iter().map(|l| l.key.clone()).collect();
        self.live_values = descriptor
            .live
            .iter()
            .map(|l| {
                descriptor
                    .params
                    .iter()
                    .find(|(k, _)| *k == l.key)
                    .and_then(|(_, v)| match v {
                        crate::law::ParamSetting::Number(x) => Some(*x),
                        crate::law::ParamSetting::Integer(i) => Some(*i as f64),
                        _ => None,
                    })
                    .unwrap_or(f64::NAN)
            })
            .collect();
    }

    /// The law's live parameters and their values in force.
    pub fn live(&self) -> impl Iterator<Item = (&str, f64)> + '_ {
        self.live_keys
            .iter()
            .map(String::as_str)
            .zip(self.live_values.iter().copied())
    }

    /// The time one law call may take, seconds (infinite without the guards).
    pub fn law_budget_s(&self) -> f64 {
        self.law_budget_s
    }

    /// The sweep rate now, Hz per second, signed (0 outside a sweep).
    fn sweep_rate_hz_per_s(&self) -> f64 {
        match &self.scheduler {
            SchedulerState::Sweep(s) => s.rate_hz_per_s(),
            _ => 0.0,
        }
    }

    /// The law's starting drive: call `engage` at the start frequency and
    /// `start_level_db`, apply the first-drive bound under the guards, and make
    /// it the drive in force. Before the lead-in is synthesized, so the first
    /// frame played is the law's. An unusable engage is returned: the run
    /// ends before anything is driven at test level.
    pub fn engage_law(&mut self, start_level_db: f64) -> Result<f64, LawFault> {
        let f = self.freq_hz();
        let dt = self.control_tick_frames as f64 / self.fs_hz;
        let rung_scale = 10f64.powf(start_level_db / 20.0);
        let target = self.profile.target_accel_g_pk(f).unwrap_or(0.0).max(1e-12) * rung_scale;
        let prediction_db = self.feedforward.as_ref().and_then(|ff| ff.ff_db(f));
        let timing = self
            .law_guards
            .as_ref()
            .and(self.law_clock.as_deref())
            .map(|c| (c, self.law_budget_s));
        let inputs = TickInputs {
            tick_index: 0,
            dt_s: dt,
            frequency_hz: f,
            sweep_rate_hz_per_s: self.sweep_rate_hz_per_s(),
            level_db: start_level_db,
            target,
            measured: 0.0,
            channel_amplitude: &self.control_phase_buf,
            channel_phase_deg: &self.control_phase_buf,
            drive: 0.0,
            prediction_db,
            hold: Hold::Free,
            notch_ceiling: None,
            settling: true,
        };
        let (outcome, call_s) = call_law(&mut *self.law, true, &inputs, &mut self.law_buf, timing);
        self.law_calls += 1;
        self.law_values = Some(self.law_buf);
        let out = outcome.inspect_err(|f| {
            self.law_misses = 1;
            self.law_last_fault = Some(f.clone());
        })?;
        self.law_faulted = out.faulted;
        let mut drive = out.drive;
        let mut capped = false;
        if self.law_guards.is_some() {
            let bound = prediction_db
                .map(|db| 10f64.powf((db + start_level_db + FIRST_DRIVE_MARGIN_DB) / 20.0))
                .unwrap_or(FIRST_DRIVE_UNTRUSTED_FRACTION * rung_scale)
                .min(self.drive_guard.vpk_ceiling);
            if drive > bound {
                drive = bound;
                capped = true;
                self.law_capped_ticks += 1;
            }
        }
        self.law_records.push(LawRecord {
            tick: 0,
            freq_hz: f,
            target,
            measured: 0.0,
            drive_in_force: 0.0,
            prediction_db: prediction_db.unwrap_or(f64::NAN),
            hold: LawRecord::hold_code(Hold::Free),
            output: out.drive,
            miss: 0,
            capped,
            applied: drive,
            call_us: (call_s * 1e6) as f32,
            values: LawRecord::values_of(&self.law_buf),
        });
        self.current_drive_scale = drive;
        if let Some(slot) = self.frame_ctx.first_mut() {
            slot.drive_scale = drive;
        }
        Ok(drive)
    }

    /// Change the law's live parameter `key` (the service has checked the key
    /// is declared live and the value in range). Between batches; the value
    /// applies from the next tick. A refusal leaves the value in force.
    pub fn adjust_law(&mut self, key: &str, value: f64) -> Result<(), LawFault> {
        let law = &mut self.law;
        let r =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| law.adjust(key, value)))
            {
                Ok(r) => r,
                Err(p) => Err(LawFault::Crashed(panic_text(&*p))),
            };
        if r.is_ok() {
            if let Some(i) = self.live_keys.iter().position(|k| k == key) {
                self.live_values[i] = value;
            }
        }
        r
    }

    /// The law's counters: calls, unusable calls in a row, ticks a cap bound,
    /// the last unusable call's fault, and its own fault judgement.
    pub fn law_counters(&self) -> (u64, u32, u64, Option<&LawFault>, bool) {
        (
            self.law_calls,
            self.law_misses,
            self.law_capped_ticks,
            self.law_last_fault.as_ref(),
            self.law_faulted,
        )
    }

    /// One tick of the law: call it, judge the call under the guards, cap
    /// what it asks for, count, record. Returns the drive the core takes from
    /// the law, before the notch and the drive guard.
    ///
    /// Without guards the law's drive is taken as it is (the shipped law
    /// clamps itself to 1 dB a tick), which is the controller's original
    /// arithmetic. With them: a late, failed, panicked or refused call is a
    /// miss and the drive in force is held; `max_misses` in a row take the
    /// controlled stop. A usable call is capped to `max_step_db` either way
    /// and `max_rise_db_per_s` up from the drive in force; the cap binding for
    /// `divergence_s` without a break takes the controlled stop. While a
    /// resonance search maps the plant the law's drive is never used.
    fn law_decision(
        &mut self,
        f: f64,
        dt: f64,
        target: f64,
        measured: f64,
        hold: Hold,
        settling: bool,
    ) -> f64 {
        let in_force = self.current_drive_scale;
        // The control channels' phases against the drive, for the law.
        for (k, &slot) in self.control_slots.iter().enumerate() {
            if let Some(p) = self.control_phase_buf.get_mut(k) {
                *p = wrap_deg(
                    (self.last_phase.get(slot).copied().unwrap_or(0.0) + self.phase_offset)
                        .to_degrees(),
                );
            }
        }
        let prediction_db = self.feedforward.as_ref().and_then(|ff| ff.ff_db(f));
        let notch_ceiling = self
            .notch
            .as_ref()
            .map(|g| g.ceiling())
            .filter(|c| c.is_finite());
        self.law_tick_index += 1;
        let timing = self
            .law_guards
            .as_ref()
            .and(self.law_clock.as_deref())
            .map(|c| (c, self.law_budget_s));
        let inputs = TickInputs {
            tick_index: self.law_tick_index,
            dt_s: dt,
            frequency_hz: f,
            sweep_rate_hz_per_s: self.sweep_rate_hz_per_s(),
            level_db: self.ladder.level_db(),
            target,
            measured,
            channel_amplitude: &self.control_amps_buf,
            channel_phase_deg: &self.control_phase_buf,
            drive: in_force,
            prediction_db,
            hold,
            notch_ceiling,
            settling,
        };
        let (outcome, call_s) = call_law(&mut *self.law, false, &inputs, &mut self.law_buf, timing);
        self.law_calls += 1;
        self.law_values = Some(self.law_buf);

        let mut record = LawRecord {
            tick: self.law_tick_index,
            freq_hz: f,
            target,
            measured,
            drive_in_force: in_force,
            prediction_db: prediction_db.unwrap_or(f64::NAN),
            hold: LawRecord::hold_code(hold),
            output: f64::NAN,
            miss: 0,
            capped: false,
            applied: in_force,
            call_us: (call_s * 1e6) as f32,
            values: LawRecord::values_of(&self.law_buf),
        };

        let Some(guards) = self.law_guards.clone() else {
            // No guards: the law's drive as it is; a failed call holds the
            // drive in force. The search ignores the law's drive.
            let drive = match outcome {
                Ok(out) => {
                    self.law_faulted = out.faulted;
                    record.output = out.drive;
                    out.drive
                }
                Err(fault) => {
                    record.miss = miss_code(Some(&fault));
                    self.law_last_fault = Some(fault);
                    in_force
                }
            };
            let drive = if hold == Hold::Probe { in_force } else { drive };
            record.applied = drive;
            self.law_records.push(record);
            return drive;
        };

        let drive = match outcome {
            Err(fault) => {
                self.law_misses += 1;
                record.miss = miss_code(Some(&fault));
                self.capped_streak_s = 0.0;
                if self.law_misses >= guards.max_misses {
                    let consecutive = self.law_misses;
                    self.law_last_fault = Some(fault.clone());
                    self.law_records.push(record);
                    self.law_stop(TripKind::LawFault { fault, consecutive }, f);
                    return in_force;
                }
                self.law_last_fault = Some(fault);
                in_force
            }
            Ok(out) => {
                self.law_misses = 0;
                self.law_faulted = out.faulted;
                record.output = out.drive;
                if hold == Hold::Probe {
                    in_force
                } else {
                    let (capped_drive, capped) = cap_drive(in_force, out.drive, &guards, dt);
                    record.capped = capped;
                    if capped {
                        self.law_capped_ticks += 1;
                        self.capped_streak_s += dt;
                        if self.capped_streak_s >= guards.divergence_s - 1e-12 {
                            let capped_s = self.capped_streak_s;
                            record.applied = in_force;
                            self.law_records.push(record);
                            self.law_stop(TripKind::Divergence { capped_s }, f);
                            return in_force;
                        }
                    } else {
                        self.capped_streak_s = 0.0;
                    }
                    capped_drive
                }
            }
        };
        record.applied = drive;
        self.law_records.push(record);
        drive
    }

    /// The law is done: hold the drive in force, take the controlled stop
    /// (the 120 dB/s ramp, then the drain), and do not call the law again.
    fn law_stop(&mut self, kind: TripKind, f: f64) {
        let record = TripRecord {
            kind,
            at_freq_hz: f,
            at_level_db: self.ladder.level_db(),
            at_tick: self.tick_count,
        };
        self.abort.set_controlled(record);
        self.law_stopped = true;
        self.stop_requested.store(true, Ordering::Release);
        self.ramping_down = true;
    }

    // --- the control tick ------------------------------------------------------

    /// Run one control tick. This thin wrapper reuses the preallocated
    /// `broadband_buf` (moved out via `mem::take`, refilled in place, moved back)
    /// so no per-tick `Vec` is allocated in steady state. The moved-out buffer is
    /// a plain local, so the tick body can freely take `&mut self` while reading
    /// it (the borrow is on the local, not on `self`).
    fn control_tick(&mut self) {
        // Reuse the buffer's allocation: take it out, refill in place from the
        // sum-of-squares accumulators, run the tick body, then put it back.
        let mut broadband = std::mem::take(&mut self.broadband_buf);
        let n_frames = self.tick_frames.max(1) as f64;
        broadband.clear();
        broadband.extend(self.broadband_ss.iter().map(|&ss| (ss / n_frames).sqrt()));
        self.control_tick_body(&broadband);
        self.broadband_buf = broadband;
    }

    /// The control-tick logic. `broadband` is the per-channel RMS this tick,
    /// borrowed from a caller-owned local so this method can take `&mut self`.
    fn control_tick_body(&mut self, broadband: &[f64]) {
        let dt = self.control_tick_frames as f64 / self.fs_hz;

        // 1. Drain the command mailbox.
        for cmd in self.mailbox.drain() {
            self.apply_command(cmd);
        }

        // 3. If a ramp-down is underway (post-abort), do not run control; just
        //    let the drive decay and publish. The service ends the run on the
        //    abort latch between chunks; the ramp keeps the drive from cliffing.
        if self.ramping_down || self.abort.is_tripped() {
            // The history stays CONTINUOUS through a ramp-down. A raw-ring trip
            // during the ramp is exactly the case where "what was the drive
            // doing" has a fast-moving answer, and a stale pre-ramp snapshot
            // would over-state it.
            self.tick_freq_hz = self.freq_hz();
            let m = self.control_composite();
            self.push_frame_context(m);
            self.publish_snapshot(broadband, false, false);
            self.reset_tick_accumulators();
            return;
        }

        let f = self.freq_hz();
        self.tick_freq_hz = f;

        // 3b. Level-settle window: on a rung change (or at engage), re-open the
        //     settle window so the protections stay suppressed while the startup
        //     ramp brings the response onto the band. This is what stops the
        //     low-side scalar abort from tripping on tick 0 (measured ~ 0 before
        //     the drive has ramped). `settling` gates the scalar abort, the
        //     servo-fault, and the kinematic monitor below.
        let level_now = self.ladder.level_db();
        if (level_now - self.last_level_db).abs() > 1e-9 {
            self.settle_remaining = self.settle_ticks;
            self.last_level_db = level_now;
        }
        let mut settling = self.settle_remaining > 0;
        if settling {
            self.settle_remaining -= 1;
        }
        // The RSTD search / settle / capture phases run a LOW-LEVEL probe that is
        // deliberately off the control target, so the control-profile scalar
        // alarm/abort + servo-fault + kinematic monitor must NOT fire there (they
        // are for the phase-tracked DWELL that follows). Treat those phases like
        // the settle window. During the RSTD Dwell the protections re-arm.
        //
        // `rstd_probe` additionally FREEZES the drive during those phases: the
        // RSTD search maps the resonance by observing the tracked amplitude vs
        // frequency, which only works if the drive is HELD FIXED so the response
        // shows the true resonance peak. If the servo + feedforward controlled to
        // the flat profile target during the search, they would flatten the very
        // peak the search is trying to find (once the feedforward actually engages,
        // which it now does after the DC-bin fix). So during the probe we hold
        // the servo and bypass the feedforward slope.
        let mut rstd_probe = false;
        if let SchedulerState::Rstd(r) = &self.scheduler {
            if matches!(
                r.state(),
                crate::scheduler::RstdState::Search
                    | crate::scheduler::RstdState::Settle
                    | crate::scheduler::RstdState::Capture
            ) {
                settling = true;
                rstd_probe = true;
            }
        }
        // Re-open the settle window on the RSTD probe -> Dwell transition. The
        // probe froze the drive at the (off-resonance) band.0 level and bypassed
        // feedforward, so at Capture -> Dwell the response rings at ~Q*target
        // (26 dB at Q=20, 34 dB at Q=50). Without a settle window the high-side
        // scalar abort / limit abort / kinematic monitor arm immediately while the
        // servo can only ramp the frozen-hot drive down at ~0.117 dB/tick, and the
        // abort debounce (3 ticks) trips spuriously on a realistic 3-6 dB band.
        // Re-opening the window lets the servo ramp the drive down to the
        // compressed level BEFORE those protections arm. This is NOT a level
        // change (the ladder rung is unchanged), so the rung-change branch above
        // does not cover it.
        if self.was_rstd_probe && !rstd_probe {
            self.settle_remaining = self.settle_ticks;
            settling = self.settle_remaining > 0;
            if settling {
                self.settle_remaining -= 1;
            }
        }

        // 4. Composite the control channels' tracked amplitudes.
        let measured = self.control_composite();

        // 5. The target at this frequency and level, which the law controls to.
        let rung_scale = 10f64.powf(self.ladder.level_db() / 20.0);
        let target_g = self.profile.target_accel_g_pk(f).unwrap_or(0.0).max(1e-12) * rung_scale;

        // 6. Observe the limit channels and compute the notch ceiling BEFORE the
        //    law, so it is told whether the ceiling binds (it must not raise the
        //    drive, and the built-in holds its servo rather than wind it up).
        let notch_binding = self.observe_notch(f, measured);

        // 7-8. THE CONTROL LAW decides the drive (the servo, the blend and the
        //    drive decision were here; see `law_decision` for the guards).
        let hold = if rstd_probe {
            Hold::Probe
        } else if notch_binding {
            Hold::Notch
        } else {
            Hold::Free
        };
        let mut requested = self.law_decision(f, dt, target_g, measured, hold, settling);
        if self.law_stopped && self.ramping_down {
            // The law's misses (or its divergence) took the controlled stop on
            // this tick: nothing below runs on a drive the law no longer owns.
            self.publish_snapshot(broadband, notch_binding, false);
            self.reset_tick_accumulators();
            return;
        }
        if !requested.is_finite() || requested < 0.0 {
            requested = 0.0;
        }
        let feed = measured / target_g.max(1e-12);

        // 9. Apply the notch governor ceiling. The apply-time
        //    `binding` flag is the AUTHORITATIVE "the ceiling actually clamped
        //    this tick's requested drive" signal - truer than the observe-time
        //    `current_drive_scale >= ceiling` approximation, which flickers on a
        //    knife-edge when the servo holds the drive right at the ceiling. The
        //    low-side abort + servo-fault are gated on the OR of the two so a
        //    single-tick flicker of the observe-time approximation cannot open a
        //    false-abort gap while the notch is legitimately holding the control
        //    point down.
        let mut binding_now = notch_binding;
        if let Some(gov) = self.notch.as_mut() {
            let (applied, clamped) = gov.apply(requested);
            requested = applied;
            binding_now = binding_now || clamped;
        }
        // Hysteresis: refresh the notch-active hold window on any binding tick;
        // otherwise decay it. `effective_notched` reads the latch, so a sustained
        // notch hold (whose instantaneous binding signal flickers on the knife
        // edge at the ceiling) is treated as continuously notched for the
        // low-side scalar abort + servo-fault gates. The hold window mirrors the
        // scalar-abort debounce so a notch that only glanced the ceiling once does
        // not indefinitely suppress a real fault.
        if binding_now {
            self.notch_hold_remaining = self.scalar_abort_debounce.max(1) + 1;
        } else {
            self.notch_hold_remaining = self.notch_hold_remaining.saturating_sub(1);
        }
        let effective_notched = binding_now || self.notch_hold_remaining > 0;
        // Keep the published `notch_binding` reflecting the latched active state so
        // the status + trace show the notch as engaged through a sustained hold.
        let notch_binding = effective_notched;

        // 10. Drive Vpk guard.
        match self.drive_guard.check(requested) {
            DriveVerdict::Ok => {}
            DriveVerdict::Clamped(v) => requested = v,
            DriveVerdict::Abort => {
                self.trip(
                    TripKind::DriveVpk {
                        realized_vpk: requested,
                        limit_vpk: self.drive_guard.vpk_abort,
                    },
                    f,
                );
                self.publish_snapshot(broadband, notch_binding, false);
                self.reset_tick_accumulators();
                return;
            }
        }
        self.current_drive_scale = requested;

        // 11. Scalar alarm/abort at the current frequency. The abort is
        //     DEBOUNCED (N consecutive out-of-band ticks) and SUPPRESSED during
        //     the level-settle window so the startup ramp is not aborted before
        //     the response reaches the band.
        let mut alarm_active = false;
        if let Some((lo, hi)) = self.profile.abort_bounds(f) {
            // The abort bounds are at the FULL profile; scale to the current
            // rung so a -12 dB engage does not trip the full-level abort floor.
            let (lo, hi) = (lo * rung_scale, hi * rung_scale);
            // HIGH-side over-test: a genuine over-test is a fault EVEN while the
            // notch binds (the notch protects a limit channel, it does not license
            // the control point to run hot), so keep it armed on `!settling` alone.
            let out_high = measured > hi;
            // LOW-side under-test: only a fault when NOT notch-binding.
            // When a more-sensitive limit channel holds the control point below its
            // abort-low band, that is the NotchGovernor correctly protecting a
            // shared resonance - a legitimately-notched under-test, not a fault.
            // Also require the drive to be meaningfully up (a low read while the
            // drive is still near zero is a settling artifact caught by the
            // servo-fault path instead).
            let out_low = measured < lo
                && measured > 0.0
                && self.current_drive_scale > 1e-9
                && !notch_binding;
            let tripworthy = !settling && (out_high || out_low);
            if tripworthy {
                self.scalar_abort_streak += 1;
                if self.scalar_abort_streak >= self.scalar_abort_debounce {
                    let (side, limit_g) = if out_high {
                        ("high".to_string(), hi)
                    } else {
                        ("low".to_string(), lo)
                    };
                    self.trip(
                        TripKind::ScalarAbort {
                            side,
                            measured_g: measured,
                            limit_g,
                        },
                        f,
                    );
                    self.publish_snapshot(broadband, notch_binding, false);
                    self.reset_tick_accumulators();
                    return;
                }
            } else {
                self.scalar_abort_streak = 0;
            }
        }
        if !settling {
            if let Some((lo, hi)) = self.profile.alarm_bounds(f) {
                let (lo, hi) = (lo * rung_scale, hi * rung_scale);
                if measured > hi || (measured < lo && measured > 0.0) {
                    alarm_active = true;
                }
            }
        }

        // 12. The limit-channel abort: any limit channel past its OWN abort band
        //     (suppressed while settling).
        if !settling {
            if let Some(trip) = self.limit_channel_abort(f) {
                self.trip(trip, f);
                self.publish_snapshot(broadband, notch_binding, false);
                self.reset_tick_accumulators();
                return;
            }
        }

        // 13. Analytic kinematic monitor: the tracked response's
        //     analytic v / d vs the shaker ratings, exact for a sine (suppressed
        //     while settling).
        if self.kinematic_monitor && !settling {
            if let Some(trip) = self.kinematic_abort(measured, f) {
                self.trip(trip, f);
                self.publish_snapshot(broadband, notch_binding, false);
                self.reset_tick_accumulators();
                return;
            }
        }

        // 14. Debounced servo-fault check: the composite collapsed
        //     (measured/target far below the fault fraction) while the drive is
        //     up. Suppressed while settling so the startup ramp is not flagged.
        //
        //     NOT gated on `!notch_binding`: the servo-fault threshold
        //     (feed < fault_fraction = 1e-2 = -40 dB) is a CATASTROPHIC
        //     control-loss detector, not an under-test detector. A legitimately
        //     notched under-test holds the control point at a MEANINGFUL level
        //     (a more-sensitive limit channel clamps the drive, but the control
        //     composite still tracks well above 1e-2), so this detector does not
        //     false-trip the tolerated notch case. Gating it on `!notch_binding`
        //     (as an earlier version did) opened a real gap: if the CONTROL channel
        //     dies (cable-fails-open) mid-dwell while a limit channel keeps
        //     reading and holds the notch, feed -> 0 (servo-fault collapse) but every
        //     other abort was masked - low-side scalar notch-gated, high-side and
        //     kinematic see 0, limit-channel abort only guards the limit
        //     channel's own excursion - so the run drove on with a dead control
        //     loop. Ungating catches that dead control channel; the notch case is
        //     safe because its feed stays well above 1e-2. The low-side SCALAR
        //     abort (the -3 dB band, step 11) stays notch-gated by design.
        if self.fault_debounce_ticks > 0 && !settling {
            let collapsed = feed < self.fault_fraction && self.current_drive_scale > 0.0;
            if collapsed {
                self.fault_streak += 1;
                if self.fault_streak >= self.fault_debounce_ticks {
                    self.trip(
                        TripKind::ServoFault {
                            consecutive_ticks: self.fault_streak,
                        },
                        f,
                    );
                    self.publish_snapshot(broadband, notch_binding, false);
                    self.reset_tick_accumulators();
                    return;
                }
            } else {
                self.fault_streak = 0;
            }
        }

        // 15. Overload proxy per control channel: |tracked| beyond 95% of the
        //     abort-band high edge latches the indicator (not an abort).
        if let Some((_, hi)) = self.profile.abort_bounds(f) {
            let hi = hi * rung_scale;
            for (slot, latched) in self
                .control_slots
                .iter()
                .zip(self.overload_latched.iter_mut())
            {
                let amp = self.tracked.get(*slot).copied().unwrap_or(0.0);
                if amp >= 0.95 * hi {
                    *latched = true;
                }
            }
        }

        // 16. Advance the scheduler + the ladder (test time). The ladder
        //     advances on completed sweeps (sweep mode feeds note_sweep_completed
        //     from the scheduler) or on seconds.
        self.advance_scheduler(dt);
        self.retune(self.freq_hz());

        // 16b. Record what this tick COMMITTED, tagged with the first frame it
        //      applies to. Everything a trip record needs to quote
        //      the breaching frame is decided by here: the retuned NCO command,
        //      the ladder rung, the drive scale applied at step 10, and the
        //      composite the servo acted on.
        self.push_frame_context(measured);

        // 17. Publish the snapshot + the swept trace points.
        self.publish_snapshot(broadband, notch_binding, alarm_active);
        self.trace.push(f, &self.tracked, broadband);
        if let Some(obs) = self.observer.as_mut() {
            let value = |ix: Option<usize>| ix.and_then(|i| self.law_buf.get(i)).unwrap_or(0.0);
            obs(&TickObservation {
                tick: self.tick_count,
                freq_hz: f,
                level_db: self.ladder.level_db(),
                measured,
                drive: self.current_drive_scale,
                servo_step_db: value(self.obs_ix.0),
                servo_level_db: value(self.obs_ix.1),
                servo_faulted: self.law_faulted,
                notch_binding,
            });
        }

        // Remember whether this tick was an RSTD probe so the NEXT tick can
        // detect the probe -> Dwell transition and re-open the settle window.
        self.was_rstd_probe = rstd_probe;

        self.tick_count += 1;
        self.reset_tick_accumulators();
    }

    /// THE DELAY-COMPENSATED DEMODULATION REFERENCE.
    ///
    /// Push this frame's NCO frequency into the ring and return the frequency the
    /// NCO held `tracking_lag_frames` frames ago - which is the frequency of the
    /// signal that is arriving NOW, because the sample on the wire is an echo of
    /// drive emitted that long ago.
    ///
    /// # Why this is the whole fix
    ///
    /// The lock-in multiplies the incoming sample by a reference at whatever
    /// frequency it is handed. Hand it `f(n)` while the signal sits at
    /// `f(n - lag)` and the wanted phasor lands at the difference instead of at
    /// baseband, where the two-pole low-pass attenuates it by
    /// `1/(1 + (df/bw)^2)`. Hand it `f(n - lag)` and the phasor lands where it
    /// belongs. MEASURED (hertzforge-dsp `tests/tracking_delay.rs`): the probe's own
    /// 60-500 Hz sweep at 60 oct/min through a `FixedHz(10)` corner with this
    /// 2089-frame lag goes from -9.17 dB of droop at 500 Hz to +0.03 dB.
    ///
    /// The FRF path already compensates this lag (it shifts the whole record by
    /// the cross-correlation peak before estimating H); the closed-loop path did
    /// not, and that asymmetry is the chassis tracking-lag overdrive.
    ///
    /// # What it does to the phase readout, exactly
    ///
    /// The reference phase becomes `phi(n - lag) + 2*pi*f_start*tau` - the
    /// constant coming from the seeded startup. A CONSTANT phase offset does not
    /// touch the magnitude, and it is exactly zero when the frequency is not
    /// moving (at a dwell `phi(n) = phi(n - lag) + 2*pi*f*tau`, so the reference
    /// is bit-for-bit what it was before this fix existed). RSTD captures its
    /// phase setpoint through the same filter it then tracks with, so a constant
    /// common offset cancels there too.
    ///
    /// # The lag is the sysID's, and the sysID's is a floor
    ///
    /// `tracking_lag_frames` is what the sysID cross-correlation measured on the
    /// burst path. The ENGAGED run adds the AO delivery queue on top of that, so
    /// the true closed-loop lag can be larger and this compensation can
    /// under-correct. Under-correcting leaves the residual droop of the
    /// UNCOMPENSATED part only - strictly better than not compensating - and the
    /// tracking-lag gate is sized against the same measured number, so the two agree
    /// about what they know.
    #[inline]
    fn reference_freq(&mut self, inst_freq: f64) -> f64 {
        if self.freq_delay.is_empty() {
            return inst_freq;
        }
        let head = self.freq_delay_head;
        let delayed = self.freq_delay[head];
        self.freq_delay[head] = inst_freq;
        self.freq_delay_head = (head + 1) % self.freq_delay.len();
        delayed
    }

    /// The control channels' tracked amplitudes composited under the configured
    /// [`CompositeMode`]. Reuses the preallocated scratch, so it allocates
    /// nothing on the control tick.
    fn control_composite(&mut self) -> f64 {
        let mut control_amps = std::mem::take(&mut self.control_amps_buf);
        control_amps.clear();
        control_amps.extend(
            self.control_slots
                .iter()
                .map(|&s| self.tracked.get(s).copied().unwrap_or(0.0)),
        );
        let m = composite_amplitude(&control_amps, &self.control_weights, self.composite_mode)
            .unwrap_or(0.0);
        self.control_amps_buf = control_amps;
        m
    }

    /// Observe the limit channels and compute the notch ceiling. Returns whether
    /// the ceiling will bind the requested (up) drive. Also uses the control
    /// channel's measured amplitude to derive the drive-to-response sensitivity
    /// only when it makes sense; here the notch uses the LIMIT channels' tracked
    /// amplitudes vs the current drive scale.
    fn observe_notch(&mut self, f: f64, _control_measured: f64) -> bool {
        if self.notch.is_none() || self.limit_channels.is_empty() {
            return false;
        }
        // The scratch is taken BEFORE the governor is borrowed mutably: both live
        // on `self`, and the borrow checker sees the pair, not the disjointness.
        let mut ticks = std::mem::take(&mut self.notch_ticks_buf);
        ticks.clear();
        ticks.extend(self.limit_channels.iter().map(|lc| {
            let tracked = self.tracked.get(lc.input_slot).copied().unwrap_or(0.0);
            let limit = lc
                .profile
                .abort_bounds(f)
                .map(|(_, hi)| hi)
                .unwrap_or(f64::INFINITY);
            let sysid_h = lc.sysid_h.as_ref().map(|(freqs, h)| interp_at(freqs, h, f));
            LimitChannelTick {
                tracked_amp: tracked,
                limit_amp_at_f: limit,
                sysid_h_mag: sysid_h,
            }
        }));
        let scale = self.current_drive_scale;
        let ceiling = {
            let gov = self.notch.as_mut().expect("checked above");
            let _ = gov.observe(scale, &ticks);
            // "Binding" is decided at apply() time against the requested scale;
            // but for the servo hold decision we approximate: the ceiling binds
            // if the ceiling is below the current drive scale (the servo would
            // push up into it). This is the notch hold trigger.
            gov.ceiling()
        };
        self.notch_ticks_buf = ticks;
        ceiling.is_finite() && self.current_drive_scale >= ceiling
    }

    /// The limit-channel abort: any limit channel whose tracked amplitude left
    /// its OWN abort band's high edge at the current frequency.
    fn limit_channel_abort(&self, f: f64) -> Option<TripKind> {
        for lc in &self.limit_channels {
            let tracked = self.tracked.get(lc.input_slot).copied().unwrap_or(0.0);
            if let Some((_, hi)) = lc.profile.abort_bounds(f) {
                if tracked > hi {
                    return Some(TripKind::LimitChannel {
                        channel_index: lc.channel_index,
                        measured: tracked,
                        limit: hi,
                    });
                }
            }
        }
        None
    }

    /// The analytic kinematic abort.
    fn kinematic_abort(&self, measured_g: f64, f: f64) -> Option<TripKind> {
        if let Some(max_v) = self.max_velocity_ips {
            let v = tracked_velocity_ips_pk(measured_g, f);
            if v > max_v {
                return Some(TripKind::Kinematic {
                    axis: "velocity".into(),
                    value: v,
                    limit: max_v,
                });
            }
        }
        if let Some(max_d) = self.max_displacement_in_pp {
            let d = tracked_displacement_in_pp(measured_g, f);
            if d > max_d {
                return Some(TripKind::Kinematic {
                    axis: "displacement".into(),
                    value: d,
                    limit: max_d,
                });
            }
        }
        None
    }

    /// Latch an abort and begin the drive ramp-down (never a cliff).
    fn trip(&mut self, kind: TripKind, f: f64) {
        let record = TripRecord {
            kind,
            at_freq_hz: f,
            at_level_db: self.ladder.level_db(),
            at_tick: self.tick_count,
        };
        self.abort.set(record);
        self.ramping_down = true;
    }

    /// Advance the active scheduler (and, for sweep, feed completed sweeps into
    /// the ladder). All hold/level state lives in the scheduler + ladder.
    fn advance_scheduler(&mut self, dt: f64) {
        // Read the RSTD-tracked channel's amp/phase up front so we do not borrow
        // `self` immutably while `&mut self.scheduler` is held (RSTD only).
        let (rstd_amp, rstd_phase) = self.control_channel_amp_phase();
        match &mut self.scheduler {
            SchedulerState::Sweep(s) => {
                let before = s.sweeps_done();
                s.tick(dt);
                let after = s.sweeps_done();
                for _ in before..after {
                    self.ladder.note_sweep_completed();
                }
                // Seconds-based rungs also accrue from the sweep's test time.
                self.ladder.tick(dt);
            }
            SchedulerState::Dwell(d) => {
                d.tick(dt);
                self.ladder.tick(dt);
            }
            SchedulerState::StepSine(s) => {
                s.tick(dt);
                self.ladder.tick(dt);
            }
            SchedulerState::Rstd(r) => {
                let (_f, ev) = r.tick(dt, rstd_amp, rstd_phase);
                self.ladder.tick(dt);
                match ev {
                    Some(RstdEvent::SearchDone { .. })
                    | Some(RstdEvent::SetpointCaptured { .. })
                    | Some(RstdEvent::BandExit)
                    | Some(RstdEvent::DwellComplete)
                    | Some(RstdEvent::SearchFailed)
                    | None => { /* events surfaced by the service via rstd_* getters */ }
                }
            }
        }
    }

    /// The control channel's latest tracked amplitude and phase (for RSTD). Uses
    /// the FIRST control channel's filter (RSTD tracks one channel pair; the
    /// service pins the control channel as the tracked one).
    fn control_channel_amp_phase(&self) -> (f64, f64) {
        // The tracking filter's phase is captured in the last update; we keep the
        // amplitude in `tracked` and re-read the filter phase via a fresh no-op
        // is not possible, so RSTD phase comes from the last stored tick. Here we
        // return the tracked amplitude and a phase proxy of 0 when unavailable.
        // The service seeds the RSTD tracked channel as control_slots[0]; the
        // phase is carried by the filter and surfaced through `last_phase`.
        let slot = self.control_slots.first().copied().unwrap_or(0);
        let amp = self.tracked.get(slot).copied().unwrap_or(0.0);
        let phase = self.last_phase.get(slot).copied().unwrap_or(0.0);
        (amp, phase)
    }

    /// Retune the NCO to the new center frequency. The NCO phase is NEVER reset
    /// (phase-continuous); only its frequency moves.
    ///
    /// `try_set_frequency`, NOT `set_frequency`. This runs on EVERY control tick
    /// (step 16 of `control_tick_body`, from whatever the scheduler advanced to),
    /// not just on an operator jog - so the panicking form is reachable by an
    /// ORDINARY SWEEP whose band top has crossed `fs/2`, and a panic here unwinds
    /// the RT thread with the AO task still clocked and the 9263 holding its last
    /// sample forever. The scheduler's band clamp is what should make this
    /// unreachable; a refusal getting here means that clamp failed, which is a
    /// FAULT and not something to swallow. The oscillator is left untouched on a
    /// refusal, so the tone holds while the abort ramp runs.
    fn retune(&mut self, f: f64) {
        if let Err(e) = self.nco.try_set_frequency(f) {
            self.trip(
                TripKind::OscillatorRange {
                    requested_hz: e.requested_hz,
                    fs_hz: e.fs_hz,
                },
                self.nco.frequency(),
            );
        }
        // The AdaptiveTrackingFilter re-derives its bandwidth from f on update;
        // no explicit retune call is needed (it takes inst_freq each sample).
    }

    fn apply_command(&mut self, cmd: Command) {
        match cmd {
            Command::Stop => {
                self.stop_requested.store(true, Ordering::Release);
                self.ramping_down = true;
            }
            // The hold stops the level clock with the test's: a rung that lasts
            // so many seconds does not count them while the run is held.
            Command::Hold => {
                match &mut self.scheduler {
                    SchedulerState::Sweep(s) => s.hold(),
                    SchedulerState::Dwell(d) => d.hold(),
                    SchedulerState::StepSine(s) => s.hold(),
                    SchedulerState::Rstd(r) => r.hold(),
                }
                self.ladder.hold();
            }
            Command::Resume => {
                match &mut self.scheduler {
                    SchedulerState::Sweep(s) => s.resume(),
                    SchedulerState::Dwell(d) => d.resume(),
                    SchedulerState::StepSine(s) => s.resume(),
                    SchedulerState::Rstd(r) => r.resume(),
                }
                self.ladder.resume();
            }
            Command::Reverse => {
                if let SchedulerState::Sweep(s) = &mut self.scheduler {
                    s.reverse();
                }
            }
            Command::Jog(dhz) => {
                if let SchedulerState::Sweep(s) = &mut self.scheduler {
                    s.jog(dhz);
                    let f = s.freq_hz();
                    self.retune(f);
                    self.display_jump = true;
                    // A commanded frequency MOVE re-opens the settle window, same
                    // as a rung change: the tracking filters re-acquire at the new
                    // center and the tracked amplitude transiently collapses. The
                    // low-side scalar abort read that collapse as a dead loop on
                    // the chassis (2026-08-08 sitting: ScalarAbort low at 80 Hz
                    // full level, 0.0634 g vs 0.1416 g, tick 930 - the settle row's
                    // first trip record). The window suspends what it suspends at
                    // every rung change (both scalar sides, servo-fault, kinematic
                    // - see 3b); the drive Vpk ceiling stays live throughout.
                    self.settle_remaining = self.settle_ticks;
                }
            }
            Command::GoTo(hz) => {
                if let SchedulerState::Sweep(s) = &mut self.scheduler {
                    let f = s.go_to(hz);
                    self.retune(f);
                    self.display_jump = true;
                    // Same re-open as Jog: an absolute move is the same filter
                    // re-acquisition transient at the destination.
                    self.settle_remaining = self.settle_ticks;
                }
            }
            Command::StepLevel(ddb) => {
                self.ladder.step_db(ddb);
            }
            Command::JumpLevel(i) => {
                let _ = self.ladder.jump_to(i);
            }
            Command::SetLevelDb(db) => {
                let _ = self.ladder.jump_to_nearest_db(db);
            }
            Command::SetRunMode(mode) => {
                self.ladder.set_manual(matches!(mode, RunMode::Manual));
            }
            Command::Adjust { index, value } => {
                if let Some(key) = self.live_keys.get(index).cloned() {
                    let _ = self.adjust_law(&key, value);
                }
            }
        }
    }

    /// `&mut self` because the two per-channel slices are PREALLOCATED buffers
    /// refilled in place (no allocation on the tick that owns the drive
    /// clock). The `ControlSnapshot` the status cell stores still owns its own
    /// copies - a snapshot that borrowed the step's buffers could not outlive the
    /// tick, and the whole point of the cell is that a slow reader never stalls
    /// one.
    fn publish_snapshot(&mut self, broadband: &[f64], notch_binding: bool, alarm_active: bool) {
        let f = self.freq_hz();
        let rung_scale = 10f64.powf(self.ladder.level_db() / 20.0);
        let target_g = self.profile.target_accel_g_pk(f).unwrap_or(0.0) * rung_scale;
        let mut per_channel_tracked = std::mem::take(&mut self.tracked_buf);
        per_channel_tracked.clear();
        per_channel_tracked.extend(
            self.control_slots
                .iter()
                .map(|&s| self.tracked.get(s).copied().unwrap_or(0.0)),
        );
        let mut per_channel_broadband = std::mem::take(&mut self.bb_pub_buf);
        per_channel_broadband.clear();
        per_channel_broadband.extend(
            self.control_slots
                .iter()
                .map(|&s| broadband.get(s).copied().unwrap_or(0.0)),
        );
        let mut overload = std::mem::take(&mut self.overload_pub_buf);
        overload.clear();
        overload.extend_from_slice(&self.overload_latched);
        let measured = composite_amplitude(
            &per_channel_tracked,
            &self.control_weights,
            self.composite_mode,
        )
        .unwrap_or(0.0);
        let elapsed_test_s = match &self.scheduler {
            SchedulerState::Sweep(s) => s.elapsed_test_s(),
            SchedulerState::Dwell(d) => d.elapsed_s(),
            SchedulerState::StepSine(s) => s.progress().step_elapsed_s,
            SchedulerState::Rstd(r) => r.dwell_elapsed_s(),
        };
        let thd = self.thd_bank.thd(
            per_channel_tracked
                .first()
                .copied()
                .unwrap_or(0.0)
                .max(1e-12),
        );

        // What the live frame and the display add: every input's phase against
        // the drive, the limit bounds, the drive and reference as played, the
        // sweep's rate and direction, the ladder's rung. All refilled in place.
        let mut phase_deg = std::mem::take(&mut self.input_phase_buf);
        phase_deg.clear();
        phase_deg.extend(
            self.last_phase
                .iter()
                .map(|&p| wrap_deg((p + self.phase_offset).to_degrees())),
        );
        let f_meas = self.tick_freq_hz;
        let mut limit_bound = std::mem::take(&mut self.limit_bound_buf);
        limit_bound.clear();
        limit_bound.extend(self.limit_channels.iter().map(|lc| {
            lc.profile
                .abort_bounds(f_meas)
                .map(|(_, hi)| hi)
                .unwrap_or(f64::NAN)
        }));
        let t_s = self.frames_processed as f64 / self.fs_hz;
        let drive_vpk = self.current_drive_scale;
        let cola_vpk = self
            .cola
            .enabled
            .then_some(self.cola.amp * self.cola_ramp_scale);
        let notch_ceiling = self
            .notch
            .as_ref()
            .map(|g| g.ceiling())
            .filter(|c| c.is_finite());
        let (direction, sweep_rate_hz_per_s, sweep_rate_oct_per_min) = match &self.scheduler {
            SchedulerState::Sweep(s) => {
                (Some(s.direction()), s.rate_hz_per_s(), s.rate_oct_per_min())
            }
            _ => (None, 0.0, None),
        };
        let ladder = self.ladder.progress();
        let held = self.is_held();
        self.status.store_from(ControlSnapshotRef {
            freq_hz: f,
            level_db: self.ladder.level_db(),
            target_g,
            measured_g: measured,
            per_channel_tracked: &per_channel_tracked,
            per_channel_broadband: &per_channel_broadband,
            thd,
            notch_binding,
            alarm_active,
            sweeps_done: self.sweeps_done(),
            elapsed_test_s,
            overload_latched: &overload,
            t_s,
            frames: self.frames_processed,
            drive_vpk,
            cola_vpk,
            notch_ceiling,
            held,
            direction,
            sweep_rate_hz_per_s,
            sweep_rate_oct_per_min,
            rung_index: ladder.rung_index,
            at_level_s: ladder.rung_elapsed_s,
            input_tracked: &self.tracked,
            input_phase_deg: &phase_deg,
            input_rms: broadband,
            limit_bound: &limit_bound,
            law_calls: self.law_calls,
            law_misses: self.law_misses,
            law_capped_ticks: self.law_capped_ticks,
            law_values: self.law_values,
        });

        // The display record, at the frequency the amplitudes were measured at.
        // `try_lock`: a reader copying the record out never holds the tick up.
        if let Ok(mut log) = self.display.log.try_lock() {
            if self.display_jump {
                log.mark_jump();
                self.display_jump = false;
            }
            let rung_scale = 10f64.powf(self.ladder.level_db() / 20.0);
            let nan2 = (f64::NAN, f64::NAN);
            let scaled = |b: Option<(f64, f64)>| {
                b.map(|(lo, hi)| (lo * rung_scale, hi * rung_scale))
                    .unwrap_or(nan2)
            };
            log.write(&DisplayRow {
                t_s,
                freq_hz: f_meas,
                level_db: self.ladder.level_db(),
                measured,
                target: self.profile.target_accel_g_pk(f_meas).unwrap_or(f64::NAN) * rung_scale,
                alarm: scaled(self.profile.alarm_bounds(f_meas)),
                abort: scaled(self.profile.abort_bounds(f_meas)),
                drive_vpk,
                cola_vpk: cola_vpk.unwrap_or(f64::NAN),
                thd,
                tracked: &self.tracked,
                phase_deg: &phase_deg,
                rms: broadband,
                limit_bound: &limit_bound,
            });
        }

        self.input_phase_buf = phase_deg;
        self.limit_bound_buf = limit_bound;
        self.tracked_buf = per_channel_tracked;
        self.bb_pub_buf = per_channel_broadband;
        self.overload_pub_buf = overload;
    }

    /// Write ONE drive frame: ch0 = control drive, ch1 = COLA.
    ///
    /// The COLA amplitude is scaled by `cola_ramp_scale` so on abort/stop it
    /// decays with ch0 (no cliff on the aux Drive-role output).
    ///
    /// Factored out so [`SineProcessStep::synthesize_leadin`] emits BYTE-IDENTICAL
    /// frames to `process`. A second expression of the drive formula for the
    /// lead-in would be a place for the pre-start frames and the first real block
    /// to disagree, which is exactly the discontinuity the lead-in exists to
    /// remove.
    #[inline]
    fn write_drive_frame(&self, sin: f64, out: &mut Vec<f32>) {
        out.push((sin * self.current_drive_scale) as f32);
        if self.out_channels >= 2 {
            let cola = if self.cola.enabled {
                let a = self.cola.amp * self.cola_ramp_scale;
                match self.cola.waveform {
                    ColaWaveform::Sine => a * sin,
                    ColaWaveform::Square => a * if sin >= 0.0 { 1.0 } else { -1.0 },
                    ColaWaveform::DcRef => a,
                }
            } else {
                0.0
            };
            out.push(cola as f32);
            // Pad any further drive channels with zeros (defensive).
            for _ in 2..self.out_channels {
                out.push(0.0);
            }
        }
    }

    /// Generate `n_frames` of the drive this step is about to produce, WITHOUT
    /// running the control loop - the engage cold-start lead-in.
    ///
    /// # What it does and, more importantly, what it must not do
    ///
    /// It advances the NCO and writes drive frames. It does NOT demodulate, does
    /// NOT accumulate broadband, does NOT touch the tick counter, and does NOT run
    /// a control tick.
    ///
    /// Calling `process` on a buffer of zeros was tried and is WRONG for this
    /// step, in a way worth recording because Explorer's arm path does exactly
    /// that legitimately: Explorer has no servo and Sine does. Feeding zeros
    /// through `process` runs real control ticks against a measured amplitude of
    /// zero - the servo winds the drive, the scheduler advances, and the
    /// level-settle window is spent before the run has acquired a single frame.
    /// MEASURED 2026-08-08: six sim_e2e fixtures aborted on a low-side
    /// ScalarAbort at engage, because the protections armed on a settle window the
    /// lead-in had already consumed.
    ///
    /// Advancing the NCO IS correct: these frames really will be played, so the
    /// phase must continue from where they left it. The drive scale is the
    /// profile-derived seed the step was built with, which is what the first real
    /// block would have used anyway - the servo has not closed yet, so there is
    /// nothing else it could be.
    pub fn synthesize_leadin(&mut self, n_frames: usize, out: &mut Vec<f32>) {
        out.clear();
        out.reserve(n_frames * self.out_channels);
        for _ in 0..n_frames {
            let (sin, _inst_freq, _phase) = self.nco.next();
            self.write_drive_frame(sin, out);
        }
    }

    fn reset_tick_accumulators(&mut self) {
        self.tick_frames = 0;
        for ss in self.broadband_ss.iter_mut() {
            *ss = 0.0;
        }
    }
}

/// An angle in degrees, wrapped to `(-180, 180]`.
fn wrap_deg(d: f64) -> f64 {
    let w = d.rem_euclid(360.0);
    if w > 180.0 {
        w - 360.0
    } else {
        w
    }
}

/// What a panic carried, as text.
fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "the law panicked".to_string()
    }
}

/// Call the law once (`engage` or `tick`) into a cleared values buffer.
/// Under the guards (`timing`: the clock and the budget) the call is timed,
/// a panic is caught, and the output checked; without them it is a plain
/// call. Returns the outcome and the call's time in seconds (0 untimed).
fn call_law(
    law: &mut dyn SineControlLaw,
    engage: bool,
    inputs: &TickInputs<'_>,
    values: &mut LawValues,
    timing: Option<(&dyn LawClock, f64)>,
) -> (Result<TickOutputs, LawFault>, f64) {
    values.clear();
    let Some((clock, budget_s)) = timing else {
        let r = if engage {
            law.engage(inputs, values)
        } else {
            law.tick(inputs, values)
        };
        return (r, 0.0);
    };
    let t0 = clock.now_s();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if engage {
            law.engage(inputs, values)
        } else {
            law.tick(inputs, values)
        }
    }));
    let took_s = (clock.now_s() - t0).max(0.0);
    let r = match caught {
        Err(p) => Err(LawFault::Crashed(panic_text(&*p))),
        Ok(Err(fault)) => Err(fault),
        Ok(Ok(_)) if took_s > budget_s => Err(LawFault::OverBudget { took_s, budget_s }),
        Ok(Ok(out)) => match output_rule(&out, values) {
            Some(rule) => Err(LawFault::BadOutput(rule)),
            None => Ok(out),
        },
    };
    (r, took_s)
}

/// The output rule a call broke, if any: the drive finite and not below
/// zero, every value finite. (An undeclared value is refused when the law
/// sets it.)
fn output_rule(out: &TickOutputs, values: &LawValues) -> Option<SineOutputRule> {
    if !out.drive.is_finite() {
        return Some(SineOutputRule::NonFinite);
    }
    if out.drive < 0.0 {
        return Some(SineOutputRule::Negative);
    }
    if values.iter().any(|(_, v)| !v.is_finite()) {
        return Some(SineOutputRule::NonFinite);
    }
    None
}

/// Cap a law's drive against the drive in force: at most `max_step_db`
/// either way and at most `max_rise_db_per_s * dt` up. Returns the capped
/// drive and whether a cap bound. A change within a billionth of a dB of a
/// cap does not count as binding (the shipped law's own 1 dB clamp lands on
/// the default cap exactly).
fn cap_drive(in_force: f64, proposed: f64, g: &LawGuards, dt: f64) -> (f64, bool) {
    const EPS_DB: f64 = 1e-9;
    if in_force.is_nan() || in_force <= 0.0 {
        return (proposed, false);
    }
    let up = g.max_step_db.min(g.max_rise_db_per_s * dt);
    let down = g.max_step_db;
    let step_db = if proposed > 0.0 {
        20.0 * (proposed / in_force).log10()
    } else {
        f64::NEG_INFINITY
    };
    if step_db > up + EPS_DB {
        (in_force * 10f64.powf(up / 20.0), true)
    } else if step_db < -(down + EPS_DB) {
        (in_force * 10f64.powf(-down / 20.0), true)
    } else {
        (proposed, false)
    }
}

/// Linear interpolation of `values` at `f` on a strictly-increasing `freqs`
/// grid; clamps to the endpoints out of range. Used for the notch sysID |H|
/// fallback lookup.
fn interp_at(freqs: &[f64], values: &[f64], f: f64) -> f64 {
    if freqs.is_empty() || values.is_empty() {
        return 0.0;
    }
    if f <= freqs[0] {
        return values[0];
    }
    let n = freqs.len();
    if f >= freqs[n - 1] {
        return values[n - 1];
    }
    let i = freqs.partition_point(|&g| g <= f);
    let (lo, hi) = (i - 1, i);
    let (fl, fh) = (freqs[lo], freqs[hi]);
    if fh <= fl {
        return values[lo];
    }
    let t = (f - fl) / (fh - fl);
    values[lo] + t * (values[hi] - values[lo])
}

// The last measured phase per input channel (for RSTD). Kept as a field via a
// small companion so the borrow checker stays happy inside the per-frame loop.
impl SineProcessStep {
    /// The last measured phase per input channel (populated each frame).
    fn ensure_phase_len(&mut self) {
        if self.last_phase.len() != self.filters.len() {
            self.last_phase = vec![0.0; self.filters.len()];
        }
    }
}

impl ProcessStep for SineProcessStep {
    fn process(&mut self, input: &[f32], n_frames: usize, drive_out: &mut Vec<f32>) {
        self.ensure_phase_len();
        let in_ch = input.len().checked_div(n_frames).unwrap_or(0);
        drive_out.clear();
        drive_out.reserve(n_frames * self.out_channels);

        // THE CHEAPEST POSSIBLE THING WITH THE ACQUIRED BLOCK: one memcpy into
        // the ring, for the display / time-stream tap the service drains between
        // batches. Everything expensive that wants these samples runs there, off
        // this tick. A full ring drops its OLDEST and counts it; nothing here
        // waits on a consumer (see `crate::service::ring`).
        if in_ch > 0 {
            self.ring.push(&input[..n_frames * in_ch]);
        }

        for fi in 0..n_frames {
            // 0. This frame's RUN INDEX, 1-based, in the same numbering the
            //    engine's raw-ring watchdog stamps onto `AbortTrip.at_frame`.
            //    Bumped BEFORE the control tick below so a snapshot pushed by
            //    that tick is tagged with the first frame it will apply to.
            self.frames_processed += 1;

            // 1. Generate the drive sample from the phase-continuous NCO.
            let (sin, inst_freq, phase) = self.nco.next();

            // 2. If ramping down, decay the drive scale each frame so it never
            //    cliffs. Decay the COLA ramp factor in LOCKSTEP so
            //    the aux ch1 output also decays to zero rather than cliffing at
            //    full amplitude on a Drive-role DAC. Otherwise use the committed
            //    control scale and a unit COLA ramp.
            if self.ramping_down {
                self.current_drive_scale *= self.ramp_per_frame;
                if self.current_drive_scale < RAMP_FLOOR_SCALE {
                    self.current_drive_scale = 0.0;
                }
                self.cola_ramp_scale *= self.ramp_per_frame;
                if self.cola_ramp_scale < RAMP_FLOOR_SCALE {
                    self.cola_ramp_scale = 0.0;
                }
            }
            // 3. Write the drive frame (ch0 = control drive, ch1 = COLA).
            self.write_drive_frame(sin, drive_out);

            // 4. Demodulate every input channel through its tracking filter, at
            //    the DELAY-COMPENSATED reference frequency. This is
            //    the frequency of the signal arriving on this frame, not the one
            //    the drive is leaving on it; see `reference_freq`. Identical to
            //    `inst_freq` whenever the lag is zero or the sweep is stationary.
            let ref_freq = self.reference_freq(inst_freq);
            // The filters demodulate this sample against the reference phase
            // BEFORE they advance it; mirror that, and keep its offset from the
            // drive's own phase for the phase-against-drive readout. Read only by
            // the status and the display, never by the control law.
            self.phase_offset = self.ref_phase_mirror - phase;
            self.ref_phase_mirror += std::f64::consts::TAU * ref_freq / self.fs_hz;
            if self.ref_phase_mirror > std::f64::consts::TAU {
                self.ref_phase_mirror -= std::f64::consts::TAU;
            }
            for (ch, filt) in self.filters.iter_mut().enumerate() {
                let sample = input.get(fi * in_ch + ch).copied().unwrap_or(0.0) as f64;
                let (amp, ph) = filt.update(sample, ref_freq);
                self.tracked[ch] = amp;
                self.last_phase[ch] = ph;
                self.broadband_ss[ch] += sample * sample;
            }
            // 5. Feed the control channel's sample into the THD bank.
            if let Some(&cs) = self.control_slots.first() {
                let sample = input.get(fi * in_ch + cs).copied().unwrap_or(0.0) as f64;
                self.thd_bank.update(sample, inst_freq, phase);
            }

            // 6. Control tick every control_tick_frames frames.
            self.tick_frames += 1;
            if self.tick_frames >= self.control_tick_frames {
                self.control_tick();
            }
        }
    }

    fn on_discontinuity(&mut self, _event: TimebaseEvent) {
        // STILL A NO-OP, but for a different and now-true reason.
        //
        // The old note said the chunks are seamless "on Sim" because the plant and
        // counter persist across the per-chunk re-arm. That reason expired with
        // the batch conversion: there is no per-chunk re-arm any more, the backend
        // runs continuously for the whole run, and the engine is persistent - so
        // nothing this step owns is discontinuous at a BATCH boundary and there is
        // nothing here to reset.
        //
        // What this callback actually reports is a DEVICE epoch reset (a
        // `TimebaseEvent` from the backend's own sample counter), which on this
        // path means the acquisition restarted underneath a run that believes it
        // is continuous. The honest response to that is not a filter reset - it is
        // to end the run - and the engine already does end it: an epoch reset
        // arrives as a `Disconnected` / `SchedulingFault` stop reason that
        // `run_one_batch` turns into a terminal `finish`. Resetting the filters
        // here would soften a fault into a glitch.
    }
}
