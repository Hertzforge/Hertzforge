//! Sine schedulers: bidirectional sweep, dwell, step-sine, RSTD
//! orchestration, and the level ladder - all PURE state machines.
//!
//! Every type here consumes per-tick inputs (`dt`, and for RSTD the tracked
//! amplitude + phase) and emits frequency commands and typed events. No I/O,
//! no engine coupling: the service owns the NCO, the tracking filters, and
//! the servo; these state machines own WHEN the frequency moves and when a
//! phase of the test is complete. That split is what makes every edge
//! rule below unit-testable with synthetic inputs.
//!
//! # The edge rules (pinned - each is a rule AND a unit test)
//!
//! - Partial sweeps count only at band-edge completion; the flip itself
//!   ([`BidirSweep::reverse`]) never counts.
//! - Sweep counting is per one-direction pass: up + down = 2 sweeps (VR
//!   convention). An odd sweep-count target ending at the far edge is valid.
//! - [`SweepTarget::WhicheverFirst`] stops at the FIRST met condition,
//!   either way round.
//! - Hold pauses the TEST clock (sweep elapsed time, dwell seconds and
//!   cycles, rung clocks). Output continues at the held frequency - that is
//!   the service's business; progress does not accrue.
//! - Jog is allowed while held and clamps to the band; jogging TO a band
//!   edge does not count a sweep (only sweeping into an edge does).
//! - Jumping ladder rungs does not restart the sweep: [`LadderPlan`] never
//!   touches the sweep engine, and [`LadderPlan::jump_to`] only moves the
//!   level and resets the RUNG clock.

use crate::control::ControlError;
use hertzforge_dsp::resonance::{half_power, PhaseTracker, PhaseTrackerConfig, ResonanceEstimate};
use hertzforge_dsp::sweep::{octaves_between, sweep_seconds, SweepRate};
use serde::{Deserialize, Serialize};

/// Serde mirror for the foreign [`SweepRate`] (hertzforge-dsp does not derive
/// serde; the remote-derive pattern keeps our config types serializable
/// without an hertzforge-dsp edit).
#[derive(Serialize, Deserialize)]
#[serde(remote = "SweepRate")]
enum SweepRateDef {
    LogOctPerMin(f64),
    LinearHzPerSec(f64),
}

fn validate_band(band: (f64, f64)) -> Result<(), ControlError> {
    if band.0 <= 0.0 || band.0.is_nan() {
        return Err(ControlError::BadConfig(format!(
            "band low edge must be > 0 (got {})",
            band.0
        )));
    }
    if band.1 <= band.0 || band.1.is_nan() {
        return Err(ControlError::BadConfig(format!(
            "band must satisfy lo < hi (got ({}, {}))",
            band.0, band.1
        )));
    }
    Ok(())
}

/// Clamp a frequency INTENT to the run band. THE one expression of that clamp.
///
/// Two callers need it and they must agree to the bit: the scheduler applies it
/// (so the NCO and `freq_hz()` are the same number), and the service returns it
/// as the ECHO before the command has reached a control tick. A second
/// expression of the same rule is how a panel comes to render a frequency the
/// device is not playing, so there is only one.
///
/// The band handed in is the run's ADOPTED band - the service narrows it against
/// the device's real Nyquist at arm, so this function never needs to know a rate.
pub fn clamp_to_band(hz: f64, band: (f64, f64)) -> f64 {
    if hz.is_nan() {
        return band.0;
    }
    hz.clamp(band.0, band.1)
}

/// The sweep-rate MAGNITUDE (direction is the scheduler's job, not the
/// rate's). Rejects zero/NaN rates.
fn normalize_rate(rate: SweepRate) -> Result<SweepRate, ControlError> {
    let ok = |r: f64| -> Result<f64, ControlError> {
        if r == 0.0 || r.is_nan() {
            Err(ControlError::BadConfig(format!(
                "sweep rate must be nonzero (got {r})"
            )))
        } else {
            Ok(r.abs())
        }
    };
    Ok(match rate {
        SweepRate::LogOctPerMin(r) => SweepRate::LogOctPerMin(ok(r)?),
        SweepRate::LinearHzPerSec(r) => SweepRate::LinearHzPerSec(ok(r)?),
    })
}

fn validate_dwell_for(d: DwellFor) -> Result<(), ControlError> {
    match d {
        DwellFor::Seconds(s) => {
            if s <= 0.0 || s.is_nan() {
                return Err(ControlError::BadConfig(format!(
                    "dwell seconds must be > 0 (got {s})"
                )));
            }
        }
        DwellFor::Cycles(n) => {
            if n == 0 {
                return Err(ControlError::BadConfig("dwell cycles must be >= 1".into()));
            }
        }
    }
    Ok(())
}

// =============================================================================
// Bidirectional sweep
// =============================================================================

/// Sweep direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SweepDirection {
    Up,
    Down,
}

impl SweepDirection {
    fn flip(self) -> Self {
        match self {
            SweepDirection::Up => SweepDirection::Down,
            SweepDirection::Down => SweepDirection::Up,
        }
    }
}

/// When a bidirectional sweep run is complete (WhicheverFirst
/// stops at the FIRST met condition).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SweepTarget {
    /// Stop after N completed one-direction passes (up + down = 2).
    Sweeps(u32),
    /// Stop after this much accumulated TEST time (holds excluded).
    DurationS(f64),
    /// Stop at whichever of the two is met first.
    WhicheverFirst { sweeps: u32, duration_s: f64 },
}

/// Config for [`BidirSweep`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BidirSweepConfig {
    /// Sweep band `(lo, hi)`, Hz; `0 < lo < hi`.
    pub band: (f64, f64),
    /// Sweep-rate law. The MAGNITUDE is used; direction comes from
    /// `initial_direction` and the band-edge bounces.
    #[serde(with = "SweepRateDef")]
    pub rate: SweepRate,
    /// Starting frequency, within the band.
    pub start_hz: f64,
    /// Initial direction. If `start_hz` sits exactly on the band edge the
    /// direction points out of, it is flipped inward at construction (a
    /// zero-length pass is not a sweep).
    pub initial_direction: SweepDirection,
    /// Completion target.
    pub target: SweepTarget,
}

