//! [`ExplorerProcessStep`] - the HOST-SYNTHESIZED half of the Explorer control core.
//!
//! # What it is
//!
//! One [`eigen_engine::ProcessStep`], PERSISTENT across engine tick batches, that
//! generates an interactive sine drive: a phase-continuous
//! [`SweepOscillator`](eigen_dsp::sweep::SweepOscillator) whose frequency and amplitude
//! are moved by SLEW LIMITS toward whatever the operator last asked for, natively in
//! VOLTS PEAK. It computes no control law: this is
//! [`Resolution::host_open_loop()`](eigen_module_abi::Resolution::host_open_loop) -
//! the host synthesizes the drive, and nothing adapts it inside the run. The operator
//! is the loop.
//!
//! # What it deliberately does NOT do
//!
//! * **It does not drain a command mailbox.** The SERVICE drains it between tick
//!   batches and applies the result through this type's `&mut self` intents, so the tick
//!   never takes a lock it did not have to. Batches are 1-2 ticks, so an intent is live
//!   within a block period either way. (`SineProcessStep` drains in-tick because its
//!   control tick is the only thing that runs; the Explorer pump runs between batches by
//!   construction.)
//! * **It does not run analysis.** It copies the acquired block into a
//!   [`SampleRing`](crate::ring::SampleRing) and returns. See that module for why the
//!   FFT cannot live here.
//! * **It does not abort.** The engine's own `AbortWatchdog` polls the raw ring inside
//!   the tick and is the authoritative limit monitor; see [`crate::service`].
//!
//! # The two slews, and why they are multiplicative
//!
//! Frequency travels in OCTAVES per second and drive in dB per second - the natural
//! rates for their axes, and the ones a sine-test operator already thinks in. Both are
//! therefore RATIOS per frame, derived from the station's ACTUAL sample rate at
//! configure time (see [`crate::slew::SlewLimits`]). A ratio cannot lift an amplitude off
//! exactly zero, which is the arm posture, so the rise off zero starts at an explicit
//! RAMP FLOOR a fixed number of dB below the target - see
//! [`AmplitudeRamp`] for the arithmetic and for why the resulting step is not the
//! stepwise rampdown the smooth-ramp rule forbids.
//!
//! # The frequency slew governs the SWEEP. A manual retune is INSTANT.
//!
//! The two frequency movers are not the same intent and no longer share a ceiling.
//! [`ExplorerProcessStep::set_target_freq`] - the operator's click, jump, arrow key or
//! `GoToFrequency` - lands the tone on its target IN ONE FRAME.
//! [`SweepState`]'s travel still walks at `freq_oct_per_s`, and the DRIVE amplitude still
//! walks at `drive_db_per_s` in both directions. Nothing about the amplitude path changed.
//!
//! **Why that is safe, stated against the property that makes it safe.** The smoothness
//! requirement a slew exists to serve protects the DRIVE AMPLITUDE: an amplitude step is a
//! broadband impulse into an armature, which is why `slew_amplitude` and
//! [`AmplitudeRamp`] are untouched here. A FREQUENCY step is not that. The oscillator is
//! `eigen_dsp::sweep::SweepOscillator`, whose retune contract is PHASE-CONTINUOUS by
//! construction: `try_set_frequency` writes `f_hz` and NEVER touches the phase
//! accumulator, so a retune changes the per-sample phase INCREMENT from the next sample on
//! and leaves the waveform itself continuous - the sample-to-sample step across an
//! arbitrarily large retune is bounded by one sample step of the higher of the two
//! frequencies. `SineProcessStep`'s `retune` (`eigen-sine`, `service/engine.rs`) holds the
//! same contract by the same call, so both host-synthesized modules retune the same way.
//! Verified by code, not by doc: [`tests::a_retune_never_steps_the_waveform`] asserts that
//! bound on a ONE-FRAME 100 Hz -> 2 kHz retune, which is exactly the move this makes
//! ordinary.
//!
//! **Why it had to change.** A 20 % coarse click is 0.263 octaves, which at the shipped
//! 1 oct/s ceiling is ~263 ms of TRAVEL. The survey capture records nothing while the tone
//! is in motion (`ProfileTracer::observe_pipelined` queues a fresh pending tone and re-arms
//! the transient guard on every frequency change that is not a sweep), so a hand-walked
//! coarse survey at three clicks a second spent all of its time travelling and anchored few
//! or no profile points - the floor the bench observed. The reference instrument the operator is
//! comparing against (a commercial controller's Fn-sweep) retunes on the up-button
//! INSTANTLY. Travel time now collapses to one frame, leaving only the acquisition pipeline
//! (~65 ms on the NI chassis), the post-arrival transient guard and the measurement window
//! between a click
//! and its recorded point - comfortably inside a 300 ms click.

use crate::ring::SampleRing;
use crate::slew::SlewLimits;
use eigen_dsp::sweep::SweepOscillator;
use eigen_engine::{ProcessStep, TimebaseEvent};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Which way a sweep travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepDirection {
    /// Toward the band's high edge.
    Up,
    /// Toward the band's low edge.
    Down,
}

impl SweepDirection {
    /// `+1.0` up, `-1.0` down.
    fn sign(self) -> f64 {
        match self {
            SweepDirection::Up => 1.0,
            SweepDirection::Down => -1.0,
        }
    }
}

