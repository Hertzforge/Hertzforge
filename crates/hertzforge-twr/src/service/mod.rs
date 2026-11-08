//! The TWR run service: orchestrates prep -> feasibility -> sysid -> the gated
//! level ladder of correction iterations -> scheduled playback, with the
//! limits/abort engine wrapped around every play.
//!
//! ```text
//! Configured --advance()--> (sysid) --> Iterating --...--> Playing --> Complete
//!      |                                    |  \-- gate-fail policy --> Holding/Aborted
//!      \-- feasibility gate --> FeasibilityBlocked (override-ack unblocks)
//! ```
//!
//! [`TwrService::advance`] performs exactly ONE unit of work (the sysid pass,
//! one correction iteration, or one playback play) so a host thread can
//! interleave operator commands between units (the shock-service pacing shape).
//! Everything the law needs is computed here once and reused: the padded grid,
//! the full-level target spectrum, the interpolated inverse, and the trust mask
//! (level-invariant: error percent and the mask's relative floor both scale out
//! the level).

mod closed;
pub mod config;
pub mod plays;
pub mod report;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use hertzforge_capture::{save_snapshot, CaptureConfig, CaptureSink, Mark, Snapshot};
use hertzforge_dsp::fft::Cplx;
use hertzforge_shock_common::law_guard::{
    LawCall, LawClock, LawFault, LawRecordRing, MissCounter, MonotonicClock, OutputRule,
};
use serde::{Deserialize, Serialize};

use crate::builtin::TwrCorrectFactory;
use crate::control::{
    PlayInputs, PlayKind, TwrControl, TwrLawFactory, TwrLawSetup, TwrPlant, Verdict,
    BIN_RISE_FLOOR_DB,
};
use crate::grid::{irfft_half, rfft_half, RecordGrid};
use crate::inverse::GridInverse;
use crate::law;
use crate::limits::{FeasibilityReport, LimitTrip};
use crate::metrics::{ConvergenceHistory, ConvergenceRecord, ErrorMetrics};
use crate::player::{PlayWatch, ReferencePlayer, TwrPathCapabilities};
use crate::reference::ReferenceWaveform;
use crate::schedule::{Section, TwrLadderState, TwrRunMode};
use crate::sysid::SysIdResult;
use crate::trip::{TripKind, TwrTrip};
use crate::TwrError;

pub use closed::{TwrCallPayload, TwrCallRecord};
pub use config::TwrConfig;
pub use plays::{PlayLawCall, PlayRow};

/// The service state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TwrState {
    /// Feasibility hard-block: a kinematic ceiling failed; override-ack unblocks.
    FeasibilityBlocked,
    /// Configured and feasible; sysid not yet run.
    Configured,
    /// The identification passed and nothing plays until
    /// [`TwrService::proceed`] builds the control law and seeds it. Only a run
    /// under a changeable control law (`control` set) holds here; without one
    /// the identification seeds the drive and the run goes on iterating.
    Review,
    /// Correcting at a ladder level.
    Iterating,
    /// A level exhausted its iteration budget under the Hold policy; the
    /// operator must step, jump, relax the gate, or abort.
    Holding,
    /// SysID came back below the coherence floor. A REAL barrier, releasable
    /// only by [`TwrService::reset`].
    ///
    /// Not the same park as [`TwrState::Holding`], and the difference is the
    /// whole point: `Holding` is a gate-fail park that the level verbs release
    /// through `unhold()`, whereas a failed identification means the plant is
    /// not known, so nothing downstream of it may run. Before this state
    /// existed a coherence failure left the run `Configured` - the state that
    /// means "ready to run sysID" - so the very next `advance()` re-drove the
    /// shaker, and the panel showed no hold, no barrier, and no reason the
    /// operator was being allowed to do it.
    CoherenceHold,
    /// Full level converged; the run schedule is playing.
    Playing,
    /// The schedule finished cleanly.
    Complete,
    /// A guard tripped; see the abort reason.
    Aborted,
    /// Operator stop.
    Stopped,
}

/// What one [`TwrService::advance`] call did.
#[derive(Debug, Clone, PartialEq)]
pub enum Advanced {
    /// The sysid pass ran (excite + estimate + invert + initial drive).
    SysId { in_band_mean_coherence: f64 },
    /// One correction iteration ran.
    Iteration(ConvergenceRecord),
    /// One scheduled playback play ran.
    Play { plays_completed: u32 },
    /// The run reached a terminal state during this call.
    Terminal(TwrState),
}

/// The pollable status snapshot (UI surface).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TwrStatus {
    pub state: TwrState,
    /// Which delivery path, on which station: `host delivery on <station>`.
    pub path_label: String,
    pub level_db: f64,
    pub level_index: usize,
    /// The table level the run is on; `None` while a manual trim has moved it
    /// off the table's levels.
    pub rung: Option<usize>,
    pub iterations_at_level: u32,
    pub total_iterations: usize,
    pub converged_at_level: bool,
    pub ladder_completed: bool,
    pub plays_completed: u32,
    pub plays_target: u32,
    pub last_metrics: Option<ErrorMetrics>,
    pub sysid_mean_coherence: Option<f64>,
    /// Fraction of target spectral energy the trust mask covers (permanent
    /// error floor = 100*sqrt(1 - coverage) percent).
    pub mask_energy_coverage: Option<f64>,
    pub alarm_active: bool,
    /// Fraction of the LAST realized drive record's samples that the clip
    /// flattened, `0.0..=1.0`.
    ///
    /// It is on status rather than only in the convergence table because a
    /// clipped run does not look like a failing one from anywhere else: the law
    /// corrects the REALIZED clipped drive by construction, so the
    /// error plateaus instead of rising and the divergence guard - which needs
    /// `divergence_n` consecutive RISES - never fires. The run burns its
    /// iteration budget, parks at Holding, and nothing anywhere says the drive
    /// is being flattened.
    pub clipped_frac: f64,
    pub abort_reason: Option<String>,
    /// What the core ended the run on, with its numbers. `None` while the run
    /// is live and on every end the core did not trip: completion, a stop, and
    /// the operator's abort.
    pub trip: Option<TwrTrip>,
    pub feasibility_blocked: bool,
    pub adapt_frozen: bool,
    pub capturing: bool,
    pub run_clock_s: f64,
    /// Station time: the frames the station clocked over every play and its
    /// rest pad since the run armed, the identification included.
    pub station_frames: u64,
    /// `station_frames` in seconds at the rate the station runs at (the rate it
    /// adopted, once it has said; the requested rate before).
    pub station_s: f64,
    /// The rate the run was BUILT against (the prepared record's rate).
    pub requested_fs_hz: f64,
    /// The rate the device actually programmed, once it has said. `None`
    /// before the path is up. Shown alongside the request, never instead of
    /// it: the 9234 coerces a request up its own ladder, and a panel that
    /// showed one number could not say which.
    pub actual_fs_hz: Option<f64>,
}

/// Everything derived from the prepared reference (immutable per run).
struct Prepared {
    grid: RecordGrid,
    /// FULL-LEVEL target on the padded window.
    target_full: Vec<f64>,
    /// FFT of `target_full`.
    r_full: Vec<Cplx>,
    /// Section duration, seconds (the run-schedule clock unit).
    section_s: f64,
    /// The prepared (post-prep, pre-section) reference - report/UI surface.
    prepared_reference: ReferenceWaveform,
}

/// The TWR run service over any [`ReferencePlayer`].
pub struct TwrService<P: ReferencePlayer> {
    player: P,
    config: TwrConfig,
    state: TwrState,
    prepared: Prepared,
    feasibility: FeasibilityReport,
    override_ack: Option<String>,
    sysid: Option<SysIdResult>,
    inv: Option<GridInverse>,
    mask: Vec<bool>,
    /// Fraction of target spectral energy the trust mask covers (loop health).
    mask_coverage: Option<f64>,
    ladder: TwrLadderState,
    history: ConvergenceHistory,
    /// Drive spectrum at the CURRENT level.
    drive_spec: Vec<Cplx>,
    /// The last drive record actually played (post-clip).
    last_drive: Vec<f64>,
    last_achieved: Vec<f64>,
    plays_completed: u32,
    alarm_latched: bool,
    /// Clipped fraction of the last realized drive record (status surface).
    last_clipped_frac: f64,
    abort_reason: Option<String>,
    adapt_frozen: bool,
    player_armed: bool,
    events: Vec<String>,
    run_clock_s: f64,
    capture: Option<CaptureSink>,
    /// One row per play after the identification ([`PlayRow`]).
    plays: Vec<PlayRow>,
    /// Frames the station clocked over every play and pad since the run armed.
    station_frames: u64,
    /// What the core ended the run on, when it tripped.
    trip: Option<TwrTrip>,
    /// The closed loop under a changeable control law; `None` without
    /// `control`, which is the iteration the crate has always run.
    closed: Option<closed::Closed>,

    // --- the control-law seam -----------------------------------------------------
    /// The factory a closed-loop run builds its control law with, at proceed.
    /// `None` builds the built-in from the configuration's correction settings.
    law_factory: Option<Arc<dyn TwrLawFactory>>,
    /// The clock law calls are timed on.
    law_clock: Arc<dyn LawClock>,
    /// The law check's identification error: the identified gain scaled by
    /// this factor before it is inverted. `None` in every run but the check's.
    identification_scale: Option<f64>,
}

