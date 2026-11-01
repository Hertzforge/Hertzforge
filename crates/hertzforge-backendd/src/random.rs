//! The daemon-side Random module: thin orchestration over `hertzforge-random` and the
//! station factory, so a REMOTE front end can run closed-loop Random on this host's
//! hardware.
//!
//! # Scope: the FIRST drive-producing module over the seam
//!
//! The Recorder proved the remote surface end to end without a remote peer being able
//! to command drive. This host is the next step, taken deliberately: the operator
//! asked to drive tests from the front-end machine, the protocol already speaks the
//! intent (`TestProfile::Random`), and the engines this wraps are the SAME crates the
//! on-host app runs - nothing here re-implements control.
//!
//! # THE CONTROL LOOP NEVER CROSSES THE NETWORK
//!
//! Exactly as the recorder host: the loop thread owns the service, steps
//! `loop_tick()` on the hardware host, and publishes DECIMATED DISPLAY traces on the
//! shared broadcast. What crosses the wire is operator INTENT down and display frames
//! back. A remote Stop/Abort is SUPERVISORY; the primary abort authority is the
//! in-process watchdog co-resident with the drive clock, backed by the physical E-stop.
//!
//! # One module drives this core at a time
//!
//! The Recorder host and this one share the station hardware. Configuring one while
//! the other is live is REFUSED with the module named - "half a recorder and half a
//! random run" is not a state an operator can reason about from a remote status pane.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hertzforge_breakpoints::{BreakpointRow, BreakpointTable, LevelSpec};
use hertzforge_protocol::{LadderMode, LevelTarget, ProtocolError, PsdLevel, RandomProfile};
use hertzforge_random::service::config::{LevelSchedule, RunMode};
use hertzforge_random::{LoopConfig, RandomService, RandomTestConfig, RunState};
use hertzforge_station::{
    try_build_station_backend, BackendKind, NiSelection, SimPlantSpec, StationSpec,
};

use crate::display::{scope_batch, wave_batch};
use crate::provenance::{mint_run_id, now_unix, RunProvenance};
use crate::recorder::{DisplayBatch, SpectrumBatch};

/// Loop-thread cadence between `loop_tick`s. Mirrors the Tauri layer's `TICK`.
const TICK: Duration = Duration::from_millis(20);
/// Safety cap on loop ticks before the thread bails (mirrors the Tauri layer).
const MAX_LOOP_TICKS: u64 = 100_000;
/// sysID averaging segments (mirrors the Tauri layer's `SYSID_SEGMENTS`).
const SYSID_SEGMENTS: usize = 4;

// The RESERVED display-trace channel ids, byte-identical to the local app's Random
// overlay ids (`ui/src-tauri/src/random.rs`, pinned by `ui/IPC_CONTRACT.md`). The
// remote front end decodes these frames with the SAME decoder as the local path, so
// the ids must be the same numbers or the overlay renders the wrong trace.
/// Composite measured PSD.
pub const CH_COMPOSITE: u32 = 900;
/// Reference (target) PSD, scaled to the current level.
pub const CH_REFERENCE: u32 = 901;
/// Alarm band, upper edge.
pub const CH_ALARM_HI: u32 = 902;
/// Alarm band, lower edge.
pub const CH_ALARM_LO: u32 = 903;
/// Abort band, upper edge.
pub const CH_ABORT_HI: u32 = 904;
/// Abort band, lower edge.
pub const CH_ABORT_LO: u32 = 905;
/// Drive PSD.
pub const CH_DRIVE: u32 = 906;

/// The daemon's Random host.
pub struct RandomHost {
    svc: Arc<Mutex<Option<RandomService>>>,
    stop: Arc<AtomicBool>,
    /// True while `start()` primes (feasibility -> arm -> sysID -> gate) under
    /// the svc mutex: the status path answers "priming" instead of a
    /// false STALLED DRIVER read off the PREVIOUS run's heartbeat.
    priming: Arc<AtomicBool>,
    pump: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// The SHARED display bus (one per daemon; the recorder publishes on it too).
    tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    /// The loop's EXPECTED tick period in ms - the segment duration
    /// (`nAvg x nPts / fs`), captured at set_profile. The stall watchdog is
    /// scaled to THIS; a fixed threshold is a watchdog that does not know what
    /// it is watching (2026-08-03: the operator saw "stalled driver" on a
    /// healthy loop because a 3 s threshold was calibrated when segments were
    /// ~1.4 s, and his settings make them 2-8 s).
    expected_tick_ms: AtomicU64,
    /// Unix ms of the pump's last completed iteration. `0` = no pump.
    /// The liveness signal that lets the status path DETECT a driver-stalled
    /// pump instead of blocking behind its mutex forever.
    last_tick_ms: Arc<AtomicU64>,
    /// The pump's last status snapshot. On hardware the pump holds the
    /// svc mutex for nearly the whole tick (the segment read waits inside the
    /// driver), so the status path serves THIS between wins instead of blocking
    /// a runtime worker - the exact starvation that froze every session for
    /// the length of a run.
    status_cache: Arc<Mutex<Option<hertzforge_random::service::report::Status>>>,
    /// The rate this profile ASKED the station for, f64 bits; `0` = nothing
    /// configured. Captured at `set_profile` from the built `LoopConfig`, which
    /// is the same number `AcquisitionProps::fs_hz` produced.
    requested_fs_bits: AtomicU64,
    /// The rate the station actually PROGRAMMED, f64 bits; `0` = not yet known.
    ///
    /// Only `arm()` can fill this: `LoopConfig::adopt_station_actual` replaces
    /// the service's `fs_hz` with `ConfiguredActual::actual_rate_hz` INSIDE
    /// `PlantLoop::open`, so before the arm the service's rate is still the
    /// request and reporting it as an actual would be the very lie this field
    /// exists to end. Cleared at `set_profile` so a reconfigure can never serve
    /// the previous run's adopted rate.
    actual_fs_bits: AtomicU64,
    provenance: Mutex<Option<RunProvenance>>,
    /// The operator's pretest-segment minimum, captured at
    /// `set_profile` for `start` to hand to `run_sysid`.
    pretest_segments: AtomicU64,
    data_root: std::path::PathBuf,
    kind: BackendKind,
    ni_devices: Vec<String>,
}

impl RandomHost {
    /// A host bound to a station kind and the shared display bus. Builds nothing until
    /// [`RandomHost::set_profile`].
    pub fn new(
        kind: BackendKind,
        ni_devices: Vec<String>,
        data_root: std::path::PathBuf,
        tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    ) -> Self {
        RandomHost {
            svc: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            priming: Arc::new(AtomicBool::new(false)),
            pump: Mutex::new(None),
            tx,
            provenance: Mutex::new(None),
            pretest_segments: AtomicU64::new(SYSID_SEGMENTS as u64),
            expected_tick_ms: AtomicU64::new(0),
            last_tick_ms: Arc::new(AtomicU64::new(0)),
            status_cache: Arc::new(Mutex::new(None)),
            requested_fs_bits: AtomicU64::new(0),
            actual_fs_bits: AtomicU64::new(0),
            data_root,
            kind,
            ni_devices,
        }
    }

    /// The provenance of the configured run, if any.
    pub fn provenance(&self) -> Option<RunProvenance> {
        self.provenance.lock().expect("provenance poisoned").clone()
    }