/// Run state readout for [`BidirSweep::progress`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SweepState {
    Running,
    Held,
    Done,
}

/// Progress readout: current f, sweeps done, elapsed test time, state.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SweepProgress {
    pub freq_hz: f64,
    pub direction: SweepDirection,
    /// Completed one-direction passes (up + down = 2).
    pub sweeps_done: u32,
    /// Accumulated TEST time, seconds - pauses during hold.
    pub elapsed_test_s: f64,
    pub state: SweepState,
}

/// Bidirectional sweep engine (named `BidirSweep`, NOT
/// "SweepScheduler" - the delegated Explorer service already exports a one-way nudge-paced
/// `SweepScheduler` and the two must not be confusable).
///
/// BOUNCES at band edges: arriving at an edge flips the direction and counts
/// ONE completed sweep (a one-direction pass). The sweep counter is
/// per-direction, so up + down = 2 sweeps (the VR convention; a UI may also
/// show "cycles" = round trips = sweeps/2).
///
/// # Pinned edge semantics (see the module docs)
///
/// [`BidirSweep::hold`] pauses the test clock; [`BidirSweep::reverse`] flips
/// mid-sweep WITHOUT counting the abandoned partial (the eventual band-edge
/// arrival still counts, wherever it happens); [`BidirSweep::jog`] is
/// allowed while held and clamps to the band without counting edges.
#[derive(Debug, Clone)]
pub struct BidirSweep {
    cfg: BidirSweepConfig,
    f_hz: f64,
    direction: SweepDirection,
    sweeps_done: u32,
    elapsed_test_s: f64,
    held: bool,
    done: bool,
}

impl BidirSweep {
    /// Build from config; validates band, rate, start, and target.
    pub fn new(cfg: BidirSweepConfig) -> Result<Self, ControlError> {
        validate_band(cfg.band)?;
        let rate = normalize_rate(cfg.rate)?;
        if cfg.start_hz.is_nan() || !(cfg.band.0..=cfg.band.1).contains(&cfg.start_hz) {
            return Err(ControlError::BadConfig(format!(
                "start_hz {} outside band ({}, {})",
                cfg.start_hz, cfg.band.0, cfg.band.1
            )));
        }
        match cfg.target {
            SweepTarget::Sweeps(n) => {
                if n == 0 {
                    return Err(ControlError::BadConfig("sweeps target must be >= 1".into()));
                }
            }
            SweepTarget::DurationS(d) => {
                if d <= 0.0 || d.is_nan() {
                    return Err(ControlError::BadConfig(format!(
                        "duration target must be > 0 (got {d})"
                    )));
                }
            }
            SweepTarget::WhicheverFirst { sweeps, duration_s } => {
                if sweeps == 0 {
                    return Err(ControlError::BadConfig("sweeps target must be >= 1".into()));
                }
                if duration_s <= 0.0 || duration_s.is_nan() {
                    return Err(ControlError::BadConfig(format!(
                        "duration target must be > 0 (got {duration_s})"
                    )));
                }
            }
        }
        // Normalize an on-edge start pointing out of the band (see config doc).
        let mut direction = cfg.initial_direction;
        if cfg.start_hz >= cfg.band.1 && direction == SweepDirection::Up {
            direction = SweepDirection::Down;
        }
        if cfg.start_hz <= cfg.band.0 && direction == SweepDirection::Down {
            direction = SweepDirection::Up;
        }
        Ok(BidirSweep {
            f_hz: cfg.start_hz,
            direction,
            sweeps_done: 0,
            elapsed_test_s: 0.0,
            held: false,
            done: false,
            cfg: BidirSweepConfig { rate, ..cfg },
        })
    }

    fn sweeps_target(&self) -> Option<u32> {
        match self.cfg.target {
            SweepTarget::Sweeps(n) => Some(n),
            SweepTarget::WhicheverFirst { sweeps, .. } => Some(sweeps),
            SweepTarget::DurationS(_) => None,
        }
    }

    fn duration_target(&self) -> Option<f64> {
        match self.cfg.target {
            SweepTarget::DurationS(d) => Some(d),
            SweepTarget::WhicheverFirst { duration_s, .. } => Some(duration_s),
            SweepTarget::Sweeps(_) => None,
        }
    }

    /// Advance the sweep by `dt` seconds of test time and return the new
    /// frequency command. While held or done (or for a non-positive `dt`)
    /// nothing moves and nothing accrues (hold pauses the test clock).
    ///
    /// Band-edge arrivals within the tick are handled exactly: the remaining
    /// tick time carries through the bounce (multiple bounces in one large
    /// `dt` are fine), each arrival counts one sweep, and a met target stops
    /// the sweep AT the stopping point (remaining tick time is discarded).
    pub fn tick(&mut self, dt: f64) -> f64 {
        if self.held || self.done || dt <= 0.0 || dt.is_nan() {
            return self.f_hz;
        }
        // Cap this tick's driving time at the remaining test duration.
        let mut t = dt;
        if let Some(dur) = self.duration_target() {
            t = t.min((dur - self.elapsed_test_s).max(0.0));
        }
        while t > 0.0 {
            let toward = match self.direction {
                SweepDirection::Up => self.cfg.band.1,
                SweepDirection::Down => self.cfg.band.0,
            };
            let t_edge = sweep_seconds(self.f_hz, toward, self.cfg.rate);
            if t_edge <= 0.0 {
                // Already AT the edge we are pointing at (e.g. after a jog to
                // the edge): flip inward without counting - a zero-length
                // pass is not a sweep (the jog rule).
                self.direction = self.direction.flip();
                continue;
            }
            if t_edge > t {
                let signed_t = match self.direction {
                    SweepDirection::Up => t,
                    SweepDirection::Down => -t,
                };
                self.f_hz = self
                    .cfg
                    .rate
                    .freq_at(self.f_hz, signed_t)
                    .clamp(self.cfg.band.0, self.cfg.band.1);
                self.elapsed_test_s += t;
                t = 0.0;
            } else {
                self.f_hz = toward;
                self.elapsed_test_s += t_edge;
                t -= t_edge;
                self.sweeps_done += 1;
                self.direction = self.direction.flip();
                if let Some(n) = self.sweeps_target() {
                    if self.sweeps_done >= n {
                        self.done = true;
                        break;
                    }
                }
            }
        }
        if let Some(dur) = self.duration_target() {
            if !self.done && self.elapsed_test_s >= dur {
                self.done = true;
            }
        }
        self.f_hz
    }