impl<P: ReferencePlayer> TwrService<P> {
    /// Build the service: validate, prepare the reference, run the feasibility
    /// gate. Ends `Configured` or `FeasibilityBlocked`.
    pub fn new(mut player: P, config: TwrConfig) -> Result<Self, TwrError> {
        let fs = player.fs_hz();
        config.validate(fs)?;

        // ---- prep pipeline (documented order) ----
        let mut r = config.reference.clone();
        if (r.fs_hz - fs).abs() > 1e-9 {
            // validate() guaranteed auto_resample is on for a mismatch.
            r = r
                .resample(fs)
                .map_err(|e| TwrError::Waveform(e.to_string()))?;
        }
        if let Some(linear) = config.prep.detrend {
            r = r.detrend(linear);
        }
        if let Some(hp) = config.prep.highpass_hz {
            r = r
                .highpass(hp)
                .map_err(|e| TwrError::Waveform(e.to_string()))?;
        }
        if (config.prep.scale - 1.0).abs() > 0.0 {
            r = r
                .scale(config.prep.scale)
                .map_err(|e| TwrError::Waveform(e.to_string()))?;
        }
        if let Some(frac) = config.prep.taper_frac {
            r = r
                .taper(frac)
                .map_err(|e| TwrError::Waveform(e.to_string()))?;
        }

        // ---- section + grid ----
        let section = config
            .section
            .unwrap_or_else(|| Section::all(r.samples.len()));
        section.validate(r.samples.len())?;
        let sec: Vec<f64> = r.samples[section.start..section.start + section.len].to_vec();
        let grid = RecordGrid::new(fs, section.len, config.pad_pre, config.pad_post);
        let target_full = grid.window_from_section(&sec);
        let r_full = rfft_half(&target_full, grid.n);
        let section_s = section.len as f64 / fs;

        // ---- feasibility (full-level kinematics vs ceilings) ----
        let kin = ReferenceWaveform::from_samples("section", fs, sec)
            .map_err(|e| TwrError::Waveform(e.to_string()))?
            .kinematics(config.kinematics_hp_hz);
        let feasibility = config.limits.feasibility.assess(&kin);
        let blocked = feasibility.blocked;

        // THE RATE THE PREPARED RECORD CARRIES, stated to the player ALWAYS.
        //
        // `arm_gate` skips its rate check entirely on `None`, and TWR is the one
        // module where the reference record is a genuine SECOND rate authority -
        // a sampled waveform played at another rate rescales the whole test in
        // frequency. Leaving it unstated made the gate vacuously green on the
        // chassis, which is the shape that declares a mode on a sweep that could
        // not have failed. The record was prepared against
        // `player.fs_hz()`, the REQUESTED rate (the rate the player asks the
        // device for at arm, the run's own when the caller stated one); the gate
        // compares that against what the device actually programs, and the 9234
        // coerces. So the record, its section and the grid are all at one rate,
        // and a device that adopts another is refused rather than re-gridded.
        player.set_reference_rate_hz(r.fs_hz);

        let ladder = TwrLadderState::new(config.schedule.clone())?;
        let config_has_control = config.control.is_some();
        let path_caps = TwrPathCapabilities::for_path(player.path());
        if !path_caps.response_observable {
            return Err(TwrError::Config(
                "the selected delivery path cannot observe the response; the TWR loop cannot \
                 close (see TwrPathCapabilities)"
                    .into(),
            ));
        }

        let mut svc = TwrService {
            player,
            config,
            state: if blocked {
                TwrState::FeasibilityBlocked
            } else {
                TwrState::Configured
            },
            prepared: Prepared {
                grid,
                target_full,
                r_full,
                section_s,
                prepared_reference: r,
            },
            feasibility,
            override_ack: None,
            sysid: None,
            inv: None,
            mask: Vec::new(),
            mask_coverage: None,
            ladder,
            history: ConvergenceHistory::default(),
            drive_spec: Vec::new(),
            last_drive: Vec::new(),
            last_achieved: Vec::new(),
            plays_completed: 0,
            alarm_latched: false,
            last_clipped_frac: 0.0,
            abort_reason: None,
            adapt_frozen: false,
            player_armed: false,
            events: Vec::new(),
            run_clock_s: 0.0,
            capture: None,
            plays: Vec::new(),
            station_frames: 0,
            trip: None,
            closed: config_has_control.then(closed::Closed::default),
            law_factory: None,
            law_clock: Arc::new(MonotonicClock::new()),
            identification_scale: None,
        };
        if blocked {
            svc.log("feasibility hard-block: a kinematic ceiling failed");
        }
        Ok(svc)
    }

    /// Install the factory a closed-loop run builds its control law with, at
    /// proceed. Without one, proceed builds the built-in law from the
    /// configuration's correction settings.
    pub fn with_law_factory(mut self, factory: Arc<dyn TwrLawFactory>) -> Self {
        self.law_factory = Some(factory);
        self
    }

    /// Install the clock law calls are timed on (a test clock makes a law
    /// "slow" deterministically).
    pub fn with_law_clock(mut self, clock: Arc<dyn LawClock>) -> Self {
        self.law_clock = clock;
        self
    }

    /// Scale the identified gain by `db` before it is inverted, as an
    /// identification that was that many dB wrong would. The control-law
    /// check's identification-error case runs this; no run that drives
    /// hardware sets it.
    pub fn with_identification_scale_db(mut self, db: f64) -> Self {
        self.identification_scale = Some(10f64.powf(db / 20.0));
        self
    }

    /// Whether a law factory has been installed ([`Self::with_law_factory`]).
    pub fn has_law_factory(&self) -> bool {
        self.law_factory.is_some()
    }

    fn log(&mut self, msg: &str) {
        self.events
            .push(format!("[{:9.3}s] {msg}", self.run_clock_s));
    }

    /// Acknowledge and override a feasibility block (recorded verbatim in the
    /// events + report - the Random feature-4 posture).
    pub fn arm_override(&mut self, ack: &str) -> Result<(), TwrError> {
        if self.state != TwrState::FeasibilityBlocked {
            return Err(TwrError::State("no feasibility block to override".into()));
        }
        if ack.trim().is_empty() {
            return Err(TwrError::Config(
                "override acknowledgment must not be empty".into(),
            ));
        }
        self.override_ack = Some(ack.to_string());
        self.state = TwrState::Configured;
        self.log(&format!("feasibility override acknowledged: {ack:?}"));
        Ok(())
    }

    /// Bring the delivery path up for the run without playing anything: the
    /// player's arm (open, configure at the requested rate, adopt the rate the
    /// device programs, consult the arm gate, park). Idempotent once up.
    ///
    /// # Why a host calls it before the first `advance()`
    ///
    /// The identification arms the path itself when nothing has, so a host
    /// that never calls this still runs. But then the device's answer - the
    /// rate it adopted, and the gate's refusal when that rate is not the
    /// record's - first exists in the middle of a unit of work, after the host
    /// has told its operator the run is armed. Calling this as the host arms
    /// puts the refusal where the operator asked to arm, by its reason, with
    /// nothing played; and it makes the rate a host reports at arm the rate
    /// the station runs at.
    ///
    /// Refused while a feasibility block stands (nothing is opened for a run
    /// that may not arm), and outside `Configured` when the path is down.
    pub fn arm_station(&mut self) -> Result<(), TwrError> {
        if self.player_armed {
            return Ok(());
        }
        match self.state {
            TwrState::Configured => {}
            TwrState::FeasibilityBlocked => {
                return Err(TwrError::State(
                    "feasibility-blocked: override-ack or fix the reference/ceilings before the \
                     station is brought up"
                        .into(),
                ))
            }
            s => {
                return Err(TwrError::State(format!(
                    "the station is brought up from Configured only (state {s:?})"
                )))
            }
        }
        self.player.arm()?;
        self.player_armed = true;
        Ok(())
    }

    /// Perform ONE unit of work (see the module docs).
    pub fn advance(&mut self) -> Result<Advanced, TwrError> {
        let r = self.advance_unit();
        // Between plays: the law's call records go to the run folder here,
        // never inside a play.
        self.drain_law_records();
        r
    }

    fn advance_unit(&mut self) -> Result<Advanced, TwrError> {
        match self.state {
            TwrState::Configured => self.do_sysid(),
            TwrState::Iterating => self.do_iteration(),
            TwrState::Playing => self.do_play(),
            TwrState::FeasibilityBlocked => Err(TwrError::State(
                "feasibility-blocked: override-ack or fix the reference/ceilings".into(),
            )),
            TwrState::Holding => Err(TwrError::State(
                "holding after gate failure: step/jump level, or abort".into(),
            )),
            TwrState::Review => Err(TwrError::State(
                "holding for review after a passing identification: proceed builds the control \
                 law and seeds it"
                    .into(),
            )),
            TwrState::CoherenceHold => Err(TwrError::State(
                "coherence hold: system identification did not meet the coherence floor, so \
                 the plant is not known and nothing may be driven from it. Adjust the sysid \
                 drive/duration/band and RESET - the level verbs do not release this hold"
                    .into(),
            )),
            s => Ok(Advanced::Terminal(s)),
        }
    }

    /// Run `advance()` until a terminal state (tests / batch use), bounded by
    /// `max_units`.
    pub fn run_to_completion(&mut self, max_units: usize) -> Result<TwrState, TwrError> {
        for _ in 0..max_units {
            match self.state {
                TwrState::Complete | TwrState::Aborted | TwrState::Stopped => break,
                TwrState::Holding
                | TwrState::FeasibilityBlocked
                | TwrState::CoherenceHold
                | TwrState::Review => break,
                _ => {
                    self.advance()?;
                }
            }
        }
        Ok(self.state)
    }

    // ---- the three work units ----

    /// Play one record through the player, mapping an operator CANCEL to the
    /// stopped state on the way out.
    ///
    /// Without this arm the cancel path returned a raw `Err` that nothing
    /// special-cased: `self.state` stayed `Iterating`, `player_armed` stayed
    /// true, nothing called `player.stop()`, and the pump's fault cell reported
    /// the run ABORTED with a stream-error string the operator had never seen.
    /// Stop is not an abort - the operator asked, no guard tripped - so it ends
    /// `Stopped`, and the error still propagates so the unit unwinds.
    ///
    /// Station time moves here, on every outcome: whatever the station clocked
    /// for the play, its rest pad and a cancelled play's frames included.
    fn play_through(
        &mut self,
        drive: &[f64],
        watch: Option<&PlayWatch<'_>>,
    ) -> Result<Option<Vec<f64>>, TwrError> {
        let played = match watch {
            Some(w) => self.player.play_watched(drive, w),
            None => self.player.play(drive),
        };
        let fallback = match &played {
            Ok(_) => drive.len(),
            Err(TwrError::Cancelled { frames_done, .. }) => *frames_done,
            Err(_) => 0,
        };
        let clocked = self.player.last_play_frames().unwrap_or(fallback);
        self.station_frames = self.station_frames.saturating_add(clocked as u64);
        match played {
            Err(TwrError::Cancelled {
                frames_done,
                frames_expected,
            }) => {
                self.state = TwrState::Stopped;
                let _ = self.player.stop();
                self.player_armed = false;
                self.log(&format!(
                    "operator stop: play cancelled after {frames_done} of {frames_expected} \
                     frames; what the device had already committed played out under the \
                     stop taper"
                ));
                Err(TwrError::Cancelled {
                    frames_done,
                    frames_expected,
                })
            }
            other => other,
        }
    }

