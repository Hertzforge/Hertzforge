//! The Path-B plant plumbing + the thin reactive kinematic monitor.
//!
//! # Plant plumbing (replicated from `eigen-demo`, not imported)
//!
//! The Sim closed loop runs the plant THROUGH the real [`eigen_engine::Engine`] one
//! control window (segment) at a time, with the heavy control DSP (Welch PSD, FRF, COLA
//! synth, servo, trim) amortized BETWEEN segments off the tick path. This is the exact
//! shape `eigen-demo` proved and the adversarial fixtures use. The service owns a
//! persistent [`PlantLoop`] so the backend/plant/noise stream is continuous across
//! segments.
//!
//! ## Block cadence
//!
//! The engine's `frames_per_tick` is DERIVED from what the backend configured
//! ([`LoopConfig::block_frames`] -> `AcquisitionConfig` -> `ConfiguredActual` ->
//! [`eigen_engine::EngineConfig::with_block`]), never hard-coded here.
//!
//! It used to be pinned at 1 with a comment citing "SimBackend latches ONE drive value
//! per block". **That constraint no longer exists** - the latch was removed, and
//! the Sim now buffers the full per-frame drive stream at any block size. The EIGEN-SIM
//! plant path still REQUESTS one frame per tick, for the reason named by
//! [`eigen_engine::SIM_NATURAL_BLOCK_FRAMES`] (sample-exact segment accounting: this loop
//! counts a segment in samples and hands that count to the engine as `max_ticks`), and
//! the tick budget is now computed from the block rather than assumed. The OTHER original
//! constraint stands unchanged: a Welch inside `process` overruns the per-tick compute
//! budget, so the block DSP stays off the tick path.
//!
//! # The kinematic monitor (reactive tri-field, mirroring the Explorer's
//! AbortMonitor shape but built DIRECTLY on `eigen_engine::limits`)
//!
//! The Random kinematic abort reuses `eigen_engine::limits::ChannelLimiter` (the ONE
//! tri-field implementation the watchdog + SMART share). This crate does NOT depend on
//! the Explorer crates, so [`KinematicMonitor`] here is a thin re-implementation of
//! the Explorer's `AbortMonitor` wrapper: one limiter per enabled non-drive channel with
//! a limit set, a settle window suppressing derived-axis (vel/disp) trips during the
//! integrator's startup transient, and a latched first-trip. The accel axis is always
//! live. Force protection is PREDICTIVE/NOTCH only, so there is no force
//! abort axis here.

use eigen_channel_table::{ChannelRole, ChannelTable};
use eigen_dsp::cola::ColaSynth;
use eigen_engine::limits::{AbortAxis, AbortTrip, ChannelLimiter};
use eigen_engine::{Engine, EngineConfig, ProcessStep, StopReason, VecCaptureSink};
use eigen_hal::Backend;

/// Loop knobs for a Random run: rate, spectral resolution, control band, engine
/// health budgets. BACKEND-NEUTRAL - it describes the LOOP, never the plant.
///
/// ## What left this struct, and why
///
/// It used to be called `PlantConfig` and carried the Sim plant description too
/// (`resonances`, `noise_level`, `feedthrough`, `plant_seed`, `shared_clock`,
/// `input_channels`), which [`PlantLoop::open`] turned into a `SimBackend` on the
/// spot. That made every Random run structurally incapable of touching hardware:
/// the ONLY backend the service could ever have was one it built itself out of a
/// physics description.
///
/// The plant half now lives with whoever builds the backend - the UI's station
/// factory for a run, the test file for a fixture - and [`PlantLoop::open`] takes
/// the finished `Box<dyn Backend>`. A name that says "plant" for a struct that
/// describes no plant is doc-vs-byte drift, so the struct is `LoopConfig` now.
#[derive(Debug, Clone)]
pub struct LoopConfig {
    pub fs_hz: f64,
    pub nfft: usize,
    pub overlap: f64,
    /// Interior control band low edge (Hz).
    pub band_lo_hz: f64,
    /// Interior control band high edge (Hz).
    pub band_hi_hz: f64,
    /// COLA drive PRNG seed.
    pub cola_seed: u64,
    /// Control windows per analysis segment (Welch averages ~ this / (1-overlap)).
    pub blocks_per_window: usize,
    /// Frames per acquisition block to REQUEST at configure.
    ///
    /// The engine's `frames_per_tick` comes from what the backend answers, not
    /// from this - see [`PlantLoop::open`]. Defaults to
    /// [`eigen_engine::SIM_NATURAL_BLOCK_FRAMES`] because the EIGEN-SIM plant path
    /// counts a segment in SAMPLES and passes that count as the engine's
    /// `max_ticks`. On a HARDWARE station [`PlantLoop::open`] replaces that Sim
    /// value with `eigen_engine::default_block_frames(fs_hz)` before it configures,
    /// and then adopts whatever the device answered - see
    /// [`LoopConfig::adopt_station_actual`]. A hardware block the caller set
    /// deliberately is honored.
    pub block_frames: usize,
    /// Allowed period-jitter fraction for the engine's scheduling health.
    ///
    /// [`eigen_engine::TEST_DISABLE_JITTER_GUARD`] on the deterministic harness
    /// path (this loop runs flat out and then stops for the control DSP between
    /// segments, so wall-clock tick spacing means nothing); a hardware path leaves
    /// the engine's own budget-derived default in place.
    pub jitter_frac: f64,
    /// Per-tick compute budget handed to the engine's scheduling health.
    ///
    /// `Some(eigen_engine::TEST_RELAX_COMPUTE_BUDGET)` on the deterministic
    /// harness path: the engine's default budget is the TICK PERIOD (~122 us at
    /// this fixture's one-frame block), so an OS hiccup inside a `process` call
    /// that genuinely takes nanoseconds trips `ComputeOverrun` and aborts the
    /// segment. That spurious fault is exactly what the eight-deep retry loop
    /// (since removed) was papering over; relaxing the budget removes the
    /// CAUSE rather than the symptom. A HARDWARE path leaves this `None` - there
    /// the tick period IS the deadline.
    pub compute_budget: Option<std::time::Duration>,
}

impl LoopConfig {
    /// The eigen-demo loop settings: 8192 Hz, 1024-point Welch at 50% overlap, a
    /// 60-780 Hz interior control band, and the deterministic-harness engine
    /// budgets. Pairs with the EIGEN-SIM demo plant (two in-band modes at 200 and
    /// 550 Hz, Q 2.5, noise 2e-4, feedthrough 0.35, seed `0x5A5A_1234`) - the
    /// plant is built by the caller now and is NOT described here.
    pub fn demo_default() -> Self {
        LoopConfig {
            fs_hz: 8_192.0,
            nfft: 1024,
            overlap: 0.5,
            band_lo_hz: 60.0,
            band_hi_hz: 780.0,
            cola_seed: 0xC0FF_EE12_3456_7890,
            blocks_per_window: 12,
            block_frames: eigen_engine::SIM_NATURAL_BLOCK_FRAMES,
            jitter_frac: eigen_engine::TEST_DISABLE_JITTER_GUARD,
            compute_budget: Some(eigen_engine::TEST_RELAX_COMPUTE_BUDGET),
        }
    }

    /// One-sided PSD bins for this nfft.
    pub fn nbins(&self) -> usize {
        self.nfft / 2 + 1
    }

    /// Frequency-bin width (Hz).
    pub fn df(&self) -> f64 {
        self.fs_hz / self.nfft as f64
    }

    /// Bin index for a frequency (rounded).
    pub fn bin_of(&self, hz: f64) -> usize {
        (hz / self.df()).round() as usize
    }

    /// The half-open interior band `[lo,hi)` in COLA/Welch bins, strictly inside
    /// `(0, Nyquist)` (the documented COLA edge-bin limitation).
    pub fn band_bins(&self) -> (usize, usize) {
        let lo = self.bin_of(self.band_lo_hz).max(1);
        let hi = self.bin_of(self.band_hi_hz).min(self.nfft / 2 - 1) + 1;
        (lo, hi.max(lo + 1))
    }