    /// Pause: the frequency command freezes and the TEST clock stops.
    /// The service keeps driving at the held frequency.
    pub fn hold(&mut self) {
        if !self.done {
            self.held = true;
        }
    }

    /// Resume from a hold.
    pub fn resume(&mut self) {
        self.held = false;
    }

    /// Flip the sweep direction mid-sweep. The abandoned partial pass does
    /// NOT count; only band-edge completions count, including the one
    /// this reversed pass eventually reaches.
    pub fn reverse(&mut self) {
        if !self.done {
            self.direction = self.direction.flip();
        }
    }

    /// Move the tone to an ABSOLUTE frequency, clamped to the band. Returns the
    /// value actually adopted, which is what the caller must echo (the panel
    /// renders the SERVICE's state, never its own last request).
    ///
    /// Distinct from [`BidirSweep::jog`] because a relative nudge cannot express
    /// "put the tone at 300 Hz": the caller would have to subtract a frequency it
    /// read at some earlier moment, and a sweep moves between the read and the
    /// post. Same rules as `jog` - allowed while held, never counts a sweep
    /// even when it lands on an edge.
    pub fn go_to(&mut self, hz: f64) -> f64 {
        if hz.is_nan() {
            return self.f_hz;
        }
        self.f_hz = clamp_to_band(hz, self.cfg.band);
        self.f_hz
    }

    /// Nudge the frequency by `delta_hz`, clamped to the band. Allowed while
    /// held (the jog rule); never counts a sweep, even when it lands on
    /// an edge.
    pub fn jog(&mut self, delta_hz: f64) {
        if self.done || delta_hz.is_nan() {
            return;
        }
        self.f_hz = clamp_to_band(self.f_hz + delta_hz, self.cfg.band);
    }

    /// Current frequency command, Hz.
    pub fn freq_hz(&self) -> f64 {
        self.f_hz
    }

    /// Completed one-direction passes (up + down = 2).
    pub fn sweeps_done(&self) -> u32 {
        self.sweeps_done
    }

    /// Accumulated test time, seconds (holds excluded).
    pub fn elapsed_test_s(&self) -> f64 {
        self.elapsed_test_s
    }

    /// Whether the completion target has been met.
    pub fn done(&self) -> bool {
        self.done
    }

    /// Whether the sweep is held.
    pub fn held(&self) -> bool {
        self.held
    }

    /// Full progress readout.
    pub fn progress(&self) -> SweepProgress {
        SweepProgress {
            freq_hz: self.f_hz,
            direction: self.direction,
            sweeps_done: self.sweeps_done,
            elapsed_test_s: self.elapsed_test_s,
            state: if self.done {
                SweepState::Done
            } else if self.held {
                SweepState::Held
            } else {
                SweepState::Running
            },
        }
    }
}

// =============================================================================
// Dwell
// =============================================================================

/// Dwell duration: wall test time or accumulated drive cycles (standards need
/// both).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DwellFor {
    Seconds(f64),
    Cycles(u64),
}

/// A dwell instruction: hold `freq_hz` for the given duration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DwellPlan {
    pub freq_hz: f64,
    /// Named `for_` because `for` is reserved.
    pub for_: DwellFor,
}

/// Runtime for a [`DwellPlan`]: cycles accrue as `freq * dt` (exact for the
/// phase-continuous NCO, whose instantaneous frequency IS the commanded one).
///
/// # Hold semantics (documented reconciliation)
///
/// A dwell "hold" means the OUTPUT CONTINUES at `freq_hz` (the service keeps
/// driving) but test PROGRESS PAUSES: neither seconds nor cycles accrue while
/// held - hold pauses the TEST clock, matching the sweep semantics. (The
/// physical shaker still accumulates real cycles during a hold; those are
/// deliberately not test progress, exactly as held sweep output is not test
/// time.)
#[derive(Debug, Clone)]
pub struct DwellRun {
    plan: DwellPlan,
    elapsed_s: f64,
    cycles: f64,
    held: bool,
    done: bool,
}

impl DwellRun {
    /// Build from a plan; validates frequency and duration.
    pub fn new(plan: DwellPlan) -> Result<Self, ControlError> {
        if plan.freq_hz <= 0.0 || plan.freq_hz.is_nan() {
            return Err(ControlError::BadConfig(format!(
                "dwell freq must be > 0 (got {})",
                plan.freq_hz
            )));
        }
        validate_dwell_for(plan.for_)?;
        Ok(DwellRun {
            plan,
            elapsed_s: 0.0,
            cycles: 0.0,
            held: false,
            done: false,
        })
    }

    /// Advance test progress by `dt` seconds; returns `true` when the dwell
    /// target is met (latched). Progress does not accrue while held/done.
    pub fn tick(&mut self, dt: f64) -> bool {
        if self.held || self.done || dt <= 0.0 || dt.is_nan() {
            return self.done;
        }
        self.elapsed_s += dt;
        self.cycles += self.plan.freq_hz * dt;
        self.done = match self.plan.for_ {
            DwellFor::Seconds(s) => self.elapsed_s >= s,
            DwellFor::Cycles(n) => self.cycles >= n as f64,
        };
        self.done
    }

    /// Pause test progress (output continues - see type docs).
    pub fn hold(&mut self) {
        if !self.done {
            self.held = true;
        }
    }