    fn do_sysid(&mut self) -> Result<Advanced, TwrError> {
        if !self.player_armed {
            self.player.arm()?;
            self.player_armed = true;
        }
        let fs = self.player.fs_hz();
        let mut excitation = self.config.sysid.build_excitation(fs);
        // The drive clip protects the sysid pass too.
        let clipped = self.config.limits.apply_drive_clip(&mut excitation);
        self.last_clipped_frac = if excitation.is_empty() {
            0.0
        } else {
            clipped as f64 / excitation.len() as f64
        };
        // The ranging excitation is DRIVE: the ceiling binds it too.
        if let Some(t) = self.config.limits.drive_peak_trip(&excitation) {
            return self.trip_with(t, None);
        }
        if let Some(t) = self.config.limits.drive_rms_trip(&excitation) {
            return self.trip_with(t, None);
        }
        let response = match self.play_through(&excitation, None)? {
            Some(y) => y,
            None => {
                return self.trip_with(
                    LimitTrip {
                        kind: TripKind::NoResponse,
                        message: "sysid response not observable on this delivery path".to_string(),
                    },
                    None,
                )
            }
        };
        self.run_clock_s += excitation.len() as f64 / fs;

        // Alignment provenance gate, BEFORE the estimate is built from the
        // pair: a no-peak xcorr means the player is aligning on the derived
        // fallback, and a constant misalignment is a LINEAR phase that Welch
        // coherence cannot see - the gate below would bless a wrong-phase H
        // (the 2026-07-30 collapse shape, this time with a green light on it).
        if self.player.lag_provenance() == Some(crate::player::LagSource::DerivedNoPeak) {
            self.state = TwrState::CoherenceHold;
            let _ = self.player.stop();
            self.player_armed = false;
            let msg = "sysid lag correlation found no peak above the noise floor; \
                       refusing the derived-lag fallback (reset to release, then check \
                       the response path - an unaligned H carries a phase error the \
                       coherence gate cannot see)";
            self.log(&format!("sysid coherence hold (reset to release): {msg}"));
            return Err(TwrError::Play(msg.to_string()));
        }

        let mut sysid = match self.config.sysid.estimate(&excitation, &response, fs) {
            Ok(s) => s,
            Err(e) => {
                // COHERENCE HOLD. The state goes up BEFORE the return: leaving
                // it `Configured` is what let the next `advance()` re-drive the
                // shaker with nothing announcing a barrier.
                self.state = TwrState::CoherenceHold;
                let _ = self.player.stop();
                self.player_armed = false;
                self.log(&format!("sysid coherence hold (reset to release): {e}"));
                return Err(e);
            }
        };
        let mean = sysid.in_band_mean_coherence;
        if let Some(k) = self.identification_scale {
            for c in sysid.h.iter_mut() {
                c.0 *= k;
                c.1 *= k;
            }
        }
        let inv = GridInverse::from_sysid(
            &sysid,
            &self.config.inverse,
            &self.prepared.grid,
            self.config.sysid.f_lo_hz,
            self.config.sysid.f_hi_hz,
        )?;
        // The trust mask: band + reference floor, then the coherence gate.
        let mut mask = law::build_mask(
            &self.prepared.r_full,
            &self.prepared.grid,
            &inv,
            &self.config.law,
        );
        law::apply_coherence_gate(&mut mask, &inv, self.config.inverse.coherence_floor);
        // Coverage guard: unmasked target energy is a PERMANENT error floor
        // (rms error can never fall below 100*sqrt(1 - coverage)), so a
        // reference whose content lies outside the correctable band must be
        // caught HERE, not discovered as an unexplainable gate failure later.
        let coverage = law::mask_energy_coverage(&self.prepared.r_full, &mask);
        if coverage < self.config.law.min_masked_energy_frac {
            let floor = self.config.law.min_masked_energy_frac;
            let error_floor_pct = 100.0 * (1.0 - coverage).sqrt();
            let message = format!(
                "trust mask covers only {:.1}% of the target spectral energy (floor {:.1}%): \
                 reference content is outside the correctable band (band limits, coherence, \
                 or the reference floor) - the residual error can never drop below ~{:.0}%",
                coverage * 100.0,
                floor * 100.0,
                error_floor_pct
            );
            return self.trip_with(
                LimitTrip {
                    kind: TripKind::Coverage {
                        coverage,
                        min_band_energy: floor,
                        error_floor_pct,
                    },
                    message,
                },
                None,
            );
        }
        self.mask_coverage = Some(coverage);

        if let Some(closed) = self.closed.as_mut() {
            // Under a control law the seed is the law's, at proceed: hold here.
            closed.plant = Some(closed::WorldPlant::new(
                &sysid,
                &inv,
                &mask,
                &self.prepared.grid,
            ));
            self.sysid = Some(sysid);
            self.inv = Some(inv);
            self.mask = mask;
            self.state = TwrState::Review;
            let coverage_pct = 100.0 * coverage;
            self.log(&format!(
                "sysid complete: in-band mean coherence {mean:.3}, mask covers {coverage_pct:.1}% \
                 of target energy; holding for review"
            ));
            return Ok(Advanced::SysId {
                in_band_mean_coherence: mean,
            });
        }

        // Initial drive at the CURRENT ladder level: D_0 = G * R * scale,
        // scaled DOWN by the conservative backoff (the iteration recovers it;
        // undershoot corrects upward safely, overshoot is an overtest).
        let scale = self.ladder.current_scale()
            * 10f64.powf(-self.config.law.initial_drive_backoff_db / 20.0);
        let mut d0 = law::initial_drive(&self.prepared.r_full, &inv, &mask);
        for c in d0.iter_mut() {
            c.0 *= scale;
            c.1 *= scale;
        }
        self.drive_spec = d0;
        self.sysid = Some(sysid);
        self.inv = Some(inv);
        self.mask = mask;
        self.state = TwrState::Iterating;
        let coverage_pct = 100.0 * self.mask_coverage.unwrap_or(0.0);
        self.log(&format!(
            "sysid complete: in-band mean coherence {mean:.3}, mask covers {coverage_pct:.1}% \
             of target energy; iterating at {} dB",
            self.ladder.current_level_db()
        ));
        Ok(Advanced::SysId {
            in_band_mean_coherence: mean,
        })
    }

    /// The level-scaled target window + spectrum at the current ladder level.
    fn level_target(&self) -> (Vec<f64>, Vec<Cplx>) {
        let s = self.ladder.current_scale();
        let t = self
            .prepared
            .target_full
            .iter()
            .map(|&v| v * s)
            .collect::<Vec<f64>>();
        let r = self
            .prepared
            .r_full
            .iter()
            .map(|&(re, im)| (re * s, im * s))
            .collect::<Vec<Cplx>>();
        (t, r)
    }

    /// Play the current drive once and check the limit engine. Shared by the
    /// iteration and playback units.
    ///
    /// The played drive and response land in `last_drive` and `last_achieved`
    /// whether or not the response check tripped.
    fn play_current(&mut self, target_l: &[f64]) -> Result<PlayStep, TwrError> {
        let s = self.ladder.current_scale();
        let mut drive = match self.closed_run() {
            // The full-level drive in force, scaled to the level.
            Some(run) => run.drive.iter().map(|&v| v * s).collect(),
            None => irfft_half(&self.drive_spec, self.prepared.grid.n),
        };
        let clipped = self.config.limits.apply_drive_clip(&mut drive);
        self.last_clipped_frac = if drive.is_empty() {
            0.0
        } else {
            clipped as f64 / drive.len() as f64
        };
        if clipped > 0 {
            // Clip-then-measure: the law corrects the REALIZED drive.
            match self.closed.as_mut().and_then(|c| c.run.as_mut()) {
                // The drive in force is the drive as played, at full level.
                Some(run) => run.drive = drive.iter().map(|&v| v / s).collect(),
                None => self.drive_spec = rfft_half(&drive, self.prepared.grid.n),
            }
        }
        // Under a control law every play starts and ends at rest, whatever the
        // law returned.
        if self.config.control.as_ref().is_some_and(|c| c.rest_taper) {
            closed::taper(&mut drive, &self.prepared.grid);
        }
        // THE CEILING, on the realized record and before the player sees it.
        if let Some(t) = self.config.limits.drive_peak_trip(&drive) {
            return Ok(PlayStep::Ended(t));
        }
        if let Some(t) = self.config.limits.drive_rms_trip(&drive) {
            return Ok(PlayStep::Ended(t));
        }
        let start_frame = self.station_frames;
        // Under a control law the response is watched as each block arrives.
        let target_peak = peak_abs(target_l);
        let watch = PlayWatch {
            target: target_l,
            band: self.config.limits.envelope_abort_frac * target_peak,
            debounce: self.config.limits.envelope_debounce_samples,
            peak: target_peak,
        };
        let watched = self.closed.is_some().then_some(&watch);
        let achieved = match self.play_through(&drive, watched) {
            Ok(Some(y)) => y,
            Ok(None) => {
                return Ok(PlayStep::Ended(LimitTrip {
                    kind: TripKind::NoResponse,
                    message: "response not observable on this delivery path".to_string(),
                }))
            }
            Err(TwrError::Aborted(message)) if watched.is_some() => {
                let Some(w) = self.player.take_watch_trip() else {
                    return Err(TwrError::Aborted(message));
                };
                return Ok(PlayStep::Cut(LimitTrip {
                    kind: TripKind::Envelope {
                        in_play: true,
                        worst_frac: w.worst_frac,
                        abort_frac: self.config.limits.envelope_abort_frac,
                        samples: w.samples,
                        at_sample: w.at_sample,
                    },
                    message,
                }));
            }
            Err(e) => return Err(e),
        };
        let frames = self.station_frames - start_frame;
        self.run_clock_s += drive.len() as f64 / self.player.fs_hz();

        let outcome = self.config.limits.check_response(target_l, &achieved);
        if outcome.alarm && !self.alarm_latched {
            self.alarm_latched = true;
            self.log("response alarm band entered (latched)");
        }
        if let Some(cap) = self.capture.as_ref() {
            let f32s: Vec<f32> = achieved.iter().map(|&v| v as f32).collect();
            cap.push(&f32s, f32s.len());
        }
        let y_half = rfft_half(&achieved, self.prepared.grid.n);
        // The play's own lag, fitted from its phase against its drive.
        let lag = match (
            self.closed.as_ref().and_then(|c| c.plant.as_ref()),
            self.player.lag_frames(),
        ) {
            (Some(plant), Some(id_lag)) => plant
                .fit_drift(&achieved, &drive)
                .map(|drift| (id_lag as f64 + drift, drift)),
            _ => None,
        };
        self.last_drive = drive;
        self.last_achieved = achieved;
        Ok(PlayStep::Played(Played {
            y_half,
            clipped,
            start_frame,
            frames,
            tripped: outcome.trip,
            lag,
        }))
    }