    /// Samples in one analysis segment (a whole number of COLA hops).
    pub fn window_len(&self) -> usize {
        let hop = ((self.nfft as f64) * (1.0 - self.overlap)).round() as usize;
        let want = self.blocks_per_window * self.nfft;
        want.div_ceil(hop) * hop
    }

    /// The segment duration (s): `window_len / fs`.
    pub fn segment_seconds(&self) -> f64 {
        self.window_len() as f64 / self.fs_hz
    }

    /// Rewrite this config to describe the loop the STATION will actually run.
    ///
    /// Called by [`PlantLoop::open`] the instant `Backend::configure` answers, and
    /// BEFORE the arm gate, the COLA synth, and the analysis grid are built from it.
    ///
    /// ## Why this exists (first real-chassis Random arm, 2026-07-30)
    ///
    /// [`LoopConfig::demo_default`] describes the EIGEN-SIM plant: 8192 Hz, a
    /// one-frame block, and the two named deterministic-harness relaxations. The UI's
    /// `loop_from_config` starts from it and adjusts only the band edges, so a Random
    /// run pointed at an NI station carried the Sim's numbers onto hardware. The
    /// delta-sigma 9234 programs 51 200 Hz; the COLA synthesizer, the analysis grid
    /// (`df = fs/nfft`) and every windowed statistic kept running at 8192, so every
    /// profile frequency would have landed 6.25x high on a shaker. The coherence gate
    /// caught it (band coherence 0.000, hold, level never raised) - the architecture
    /// working - but a module must not run 6.25x wrong and lean on a downstream gate.
    ///
    /// Three things are adopted, each from the authority that actually knows it:
    ///
    /// 1. **The rate**, from [`eigen_hal::ConfiguredActual::actual_rate_hz`] - what
    ///    the driver programmed, not what anyone asked for. Random has no baked-in
    ///    rate to contradict: its profile is a Hz-denominated breakpoint table and its
    ///    band edges are Hz, so re-deriving the grid at the device rate IS the
    ///    regeneration the arm gate's rate policy would otherwise demand.
    /// 2. **The block**, from `ConfiguredActual::block_frames` rather than re-derived
    ///    from [`eigen_engine::default_block_frames`]. That helper answers "what block
    ///    should a hardware station REQUEST at this rate"; by the time we are here the
    ///    request is spent and the device has answered. `EngineConfig::with_block`
    ///    already builds the engine's cadence from the same `actual`, so taking the
    ///    device's number keeps ONE source instead of two that can drift.
    /// 3. **The scheduling-health guards**, from the STATION. The harness values are
    ///    documented as never-on-hardware ([`eigen_engine::TEST_DISABLE_JITTER_GUARD`]
    ///    disables the period-jitter check; [`eigen_engine::TEST_RELAX_COMPUTE_BUDGET`]
    ///    widens a per-tick deadline to one second) and eigen-sine already gates them
    ///    by station. Random did not, so `demo_default` carried both onto NI - which
    ///    mattered nothing while the rate divergence kept Random from arming at all,
    ///    and would have mattered the moment this fix let it arm.
    ///
    /// An `EigenSim` station is left EXACTLY as configured: its rate never diverges
    /// (the Sim adopts the request), its one-frame block is the deliberate
    /// sample-exact segment accounting [`eigen_engine::SIM_NATURAL_BLOCK_FRAMES`]
    /// names, and the harness relaxations are true there.
    ///
    /// Refuses, rather than silently running a grid that cannot carry the profile -
    /// see [`GridRefusal`].
    pub fn adopt_station_actual(
        &mut self,
        station: eigen_module_abi::StationClass,
        actual: &eigen_hal::ConfiguredActual,
    ) -> Result<StationAdoption, GridRefusal> {
        let before = GridSnapshot::of(self);
        let rate = actual.actual_rate_hz;

        // A rate we cannot compute a grid from is a refusal, never a fallback to the
        // Sim default: silently keeping 8192 Hz is the exact failure this seam exists
        // to end.
        if !(rate.is_finite() && rate > 0.0) {
            return Err(GridRefusal::UnusableRate {
                actual_rate_hz: rate,
            });
        }

        // The rate is EXACT-equal by the same tolerance the arm gate's rate policy
        // uses, so "adopted" and "the gate accepts" can never disagree about what
        // counts as a match.
        let diverged = (rate - self.fs_hz).abs() / rate > eigen_module_abi::RATE_EXACT_TOLERANCE;

        if diverged {
            // Probe the grid BEFORE committing: `band_bins` clamps, so an unusable
            // grid would otherwise present as a quietly narrower control band.
            let mut probe = self.clone();
            probe.fs_hz = rate;
            probe.check_resolves_band()?;
            self.fs_hz = rate;
        }

        if station.is_hardware() {
            self.block_frames = actual.block_frames.max(1);
            // The engine's own block-derived allowance and deadline. Named
            // explicitly rather than by absence, so the hardware path reads as a
            // decision instead of a default nobody revisited.
            self.jitter_frac = EngineConfig::new(1, 1).jitter_frac;
            self.compute_budget = None;
        }

        Ok(StationAdoption {
            station,
            before,
            after: GridSnapshot::of(self),
        })
    }

    /// Does this config's grid still carry the control band the profile asked for?
    ///
    /// Two ways it fails to, both of which [`LoopConfig::band_bins`] would otherwise
    /// absorb into a silently different band:
    ///
    /// - the band's high edge sits at or above the grid's top usable bin (the COLA
    ///   edge-bin limit at `nfft/2 - 1`), so the top of the profile would never be
    ///   driven and nothing downstream would say so;
    /// - the band collapses to fewer than [`MIN_CONTROL_LINES`] lines.
    fn check_resolves_band(&self) -> Result<(), GridRefusal> {
        let df = self.df();
        if !(df.is_finite() && df > 0.0) {
            return Err(GridRefusal::UnusableRate {
                actual_rate_hz: self.fs_hz,
            });
        }
        let top_usable = self.nfft / 2 - 1;
        let hi_unclamped = (self.band_hi_hz / df).round() as usize;
        if hi_unclamped > top_usable {
            return Err(GridRefusal::BandAboveGrid {
                band_hi_hz: self.band_hi_hz,
                top_usable_hz: top_usable as f64 * df,
                fs_hz: self.fs_hz,
                nfft: self.nfft,
            });
        }
        let (lo, hi) = self.band_bins();
        let lines = hi.saturating_sub(lo);
        if lines < MIN_CONTROL_LINES {
            return Err(GridRefusal::TooFewControlLines {
                lines,
                minimum: MIN_CONTROL_LINES,
                df_hz: df,
                band_lo_hz: self.band_lo_hz,
                band_hi_hz: self.band_hi_hz,
                fs_hz: self.fs_hz,
                nfft: self.nfft,
            });
        }
        Ok(())
    }
}

/// The fewest control lines a Random loop will run a profile on.
///
/// A FLOOR, not a target. `nfft` is fixed by the caller while `df = fs/nfft`
/// follows the device, so adopting a much higher station rate coarsens the grid:
/// the demo's 1024-point grid is 8 Hz at 8192 Hz and 50 Hz at the 9234's 51 200 Hz.
/// Below a handful of lines the H-inverse law, the notch limiter and the envelope
/// check have fewer degrees of freedom than a typical breakpoint table has
/// segments, and what the loop controls is no longer the profile that was asked
/// for - so it refuses instead.
///
/// The real answer is to DERIVE `nfft` from the requested resolution
/// (`RandomTestConfig::df_hz`) at the adopted rate rather than inheriting the Sim's
/// 1024. That is a larger change than the rate divergence this seam closes, and it
/// is filed rather than smuggled in here; this floor is what keeps the interim
/// honest.
pub const MIN_CONTROL_LINES: usize = 8;