/// The live sweep state. Sweeping is a FLAG on a running step, never a run state: the
/// operator starts and stops it mid-run without the run changing phase.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepState {
    /// Travel direction.
    pub direction: SweepDirection,
    /// Sweep rate, octaves per minute (always positive; `direction` carries the sign).
    pub rate_oct_per_min: f64,
    /// True once the sweep reached the band edge in its direction. HALT-AND-DWELL: the
    /// tone parks at the edge and holds. It does NOT bounce - a bidirectional sweep is a
    /// scheduled test (that is Sine's job), whereas Explorer is an operator with a knob,
    /// and a tone that reverses on its own is a tone that moved without being asked.
    pub halted: bool,
}

/// How the commanded amplitude gets from zero to a target and back, in dB/s.
///
/// # The problem this names
///
/// The drive slew is a RATIO per frame, so it multiplies. Multiplication cannot leave
/// zero, and zero is exactly where an Explorer run arms. Something has to bridge the
/// gap between "off" and "on the ramp".
///
/// # The rule
///
/// A ramp always spans a fixed [`ramp_span_db`](Self::ramp_span_db) (default 40 dB)
/// below its ANCHOR - the larger of where the drive is and where it is going. Rising
/// from zero, the amplitude steps once to `anchor * 10^(-span/20)` and ramps the rest of
/// the way; falling to zero, it ramps down to that same floor and then steps to zero.
///
/// # Why the step is not the forbidden amplitude cliff
///
/// The step is 1 % of the anchor at the default 40 dB, and it happens at the BOTTOM of
/// the ramp, where the drive is 40 dB down. The smooth-ramp rule exists because a step INTO an
/// armature is a broadband impulse; a 1 %-of-a-small-number transition is orders of
/// magnitude below the excitation the run is already producing. The alternative - an
/// asymptote that never reaches zero - leaves the output holding a DC-ish residue after
/// a stop, which is worse in every way. The span is configurable precisely so an
/// operator who wants the step smaller can buy that with ramp time.
///
/// Stating the span in dB (rather than an absolute volts floor) also makes the ramp
/// DURATION independent of the target: 40 dB at 6 dB/s is ~6.7 s whether the target is
/// 20 mV or 2 V, so an operator's expectation transfers across levels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmplitudeRamp {
    /// dB below the ramp anchor at which the amplitude steps to/from zero.
    pub ramp_span_db: f64,
}

impl Default for AmplitudeRamp {
    fn default() -> Self {
        Self { ramp_span_db: 40.0 }
    }
}

impl AmplitudeRamp {
    /// The floor for a ramp anchored at `anchor_vpk`.
    fn floor_for(&self, anchor_vpk: f64) -> f64 {
        if anchor_vpk > 0.0 && anchor_vpk.is_finite() {
            anchor_vpk * 10f64.powf(-self.ramp_span_db.abs() / 20.0)
        } else {
            0.0
        }
    }
}

/// Everything the service needs to build an [`ExplorerProcessStep`].
///
/// Every field is stated in terms the ACTUAL configured station answered with, never the
/// request: `fs_hz` is `ConfiguredActual::actual_rate_hz`, and `band_hz` has already been
/// narrowed against it. Building the step is the LAST thing the arm path does, after the
/// rate has been adopted, so there is no window in which this type holds a rate the
/// hardware is not running at.
#[derive(Debug, Clone)]
pub struct StepBuild {
    /// The station's ACTUAL sample rate.
    pub fs_hz: f64,
    /// Drive channels in the output frame (channel 0 carries the tone; the rest are
    /// written as zeros).
    pub out_channels: usize,
    /// The clamped frequency band `[lo, hi]`, already narrowed to `0.45 * fs`.
    pub band_hz: (f64, f64),
    /// Where the oscillator starts.
    pub start_hz: f64,
    /// The hard volts-peak ceiling. Every amplitude intent is clamped to `[0, this]`.
    pub max_drive_vpk: f64,
    /// Operator slew ceilings (octaves/s, dB/s).
    pub slew: SlewLimits,
    /// The zero-crossing rule for the amplitude ramp.
    pub ramp: AmplitudeRamp,
    /// Blocks of acquisition the analysis ring holds before it drops the oldest.
    pub ring_blocks: usize,
    /// Frames per acquired block (ring pre-allocation).
    pub block_frames: usize,
    /// Interleave width of an acquired frame (ring pre-allocation).
    pub n_interleave: usize,
}

/// Why [`ExplorerProcessStep::new`] refused to build.
///
/// ## Why this is a refusal and not an assertion
///
/// The two things that can be wrong here - a rate nothing can be denominated in, and a
/// band with nothing left in it - both arrive from a DEVICE, via
/// `ConfiguredActual::actual_rate_hz`. `SweepOscillator::new` asserts on the first of
/// them, so a driver that answers `0` would panic the construction of a module. That is
/// the wire-fed-hazard shape again: rather than trusting every caller to pre-check, the
/// hazardous path is made unreachable from here.
///
/// This is defence in depth, not the primary gate: the arm gate's rate policy refuses an
/// unusable rate before this constructor is reached (see
/// `ExplorerHostService::arm`). Found by RUNNING the `Some(fs)` -> `None` red mutation
/// on 2026-08-08 - with the gate's rate check removed, arm reached this constructor and
/// panicked inside `eigen-dsp` instead of refusing. The gate was restored AND this
/// refusal added, because a safety layer whose failure mode is a panic three crates down
/// is one whose failure mode nobody has looked at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StepBuildError {
    /// The sample rate is not a positive finite number, so nothing - not the phase
    /// increment, not the slew increments, not a bin width - can be derived from it.
    UnusableRate { fs_hz: f64 },
    /// After narrowing to the rate, no usable band remains.
    BandEmpty { fs_hz: f64, band_hz: (f64, f64) },
}

