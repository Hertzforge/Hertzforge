//! The daemon-side SRS SYNTHESIS module.
//!
//! # What it shares with [`crate::shock`], and what it does not
//!
//! The rails are the same and live next door: the published [`ShockCell`], the live
//! [`Progress`] counters, the bounded [`PulseRing`], the ordinal-axis SRS batch, the
//! [`TransientTap`] wall-clock display clock, the station build and the drive ceiling.
//! Everything below is what an SRS run does DIFFERENTLY from a classical one.
//!
//! ## The REVIEW HOLD is the whole shape of this module
//!
//! A passing system ID parks at `AwaitingReview`. The service stops the plant on the
//! way in - tasks parked and unreserved, so a minutes-long operator pause cannot
//! overrun the input ring or leave an output task sitting reserved - and
//! `ProceedAfterReview` re-arms and releases it. Two consequences this host is
//! responsible for, both stated in the service's own docs and both asserted by the
//! wire gate:
//!
//! * **`holds()` answers `Configured = true` AND `Live = true` through the hold.** A
//!   hold is a PAUSED run, not an idle one: the core stays claimed and a second module
//!   must not be able to bind the chassis while an operator is looking at an FRF.
//!   [`SrsHost::is_live`] therefore counts `AwaitingReview`.
//! * **A controller that vanishes during the hold leaves the hold STANDING.** The
//!   daemon's safe-stop cancels a run that is DRIVING; nothing is driving here, the
//!   hardware is already parked, and tearing the run down would throw away a measured
//!   system ID because a network went quiet.
//!
//! ## The classical named refusals
//!
//! `ProceedAfterReview` reaches this module through `live_module()`, and the classical
//! shock host has to answer it too - with a refusal that NAMES why it has no hold
//! rather than a generic "unsupported". That refusal lives in `core.rs`'s router beside
//! every other module's, which is the point of routing the four level verbs at all.
//!
//! ## What this host CANNOT do that [`crate::shock`] can, stated rather than implied
//!
//! `eigen_srs::service::plant::PulsePlant::fire` has **no per-block observer** - the
//! seam takes a cancel flag and nothing else. Two things follow, and neither is hidden:
//!
//! 1. **There is no stall watchdog here.** A watchdog whose beat can only be stored at
//!    job boundaries measures how long a job took, not whether the driver is returning;
//!    it would fire on a healthy nfft-32768 system ID and stay silent on a wedged read
//!    inside one. A check that cannot distinguish the thing it is named for is not
//!    evidence, so this host does not pretend to have one. What DOES bound a wedged
//!    read is `play_block_wise`'s own read deadline inside the plant, and what bounds
//!    a wedged daemon is the controller timeout. The recorded fix is to widen
//!    `PulsePlant::fire` with the classical firer's observer.
//! 2. **The AUTO ladder is walked HERE, one `fire_pulse` per iteration**, rather than
//!    through `SrsShockService::run_auto`. Two reasons, and the first is the one that
//!    matters: driving the ladder from this loop is what makes `pulses_fired` advance
//!    BETWEEN pulses (the mid-train requirement) without an observer to advance it
//!    during one. The second is that the service's `run_auto` is a whole LIFECYCLE
//!    convenience - it assesses feasibility, arms, runs the system ID and only then
//!    fires - which would re-run the arm this host has already performed.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use eigen_channel_table::ChannelTable;
use eigen_protocol::{Doc, ProtocolError, SchemaKind, SrsProfile, StatusReport};
use eigen_shock_common::pulse_store::PulseRecord;
use eigen_srs::service::config::{SrsTestConfig, SrsToleranceBand};
use eigen_srs::service::report::RunState as SrsRunState;
use eigen_srs::{FirePath, HalPulsePlant, ServiceError, SrsShockService};
use eigen_station::BackendKind;

use crate::provenance::{mint_run_id, now_unix, RunProvenance};
use crate::recorder::DisplayBatch;
use crate::shock::{
    build_shock_station, commanded_drive_vpk, drive_limit, drive_volts_per_eu,
    enforce_drive_ceiling, feasibility_inputs, level_schedule, no_such_pulse, now_ms, panic_text,
    srs_grid_json, srs_spectrum_batch, PrimingGuard, Progress, PulseRing, ShockCell, ShockSnapshot,
    TransientTap, CH_SRS_ABORT_HI, CH_SRS_ALARM_HI, CH_SRS_LIMIT_PRED, CH_SRS_MEASURED,
    CH_SRS_TARGET,
};

/// How long a level / query verb parks on the service mutex before refusing by name.
/// See `crate::shock`'s constant of the same name.
const LOCK_WAIT: Duration = Duration::from_millis(500);

/// The pump's idle wake-up cadence.
const JOB_POLL: Duration = Duration::from_millis(100);

/// One unit of work the SRS pump performs under the service mutex.
#[derive(Debug, Clone)]
enum Job {
    Arm { ack: Option<String> },
    RunSysId,
    ProceedAfterReview,
    FirePulse,
    RunAuto { max_pulses: u64 },
    Stop { reason: String },
    Abort { reason: String },
    Reset,
}

/// The daemon's SRS host.
pub struct SrsHost {
    svc: Arc<Mutex<Option<SrsShockService>>>,
    jobs: Arc<(Mutex<VecDeque<Job>>, Condvar)>,
    priming: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    pump: Mutex<Option<std::thread::JoinHandle<()>>>,
    cancel: Arc<AtomicBool>,
    tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    cell: Arc<ShockCell>,
    progress: Arc<Progress>,
    fault: Arc<Mutex<Option<String>>>,
    ring: Arc<Mutex<PulseRing>>,
    band_freqs: Arc<Mutex<Vec<f64>>>,
    /// The predictive-notch readout, PRE-SERIALIZED at the last job boundary.
    ///
    /// A mirror for the same reason `band_freqs` is one: the readout is
    /// polled beside the status, a fire holds the service mutex for a whole pulse,
    /// and a query that parked behind one would make a 200 ms poll look like a hung
    /// daemon. Serialized rather than stored as a struct because the only consumer is
    /// a `Doc`, and re-serializing per poll would move work onto the query path for
    /// data that changes at most once per pulse.
    notch_json: Arc<Mutex<String>>,
    last_pulse_at_ms: Arc<AtomicU64>,
    provenance: Mutex<Option<RunProvenance>>,
    last_profile: Mutex<Option<(SrsProfile, ChannelTable, String)>>,
    max_drive_vpk: Arc<Mutex<(f64, f64, f64)>>,
    kind: BackendKind,
    ni_devices: Vec<String>,
    data_root: std::path::PathBuf,
}

