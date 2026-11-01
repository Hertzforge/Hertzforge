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

pub mod config;
pub mod report;

use std::path::{Path, PathBuf};

use hertzforge_capture::{save_snapshot, CaptureConfig, CaptureSink, Mark, Snapshot};
use hertzforge_dsp::fft::Cplx;
use serde::{Deserialize, Serialize};

use crate::grid::{irfft_half, rfft_half, RecordGrid};
use crate::inverse::GridInverse;
use crate::law;
use crate::limits::FeasibilityReport;
use crate::metrics::{ConvergenceHistory, ConvergenceRecord, ErrorMetrics};
use crate::player::{ReferencePlayer, TwrPathCapabilities};
use crate::reference::ReferenceWaveform;
use crate::schedule::{Section, TwrLadderState, TwrRunMode};
use crate::sysid::SysIdResult;
use crate::TwrError;

pub use config::TwrConfig;

/// The service state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TwrState {
    /// Feasibility hard-block: a kinematic ceiling failed; override-ack unblocks.
    FeasibilityBlocked,
    /// Configured and feasible; sysid not yet run.
    Configured,
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
    /// Honest path label (which delivery path, which plant).
    pub path_label: String,
    pub level_db: f64,
    pub level_index: usize,
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
    pub feasibility_blocked: bool,
    pub adapt_frozen: bool,
    pub capturing: bool,
    pub run_clock_s: f64,
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
        // `player.fs_hz()`, the REQUESTED rate; the gate compares that against
        // what the device actually programs, and the 9234 coerces.
        player.set_reference_rate_hz(r.fs_hz);

        let ladder = TwrLadderState::new(config.schedule.clone())?;
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
        };
        if blocked {
            svc.log("feasibility hard-block: a kinematic ceiling failed");
        }
        Ok(svc)
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

    /// Perform ONE unit of work (see the module docs).
    pub fn advance(&mut self) -> Result<Advanced, TwrError> {
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
                TwrState::Holding | TwrState::FeasibilityBlocked | TwrState::CoherenceHold => break,
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
    fn play_through(&mut self, drive: &[f64]) -> Result<Option<Vec<f64>>, TwrError> {
        match self.player.play(drive) {
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
        if let Some(reason) = self.config.limits.check_drive_peak(&excitation) {
            return self.abort_with(reason);
        }
        if let Some(reason) = self.config.limits.check_drive(&excitation) {
            return self.abort_with(reason);
        }
        let response = match self.play_through(&excitation)? {
            Some(y) => y,
            None => {
                return self
                    .abort_with("sysid response not observable on this delivery path".to_string())
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

        let sysid = match self.config.sysid.estimate(&excitation, &response, fs) {
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
            return self.abort_with(format!(
                "trust mask covers only {:.1}% of the target spectral energy (floor {:.1}%): \
                 reference content is outside the correctable band (band limits, coherence, \
                 or the reference floor) - the residual error can never drop below ~{:.0}%",
                coverage * 100.0,
                self.config.law.min_masked_energy_frac * 100.0,
                100.0 * (1.0 - coverage).sqrt()
            ));
        }
        self.mask_coverage = Some(coverage);

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
    fn play_current(&mut self, target_l: &[f64]) -> Result<(Vec<f64>, Vec<Cplx>, usize), TwrError> {
        let mut drive = irfft_half(&self.drive_spec, self.prepared.grid.n);
        let clipped = self.config.limits.apply_drive_clip(&mut drive);
        self.last_clipped_frac = if drive.is_empty() {
            0.0
        } else {
            clipped as f64 / drive.len() as f64
        };
        if clipped > 0 {
            // Clip-then-measure: the law corrects the REALIZED drive.
            self.drive_spec = rfft_half(&drive, self.prepared.grid.n);
        }
        // THE CEILING, on the realized record and before the player sees it.
        if let Some(reason) = self.config.limits.check_drive_peak(&drive) {
            return Err(TwrError::Aborted(reason));
        }
        if let Some(reason) = self.config.limits.check_drive(&drive) {
            return Err(TwrError::Aborted(reason));
        }
        let achieved = match self.play_through(&drive)? {
            Some(y) => y,
            None => {
                return Err(TwrError::Aborted(
                    "response not observable on this delivery path".to_string(),
                ))
            }
        };
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
        self.last_drive = drive;
        self.last_achieved = achieved;
        if let Some(reason) = outcome.abort {
            return Err(TwrError::Aborted(reason));
        }
        Ok((self.last_achieved.clone(), y_half, clipped))
    }

    fn do_iteration(&mut self) -> Result<Advanced, TwrError> {
        let (target_l, r_l) = self.level_target();
        let (achieved, y_half, clipped) = match self.play_current(&target_l) {
            Ok(v) => v,
            Err(TwrError::Aborted(r)) => return self.abort_with(r),
            Err(e) => return Err(e),
        };

        let metrics = ErrorMetrics::compute(
            &target_l,
            &achieved,
            &self.last_drive,
            &r_l,
            &y_half,
            &self.mask,
        );
        let iteration = self.ladder.on_iteration();
        let level_db = self.ladder.current_level_db();

        // Gate BEFORE correcting: a gated level's drive is left as-played.
        let converged = self.ladder.check_gate(metrics.rms_error_pct);
        let mut clamped_bins = 0usize;
        if !converged && !self.adapt_frozen {
            let inv = self.inv.as_ref().expect("iterating implies inverse");
            clamped_bins = law::correct(
                &mut self.drive_spec,
                &r_l,
                &y_half,
                inv,
                &self.mask,
                &self.config.law,
            );
        }

        let record = ConvergenceRecord {
            level_db,
            iteration,
            metrics,
            clamped_bins,
            clipped_samples: clipped,
        };
        self.history.push(record);

        // Divergence guard.
        if self.history.is_diverging(
            level_db,
            self.ladder.schedule().gate_rms_error_pct,
            self.config.limits.divergence_tol_frac,
            self.config.limits.divergence_n,
        ) {
            return self.abort_with(format!(
                "diverging at {level_db} dB: rms error rose {} consecutive iterations",
                self.config.limits.divergence_n
            ));
        }

        if converged {
            self.log(&format!(
                "level {level_db} dB converged in {iteration} iteration(s) at {:.2}% rms error",
                record.metrics.rms_error_pct
            ));
            if self.ladder.schedule().mode == TwrRunMode::Auto {
                if let Some(step_db) = self.ladder.advance_level() {
                    if let Err(e) = self.scale_drive_db(step_db) {
                        return self.abort_with(format!("auto ladder advance: {e}"));
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
            return match self.ladder.schedule().on_gate_fail {
                crate::schedule::GateFailPolicy::Abort => self.abort_with(format!(
                    "level {level_db} dB failed to gate within {} iterations",
                    self.ladder.schedule().max_iterations_per_level
                )),
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
        let (achieved, y_half, clipped) = match self.play_current(&target_l) {
            Ok(v) => v,
            Err(TwrError::Aborted(r)) => return self.abort_with(r),
            Err(e) => return Err(e),
        };
        // Optional continued adaptation during playback (default OFF).
        if self.config.adapt_during_playback && !self.adapt_frozen {
            let inv = self.inv.as_ref().expect("playing implies inverse");
            law::correct(
                &mut self.drive_spec,
                &r_l,
                &y_half,
                inv,
                &self.mask,
                &self.config.law,
            );
        }
        // Keep the metrics stream alive for the UI during playback.
        let metrics = ErrorMetrics::compute(
            &target_l,
            &achieved,
            &self.last_drive,
            &r_l,
            &y_half,
            &self.mask,
        );
        self.history.push(ConvergenceRecord {
            level_db: self.ladder.current_level_db(),
            iteration: 0, // 0 marks a playback play, not a correction pass
            metrics,
            clamped_bins: 0,
            clipped_samples: clipped,
        });

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

    fn abort_with(&mut self, reason: String) -> Result<Advanced, TwrError> {
        self.log(&format!("ABORT: {reason}"));
        self.abort_reason = Some(reason);
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
    fn scale_drive_db(&mut self, step_db: f64) -> Result<(), TwrError> {
        let s = 10f64.powf(step_db / 20.0);
        if !self.drive_spec.is_empty() {
            let realized = irfft_half(&self.drive_spec, self.prepared.grid.n);
            let peak_after = realized.iter().fold(0.0f64, |m, &v| m.max(v.abs())) * s;
            if let Some(reason) = self.config.limits.check_peak_value(peak_after) {
                self.log(&format!("level step {step_db:+.1} dB REFUSED: {reason}"));
                return Err(TwrError::State(format!(
                    "level step {step_db:+.1} dB refused: {reason}. The level and the drive \
                     are unchanged"
                )));
            }
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
        self.scale_drive_db(crate::schedule::MANUAL_STEP_DB)?;
        self.ladder.step_up();
        self.unhold();
        Ok(())
    }

    /// Manual -1 dB trim.
    pub fn step_level_down(&mut self) -> Result<(), TwrError> {
        self.require_live()?;
        self.scale_drive_db(-crate::schedule::MANUAL_STEP_DB)?;
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
        self.scale_drive_db(step)?;
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
            TwrState::Iterating | TwrState::Holding => Ok(()),
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
    /// spectrum, the convergence history, the ladder position, the alarm and
    /// abort latches. What it keeps: the config, the prepared reference, the
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

    /// The honesty banner: WHICH path, on WHICH station, with WHAT evidence.
    ///
    /// All three are derived, none is a constant. The string this replaces was
    /// hardcoded "acceptance = Sim convergence (real delegated drive pending the arm
    /// capture)" and it shipped in the panel long after TWR had run on NI - a
    /// claim about a station the run might not be on and a gap that was no
    /// longer the live one. The current gap IS worth naming: ZERO TWR
    /// replication iterations have run on hardware (the 2026-07-30 sweep called
    /// `run_to_completion(1)`, which from Configured is the sysid pass and
    /// nothing else), so the posture is "chassis gate pending" until the chassis
    /// gate's rows pass twice and the result is recorded.
    fn honest_path_label(&self) -> String {
        let station = self
            .player
            .station_label()
            .unwrap_or_else(|| "an unnamed station".to_string());
        format!(
            "{:?} delivery on {station}; host-owned iterative inverse-FRF law (it runs \
             BETWEEN plays, on every path); evidence: CHASSIS GATE PENDING - no TWR \
             replication iteration has yet run on hardware",
            self.player.path()
        )
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
            feasibility_blocked: self.state == TwrState::FeasibilityBlocked,
            adapt_frozen: self.adapt_frozen,
            capturing: self.capture.is_some(),
            run_clock_s: self.run_clock_s,
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
        self.drive_spec
            .iter()
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

#[cfg(test)]
mod tests;