    /// Is a run live (driving or about to drive) right now?
    ///
    /// try_lock for the same reason as [`RandomHost::is_configured`]: a
    /// contended svc mutex IS the pump mid-tick, which is the definition of
    /// live - answering `true` without waiting is both correct and the only
    /// answer that cannot starve the runtime.
    pub fn is_live(&self) -> bool {
        let guard = match self.svc.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::WouldBlock) => return true,
            Err(std::sync::TryLockError::Poisoned(_)) => return false,
        };
        guard.as_ref().is_some_and(|s| {
            matches!(
                s.state(),
                RunState::Feasibility
                    | RunState::SysId
                    | RunState::CoherenceGate
                    | RunState::Engaged
                    | RunState::FullLevel
            )
        })
    }

    /// Is anything configured at all?
    ///
    /// NEVER a blocking lock: this runs on a tokio worker inside
    /// dispatch, and during a hardware run the pump holds the svc mutex for
    /// nearly the whole tick with an immediate unfair re-acquire - a blocking
    /// waiter here starved for the LENGTH OF THE RUN (measured 29 s), freezing
    /// the session select and with it heartbeats, streams and the accept path.
    /// A contended mutex proves a live service is inside a tick: configured.
    pub fn is_configured(&self) -> bool {
        match self.svc.try_lock() {
            Ok(g) => g.is_some(),
            Err(std::sync::TryLockError::WouldBlock) => true,
            Err(std::sync::TryLockError::Poisoned(_)) => false,
        }
    }

    /// Translate the wire profile and build the service over a freshly built station.
    ///
    /// The translation is deliberately THIN: rows map to rows, the schedule maps to
    /// the schedule, and every numeric rule (non-decreasing ladder, first-row anchor,
    /// band validity) is enforced by `RandomTestConfig::validate` inside
    /// `RandomService::new` - one validator, the same one the local app answers to.
    ///
    /// **The CROSS-MODULE exclusion is NOT here.** Whether some OTHER module owns the
    /// core is `Core::live_module`'s question, asked by the caller before this runs; a
    /// `recorder_live: bool` parameter used to thread one direction of that rule down
    /// into this host, which made a module-count fact into a Random implementation
    /// detail and left the other direction unexpressed. What stays here is the
    /// self-check below - only Random knows what "already live" means for Random.
    pub fn set_profile(
        &self,
        profile: RandomProfile,
        table: hertzforge_channel_table::ChannelTable,
        lease_owner: &str,
    ) -> Result<(), ProtocolError> {
        if self.is_live() {
            return Err(ProtocolError::Refused {
                message: "a Random run is live on this core; Stop it before reconfiguring".into(),
            });
        }

        // ---- breakpoints ----------------------------------------------------
        let mut bp = BreakpointTable::new("remote profile");
        for row in &profile.breakpoints {
            bp.rows.push(BreakpointRow {
                freq_hz: row.freq_hz,
                level: match row.level {
                    PsdLevel::Psd(v) => LevelSpec::Psd(v),
                    PsdLevel::SlopeDbPerOct(v) => LevelSpec::SlopeDbPerOct(v),
                },
                alarm_db: row.alarm_db,
                abort_db: row.abort_db,
            });
        }

        // ---- control channel: the table's first enabled Control row ---------
        let control_index = table
            .channels
            .iter()
            .find(|c| c.enabled && c.role == hertzforge_channel_table::ChannelRole::Control)
            .or_else(|| {
                table
                    .channels
                    .iter()
                    .find(|c| c.enabled && c.role != hertzforge_channel_table::ChannelRole::Drive)
            })
            .map(|c| c.index)
            .ok_or_else(|| ProtocolError::Refused {
                message: "the channel table has no enabled input channel to control on; \
                          edit the table (PutChannelTable) first"
                    .into(),
            })?;

        let mut cfg = RandomTestConfig::siso(bp, control_index);

        // ---- schedule -------------------------------------------------------
        let mode = match profile.ladder {
            LadderMode::Auto => RunMode::Auto,
            LadderMode::Manual => RunMode::Manual,
        };
        if !profile.schedule.is_empty() {
            let table_db: Vec<f64> = profile.schedule.iter().map(|e| e.db).collect();
            let dwell_s: Vec<f64> = profile
                .schedule
                .iter()
                .map(|e| e.dwell_s.unwrap_or(3.0))
                .collect();
            cfg.schedule = LevelSchedule {
                mode,
                start_db: table_db.first().copied().unwrap_or(-12.0),
                table_db,
                dwell_s,
            };
        } else {
            cfg.schedule = match mode {
                RunMode::Auto => LevelSchedule::auto_default(),
                RunMode::Manual => LevelSchedule::manual_default(),
            };
        }
        if let Some(d) = profile.duration_s {
            cfg.full_level_duration_s = d;
        }
        // Control parameters that are TEST CONFIG (as opposed to the
        // acquisition settings, which shape the loop). Only the ones with a
        // consuming engine are applied; the rest stay operator-visible as
        // config-only until their engine lands, and the panel says so.
        if let Some(c) = profile.control {
            if let Some(v) = c.max_drive_vrms {
                if v > 0.0 {
                    cfg.drive_ceiling_vrms = v;
                }
            }
            if let Some(n) = c.max_lines_out {
                cfg.max_lines_out = n;
            }
            if let Some(d) = c.damping {
                cfg.hinv.damping = d.clamp(0.0, 10.0);
            }
            // 2026-08-03 audit: the Hinv card + gate/pretest knobs used
            // to be edited in the UI and silently DEFAULTED here.
            if let Some(v) = c.coherence_floor {
                cfg.coherence_floor = v.clamp(0.0, 1.0);
            }
            if let Some(v) = c.trust_coherence_floor {
                cfg.hinv.trust_coherence_floor = v.clamp(0.0, 1.0);
            }
            if let Some(v) = c.trust_h_floor_frac {
                cfg.hinv.trust_h_floor_frac = v.clamp(0.0, 1.0);
            }
            if let Some(v) = c.max_correction_db_per_iter {
                if v > 0.0 {
                    cfg.hinv.max_correction_db_per_iter = v;
                }
            }
            if let Some(v) = c.lambda {
                cfg.hinv.lambda = if v > 0.0 { Some(v) } else { None };
            }
            if let Some(n) = c.divergence_n {
                if n >= 1 {
                    cfg.hinv.divergence_n = n;
                }
            }
            if let Some(n) = c.n_dof {
                // MIMO-forward: SISO is the rank-1 degenerate case; the field
                // is validated and carried so a MIMO station picks it up.
                cfg.hinv.svd_keep = if n >= 1 { Some(n) } else { None };
            }
            if let Some(v) = c.sysid_level_db {
                cfg.sysid_level_db = v.clamp(-60.0, 0.0);
            }
            if let Some(v) = c.max_sigma {
                if v > 0.0 {
                    cfg.drive_clip_sigma = Some(v.max(1.0));
                }
            }
            if let Some(v) = c.startup_ramp_s {
                cfg.level_ramp_s = v.max(0.0);
            }
            if let Some(v) = c.shutdown_ramp_s {
                cfg.shutdown_ramp_s = v.max(0.0);
            }
            if let Some(v) = c.max_pretest_drive_v {
                if v > 0.0 {
                    cfg.pretest_max_vrms = Some(v);
                }
            }
            if let Some(v) = c.min_pretest_drive_v {
                if v >= 0.0 {
                    cfg.pretest_min_vrms = Some(v);
                }
            }
            if let Some(b) = c.shaped_pretest {
                cfg.shaped_pretest = b;
            }
            if let Some(v) = c.soft_start_db {
                cfg.soft_start_db = v.max(0.0);
            }
            if let Some(v) = c.soft_start_relax_db {
                cfg.soft_start_relax_db = v.max(0.0);
            }
            if let Some(v) = c.control_dof {
                cfg.control_dof = v.max(2.0);
            }

            if let Some(n) = c.max_loops_out {
                if n >= 1 {
                    cfg.divergence_loops = n;
                }
            }
        }

        // ---- loop config + station (the Tauri hosted path, transcribed) -----
        // FEASIBILITY: rebuild the core's own types from the wire scalars. Absent =
        // no shaker, and the core skips the check with an advisory note - the same
        // honest outcome as a local run with no shaker selected, rather than the
        // SILENT skip every remote run used to get while the operator believed a
        // shaker was configured.
        if let Some(f) = profile.feasibility.as_ref() {
            match serde_json::from_str::<hertzforge_shaker_db::ShakerRecord>(&f.shaker_json) {
                Ok(rec) => {
                    tracing::info!(
                        target: "hertzforge::backendd",
                        shaker = %rec.name,
                        "feasibility inputs adopted from the wire"
                    );
                    cfg.shaker = Some(rec);
                    cfg.moving_mass = hertzforge_shaker_db::MovingMass {
                        armature_lb: f.armature_lb,
                        slip_table_lb: f.slip_table_lb,
                        drive_adapter_lb: f.drive_adapter_lb,
                        fixturing_lb: f.fixturing_lb,
                        test_article_lb: f.test_article_lb,
                    };
                }
                Err(e) => {
                    // REFUSE. Running with feasibility silently skipped is the
                    // exact failure this ticket closes.
                    return Err(ProtocolError::InvalidArgument {
                        message: format!(
                            "feasibility.shaker_json did not parse as a shaker record \
                             ({e}); refusing rather than running with the shaker-limit \
                             check silently skipped"
                        ),
                    });
                }
            }
        }
        // The operator's FRF pairs. Validated HERE and REFUSED on a bad
        // channel (audit A-2) - an unenabled response would otherwise publish
        // zeros for the life of the run and read as a dead sensor.
        let mut frf_reqs: Vec<hertzforge_random::service::FrfPairReq> = Vec::new();
        if let Some(pairs) = profile.frf_pairs.as_ref() {
            if pairs.len() > hertzforge_protocol::MAX_FRF_PAIRS {
                return Err(ProtocolError::InvalidArgument {
                    message: format!(
                        "{} FRF pairs requested; the cap is {} (each pair owns a                          3-id trace block in the {}..999 range)",
                        pairs.len(),
                        hertzforge_protocol::MAX_FRF_PAIRS,
                        hertzforge_protocol::FRF_PAIR_BASE
                    ),
                });
            }
            // Slot = position among the ENABLED, non-drive rows, which is the
            // order the segment interleaves.
            let slot_of = |idx: u32| -> Option<usize> {
                table
                    .channels
                    .iter()
                    .filter(|c| c.enabled && c.role != hertzforge_channel_table::ChannelRole::Drive)
                    .position(|c| c.index == idx)
            };
            for p in pairs {
                let response_slot = slot_of(p.response_ch).ok_or_else(|| {
                    ProtocolError::InvalidArgument {
                        message: format!(
                            "FRF pair response CH{} is not an enabled input channel;                              refusing rather than publishing an empty trace",
                            p.response_ch
                        ),
                    }
                })?;
                let (reference_slot, reference_ch) = match p.reference {
                    hertzforge_protocol::FrfSource::Drive => (None, None),
                    hertzforge_protocol::FrfSource::Channel(c) => (
                        Some(slot_of(c).ok_or_else(|| ProtocolError::InvalidArgument {
                            message: format!(
                                "FRF pair reference CH{c} is not an enabled input channel"
                            ),
                        })?),
                        Some(c),
                    ),
                };
                frf_reqs.push(hertzforge_random::service::FrfPairReq {
                    reference_slot,
                    response_slot,
                    reference_ch,
                    response_ch: p.response_ch,
                });
            }
            tracing::info!(
                target: "hertzforge::backendd",
                pairs = frf_reqs.len(),
                "FRF pairs adopted from the wire"
            );
        }

        // Captured OUTSIDE the control-block conditional: a profile without a
        // control block must reset to the default, not inherit the previous
        // profile's value.
        self.pretest_segments.store(
            profile
                .control
                .as_ref()
                .and_then(|c| c.pretest_frf_averages)
                .filter(|&n| n >= 1)
                .unwrap_or(SYSID_SEGMENTS) as u64,
            Ordering::SeqCst,
        );
        let (loop_cfg, n_inputs) = loop_from_config(&cfg, &table, profile.acquisition);
        // The REQUEST, taken from the built loop rather than from the wire
        // payload: `loop_from_config` is what decides whether a partial or
        // absent `acquisition` falls back to the core default, so reading the
        // payload here would report a rate the run is not using.
        self.requested_fs_bits
            .store(loop_cfg.fs_hz.to_bits(), Ordering::SeqCst);
        // The station has not answered yet. Nothing may stand in for it.
        self.actual_fs_bits.store(0, Ordering::SeqCst);
        // The loop ticks once per SEGMENT; the stall watchdog must be scaled to
        // that, not to a constant.
        self.expected_tick_ms.store(
            (loop_cfg.segment_seconds() * 1e3).round().max(0.0) as u64,
            Ordering::SeqCst,
        );
        // The notch tables, the control weights and the composite mode
        // now cross the wire. Interpolated HERE, against the grid this run will
        // actually use (`loop_cfg`), because the core skips a profile whose
        // length does not match its nbins - a sender-side interpolation would
        // be silently discarded the moment the operator changed a block size.
        if let Some(limits) = profile.limits.as_ref() {
            let nbins = loop_cfg.nbins();
            let df = loop_cfg.df();
            cfg.limit_profiles = limits
                .iter()
                .map(|lp| hertzforge_random::service::config::LimitProfile {
                    channel_index: lp.channel_index,
                    kind: if lp.kind.eq_ignore_ascii_case("force") {
                        hertzforge_random::service::config::LimitKind::Force
                    } else {
                        hertzforge_random::service::config::LimitKind::Response
                    },
                    resp_ceiling: resp_ceiling_from_rows(&lp.rows, nbins, df),
                })
                .collect();
            let notching: usize = cfg
                .limit_profiles
                .iter()
                .filter(|p| p.resp_ceiling.iter().any(|c| c.is_finite()))
                .count();
            tracing::info!(
                target: "hertzforge::backendd",
                profiles = cfg.limit_profiles.len(),
                notching,
                nbins,
                "limit profiles adopted from the wire"
            );
        }
        if let Some(w) = profile.control_weights.as_ref() {
            for (idx, weight) in w {
                if let Some(c) = cfg
                    .control
                    .channels
                    .iter_mut()
                    .find(|c| c.channel_index == *idx)
                {
                    c.weight = *weight;
                }
            }
        }
        if let Some(m) = profile.composite_mode.as_deref() {
            cfg.control.mode = match m {
                "min" => hertzforge_random::control::CompositeMode::Min,
                "max" => hertzforge_random::control::CompositeMode::Max,
                _ => hertzforge_random::control::CompositeMode::WeightedAvg,
            };
        }
        let spec = StationSpec {
            // The HERTZFORGE-SIM half is the local app's Random plant, transcribed
            // (`random_sim_plant` in `ui/src-tauri/src/random.rs`): the two-mode
            // resonance plant the Random acceptance gates were validated against.
            // `shared_clock: true` is the Sim's LEGITIMATE claim - a software plant
            // advances input and output on one synthetic sample index - and without
            // stating it the arm gate (correctly) refuses per-tick correction.
            sim: SimPlantSpec {
                sample_rate_hz: loop_cfg.fs_hz,
                resonances: vec![(200.0, 2.5), (550.0, 2.5)],
                noise_level: 2e-4,
                feedthrough: 0.35,
                shared_clock: true,
                seed: 0x5A5A_1234,
                max_input_channels: n_inputs.max(1),
                max_output_channels: 1,
                ..SimPlantSpec::default()
            },
            ni: NiSelection {
                inputs: hertzforge_station::physical_from_table(&table).0,
                outputs: hertzforge_station::physical_from_table(&table).1,
                derive_inputs: n_inputs.max(1) as usize,
                derive_outputs: 1,
                iepe: false,
                restrict_devices: self.ni_devices.clone(),
            },
            ni_block_frames: hertzforge_hal::default_block_frames(loop_cfg.fs_hz),
        };

        // THE ONE CONSTRUCTION POINT - a refusal is typed all the way to the remote
        // operator, never a Sim substitution.
        let station = try_build_station_backend(self.kind, &spec).map_err(|e| {
            tracing::warn!(target: "hertzforge::backendd", "random station refused: {e}");
            ProtocolError::Refused {
                message: e.to_string(),
            }
        })?;
        tracing::info!(target: "hertzforge::backendd", "{}", station.provenance.log_line());

        let run_id = mint_run_id();
        let prov = RunProvenance::assemble("random", run_id, &station, lease_owner, now_unix());
        tracing::info!(target: "hertzforge::backendd", "{}", prov.log_line());

        let mut svc = RandomService::new(
            cfg,
            table,
            loop_cfg,
            station.backend,
            station.station,
            Some(self.data_root.clone()),
        )
        .map_err(|e| ProtocolError::Refused {
            message: e.to_string(),
        })?;
        // The review hold is per-TEST policy, so it rides the profile.
        // Absent = the service keeps its own default (ON).
        if let Some(r) = profile.control.and_then(|c| c.review_after_sysid) {
            svc.set_review_after_sysid(r);
        }
        if !frf_reqs.is_empty() {
            svc.set_frf_pairs(frf_reqs);
        }

        *self.svc.lock().expect("svc poisoned") = Some(svc);
        *self.provenance.lock().expect("provenance poisoned") = Some(prov);
        Ok(())
    }

    /// Start the configured run and hand the tick cadence to the loop thread.
    ///
    /// The CHEAP refusals are synchronous - not configured, wrong state,
    /// feasibility exceeded, arm failed - so the operator still gets an
    /// immediate, specific "no" for everything that can be decided without
    /// running the plant.
    ///
    /// sysID is NOT. Its duration is `pretest_segments x nAvg x nPts / fs` and
    /// the front end waits a fixed 60 s for this call, so raising either
    /// acquisition knob used to blow past the timeout: the UI reported a crash
    /// while the daemon ran happily on (operator, 2026-08-03; zero panics in
    /// the journal, and BOTH his reported bugs - "crash when nAvg doesn't match
    /// pretest" and "refuses to arm in the 60 s window" - were this one
    /// mechanism). A command whose latency scales with a display setting is a
    /// broken contract, so the pretest now runs on the run thread and the state
    /// machine reports its progress (`SysId` -> `CoherenceGate` -> park or
    /// engage), which is what the status pane already renders.
    ///
    /// The cross-module exclusion is the CALLER's, through `Core::live_module` - see
    /// [`RandomHost::set_profile`] for why it stopped being a parameter here.
    pub fn start(&self) -> Result<(), ProtocolError> {
        {
            // Priming window: the beat is reset so the stalled-driver
            // math never reads the PREVIOUS run's heartbeat, and the flag makes
            // a contended status answer "priming" honestly. The guard clears
            // the flag on EVERY exit path, refusals included.
            self.last_tick_ms.store(now_ms(), Ordering::Relaxed);
            self.priming.store(true, Ordering::SeqCst);
            struct PrimingGuard<'a>(&'a AtomicBool);
            impl Drop for PrimingGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _priming = PrimingGuard(&self.priming);
            let mut guard = self.svc.lock().expect("svc poisoned");
            let svc = guard.as_mut().ok_or_else(|| ProtocolError::Refused {
                message: "Random is not configured on this core; send SetProfile first".into(),
            })?;
            match svc.state() {
                RunState::Idle => {
                    svc.assess_feasibility().map_err(svc_err)?;
                    if matches!(svc.state(), RunState::FeasibilityBlocked) {
                        return Err(ProtocolError::Refused {
                            message: "feasibility EXCEEDED: the run is parked at \
                                      FeasibilityBlocked. Review the profile against the \
                                      shaker limits (this seam has no override verb, \
                                      deliberately)."
                                .into(),
                        });
                    }
                    svc.arm().map_err(svc_err)?;
                    // ARM IS THE ONLY MOMENT THE ACTUAL RATE EXISTS. `arm()`
                    // opens the `PlantLoop`, which runs
                    // `LoopConfig::adopt_station_actual` and replaces the
                    // service's `fs_hz` with what the driver programmed. Read
                    // it here, on the success path only: a refused arm leaves
                    // the field cleared, which reports honestly as "not known".
                    self.actual_fs_bits
                        .store(svc.loop_config().fs_hz.to_bits(), Ordering::SeqCst);
                    // Everything past arm() is UNBOUNDED in duration (the
                    // pretest) and runs on the run thread - see the fn doc.
                    // Answering here means ACCEPTED: armed, pretest under way.
                }
                RunState::Engaged | RunState::FullLevel => { /* already engaged */ }
                other => {
                    return Err(ProtocolError::Refused {
                        message: format!(
                            "Random cannot start from {other:?}; reconfigure \
                                          (SetProfile) to reset"
                        ),
                    });
                }
            }
        }
        // Re-raise for the ASYNC half: the guard above cleared it on the way
        // out of the sync block, but the pretest is still ahead of us. The run
        // thread clears it when priming finishes.
        self.priming.store(true, Ordering::SeqCst);
        self.spawn_pump();
        Ok(())
    }

    /// Absolute level intent. Db targets jump the schedule ladder to the matching
    /// rung; anything else is refused with the ladder printed.
    pub fn set_level(&self, level: LevelTarget) -> Result<f64, ProtocolError> {
        let db = match level {
            LevelTarget::Db(db) => db,
            LevelTarget::DriveVolts(_) => {
                return Err(ProtocolError::InvalidArgument {
                    message: "Random levels are dB relative to full level; DriveVolts is \
                              an open-loop-sine base"
                        .into(),
                })
            }
        };
        let mut guard = self.svc.lock().expect("svc poisoned");
        let svc = guard.as_mut().ok_or_else(not_configured)?;
        let ladder: Vec<f64> = svc.config().schedule.table_db.clone();
        let idx = ladder
            .iter()
            .position(|&t| (t - db).abs() < 1e-9)
            .ok_or_else(|| ProtocolError::InvalidArgument {
                message: format!(
                    "level {db} dB is not a rung of the configured ladder {ladder:?}; \
                     use AdjustLevel for relative steps"
                ),
            })?;
        svc.jump_to_table_level(idx).map_err(svc_err)
    }

    /// Relative level intent.
    pub fn adjust_level(&self, delta_db: f64) -> Result<f64, ProtocolError> {
        let mut guard = self.svc.lock().expect("svc poisoned");
        let svc = guard.as_mut().ok_or_else(not_configured)?;
        svc.step_level(delta_db).map_err(svc_err)
    }

    /// Release a run holding for operator review.
    pub fn proceed_after_review(&self) -> Result<(), ProtocolError> {
        let mut guard = self.svc.lock().expect("svc poisoned");
        let svc = guard.as_mut().ok_or_else(not_configured)?;
        svc.proceed_after_review().map_err(svc_err)?;
        drop(guard);
        // The pump exited when the run parked at the hold; engaging again needs
        // it back.
        self.spawn_pump();
        Ok(())
    }

    /// LIVE ladder-mode takeover, both directions. Level-neutral by the
    /// service's contract: `Auto -> Manual` freezes the schedule where it is;
    /// `Manual -> Auto` re-engages from the operator's current level with the
    /// dwell restarted.
    pub fn set_ladder_mode(
        &self,
        ladder: hertzforge_protocol::LadderMode,
    ) -> Result<(), ProtocolError> {
        let mode = match ladder {
            hertzforge_protocol::LadderMode::Auto => {
                hertzforge_random::service::config::RunMode::Auto
            }
            hertzforge_protocol::LadderMode::Manual => {
                hertzforge_random::service::config::RunMode::Manual
            }
        };
        let mut guard = self.svc.lock().expect("svc poisoned");
        let svc = guard.as_mut().ok_or_else(not_configured)?;
        svc.set_run_mode(mode).map_err(svc_err)
    }

    /// The controlled stop (the operator's normal end-of-test).
    pub fn stop(&self) -> Result<(), ProtocolError> {
        // CONTROLLED stop: when a shutdown ramp is configured and
        // the run is driving, `request_stop` arms the ramp-down and the LOOP
        // walks itself to the floor and stops - killing the pump first (the
        // old order) would leave nobody to walk the ramp. Aborts do not come
        // through here and stay immediate.
        {
            let mut guard = self.svc.lock().expect("svc poisoned");
            match guard.as_mut() {
                Some(s) => {
                    s.request_stop().map_err(svc_err)?;
                    if matches!(s.state(), RunState::Engaged | RunState::FullLevel) {
                        // Ramp armed: the loop thread finishes the descent and
                        // exits on the Stopped transition. Do NOT set the stop
                        // flag (it would halt the walk mid-ramp).
                        return Ok(());
                    }
                }
                None => return Ok(()),
            }
        }
        // Stopped immediately (no ramp / nothing driving): reap the pump.
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
        Ok(())
    }

    /// Drop a BUILT-BUT-IDLE Random run so this host stops holding the core.
    ///
    /// Called only by [`crate::core::Core::release_idle_claims`], which asks
    /// `holds(Random, Live)` first. The guard below is still real: `is_live` is the
    /// authority on what "idle" means for Random and a caller that got it wrong must
    /// not silently drop a driving station.
    pub fn release_idle(&self) -> bool {
        if self.is_live() || !self.is_configured() {
            return false;
        }
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
        // The drop COMPLETES here, releasing the station's tasks, before the module
        // that displaced this one builds its own.
        *self.svc.lock().expect("svc poisoned") = None;
        self.stop.store(false, Ordering::SeqCst);
        if let Ok(mut c) = self.status_cache.lock() {
            *c = None;
        }
        if let Ok(mut p) = self.provenance.lock() {
            *p = None;
        }
        self.requested_fs_bits.store(0, Ordering::SeqCst);
        self.actual_fs_bits.store(0, Ordering::SeqCst);
        self.last_tick_ms.store(0, Ordering::Relaxed);
        self.expected_tick_ms.store(0, Ordering::SeqCst);
        true
    }

    /// THE SAFE-STOP PATH, invoked when the controller times out. Same rule as the
    /// recorder's: "nobody is watching" is a reason to stop SAFELY (the controlled
    /// ramp-down, which on the NI backend is the measured taper), never to slam the
    /// drive. Never fails.
    pub fn safe_stop(&self, reason: &str) {
        if !self.is_live() {
            return;
        }
        tracing::warn!(target: "hertzforge::backendd", reason, "SAFE-STOP: ramping the Random run down");
        if let Err(e) = self.stop() {
            tracing::warn!(target: "hertzforge::backendd", "random safe-stop: {e}");
        }
    }

    /// Fold this module's truth into a protocol status report.
    ///
    /// TRY-locks the service: the pump holds this mutex across driver
    /// calls, and an NI runtime stall (observed 2026-08-01: an ioctl parked in
    /// libnipalu BELOW DAQmx's own finite read timeout, for minutes) would
    /// otherwise wedge every status poll - the operator saw a frozen IDLE card
    /// while a run was engaged. If the lock is contended AND the pump's
    /// heartbeat is stale, the report SAYS SO instead of hanging the wire.
    pub fn fill_status(&self, s: &mut hertzforge_protocol::StatusReport) {
        // BEFORE the lock, so the rate pair survives every early return below -
        // priming, stalled-driver, and the bounded-wait miss all describe a run
        // whose rates are already decided, and a status that drops them there
        // is exactly the window an operator stares at while a run arms.
        self.fill_rates(s);
        let guard = match self.svc.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::WouldBlock) => {
                if self.priming.load(Ordering::SeqCst) {
                    // start() holds the mutex through feasibility/sysID/gate: say so.
                    s.run = hertzforge_protocol::RunState::Armed;
                    s.module_state = Some(
                        "priming (feasibility / sysid / coherence gate - the run \
                         engages when it passes)"
                            .into(),
                    );
                    return;
                }
                let beat = self.last_tick_ms.load(Ordering::Relaxed);
                let stale_ms = now_ms().saturating_sub(beat);
                // THREE MISSED TICKS, never a constant. One segment is
                // `nAvg x nPts / fs` seconds - 2.0 s at the shipped default and
                // 8 s at nPts 32768 - so a fixed 3 s threshold reported a
                // perfectly healthy loop as a stalled driver as soon as the
                // operator touched the acquisition settings (2026-08-03). The
                // 3 s floor stays for the case where no cadence is known yet.
                let period = self.expected_tick_ms.load(Ordering::SeqCst).max(1);
                let stall_after = (3 * period).max(3_000);
                if beat != 0 && stale_ms > stall_after {
                    s.run = hertzforge_protocol::RunState::Faulted;
                    s.module_state = Some(format!(
                        "stalled_driver ({:.1} s without a loop tick, expected one every \
                         {:.1} s - the NI runtime is not returning; restart the daemon, \
                         and the chassis if preflight then fails)",
                        stale_ms as f64 / 1e3,
                        period as f64 / 1e3
                    ));
                } else if let Some(st) = self.status_cache.lock().ok().and_then(|c| c.clone()) {
                    // The pump is mid-tick (it holds the mutex for nearly the
                    // whole tick on hardware): serve its last completed
                    // tick's snapshot - at most one tick old, which is exactly
                    // what a display poll wants - instead of "busy" with no
                    // numbers.
                    apply_status(&st, s);
                } else {
                    // No snapshot yet (the pump has not completed its first
                    // tick). A BOUNDED wait - up to ~50 ms of 1 ms retries -
                    // covers the poll-right-after-Start window without ever
                    // recreating the unbounded starvation this path exists to
                    // prevent.
                    for _ in 0..50 {
                        std::thread::sleep(Duration::from_millis(1));
                        if let Ok(g) = self.svc.try_lock() {
                            if let Some(svc) = g.as_ref() {
                                apply_status(&svc.status(), s);
                            }
                            return;
                        }
                        if let Some(st) = self.status_cache.lock().ok().and_then(|c| c.clone()) {
                            apply_status(&st, s);
                            return;
                        }
                    }
                    s.module_state = Some("busy".into());
                }
                return;
            }
            Err(std::sync::TryLockError::Poisoned(_)) => return,
        };
        let Some(svc) = guard.as_ref() else { return };
        let st = svc.status();
        apply_status(&st, s);
    }

    /// The requested/actual sample-rate pair, lock-free. `0` bits = unknown,
    /// which crosses the wire as `None` rather than as a number.
    fn fill_rates(&self, s: &mut hertzforge_protocol::StatusReport) {
        let read = |bits: &AtomicU64| -> Option<f64> {
            let v = f64::from_bits(bits.load(Ordering::SeqCst));
            (v.is_finite() && v > 0.0).then_some(v)
        };
        s.requested_fs_hz = read(&self.requested_fs_bits);
        s.actual_fs_hz = read(&self.actual_fs_bits);
    }
}