    /// Resume test progress.
    pub fn resume(&mut self) {
        self.held = false;
    }

    /// The dwell frequency command, Hz.
    pub fn freq_hz(&self) -> f64 {
        self.plan.freq_hz
    }

    /// Accumulated test seconds (holds excluded).
    pub fn elapsed_s(&self) -> f64 {
        self.elapsed_s
    }

    /// Accumulated cycles (`freq * dt`, holds excluded). Fractional: the
    /// caller display may floor it.
    pub fn cycles(&self) -> f64 {
        self.cycles
    }

    pub fn held(&self) -> bool {
        self.held
    }

    pub fn done(&self) -> bool {
        self.done
    }
}

// =============================================================================
// Step sine (research extra adopted into v1)
// =============================================================================

/// Frequency spacing of the step ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepSpacing {
    /// Geometric steps `f_k = lo * 2^(k / steps_per_octave)` (the classic
    /// step-sine grid).
    Log,
    /// Evenly spaced steps; the step COUNT matches what the log rule would
    /// give (`ceil(octaves(band) * steps_per_octave)` intervals), so the same
    /// `steps_per_octave` knob sizes both.
    Linear,
}

/// Config for [`StepSine`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StepSineConfig {
    /// Test band `(lo, hi)`, Hz.
    pub band: (f64, f64),
    /// Steps per octave (10..=20 typical).
    pub steps_per_octave: f64,
    /// Ladder spacing rule.
    pub spacing: StepSpacing,
    /// Per-step dwell. `None` = the research default, computed PER STEP as
    /// `max(60 s, 20 cycles)` at that step's frequency (20 cycles dominates
    /// only below 1/3 Hz).
    pub dwell: Option<DwellFor>,
}

/// Progress readout for [`StepSine`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StepSineProgress {
    pub step_index: usize,
    pub n_steps: usize,
    pub freq_hz: f64,
    pub step_elapsed_s: f64,
    pub step_cycles: f64,
    pub state: SweepState,
}

/// Step-sine scheduler: emits the step-frequency ladder and dwells at each step
/// for the configured (or per-step default) duration. Hold semantics follow the
/// edge rules: [`StepSine::hold`] pauses the step clock (seconds AND cycles);
/// output continues at the current step.
#[derive(Debug, Clone)]
pub struct StepSine {
    cfg: StepSineConfig,
    ladder: Vec<f64>,
    idx: usize,
    step_elapsed_s: f64,
    step_cycles: f64,
    held: bool,
    done: bool,
}

impl StepSine {
    /// Build the ladder from config; validates band / steps / dwell.
    pub fn new(cfg: StepSineConfig) -> Result<Self, ControlError> {
        validate_band(cfg.band)?;
        if cfg.steps_per_octave <= 0.0 || cfg.steps_per_octave.is_nan() {
            return Err(ControlError::BadConfig(format!(
                "steps_per_octave must be > 0 (got {})",
                cfg.steps_per_octave
            )));
        }
        if let Some(d) = cfg.dwell {
            validate_dwell_for(d)?;
        }
        let (lo, hi) = cfg.band;
        let octs = octaves_between(lo, hi);
        let n_intervals = (octs * cfg.steps_per_octave).ceil().max(1.0);
        if n_intervals > 1_000_000.0 {
            return Err(ControlError::BadConfig(format!(
                "step ladder would have {n_intervals} steps; check steps_per_octave"
            )));
        }
        let mut ladder = Vec::new();
        match cfg.spacing {
            StepSpacing::Log => {
                let mut k = 0u64;
                loop {
                    let f = lo * (k as f64 / cfg.steps_per_octave).exp2();
                    if f >= hi * (1.0 - 1e-12) {
                        ladder.push(hi); // land exactly on the top edge
                        break;
                    }
                    ladder.push(f);
                    k += 1;
                }
            }
            StepSpacing::Linear => {
                let n = n_intervals as usize;
                for i in 0..n {
                    ladder.push(lo + (hi - lo) * (i as f64) / (n as f64));
                }
                ladder.push(hi);
            }
        }
        Ok(StepSine {
            cfg,
            ladder,
            idx: 0,
            step_elapsed_s: 0.0,
            step_cycles: 0.0,
            held: false,
            done: false,
        })
    }

    /// The full step-frequency ladder.
    pub fn ladder(&self) -> &[f64] {
        &self.ladder
    }

    /// The dwell rule at step `i`: the configured dwell, or the per-step
    /// default `max(60 s, 20 cycles)` expressed in seconds at that step's
    /// frequency. `None` for an out-of-range index.
    pub fn step_dwell(&self, i: usize) -> Option<DwellFor> {
        let f = *self.ladder.get(i)?;
        Some(match self.cfg.dwell {
            Some(d) => d,
            None => DwellFor::Seconds(60f64.max(20.0 / f)),
        })
    }

    /// Advance test progress by `dt`; returns the current step frequency.
    /// When the current step's dwell completes, the scheduler moves to the
    /// next step (the returned frequency is the NEW step's). After the last
    /// step completes the run is done and the frequency stays at the top.
    pub fn tick(&mut self, dt: f64) -> f64 {
        if self.held || self.done || dt <= 0.0 || dt.is_nan() {
            return self.freq_hz();
        }
        let f = self.ladder[self.idx];
        self.step_elapsed_s += dt;
        self.step_cycles += f * dt;
        let met = match self.step_dwell(self.idx) {
            Some(DwellFor::Seconds(s)) => self.step_elapsed_s >= s,
            Some(DwellFor::Cycles(n)) => self.step_cycles >= n as f64,
            None => false,
        };
        if met {
            if self.idx + 1 >= self.ladder.len() {
                self.done = true;
            } else {
                self.idx += 1;
                self.step_elapsed_s = 0.0;
                self.step_cycles = 0.0;
            }
        }
        self.freq_hz()
    }

    /// Pause the step clock (output continues at the current step).
    pub fn hold(&mut self) {
        if !self.done {
            self.held = true;
        }
    }

    /// Resume the step clock.
    pub fn resume(&mut self) {
        self.held = false;
    }