impl std::fmt::Display for StepBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StepBuildError::UnusableRate { fs_hz } => write!(
                f,
                "the station reported a sample rate of {fs_hz} Hz, which is not a positive \
                 finite rate: no oscillator increment, slew increment or bin width can be \
                 denominated in it"
            ),
            StepBuildError::BandEmpty { fs_hz, band_hz } => write!(
                f,
                "no usable band remains: {}..{} Hz narrowed against {fs_hz} Hz leaves nothing \
                 to sweep",
                band_hz.0, band_hz.1
            ),
        }
    }
}

impl std::error::Error for StepBuildError {}

/// The shared handles the service keeps to observe a running step.
#[derive(Clone)]
pub struct StepHandles {
    /// The acquisition tap the pump drains into the analysis engines.
    pub ring: Arc<SampleRing>,
    /// Frames the step has generated drive for, since arm. A RUN-RELATIVE clock: it
    /// starts at zero for this run rather than inheriting a device counter, so a
    /// "seconds into the run" readout means what it says across a re-arm.
    pub frames_generated: Arc<AtomicU64>,
}

/// The persistent Explorer host control step. See the module docs.
pub struct ExplorerProcessStep {
    // --- adopted-at-configure immutables --------------------------------------
    fs_hz: f64,
    out_channels: usize,
    band_hz: (f64, f64),
    max_drive_vpk: f64,
    slew: SlewLimits,
    ramp: AmplitudeRamp,
    /// `2^(oct/frame)` at the ACTUAL rate: the per-frame frequency travel ceiling.
    freq_factor_per_frame: f64,
    /// `10^(dB/frame / 20)` at the ACTUAL rate: the per-frame drive travel ceiling.
    drive_factor_per_frame: f64,

    // --- live state -----------------------------------------------------------
    nco: SweepOscillator,
    /// Where the tone IS (the slewed, generated frequency).
    freq_hz: f64,
    /// Where the tone is GOING (the operator's / sweep's intent).
    target_freq_hz: f64,
    /// A MANUAL retune is waiting to be applied, and it is applied in ONE FRAME.
    ///
    /// Set only by [`ExplorerProcessStep::set_target_freq`], i.e. only by an operator
    /// intent; [`ExplorerProcessStep::advance_sweep`] moves `target_freq_hz` directly and
    /// deliberately does NOT set it, which is what keeps the sweep on its oct/s ceiling
    /// while a click is instant. Cleared by the frame that lands the tone.
    ///
    /// A FLAG rather than a second slew rate: an "instant enough" ceiling (say 100 oct/s)
    /// would still leave a travel window a few milliseconds wide, and an analysis pass
    /// whose boundary fell inside it would see - and queue a survey point against - an
    /// intermediate frequency the operator never asked for. One frame cannot straddle a
    /// pass boundary, so the intermediate state does not exist to be observed.
    jump_pending: bool,
    /// The amplitude being generated right now, volts peak.
    vpk: f64,
    /// The amplitude the operator asked for, volts peak.
    target_vpk: f64,
    /// The floor of the ramp currently in progress (recomputed when the target moves).
    ramp_floor_vpk: f64,
    sweep: Option<SweepState>,
    /// Sweep travel per frame as an octave count (derived from the rate and the ACTUAL
    /// sample rate; recomputed when the sweep changes).
    sweep_oct_per_frame: f64,

    // --- shared surfaces ------------------------------------------------------
    ring: Arc<SampleRing>,
    frames_generated: Arc<AtomicU64>,
}

impl ExplorerProcessStep {
    /// Build the step and the handles the service observes it through.
    ///
    /// The arm posture is baked in here: **the amplitude starts at 0.000 V**, both
    /// commanded and generated. See [`crate::service::ExplorerHostService::arm`] for why
    /// that is what stands in for a review hold.
    ///
    /// Refuses rather than panics on a rate or band nothing can be derived from; see
    /// [`StepBuildError`].
    pub fn new(b: StepBuild) -> Result<(Self, StepHandles), StepBuildError> {
        let fs_hz = b.fs_hz;
        if !(fs_hz.is_finite() && fs_hz > 0.0) {
            return Err(StepBuildError::UnusableRate { fs_hz });
        }
        let band = normalize_band(b.band_hz, fs_hz);
        if !(band.0.is_finite() && band.1.is_finite() && band.1 > band.0 && band.0 > 0.0) {
            return Err(StepBuildError::BandEmpty {
                fs_hz,
                band_hz: b.band_hz,
            });
        }
        let start = b.start_hz.clamp(band.0, band.1);
        let mut nco = SweepOscillator::new(fs_hz, 0.0);
        // Cannot fail: `band` is already inside `[0, 0.45 * fs]` by construction, and
        // `start` is clamped into it. `try_` rather than the panicking form all the same
        // - this crate never names the panicking one, so no later edit can
        // reintroduce a reachable panic by copying a line from here.
        let _ = nco.try_set_frequency(start);

        let ring = Arc::new(SampleRing::new(
            b.ring_blocks,
            b.block_frames,
            b.n_interleave,
        ));
        let frames_generated = Arc::new(AtomicU64::new(0));
        let handles = StepHandles {
            ring: ring.clone(),
            frames_generated: frames_generated.clone(),
        };

        let step = ExplorerProcessStep {
            fs_hz,
            out_channels: b.out_channels.max(1),
            band_hz: band,
            max_drive_vpk: b.max_drive_vpk.max(0.0),
            slew: b.slew,
            ramp: b.ramp,
            freq_factor_per_frame: b.slew.freq_factor_per_frame(fs_hz),
            drive_factor_per_frame: b.slew.drive_factor_per_frame(fs_hz),
            nco,
            freq_hz: start,
            target_freq_hz: start,
            jump_pending: false,
            vpk: 0.0,
            target_vpk: 0.0,
            ramp_floor_vpk: 0.0,
            sweep: None,
            sweep_oct_per_frame: 0.0,
            ring,
            frames_generated,
        };
        Ok((step, handles))
    }