/// Map one service status snapshot onto the wire report - shared by the
/// direct-lock path and the pump-snapshot path, so the two can never
/// drift.
fn apply_status(
    st: &hertzforge_random::service::report::Status,
    s: &mut hertzforge_protocol::StatusReport,
) {
    {
        // The module's OWN state, in the EXACT strings the front end's
        // RandomRunState union speaks (the Tauri layer's `state_str` mapping,
        // duplicated here because the daemon may not depend on the Tauri
        // crate). The front end renders this verbatim instead of guessing a
        // fine state from the coarse lifecycle - the guess showed FULL LEVEL
        // while engaged at -12 dB (2026-08-01).
        s.module_state = Some(
            match st.state {
                RunState::Idle => "idle",
                RunState::Feasibility => "feasibility",
                RunState::FeasibilityBlocked => "feasibility_blocked",
                RunState::SysId => "sysid",
                RunState::CoherenceGate => "coherence_gate",
                RunState::CoherenceHold => "coherence_hold",
                RunState::AwaitingReview => "awaiting_review",
                RunState::Engaged => "engaged",
                RunState::FullLevel => "full_level",
                RunState::Stopped => "stopped",
                RunState::Aborted => "aborted",
            }
            .to_string(),
        );
        s.run = match st.state {
            RunState::Idle => hertzforge_protocol::RunState::Configured,
            RunState::Feasibility
            | RunState::FeasibilityBlocked
            | RunState::SysId
            | RunState::CoherenceGate
            | RunState::CoherenceHold
            | RunState::AwaitingReview => hertzforge_protocol::RunState::Armed,
            RunState::Engaged | RunState::FullLevel => hertzforge_protocol::RunState::Running,
            RunState::Stopped => hertzforge_protocol::RunState::Stopped,
            RunState::Aborted => hertzforge_protocol::RunState::Faulted,
        };
        s.level_db = Some(st.current_level_db);
        s.target_grms = Some(st.target_grms);
        s.band_coherence = Some(st.band_coherence);
        s.measured_grms = st.measured_grms;
        s.alarm_active = st.alarm_active;
        s.abort_active = st.abort_active;
        s.elapsed_s = Some(st.elapsed_at_level_s);
        s.remaining_s = st.remaining_at_level_s;
        // Who is advancing the schedule RIGHT NOW: live-switchable, so
        // the panel renders this rather than its own last request.
        s.ladder_mode = Some(match st.run_mode {
            hertzforge_random::service::config::RunMode::Auto => {
                hertzforge_protocol::LadderMode::Auto
            }
            hertzforge_random::service::config::RunMode::Manual => {
                hertzforge_protocol::LadderMode::Manual
            }
        });
    }
}