    /// Current step frequency, Hz.
    pub fn freq_hz(&self) -> f64 {
        self.ladder[self.idx]
    }

    pub fn done(&self) -> bool {
        self.done
    }

    /// Full progress readout.
    pub fn progress(&self) -> StepSineProgress {
        StepSineProgress {
            step_index: self.idx,
            n_steps: self.ladder.len(),
            freq_hz: self.freq_hz(),
            step_elapsed_s: self.step_elapsed_s,
            step_cycles: self.step_cycles,
            state: if self.done {
                SweepState::Done
            } else if self.held {
                SweepState::Held
            } else {
                SweepState::Running
            },
        }
    }
}

// =============================================================================
// RSTD orchestration (hardened - BINDING)
// =============================================================================

/// Dwell behavior after the RSTD search + capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RstdDwellMode {
    /// Hold the captured peak frequency (no tracking).
    Fixed,
    /// Phase-locked tracking of the resonance via
    /// [`hertzforge_dsp::resonance::PhaseTracker`] on the captured setpoint.
    PhaseTracked,
    // NOTE (v2): a third variant `PeakDither` - the VR-style phase-dither
    // hill-climb peak tracker - is deliberately deferred; add it here when
    // hertzforge-dsp's PeakTracker stretch lands.
}

/// Config for [`RstdPlan`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RstdConfig {
    /// Search band `(lo, hi)`, Hz. Also the dwell tracker's clamp band.
    pub band: (f64, f64),
    /// Search drive level in dB relative to the full profile target
    /// (informational for the SERVICE, which owns levels; the scheduler emits
    /// frequency only). Typical -20.
    pub level_db: f64,
    /// Search sweep rate (magnitude; the search always sweeps UP from the
    /// band's low edge).
    #[serde(with = "SweepRateDef")]
    pub rate: SweepRate,
    /// Tracking-filter bandwidth at the settle point, Hz. Sets the
    /// settle-before-capture hold: `settle_k / tracking_bw_hz` seconds.
    pub tracking_bw_hz: f64,
    /// Settle multiplier `k` (default 5): settle time = `k / bw`.
    pub settle_k: f64,
    /// Dwell behavior after capture.
    pub dwell_mode: RstdDwellMode,
    /// Dwell duration (test seconds or cycles; level scheduling applies at
    /// the service layer).
    pub dwell_for: DwellFor,
}

/// RSTD phase readout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RstdState {
    /// Sweeping the band, accumulating `(f, amp, phase)` samples.
    Search,
    /// Holding at the detected peak for `settle_k / tracking_bw_hz` seconds
    /// so the tracking filter's phase readout settles.
    Settle,
    /// Capturing the phase setpoint from the SETTLED phase (one tick).
    Capture,
    /// Dwelling (fixed or phase-tracked).
    Dwell,
    /// Dwell target met.
    Complete,
    /// The search could not produce a well-posed resonance estimate.
    Failed,
}

/// Typed events emitted by [`RstdPlan::tick`]. Never silent: every state
/// transition that matters to the operator has an event.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RstdEvent {
    /// Search finished and the half-power fit succeeded.
    SearchDone { f_peak: f64, q: f64, amp: f64 },
    /// Search finished but the half-power fit was not well-posed (peak on the
    /// band boundary, missing half-power crossing, degenerate data). The plan
    /// latches [`RstdState::Failed`] and holds; re-run the search on a better
    /// band/level. (Additive to the audit event set - the failure path must
    /// not be silent either.)
    SearchFailed,
    /// The settled phase setpoint was captured (radians, tracking-filter
    /// convention).
    SetpointCaptured { phase_rad: f64 },
    /// The phase tracker pinned at a band edge long enough to latch band
    /// exit (alarm + hold; NEVER silent). The plan holds at the
    /// edge; progress pauses; the latch persists until a new plan/search.
    BandExit,
    /// The dwell duration target was met.
    DwellComplete,
}

/// Resonance search-track-dwell orchestration (BINDING):
/// `Search -> Settle -> Capture -> Dwell`.
///
/// # Phase and slope conventions (verified against hertzforge-dsp)
///
/// Phase here is the tracking-filter convention
/// ([`hertzforge_dsp::sine::TrackingFilter::update`]): response phase relative to
/// the drive, in `(-pi, pi]`, passing `~ -pi/2` at resonance with a NEGATIVE
/// phase-vs-frequency slope. The dwell tracker's slope is derived
/// ANALYTICALLY from the search's Q (never from lag-biased sweep
/// differencing):
///
/// ```text
///   d(phi)/df |_(f_n)  =  -2 * Q / f_n   [rad/Hz]
/// ```
///
/// This is exactly the convention `hertzforge_dsp::resonance` documents and its
/// `PhaseTracker` expects (its damped-Newton step handles the sign
/// internally; passing the NEGATIVE analytic slope here is load-bearing -
/// a positive slope would drive the tracker away from the resonance).
///
/// # Caller pattern (pure state machine)
///
/// Each control tick, feed the CURRENT tracked amplitude and phase measured
/// at [`RstdPlan::freq_hz`]:
///
/// ```ignore
/// let (f_next, event) = plan.tick(dt, tracked_amp, phase_rad);
/// ```
///
/// During Search the samples are accumulated against the commanded frequency;
/// on band completion [`hertzforge_dsp::resonance::half_power`] produces
/// `(f_peak, Q)`.
#[derive(Debug, Clone)]
pub struct RstdPlan {
    cfg: RstdConfig,
    state: RstdState,
    f_cmd: f64,
    /// Search sample log: `(freq, amp, phase)` per tick.
    samples: Vec<(f64, f64, f64)>,
    search_elapsed_s: f64,
    settled_s: f64,
    est: Option<ResonanceEstimate>,
    setpoint_rad: Option<f64>,
    tracker: Option<PhaseTracker>,
    dwell_elapsed_s: f64,
    dwell_cycles: f64,
    held: bool,
    band_exit_latched: bool,
}