/// The grid numbers, before and after an adoption, for the event log.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSnapshot {
    pub fs_hz: f64,
    pub df_hz: f64,
    pub block_frames: usize,
    pub lines: usize,
    pub jitter_frac: f64,
    pub compute_budget: Option<std::time::Duration>,
}

impl GridSnapshot {
    fn of(cfg: &LoopConfig) -> Self {
        let (lo, hi) = cfg.band_bins();
        GridSnapshot {
            fs_hz: cfg.fs_hz,
            df_hz: cfg.df(),
            block_frames: cfg.block_frames,
            lines: hi.saturating_sub(lo),
            jitter_frac: cfg.jitter_frac,
            compute_budget: cfg.compute_budget,
        }
    }
}

/// What [`LoopConfig::adopt_station_actual`] changed. Carried on the [`PlantLoop`]
/// so the service can put it in the run's event log verbatim - a rate that moved
/// under a run is exactly the kind of thing that must be readable afterwards from
/// the log alone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StationAdoption {
    pub station: eigen_module_abi::StationClass,
    pub before: GridSnapshot,
    pub after: GridSnapshot,
}

impl StationAdoption {
    /// Did the station program a different rate than the loop asked for?
    pub fn rate_changed(&self) -> bool {
        self.before.fs_hz != self.after.fs_hz
    }

    /// Did the analysis grid move? (`nfft` is fixed here, so this tracks the rate,
    /// but the service asks the question it actually means: must the grid-derived
    /// state - band, target, envelope - be rebuilt?)
    pub fn grid_changed(&self) -> bool {
        self.before.df_hz != self.after.df_hz
    }

    /// Were the deterministic-harness scheduling relaxations dropped for a hardware
    /// station?
    pub fn guards_restored(&self) -> bool {
        self.before.jitter_frac != self.after.jitter_frac
            || self.before.compute_budget != self.after.compute_budget
    }
}

/// Why a station's rate could not be adopted. Typed so a UI can branch on the
/// variant later; `Display` is what reaches the operator today.
#[derive(Debug, Clone, PartialEq)]
pub enum GridRefusal {
    /// The backend reported a rate no grid can be built from.
    UnusableRate { actual_rate_hz: f64 },
    /// The profile band's high edge sits above the adopted grid's top usable bin.
    BandAboveGrid {
        band_hi_hz: f64,
        top_usable_hz: f64,
        fs_hz: f64,
        nfft: usize,
    },
    /// The adopted grid leaves fewer than [`MIN_CONTROL_LINES`] lines in band.
    TooFewControlLines {
        lines: usize,
        minimum: usize,
        df_hz: f64,
        band_lo_hz: f64,
        band_hi_hz: f64,
        fs_hz: f64,
        nfft: usize,
    },
}

impl std::fmt::Display for GridRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GridRefusal::UnusableRate { actual_rate_hz } => write!(
                f,
                "the station configured {actual_rate_hz} Hz, which is not a rate an \
                 analysis grid can be built from. Eigen will not fall back to the \
                 configured rate: the loop would then synthesize drive for a rate the \
                 device is not playing at."
            ),
            GridRefusal::BandAboveGrid {
                band_hi_hz,
                top_usable_hz,
                fs_hz,
                nfft,
            } => write!(
                f,
                "the control band reaches {band_hi_hz} Hz but the station's {fs_hz} Hz \
                 at nfft {nfft} resolves only to {top_usable_hz:.1} Hz. Arming would \
                 drive the profile up to {top_usable_hz:.1} Hz and silently leave the \
                 rest undriven. Lower the profile's top breakpoint or raise nfft, and \
                 arm again."
            ),
            GridRefusal::TooFewControlLines {
                lines,
                minimum,
                df_hz,
                band_lo_hz,
                band_hi_hz,
                fs_hz,
                nfft,
            } => write!(
                f,
                "the station configured {fs_hz} Hz, which at nfft {nfft} is {df_hz:.3} Hz \
                 per line and leaves {lines} control lines across {band_lo_hz}-{band_hi_hz} \
                 Hz (minimum {minimum}). That is too coarse to control the requested \
                 profile. Raise nfft for this rate, or narrow the profile, and arm again."
            ),
        }
    }
}

/// A cheap engine step: play a pre-synthesized drive stream and record EVERY input
/// channel's response per frame (interleaved), so the multi-channel monitor path can
/// read the raw telemetry. Does no FFT per tick (never overruns the tick budget).
/// A DISPLAY tap fired once per engine tick with the block just acquired
/// (interleaved, `n_ch` samples per frame).
///
/// Why this exists: a control segment is many engine ticks
/// long (`segment_ticks()`), so a display fed only at segment boundaries gets
/// ~1.4 s of samples in one burst every ~1.4 s - the operator sees flashing
/// chunks and, worse, cannot get a near-real-time picture to make decisions
/// on. The samples exist per TICK; this hands them out as they land. It does
/// NOT add a second reader of the device task (which would steal frames from
/// control) and it does not change the control math one bit - the same
/// samples still accumulate into the same segment.
///
/// Runs INSIDE the control hot path: implementations must be cheap and must
/// never block (copy + non-blocking publish).
pub type BlockObserver = std::sync::Arc<dyn Fn(&[f32], usize) + Send + Sync>;

struct PlaybackStep {
    drive: Vec<f64>,
    pos: usize,
    n_ch: usize,
    /// Interleaved recorded response: `n_ch` samples per frame.
    interleaved: Vec<f64>,
    drive_peak: f64,
    /// Per-tick display tap. `None` = nothing observes.
    observer: Option<BlockObserver>,
    /// Frames emitted as ZERO because the tick count outran the synthesized drive
    /// record. Non-zero means the engine asked for more frames than the segment
    /// synthesized and the tail went silent - a real per-segment drive dropout, not a
    /// display artifact.
    zero_filled: usize,
}

impl PlaybackStep {
    fn new(drive: Vec<f64>, n_ch: usize) -> Self {
        PlaybackStep {
            drive,
            pos: 0,
            n_ch: n_ch.max(1),
            interleaved: Vec::new(),
            drive_peak: 0.0,
            observer: None,
            zero_filled: 0,
        }
    }

    fn with_observer(mut self, o: Option<BlockObserver>) -> Self {
        self.observer = o;
        self
    }
}

impl ProcessStep for PlaybackStep {
    fn process(&mut self, input: &[f32], n_frames: usize, drive_out: &mut Vec<f32>) {
        let in_ch = input
            .len()
            .checked_div(n_frames)
            .unwrap_or(self.n_ch)
            .max(1);
        drive_out.clear();
        drive_out.reserve(n_frames);
        for f in 0..n_frames {
            // Record every input channel for this frame (interleaved), up to n_ch.
            for c in 0..self.n_ch.min(in_ch) {
                self.interleaved.push(input[f * in_ch + c] as f64);
            }
            // Emit the next pre-synthesized drive sample (zero past the end).
            let d = match self.drive.get(self.pos) {
                Some(&v) => v,
                None => {
                    self.zero_filled += 1;
                    0.0
                }
            };
            self.pos += 1;
            if d.abs() > self.drive_peak {
                self.drive_peak = d.abs();
            }
            drive_out.push(d as f32);
        }
        // Display tap: hand this tick's freshly acquired block out
        // BEFORE the segment completes. Cheap by contract; never blocks.
        if let Some(obs) = &self.observer {
            obs(input, n_frames);
        }
    }
}