impl RandomHost {
    /// The loop thread: step `loop_tick`, publish the display traces, exit when the
    /// run leaves the driving states.
    fn spawn_pump(&self) {
        let mut slot = self.pump.lock().expect("pump poisoned");
        // A FINISHED handle is not a running pump (read live 2026-08-01):
        // a run that ends on its own leaves its JoinHandle in the slot - only
        // stop() used to take it - and the next run's spawn saw "already
        // pumping" and returned. The service engaged, the level servo answered
        // the operator's steps, and not one loop ticked: no spectra, no
        // measurement, no control. Reap a finished handle and spawn; only a
        // LIVE pump short-circuits.
        if let Some(h) = slot.take() {
            if h.is_finished() {
                let _ = h.join();
            } else {
                *slot = Some(h);
                return; // genuinely already pumping
            }
        }
        self.stop.store(false, Ordering::SeqCst);
        if let Ok(mut c) = self.status_cache.lock() {
            *c = None; // a fresh run must never serve the previous run's snapshot
        }
        // ---- the LIVE display tap --------------------------------------
        // Publish the time domain at the ENGINE's tick rate instead of once
        // per control segment. A segment is many ticks long, so the old path
        // handed the UI ~1.4 s of samples in one burst every ~1.4 s: the pane
        // flashed chunks and could never be near-real-time, which the operator
        // needs because run decisions are made off that display.
        //
        // This runs INSIDE the control hot path, so it is deliberately cheap:
        // decimate into a reused buffer and publish on the existing broadcast
        // (non-blocking; a lagging subscriber drops frames, it never stalls
        // the loop). The control math is untouched - the same samples still
        // accumulate into the same segment.
        {
            let tx = self.tx.clone();
            let run_id_obs = self
                .provenance()
                .and_then(|p| p.run_id.parse::<u64>().ok())
                .unwrap_or(0);
            let fs_obs = self
                .svc
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(|s| s.loop_fs_hz()))
                .unwrap_or(0.0);
            // Run-relative frame clock for the tap, independent of the
            // segment-boundary clock so the two never fight.
            let cum = Arc::new(AtomicU64::new(0));
            let observer: hertzforge_random::service::engine::BlockObserver =
                Arc::new(move |input: &[f32], n_frames: usize| {
                    if n_frames == 0 || fs_obs <= 0.0 {
                        return;
                    }
                    let n_ch = (input.len() / n_frames).max(1);
                    let end = cum.fetch_add(n_frames as u64, Ordering::Relaxed) + n_frames as u64;
                    if let Some(wb) = wave_batch(input, n_ch, fs_obs, run_id_obs, end) {
                        let _ = tx.send(DisplayBatch::Wave(wb));
                    }
                });
            if let Ok(mut g) = self.svc.lock() {
                if let Some(s) = g.as_mut() {
                    s.set_block_observer(Some(observer));
                }
            }
        }
        let svc = Arc::clone(&self.svc);
        let stop = Arc::clone(&self.stop);
        let tx = self.tx.clone();
        let beat = Arc::clone(&self.last_tick_ms);
        let cache = Arc::clone(&self.status_cache);
        let run_id = self
            .provenance()
            .and_then(|p| p.run_id.parse::<u64>().ok())
            .unwrap_or(0);