impl RstdPlan {
    /// Build from config; validates band, rate, bandwidth, settle, and dwell.
    pub fn new(cfg: RstdConfig) -> Result<Self, ControlError> {
        validate_band(cfg.band)?;
        let rate = normalize_rate(cfg.rate)?;
        if cfg.tracking_bw_hz <= 0.0 || cfg.tracking_bw_hz.is_nan() {
            return Err(ControlError::BadConfig(format!(
                "tracking_bw_hz must be > 0 (got {})",
                cfg.tracking_bw_hz
            )));
        }
        if cfg.settle_k <= 0.0 || cfg.settle_k.is_nan() {
            return Err(ControlError::BadConfig(format!(
                "settle_k must be > 0 (got {})",
                cfg.settle_k
            )));
        }
        validate_dwell_for(cfg.dwell_for)?;
        Ok(RstdPlan {
            f_cmd: cfg.band.0,
            cfg: RstdConfig { rate, ..cfg },
            state: RstdState::Search,
            samples: Vec::new(),
            search_elapsed_s: 0.0,
            settled_s: 0.0,
            est: None,
            setpoint_rad: None,
            tracker: None,
            dwell_elapsed_s: 0.0,
            dwell_cycles: 0.0,
            held: false,
            band_exit_latched: false,
        })
    }

    /// The settle hold duration, seconds: `settle_k / tracking_bw_hz`.
    pub fn settle_s(&self) -> f64 {
        self.cfg.settle_k / self.cfg.tracking_bw_hz
    }

    /// One control tick: feed the tracked amplitude and phase measured at the
    /// current [`RstdPlan::freq_hz`]; returns the next frequency command and
    /// at most one typed event. While held (operator hold or the band-exit
    /// alarm hold) nothing advances.
    pub fn tick(&mut self, dt: f64, tracked_amp: f64, phase_rad: f64) -> (f64, Option<RstdEvent>) {
        if self.held || dt <= 0.0 || dt.is_nan() {
            return (self.f_cmd, None);
        }
        match self.state {
            RstdState::Search => {
                self.samples.push((self.f_cmd, tracked_amp, phase_rad));
                if self.f_cmd >= self.cfg.band.1 {
                    let freqs: Vec<f64> = self.samples.iter().map(|s| s.0).collect();
                    let amps: Vec<f64> = self.samples.iter().map(|s| s.1).collect();
                    return match half_power(&freqs, &amps) {
                        Some(e) => {
                            self.est = Some(e);
                            self.f_cmd = e.f_peak;
                            self.state = RstdState::Settle;
                            (
                                self.f_cmd,
                                Some(RstdEvent::SearchDone {
                                    f_peak: e.f_peak,
                                    q: e.q,
                                    amp: e.amp_peak,
                                }),
                            )
                        }
                        None => {
                            self.state = RstdState::Failed;
                            (self.f_cmd, Some(RstdEvent::SearchFailed))
                        }
                    };
                }
                self.search_elapsed_s += dt;
                self.f_cmd = self
                    .cfg
                    .rate
                    .freq_at(self.cfg.band.0, self.search_elapsed_s)
                    .min(self.cfg.band.1);
                (self.f_cmd, None)
            }
            RstdState::Settle => {
                self.settled_s += dt;
                if self.settled_s >= self.settle_s() {
                    self.state = RstdState::Capture;
                }
                (self.f_cmd, None)
            }
            RstdState::Capture => {
                // The settle hold just completed, so THIS tick's phase input
                // is the settled phase (the setpoint is captured,
                // never a hardcoded -90 degrees).
                self.setpoint_rad = Some(phase_rad);
                if self.cfg.dwell_mode == RstdDwellMode::PhaseTracked {
                    // est is always Some here (Settle is only reachable from a
                    // successful fit); guard anyway - a missing estimate falls
                    // back to a fixed dwell rather than panicking.
                    if let Some(e) = self.est {
                        let slope = -2.0 * e.q / e.f_peak; // rad/Hz, NEGATIVE (see type docs)
                        let tcfg = PhaseTrackerConfig::new(phase_rad, slope, self.cfg.band);
                        self.tracker = Some(PhaseTracker::new(tcfg, self.f_cmd));
                    }
                }
                self.state = RstdState::Dwell;
                (self.f_cmd, Some(RstdEvent::SetpointCaptured { phase_rad }))
            }
            RstdState::Dwell => {
                if let Some(tracker) = self.tracker.as_mut() {
                    self.f_cmd = tracker.update(phase_rad);
                    if tracker.band_exit() && !self.band_exit_latched {
                        // Alarm + hold, never silent. Progress
                        // pauses; the frequency pins at the band edge.
                        self.band_exit_latched = true;
                        self.held = true;
                        return (self.f_cmd, Some(RstdEvent::BandExit));
                    }
                }
                self.dwell_elapsed_s += dt;
                self.dwell_cycles += self.f_cmd * dt;
                let met = match self.cfg.dwell_for {
                    DwellFor::Seconds(s) => self.dwell_elapsed_s >= s,
                    DwellFor::Cycles(n) => self.dwell_cycles >= n as f64,
                };
                if met {
                    self.state = RstdState::Complete;
                    return (self.f_cmd, Some(RstdEvent::DwellComplete));
                }
                (self.f_cmd, None)
            }
            RstdState::Complete | RstdState::Failed => (self.f_cmd, None),
        }
    }

    /// Operator hold: pauses search/settle/dwell progress. Does not
    /// clear a band-exit alarm hold.
    pub fn hold(&mut self) {
        self.held = true;
    }

    /// Resume from an OPERATOR hold. A band-exit alarm hold cannot be
    /// resumed - the resonance left the band; re-search with a new plan.
    pub fn resume(&mut self) {
        if !self.band_exit_latched {
            self.held = false;
        }
    }

    /// Current frequency command, Hz.
    pub fn freq_hz(&self) -> f64 {
        self.f_cmd
    }

    /// Current phase of the orchestration.
    pub fn state(&self) -> RstdState {
        self.state
    }