fn clone_cfg(c: &EngineConfig) -> EngineConfig {
    let mut e = EngineConfig::new(c.in_channels, c.out_channels);
    e.frames_per_tick = c.frames_per_tick;
    e.ring_frames = c.ring_frames;
    e.watchdog_hp_hz = c.watchdog_hp_hz;
    e.watchdog_sustain = c.watchdog_sustain;
    e.jitter_frac = c.jitter_frac;
    e.compute_budget = c.compute_budget;
    e.stall_deadline = c.stall_deadline;
    // Fixed 2026-08-08: `blanking` was silently DROPPED here for the life of
    // the file - harmless for Random (which never sets it) but a false-abort
    // generator for any caller that does (an IEPE settle window vanishing on
    // a rebuilt engine arms the watchdog into the turn-on transient). Copy
    // EVERY field; the value came from a validated config so the cap holds.
    e.set_blanking(c.blanking())
        .expect("blanking was validated when the source config was built");
    e
}

/// What the per-segment `arm()`/`start()` calls actually did.
///
/// The expectation is PER SEAM KIND:
/// * WARM (mid-run, `output_primed`): both REFUSE - the AI/AO tasks stay
///   Running across segment boundaries, and `run_segment` keeps the calls only
///   as a safety net. An Ok on a warm seam means the backend stopped for its
///   own reasons, and `NiBackend::start`'s silence top-up would put a
///   lookahead-sized quiet window on the wire - which is why the warm path
///   never wants to see it.
/// * COLD (segment 0, and every resume after `suspend()`): both SUCCEED, and
///   the prologue has already loaded the head of the segment's real drive, so
///   the restart plays drive from its first clocked sample (no silence
///   lead-in; the 2026-08-08 cold-start fix).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundaryTrace {
    /// `arm()` succeeded - the backend was NOT Running.
    pub armed_ok: bool,
    /// `start()` succeeded - the backend (re)started.
    pub started_ok: bool,
    /// AO FREE space before the segment's first write (frames). Large = empty.
    pub space_before: Option<usize>,
    /// AO FREE space after the segment's last write (frames). Large = the queue
    /// is nearly drained, so the inter-segment window plays silence.
    pub space_after: Option<usize>,
}

/// What one segment run produced.
pub struct SegmentResult {
    /// Interleaved per-input-channel response (`n_ch` per frame).
    pub interleaved: Vec<f64>,
    /// Number of input channels in the interleave.
    pub n_ch: usize,
    /// Peak absolute drive sample emitted.
    pub drive_peak: f64,
    /// The engine's raw-ring watchdog trip, if this segment aborted on a tri-field
    /// breach. The engine's [`AbortWatchdog`] is the PRIMARY kinematic abort (it reads
    /// the raw ring before any decimation and is faster than a post-segment check); the
    /// service surfaces this as a Kinematic trip. `None` if the segment ran clean.
    pub watchdog_trip: Option<AbortTrip>,
    /// A FAULTED engine stop the plant loop cannot itself recover from, as an
    /// operator-readable string: a stalled input, a
    /// raw-ring overrun with limits armed, a lost link, an underrun, or a
    /// `SchedulingFault` on a HARDWARE station. `None` when the segment ran to its
    /// tick budget.
    ///
    /// Previously every non-watchdog stop was silently discarded here, so a
    /// segment that acquired nothing came back looking exactly like a clean one.
    pub faulted: Option<String>,
    /// Frames this segment drove as ZERO because the engine's tick budget
    /// outran the synthesized drive record. Any non-zero value is a
    /// real silence written to the shaker at the segment tail.
    pub zero_filled: usize,
    /// Non-fatal DRIVER diagnostics reported during this segment (DAQmx
    /// underruns, late samples, rate coercions). Empty on a clean segment and
    /// on backends that report none.
    pub driver_warnings: Vec<String>,
}

impl SegmentResult {
    /// Extract input channel at interleave slot `slot` as an owned f64 vector.
    pub fn channel(&self, slot: usize) -> Vec<f64> {
        if self.n_ch == 0 {
            return Vec::new();
        }
        let frames = self.interleaved.len() / self.n_ch;
        (0..frames)
            .map(|f| {
                self.interleaved
                    .get(f * self.n_ch + slot)
                    .copied()
                    .unwrap_or(0.0)
            })
            .collect()
    }
}

/// A persistent closed loop: owns the backend (so the plant/noise stream is
/// continuous across segments) + the engine config + the interleave width. The service
/// synthesizes each segment's drive PSD, calls [`PlantLoop::run_segment`], and feeds
/// the recorded response back into the control law.
///
/// The backend is a `Box<dyn Backend>`: the loop is handed a finished station
/// rather than a physics description it turns into a `SimBackend`. Dynamic dispatch,
/// not generics, because the trait is object-safe by design and monomorphizing a
/// per-segment loop that already runs an FFT between segments buys nothing.
pub struct PlantLoop {
    /// Per-tick display tap; installed by the service, fired by
    /// `PlaybackStep` inside `run_segment`.
    block_observer: Option<BlockObserver>,
    /// What the last segment boundary's arm/start actually did.
    last_boundary: Option<BoundaryTrace>,
    /// Whether THIS run has already primed the output.
    output_primed: bool,
    /// The previous segment's undelivered drive frames, carried across the
    /// boundary so the next segment does not start on primed ZEROS.
    carry_queue: Vec<Vec<f32>>,
    backend: Box<dyn Backend>,
    table: ChannelTable,
    ecfg: EngineConfig,
    window_len: u64,
    n_ch: usize,
    /// The station class this loop armed against, captured at `open()` from the
    /// same value handed to the arm gate. It is what decides whether a
    /// `SchedulingFault` is a harness artifact or a real fault - see
    /// [`PlantLoop::run_segment`].
    station: eigen_module_abi::StationClass,
    /// What the station's configure answer changed about the loop config, kept so
    /// the service can log it against the run.
    adoption: StationAdoption,
    /// Drive-to-response PIPELINE LAG in frames, DERIVED as the output lookahead plus
    /// the device's reported group delay. Any estimate that correlates the two SIGNALS
    /// (H, coherence) must align by lag first - at nfft 1024 a ~2k-frame lag pairs
    /// every analysis window with a DIFFERENT random segment and in-band coherence on a
    /// jumpered wire reads ~0.04 (found 2026-08-01; TWR hit the identical failure at
    /// 0.010). Power estimates (the per-segment PSDs the control law consumes) are
    /// lag-blind in steady state and need no alignment.
    ///
    /// STATUS OF THE DERIVED VALUE (updated 2026-08-08): it was
    /// always an UNDERSHOOT of the measured truth (device/USB queues are
    /// invisible to caps), and since the cold-start prefill became REAL DRIVE
    /// it also OVERSTATES the lookahead term (the record now starts playing at
    /// clock start, so the record-to-response lag is ~group delay plus start
    /// skew). It is a diagnostic reference only; sysID measures its own lag
    /// from the pretest (floor-gated xcorr) and THAT is what aligns the
    /// estimate. TWR still derives its lag from the same formula and TWR's
    /// prefill is unchanged (still silence), so its derivation stays true.
    pipeline_lag_frames: usize,
}

impl Drop for PlantLoop {
    fn drop(&mut self) {
        // The loop OWNS the stop now that `run_segment` uses `run_ticks` (which
        // deliberately leaves the backend running across segment boundaries).
        // Without this the drive would keep playing from the last written buffer
        // until the HAL's own `Drop` cleared the tasks - true today via
        // `NiBackend::drop`, but a drive that stops as a side effect of a
        // deallocation order is not a safety property anyone should rely on.
        // `NiBackend::stop` takes AO down first, then AI: drive off, then
        // acquisition.
        let _ = self.backend.stop();
    }
}