    /// Append the row of the play just played (its drive and response are
    /// `last_drive` and `last_achieved`) at the level the ladder is on now, and
    /// return its index.
    #[allow(clippy::too_many_arguments)]
    fn push_row(
        &mut self,
        played: &Played,
        kind: PlayKind,
        iteration: Option<u32>,
        metrics: ErrorMetrics,
        gated: bool,
        clamped_bins: usize,
        law: Option<PlayLawCall>,
    ) -> u64 {
        let peak = |v: &[f64]| v.iter().fold(0.0f64, |m, &x| m.max(x.abs()));
        let index = self.plays.len() as u64;
        let row = PlayRow {
            index,
            kind,
            level_db: self.ladder.current_level_db(),
            rung: self.ladder.rung(),
            iteration,
            metrics,
            gated,
            clipped_samples: played.clipped,
            clamped_bins,
            drive_vpk: peak(&self.last_drive),
            response_peak: peak(&self.last_achieved),
            lag_frames: played.lag.map(|l| l.0),
            drift_frames: played.lag.map(|l| l.1),
            start_frame: played.start_frame,
            frames: played.frames,
            at_s: self.station_seconds(played.start_frame),
            law,
        };
        self.plays.push(row);
        index
    }

    fn do_iteration(&mut self) -> Result<Advanced, TwrError> {
        let (target_l, r_l) = self.level_target();
        let played = match self.play_current(&target_l)? {
            PlayStep::Ended(t) => return self.trip_with(t, None),
            PlayStep::Cut(t) => {
                let index = self.plays.len() as u64;
                return self.trip_with(t, Some(index));
            }
            PlayStep::Played(p) => p,
        };
        // The call behind this play's drive, for its row.
        let behind = self.take_pending_call();

        let metrics = ErrorMetrics::compute(
            &target_l,
            &self.last_achieved,
            &self.last_drive,
            &r_l,
            &played.y_half,
            &self.mask,
        );
        if let Some(t) = played.tripped.clone() {
            // Played, measured and tripped: the play's row, then the end. The
            // ladder does not count it and nothing is corrected from it.
            let iteration = self.ladder.iterations_at_level().saturating_add(1);
            let index = self.push_row(
                &played,
                PlayKind::Iteration,
                Some(iteration),
                metrics,
                false,
                0,
                behind,
            );
            return self.trip_with(t, Some(index));
        }
        if let Some(t) = self.alignment_trip(&played) {
            // Misaligned: nothing is corrected from this play.
            let iteration = self.ladder.iterations_at_level().saturating_add(1);
            let index = self.push_row(
                &played,
                PlayKind::Iteration,
                Some(iteration),
                metrics,
                false,
                0,
                behind,
            );
            return self.trip_with(t, Some(index));
        }
        let iteration = self.ladder.on_iteration();
        let level_db = self.ladder.current_level_db();

        // Gate BEFORE correcting: a gated level's drive is left as-played.
        let converged = self.ladder.check_gate(metrics.rms_error_pct);
        let mut clamped_bins = 0usize;
        let mut law_trip = None;
        if !converged && !self.adapt_frozen {
            if self.closed.is_some() {
                let verdict = verdict(&metrics, converged);
                match self.call_law(
                    PlayKind::Iteration,
                    verdict,
                    played.clipped > 0,
                    played.lag.map(|l| l.0),
                ) {
                    Ok(n) => clamped_bins = n,
                    Err(t) => law_trip = Some(t),
                }
            } else {
                let inv = self.inv.as_ref().expect("iterating implies inverse");
                clamped_bins = law::correct(
                    &mut self.drive_spec,
                    &r_l,
                    &played.y_half,
                    inv,
                    &self.mask,
                    &self.config.law,
                );
            }
        }

        let record = ConvergenceRecord {
            level_db,
            iteration,
            metrics,
            clamped_bins,
            clipped_samples: played.clipped,
        };
        self.history.push(record);
        let index = self.push_row(
            &played,
            PlayKind::Iteration,
            Some(iteration),
            metrics,
            converged,
            clamped_bins,
            behind,
        );
        if let Some(t) = law_trip {
            // The play was judged and has its row; the law's misses end the run.
            return self.trip_with(t, Some(index));
        }

        // Divergence guard.
        let divergence_n = self.config.limits.divergence_n;
        if self.history.is_diverging(
            level_db,
            self.ladder.schedule().gate_rms_error_pct,
            self.config.limits.divergence_tol_frac,
            divergence_n,
        ) {
            return self.trip_with(
                LimitTrip {
                    kind: TripKind::Divergence {
                        plays: divergence_n,
                        error_pct: metrics.rms_error_pct,
                    },
                    message: format!(
                        "diverging at {level_db} dB: rms error rose {divergence_n} consecutive \
                         iterations"
                    ),
                },
                Some(index),
            );
        }

        if converged {
            self.log(&format!(
                "level {level_db} dB converged in {iteration} iteration(s) at {:.2}% rms error",
                record.metrics.rms_error_pct
            ));
            if self.ladder.schedule().mode == TwrRunMode::Auto {
                if let Some(step_db) = self.ladder.advance_level() {
                    let to_db = self.ladder.current_level_db();
                    if let Err(refused) = self.scale_drive_db(step_db, to_db) {
                        // Found on the step's exact projection: nothing played.
                        return self.trip_with(
                            LimitTrip {
                                kind: refused.kind,
                                message: format!("auto ladder advance: {}", refused.error),
                            },
                            None,
                        );
                    }
                } else if self.ladder.completed() {
                    self.state = TwrState::Playing;
                    self.plays_completed = 0;
                    self.log("full level converged; starting the run schedule");
                }
            }
            // MANUAL: hold converged at the level; the operator steps or starts
            // playback explicitly.
        } else if self.ladder.budget_exhausted() {
            let max_iterations = self.ladder.schedule().max_iterations_per_level;
            return match self.ladder.schedule().on_gate_fail {
                crate::schedule::GateFailPolicy::Abort => {
                    let gate_pct = self.ladder.schedule().gate_rms_error_pct;
                    self.trip_with(
                        LimitTrip {
                            kind: TripKind::NotConverged {
                                iterations: max_iterations,
                                error_pct: metrics.rms_error_pct,
                                gate_pct,
                            },
                            message: format!(
                                "level {level_db} dB failed to gate within {max_iterations} \
                                 iterations"
                            ),
                        },
                        Some(index),
                    )
                }
                crate::schedule::GateFailPolicy::Hold => {
                    self.state = TwrState::Holding;
                    self.log(&format!(
                        "level {level_db} dB failed to gate; holding for the operator"
                    ));
                    Ok(Advanced::Iteration(record))
                }
            };
        }
        Ok(Advanced::Iteration(record))
    }

    fn do_play(&mut self) -> Result<Advanced, TwrError> {
        let (target_l, r_l) = self.level_target();
        let played = match self.play_current(&target_l)? {
            PlayStep::Ended(t) => return self.trip_with(t, None),
            PlayStep::Cut(t) => {
                let index = self.plays.len() as u64;
                return self.trip_with(t, Some(index));
            }
            PlayStep::Played(p) => p,
        };
        let behind = self.take_pending_call();
        if let Some(t) = played.tripped.clone() {
            // Played, measured and tripped: the play's row, then the end.
            let metrics = ErrorMetrics::compute(
                &target_l,
                &self.last_achieved,
                &self.last_drive,
                &r_l,
                &played.y_half,
                &self.mask,
            );
            let index = self.push_row(&played, PlayKind::Playback, None, metrics, false, 0, behind);
            return self.trip_with(t, Some(index));
        }
        // Optional continued adaptation during playback (default OFF).
        // Keep the metrics stream alive for the UI during playback.
        let metrics = ErrorMetrics::compute(
            &target_l,
            &self.last_achieved,
            &self.last_drive,
            &r_l,
            &played.y_half,
            &self.mask,
        );
        // Judged against the same gate as the ladder, without moving it.
        let gated = metrics.rms_error_pct <= self.ladder.schedule().gate_rms_error_pct;
        if let Some(t) = self.alignment_trip(&played) {
            let index = self.push_row(&played, PlayKind::Playback, None, metrics, false, 0, behind);
            return self.trip_with(t, Some(index));
        }
        // Optional continued adaptation during playback (default OFF).
        let mut clamped_bins = 0usize;
        let mut law_trip = None;
        if self.config.adapt_during_playback && !self.adapt_frozen {
            if self.closed.is_some() {
                match self.call_law(
                    PlayKind::Playback,
                    verdict(&metrics, gated),
                    played.clipped > 0,
                    played.lag.map(|l| l.0),
                ) {
                    Ok(n) => clamped_bins = n,
                    Err(t) => law_trip = Some(t),
                }
            } else {
                let inv = self.inv.as_ref().expect("playing implies inverse");
                clamped_bins = law::correct(
                    &mut self.drive_spec,
                    &r_l,
                    &played.y_half,
                    inv,
                    &self.mask,
                    &self.config.law,
                );
            }
        }
        self.history.push(ConvergenceRecord {
            level_db: self.ladder.current_level_db(),
            iteration: 0, // 0 marks a playback play, not a correction pass
            metrics,
            clamped_bins: 0,
            clipped_samples: played.clipped,
        });
        let index = self.push_row(
            &played,
            PlayKind::Playback,
            None,
            metrics,
            gated,
            clamped_bins,
            behind,
        );
        if let Some(t) = law_trip {
            return self.trip_with(t, Some(index));
        }

        self.plays_completed += 1;
        let target = self.config.run.plays_target(self.prepared.section_s);
        if self.plays_completed >= target {
            self.state = TwrState::Complete;
            self.log(&format!(
                "run schedule complete: {} play(s) at {} dB",
                self.plays_completed,
                self.ladder.current_level_db()
            ));
            let _ = self.player.stop();
            self.player_armed = false;
            return Ok(Advanced::Terminal(TwrState::Complete));
        }
        Ok(Advanced::Play {
            plays_completed: self.plays_completed,
        })
    }

    /// End the run on a trip the core found: the trip is recorded with the play
    /// it judged and the level now, and the sentence becomes the abort reason.
    fn trip_with(&mut self, t: LimitTrip, play: Option<u64>) -> Result<Advanced, TwrError> {
        let trip = TwrTrip {
            kind: t.kind,
            play,
            at_level_db: self.ladder.current_level_db(),
        };
        self.end_with(Some(trip), t.message)
    }

    /// End the run with no trip: the operator's abort.
    fn abort_with(&mut self, reason: String) -> Result<Advanced, TwrError> {
        self.end_with(None, reason)
    }