impl SrsHost {
    /// A host bound to a station kind and the shared display bus.
    pub fn new(
        kind: BackendKind,
        ni_devices: Vec<String>,
        data_root: std::path::PathBuf,
        tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    ) -> Self {
        SrsHost {
            svc: Arc::new(Mutex::new(None)),
            jobs: Arc::new((Mutex::new(VecDeque::new()), Condvar::new())),
            priming: Arc::new(AtomicBool::new(false)),
            stopping: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            pump: Mutex::new(None),
            cancel: Arc::new(AtomicBool::new(false)),
            tx,
            cell: Arc::new(ShockCell::default()),
            progress: Arc::new(Progress::default()),
            fault: Arc::new(Mutex::new(None)),
            ring: Arc::new(Mutex::new(PulseRing::default())),
            band_freqs: Arc::new(Mutex::new(Vec::new())),
            notch_json: Arc::new(Mutex::new(empty_notch_json())),
            last_pulse_at_ms: Arc::new(AtomicU64::new(0)),
            provenance: Mutex::new(None),
            last_profile: Mutex::new(None),
            max_drive_vpk: Arc::new(Mutex::new((
                eigen_protocol::DEFAULT_SHOCK_MAX_DRIVE_VPK,
                1.0,
                0.0,
            ))),
            kind,
            ni_devices,
            data_root,
        }
    }

    /// The provenance of the configured run, if any.
    pub fn provenance(&self) -> Option<RunProvenance> {
        self.provenance.lock().expect("provenance poisoned").clone()
    }

    /// Is a run live? **`Armed` counts, `SysId` counts, and `AwaitingReview` counts**:
    /// the hold is a paused run holding a claimed core, not an idle one.
    pub fn is_live(&self) -> bool {
        if self.priming.load(Ordering::SeqCst) || self.progress.firing.load(Ordering::SeqCst) {
            return true;
        }
        self.cell.read().is_some_and(|s| {
            matches!(
                s.module_state.split(':').next(),
                Some("armed") | Some("firing") | Some("sysid") | Some("awaiting_review")
            )
        })
    }

    /// Is anything configured at all? NEVER a blocking lock.
    pub fn is_configured(&self) -> bool {
        match self.svc.try_lock() {
            Ok(g) => g.is_some(),
            Err(std::sync::TryLockError::WouldBlock) => true,
            Err(std::sync::TryLockError::Poisoned(_)) => false,
        }
    }

    /// The analysis band grid, for `SrsFreqs`. Lock-free of the service, so an observer
    /// that joined mid-run and a controller that reconnected after a vanish can both
    /// fetch the frequency axis their spectra are drawn against.
    pub fn srs_freqs_doc(&self) -> Result<Doc, ProtocolError> {
        let f = self.band_freqs.lock().expect("freqs poisoned").clone();
        if f.is_empty() {
            return Err(not_configured());
        }
        Ok(Doc::new(SchemaKind::ShockSrsGrid, srs_grid_json(&f, 0.0)))
    }

    /// `NotchStatus` - the predictive-notch readout, out of the mirror.
    ///
    /// Never touches the service: see [`SrsHost::notch_json`]. An unconfigured
    /// core answers the same refusal every other query does, rather than an empty plan -
    /// "no core is running" and "the notch predictor produced nothing" are different
    /// facts and only the first one is the operator's to act on.
    pub fn notch_doc(&self) -> Result<Doc, ProtocolError> {
        if !self.is_configured() {
            return Err(not_configured());
        }
        let j = self.notch_json.lock().expect("notch poisoned").clone();
        Ok(Doc::new(SchemaKind::ShockNotch, j))
    }

    /// `PulseList` - rows only, out of the bounded ring, never the service.
    pub fn pulse_list_doc(&self) -> Result<Doc, ProtocolError> {
        let ring = self.ring.lock().expect("ring poisoned");
        Ok(Doc::new(SchemaKind::ShockPulse, ring.list_json()))
    }

    /// `PulseDetail{seq}` - the whole record.
    pub fn pulse_detail_doc(&self, seq: u64) -> Result<Doc, ProtocolError> {
        let ring = self.ring.lock().expect("ring poisoned");
        match ring.detail_json(seq) {
            Some(j) => Ok(Doc::new(SchemaKind::ShockPulse, j)),
            None => Err(no_such_pulse(seq, ring.dropped())),
        }
    }

    /// Translate the wire profile, build the station, construct the service, start the
    /// job pump. See `ShockHost::set_profile` for the reconfigure ordering.
    pub fn set_profile(
        &self,
        profile: SrsProfile,
        table: ChannelTable,
        lease_owner: &str,
    ) -> Result<(), ProtocolError> {
        if self.is_live() {
            return Err(ProtocolError::Refused {
                message: "an SRS run is live on this core; Stop it before reconfiguring \
                          (a run parked at the post-sysID review hold is LIVE - it holds the \
                          chassis - so Stop it or send ProceedAfterReview first)"
                    .into(),
            });
        }
        self.shutdown_pump();
        *self.svc.lock().expect("svc poisoned") = None;

        let cfg = translate_srs(&profile)?;
        // The SRS synthesis has no authored rate of its own: the run is authored as a
        // SPECTRUM and synthesized at whatever the station adopted. The station is
        // therefore built at the driver's own default for the analysis band's Nyquist
        // requirement, and the plant adopts what the device programmed at
        // `ensure_open`. Stated here because the classical profile DOES carry a
        // requested rate and the asymmetry is deliberate (`SrsProfile`'s docs).
        let build_fs = (4.0 * profile.f_hi_hz).max(5_120.0);
        let station = build_shock_station(self.kind, &self.ni_devices, &table, build_fs)?;
        tracing::info!(target: "eigen::backendd", "{}", station.provenance.log_line());

        let volts_per_eu = drive_volts_per_eu(&table);
        // The SRS demand's peak is the target's largest band, which is the honest
        // stand-in for a synthesized waveform nobody has generated yet: the composite's
        // true crest is only known after synthesis, and a ceiling that waited for it
        // would let a profile be accepted that can never run.
        let peak_eu = srs_peak_demand_g(&cfg);
        let top_db = cfg
            .schedule
            .table_db
            .iter()
            .copied()
            .chain(std::iter::once(cfg.schedule.start_db))
            .fold(f64::NEG_INFINITY, f64::max);
        enforce_drive_ceiling(
            "the profile's top ladder rung against the target's peak band",
            peak_eu,
            top_db,
            volts_per_eu,
            profile.max_drive_vpk,
        )?;

        let station_class = station.station;
        let run_id = mint_run_id();
        let prov = RunProvenance::assemble("shock.srs", run_id, &station, lease_owner, now_unix());
        tracing::info!(target: "eigen::backendd", "{}", prov.log_line());

        let plant = HalPulsePlant::new(station.backend, table.clone(), station_class)
            .map_err(|e| ProtocolError::Refused { message: e })?;
        let mut svc = SrsShockService::new(
            cfg,
            table.clone(),
            FirePath::Host(Box::new(plant)),
            Some(self.data_root.clone()),
        )
        .map_err(svc_err)?;
        svc.set_cancel_handle(Arc::clone(&self.cancel));

        self.cancel.store(false, Ordering::SeqCst);
        self.stopping.store(false, Ordering::SeqCst);
        self.progress.end();
        *self.fault.lock().expect("fault poisoned") = None;
        self.ring.lock().expect("ring poisoned").clear();
        *self.band_freqs.lock().expect("freqs poisoned") = svc.band_freqs().to_vec();
        *self.notch_json.lock().expect("notch poisoned") = empty_notch_json();
        *self.max_drive_vpk.lock().expect("ceiling poisoned") =
            (profile.max_drive_vpk, volts_per_eu, peak_eu);
        self.cell.write(ShockSnapshot {
            run: eigen_protocol::RunState::Configured,
            module_state: "configured".into(),
            level_db: Some(svc.current_level_db()),
            // The SRS panel shows the ACTUAL rate only - the operator never authored a
            // request, so a `requested_fs_hz` here would put a number on the panel that
            // means nothing (`SrsProfile`'s docs).
            requested_fs_hz: None,
            ..ShockSnapshot::default()
        });
        *self.svc.lock().expect("svc poisoned") = Some(svc);
        *self.provenance.lock().expect("provenance poisoned") = Some(prov);
        *self.last_profile.lock().expect("profile poisoned") =
            Some((profile, table, lease_owner.to_string()));
        self.spawn_pump();
        Ok(())
    }