impl PlantLoop {
    /// Open + configure + arm + start `backend` for `cfg` against `table`.
    /// Returns an error only if the backend refuses the table (validated up front).
    ///
    /// `backend` and `station` are supplied by the caller (the UI's station
    /// factory in a run, the test file in a fixture). They travel TOGETHER because
    /// the class is a property of what the backend is bound to, not of the backend
    /// type: the same `NiBackend` is `NiSimulated` or `NiReal` depending on what
    /// the driver says about the devices behind it.
    pub fn open(
        backend: Box<dyn Backend>,
        station: eigen_module_abi::StationClass,
        cfg: &mut LoopConfig,
        table: ChannelTable,
    ) -> Result<Self, String> {
        let mut backend = backend;
        backend.open().map_err(|e| format!("open: {e:?}"))?;

        // The block to REQUEST, before anyone can ask the device anything.
        //
        // `LoopConfig::block_frames` has always DOCUMENTED that a hardware station
        // uses `default_block_frames(fs)` and that the Sim's one-frame block is the
        // EIGEN-SIM sample-exact segment accounting. Nothing implemented it: the UI's
        // `loop_from_config` starts from `demo_default` and never revisits the field,
        // so a Random run on NI requested a ONE-FRAME block, and the NI backend -
        // which echoes the requested block back as `ConfiguredActual::block_frames` -
        // programmed it. At the 9234's 51 200 Hz that is a 19.5 us tick period, which
        // is also the engine's per-tick compute deadline once the harness relaxations
        // come off below. Requesting a Sim block on hardware and restoring the
        // scheduling guards are not two changes; either alone is wrong.
        //
        // Only the SIM value is overridden. A caller who set a hardware block
        // deliberately keeps it; `default_block_frames` never returns below
        // `MIN_BLOCK_FRAMES`, so a one-frame block on a DAQ card is never a choice.
        if station.is_hardware() && cfg.block_frames <= eigen_engine::SIM_NATURAL_BLOCK_FRAMES {
            cfg.block_frames = eigen_engine::default_block_frames(cfg.fs_hz);
        }
        let acq = eigen_hal::AcquisitionConfig::new(cfg.fs_hz, cfg.block_frames.max(1));
        let actual = backend
            .configure(&table, &acq)
            .map_err(|e| format!("configure: {e:?}"))?;

        // ADOPT WHAT THE DEVICE PROGRAMMED, before anything is derived from `cfg`.
        //
        // `cfg` is `&mut` for exactly this: the caller's copy must come back
        // describing the loop that will really run, because the caller builds the
        // COLA synth, the analysis grid and the protection envelopes from it after
        // `open` returns. Everything below this line - the arm gate included - sees
        // the adopted numbers. See `LoopConfig::adopt_station_actual` for why each
        // of the three adopted things comes from where it does.
        let adoption = cfg
            .adopt_station_actual(station, &actual)
            .map_err(|refusal| format!("arm refused: {refusal}"))?;

        // THE ARM GATE. Random requests `{HostSynthesized, HostPerTick}`: the
        // COLA synthesizer produces every drive sample and the control law
        // corrects inside the run. Carries the shared-clock precondition.
        //
        // The station comes from the factory. It is held on the loop from
        // here so the SchedulingFault rule reads the SAME value the gate
        // was given - one source, not two that can drift.
        eigen_module_abi::arm_gate(
            &backend.caps(),
            station,
            eigen_module_abi::Resolution::host_closed_loop(),
            // THE LOOP'S OWN DSP RATE is the rate that must match what the driver
            // programmed. The `None` this replaced said "Random carries no
            // baked-in rate", and that was false: the COLA synthesizer, the
            // analysis grid (df = fs/nfft) and every windowed statistic run at
            // `cfg.fs_hz`. On real hardware the two diverged silently - Random
            // asked for its 8192 Hz Sim default, the 9234 coerced to 51 200 Hz,
            // and the loop kept synthesizing drive as if it played at 8192: every
            // profile frequency landed 6.25x high on the shaker (first
            // real-chassis Random run 2026-07-30).
            //
            // The adoption above now re-derives the loop AT the device's rate
            // before this line, so for a station that reports a usable rate this
            // check agrees by construction. That is deliberate and it is not the
            // check going vacuous: Random's profile is Hz-denominated, so
            // re-deriving IS the regeneration `RateMismatch` would have asked the
            // operator to perform by hand. What CAN still fail here is a backend
            // that reports a rate the adoption refused to build a grid from - and
            // the falsifiable half of the rate story moved one line up, into
            // `check_resolves_band`, which refuses a grid the profile does not
            // survive. Passing the loop's own rate (not `None`) keeps the gate's
            // statement true rather than absent.
            Some(cfg.fs_hz),
            actual.actual_rate_hz,
        )
        .map_err(|reason| format!("arm refused: {reason}"))?;

        backend.arm().map_err(|e| format!("arm: {e:?}"))?;
        // Changed 2026-08-08: `open` used to pre-write a lookahead of ZEROS
        // and START the AO clock here - before any drive existed. That gave the
        // host exactly `lookahead_frames / rate` (30 ms on the cDAQ) to deliver
        // the first real write, and at nfft 32768 the work between this point
        // and that write (rest of arm, first synth_segment, full-record clones)
        // did not fit: the AO generated its 512 zeros and underran. The clock
        // now starts in `run_segment`'s cold-path prologue, AFTER the segment's
        // drive is in hand and its head is already in the device buffer - so
        // the first thing a run ever plays is drive. `arm()` leaves the tasks
        // committed; `write_drive` accepts into a committed task before start.

        let n_ch = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .count()
            .max(1);
        let n_drive = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
            .count()
            .max(1);

        // The engine interleave width MUST match the table's enabled input/drive counts
        // (the backend delivers `n_ch` input samples per frame and expects `n_drive`
        // drive samples). A mismatch de-interleaves wrong and collapses the FRF/coherence.
        // The BLOCK comes from what the backend configured, which also
        // sets the ring depth and the scheduling-health budget.
        let mut ecfg = EngineConfig::with_block(n_ch, n_drive, &actual);
        // Random control is peaky; a spike-sensitive watchdog would false-trip on
        // Gaussian tails. The healthy converged loop stays well under the abort limit.
        //
        // Constants review: `watchdog_sustain` counts RAW FRAMES, so 64
        // is a TIME only against a rate - the EIGEN-SIM plant's 65 536 Hz, where
        // it is ~1 ms. On a hardware station the rate is whatever the driver
        // programmed, and the same 64 at a 1 kHz configuration would be 64 ms of
        // continuous over-limit before the e-stop. EIGEN-SIM keeps the literal
        // (this review does not change Sim behaviour); hardware states the
        // debounce in SECONDS and converts at the actual rate.
        ecfg.watchdog_sustain = if station.is_hardware() {
            eigen_engine::watchdog_sustain_frames(WATCHDOG_SUSTAIN_SECONDS, actual.actual_rate_hz)
        } else {
            SIM_WATCHDOG_SUSTAIN_FRAMES
        };
        // The adoption above already resolved these BY STATION - a hardware station
        // holds the engine's own block-derived allowance and deadline, EIGEN-SIM
        // holds the two named harness relaxations. Copying them here unconditionally
        // is what carried `demo_default`'s Sim values onto NI.
        ecfg.jitter_frac = cfg.jitter_frac;
        ecfg.compute_budget = cfg.compute_budget;

        // The drive-to-response pipeline lag, exactly as the TWR player derives it
        // at arm: the output lookahead the loop itself prefilled with zeros above,
        // plus the device's reported input group delay. See the field doc.
        let pipeline_lag_frames = backend
            .caps()
            .output_buffer
            .map(|c| c.min_lookahead_frames as usize)
            .unwrap_or(0)
            + actual.group_delay_frames.unwrap_or(0) as usize;

        Ok(PlantLoop {
            block_observer: None,
            last_boundary: None,
            output_primed: false,
            carry_queue: Vec::new(),
            backend,
            window_len: cfg.window_len() as u64,
            table,
            ecfg,
            n_ch,
            station,
            adoption,
            pipeline_lag_frames,
        })
    }