    fn end_with(&mut self, trip: Option<TwrTrip>, reason: String) -> Result<Advanced, TwrError> {
        self.log(&format!("ABORT: {reason}"));
        self.abort_reason = Some(reason);
        self.trip = trip;
        self.state = TwrState::Aborted;
        let _ = self.player.stop();
        self.player_armed = false;
        Ok(Advanced::Terminal(TwrState::Aborted))
    }

    /// Scale the carried drive by a dB step, THE CEILING CHECKED FIRST.
    ///
    /// The check is inside this function rather than at its callers because
    /// that is where the hole was: the level verbs mutated the drive and
    /// returned `()`, so the ceiling was only consulted at the NEXT play, where
    /// it aborts. Four +1 dB clicks all reported success and the play after
    /// them threw away a measured sysID and three converged levels. Nothing
    /// moves until the step is known to fit.
    ///
    /// The projection is EXACT, not an estimate: scaling every bin by `s`
    /// scales the time record by exactly `s`, so the realized peak after the
    /// step is `peak(irfft(drive_spec)) * s` with no play required.
    ///
    /// Under a control law the drive in force is a full-level record and the
    /// level is the scale a play applies to it, so the step moves nothing: the
    /// projection is the drive in force's peak at `to_db`.
    fn scale_drive_db(&mut self, step_db: f64, to_db: f64) -> Result<(), StepRefused> {
        let s = 10f64.powf(step_db / 20.0);
        let projected = match self.closed.as_ref() {
            Some(c) => c
                .run
                .as_ref()
                .map(|run| peak_abs(&run.drive) * 10f64.powf(to_db / 20.0)),
            None if !self.drive_spec.is_empty() => {
                let realized = irfft_half(&self.drive_spec, self.prepared.grid.n);
                Some(realized.iter().fold(0.0f64, |m, &v| m.max(v.abs())) * s)
            }
            None => None,
        };
        if let Some(peak_after) = projected {
            if let Some(t) = self.config.limits.peak_value_trip(peak_after, true) {
                let reason = t.message;
                self.log(&format!("level step {step_db:+.1} dB REFUSED: {reason}"));
                return Err(StepRefused {
                    kind: t.kind,
                    error: TwrError::State(format!(
                        "level step {step_db:+.1} dB refused: {reason}. The level and the drive \
                         are unchanged"
                    )),
                });
            }
        }
        if self.closed.is_some() {
            self.log(&format!(
                "level step {step_db:+.1} dB: the full-level drive in force plays x{:.4}",
                10f64.powf(to_db / 20.0)
            ));
            return Ok(());
        }
        for c in self.drive_spec.iter_mut() {
            c.0 *= s;
            c.1 *= s;
        }
        self.log(&format!(
            "level step {step_db:+.1} dB: drive carried forward scaled x{s:.4}"
        ));
        Ok(())
    }

    // ---- operator verbs ----

    /// Manual +1 dB trim (drive carried, convergence reset).
    ///
    /// THE DRIVE MOVES FIRST, and only if it fits. A refusal leaves the ladder
    /// where it was, so the echo the operator gets back is the level actually
    /// reached.
    pub fn step_level_up(&mut self) -> Result<(), TwrError> {
        self.require_live()?;
        let to_db = self.ladder.current_level_db() + crate::schedule::MANUAL_STEP_DB;
        self.scale_drive_db(crate::schedule::MANUAL_STEP_DB, to_db)
            .map_err(|r| r.error)?;
        self.ladder.step_up();
        self.unhold();
        Ok(())
    }

    /// Manual -1 dB trim.
    pub fn step_level_down(&mut self) -> Result<(), TwrError> {
        self.require_live()?;
        let to_db = self.ladder.current_level_db() - crate::schedule::MANUAL_STEP_DB;
        self.scale_drive_db(-crate::schedule::MANUAL_STEP_DB, to_db)
            .map_err(|r| r.error)?;
        self.ladder.step_down();
        self.unhold();
        Ok(())
    }

    /// Jump to table level `i` (drive carried by the jump step).
    pub fn jump_to_level(&mut self, i: usize) -> Result<(), TwrError> {
        self.require_live()?;
        // The step is COMPUTED before it is taken, so a ceiling refusal does
        // not leave the ladder somewhere the drive never followed it to.
        let step = self.ladder.jump_step_to(i)?;
        let to_db = self.ladder.schedule().table_db[i];
        self.scale_drive_db(step, to_db).map_err(|r| r.error)?;
        let applied = self.ladder.jump_to_level(i)?;
        debug_assert!((applied - step).abs() < 1e-12, "jump step disagreed");
        self.unhold();
        Ok(())
    }

    /// Begin scheduled playback from a converged level (MANUAL flow; AUTO
    /// transitions by itself at the top of the ladder).
    pub fn begin_playback(&mut self) -> Result<(), TwrError> {
        if self.state != TwrState::Iterating && self.state != TwrState::Holding {
            return Err(TwrError::State("no iterating run to play".into()));
        }
        if !self.ladder.converged_at_level() {
            return Err(TwrError::State(
                "current level has not converged; iterate to the gate first".into(),
            ));
        }
        self.state = TwrState::Playing;
        self.plays_completed = 0;
        self.log("operator started the run schedule");
        Ok(())
    }

    /// Release the review hold after a passing identification: build the
    /// control law from the installed factory (the built-in from the
    /// configuration's correction settings when none is installed), seed it,
    /// and go on iterating at the present level.
    ///
    /// The run ends here, before anything plays, when in AUTO the core's own
    /// prediction of the full-level drive is over the ceiling (`drive_vpk`,
    /// predicted), when the law cannot be built or its seed is not usable
    /// (`law_fault`), or when the seed's predicted response peak is over
    /// `first_drive_ratio` times the target's (`first_drive`). The state says
    /// which.
    pub fn proceed(&mut self) -> Result<(), TwrError> {
        if self.state != TwrState::Review {
            return Err(TwrError::State(format!(
                "nothing to proceed from: proceed releases the review hold after a passing \
                 identification (state {:?})",
                self.state
            )));
        }
        let control = self.config.control.clone().expect("review implies control");
        let target_peak = peak_abs(&self.prepared.target_full);
        let n = self.prepared.grid.n;

        // AUTO: the core's own full-level drive against the ceiling. MANUAL
        // refuses the level move that would need it instead.
        if self.ladder.schedule().mode == TwrRunMode::Auto {
            let plant = self.closed.as_ref().and_then(|c| c.plant.as_ref());
            if let Some(Ok(full)) = plant.map(|p| p.full_level_drive(&self.prepared.target_full)) {
                if let Some(t) = self.config.limits.peak_value_trip(peak_abs(&full), true) {
                    let message =
                        format!("the predicted full-level drive is refused: {}", t.message);
                    let _ = self.trip_with(LimitTrip { message, ..t }, None);
                    return Ok(());
                }
            }
        }

        let factory = self
            .law_factory
            .clone()
            .unwrap_or_else(|| Arc::new(TwrCorrectFactory::from_law_config(&self.config.law)));
        let descriptor = factory.describe();
        let (built, budget_s) = {
            let setup = self.law_setup().expect("review implies a plant");
            (
                closed::build_law(factory.as_ref(), &setup),
                setup.call_budget_s,
            )
        };
        let mut law = match built {
            Ok(law) => law,
            Err(fault) => {
                let _ = self.trip_with(law_fault(fault, 1), None);
                return Ok(());
            }
        };
        let inputs = PlayInputs {
            play_index: 0,
            kind: PlayKind::Iteration,
            response: &[],
            drive: &[],
            judged: None,
            limited: false,
        };
        let (r, took_s) = closed::guarded_call(
            law.as_mut(),
            LawCall::Seed,
            &inputs,
            self.law_clock.as_ref(),
            budget_s,
        );
        let mut ratio = None;
        let checked = r.and_then(|out| {
            closed::check_shape(&out, n)?;
            let mut as_played = out.drive.clone();
            if control.rest_taper {
                closed::taper(&mut as_played, &self.prepared.grid);
            }
            let plant = self
                .closed
                .as_ref()
                .and_then(|c| c.plant.as_ref())
                .expect("plant");
            let predicted = plant
                .predict(&as_played)
                .map_err(|_| LawFault::BadOutput(OutputRule::WrongLength))?;
            ratio = Some(peak_abs(&predicted) / target_peak.max(f64::MIN_POSITIVE));
            Ok(out)
        });
        let mut run = closed::LawRun {
            law,
            descriptor,
            drive: Vec::new(),
            last_values: Vec::new(),
            misses: MissCounter::new(control.guards.max_misses),
            stats: closed::LawStats::default(),
            records: LawRecordRing::default(),
            pending: None,
            budget_s,
        };
        run.stats.calls = 1;
        run.stats.worst_call_s = took_s;
        let level_db = self.ladder.current_level_db();
        let (outcome, values, drive_out) = match &checked {
            Ok(out) => (None, out.values.clone(), out.drive.clone()),
            Err(f) => (Some(f.clone()), Vec::new(), Vec::new()),
        };
        run.records.push(closed::TwrCallRecord {
            pulse_index: 0,
            call: LawCall::Seed,
            miss: outcome.clone(),
            call_us: (took_s * 1e6) as u64,
            payload: closed::TwrCallPayload {
                kind: PlayKind::Iteration,
                level_db,
                rms_error_pct: None,
                peak_rise_db: None,
                bin_rise_db: None,
                predicted_peak_ratio: ratio,
                lag_frames: None,
                values: values.clone(),
                drive_out: drive_out.iter().map(|&v| v as f32).collect(),
            },
        });
        let first_drive_ratio = control.first_drive_ratio;
        let over = ratio.filter(|&r| r > first_drive_ratio);
        if let Some(c) = self.closed.as_mut() {
            c.run = Some(run);
        }
        let out = match checked {
            Ok(out) => out,
            Err(fault) => {
                // A seed miss plays nothing: there is no drive to replay.
                if let Some(run) = self.closed.as_mut().and_then(|c| c.run.as_mut()) {
                    run.misses.miss();
                    run.stats.misses = 1;
                    run.stats.refused = u64::from(matches!(fault, LawFault::BadOutput(_)));
                }
                let _ = self.trip_with(law_fault(fault, 1), None);
                return Ok(());
            }
        };
        if let Some(r) = over {
            if let Some(run) = self.closed.as_mut().and_then(|c| c.run.as_mut()) {
                run.stats.misses = 1;
                run.stats.refused = 1;
            }
            let message = format!(
                "the control law's first drive predicts a response peak {r:.2} times the \
                 target's, over its bound of {first_drive_ratio}: nothing plays"
            );
            let _ = self.trip_with(
                LimitTrip {
                    kind: TripKind::FirstDrive {
                        predicted_peak: r * target_peak,
                        target_peak,
                        ratio_limit: first_drive_ratio,
                    },
                    message,
                },
                None,
            );
            return Ok(());
        }
        let name = self
            .closed_run()
            .map(|r| r.descriptor.name.clone())
            .unwrap_or_default();
        self.log(&format!(
            "control law {name} seeded; iterating at {} dB",
            self.ladder.current_level_db()
        ));
        if let Some(run) = self.closed.as_mut().and_then(|c| c.run.as_mut()) {
            run.pending = Some(PlayLawCall {
                used: true,
                miss: None,
                call_ms: took_s * 1e3,
                peak_rise_db: None,
                bin_rise_db: None,
                predicted_peak_ratio: ratio,
                values: out.values.clone(),
            });
            run.drive = out.drive;
            run.last_values = out.values;
        }
        self.state = TwrState::Iterating;
        self.drain_law_records();
        Ok(())
    }