        let priming = Arc::clone(&self.priming);
        let pretest_segs = self.pretest_segments.load(Ordering::SeqCst).max(1) as usize;

        *slot = Some(std::thread::spawn(move || {
            // ---- PRIMING PHASE (async since 2026-08-03) --------------------
            // The pretest runs HERE, not in the `start` command, because its
            // duration scales with the operator's acquisition settings and the
            // front end waits a fixed 60 s for that call. See `start`'s doc.
            {
                let mut guard = match svc.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                if let Some(s) = guard.as_mut() {
                    // `arm()` leaves the service at SysId - the pretest is the
                    // next thing owed, and this thread is what owes it.
                    if matches!(s.state(), RunState::SysId) {
                        // The operator watches sysID HAPPEN: the display tap is
                        // installed BEFORE the pretest, so the time panes stream
                        // it live instead of sitting blank until it finishes.
                        let cum = Arc::new(AtomicU64::new(0));
                        let fs_obs = s.loop_fs_hz();
                        let txo = tx.clone();
                        let observer: hertzforge_random::service::engine::BlockObserver =
                            Arc::new(move |input: &[f32], n_frames: usize| {
                                if n_frames == 0 || fs_obs <= 0.0 {
                                    return;
                                }
                                let n_ch = (input.len() / n_frames).max(1);
                                let end = cum.fetch_add(n_frames as u64, Ordering::Relaxed)
                                    + n_frames as u64;
                                if let Some(wb) = wave_batch(input, n_ch, fs_obs, run_id, end) {
                                    let _ = txo.send(DisplayBatch::Wave(wb));
                                }
                            });
                        s.set_block_observer(Some(observer));

                        if let Err(e) = s.run_sysid(pretest_segs) {
                            tracing::warn!(target: "hertzforge::backendd", "sysID failed: {e}");
                            s.fault_from_loop(&e.to_string());
                            if let Ok(mut c) = cache.lock() {
                                *c = Some(s.status());
                            }
                            priming.store(false, Ordering::SeqCst);
                            return;
                        }
                        let gate = s.coherence_gate();
                        // Publish the sysID RESULTS on the reserved FRF ids
                        // (930 |H1| / 931 coherence) - the FRF pane's native
                        // feed - on PASS and FAIL alike: a failed gate is when
                        // the operator most needs to see where coherence went.
                        let coh = s.coherence_curve().to_vec();
                        if !coh.is_empty() {
                            let mag: Vec<f32> = s
                                .frf_mag2()
                                .iter()
                                .map(|&m2| (m2.max(0.0) as f32).sqrt())
                                .collect();
                            let df = s.report_data().target_psd.df_hz;
                            let _ = tx.send(DisplayBatch::Spectrum(SpectrumBatch {
                                f0_hz: 0.0,
                                df_hz: df,
                                window_seq: 0,
                                run_id,
                                traces: vec![
                                    (crate::recorder::CH_FRF_MAG, None, Arc::new(mag)),
                                    (
                                        crate::recorder::CH_FRF_COH,
                                        None,
                                        Arc::new(
                                            coh.iter().map(|&c| c as f32).collect::<Vec<f32>>(),
                                        ),
                                    ),
                                ],
                            }));
                        }
                        // The alignment diagnostics are in the event
                        // log; mirror them to the journal so a bad FRF can be
                        // attributed without a UI round-trip.
                        for ev in s.log().events().iter().rev().take(14) {
                            let t = format!("{ev:?}");
                            if t.contains("sysID align")
                                || t.contains("pretest record")
                                || t.contains("sysID drive check")
                                || t.contains("sysID response check")
                            {
                                tracing::info!(target: "hertzforge::backendd", "{t}");
                            }
                        }
                        if let Err(e) = gate {
                            tracing::warn!(target: "hertzforge::backendd", "coherence gate: {e}");
                            s.fault_from_loop(&e.to_string());
                        }
                        if let Ok(mut c) = cache.lock() {
                            *c = Some(s.status());
                        }
                    }
                }
            }
            priming.store(false, Ordering::SeqCst);
            // Parked (review hold / coherence hold / fault)? The tick loop's
            // own state gate would exit immediately; leaving now is the same
            // outcome without a wasted lock acquisition. `proceed_after_review`
            // re-spawns this thread for the driving phase.
            if let Ok(g) = svc.lock() {
                if let Some(s) = g.as_ref() {
                    if !matches!(s.state(), RunState::Engaged | RunState::FullLevel) {
                        return;
                    }
                }
            }

            let mut ticks = 0u64;
            // Run-relative frames pumped so far (the t_end_s clock; the local
            // Tauri packer's `samples_seen` analog).
            let mut cum_frames: u64 = 0;
            let mut window_seq: u32 = 0;
            while !stop.load(Ordering::Relaxed) && ticks < MAX_LOOP_TICKS {
                let started = Instant::now();
                let batch = {
                    let mut guard = match svc.lock() {
                        Ok(g) => g,
                        Err(_) => break,
                    };
                    let Some(s) = guard.as_mut() else { break };
                    if !matches!(s.state(), RunState::Engaged | RunState::FullLevel) {
                        break;
                    }
                    match s.loop_tick() {
                        Ok(more) => {
                            // MEASUREMENT: the drive-continuity numbers for
                            // the segment this tick just ran, at their source.
                            if let Some(t) = s.last_segment_timing() {
                                tracing::info!(
                                    target: "hertzforge::backendd",
                                    gap_ms = format!("{:.1}", t.gap_ms),
                                    ran_ms = format!("{:.1}", t.ran_ms),
                                    zero_filled = t.zero_filled,
                                    zero_ms = format!("{:.1}", t.zero_ms),
                                    synth_ms = format!("{:.1}", t.synth_ms),
                                    pre_ms = format!("{:.1}", t.pre_ms),
                                    boundary = format!("{:?}", s.last_boundary()),
                                    "segment timing"
                                );
                                let w = s.last_driver_warnings();
                                if !w.is_empty() {
                                    tracing::warn!(
                                        target: "hertzforge::backendd",
                                        "DRIVER diagnostics this segment: {}",
                                        w.join(" | ")
                                    );
                                }
                            }
                            // Publish this tick's status snapshot: the
                            // status path serves it whenever this thread holds
                            // the svc mutex, which on hardware is nearly always.
                            if let Ok(mut c) = cache.lock() {
                                *c = Some(s.status());
                            }
                            window_seq = window_seq.wrapping_add(1);
                            let b = spectra_batch(s, window_seq, run_id);
                            // The raw time-domain views of the SAME segment:
                            // envelope columns + decimated samples.
                            // `cum_frames` is the RUN-relative frame count
                            // through this segment - the batches stamp
                            // t_end_s from it. The first version stamped the
                            // SEGMENT length (constant every tick), which
                            // froze every remote time pane's follow window
                            // and built a sawtooth time base in the UI rings
                            // (48/48 wire frames measured carrying
                            // tEnd=1.4400; the contract in recorder.rs says
                            // run-relative).
                            if let Some((raw, n_ch, fs)) = s.last_segment() {
                                cum_frames += (raw.len() / n_ch.max(1)) as u64;
                                if let Some(sb) = scope_batch(raw, n_ch, fs, run_id, cum_frames) {
                                    let _ = tx.send(DisplayBatch::Scope(sb));
                                }
                                // NOTE: the WAVE stream is published by the
                                // per-tick display tap installed in
                                // spawn_pump, at the engine's tick rate.
                                // Publishing it again here would duplicate
                                // samples on the same time base.
                            }
                            if !more {
                                // The run ended this tick (duration elapsed, abort, ...):
                                // publish the final traces, then exit.
                                if let Some(b) = b {
                                    let _ = tx.send(DisplayBatch::Spectrum(b));
                                }
                                break;
                            }
                            b
                        }
                        Err(e) => {
                            tracing::warn!(target: "hertzforge::backendd", "random loop faulted: {e}");
                            // The loop is dead - say so on the wire.
                            // Before this, the state stayed `engaged` after the
                            // thread exited, and a run that drove nothing and
                            // streamed nothing reported itself healthy for the
                            // rest of the session.
                            s.fault_from_loop(&e.to_string());
                            if let Ok(mut c) = cache.lock() {
                                *c = Some(s.status());
                            }
                            break;
                        }
                    }
                };
                if let Some(b) = batch {
                    let _ = tx.send(DisplayBatch::Spectrum(b));
                }
                beat.store(now_ms(), Ordering::Relaxed);
                ticks += 1;
                // The service's own tick pacing dominates on hardware (the segment
                // read waits on the driver); the TICK top-up throttles the Sim
                // path. The FLOOR: std's mutex is an unfair futex, and a loop that
                // re-acquires microseconds after releasing starves every parked
                // waiter for the length of the run (measured: a status dispatch
                // waited 29 s and froze its whole session select). One yielded
                // millisecond per tick hands the mutex to whoever is parked (level
                // intents, stop, the rare direct status win) at ~2% duty cost.
                const RELEASE_FLOOR: Duration = Duration::from_millis(1);
                let elapsed = started.elapsed();
                let pause = if elapsed < TICK {
                    TICK - elapsed
                } else {
                    RELEASE_FLOOR
                };
                std::thread::sleep(pause);
            }
            // WHY did the loop end? An abort's trip record used to live only in
            // the in-memory event log, so a tripped run read as an unexplained
            // exit in the journal (2026-08-03: three deploy cycles blind).
            if let Ok(g) = svc.lock() {
                if let Some(s) = g.as_ref() {
                    if matches!(s.state(), hertzforge_random::RunState::Aborted) {
                        tracing::warn!(
                            target: "hertzforge::backendd",
                            trip = format!("{:?}", s.trip()),
                            "random loop ended ABORTED"
                        );
                        for ev in s.log().events().iter().rev().take(6) {
                            tracing::warn!(target: "hertzforge::backendd", "  abort-tail event: {ev:?}");
                        }
                    }
                }
            }
            tracing::info!(target: "hertzforge::backendd", "random loop thread exited");
        }));
    }
}