    /// Install (or clear) the per-tick DISPLAY tap. Takes effect on the
    /// next segment; the control path is unaffected either way.
    /// Declare that the drive was INTERRUPTED, so the next segment must re-prime
    /// the device instead of assuming a continuous seam.
    ///
    /// Suppresses the per-segment device prefill and carries the previous
    /// segment's undelivered drive across the boundary. That is right for a
    /// back-to-back seam - the AO still holds the last segment's tail, and
    /// re-priming would write a lookahead of SILENCE into it every 1.4 s, which
    /// is the dropout this all started with.
    ///
    /// It is WRONG across a genuine hold, where the AO has drained and the NI
    /// task has stopped. The carry is dropped with it: drive synthesized before
    /// an interruption is stale by the time the run resumes.
    pub fn mark_drive_discontinuity(&mut self) {
        self.output_primed = false;
        self.carry_queue.clear();
    }

    /// How many frames of synthesized LEAD-IN the next segment should bring if
    /// the output is COLD. Zero on a warm seam - synthesizing a
    /// lead-in that never plays would advance the COLA stream and splice the
    /// record - and zero when the backend declares no output contract (Sim),
    /// which keeps the Sim byte-identical to the pre-fix behavior. Three
    /// lookaheads: the device buffer holds up to ~1.5x lookahead and the rest
    /// seeds the delivery queue's low-water reserve, so the first process-
    /// produced frame is not needed until well after the first AI block lands.
    pub fn cold_leadin_frames(&self) -> usize {
        if self.output_primed {
            return 0;
        }
        let caps = self.backend.caps();
        if caps.max_output_channels != 1 {
            return 0;
        }
        caps.output_buffer
            .map(|c| 3 * c.min_lookahead_frames as usize)
            .unwrap_or(0)
    }

    /// Deliberately stop the device tasks for an operator-length hold (the
    /// operator review hold, the coherence hold).
    ///
    /// A hold is unbounded: with the tasks left Running, the AO drains its
    /// queue past the lookahead and halts (a non-regenerating output does not
    /// loop), and the AI fills its buffer with samples nobody reads until the
    /// driver kills the read (`-200279`, measured 11 ms after Proceed on
    /// 2026-08-02). Stopping BOTH tasks at hold entry removes both failure
    /// modes at their source.
    ///
    /// Resume needs no special verb: the HAL accepts `arm()` from Stopped, and
    /// `run_segment`'s cold-path prologue loads the head of the NEXT segment's
    /// real drive before `start()`, so the restart's first clocked
    /// sample is drive - `start()`'s silence top-up remains only the safety
    /// net. The software queue is reset with
    /// [`Self::mark_drive_discontinuity`] so the engine side re-primes too.
    ///
    /// CALLED at the sysID -> engage seam, the coherence hold, and the review
    /// hold. (An earlier doc here claimed engage did NOT suspend; the
    /// 2026-08-03 backlog fix made engage a full suspend/resume and the doc
    /// outlived the byte - corrected 2026-08-08.)
    pub fn suspend(&mut self) {
        let _ = self.backend.stop();
        self.mark_drive_discontinuity();
    }

    pub fn set_block_observer(&mut self, o: Option<BlockObserver>) {
        self.block_observer = o;
    }

    /// Drive-to-response pipeline lag, frames. See the field doc; consumers that
    /// correlate the drive SIGNAL against the response SIGNAL must align by this.
    pub fn pipeline_lag_frames(&self) -> usize {
        self.pipeline_lag_frames
    }

    /// What [`LoopConfig::adopt_station_actual`] changed when this loop opened.
    pub fn adoption(&self) -> &StationAdoption {
        &self.adoption
    }

    /// The number of enabled non-drive input channels (interleave width).
    pub fn n_input_channels(&self) -> usize {
        self.n_ch
    }

    /// The engine ticks one segment spans, at the configured block cadence. The
    /// segment is `window_len` SAMPLES; at a block of N frames that is
    /// `ceil(window_len / N)` ticks. (At the EIGEN-SIM natural block of 1 the two
    /// numbers coincide, which is exactly why that path keeps it.)
    fn segment_ticks(&self) -> u64 {
        let fpt = self.ecfg.frames_per_tick.max(1) as u64;
        self.window_len.div_ceil(fpt)
    }

    /// What the last segment boundary's arm/start actually did.
    pub fn last_boundary(&self) -> Option<BoundaryTrace> {
        self.last_boundary
    }

    /// The station class this loop armed against.
    pub fn station(&self) -> eigen_module_abi::StationClass {
        self.station
    }