    /// The setup a law is built for, borrowing the prepared record and the
    /// plant. `None` before a passing identification.
    fn law_setup(&self) -> Option<TwrLawSetup<'_>> {
        let plant = self.closed.as_ref()?.plant.as_ref()?;
        let control = self.config.control.as_ref()?;
        let grid = &self.prepared.grid;
        Some(TwrLawSetup {
            fs_hz: grid.fs_hz,
            record_len: grid.n,
            band_lo_hz: self.config.law.f_lo_hz,
            band_hi_hz: self.config.law.f_hi_hz,
            reference: &self.prepared.target_full,
            section_start: grid.pad_pre,
            section_len: grid.section_len,
            plant: TwrPlant {
                frf: &plant.h,
                coherence: &plant.coherence,
                inverse: &plant.inverse,
                trusted: &plant.trusted,
                epsilon: plant.eps,
            },
            drive_ceiling_v: self.config.limits.max_drive_vpk.unwrap_or(0.0),
            call_budget_s: control.call_budget_s(grid.n as f64 / grid.fs_hz),
        })
    }

    /// Call the law after a judged play, under the guards: the play's response
    /// and its drive as played, both referred to full level, and the core's
    /// verdict. A usable drive becomes the drive in force. A late, failed,
    /// panicked or refused call is a miss: the drive in force stays, so the
    /// next play replays it, and `law_misses` misses in a row end the run.
    /// Returns the bins the law reports it held back, or the trip that ends
    /// the run.
    fn call_law(
        &mut self,
        kind: PlayKind,
        judged: Verdict,
        limited: bool,
        lag_frames: Option<f64>,
    ) -> Result<usize, LimitTrip> {
        let s = self.ladder.current_scale();
        let response: Vec<f64> = self.last_achieved.iter().map(|&v| v / s).collect();
        let drive: Vec<f64> = self.last_drive.iter().map(|&v| v / s).collect();
        let play_index = self.plays.len() as u64 + 1;
        let level_db = self.ladder.current_level_db();
        let control = self.config.control.clone().expect("a law implies control");
        let bound = TwrControl::predicted_peak_ratio(&self.config.limits);
        let target_peak = peak_abs(&self.prepared.target_full);
        let grid = &self.prepared.grid;
        let clock = Arc::clone(&self.law_clock);
        let closed = self.closed.as_mut().expect("a law implies control");
        let plant = closed.plant.as_ref().expect("a law implies a plant");
        let run = closed
            .run
            .as_mut()
            .expect("a judged play under control implies a law");
        let inputs = PlayInputs {
            play_index,
            kind,
            response: &response,
            drive: &drive,
            judged: Some(judged),
            limited,
        };
        let (r, took_s) = closed::guarded_call(
            run.law.as_mut(),
            LawCall::Iterate,
            &inputs,
            clock.as_ref(),
            run.budget_s,
        );
        run.stats.calls += 1;
        run.stats.worst_call_s = run.stats.worst_call_s.max(took_s);
        let (mut peak_db, mut bin_db, mut ratio) = (None, None, None);
        let checked = r.and_then(|out| {
            closed::check_shape(&out, grid.n)?;
            let mut candidate = out.drive.clone();
            if control.rest_taper {
                closed::taper(&mut candidate, grid);
            }
            let (pk, bin) = closed::rises(&candidate, &drive, BIN_RISE_FLOOR_DB);
            peak_db = Some(pk);
            bin_db = Some(bin);
            if pk > control.max_peak_rise_db {
                return Err(LawFault::BadOutput(OutputRule::PeakRise));
            }
            if bin > control.max_bin_rise_db {
                return Err(LawFault::BadOutput(OutputRule::BinRise));
            }
            let predicted = plant
                .predict(&candidate)
                .map_err(|_| LawFault::BadOutput(OutputRule::WrongLength))?;
            let rt = peak_abs(&predicted) / target_peak.max(f64::MIN_POSITIVE);
            ratio = Some(rt);
            if rt > bound {
                return Err(LawFault::BadOutput(OutputRule::PredictedPeak));
            }
            Ok(out)
        });
        let (miss, values, drive_out) = match &checked {
            Ok(out) => (None, out.values.clone(), out.drive.clone()),
            Err(f) => (Some(f.clone()), Vec::new(), Vec::new()),
        };
        run.records.push(closed::TwrCallRecord {
            pulse_index: play_index,
            call: LawCall::Iterate,
            miss: miss.clone(),
            call_us: (took_s * 1e6) as u64,
            payload: closed::TwrCallPayload {
                kind,
                level_db,
                rms_error_pct: Some(judged.rms_error_pct),
                peak_rise_db: peak_db,
                bin_rise_db: bin_db,
                predicted_peak_ratio: ratio,
                lag_frames,
                values: values.clone(),
                drive_out: drive_out.iter().map(|&v| v as f32).collect(),
            },
        });
        run.pending = Some(PlayLawCall {
            used: miss.is_none(),
            miss: miss.clone(),
            call_ms: took_s * 1e3,
            peak_rise_db: peak_db,
            bin_rise_db: bin_db,
            predicted_peak_ratio: ratio,
            values,
        });
        match checked {
            Ok(out) => {
                run.misses.used();
                let clamped = closed::value_of(&out.values, "clamped_bins").unwrap_or(0.0);
                run.drive = out.drive;
                run.last_values = out.values;
                Ok(clamped.max(0.0) as usize)
            }
            Err(fault) => {
                run.stats.misses += 1;
                if matches!(fault, LawFault::BadOutput(_)) {
                    run.stats.refused += 1;
                }
                let tripped = run.misses.miss();
                let in_a_row = run.misses.in_a_row();
                if tripped {
                    return Err(law_fault(fault, in_a_row));
                }
                self.log(&format!(
                    "control law call {play_index} not used ({}): the drive in force replays",
                    fault.reason()
                ));
                Ok(0)
            }
        }
    }

    /// The call behind the drive the next row's play played, taken for that
    /// row.
    fn take_pending_call(&mut self) -> Option<PlayLawCall> {
        self.closed.as_mut()?.run.as_mut()?.pending.take()
    }

    /// The alignment check on a play under a control law: its fitted lag's
    /// drift from the identification's over `alignment_frames` ends the run.
    fn alignment_trip(&self, played: &Played) -> Option<LimitTrip> {
        let control = self.config.control.as_ref()?;
        let (lag, drift) = played.lag?;
        if drift.abs() <= control.alignment_frames {
            return None;
        }
        Some(LimitTrip {
            kind: TripKind::Alignment {
                lag_frames: lag,
                identification_lag_frames: lag - drift,
                drift_frames: drift,
                limit_frames: control.alignment_frames,
            },
            message: format!(
                "the play's lag drifted {drift:+.2} frames from the identification's, over the \
                 limit of {} frames: the response is no longer aligned with its drive",
                control.alignment_frames
            ),
        })
    }

    /// Write the law's call records into the capture's run folder, between
    /// plays: `law.json` once, then `law_calls.bin` (each call's drive, `f32`)
    /// with its `law_calls.jsonl` index. Without a capture the ring keeps the
    /// most recent calls for a host to take ([`Self::take_law_records`]).
    fn drain_law_records(&mut self) {
        let Some(dir) = self.capture.as_ref().map(|c| c.run_dir().to_path_buf()) else {
            return;
        };
        let control = self.config.control.clone();
        let Some(closed) = self.closed.as_mut() else {
            return;
        };
        let Some(run) = closed.run.as_mut() else {
            return;
        };
        if !closed.descriptor_written {
            let doc = serde_json::json!({
                "law": run.descriptor,
                "guards": control.as_ref().map(|c| &c.guards),
                "call_budget_s": run.budget_s,
                "max_peak_rise_db": control.as_ref().map(|c| c.max_peak_rise_db),
                "max_bin_rise_db": control.as_ref().map(|c| c.max_bin_rise_db),
                "first_drive_ratio": control.as_ref().map(|c| c.first_drive_ratio),
                "alignment_frames": control.as_ref().map(|c| c.alignment_frames),
                "rest_taper": control.as_ref().map(|c| c.rest_taper),
            });
            let text = serde_json::to_string_pretty(&doc).unwrap_or_default();
            closed.descriptor_written = std::fs::write(dir.join("law.json"), text).is_ok();
        }
        for r in run.records.drain() {
            let mut index = serde_json::Map::new();
            index.insert("play_index".into(), r.pulse_index.into());
            index.insert(
                "call".into(),
                serde_json::to_value(r.call).unwrap_or_default(),
            );
            index.insert("used".into(), r.miss.is_none().into());
            index.insert("miss".into(), r.miss.as_ref().map(|f| f.reason()).into());
            index.insert("call_us".into(), r.call_us.into());
            let p = &r.payload;
            index.insert("kind".into(), p.kind.as_str().into());
            index.insert("level_db".into(), p.level_db.into());
            index.insert("rms_error_pct".into(), p.rms_error_pct.into());
            index.insert("peak_rise_db".into(), finite(p.peak_rise_db).into());
            index.insert("bin_rise_db".into(), finite(p.bin_rise_db).into());
            index.insert("predicted_peak_ratio".into(), p.predicted_peak_ratio.into());
            index.insert("lag_frames".into(), p.lag_frames.into());
            index.insert(
                "values".into(),
                serde_json::to_value(&p.values).unwrap_or_default(),
            );
            index.insert("drive_len".into(), p.drive_out.len().into());
            let mut bytes = Vec::with_capacity(4 * p.drive_out.len());
            for v in &p.drive_out {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            if hertzforge_capture::append_indexed_record(&dir, "law_calls", &index, &bytes).is_err()
            {
                // Keep it for the host rather than lose it.
                run.records.push(r);
                break;
            }
        }
    }

    /// Take the law's call records the ring holds, oldest first: what a host
    /// writes when the core is not capturing. Drained between plays.
    pub fn take_law_records(&mut self) -> Vec<TwrCallRecord> {
        self.closed
            .as_mut()
            .and_then(|c| c.run.as_mut())
            .map(|r| r.records.drain())
            .unwrap_or_default()
    }

    /// The law run, from proceed on.
    fn closed_run(&self) -> Option<&closed::LawRun> {
        self.closed.as_ref()?.run.as_ref()
    }

    /// Unusable law calls in a row now (0 without a law).
    pub fn law_misses_in_a_row(&self) -> u32 {
        self.closed_run().map(|r| r.misses.in_a_row()).unwrap_or(0)
    }

    /// Freeze / unfreeze adaptation (the converged drive stays fixed while
    /// frozen).
    pub fn freeze_adaptation(&mut self, frozen: bool) {
        self.adapt_frozen = frozen;
        self.log(if frozen {
            "adaptation frozen"
        } else {
            "adaptation unfrozen"
        });
    }

    /// Operator stop (graceful; distinct from abort).
    ///
    /// The cancel flag goes up FIRST, before anything else this method does.
    /// A play in flight sees it at its next block boundary and unwinds through
    /// `play_through`; without it the stop would only take effect after the
    /// current record finished, which for a 30 s TWR record is 30 s of shaker.
    pub fn stop(&mut self) -> Result<(), TwrError> {
        self.player.request_cancel();
        if matches!(
            self.state,
            TwrState::Complete | TwrState::Aborted | TwrState::Stopped
        ) {
            return Ok(());
        }
        self.state = TwrState::Stopped;
        self.log("operator stop");
        let _ = self.player.stop();
        self.player_armed = false;
        Ok(())
    }

    /// Operator abort (records the reason).
    ///
    /// Cancel flag first, then the IMMEDIATE teardown: abort takes the HAL's
    /// 25 ms taper, not stop's 250 ms one. Both are bounded below by whatever
    /// the device has already committed - mid-record preemption is refused by
    /// the hardware (-200289), so the taper is appended behind the queue.
    pub fn abort(&mut self, reason: &str) {
        self.player.request_cancel();
        let _ = self.player.emergency_stop();
        let _ = self.abort_with(format!("operator abort: {reason}"));
    }

    fn require_live(&self) -> Result<(), TwrError> {
        match self.state {
            TwrState::Iterating | TwrState::Holding | TwrState::Review => Ok(()),
            s => Err(TwrError::State(format!(
                "level control requires an iterating run (state {s:?})"
            ))),
        }
    }

    /// Release the GATE-FAIL park only.
    ///
    /// It names `Holding` explicitly rather than testing "not iterating",
    /// because the other park it could reach is [`TwrState::CoherenceHold`] -
    /// and a level trim must not release a hold that means the plant was never
    /// identified. (`require_live()` already refuses the level verbs from
    /// there, so this is the second of the two barriers.)
    fn unhold(&mut self) {
        if self.state == TwrState::Holding {
            self.state = TwrState::Iterating;
        }
    }

    /// Drop everything the identification produced and return to the top of
    /// the run. **The ONLY release for [`TwrState::CoherenceHold`]** (the Sine
    /// one-click-bypass barrier, as a state rather than as a
    /// property imported from a module that has one).
    ///
    /// What it clears: the FRF and its inverse, the trust mask, the drive
    /// spectrum, the convergence history, the plays table, station time, the
    /// ladder position, the alarm and abort latches and the trip. What it keeps: the config, the prepared reference, the
    /// feasibility verdict and its override-ack (the operator already answered
    /// that question, and re-asking it would train them to click through).
    /// A cancel raised by an earlier stop is NOT cleared here: the player
    /// clears its own flag in `open_and_configure`, which the next `advance()`
    /// reaches through `player_armed == false`. Clearing it from two places
    /// would be two authorities on one flag.
    pub fn reset(&mut self) -> Result<(), TwrError> {
        let _ = self.player.stop();
        self.player_armed = false;
        self.sysid = None;
        self.inv = None;
        self.mask.clear();
        self.mask_coverage = None;
        self.drive_spec.clear();
        self.last_drive.clear();
        self.last_achieved.clear();
        self.history = ConvergenceHistory::default();
        self.ladder = TwrLadderState::new(self.config.schedule.clone())?;
        self.plays_completed = 0;
        self.alarm_latched = false;
        self.abort_reason = None;
        self.adapt_frozen = false;
        self.last_clipped_frac = 0.0;
        self.plays.clear();
        self.station_frames = 0;
        self.trip = None;
        if let Some(c) = self.closed.as_mut() {
            *c = closed::Closed::default();
        }
        self.state = if self.feasibility.blocked && self.override_ack.is_none() {
            TwrState::FeasibilityBlocked
        } else {
            TwrState::Configured
        };
        self.log("reset: identification and iteration state dropped");
        Ok(())
    }

    // ---- capture / snapshot ----

    /// Start streaming achieved records to disk under `data_root`.
    pub fn start_capture(&mut self, data_root: &Path) -> Result<PathBuf, TwrError> {
        if self.capture.is_some() {
            return Err(TwrError::State("capture already running".into()));
        }
        let cfg = CaptureConfig::at_rate(1, self.player.fs_hz());
        let run_json = serde_json::to_string(&self.config)
            .map_err(|e| TwrError::Config(format!("config serialize: {e}")))?;
        let sink = CaptureSink::start(
            data_root,
            &self.config.capture.run_name,
            cfg,
            hertzforge_channel_table::ChannelTable::new(),
            run_json,
        )
        .map_err(|e| TwrError::Play(format!("capture start: {e}")))?;
        let dir = sink.run_dir().to_path_buf();
        self.capture = Some(sink);
        self.log("capture started");
        Ok(dir)
    }

    /// Stop the capture (writes the sidecar).
    pub fn stop_capture(&mut self) -> Result<(), TwrError> {
        if let Some(sink) = self.capture.take() {
            sink.finish()
                .map_err(|e| TwrError::Play(format!("capture finish: {e}")))?;
            self.log("capture stopped");
        }
        Ok(())
    }

    /// Save a timestamped snapshot bundle (status + convergence history) into
    /// `dir`.
    pub fn save_snapshot(&mut self, dir: &Path, marks: Vec<Mark>) -> Result<PathBuf, TwrError> {
        let payload = report::TwrSnapshotPayload {
            status: self.status(),
            history: self.history.clone(),
        };
        let snap = Snapshot {
            unix_secs: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            settings_json: serde_json::to_string(&self.config)
                .map_err(|e| TwrError::Config(format!("config serialize: {e}")))?,
            marks,
            payload,
        };
        save_snapshot(dir, &snap).map_err(|e| TwrError::Play(format!("snapshot: {e}")))
    }

    // ---- read surface (status / traces / report) ----

    pub fn state(&self) -> TwrState {
        self.state
    }

    pub fn config(&self) -> &TwrConfig {
        &self.config
    }

    pub fn feasibility(&self) -> &FeasibilityReport {
        &self.feasibility
    }

    pub fn history(&self) -> &ConvergenceHistory {
        &self.history
    }

    pub fn grid(&self) -> &RecordGrid {
        &self.prepared.grid
    }

    /// The prepared (post-prep) reference.
    pub fn prepared_reference(&self) -> &ReferenceWaveform {
        &self.prepared.prepared_reference
    }

    /// The status label: which delivery path, on which station, as the player
    /// states them (`host delivery on HERTZFORGE-SIM`). Derived, never a
    /// constant, so it cannot name a station the run is not on. The report's
    /// note is the same sentence.
    fn honest_path_label(&self) -> String {
        let station = self
            .player
            .station_label()
            .unwrap_or_else(|| "an unnamed station".to_string());
        let delivery = match self.player.path() {
            crate::player::TwrPath::HostBackend => "host",
            crate::player::TwrPath::Delegated => "delegated",
        };
        format!("{delivery} delivery on {station}")
    }

    /// The rate station time is counted at: the rate the station adopted, once
    /// it has said, and the requested rate before.
    fn station_rate_hz(&self) -> f64 {
        self.player
            .actual_fs_hz()
            .unwrap_or_else(|| self.player.fs_hz())
    }

    fn station_seconds(&self, frames: u64) -> f64 {
        frames as f64 / self.station_rate_hz()
    }

    /// Take what the player's display tap has collected since the last drain
    /// (`None` when no tap is attached). Called BETWEEN units.
    pub fn drain_display_tap(&mut self) -> Option<crate::player::TapDrain> {
        self.player.drain_display_tap()
    }

    // ---- the HOST READ PATH ---------------------------------------------
    //
    // Four narrow delegations, added because a daemon host has to report on a
    // run it cannot see into and `player()` is `pub(crate)`. They are stated as
    // four named facts rather than by widening `player()` to `pub`, which would
    // hand every caller the whole player - including its mutating surface - to
    // answer four read-only questions. The chassis gate reached the same
    // facts by WRAPPING the backend (`ProbedBackend`); that shape belongs to a
    // gate that owns its own backend and not to a host that is handed one.

    /// Frames the response trails the drive by, as the player has it.
    pub fn lag_frames(&self) -> Option<usize> {
        self.player.lag_frames()
    }

    /// Where that alignment came from - derived, measured, or measured-and-no-peak.
    pub fn lag_provenance(&self) -> Option<crate::player::LagSource> {
        self.player.lag_provenance()
    }

    /// Frames the device accepted before `start()` on the last play.
    pub fn committed_frames(&self) -> Option<usize> {
        self.player.committed_frames()
    }

    /// Drain the DRIVER's diagnostics since the last call.
    pub fn take_driver_warnings(&mut self) -> Vec<String> {
        self.player.take_driver_warnings()
    }

    /// The run's event log, oldest first - the abort tail a host journals on
    /// the way out.
    ///
    /// PUBLIC since the daemon host landed (it was `pub(crate)` for the report assembler). The
    /// daemon's pump prints the last few entries when a run ends aborted,
    /// because a tripped run that reads as an unexplained thread exit in the
    /// journal is the Random path's 2026-08-03 lesson.
    pub fn event_log(&self) -> &[String] {
        &self.events
    }

    pub fn status(&self) -> TwrStatus {
        TwrStatus {
            state: self.state,
            path_label: self.honest_path_label(),
            level_db: self.ladder.current_level_db(),
            level_index: self.ladder.level_index(),
            rung: self.ladder.rung(),
            iterations_at_level: self.ladder.iterations_at_level(),
            total_iterations: self
                .history
                .records
                .iter()
                .filter(|r| r.iteration > 0)
                .count(),
            converged_at_level: self.ladder.converged_at_level(),
            ladder_completed: self.ladder.completed(),
            plays_completed: self.plays_completed,
            plays_target: self.config.run.plays_target(self.prepared.section_s),
            last_metrics: self.history.last().map(|r| r.metrics),
            sysid_mean_coherence: self.sysid.as_ref().map(|s| s.in_band_mean_coherence),
            mask_energy_coverage: self.mask_coverage,
            alarm_active: self.alarm_latched,
            clipped_frac: self.last_clipped_frac,
            abort_reason: self.abort_reason.clone(),
            trip: self.trip.clone(),
            feasibility_blocked: self.state == TwrState::FeasibilityBlocked,
            adapt_frozen: self.adapt_frozen,
            capturing: self.capture.is_some(),
            run_clock_s: self.run_clock_s,
            station_frames: self.station_frames,
            station_s: self.station_seconds(self.station_frames),
            requested_fs_hz: self.player.fs_hz(),
            actual_fs_hz: self.player.actual_fs_hz(),
        }
    }

    /// Level-scaled target window (time trace 980).
    pub fn target_time(&self) -> Vec<f64> {
        let s = self.ladder.current_scale();
        self.prepared.target_full.iter().map(|&v| v * s).collect()
    }

    /// Last achieved window (time trace 981).
    pub fn achieved_time(&self) -> &[f64] {
        &self.last_achieved
    }

    /// Last error trace `achieved - target_L` (time trace 982).
    pub fn error_time(&self) -> Vec<f64> {
        let t = self.target_time();
        self.last_achieved
            .iter()
            .zip(t.iter())
            .map(|(&a, &r)| a - r)
            .collect()
    }

    /// Last played drive record.
    pub fn drive_time(&self) -> &[f64] {
        &self.last_drive
    }

    /// FRF magnitude on the sysid grid: `(freq_hz, |H|)` (trace 983).
    pub fn frf_mag(&self) -> Vec<(f64, f64)> {
        let Some(s) = self.sysid.as_ref() else {
            return Vec::new();
        };
        let df = s.df();
        s.h.iter()
            .enumerate()
            .map(|(k, &(re, im))| (k as f64 * df, (re * re + im * im).sqrt()))
            .collect()
    }

    /// FRF phase on the sysid grid beside [`Self::frf_mag`]: `(freq_hz,
    /// degrees)`, in `[-180, 180]`. The identification's own convention: a
    /// response that lags its drive has a negative phase (a delay of `d` frames
    /// reads `-360 f d / fs`, wrapped).
    pub fn frf_phase_deg(&self) -> Vec<(f64, f64)> {
        let Some(s) = self.sysid.as_ref() else {
            return Vec::new();
        };
        let df = s.df();
        s.h.iter()
            .enumerate()
            .map(|(k, &(re, im))| (k as f64 * df, im.atan2(re).to_degrees()))
            .collect()
    }

    /// The trust mask as a curve on the record grid: `(freq_hz, 1.0)` on a bin
    /// the correction trusts (in the correction band, the reference within its
    /// floor, the coherence at or over its floor), `(freq_hz, 0.0)` elsewhere.
    /// Empty until the identification has built it.
    pub fn trust_mask(&self) -> Vec<(f64, f64)> {
        self.mask
            .iter()
            .enumerate()
            .map(|(k, &m)| (self.prepared.grid.freq(k), if m { 1.0 } else { 0.0 }))
            .collect()
    }

    /// The plays table, oldest first: one row per play after the
    /// identification ([`PlayRow`]).
    pub fn plays(&self) -> &[PlayRow] {
        &self.plays
    }

    /// Station time, frames: what the station clocked over every play and pad
    /// since the run armed.
    pub fn station_frames(&self) -> u64 {
        self.station_frames
    }

    /// What the core ended the run on, when it tripped.
    pub fn trip(&self) -> Option<&TwrTrip> {
        self.trip.as_ref()
    }

    /// The control law the run built at proceed: its id, name, version, hash
    /// (a law file's) and parameters. `None` without `control`, and before
    /// proceed.
    pub fn law_descriptor(&self) -> Option<&hertzforge_shock_common::law_guard::LawDescriptor> {
        self.closed_run().map(|run| &run.descriptor)
    }

    /// The report's law line; `None` before a law is built.
    pub(crate) fn law_report_line(&self) -> Option<report::LawReportLine> {
        let run = self.closed_run()?;
        let d = &run.descriptor;
        Some(report::LawReportLine {
            id: d.id.clone(),
            name: d.name.clone(),
            version: d.version.clone(),
            short_hash: d.sha256.as_ref().map(|h| h.chars().take(12).collect()),
            calls: run.stats.calls,
            misses: run.stats.misses,
            refused: run.stats.refused,
            worst_call_ms: run.stats.worst_call_s * 1e3,
        })
    }

    /// The peak of the drive in force at the present level, volts: what the
    /// next play would play before the clip. `None` before there is a drive.
    pub fn drive_in_force_vpk(&self) -> Option<f64> {
        match self.closed.as_ref() {
            Some(c) => c
                .run
                .as_ref()
                .map(|run| peak_abs(&run.drive) * self.ladder.current_scale()),
            None if !self.drive_spec.is_empty() => Some(peak_abs(&irfft_half(
                &self.drive_spec,
                self.prepared.grid.n,
            ))),
            None => None,
        }
    }

    /// Coherence on the sysid grid: `(freq_hz, gamma^2)` (trace 984).
    pub fn coherence(&self) -> Vec<(f64, f64)> {
        let Some(s) = self.sysid.as_ref() else {
            return Vec::new();
        };
        let df = s.df();
        s.coherence
            .iter()
            .enumerate()
            .map(|(k, &c)| (k as f64 * df, c))
            .collect()
    }

    /// Drive spectrum magnitude on the record grid (trace 985).
    pub fn drive_spectrum_mag(&self) -> Vec<(f64, f64)> {
        let at_level;
        let spec = match self.closed_run() {
            Some(run) => {
                let s = self.ladder.current_scale();
                let d: Vec<f64> = run.drive.iter().map(|&v| v * s).collect();
                at_level = rfft_half(&d, self.prepared.grid.n);
                &at_level
            }
            None => &self.drive_spec,
        };
        spec.iter()
            .enumerate()
            .map(|(k, &(re, im))| (self.prepared.grid.freq(k), (re * re + im * im).sqrt()))
            .collect()
    }

    /// Level-scaled target spectrum magnitude (trace 986).
    pub fn target_spectrum_mag(&self) -> Vec<(f64, f64)> {
        let s = self.ladder.current_scale();
        self.prepared
            .r_full
            .iter()
            .enumerate()
            .map(|(k, &(re, im))| (self.prepared.grid.freq(k), s * (re * re + im * im).sqrt()))
            .collect()
    }

    /// Last achieved spectrum magnitude (trace 987).
    pub fn achieved_spectrum_mag(&self) -> Vec<(f64, f64)> {
        if self.last_achieved.is_empty() {
            return Vec::new();
        }
        let y = rfft_half(&self.last_achieved, self.prepared.grid.n);
        y.iter()
            .enumerate()
            .map(|(k, &(re, im))| (self.prepared.grid.freq(k), (re * re + im * im).sqrt()))
            .collect()
    }

    /// The +/- abort envelope curves around the level target (traces 988/989).
    pub fn envelope_curves(&self) -> (Vec<f64>, Vec<f64>) {
        let t = self.target_time();
        let peak = t.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
        let w = self.config.limits.envelope_abort_frac * peak;
        (
            t.iter().map(|&v| v + w).collect(),
            t.iter().map(|&v| v - w).collect(),
        )
    }

    /// The run report.
    pub fn report(&self) -> report::TwrReport {
        report::TwrReport::assemble(self)
    }

    pub(crate) fn events(&self) -> &[String] {
        &self.events
    }

    pub(crate) fn player_path(&self) -> crate::player::TwrPath {
        self.player.path()
    }

    /// The player, for tests that assert which teardown the service took.
    #[cfg(test)]
    pub(crate) fn player(&self) -> &P {
        &self.player
    }

    pub(crate) fn override_ack(&self) -> Option<&str> {
        self.override_ack.as_deref()
    }
}