    /// `Command::Start` **IS ARM** for this module too.
    pub fn arm(&self, ack: Option<String>) -> Result<(), ProtocolError> {
        {
            let guard = self.try_lock_svc(LOCK_WAIT).ok_or_else(|| busy("Arm"))?;
            let svc = guard.as_ref().ok_or_else(not_configured)?;
            match svc.state() {
                SrsRunState::Idle | SrsRunState::Feasibility if ack.is_none() => {}
                SrsRunState::FeasibilityBlocked if ack.is_some() => {}
                SrsRunState::FeasibilityBlocked => {
                    return Err(ProtocolError::Refused {
                        message: "arming is HARD-BLOCKED: the feasibility assessment came back \
                                  EXCEEDED. Send Srs(ArmWithOverride) with a non-empty \
                                  acknowledgement to proceed anyway."
                            .into(),
                    })
                }
                other => {
                    return Err(ProtocolError::Refused {
                        message: format!(
                            "the SRS run cannot arm from {other:?}; send SetProfile to start a \
                             fresh run"
                        ),
                    })
                }
            }
        }
        self.priming.store(true, Ordering::SeqCst);
        self.post(Job::Arm { ack });
        Ok(())
    }

    /// Run the reduced-level system ID. It DRIVES, which is why it is gated like Start
    /// and why a passing result parks at the review hold.
    pub fn run_sysid(&self) -> Result<(), ProtocolError> {
        self.require_state("RunSysId", &["sysid", "armed"])?;
        self.post(Job::RunSysId);
        Ok(())
    }

    /// Release the review hold.
    pub fn proceed_after_review(&self) -> Result<(), ProtocolError> {
        self.require_state("ProceedAfterReview", &["awaiting_review"])?;
        self.post(Job::ProceedAfterReview);
        Ok(())
    }

    /// Fire ONE synthesized pulse at the current level.
    pub fn fire_pulse(&self) -> Result<(), ProtocolError> {
        self.require_state("FirePulse", &["armed", "firing"])?;
        self.post(Job::FirePulse);
        Ok(())
    }

    /// Walk the AUTO ladder, bounded. See this module's docs for why the loop lives
    /// HERE rather than in the service's own `run_auto`.
    pub fn run_auto(&self, max_pulses: Option<u32>) -> Result<(), ProtocolError> {
        self.require_state("RunAuto", &["armed", "firing"])?;
        self.post(Job::RunAuto {
            max_pulses: max_pulses.unwrap_or(u32::MAX) as u64,
        });
        Ok(())
    }

    /// The configured ladder's rungs, dB. Answers the router's nearest-rung jump
    /// WITHOUT the service mutex being held across the decision - it is read, released,
    /// and the jump takes the lock again on its own. A stale read between the two is
    /// harmless: the table is authored at configure and cannot change during a run.
    pub fn level_table(&self) -> Result<Vec<f64>, ProtocolError> {
        let guard = self
            .try_lock_svc(LOCK_WAIT)
            .ok_or_else(|| busy("reading the level table"))?;
        let svc = guard.as_ref().ok_or_else(not_configured)?;
        Ok(svc.config().schedule.table_db.clone())
    }

    /// Jump to a level-table index. The drive CEILING is re-checked at the new level.
    pub fn jump_to_level(&self, index: usize) -> Result<f64, ProtocolError> {
        let mut guard = self
            .try_lock_svc(LOCK_WAIT)
            .ok_or_else(|| busy("a level jump"))?;
        let svc = guard.as_mut().ok_or_else(not_configured)?;
        let want = svc
            .config()
            .schedule
            .table_db
            .get(index)
            .copied()
            .ok_or_else(|| ProtocolError::InvalidArgument {
                message: format!(
                    "level index {index} is not a rung of the configured ladder ({} rung(s))",
                    svc.config().schedule.table_db.len()
                ),
            })?;
        self.check_level_ceiling(want)?;
        svc.jump_to_level(index).map_err(svc_err)
    }

    /// See `ShockHost::adjust_level`: whole dB only, and the echo is what was reached.
    pub fn adjust_level(&self, delta_db: f64) -> Result<f64, ProtocolError> {
        if !delta_db.is_finite() || (delta_db - delta_db.round()).abs() > 1e-9 {
            return Err(ProtocolError::InvalidArgument {
                message: format!(
                    "the shock ladder steps in WHOLE dB (the module's manual step is +/-1 dB), \
                     so a {delta_db} dB step cannot be honored. Send a whole number of dB, or \
                     Srs(JumpToLevel) to name a rung."
                ),
            });
        }
        let mut guard = self
            .try_lock_svc(LOCK_WAIT)
            .ok_or_else(|| busy("a level step"))?;
        let svc = guard.as_mut().ok_or_else(not_configured)?;
        let steps = delta_db.round() as i64;
        self.check_level_ceiling(svc.current_level_db() + steps as f64)?;
        let mut last = svc.current_level_db();
        for _ in 0..steps.unsigned_abs() {
            last = if steps > 0 {
                svc.step_up().map_err(svc_err)?
            } else {
                svc.step_down().map_err(svc_err)?
            };
        }
        Ok(last)
    }

    fn check_level_ceiling(&self, level_db: f64) -> Result<(), ProtocolError> {
        let (ceiling, volts_per_eu, peak) = *self.max_drive_vpk.lock().expect("ceiling poisoned");
        enforce_drive_ceiling("that level", peak, level_db, volts_per_eu, ceiling).map(|_| ())
    }