    /// The ACTUAL sample rate this step generates at.
    pub fn fs_hz(&self) -> f64 {
        self.fs_hz
    }

    /// The clamped frequency band `[lo, hi]` every intent is held inside.
    pub fn band_hz(&self) -> (f64, f64) {
        self.band_hz
    }

    /// The hard volts-peak ceiling.
    pub fn max_drive_vpk(&self) -> f64 {
        self.max_drive_vpk
    }

    /// The operator slew ceilings in force.
    pub fn slew(&self) -> SlewLimits {
        self.slew
    }

    /// The frequency the oscillator is generating RIGHT NOW (Hz).
    pub fn freq_hz(&self) -> f64 {
        self.freq_hz
    }

    /// The frequency the tone is travelling toward (Hz).
    pub fn target_freq_hz(&self) -> f64 {
        self.target_freq_hz
    }

    /// The amplitude being generated right now (volts peak).
    pub fn drive_vpk(&self) -> f64 {
        self.vpk
    }

    /// The amplitude the operator last commanded (volts peak).
    pub fn target_drive_vpk(&self) -> f64 {
        self.target_vpk
    }

    /// The live sweep, if one is running.
    pub fn sweep(&self) -> Option<SweepState> {
        self.sweep
    }

    /// Clamp a frequency intent into `[band_lo, band_hi]` (the band was already narrowed
    /// to `0.45 * actual_fs` at construction). A non-finite intent lands on the low edge:
    /// the safe direction, and never a NaN into the phase accumulator.
    pub fn clamp_freq(&self, hz: f64) -> f64 {
        if hz.is_finite() {
            hz.clamp(self.band_hz.0, self.band_hz.1)
        } else {
            self.band_hz.0
        }
    }

    /// Clamp an amplitude intent into `[0, max_drive_vpk]`. Non-finite lands on 0.
    pub fn clamp_vpk(&self, vpk: f64) -> f64 {
        if vpk.is_finite() {
            vpk.clamp(0.0, self.max_drive_vpk)
        } else {
            0.0
        }
    }

    /// Command a frequency BY HAND. Returns the value ACTUALLY adopted after clamping,
    /// which is what the operator surface must render (the panel shows the
    /// service's state, never its own last request).
    ///
    /// **The retune is INSTANT**: the tone lands on the clamped target within one
    /// generated frame, phase-continuously. The frequency slew is the SWEEP's ceiling and
    /// no longer paces a click - see the module docs for the property that makes a
    /// one-frame frequency step safe, and for the survey-capture defect that made the
    /// paced click wrong.
    ///
    /// Setting a frequency by hand also HALTS any running sweep - the operator took the
    /// knob, and a sweep that kept dragging the tone away from where it was just put is a
    /// control that lies.
    pub fn set_target_freq(&mut self, hz: f64) -> f64 {
        let clamped = self.clamp_freq(hz);
        self.target_freq_hz = clamped;
        self.jump_pending = true;
        if let Some(s) = self.sweep.as_mut() {
            s.halted = true;
        }
        clamped
    }

    /// Command an amplitude in volts peak. Returns the value actually adopted after
    /// clamping to `[0, max_drive_vpk]`.
    pub fn set_target_vpk(&mut self, vpk: f64) -> f64 {
        let clamped = self.clamp_vpk(vpk);
        self.target_vpk = clamped;
        // Re-anchor the ramp on the larger of where we are and where we are going, so a
        // ramp's DURATION is the same in both directions and a target changed mid-ramp
        // does not shorten the remaining travel.
        self.ramp_floor_vpk = self.ramp.floor_for(self.vpk.max(clamped));
        clamped
    }

    /// Start (or re-aim) a sweep. `rate_oct_per_min` is taken as a magnitude; the
    /// direction carries the sign. Starting a sweep clears any halt, so the operator can
    /// walk a parked tone off a band edge by sweeping the other way.
    pub fn start_sweep(&mut self, direction: SweepDirection, rate_oct_per_min: f64) {
        let rate = if rate_oct_per_min.is_finite() {
            rate_oct_per_min.abs()
        } else {
            0.0
        };
        self.sweep = Some(SweepState {
            direction,
            rate_oct_per_min: rate,
            halted: false,
        });
        // Octaves per frame at the ACTUAL rate: oct/min -> oct/s -> oct/frame.
        self.sweep_oct_per_frame = if self.fs_hz.is_finite() && self.fs_hz > 0.0 {
            rate / 60.0 / self.fs_hz
        } else {
            0.0
        };
    }