    /// Run ONE segment of `window_len` samples through the real engine at the
    /// configured block cadence, playing back `drive`.
    ///
    /// ## The `SchedulingFault` rule
    ///
    /// This used to retry a `SchedulingFault` up to EIGHT times and then, if all
    /// eight failed, run a ninth and keep whatever came out. Two things were wrong
    /// with that, and the second was "wrong and potentially unsafe on real
    /// hardware":
    ///
    /// 1. **It retried segments that had made PROGRESS.** A retry re-runs the same
    ///    drive record from the top, but the plant is PERSISTENT - the backend, its
    ///    resonator state and its noise stream all carry forward. Replaying drive
    ///    into an already-advanced plant is not a clean re-run of the segment, it
    ///    is a different experiment wearing the first one's name.
    /// 2. **It applied on every station.** On hardware a `SchedulingFault` means
    ///    the host could not compute the drive on time WHILE A SHAKER WAS MOVING,
    ///    and the engine has already called `emergency_stop()` by the time it is
    ///    reported. Silently re-arming and re-driving is the opposite of the
    ///    correct response.
    ///
    /// The rule now: ONE retry, and only when the segment made ZERO progress
    /// (`frames_in == 0` - the only case where a re-run is genuinely a re-run) AND
    /// the station is `EigenSim` (the deterministic-harness wall-clock artifact the
    /// retry was built for: the harness runs the engine flat out and then stops for
    /// the control DSP, so a single OS hiccup can exceed a ~122 us tick budget on
    /// work that genuinely takes nanoseconds). On any HARDWARE station, and on a
    /// PROGRESSING fault anywhere, the fault SURFACES through
    /// [`SegmentResult::faulted`] and the service turns it into an error.
    pub fn run_segment(&mut self, leadin: Vec<f64>, drive: Vec<f64>) -> SegmentResult {
        let ticks = self.segment_ticks();
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            // `run_ticks`, NOT `run`: a Random run is ONE continuous acquisition
            // that happens to be scored in segments. `run`'s epilogue stops the
            // backend at every segment boundary, and on an NI station the
            // re-arm+start below does NOT bring the AO back - `output_space()`
            // stays 0 and the very next segment dies on the engine's stall
            // deadman ("9216 frames staged and unaccepted"). Measured on the
            // real cDAQ-9178 2026-07-30: segment 0 clean, segment 1 faulted.
            // The recorder hit this identically and was fixed the same way;
            // Random was the remaining caller of the stopping form.
            // The re-arm/start guards stay as a no-op safety net for a backend
            // that DID stop for its own reasons.
            // BOUNDARY CONTRACT (updated 2026-08-08):
            //   * WARM seam (`output_primed`): arm/start are expected to REFUSE
            //     mid-run - the tasks stay Running across segments. An Ok here
            //     means the backend stopped for its own reasons.
            //   * COLD start (`!output_primed`: segment 0, and every resume
            //     after `suspend()` - the engage seam, the coherence hold, the
            //     review hold): `start()` is expected to SUCCEED (`arm()`
            //     succeeds after a suspend and refuses harmlessly on segment 0,
            //     where `open()` already armed), and the device buffer is
            //     loaded with the head of THIS segment's real drive BEFORE
            //     start, so the first clocked sample is drive.
            //     `NiBackend::start`'s own silence top-up then finds
            //     `queued >= want` and adds nothing; it remains the safety net
            //     for a short record or a caller with no drive in hand.
            let arm_r = self.backend.arm();
            let cold = !self.output_primed;
            let mut prefilled_frames = 0usize;
            if cold && !leadin.is_empty() {
                // Load the LEAD-IN's head into the device before the clock
                // starts: write until the device stops accepting (its true
                // buffer depth, not the lookahead - the deeper the runway, the
                // more margin the first delivering tick has) or the head cap
                // is reached. The cap holds back at least one lookahead of
                // lead-in for the delivery queue's reserve below - a device
                // roomy enough to swallow the whole lead-in (the Sim) would
                // otherwise leave the reserve EMPTY, and `adopt_queue` keeps
                // its zero priming on empty input: the silence sandwich back
                // through a corner case (caught by the unit test's wire
                // check, 2026-08-08).
                let hold_back = self
                    .backend
                    .caps()
                    .output_buffer
                    .map(|c| c.min_lookahead_frames as usize)
                    .unwrap_or(0);
                let head_cap = leadin.len().saturating_sub(hold_back);
                let head: Vec<f32> = leadin[..head_cap].iter().map(|&v| v as f32).collect();
                while prefilled_frames < head.len() {
                    match self.backend.write_drive(&head[prefilled_frames..]) {
                        Ok(0) | Err(_) => break, // start()'s top-up decides
                        Ok(frames) => prefilled_frames += frames,
                    }
                }
            }
            let start_r = self.backend.start();
            let space_before = self.backend.output_space();
            if cold {
                // TEMP-DIAG (temporary cold-start diagnostic): the one number that
                // discriminates "prefill landed" from "prefill refused".
                eprintln!(
                    "cold prologue: leadin={} prefilled={prefilled_frames} arm_ok={} \
                     start_ok={} space_after_start={space_before:?} drive_len={}",
                    leadin.len(),
                    arm_r.is_ok(),
                    start_r.is_ok(),
                    drive.len()
                );
            }
            // A FRESH Engine per segment, deliberately: its health budget
            // measures per-tick period jitter, and the inter-segment control
            // math is a ~7 ms pause the budget must not see. Persisting one
            // engine across segments was tried and REVERTED - the run faulted
            // `SchedulingFault(PeriodJitter)` on NI-REAL within 128 frames,
            // because the boundary gap became jitter inside a continuous
            // timing baseline.
            //
            // But a fresh engine re-runs two PER-RUN primings, and both put
            // silence on the wire at every boundary:
            //   * `prefill_output` writes a lookahead of ZEROS to the device to
            //     satisfy a non-regenerating output's start precondition;
            //   * `Engine::new` primes the delivery queue with a lookahead of
            //     ZERO frames so the first ticks are covered.
            // The first is suppressed by `mark_output_prefilled`; the second by
            // ADOPTING the previous segment's undelivered queue in place of the
            // zeros, which also carries the real drive across the seam instead
            // of merely not-zeroing it.
            let mut engine = Engine::new(clone_cfg(&self.ecfg), &self.table, &self.backend);
            if self.output_primed {
                engine.mark_output_prefilled();
                engine.adopt_output_queue(std::mem::take(&mut self.carry_queue));
            } else if prefilled_frames > 0 {
                // COLD start with the lead-in's head already in the device
                // (the cold-start fix): suppress the engine's lazy zero prefill (mark),
                // and replace the delivery queue's ZERO priming with the
                // lead-in's REMAINDER. The queue keeps a PERMANENT low-water
                // reserve (`next_frame` enters the underrun policy the moment
                // it is not ABOVE `min_lookahead`), so the reserve cannot be
                // simply drained - it must exist, and zeros there would be
                // delivered right behind the head. FIFO order then plays
                // [device: leadin head] [queue: leadin rest] [step: record] -
                // one continuous synthesis stream, and the RECORD is untouched
                // (full length, no cursor games, the pairing and tick budget
                // never learn the lead-in existed). A cold path where the
                // write did not land keeps the engine-managed zero behavior.
                engine.mark_output_prefilled();
                let reserve: Vec<Vec<f32>> = leadin
                    .iter()
                    .skip(prefilled_frames)
                    .map(|&v| vec![v as f32])
                    .collect();
                engine.adopt_output_queue(reserve);
            }
            self.output_primed = true;
            let mut step = PlaybackStep::new(drive.clone(), self.n_ch)
                .with_observer(self.block_observer.clone());
            let mut sink = VecCaptureSink::new();
            let report = engine.run_ticks(&mut self.backend, &mut step, &mut sink, ticks);
            // AO free space right after the last write of the segment.
            // `output_space` is FREE room, so a LARGE value means the queue is
            // nearly EMPTY - i.e. the drive about to play is nearly exhausted
            // and the boundary silence is a genuine drain, not a restart.
            let space_after = self.backend.output_space();
            self.last_boundary = Some(BoundaryTrace {
                armed_ok: arm_r.is_ok(),
                started_ok: start_r.is_ok(),
                space_before,
                space_after,
            });

            if let StopReason::SchedulingFault(status) = report.reason {
                if scheduling_fault_retry_allowed(self.station, report.frames_in, attempt) {
                    continue;
                }
                return SegmentResult {
                    interleaved: step.interleaved,
                    n_ch: self.n_ch,
                    drive_peak: step.drive_peak,
                    // A scheduling fault is not a limit breach.
                    watchdog_trip: None,
                    faulted: Some(format!(
                        "scheduling fault ({status:?}) on station {} after {attempt} attempt(s), \
                         {} frame(s) acquired; the engine already called emergency_stop",
                        self.station, report.frames_in
                    )),
                    zero_filled: step.zero_filled,
                    driver_warnings: self.backend.take_warnings(),
                };
            }

            let watchdog_trip = match report.reason {
                StopReason::AbortTripped(t) => Some(t),
                _ => None,
            };
            // Carry whatever real drive the engine had not yet delivered into
            // the next segment.
            self.carry_queue = engine.take_output_queue();
            let faulted = fault_message(&report.reason).map(|f| {
                // A fault message that does not say what the boundary did is a
                // fault nobody can attribute (2026-08-03: three deploy cycles
                // were spent guessing whether arm/start restarted the tasks).
                format!(
                    "{f} [boundary: armed_ok={} started_ok={} space_before={:?} \
                     space_after={:?} frames_in={}]",
                    arm_r.is_ok(),
                    start_r.is_ok(),
                    space_before,
                    self.backend.output_space(),
                    report.frames_in,
                )
            });
            // The DRIVER's own diagnostics for this segment (underruns,
            // late samples, coercions). Previously collected and unreachable.
            let driver_warnings = self.backend.take_warnings();
            return SegmentResult {
                interleaved: step.interleaved,
                n_ch: self.n_ch,
                drive_peak: step.drive_peak,
                watchdog_trip,
                faulted,
                zero_filled: step.zero_filled,
                driver_warnings,
            };
        }
    }
}

/// The EIGEN-SIM abort-watchdog debounce, in RAW FRAMES. Unchanged from the
/// value this module has always used; ~1 ms at the Sim plant's 65 536 Hz.
pub const SIM_WATCHDOG_SUSTAIN_FRAMES: u32 = 64;

/// The abort-watchdog debounce on a HARDWARE station, in SECONDS.
///
/// The same ~1 ms [`SIM_WATCHDOG_SUSTAIN_FRAMES`] represents at the Sim rate,
/// stated in the unit that keeps its meaning when the driver picks a different
/// sample rate. Long enough that Gaussian tails on a peaky random control signal
/// do not false-trip; short enough that a genuine breach is stopped inside one
/// block at every supported rate. See
/// `eigen_engine::watchdog_sustain_frames`.
pub const WATCHDOG_SUSTAIN_SECONDS: f64 = 0.001;

/// How many attempts a segment gets on an EIGEN-SIM station: the first one plus
/// ONE retry. Not a tunable - see [`scheduling_fault_retry_allowed`].
pub const SIM_SEGMENT_ATTEMPTS: u32 = 2;