    /// The half-power fit from the search, once available.
    pub fn estimate(&self) -> Option<ResonanceEstimate> {
        self.est
    }

    /// The captured phase setpoint (radians), once available.
    pub fn setpoint_rad(&self) -> Option<f64> {
        self.setpoint_rad
    }

    /// Latched band-exit state (persists until a new plan).
    pub fn band_exit_latched(&self) -> bool {
        self.band_exit_latched
    }

    /// Accumulated dwell test seconds (holds excluded).
    pub fn dwell_elapsed_s(&self) -> f64 {
        self.dwell_elapsed_s
    }

    /// Accumulated dwell cycles (`freq * dt`, holds excluded).
    pub fn dwell_cycles(&self) -> f64 {
        self.dwell_cycles
    }

    pub fn held(&self) -> bool {
        self.held
    }
}

// =============================================================================
// Level ladder (sine semantics)
// =============================================================================

/// How long a ladder rung runs before auto-advancing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RungFor {
    /// N completed sweeps (fed via [`LadderPlan::note_sweep_completed`]).
    Sweeps(u32),
    /// Accumulated rung test seconds (fed via [`LadderPlan::tick`]).
    Seconds(f64),
}

/// Config for [`LadderPlan`]. Defaults follow the hertzforge-random ladder with sine
/// semantics: sysID at -20 dB, start at -12 dB, table
/// `[-12, -9, -6, -3, 0]`, one sweep per rung.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LadderConfig {
    /// SysID pass level, dB relative to full (informational for the service,
    /// which runs sysID BEFORE the ladder starts).
    pub sysid_level_db: f64,
    /// Starting level, dB (conventionally `table_db[0]`).
    pub start_db: f64,
    /// The rung table, dB relative to full level. The LAST entry is the full
    /// rung governed by `full_for`.
    pub table_db: Vec<f64>,
    /// Auto-advance rule for every rung except the last.
    pub per_rung: RungFor,
    /// Completion rule for the last (full) rung.
    pub full_for: RungFor,
}

impl Default for LadderConfig {
    fn default() -> Self {
        LadderConfig {
            sysid_level_db: -20.0,
            start_db: -12.0,
            table_db: vec![-12.0, -9.0, -6.0, -3.0, 0.0],
            per_rung: RungFor::Sweeps(1),
            full_for: RungFor::Sweeps(1),
        }
    }
}

/// Progress readout for [`LadderPlan`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LadderProgress {
    /// Index of the rung whose clock is running.
    pub rung_index: usize,
    /// The commanded level in dB (may sit between rungs after manual
    /// [`LadderPlan::step_db`] moves).
    pub level_db: f64,
    pub rung_elapsed_s: f64,
    pub rung_sweeps_done: u32,
    pub state: SweepState,
}

/// The level ladder (the hertzforge-random ladder with sine semantics): run each
/// rung for its configured sweeps/seconds, then step up; the final table entry
/// is the full level governed by `full_for`.
///
/// # What this type does NOT own (jump does not restart the sweep)
///
/// The ladder never touches the sweep engine. [`LadderPlan::jump_to`] and
/// [`LadderPlan::step_db`] move the LEVEL only; the caller's [`BidirSweep`]
/// keeps its frequency, direction, and counters - a rung jump mid-sweep
/// continues at the current frequency. Only the RUNG clock resets on a
/// jump.
///
/// # Manual mode
///
/// MANUAL means the ladder does not ADVANCE ITSELF: rung clocks still run and
/// are still reported, but a met rule does not step the level or complete the
/// ladder. The operator moves it, with [`LadderPlan::step_db`] (a +-1 dB trim
/// that may land between rungs and does not reset the rung clock) or
/// [`LadderPlan::jump_to`] / [`LadderPlan::jump_to_nearest_db`] (a rung change,
/// which does).
///
/// **This paragraph used to describe manual mode without there being one.** The
/// type had no manual flag at all, so `SineState::run_manual` in the Tauri host
/// was literally `self.run_auto(...)` and the ladder auto-advanced through every
/// rung while the panel said MANUAL. [`LadderPlan::set_manual`] is the mode; the
/// doc now describes code that exists.
#[derive(Debug, Clone)]
pub struct LadderPlan {
    cfg: LadderConfig,
    rung_idx: usize,
    level_db: f64,
    rung_elapsed_s: f64,
    rung_sweeps: u32,
    held: bool,
    done: bool,
    /// MANUAL: rung rules are evaluated and reported but never acted on. See the
    /// type doc; the operator owns every level move while this is set.
    manual: bool,
}

impl LadderPlan {
    /// Build from config; the table must be nonempty with finite entries.
    pub fn new(cfg: LadderConfig) -> Result<Self, ControlError> {
        if cfg.table_db.is_empty() {
            return Err(ControlError::BadConfig("ladder table is empty".into()));
        }
        for (i, &l) in cfg.table_db.iter().enumerate() {
            if l.is_nan() || !l.is_finite() {
                return Err(ControlError::BadConfig(format!(
                    "ladder table entry {i} is not finite ({l})"
                )));
            }
        }
        validate_rung_for(cfg.per_rung)?;
        validate_rung_for(cfg.full_for)?;
        Ok(LadderPlan {
            level_db: cfg.table_db[0],
            cfg,
            rung_idx: 0,
            rung_elapsed_s: 0.0,
            rung_sweeps: 0,
            held: false,
            done: false,
            manual: false,
        })
    }

    /// Set AUTO (`false`) / MANUAL (`true`). See the type doc.
    ///
    /// Switching to AUTO does NOT retroactively advance a rung whose rule was met
    /// while manual: the next `tick` / `note_sweep_completed` re-evaluates against
    /// the clock as it stands, so taking over and handing back holds the level the
    /// operator is at rather than jumping (a live authority change must be
    /// neutral in the physical quantity).
    pub fn set_manual(&mut self, manual: bool) {
        self.manual = manual;
    }

    /// Whether the ladder is in MANUAL.
    pub fn manual(&self) -> bool {
        self.manual
    }