    /// Stop the sweep. The tone stays exactly where it is (taking over
    /// must never move the shaker).
    pub fn stop_sweep(&mut self) {
        self.sweep = None;
        self.sweep_oct_per_frame = 0.0;
    }

    /// The run-relative frame count this step has generated drive for.
    pub fn frames_generated(&self) -> u64 {
        self.frames_generated.load(Ordering::Relaxed)
    }

    // --- the per-frame arithmetic ----------------------------------------------

    /// Advance the sweep's TARGET one frame. The slew below then decides how much of
    /// that the tone actually gets to travel, so a sweep faster than the slew ceiling is
    /// bounded by the ceiling rather than overriding it.
    fn advance_sweep(&mut self) {
        let Some(s) = self.sweep else { return };
        if s.halted || self.sweep_oct_per_frame <= 0.0 {
            return;
        }
        let next = self.target_freq_hz * (s.direction.sign() * self.sweep_oct_per_frame).exp2();
        let (lo, hi) = self.band_hz;
        if next >= hi {
            self.target_freq_hz = hi;
            if let Some(m) = self.sweep.as_mut() {
                m.halted = true;
            }
        } else if next <= lo {
            self.target_freq_hz = lo;
            if let Some(m) = self.sweep.as_mut() {
                m.halted = true;
            }
        } else {
            self.target_freq_hz = next;
        }
    }

    /// Move the generated frequency toward its target, then RETUNE the oscillator.
    ///
    /// A MANUAL retune ([`Self::jump_pending`]) lands in this one frame. Otherwise the
    /// tone travels one frame's worth at the frequency slew - which is now the SWEEP's
    /// ceiling and nothing else's.
    ///
    /// The retune is phase-continuous by the oscillator's own contract
    /// (`SweepOscillator::try_set_frequency` writes the frequency and never the phase
    /// accumulator); this method never touches the phase, on either path.
    fn slew_frequency(&mut self) {
        let target = self.target_freq_hz;
        if (self.freq_hz - target).abs() > f64::EPSILON {
            if self.jump_pending {
                // THE OPERATOR'S CLICK. One frame, phase-continuous, no travel.
                self.freq_hz = target;
            } else {
                let f = self.freq_factor_per_frame;
                self.freq_hz = if target > self.freq_hz {
                    (self.freq_hz * f).min(target)
                } else {
                    (self.freq_hz / f).max(target)
                };
            }
            self.freq_hz = self.freq_hz.clamp(self.band_hz.0, self.band_hz.1);
            // Refused only if the band were outside `[0, fs/2)`, which construction
            // prevents. On a refusal the oscillator holds its previous tone, which is
            // the safe degradation - never a panic on the tick that owns the drive.
            let _ = self.nco.try_set_frequency(self.freq_hz);
        }
        // Cleared whether or not there was anywhere to travel: a click onto the frequency
        // the tone is ALREADY at is still spent, and a flag that outlived it would make
        // the NEXT sweep step jump.
        self.jump_pending = false;
    }

    /// Move the generated amplitude one frame's worth toward its target. See
    /// [`AmplitudeRamp`] for the zero-crossing rule.
    fn slew_amplitude(&mut self) {
        let target = self.target_vpk;
        if (self.vpk - target).abs() <= f64::EPSILON {
            return;
        }
        let f = self.drive_factor_per_frame;
        if target > self.vpk {
            if self.vpk < self.ramp_floor_vpk {
                // Lift off zero (or off a floor left by an aborted ramp) onto the ramp.
                self.vpk = self.ramp_floor_vpk.min(target);
            } else if f > 1.0 {
                self.vpk = (self.vpk * f).min(target);
            } else {
                // An unusable rate yields a factor of exactly 1.0 (see SlewLimits): no
                // travel, which is the fail-safe direction.
                self.vpk = self.vpk.min(target);
            }
        } else if f > 1.0 {
            self.vpk = (self.vpk / f).max(target);
            if target <= 0.0 && self.vpk <= self.ramp_floor_vpk {
                self.vpk = 0.0;
            }
        } else {
            // No usable slew: a DOWNWARD move still has to complete, or a stop would
            // never reach zero. Going straight to the target is the safe direction here
            // for the same reason refusing to rise is safe above.
            self.vpk = target;
        }
        self.vpk = self.vpk.clamp(0.0, self.max_drive_vpk);
    }
}

/// Narrow a requested band to what the rate can carry: `[lo, min(hi, 0.45 * fs)]`, with
/// `lo` held strictly positive and below the narrowed high edge.
///
/// `0.45 * fs` rather than `0.5 * fs`: the oscillator's own domain rule is `f < fs/2`,
/// but a tone AT Nyquist is not a tone, and the acquisition side needs bins above the
/// fundamental to be worth looking at. The 10 % standoff is what makes the commanded tone
/// resolvable in the display spectrum rather than merely representable.
fn normalize_band(band: (f64, f64), fs_hz: f64) -> (f64, f64) {
    let nyquist_guard = if fs_hz.is_finite() && fs_hz > 0.0 {
        0.45 * fs_hz
    } else {
        0.0
    };
    let hi = band.1.min(nyquist_guard).max(0.0);
    // A low edge at or above the high edge would make every clamp meaningless; pin it
    // just below, and keep it strictly positive (0 Hz is a parked oscillator, not a
    // sweepable band edge).
    let lo = band.0.max(f64::MIN_POSITIVE).min(hi);
    (lo, hi.max(lo))
}