/// THE `SchedulingFault` RETRY RULE, as a pure
/// decision so it is testable without a hardware station.
///
/// `attempts_so_far` counts the attempt that just faulted (1 on the first).
/// Retry is allowed when ALL THREE hold:
///
/// * the station is NOT hardware. On NI-SIM or NI-REAL a scheduling fault means
///   the host missed its deadline while a shaker was moving, and the engine has
///   already called `emergency_stop()`; re-arming and re-driving is the opposite
///   of the correct response.
/// * the segment made ZERO progress. The plant is persistent, so replaying the
///   drive record into an already-advanced plant is a different experiment, not a
///   re-run.
/// * this is the first attempt. One retry, not eight.
pub fn scheduling_fault_retry_allowed(
    station: eigen_module_abi::StationClass,
    frames_in: u64,
    attempts_so_far: u32,
) -> bool {
    !station.is_hardware() && frames_in == 0 && attempts_so_far < SIM_SEGMENT_ATTEMPTS
}

/// Operator-readable text for an engine stop the plant loop must NOT treat as a
/// clean segment end. `None` for a normal end and for
/// a watchdog trip (which `SegmentResult::watchdog_trip` already carries).
fn fault_message(reason: &StopReason) -> Option<String> {
    match reason {
        StopReason::InputStalled { elapsed } => {
            Some(format!("input stalled: no frames for {elapsed:?}"))
        }
        StopReason::RingOverrun { frames } => Some(format!(
            "raw ring overrun: {frames} frame(s) dropped with limits armed"
        )),
        StopReason::BackendError(m) => Some(format!("engine backend error: {m}")),
        StopReason::Disconnected(m) => Some(format!("device link lost mid-run: {m}")),
        StopReason::CaptureFatal(m) => Some(format!("capture fatal: {m}")),
        StopReason::DriveUnderrun(u) => Some(format!("drive underrun abort: {u:?}")),
        _ => None,
    }
}

/// A continuous COLA synthesizer for the drive stream: turns a drive PSD into a
/// window-length segment, seamless across calls (the synth state carries over).
pub struct DriveSynth {
    cola: ColaSynth,
    window_len: usize,
}

impl DriveSynth {
    pub fn new(cfg: &LoopConfig) -> Self {
        DriveSynth {
            cola: ColaSynth::with_seed(cfg.nfft, cfg.overlap, cfg.fs_hz, cfg.cola_seed),
            window_len: cfg.window_len(),
        }
    }

    /// Synthesize a LEAD-IN of `frames`: real drive played ahead of
    /// a cold start's record so the output's first clocked sample is drive.
    /// Same stream as [`Self::segment`] - call it immediately BEFORE the
    /// record's segment so the wire is one continuous synthesis.
    pub fn leadin(&mut self, frames: usize, psd: &[f64]) -> Vec<f64> {
        let mut out = Vec::with_capacity(frames);
        while out.len() < frames {
            out.extend_from_slice(&self.cola.next_frame(psd));
        }
        out.truncate(frames);
        out
    }

    /// Synthesize one window-length drive segment from a one-sided drive PSD
    /// (`nfft/2+1` bins). Seamless with the previous call.
    pub fn segment(&mut self, psd: &[f64]) -> Vec<f64> {
        let mut out = Vec::with_capacity(self.window_len);
        while out.len() < self.window_len {
            out.extend_from_slice(&self.cola.next_frame(psd));
        }
        out.truncate(self.window_len);
        out
    }
}

/// A tri-field kinematic abort, enriched with the run context (level + loop) the report
/// carries. Wraps the shared [`AbortTrip`].
#[derive(Debug, Clone, PartialEq)]
pub struct KinematicTrip {
    pub channel_index: u32,
    pub axis: String,
    pub value: f64,
    pub limit: f64,
    pub at_frame: u64,
    pub at_level_db: f64,
    pub at_loop: u64,
}

/// The thin reactive kinematic monitor (mirrors the Explorer's `AbortMonitor`, built
/// directly on `eigen_engine::limits::ChannelLimiter`). One limiter per enabled
/// non-drive channel with a tri-field limit set; a settle window suppresses derived-axis
/// (vel/disp) trips during the integrator startup transient; the accel axis is always
/// live. Latches the FIRST trip.
pub struct KinematicMonitor {
    limiters: Vec<ChannelLimiter>,
    /// The interleave slot each limiter reads.
    slots: Vec<usize>,
    n_enabled: usize,
    settle_remaining: u64,
    settle_frames: u64,
    frames_seen: u64,
    trip: Option<AbortTrip>,
}

impl KinematicMonitor {
    /// Build the monitor from a channel table. `fs_hz` is the raw telemetry sample rate,
    /// `hp_hz` the integrator high-pass corner, `sustain` the debounce,
    /// `settle_hp_constants` how many HP time-constants to hold the settle flag (derived
    /// axes suppressed) after arming.
    pub fn new(
        table: &ChannelTable,
        fs_hz: f64,
        hp_hz: f64,
        sustain: u32,
        settle_hp_constants: f64,
    ) -> Self {
        let mut limiters = Vec::new();
        let mut slots = Vec::new();
        let mut slot = 0usize;
        for ch in &table.channels {
            if !ch.enabled {
                continue;
            }
            let this_slot = slot;
            slot += 1;
            if ch.role == ChannelRole::Drive {
                continue; // outputs are not limit-monitored inputs
            }
            if !ChannelLimiter::has_limit(&ch.abort) {
                continue;
            }
            limiters.push(ChannelLimiter::new(
                ch.index, ch.abort, fs_hz, hp_hz, sustain,
            ));
            slots.push(this_slot);
        }
        // Settle window = settle_hp_constants HP time constants. One HP tc ~
        // 1/(2*pi*hp_hz) s -> * fs frames.
        let tc_frames = if hp_hz > 0.0 {
            (fs_hz / (std::f64::consts::TAU * hp_hz)).max(0.0)
        } else {
            0.0
        };
        let settle_frames = (settle_hp_constants * tc_frames).round().max(0.0) as u64;
        KinematicMonitor {
            n_enabled: slot,
            limiters,
            slots,
            settle_remaining: settle_frames,
            settle_frames,
            frames_seen: 0,
            trip: None,
        }
    }

    /// Whether the monitor watches any channel at all.
    pub fn is_active(&self) -> bool {
        !self.limiters.is_empty()
    }

    /// True while the settle window is open (derived-axis trips suppressed).
    pub fn settling(&self) -> bool {
        self.settle_remaining > 0
    }

    /// The settle window span in frames.
    pub fn settle_frames(&self) -> u64 {
        self.settle_frames
    }

    /// The latched trip, if any.
    pub fn trip(&self) -> Option<AbortTrip> {
        self.trip
    }

    /// Feed one interleaved telemetry chunk (`n_enabled` samples per frame) through the
    /// tri-field check. Latches + returns the FIRST trip. During the settle window,
    /// velocity/displacement trips are suppressed; the accel axis is always live.
    pub fn feed(&mut self, interleaved: &[f64]) -> Option<AbortTrip> {
        if self.trip.is_some() || self.n_enabled == 0 || self.limiters.is_empty() {
            return self.trip;
        }
        let frames = interleaved.len() / self.n_enabled;
        for f in 0..frames {
            self.frames_seen += 1;
            let settling = self.settle_remaining > 0;
            if settling {
                self.settle_remaining -= 1;
            }
            let base = f * self.n_enabled;
            for (lim, &slot) in self.limiters.iter_mut().zip(self.slots.iter()) {
                let accel = interleaved[base + slot];
                if let Some(t) = lim.push(accel, self.frames_seen) {
                    if settling && matches!(t.axis, AbortAxis::Velocity | AbortAxis::Displacement) {
                        continue; // suppress derived-axis trips during settle
                    }
                    self.trip = Some(t);
                    return self.trip;
                }
            }
        }
        None
    }
}