    /// The rung whose level is NEAREST `db`, and that rung's level.
    ///
    /// THE one expression of the SetLevel{Db} resolution rule, shared by the
    /// ladder and by the service's echo so the number the operator sees is the
    /// number the ladder applied. Nearest, not floor and not ceiling: an operator
    /// typing a dB between two rungs means the one they are closest to, and a
    /// request outside the table lands on the end rung it is nearest - which is
    /// the CLAMP, expressed by the same arithmetic instead of a separate branch
    /// that could disagree with it. Ties go to the LOWER rung (`<` not `<=`),
    /// because the safe side of an ambiguous level request is down.
    pub fn nearest_rung(table_db: &[f64], db: f64) -> Option<(usize, f64)> {
        if table_db.is_empty() || db.is_nan() {
            return None;
        }
        let mut best = (0usize, table_db[0]);
        let mut best_d = (table_db[0] - db).abs();
        for (i, &l) in table_db.iter().enumerate().skip(1) {
            let d = (l - db).abs();
            if d < best_d {
                best = (i, l);
                best_d = d;
            }
        }
        Some(best)
    }

    /// Jump to the rung NEAREST `db` (see [`LadderPlan::nearest_rung`]). Returns
    /// the level actually applied - the ECHO, never the request.
    pub fn jump_to_nearest_db(&mut self, db: f64) -> Option<f64> {
        let (i, _) = Self::nearest_rung(&self.cfg.table_db, db)?;
        self.jump_to(i).ok()
    }

    fn rung_rule(&self) -> RungFor {
        if self.rung_idx + 1 == self.cfg.table_db.len() {
            self.cfg.full_for
        } else {
            self.cfg.per_rung
        }
    }

    fn advance_if_met(&mut self) {
        // MANUAL suppresses the ADVANCE, not the clock: `rung_elapsed_s` and
        // `rung_sweeps` keep accruing above this line so the progress readout
        // stays truthful, and a switch back to AUTO re-evaluates from there.
        if self.manual {
            return;
        }
        let met = match self.rung_rule() {
            RungFor::Sweeps(n) => self.rung_sweeps >= n,
            RungFor::Seconds(s) => self.rung_elapsed_s >= s,
        };
        if !met {
            return;
        }
        if self.rung_idx + 1 >= self.cfg.table_db.len() {
            self.done = true;
        } else {
            self.rung_idx += 1;
            self.level_db = self.cfg.table_db[self.rung_idx];
            self.rung_elapsed_s = 0.0;
            self.rung_sweeps = 0;
        }
    }

    /// Advance the rung clock by `dt` (only meaningful for `Seconds` rungs);
    /// returns the commanded level in dB. The rung clock pauses during hold.
    pub fn tick(&mut self, dt: f64) -> f64 {
        if self.held || self.done || dt <= 0.0 || dt.is_nan() {
            return self.level_db;
        }
        self.rung_elapsed_s += dt;
        self.advance_if_met();
        self.level_db
    }

    /// Report one completed sweep pass to the rung counter (only meaningful
    /// for `Sweeps` rungs). Ignored while held (the rung clock is
    /// paused; in practice a held sweep produces no completions anyway).
    pub fn note_sweep_completed(&mut self) {
        if self.held || self.done {
            return;
        }
        self.rung_sweeps += 1;
        self.advance_if_met();
    }

    /// Manual level trim: move the commanded level by `delta_db`, clamped to
    /// the table's `[min, max]` range. Returns the new level. Does NOT reset
    /// the rung clock and does not restart the sweep.
    pub fn step_db(&mut self, delta_db: f64) -> f64 {
        if delta_db.is_nan() {
            return self.level_db;
        }
        let lo = self
            .cfg
            .table_db
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let hi = self
            .cfg
            .table_db
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        self.level_db = (self.level_db + delta_db).clamp(lo, hi);
        self.level_db
    }

    /// Jump to rung `i`: the level moves to `table_db[i]` and the RUNG clock
    /// resets; the sweep is NOT restarted (the caller's sweep continues
    /// at its current frequency). Jumping re-opens a completed ladder.
    pub fn jump_to(&mut self, i: usize) -> Result<f64, ControlError> {
        if i >= self.cfg.table_db.len() {
            return Err(ControlError::BadInputs(format!(
                "rung {i} out of range (table has {})",
                self.cfg.table_db.len()
            )));
        }
        self.rung_idx = i;
        self.level_db = self.cfg.table_db[i];
        self.rung_elapsed_s = 0.0;
        self.rung_sweeps = 0;
        self.done = false;
        Ok(self.level_db)
    }

    /// Pause the rung clock.
    pub fn hold(&mut self) {
        if !self.done {
            self.held = true;
        }
    }

    /// Resume the rung clock.
    pub fn resume(&mut self) {
        self.held = false;
    }

    /// The commanded level in dB.
    pub fn level_db(&self) -> f64 {
        self.level_db
    }

    /// The active rung index.
    pub fn rung_index(&self) -> usize {
        self.rung_idx
    }

    pub fn done(&self) -> bool {
        self.done
    }

    /// Full progress readout.
    pub fn progress(&self) -> LadderProgress {
        LadderProgress {
            rung_index: self.rung_idx,
            level_db: self.level_db,
            rung_elapsed_s: self.rung_elapsed_s,
            rung_sweeps_done: self.rung_sweeps,
            state: if self.done {
                SweepState::Done
            } else if self.held {
                SweepState::Held
            } else {
                SweepState::Running
            },
        }
    }
}

fn validate_rung_for(r: RungFor) -> Result<(), ControlError> {
    match r {
        RungFor::Sweeps(n) => {
            if n == 0 {
                return Err(ControlError::BadConfig("rung sweeps must be >= 1".into()));
            }
        }
        RungFor::Seconds(s) => {
            if s <= 0.0 || s.is_nan() {
                return Err(ControlError::BadConfig(format!(
                    "rung seconds must be > 0 (got {s})"
                )));
            }
        }
    }
    Ok(())
}