impl Drop for RandomHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.pump.lock().expect("pump poisoned").take() {
            let _ = h.join();
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn not_configured() -> ProtocolError {
    ProtocolError::Refused {
        message: "Random is not configured on this core; send SetProfile first".into(),
    }
}

fn svc_err(e: hertzforge_random::ServiceError) -> ProtocolError {
    ProtocolError::Refused {
        message: e.to_string(),
    }
}

/// The loop config for a profile + table - `ui/src-tauri/src/random.rs`'s
/// Interpolate authored limit rows onto the run's OWN line grid.
///
/// Log-log between breakpoints - the same convention the reference profile
/// resolves with, and a byte-for-byte mirror of the front end's
/// `respCeilingFromRows` (ui/src/lib/random/limits.ts), which is the authority
/// for the semantics: outside the authored band a line is UNCONSTRAINED, a
/// single row is a flat ceiling across the grid, and no usable rows means no
/// limit anywhere.
///
/// The grid comes from the RUN (nbins, df), never from the sender: the core
/// skips a profile whose length does not match its nbins, so a client-side
/// interpolation against an assumed grid is silently discarded - exactly the
/// class of failure the fidelity fence exists to end.
fn resp_ceiling_from_rows(
    rows: &[hertzforge_protocol::LimitRow],
    nbins: usize,
    df_hz: f64,
) -> Vec<f64> {
    let mut pts: Vec<(f64, f64)> = rows
        .iter()
        .filter(|r| {
            r.freq_hz.is_finite() && r.freq_hz > 0.0 && r.ceiling.is_finite() && r.ceiling > 0.0
        })
        .map(|r| (r.freq_hz, r.ceiling))
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = vec![f64::INFINITY; nbins];
    if pts.is_empty() {
        return out;
    }
    if pts.len() == 1 {
        out.fill(pts[0].1);
        return out;
    }
    let f_lo = pts[0].0;
    let f_hi = pts[pts.len() - 1].0;
    let mut seg = 0usize;
    for (k, o) in out.iter_mut().enumerate() {
        let f = k as f64 * df_hz;
        if f < f_lo || f > f_hi {
            continue;
        }
        while seg + 2 < pts.len() && f > pts[seg + 1].0 {
            seg += 1;
        }
        let (fa, ca) = pts[seg];
        let (fb, cb) = pts[seg + 1];
        *o = if f <= fa {
            ca
        } else if f >= fb {
            cb
        } else {
            let t = (f / fa).ln() / (fb / fa).ln();
            (ca.ln() + t * (cb / ca).ln()).exp()
        };
    }
    out
}

/// `loop_from_config`, transcribed (the daemon may not depend on the Tauri crate).
fn loop_from_config(
    cfg: &RandomTestConfig,
    table: &hertzforge_channel_table::ChannelTable,
    acquisition: Option<hertzforge_protocol::AcquisitionProps>,
) -> (LoopConfig, u32) {
    let mut plant = LoopConfig::demo_default();
    // The operator's acquisition choices, when the front end sent them.
    // Absent = keep the core's defaults; never invent a rate from a partial
    // payload. The station may still COERCE the rate (the 9234 rides a
    // 13.1072 MHz/(256n) ladder) - what it actually programmed comes back
    // through the adoption path and is what the run reports.
    if let Some(a) = acquisition {
        if a.fs_hz > 0.0 {
            plant.fs_hz = a.fs_hz;
        }
        // The 2026-08-02 interim clamp to 1024 is REMOVED: the stall it worked
        // around was the sysID gate's O(n^2) lag search starving the AO between
        // the last pretest drive and the first control drive. The
        // search is now bounded (fixed correlation window, coarse+refine
        // ladder), so the gate cost no longer scales with the block size.
        // Hardware re-proof of the full nPts table is the merge gate for this
        // removal.
        if a.n_pts >= 4 && a.n_pts.is_power_of_two() {
            plant.nfft = a.n_pts;
        }
        if a.n_avg >= 1 {
            plant.blocks_per_window = a.n_avg;
        }
    }
    if let Ok(anchors) = cfg.breakpoints.resolve() {
        if let (Some(first), Some(last)) = (anchors.first(), anchors.last()) {
            let nyq = plant.fs_hz / 2.0;
            let lo = first.freq_hz.max(plant.df());
            let hi = last.freq_hz.min(nyq * 0.95);
            if hi > lo {
                plant.band_lo_hz = lo;
                plant.band_hi_hz = hi;
            }
        }
    }
    let n_inputs = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role != hertzforge_channel_table::ChannelRole::Drive)
        .count()
        .max(1) as u32;
    (plant, n_inputs)
}