impl ProcessStep for ExplorerProcessStep {
    fn process(&mut self, input: &[f32], n_frames: usize, drive_out: &mut Vec<f32>) {
        // 1. The acquisition tap. First, so a panic anywhere below still leaves the
        //    display with the data that arrived.
        if !input.is_empty() {
            self.ring.push(input);
        }

        // 2. Synthesis, one frame at a time.
        drive_out.clear();
        drive_out.reserve(n_frames * self.out_channels);
        for _ in 0..n_frames {
            self.advance_sweep();
            self.slew_frequency();
            self.slew_amplitude();
            let (sin, _inst_freq, _phase) = self.nco.next();
            let sample = (sin * self.vpk) as f32;
            drive_out.push(sample);
            for _ in 1..self.out_channels {
                drive_out.push(0.0);
            }
        }
        self.frames_generated
            .fetch_add(n_frames as u64, Ordering::Relaxed);
    }

    fn on_discontinuity(&mut self, _event: TimebaseEvent) {
        // Nothing framed lives here. The oscillator is a phase accumulator with no
        // transform spanning a boundary, and the analysis that DOES frame runs off the
        // tick against a ring whose blocks each carry their own contiguity. A device
        // epoch reset therefore changes nothing about what this step should generate -
        // and deliberately does NOT reset the tone, because an operator holding a
        // frequency through a re-arm expects to still be holding it.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 8_192.0;

    fn build(max_vpk: f64) -> (ExplorerProcessStep, StepHandles) {
        ExplorerProcessStep::new(StepBuild {
            fs_hz: FS,
            out_channels: 1,
            band_hz: (10.0, 2_000.0),
            start_hz: 100.0,
            max_drive_vpk: max_vpk,
            slew: SlewLimits::default(),
            ramp: AmplitudeRamp::default(),
            ring_blocks: 4,
            block_frames: 64,
            n_interleave: 1,
        })
        .expect("the fixture rate and band are usable")
    }

    /// The band narrows to `0.45 * fs` even when the caller asked for more, and every
    /// intent is REPORTED BACK at the value actually adopted.
    #[test]
    fn frequency_intents_clamp_to_the_rate_narrowed_band_and_report_back() {
        let (mut step, _h) = ExplorerProcessStep::new(StepBuild {
            fs_hz: 1_000.0,
            out_channels: 1,
            band_hz: (5.0, 10_000.0),
            start_hz: 100.0,
            max_drive_vpk: 1.0,
            slew: SlewLimits::default(),
            ramp: AmplitudeRamp::default(),
            ring_blocks: 2,
            block_frames: 16,
            n_interleave: 1,
        })
        .unwrap();
        assert_eq!(step.band_hz(), (5.0, 450.0));
        assert_eq!(step.set_target_freq(9_000.0), 450.0);
        assert_eq!(step.set_target_freq(0.1), 5.0);
        assert_eq!(step.set_target_freq(f64::NAN), 5.0);
        assert_eq!(step.set_target_freq(123.0), 123.0);
    }

    #[test]
    fn amplitude_intents_clamp_to_the_volts_ceiling() {
        let (mut step, _h) = build(0.5);
        assert_eq!(step.set_target_vpk(10.0), 0.5);
        assert_eq!(step.set_target_vpk(-1.0), 0.0);
        assert_eq!(step.set_target_vpk(f64::INFINITY), 0.0);
        assert_eq!(step.set_target_vpk(0.2), 0.2);
    }

    /// The arm posture, asserted rather than asserted-in-a-comment: nothing drives until
    /// a human commands a level.
    #[test]
    fn the_step_arms_at_exactly_zero_volts() {
        let (mut step, _h) = build(1.0);
        assert_eq!(step.drive_vpk(), 0.0);
        let mut drive = Vec::new();
        step.process(&vec![0.0f32; 256], 256, &mut drive);
        assert_eq!(drive.len(), 256);
        assert!(
            drive.iter().all(|&v| v == 0.0),
            "an armed, uncommanded Explorer must put exactly zero on the wire"
        );
    }

    /// THE retune-continuity property. Generate a tone, retune far away, and assert the
    /// waveform has no discontinuity: the sample-to-sample step across the retune is no
    /// larger than one sample step of the tone's own frequency.
    #[test]
    fn a_retune_never_steps_the_waveform() {
        let (mut step, _h) = build(1.0);
        // A slew ceiling high enough that the retune lands in ONE frame - the worst case
        // for continuity, and the one a phase-resetting oscillator would fail.
        step.slew = SlewLimits {
            freq_oct_per_s: 1.0e6,
            drive_db_per_s: 1.0e6,
        };
        step.freq_factor_per_frame = step.slew.freq_factor_per_frame(FS);
        step.drive_factor_per_frame = step.slew.drive_factor_per_frame(FS);
        step.vpk = 1.0;
        step.target_vpk = 1.0;

        let mut drive = Vec::new();
        step.set_target_freq(100.0);
        step.process(&vec![0.0f32; 512], 512, &mut drive);
        let before: Vec<f32> = drive.clone();
        step.set_target_freq(2_000.0);
        step.process(&[0.0f32; 8], 8, &mut drive);

        let last = *before.last().unwrap() as f64;
        let first = drive[0] as f64;
        // One sample step at the HIGHER of the two frequencies bounds the legal jump for
        // a phase-continuous oscillator; a phase RESET would land anywhere in [-2, 2].
        let max_step = (std::f64::consts::TAU * 2_000.0 / FS).sin() + 1e-9;
        assert!(
            (first - last).abs() <= max_step,
            "retune stepped the waveform by {} (one-sample bound {max_step})",
            (first - last).abs()
        );
    }

    /// THE FREQUENCY SLEW IS THE SWEEP'S CEILING. A sweep asked to travel far faster
    /// than the slew allows is bounded by the slew: 4 octaves at 1 oct/s cannot arrive
    /// in less than 4 seconds of frames.
    ///
    /// ~~This test used to make the same statement about a MANUAL `set_target_freq`.~~
    /// It no longer can, and that is the change: a click is instant (see
    /// [`a_manual_retune_lands_in_one_frame`]). Re-aimed at the sweep rather than
    /// deleted, because the ceiling itself is unchanged and still has to be pinned - what
    /// moved is WHICH mover it governs.
    #[test]
    fn the_frequency_slew_bounds_how_fast_a_sweep_travels() {
        let (mut step, _h) = build(1.0);
        // 600 oct/min = 10 oct/s of sweep INTENT against a 1 oct/s slew ceiling.
        step.start_sweep(SweepDirection::Up, 600.0);
        let mut drive = Vec::new();
        // Two seconds of frames: at the ceiling that is 2 octaves from the 100 Hz start.
        let n = (2.0 * FS) as usize;
        step.process(&vec![0.0f32; n], n, &mut drive);
        assert!(
            (step.freq_hz() - 400.0).abs() < 1.0,
            "2 s at the 1 oct/s ceiling from 100 Hz is 400 Hz, got {} (the sweep's own \
             10 oct/s must NOT override the ceiling)",
            step.freq_hz()
        );
    }

    /// THE INSTANT MANUAL RETUNE. A click lands the tone on its target within ONE frame
    /// at the SHIPPED 1 oct/s slew - the ceiling governs the sweep, not the knob.
    ///
    /// The number that makes this the operator's bug: 100 -> 120 Hz is a 20 % coarse
    /// click, 0.263 octaves, which at 1 oct/s used to be ~263 ms of travel. The survey
    /// capture records nothing while the tone moves, so a 3 clicks/s coarse walk anchored
    /// almost no points. RED-MUTATION: delete the `jump_pending` branch in
    /// `slew_frequency` and the first assertion fails at 100.003 Hz.
    #[test]
    fn a_manual_retune_lands_in_one_frame() {
        let (mut step, _h) = build(1.0);
        assert_eq!(step.slew().freq_oct_per_s, 1.0, "the SHIPPED ceiling");
        let mut drive = Vec::new();
        step.set_target_freq(120.0); // the 20 % coarse click
        step.process(&[0.0f32; 1], 1, &mut drive);
        assert_eq!(
            step.freq_hz(),
            120.0,
            "one frame must land the click; travel time is the defect"
        );
        // A FULL-BAND jump is the same one frame - there is no distance term left.
        step.set_target_freq(2_000.0);
        step.process(&[0.0f32; 1], 1, &mut drive);
        assert_eq!(step.freq_hz(), 2_000.0);
        step.set_target_freq(10.0);
        step.process(&[0.0f32; 1], 1, &mut drive);
        assert_eq!(step.freq_hz(), 10.0);
    }

    /// The jump is spent by the frame that lands it and does NOT leak into the sweep
    /// that follows. Without the unconditional clear in `slew_frequency`, a click
    /// followed by a sweep would let the sweep's first step arrive instantly too.
    #[test]
    fn a_click_does_not_make_the_following_sweep_instant() {
        let (mut step, _h) = build(1.0);
        let mut drive = Vec::new();
        step.set_target_freq(100.0); // a click onto where the tone already is
        step.process(&[0.0f32; 1], 1, &mut drive);
        step.start_sweep(SweepDirection::Up, 600.0); // 10 oct/s of intent
        let n = (1.0 * FS) as usize;
        step.process(&vec![0.0f32; n], n, &mut drive);
        assert!(
            (step.freq_hz() - 200.0).abs() < 1.0,
            "the sweep must still be paced by the 1 oct/s ceiling, got {}",
            step.freq_hz()
        );
    }

    /// The DRIVE slew is untouched by the frequency change: a click retunes instantly
    /// while the amplitude keeps walking at dB/s. The two axes are separate movers and
    /// this is the assertion that says so.
    #[test]
    fn an_instant_retune_leaves_the_drive_ramp_paced() {
        let (mut step, _h) = build(1.0);
        let mut drive = Vec::new();
        step.set_target_vpk(0.2);
        step.set_target_freq(1_000.0);
        // One second of frames: the tone arrived in frame 1, the drive is still climbing.
        let n = FS as usize;
        step.process(&vec![0.0f32; n], n, &mut drive);
        assert_eq!(step.freq_hz(), 1_000.0);
        assert!(
            step.drive_vpk() < 0.2,
            "6 dB/s cannot cross the 40 dB ramp span in 1 s (at {})",
            step.drive_vpk()
        );
    }

    /// The drive slew is the same kind of ceiling, in dB/s, and the ramp off zero starts
    /// at the stated span below the target rather than at zero.
    #[test]
    fn the_drive_ramp_starts_a_stated_span_below_the_target_and_climbs_at_the_slew() {
        let (mut step, _h) = build(1.0);
        step.set_target_vpk(0.2);
        let mut drive = Vec::new();
        step.process(&[0.0f32; 1], 1, &mut drive);
        // One frame in: on the ramp floor, 40 dB below 0.2 V.
        assert!(
            (step.drive_vpk() - 0.002).abs() < 1e-9,
            "{}",
            step.drive_vpk()
        );
        // 40 dB at the default 6 dB/s is 6.667 s; after 3 s we must be ~18 dB up.
        let n = (3.0 * FS) as usize;
        step.process(&vec![0.0f32; n], n, &mut drive);
        let db_up = 20.0 * (step.drive_vpk() / 0.002).log10();
        assert!((db_up - 18.0).abs() < 0.1, "climbed {db_up} dB in 3 s");
        // And it arrives.
        let n = (5.0 * FS) as usize;
        step.process(&vec![0.0f32; n], n, &mut drive);
        assert!((step.drive_vpk() - 0.2).abs() < 1e-9);
    }

    /// A stop ramps DOWN through the same span and then reaches exactly zero. The
    /// "reaches zero" half is the one a purely multiplicative ramp fails.
    #[test]
    fn a_ramp_to_zero_actually_reaches_zero() {
        let (mut step, _h) = build(1.0);
        step.vpk = 0.2;
        step.set_target_vpk(0.0);
        let mut drive = Vec::new();
        let n = (8.0 * FS) as usize;
        step.process(&vec![0.0f32; n], n, &mut drive);
        assert_eq!(step.drive_vpk(), 0.0);
        assert_eq!(*drive.last().unwrap(), 0.0);
    }

    /// HALT-AND-DWELL, not bounce: a sweep that reaches the edge parks there.
    #[test]
    fn a_sweep_halts_at_the_band_edge_and_does_not_bounce() {
        let (mut step, _h) = build(1.0);
        step.set_target_freq(1_000.0);
        // Let the tone arrive before sweeping, so the assertion is about the SWEEP.
        let mut drive = Vec::new();
        let n = (5.0 * FS) as usize;
        step.process(&vec![0.0f32; n], n, &mut drive);
        step.start_sweep(SweepDirection::Up, 600.0); // 10 oct/s: hits the edge fast
        let n = (2.0 * FS) as usize;
        step.process(&vec![0.0f32; n], n, &mut drive);
        assert_eq!(step.target_freq_hz(), 2_000.0);
        assert!(step.sweep().expect("still sweeping").halted);
        // Held, not reversed, over a long dwell.
        let n = (5.0 * FS) as usize;
        step.process(&vec![0.0f32; n], n, &mut drive);
        assert!((step.freq_hz() - 2_000.0).abs() < 1e-6);
    }

    /// Taking the knob halts a running sweep - the tone goes where it was put and stays.
    #[test]
    fn a_manual_frequency_halts_a_running_sweep() {
        let (mut step, _h) = build(1.0);
        step.start_sweep(SweepDirection::Up, 60.0);
        assert!(!step.sweep().unwrap().halted);
        step.set_target_freq(300.0);
        assert!(step.sweep().unwrap().halted);
        assert_eq!(step.target_freq_hz(), 300.0);
    }

    /// The step's only job on the acquisition side is to hand the block off.
    #[test]
    fn every_acquired_block_reaches_the_ring() {
        let (mut step, h) = build(1.0);
        let mut drive = Vec::new();
        step.process(&vec![1.0f32; 64], 64, &mut drive);
        step.process(&vec![2.0f32; 64], 64, &mut drive);
        let mut out = Vec::new();
        h.ring.drain_into(&mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), 64);
        assert_eq!(h.frames_generated.load(Ordering::Relaxed), 128);
    }

    /// A multi-drive-row table gets zeros on the aux rows, not a copy of the tone: only
    /// channel 0 is the Explorer drive, and a second energized output is a second thing
    /// moving that nobody commanded.
    #[test]
    fn only_channel_zero_carries_the_tone() {
        let (mut step, _h) = ExplorerProcessStep::new(StepBuild {
            fs_hz: FS,
            out_channels: 3,
            band_hz: (10.0, 2_000.0),
            start_hz: 100.0,
            max_drive_vpk: 1.0,
            slew: SlewLimits::default(),
            ramp: AmplitudeRamp::default(),
            ring_blocks: 2,
            block_frames: 16,
            n_interleave: 1,
        })
        .unwrap();
        step.vpk = 0.5;
        step.target_vpk = 0.5;
        let mut drive = Vec::new();
        step.process(&[0.0f32; 16], 16, &mut drive);
        assert_eq!(drive.len(), 48);
        assert!(drive.chunks(3).any(|f| f[0] != 0.0));
        assert!(drive.chunks(3).all(|f| f[1] == 0.0 && f[2] == 0.0));
    }
}