/// The core's verdict on a play, as a law sees it.
fn verdict(m: &ErrorMetrics, gated: bool) -> Verdict {
    Verdict {
        gated,
        rms_error_pct: m.rms_error_pct,
        peak_error_pct: m.peak_error_pct,
        spectral_error_db: m.spectral_error_db,
    }
}

/// A law fault as the trip that ends the run.
fn law_fault(fault: LawFault, consecutive: u32) -> LimitTrip {
    let message = format!("control law fault: {fault}");
    LimitTrip {
        kind: TripKind::LawFault {
            reason: fault,
            consecutive,
        },
        message,
    }
}

/// A rise as the records carry it: an infinite or undefined rise is `None`.
fn finite(x: Option<f64>) -> Option<f64> {
    x.filter(|v| v.is_finite())
}

fn peak_abs(x: &[f64]) -> f64 {
    x.iter().fold(0.0f64, |m, &v| m.max(v.abs()))
}

/// What playing the current drive came to.
enum PlayStep {
    /// The play ended the run without a judgement: the drive was refused
    /// before it reached the station, or the path returned no response.
    Ended(LimitTrip),
    /// The play was cut short during the play by the watch on its response;
    /// it has no row.
    Cut(LimitTrip),
    /// The drive played and the response was measured.
    Played(Played),
}

/// A play that was played and measured. Its drive and response are the
/// service's `last_drive` and `last_achieved`.
struct Played {
    y_half: Vec<Cplx>,
    clipped: usize,
    start_frame: u64,
    frames: u64,
    /// The response check's trip, when it tripped.
    tripped: Option<LimitTrip>,
    /// Under a control law, the play's fitted lag and its drift from the
    /// identification's, frames.
    lag: Option<(f64, f64)>,
}

/// A level step the ceiling refused: the trip's numbers, and the refusal the
/// level verbs return.
struct StepRefused {
    kind: TripKind,
    error: TwrError,
}

#[cfg(test)]
mod tests;