    /// `Srs(Reset)`: take the run down and **RELEASE THE CORE**.
    ///
    /// # Why this is a teardown and not a rewind
    ///
    /// The arm moved the station backend into the fire path with no reclaim path, so
    /// there is no in-place route back to `Idle`: a service reset in place would refuse
    /// its next arm. Reset therefore stops the run, drops the service, and leaves the
    /// operator to send `SetProfile` - which is the same rebuild the local app's Reset
    /// escape hatch performs.
    ///
    /// **Releasing the core is the load-bearing half**, and it was found by running the
    /// wire probe rather than reasoned about. A module whose service is still present
    /// holds the core at `Claim::Configured` even after its run has STOPPED, and a bare
    /// `Command::Start` is dispatched on that claim through the documented priority
    /// order. So a stopped shock run left configured takes the next `Start` and answers
    /// "cannot arm from Stopped" - about a module the operator had already moved on
    /// from. Reset is the verb that says "I am done with this module"; it has to mean
    /// it.
    ///
    /// The published cell is CLEARED with the service. A status cell outliving its
    /// service would report a run this core can no longer act on.
    pub fn reset(&self) -> Result<(), ProtocolError> {
        if self
            .last_profile
            .lock()
            .expect("profile poisoned")
            .is_none()
        {
            return Err(not_configured());
        }
        self.cancel.store(true, Ordering::SeqCst);
        self.stopping.store(true, Ordering::SeqCst);
        self.post(Job::Reset);
        self.shutdown_pump();
        *self.svc.lock().expect("svc poisoned") = None;
        self.cell.clear();
        self.progress.end();
        *self.fault.lock().expect("fault poisoned") = None;
        self.ring.lock().expect("ring poisoned").clear();
        *self.band_freqs.lock().expect("freqs poisoned") = Vec::new();
        *self.notch_json.lock().expect("notch poisoned") = empty_notch_json();
        self.last_pulse_at_ms.store(0, Ordering::Relaxed);
        *self.provenance.lock().expect("provenance poisoned") = None;
        self.stopping.store(false, Ordering::SeqCst);
        self.cancel.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// THE CONTROLLED STOP.
    ///
    /// **A run parked at the review hold is NOT stopped by this**: the hold
    /// already parked the hardware, nothing is driving, and a safe-stop that tore the
    /// hold down would throw away a measured system ID because a network went quiet.
    /// The hold stands until the operator proceeds or explicitly stops.
    pub fn safe_stop(&self, reason: &str) {
        if self.at_hold() {
            tracing::info!(
                target: "eigen::backendd",
                reason,
                "SAFE-STOP reached an SRS run at the review HOLD: nothing is driving and the \
                 plant is already parked, so the hold STANDS"
            );
            return;
        }
        if !self.is_live() && !self.is_configured() {
            return;
        }
        tracing::warn!(
            target: "eigen::backendd",
            reason,
            "SAFE-STOP: cancelling the SRS run at the next boundary"
        );
        self.stopping.store(true, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
        self.post(Job::Stop {
            reason: reason.to_string(),
        });
    }

    /// An OPERATOR stop, which does end a hold - the difference from
    /// [`SrsHost::safe_stop`] is who asked.
    pub fn operator_stop(&self, reason: &str) {
        self.stopping.store(true, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
        self.post(Job::Stop {
            reason: reason.to_string(),
        });
    }

    /// ABORT: `emergency_stop`, tasks unreserved. The latency arithmetic is the
    /// service's (`SrsShockService::abort`'s docs): the committed queue plays out.
    pub fn abort(&self, reason: &str) {
        tracing::warn!(target: "eigen::backendd", reason, "ABORT: SRS");
        self.stopping.store(true, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
        self.post(Job::Abort {
            reason: reason.to_string(),
        });
    }

    /// Is the run parked at the review hold?
    pub fn at_hold(&self) -> bool {
        self.cell
            .read()
            .is_some_and(|s| s.module_state.starts_with("awaiting_review"))
    }

    /// Fold this module's truth into a protocol status report. Never takes the service
    /// mutex.
    pub fn fill_status(&self, s: &mut StatusReport) {
        let Some(snap) = self.cell.read() else {
            return;
        };
        s.requested_fs_hz = snap.requested_fs_hz;
        s.actual_fs_hz = snap.actual_fs_hz;
        if let Some(f) = self.fault.lock().ok().and_then(|f| f.clone()) {
            s.run = eigen_protocol::RunState::Faulted;
            s.module_state = Some(f);
            return;
        }
        if self.priming.load(Ordering::SeqCst) {
            s.run = eigen_protocol::RunState::Armed;
            s.module_state = Some(
                "priming (opening the station, configuring it, adopting the rate the driver \
                 programmed, consulting the arm gate, resolving the band grid and synthesizing \
                 the drive to the target - the run is ready when it reports armed)"
                    .into(),
            );
            return;
        }
        s.run = snap.run;
        s.module_state = Some(snap.module_state.clone());
        s.level_db = snap.level_db;
        s.alarm_active = snap.alarm_active;
        s.abort_active = snap.abort_active;
        s.band_coherence = snap.band_coherence;
        s.display_blocks_dropped = Some(snap.records_dropped);

        // THE DRIVE ECHO - the classical host's note applies verbatim: the volts
        // this core will COMMAND at the current rung, from the same derivation the
        // ceiling refuses on, never a readback.
        if let Some(db) = snap.level_db {
            if let Ok(g) = self.max_drive_vpk.lock() {
                let (_ceiling, volts_per_eu, peak_eu) = *g;
                let v = commanded_drive_vpk(peak_eu, db, volts_per_eu);
                if v.is_finite() {
                    s.drive_vpk = Some(v);
                }
            }
        }
        s.pulses_fired = Some(snap.pulses_fired);
        s.pulses_at_level = Some(snap.pulses_at_level);
        s.train_remaining = snap.train_remaining;
        s.last_pulse_seq = snap.last_pulse_seq;
        let at = self.last_pulse_at_ms.load(Ordering::Relaxed);
        if at != 0 && now_ms().saturating_sub(at) <= crate::shock::LAST_PULSE_TTL.as_millis() as u64
        {
            if let Some(j) = self.ring.lock().ok().and_then(|r| r.last_status_json()) {
                s.last_pulse = Some(Doc::new(SchemaKind::ShockPulse, j));
            }
        }
    }

    // --- internals ------------------------------------------------------------------

    fn require_state(&self, what: &str, want: &[&str]) -> Result<(), ProtocolError> {
        if self.priming.load(Ordering::SeqCst) {
            return Ok(());
        }
        let snap = self.cell.read().ok_or_else(not_configured)?;
        let token = snap.module_state.split(':').next().unwrap_or("");
        if want.contains(&token) {
            return Ok(());
        }
        Err(ProtocolError::Refused {
            message: format!(
                "{what} needs an SRS run in one of {want:?} and this one is {token}. Start \
                 means ARM for this module; RunSysId drives the reduced-level system ID; \
                 ProceedAfterReview releases the post-sysID hold."
            ),
        })
    }

    fn try_lock_svc(
        &self,
        budget: Duration,
    ) -> Option<std::sync::MutexGuard<'_, Option<SrsShockService>>> {
        let deadline = Instant::now() + budget;
        loop {
            match self.svc.try_lock() {
                Ok(g) => return Some(g),
                Err(std::sync::TryLockError::Poisoned(_)) => return None,
                Err(std::sync::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }

    fn post(&self, job: Job) {
        let (lock, cv) = &*self.jobs;
        if let Ok(mut q) = lock.lock() {
            q.push_back(job);
            cv.notify_all();
        }
    }

    fn shutdown_pump(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.jobs.1.notify_all();
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
        if let Ok(mut q) = self.jobs.0.lock() {
            q.clear();
        }
        self.stop.store(false, Ordering::SeqCst);
    }

    fn spawn_pump(&self) {
        let mut slot = self.pump.lock().expect("pump poisoned");
        if let Some(h) = slot.take() {
            if h.is_finished() {
                let _ = h.join();
            } else {
                *slot = Some(h);
                return;
            }
        }
        self.stop.store(false, Ordering::SeqCst);
        let ctx = PumpCtx {
            svc: Arc::clone(&self.svc),
            jobs: Arc::clone(&self.jobs),
            stop: Arc::clone(&self.stop),
            priming: Arc::clone(&self.priming),
            stopping: Arc::clone(&self.stopping),
            cancel: Arc::clone(&self.cancel),
            tx: self.tx.clone(),
            cell: Arc::clone(&self.cell),
            progress: Arc::clone(&self.progress),
            fault: Arc::clone(&self.fault),
            ring: Arc::clone(&self.ring),
            band_freqs: Arc::clone(&self.band_freqs),
            notch_json: Arc::clone(&self.notch_json),
            last_pulse_at: Arc::clone(&self.last_pulse_at_ms),
            run_id: self
                .provenance()
                .and_then(|p| p.run_id.parse::<u64>().ok())
                .unwrap_or(0),
        };
        let svc_for_panic = Arc::clone(&self.svc);
        let fault_out = Arc::clone(&self.fault);
        let priming_out = Arc::clone(&self.priming);
        let handle = std::thread::spawn(move || {
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                pump_body(ctx);
            }));
            if let Err(payload) = caught {
                let what = panic_text(&payload);
                tracing::error!(
                    target: "eigen::backendd",
                    panic = %what,
                    "SRS PUMP PANICKED - issuing emergency_stop"
                );
                if let Ok(mut g) = svc_for_panic.lock() {
                    if let Some(s) = g.as_mut() {
                        s.abort_now(format!("daemon pump panicked: {what}"));
                    }
                }
                if let Ok(mut f) = fault_out.lock() {
                    *f = Some(format!("pump_panicked: {what}; emergency_stop was issued"));
                }
                priming_out.store(false, Ordering::SeqCst);
            }
        });
        *slot = Some(handle);
    }
}

impl Drop for SrsHost {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
        self.jobs.1.notify_all();
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
    }
}

// =====================================================================================
// The pump
// =====================================================================================

struct PumpCtx {
    svc: Arc<Mutex<Option<SrsShockService>>>,
    jobs: Arc<(Mutex<VecDeque<Job>>, Condvar)>,
    stop: Arc<AtomicBool>,
    priming: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    cell: Arc<ShockCell>,
    progress: Arc<Progress>,
    fault: Arc<Mutex<Option<String>>>,
    ring: Arc<Mutex<PulseRing>>,
    band_freqs: Arc<Mutex<Vec<f64>>>,
    notch_json: Arc<Mutex<String>>,
    last_pulse_at: Arc<AtomicU64>,
    run_id: u64,
}

fn pump_body(ctx: PumpCtx) {
    let mut tap = TransientTap::new(ctx.run_id);
    let mut window_seq: u32 = 0;
    let mut published_seq: Option<u64> = None;

    while !ctx.stop.load(Ordering::Relaxed) {
        let job = {
            let (lock, cv) = &*ctx.jobs;
            let mut q = match lock.lock() {
                Ok(q) => q,
                Err(_) => break,
            };
            if q.is_empty() && !ctx.stop.load(Ordering::Relaxed) {
                q = match cv.wait_timeout(q, JOB_POLL) {
                    Ok((g, _)) => g,
                    Err(_) => return,
                };
            }
            q.pop_front()
        };
        let Some(job) = job else { continue };
        if ctx.stop.load(Ordering::Relaxed) {
            break;
        }
        run_job(&ctx, job, &mut tap, &mut window_seq, &mut published_seq);
    }

    ctx.progress.end();
    if let Ok(g) = ctx.svc.lock() {
        if let Some(s) = g.as_ref() {
            publish_snapshot(&ctx, s, None);
            if let Some(t) = s.trip() {
                tracing::warn!(
                    target: "eigen::backendd",
                    trip = format!("{:?}", t.kind),
                    at_level_db = t.at_level_db,
                    at_pulse = t.at_pulse,
                    "SRS run ended ABORTED"
                );
            }
        }
    }
    tracing::info!(target: "eigen::backendd", "srs pump thread exited");
}

fn run_job(
    ctx: &PumpCtx,
    job: Job,
    tap: &mut TransientTap,
    window_seq: &mut u32,
    published_seq: &mut Option<u64>,
) {
    let guard = matches!(job, Job::Arm { .. }).then(|| PrimingGuard(Arc::clone(&ctx.priming)));

    let mut lock = match ctx.svc.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(svc) = lock.as_mut() else { return };

    ctx.progress.begin(0, 0.0);

    let outcome: Result<(), ProtocolError> = match job {
        Job::Arm { ack } => {
            let r = match ack {
                Some(a) => svc.arm_with_override(&a),
                None => match svc.assess_feasibility() {
                    Ok(_) | Err(ServiceError::BadState(_)) => svc.arm(),
                    Err(e) => Err(e),
                },
            };
            match r {
                Ok(()) => {
                    *ctx.band_freqs.lock().expect("freqs poisoned") = svc.band_freqs().to_vec();
                    // The arm-time predictive notch is computed INSIDE `arm()`, so this
                    // is the moment the plan exists.
                    *ctx.notch_json.lock().expect("notch poisoned") = notch_json(svc);
                    ctx.stopping.store(false, Ordering::SeqCst);
                    Ok(())
                }
                Err(e) => {
                    tracing::warn!(target: "eigen::backendd", "srs arm refused: {e}");
                    if let Ok(mut f) = ctx.fault.lock() {
                        *f = Some(format!("arm_refused: {e}"));
                    }
                    Ok(())
                }
            }
        }
        Job::RunSysId => svc.run_sysid().map_err(svc_err),
        Job::ProceedAfterReview => svc.proceed_after_review().map_err(svc_err),
        Job::FirePulse => svc.fire_pulse().map(|_| ()).map_err(svc_err),
        Job::RunAuto { max_pulses } => {
            // THE LADDER IS WALKED HERE, one pulse per iteration - see this module's
            // docs. Publishing between pulses is what makes `pulses_fired` advance
            // mid-run on a seam with no per-block observer.
            let mut fired = 0u64;
            let mut out = Ok(());
            while fired < max_pulses && !ctx.cancel.load(Ordering::Relaxed) {
                if svc.status().ladder_completed {
                    break;
                }
                if !matches!(svc.state(), SrsRunState::Armed | SrsRunState::Firing) {
                    break;
                }
                match svc.fire_pulse() {
                    Ok(true) => fired += 1,
                    Ok(false) => break,
                    Err(e) => {
                        out = Err(svc_err(e));
                        break;
                    }
                }
                publish_snapshot(ctx, svc, *published_seq);
                *ctx.notch_json.lock().expect("notch poisoned") = notch_json(svc);
            }
            if out.is_ok() {
                let _ = svc.stop();
            }
            out
        }
        Job::Stop { reason } => {
            tracing::info!(target: "eigen::backendd", reason, "srs controlled stop");
            svc.stop().map_err(svc_err)
        }
        Job::Abort { reason } => {
            svc.abort_now(reason);
            Ok(())
        }
        Job::Reset => {
            let _ = svc.stop();
            Ok(())
        }
    };

    if let Err(e) = outcome {
        tracing::warn!(target: "eigen::backendd", "srs job refused: {e:?}");
        if let Ok(mut f) = ctx.fault.lock() {
            if svc.state() != SrsRunState::Aborted {
                *f = Some(format!("job_refused: {e:?}"));
            }
        }
    }
    ctx.progress.end();
    ctx.cancel.store(false, Ordering::SeqCst);

    let mut fresh: Vec<PulseRecord> = Vec::new();
    {
        let after = published_seq.map(|s| s + 1).unwrap_or(0);
        for r in svc.store().records().iter().filter(|r| r.seq >= after) {
            fresh.push(r.clone());
        }
    }
    let target = svc.target_full().to_vec();
    let level_scale = 10f64.powf(svc.current_level_db() / 20.0);
    // The envelope RESOLVED onto this run's band grid, read here under the same lock
    // as the target it brackets. `EnvelopeSpec::resolve` rather than `report_data()`:
    // the report builds the whole run record (pulse table, level history, event log) and
    // this needs four numbers per band, once per publish.
    let envelope = svc.config().envelope.resolve(target.len()).ok();
    // The first limit channel's PREDICTED SRS, if the notch predictor has run. `None`
    // when limiting was not configured or no system ID stands behind it - and absent is
    // what a pane must then draw, because a zeroed curve is a prediction nobody made.
    let predicted_limit: Option<Vec<f64>> = svc
        .last_predicted_limit_srs()
        .first()
        .map(|(_name, curve)| curve.clone());
    let last_seq = fresh.last().map(|r| r.seq);
    if last_seq.is_some() {
        *published_seq = last_seq;
    }
    publish_snapshot(ctx, svc, last_seq);
    // The predicted limit-channel curves are re-derived per pulse, so the readout is
    // refreshed at every job boundary and not only at arm.
    *ctx.notch_json.lock().expect("notch poisoned") = notch_json(svc);

    drop(lock);
    drop(guard);

    if !fresh.is_empty() {
        ctx.last_pulse_at.store(now_ms(), Ordering::Relaxed);
    }
    for r in fresh {
        tap.publish_pulse(&ctx.tx, &r.time_history, r.fs_hz);
        let mut traces: Vec<(u32, Arc<Vec<f32>>)> = Vec::new();
        if !r.srs.is_empty() {
            traces.push((
                CH_SRS_MEASURED,
                Arc::new(r.srs.iter().map(|&v| v as f32).collect()),
            ));
            // The TARGET at the level this pulse was fired at, on the same ordinal
            // axis, so the operator reads demand and response on one plot rather than
            // holding one of them in their head.
            if target.len() == r.srs.len() {
                let at_level: Vec<f64> = target.iter().map(|&v| v * level_scale).collect();
                traces.push((
                    CH_SRS_TARGET,
                    Arc::new(at_level.iter().map(|&v| v as f32).collect()),
                ));
                // The two ENVELOPE edges, bracketing the target that was just pushed.
                //
                // On the SAME level-scaled base, deliberately: an alarm edge computed
                // off the full-level target while the plotted target is at -12 dB is a
                // tolerance band belonging to no curve on the plot. The local core makes
                // the other choice (everything at full level) - the divergence is real,
                // stated at `CH_SRS_ALARM_HI`, and still open.
                //
                // Only the HIGH edges are traces. The low edges are the mirror image and
                // doubling the trace count to draw them would double the pane's channel
                // list for information the high edges already carry.
                if let Some(env) = envelope.as_ref() {
                    if env.bands.len() == at_level.len() {
                        let alarm = envelope_edge(&at_level, &env.bands, |b| b.alarm_hi_db);
                        let abort = envelope_edge(&at_level, &env.bands, |b| b.abort_hi_db);
                        traces.push((CH_SRS_ALARM_HI, alarm));
                        traces.push((CH_SRS_ABORT_HI, abort));
                    }
                }
            }
            // The predictive-notch readout. Published on the band grid like everything
            // else in this batch, and only when the predictor produced one.
            if let Some(curve) = predicted_limit.as_ref() {
                if curve.len() == r.srs.len() {
                    traces.push((
                        CH_SRS_LIMIT_PRED,
                        Arc::new(curve.iter().map(|&v| v as f32).collect()),
                    ));
                }
            }
        }
        *window_seq = window_seq.wrapping_add(1);
        if let Some(b) = srs_spectrum_batch(ctx.run_id, *window_seq, traces) {
            let _ = ctx.tx.send(DisplayBatch::Spectrum(b));
        }
        ctx.ring.lock().expect("ring poisoned").push(r);
    }
}

fn publish_snapshot(ctx: &PumpCtx, svc: &SrsShockService, last_seq: Option<u64>) {
    let st = svc.status();
    let dropped = ctx.ring.lock().map(|r| r.dropped()).unwrap_or(0);
    let prev = ctx.cell.read();
    let (run, token) = srs_wire_state(svc, &st);
    ctx.cell.write(ShockSnapshot {
        run,
        module_state: token,
        level_db: Some(st.current_level_db),
        pulses_fired: st.pulses_fired.min(u32::MAX as u64) as u32,
        pulses_at_level: st.pulses_at_level,
        train_remaining: None,
        last_pulse_seq: last_seq.or_else(|| prev.as_ref().and_then(|p| p.last_pulse_seq)),
        alarm_active: st.alarm_active,
        abort_active: st.abort_active,
        requested_fs_hz: None,
        actual_fs_hz: Some(svc.fs_hz()),
        band_coherence: (st.band_coherence > 0.0).then_some(st.band_coherence),
        records_dropped: dropped,
    });
}

/// The service state onto the wire, with a NAMESPACED module-state token.
///
/// **`AwaitingReview` maps to `Armed` plus a token, not to a new wire RunState**
/// (the Random precedent): a new `RunState` variant would be a
/// one-directional wire addition that only new peers could interpret, for a state an
/// old peer already renders correctly as "armed, not running".
fn srs_wire_state(
    svc: &SrsShockService,
    st: &eigen_srs::service::report::Status,
) -> (eigen_protocol::RunState, String) {
    match st.state {
        SrsRunState::Idle => (eigen_protocol::RunState::Configured, "idle".into()),
        SrsRunState::Feasibility => (eigen_protocol::RunState::Configured, "feasibility".into()),
        SrsRunState::FeasibilityBlocked => (
            eigen_protocol::RunState::Configured,
            format!(
                "feasibility_blocked: {}",
                svc.feasibility()
                    .map(|d| format!("{:?}", d.report.overall))
                    .unwrap_or_else(|| "exceeded".into())
            ),
        ),
        SrsRunState::SysId => (eigen_protocol::RunState::Running, "sysid".into()),
        SrsRunState::AwaitingReview => (
            eigen_protocol::RunState::Armed,
            format!(
                "awaiting_review: system ID complete, mean in-band coherence {:.4}; nothing is \
                 driving and the plant is parked. Send ProceedAfterReview to re-arm and \
                 continue, or Stop to end the run.",
                st.band_coherence
            ),
        ),
        SrsRunState::Armed => (eigen_protocol::RunState::Armed, "armed".into()),
        SrsRunState::Firing => (eigen_protocol::RunState::Running, "firing".into()),
        SrsRunState::Stopped => (
            eigen_protocol::RunState::Stopped,
            "stopped: the run ended cleanly".into(),
        ),
        SrsRunState::Aborted => (
            eigen_protocol::RunState::Faulted,
            format!(
                "aborted: {}",
                svc.trip()
                    .map(|t| format!("{:?}", t.kind))
                    .unwrap_or_else(|| "a protection tripped".into())
            ),
        ),
    }
}

// =====================================================================================
// Wire profile -> service config
// =====================================================================================

fn svc_err(e: ServiceError) -> ProtocolError {
    match e {
        ServiceError::Config(m) => ProtocolError::InvalidArgument {
            message: m.to_string(),
        },
        ServiceError::BadState(m) => ProtocolError::Refused { message: m },
        other => ProtocolError::Refused {
            message: other.to_string(),
        },
    }
}

fn not_configured() -> ProtocolError {
    ProtocolError::Refused {
        message: "the SRS module is not configured on this core; send SetProfile with an Srs \
                  profile first"
            .into(),
    }
}

/// One tolerance-envelope edge as a display trace: the level-scaled target lifted by the
/// per-band half-width `pick` selects.
///
/// `pick` is a plain `fn` pointer rather than a closure so both edges go through one
/// body: the alarm and abort edges differ only in which field of the band they read, and
/// two hand-written loops is two places for the level convention to drift apart.
fn envelope_edge(
    target_at_level: &[f64],
    bands: &[SrsToleranceBand],
    pick: fn(&SrsToleranceBand) -> f64,
) -> Arc<Vec<f32>> {
    Arc::new(
        target_at_level
            .iter()
            .zip(bands.iter())
            .map(|(&t, b)| (t * 10f64.powf(pick(b) / 20.0)) as f32)
            .collect(),
    )
}

// =====================================================================================
// The predictive-notch readout
// =====================================================================================

/// The readout when nothing has been predicted: the three fields, all empty.
///
/// The SAME SHAPE as a populated one, not an absent document. A consumer must be able to
/// distinguish "limiting was not configured, so nothing was notched" from "this core
/// cannot answer", and the second is what a refusal is for (see
/// [`SrsHost::notch_doc`]). Field names mirror the local core's
/// `shock_srs_notch_status` exactly, so one front-end decoder serves both transports.
fn empty_notch_json() -> String {
    serde_json::json!({
        "arm_plan": serde_json::Value::Null,
        "limit_report": serde_json::Value::Null,
        "last_predicted_srs": Vec::<(String, Vec<f64>)>::new(),
    })
    .to_string()
}

/// Serialize the service's notch state, or the empty readout when there is none.
///
/// **The expensive half is GUARDED.** `limit_report` lives inside `report_data()`, which
/// builds the whole run record (pulse table, level history, event log) to hand back one
/// optional field. A run with no limit channels - the common case - has no arm plan and
/// no predictions, so it answers the empty readout without building anything. Only a run
/// that actually notched pays, and that run's local twin pays the same cost per pulse
/// inside `srs_frames`.
fn notch_json(svc: &SrsShockService) -> String {
    let plan = svc.arm_notch_report();
    let predicted = svc.last_predicted_limit_srs();
    if plan.is_none() && predicted.is_empty() {
        return empty_notch_json();
    }
    let report = svc.report_data();
    serde_json::json!({
        "arm_plan": plan,
        "limit_report": report.limit_report,
        "last_predicted_srs": predicted,
    })
    .to_string()
}

fn busy(what: &str) -> ProtocolError {
    ProtocolError::Refused {
        message: format!(
            "a pulse is in flight on this core, so {what} cannot be applied right now. Retry \
             between pulses. Stop and Abort DO reach a fire in flight - they raise the cancel \
             flag, which needs no lock."
        ),
    }
}

/// The largest per-band demand in the target, g - the ceiling check's peak stand-in.
fn srs_peak_demand_g(cfg: &SrsTestConfig) -> f64 {
    match &cfg.target {
        eigen_srs::service::config::TargetSrs::Flat { accel_g } => accel_g.abs(),
        eigen_srs::service::config::TargetSrs::Breakpoints { points } => {
            points.iter().map(|(_, a)| a.abs()).fold(0.0f64, f64::max)
        }
    }
}

/// The SRS wire profile -> the service config.
///
/// Carries the same two obligations as the classical translation (see
/// `crate::shock::translate_classical`): the capture block is FORCED off because the
/// wire has no field for it (remote capture is not served over this seam), and every
/// operator input crosses verbatim rather than being defaulted daemon-side.
fn translate_srs(p: &SrsProfile) -> Result<SrsTestConfig, ProtocolError> {
    use eigen_srs::service::config as sc;

    let (shaker, moving_mass) = feasibility_inputs(p.feasibility.as_ref())?;
    let cfg = SrsTestConfig {
        f_lo_hz: p.f_lo_hz,
        f_hi_hz: p.f_hi_hz,
        fraction: match p.fraction {
            eigen_protocol::SrsBandFraction::Whole => sc::BandFraction::Whole,
            eigen_protocol::SrsBandFraction::Third => sc::BandFraction::Third,
            eigen_protocol::SrsBandFraction::Sixth => sc::BandFraction::Sixth,
            eigen_protocol::SrsBandFraction::Twelfth => sc::BandFraction::Twelfth,
            eigen_protocol::SrsBandFraction::Arbitrary(n) => sc::BandFraction::Arbitrary(n),
        },
        target: match &p.target {
            eigen_protocol::SrsTarget::Flat { accel_g } => {
                sc::TargetSrs::Flat { accel_g: *accel_g }
            }
            eigen_protocol::SrsTarget::Breakpoints { points } => sc::TargetSrs::Breakpoints {
                points: points.clone(),
            },
        },
        q: p.q,
        engine: match p.engine {
            eigen_protocol::SrsEngineKind::Wavsyn => sc::EngineKind::Wavsyn,
            eigen_protocol::SrsEngineKind::Seds => sc::EngineKind::Seds,
        },
        synthesis: eigen_srs::iterate::SynthesisOpts {
            max_iters: p.synthesis.max_iters,
            tol_db: p.synthesis.tol_db,
            max_gain_db_per_iter: p.synthesis.max_gain_db_per_iter,
            q: p.synthesis.q,
        },
        schedule: level_schedule(&p.schedule),
        limits: p.limits.as_ref().map(|l| sc::LimitSet {
            channels: l
                .channels
                .iter()
                .map(|c| sc::LimitChannelSpec {
                    name: c.name.clone(),
                    limit: match &c.limit {
                        eigen_protocol::SrsLimitTarget::Flat { accel_g } => {
                            sc::LimitTarget::Flat { accel_g: *accel_g }
                        }
                        eigen_protocol::SrsLimitTarget::PerBand { bounds_g } => {
                            sc::LimitTarget::PerBand {
                                bounds_g: bounds_g.clone(),
                            }
                        }
                    },
                    frf: c.frf.as_ref().map(|f| sc::FrfSpec {
                        freqs_hz: f.freqs_hz.clone(),
                        mag: f.mag.clone(),
                    }),
                })
                .collect(),
            headroom_db: l.headroom_db,
        }),
        shaker,
        moving_mass,
        feasibility_tol: sc::FeasibilityTol {
            residual_velocity_ips: p.feasibility_tol.residual_velocity_ips,
            residual_disp_in: p.feasibility_tol.residual_disp_in,
        },
        drive_limit: drive_limit(&p.drive_limit),
        drive_abort_factor: p.drive_abort_factor,
        kinematic_abort: sc::KinematicAbort {
            accel_g: p.kinematic_abort.accel_g,
            velocity_ips: p.kinematic_abort.velocity_ips,
            disp_pp_in: p.kinematic_abort.disp_pp_in,
        },
        envelope: sc::EnvelopeSpec {
            alarm_db: p.envelope.alarm_db,
            abort_db: p.envelope.abort_db,
            per_band: p.envelope.per_band.as_ref().map(|b| {
                b.iter()
                    .map(|t| sc::SrsToleranceBand {
                        alarm_lo_db: t.alarm_lo_db,
                        alarm_hi_db: t.alarm_hi_db,
                        abort_lo_db: t.abort_lo_db,
                        abort_hi_db: t.abort_hi_db,
                    })
                    .collect()
            }),
        },
        sysid: p.sysid.map(|s| sc::SysIdSpec {
            level_db: s.level_db,
            excitation: match s.excitation {
                eigen_protocol::SrsSysIdExcitation::Shaped => sc::SysIdExcitation::Shaped,
                eigen_protocol::SrsSysIdExcitation::StandardPulse => {
                    sc::SysIdExcitation::StandardPulse
                }
            },
            nfft: s.nfft,
            h_floor_frac: s.h_floor_frac,
        }),
        // FORCED OFF - the remote-capture obligation. `ShockCaptureTrigger::Off` is the SRS
        // module's spelling of "no disk writer opens at arm"; `snapshot_on_stop` goes
        // with it, because a snapshot bundle is a file on the core host the remote
        // operator can neither see nor fetch.
        capture: sc::ShockCaptureRules {
            trigger: sc::ShockCaptureTrigger::Off,
            snapshot_on_stop: false,
        },
        compute_pvss: p.compute_pvss,
        ring_out_s: p.ring_out_s,
        review_after_sysid: p.review_after_sysid,
        metadata: p
            .metadata
            .iter()
            .map(|m| sc::MetaField {
                key: m.key.clone(),
                label: m.label.clone(),
                value: m.value.clone(),
            })
            .collect(),
    };
    cfg.validate().map_err(|e| ProtocolError::InvalidArgument {
        message: e.to_string(),
    })?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire() -> SrsProfile {
        use eigen_protocol::*;
        SrsProfile {
            schema_version: PROFILE_SCHEMA,
            f_lo_hz: 20.0,
            f_hi_hz: 2000.0,
            fraction: SrsBandFraction::Third,
            target: SrsTarget::Flat { accel_g: 50.0 },
            q: 10.0,
            engine: SrsEngineKind::Wavsyn,
            synthesis: SrsSynthesisOpts {
                max_iters: 20,
                tol_db: 1.5,
                max_gain_db_per_iter: 3.0,
                q: 10.0,
            },
            schedule: ShockLevelSchedule {
                mode: ShockRunMode::Auto,
                start_db: -12.0,
                table_db: vec![-12.0, -6.0],
                pulses_per_level: vec![1],
            },
            limits: None,
            feasibility: None,
            feasibility_tol: SrsFeasibilityTol {
                residual_velocity_ips: 1.0,
                residual_disp_in: 0.1,
            },
            drive_limit: ShockDriveLimit {
                peak_accel_g: None,
                monitors: Vec::new(),
            },
            drive_abort_factor: 2.0,
            kinematic_abort: SrsKinematicAbort {
                accel_g: None,
                velocity_ips: None,
                disp_pp_in: None,
            },
            envelope: SrsEnvelopeSpec {
                alarm_db: (3.0, 3.0),
                abort_db: (9.0, 6.0),
                per_band: None,
            },
            sysid: None,
            compute_pvss: false,
            ring_out_s: 0.05,
            review_after_sysid: true,
            max_drive_vpk: DEFAULT_SHOCK_MAX_DRIVE_VPK,
            metadata: Vec::new(),
        }
    }

    /// **THE REMOTE-CAPTURE OBLIGATION, SRS half.** The wire profile has no capture block
    /// and the translation must not inherit whatever the core default is.
    ///
    /// RED-MUTATION: set `trigger: ShockCaptureTrigger::EveryPulse` and this reds - the
    /// state where a remote configure opens a disk writer on the core host that the
    /// operator cannot see, size, stop or fetch.
    #[test]
    fn the_srs_translation_forces_capture_off_and_snapshots_off() {
        let cfg = translate_srs(&wire()).expect("translate");
        assert!(
            matches!(
                cfg.capture.trigger,
                eigen_srs::service::config::ShockCaptureTrigger::Off
            ),
            "the daemon must force the SRS capture trigger OFF"
        );
        assert!(
            !cfg.capture.snapshot_on_stop,
            "a snapshot bundle is a file on the CORE host that no fetch path crosses this \
             seam"
        );
    }

    /// The review hold DEFAULTS ON and rides the profile.
    #[test]
    fn the_review_hold_rides_the_profile() {
        let mut p = wire();
        assert!(translate_srs(&p).expect("translate").review_after_sysid);
        p.review_after_sysid = false;
        assert!(!translate_srs(&p).expect("translate").review_after_sysid);
    }

    /// The ceiling's peak stand-in is the target's LARGEST band, on both target shapes.
    #[test]
    fn the_ceiling_peak_is_the_targets_largest_band() {
        let flat = translate_srs(&wire()).expect("translate");
        assert!((srs_peak_demand_g(&flat) - 50.0).abs() < 1e-9);
        let mut p = wire();
        p.target = eigen_protocol::SrsTarget::Breakpoints {
            // The table must COVER the band grid, whose last center sits at or ABOVE
            // f_hi (octave_freqs' cover-the-top rule), so the last point is past 2000.
            points: vec![(20.0, 10.0), (200.0, 120.0), (2500.0, 40.0)],
        };
        let bp = translate_srs(&p).expect("translate");
        assert!((srs_peak_demand_g(&bp) - 120.0).abs() < 1e-9);
    }
}