/// One display batch from the live service: reference + bands (level-scaled),
/// composite, drive - the same traces, on the SAME reserved ids, as the local app's
/// overlay (`spectra_frames` in `ui/src-tauri/src/random.rs`).
fn spectra_batch(svc: &RandomService, window_seq: u32, run_id: u64) -> Option<SpectrumBatch> {
    let report = svc.report_data();
    let status = svc.status();
    let df = report.target_psd.df_hz;
    if df <= 0.0 || report.target_psd.psd.is_empty() {
        return None;
    }
    let mut traces: Vec<(u32, Option<u32>, Arc<Vec<f32>>)> = Vec::new();

    let level_scale = 10f64.powf(status.current_level_db / 10.0);
    let scaled = |band_db: f64| -> Arc<Vec<f32>> {
        Arc::new(
            report
                .target_psd
                .psd
                .iter()
                .map(|&r| {
                    if r > 0.0 {
                        (r * level_scale * 10f64.powf(band_db / 10.0)) as f32
                    } else {
                        0.0
                    }
                })
                .collect(),
        )
    };

    traces.push((CH_REFERENCE, None, scaled(0.0)));
    let (alarm_m, alarm_p) = report.settings.alarm_db;
    let (abort_m, abort_p) = report.settings.abort_db;
    traces.push((CH_ALARM_HI, None, scaled(alarm_p)));
    traces.push((CH_ALARM_LO, None, scaled(-alarm_m)));
    traces.push((CH_ABORT_HI, None, scaled(abort_p)));
    traces.push((CH_ABORT_LO, None, scaled(-abort_m)));

    if let Some(c) = &report.final_composite_psd {
        traces.push((
            CH_COMPOSITE,
            None,
            Arc::new(c.psd.iter().map(|&p| p.max(0.0) as f32).collect()),
        ));
    }
    if let Some(d) = &report.final_drive_psd {
        traces.push((
            CH_DRIVE,
            None,
            Arc::new(d.psd.iter().map(|&p| p.max(0.0) as f32).collect()),
        ));
    }
    // One 3-id block per requested FRF pair - magnitude, coherence,
    // phase - starting at FRF_PAIR_BASE. Deliberately NOT 930+3k: those are
    // the RECORDER's reserved FRF ids and pair 0 would have collided.
    for (k, r) in svc.frf_results().iter().enumerate() {
        if r.mag.is_empty() {
            continue;
        }
        let base = hertzforge_protocol::FRF_PAIR_BASE + 3 * k as u32;
        traces.push((
            base,
            None,
            Arc::new(r.mag.iter().map(|&v| v as f32).collect()),
        ));
        traces.push((
            base + 1,
            None,
            Arc::new(r.coherence.iter().map(|&v| v as f32).collect()),
        ));
        traces.push((
            base + 2,
            None,
            Arc::new(r.phase.iter().map(|&v| v as f32).collect()),
        ));
    }
    // Per-channel measured PSDs on their REAL channel indices.
    for (i, line) in report.final_per_channel_psds.iter().enumerate() {
        let ch = report
            .settings
            .control_indices
            .get(i)
            .copied()
            .unwrap_or((i + 1) as u32);
        traces.push((
            ch,
            None,
            Arc::new(line.psd.iter().map(|&p| p.max(0.0) as f32).collect()),
        ));
    }

    Some(SpectrumBatch {
        f0_hz: report.target_psd.f0_hz,
        df_hz: df,
        window_seq,
        run_id,
        traces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_protocol::{PsdBreakpoint, ScheduleEntry};

    fn profile() -> RandomProfile {
        RandomProfile {
            frf_pairs: None,
            feasibility: None,
            schema_version: hertzforge_protocol::PROFILE_SCHEMA,
            limits: None,
            control_weights: None,
            composite_mode: None,
            breakpoints: vec![
                PsdBreakpoint {
                    freq_hz: 20.0,
                    level: PsdLevel::Psd(0.01),
                    alarm_db: None,
                    abort_db: None,
                },
                PsdBreakpoint {
                    freq_hz: 2000.0,
                    level: PsdLevel::Psd(0.01),
                    alarm_db: None,
                    abort_db: None,
                },
            ],
            schedule: vec![
                ScheduleEntry {
                    db: -12.0,
                    dwell_s: Some(0.5),
                },
                ScheduleEntry {
                    db: 0.0,
                    dwell_s: Some(0.5),
                },
            ],
            ladder: LadderMode::Auto,
            duration_s: Some(1.0),
            acquisition: None,
            // These tests exercise the DRIVING path, so they opt out of the
            // Review hold. The hold's own behaviour (and its default-ON
            // policy) is covered by the service acceptance test, which is where
            // the state machine lives.
            control: Some(hertzforge_protocol::ControlProps {
                review_after_sysid: Some(false),
                ..Default::default()
            }),
        }
    }

    fn host() -> RandomHost {
        let root = std::env::temp_dir().join(format!(
            "hertzforge-backendd-random-{}-{}",
            std::process::id(),
            now_unix()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (tx, _) = tokio::sync::broadcast::channel(64);
        RandomHost::new(BackendKind::Sim, Vec::new(), root, tx)
    }

    /// The wire profile translates, the station builds, and the service configures -
    /// on the Sim station, hardware-free.
    #[test]
    fn a_wire_profile_configures_a_service() {
        let h = host();
        let table = hertzforge_channel_table::ChannelTable::default_with_channels(4);
        h.set_profile(profile(), table, "test").expect("configure");
        assert!(h.is_configured());
        assert!(!h.is_live(), "configured is not live");
        let p = h.provenance().expect("provenance recorded");
        assert_eq!(p.module, "random");
    }

    /// RECONFIGURE while THIS module's own run is live is refused by name.
    ///
    /// The CROSS-module half of the exclusion moved out of this host with the
    /// `recorder_live: bool` parameter and is now
    /// `core::tests::the_exclusion_names_the_occupant_in_both_directions`, asked
    /// through `Core::live_module`. What stays here is the self-check: only Random
    /// knows what "already live" means for Random.
    #[test]
    fn reconfigure_refuses_while_this_modules_own_run_is_live() {
        let h = host();
        let table = hertzforge_channel_table::ChannelTable::default_with_channels(4);
        h.set_profile(profile(), table.clone(), "test")
            .expect("configure");
        h.start().expect("start");
        let err = h
            .set_profile(profile(), table, "test")
            .expect_err("must refuse");
        match err {
            ProtocolError::Refused { message } => {
                assert!(message.contains("live"), "{message}");
                assert!(message.contains("Stop"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        h.safe_stop("test teardown");
    }

    /// Start before SetProfile is a refusal that says what to send first.
    #[test]
    fn start_before_configure_says_what_to_send_first() {
        let h = host();
        match h.start() {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains("SetProfile"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// THE WHOLE RUN, on the Sim station: configure -> start (primes to Engaged and
    /// spawns the pump) -> traces on the bus -> controlled stop. This is the
    /// hardware-free proof that a remote peer's verb sequence drives a real service,
    /// not a mock.
    #[test]
    fn the_sim_station_runs_and_publishes_the_overlay_traces() {
        let h = host();
        let mut rx = h.tx.subscribe();
        let table = hertzforge_channel_table::ChannelTable::default_with_channels(4);
        h.set_profile(profile(), table, "test").expect("configure");
        h.start().expect("start");
        assert!(h.is_live(), "started means live");

        // Wait for a batch from a COMPLETED loop (composite present): the very
        // first batches are priming-era (reference + bands only), and asserting
        // on one of those says nothing about the measured traces. Once the
        // composite exists, the DRIVE trace must ride the SAME batch - the
        // operator's "DRV never became selectable" report (2026-08-02) is
        // exactly the claim this pins at the producer.
        let mut got: Option<SpectrumBatch> = None;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match rx.try_recv() {
                Ok(DisplayBatch::Spectrum(b)) => {
                    let has_composite = b.traces.iter().any(|(ch, _, _)| *ch == CH_COMPOSITE);
                    got = Some(b);
                    if has_composite {
                        break;
                    }
                }
                Ok(_) => continue, // scope/wave batches also flow now
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
        let b = got.expect("the Random pump published NO batch - the producer is the problem");
        let ids: Vec<u32> = b.traces.iter().map(|(ch, _, _)| *ch).collect();
        assert!(
            ids.contains(&CH_REFERENCE),
            "reference trace missing: {ids:?}"
        );
        assert!(ids.contains(&CH_ABORT_HI), "abort band missing: {ids:?}");
        assert!(
            ids.contains(&CH_COMPOSITE),
            "no batch with the composite arrived within the deadline: {ids:?}"
        );
        assert!(
            ids.contains(&CH_DRIVE),
            "DRIVE missing from a batch that carries the composite: {ids:?}"
        );
        assert!(b.df_hz > 0.0);

        h.stop().expect("stop");
        assert!(!h.is_live(), "stopped is not live");

        // A SECOND run on the same host must pump again: a run that
        // ended left its finished JoinHandle in the slot, and spawn_pump used
        // to read that as "already pumping" - engaged service, zero loop
        // ticks, zero spectra, found live on the bench.
        let table2 = hertzforge_channel_table::ChannelTable::default_with_channels(4);
        h.set_profile(profile(), table2, "test")
            .expect("reconfigure");
        let mut rx2 = h.tx.subscribe();
        h.start().expect("second start");
        let mut got2 = false;
        let deadline2 = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline2 {
            match rx2.try_recv() {
                Ok(_) => {
                    got2 = true;
                    break;
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
        assert!(
            got2,
            "the SECOND run published nothing - the stale-handle reap regressed"
        );
        h.stop().expect("second stop");
    }

    /// Block until the async pretest has handed off to the driving loop, or
    /// give up. Bounded so a genuine failure fails the test instead of hanging.
    fn wait_engaged(h: &RandomHost) {
        for _ in 0..600 {
            if h.is_live()
                && !h.priming.load(Ordering::SeqCst)
                && matches!(
                    h.svc
                        .lock()
                        .ok()
                        .and_then(|g| g.as_ref().map(|s| s.state())),
                    Some(RunState::Engaged) | Some(RunState::FullLevel)
                )
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("run never engaged after start");
    }

    /// Level intents: a ladder rung jumps; a non-rung value is refused WITH the
    /// ladder; DriveVolts is refused as the wrong base. A MANUAL ladder, because the
    /// service (correctly) refuses operator jumps on an AUTO one - the dwell timers
    /// own that ladder, and my first version of this test learned it from the
    /// refusal.
    #[test]
    fn level_intents_follow_the_ladder() {
        let h = host();
        let table = hertzforge_channel_table::ChannelTable::default_with_channels(4);
        let mut p = profile();
        p.ladder = LadderMode::Manual;
        h.set_profile(p, table, "test").expect("configure");
        h.start().expect("start");
        // The pretest is ASYNC since 2026-08-03 (its duration scales with the
        // acquisition settings and must not sit inside a 60 s command), so a
        // level intent issued the instant `start` returns arrives BEFORE the
        // loop engages - which the run panel already gates on ("level control
        // unlocks once the run engages"). Wait for the engage the same way the
        // operator's UI does.
        wait_engaged(&h);

        h.set_level(LevelTarget::Db(0.0)).expect("jump to full");
        match h.set_level(LevelTarget::Db(-7.3)) {
            Err(ProtocolError::InvalidArgument { message }) => {
                assert!(message.contains("-12"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        match h.set_level(LevelTarget::DriveVolts(1.0)) {
            Err(ProtocolError::InvalidArgument { message }) => {
                assert!(message.contains("open-loop"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        h.stop().expect("stop");
    }
}
