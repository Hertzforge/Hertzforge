//! The daemon-side Explorer module: thin orchestration over `eigen-explorer-host` and
//! the station factory, so a REMOTE front end can run a host-synthesized open-loop
//! survey on this host's hardware.
//!
//! # Scope: the SECOND drive-producing module over the seam
//!
//! [`crate::random::RandomHost`] was the first and this one follows its rails: the
//! try-lock status path with a published snapshot, the priming flag over an asynchronous
//! arm, the dispatch-beside-forwarder command discipline, the out-of-band fault
//! journalling and abort tail, and the shared display bus.
//!
//! The run machinery itself (pump thread, bounded tick batches, status snapshot, stall
//! watchdog, priming flag, abort tail) is [`eigen_explorer_host::host::RunHost`], shared
//! with the desktop app's in-process Explorer. What this file keeps is what only the
//! daemon has: the wire profile and its refusals, the safe-stop budget against the
//! controller timeout, the run provenance, the status report, and [`BusSink`], which puts
//! the pump's output on the display bus and its journal under this daemon's target.
//!
//! # THE CONTROL LOOP NEVER CROSSES THE NETWORK
//!
//! The pump thread owns the service, runs BOUNDED tick batches on the hardware host, and
//! publishes DECIMATED display traces on the shared broadcast. What crosses the wire is
//! operator INTENT down and display frames back. A remote Stop/Abort is SUPERVISORY; the
//! primary abort authority is the in-process engine watchdog co-resident with the drive
//! clock, backed by the physical E-stop.
//!
//! # What is different from Random, and why
//!
//! * **The arm is ASYNC, not the pretest.** Random's `start` arms synchronously and
//!   runs its pretest on the pump thread. Explorer has no pretest - but its `arm()`
//!   opens the station, configures it, adopts the programmed rate, consults the gate,
//!   builds the step and analysis, writes the drive lead-in and starts the clock, and
//!   on an NI chassis that is seconds. A command whose latency scales with a driver's
//!   task setup is the same broken contract the pretest was, so it moves to the pump
//!   thread behind the same priming flag.
//! * **The status path never touches the service mutex.** The service publishes an
//!   [`ExplorerStatus`] into its own status cell at the end of every off-RT analysis
//!   pass, and the host's snapshot reads THAT. Random needed a second cache of its own
//!   because `RandomService::status()` requires the lock; here the cache already exists
//!   one layer down and duplicating it would be two snapshots that can disagree.
//! * **Levels are VOLTS.** `SetLevel { DriveVolts }` is served here and refused by
//!   Random; `SetLevel { Db }` is the mirror image. Each host refuses the other's basis
//!   by name rather than silently reinterpreting it.

use std::sync::{Arc, Mutex};

use eigen_explorer_host::analysis::SpectrumFrame;
use eigen_explorer_host::host::{
    acquired_inputs, HostError, HostNote, HostSink, HostView, RunHost, TableRefusal,
};
use eigen_explorer_host::service::{
    ExplorerHostConfig, ExplorerHostService, ExplorerStatus, RunState, ServiceError,
};
use eigen_explorer_host::slew::SlewLimits;
use eigen_explorer_host::step::{AmplitudeRamp, SweepDirection};
use eigen_protocol::{ExplorerProfile, LevelTarget, ProtocolError};
use eigen_station::{
    try_build_station_backend, BackendKind, NiSelection, SimPlantSpec, StationSpec,
};

use crate::display::TimeTap;
use crate::provenance::{mint_run_id, now_unix, RunProvenance};
use crate::recorder::{DisplayBatch, SpectrumBatch};

// The names the tests below read that this file's own code no longer does: the period
// arithmetic moved to the host, and the decimation clock with it.
#[cfg(test)]
use eigen_explorer_host::host::iter_period_ms;
#[cfg(test)]
use std::time::{Duration, Instant};

/// Base of the Explorer's RESERVED display-trace block (`ui/IPC_CONTRACT.md`).
///
/// The layout is FLAT: `1100 + slot` carries the acceleration magnitude spectrum of the
/// acquired channel at interleave position `slot`. The alternative the plan offered -
/// `1100 + 3k` with magnitude / derived-velocity / derived-displacement per channel -
/// was not taken because two of every three ids would be a permanent reservation no
/// producer writes into: the service publishes one [`SpectrumKind::Accel`] frame per
/// channel and has no derived-axis spectra to publish. The derived axes exist as SCALARS
/// (`ExplorerStatus::derived`), not as spectra. The tail of the block is reserved for
/// them, so the flat layout can grow into the 3-id shape later without moving what is
/// already published.
///
/// **CORRECTED 2026-08-08, and the correction is the operative statement:** the flat
/// `1100 + slot` layout described above was NOT built. Per-channel spectra ride their REAL
/// table index (the Random precedent - see [`spectra_batch`]), and 1100-1149 is reserved
/// for SYNTHETIC Explorer traces only, which is what `ui/src/lib/explorer/traces.ts`
/// assigns and what the daemon publishes. [`EXPLORER_SPECTRUM_SLOTS`] survives as the
/// over-wide-table warning threshold only. The paragraph above is kept because the
/// reasoning about the reserved tail still governs how the block may grow.
///
/// [`SpectrumKind::Accel`]: eigen_explorer_host::analysis::SpectrumKind::Accel
pub const CH_EXPLORER_BASE: u32 = 1100;

/// THE PEAK-LEVEL SWEEP PROFILE trace id: `1100 + 3`, inside the reserved block.
///
/// Mirrors `EXPLORER_TRACE.profile` in `ui/src/lib/explorer/traces.ts`; the two halves of
/// this census are pinned against each other by
/// `the_peak_profile_rides_its_reserved_id_on_the_spectra_grid`. It is ROLE-TAGGED
/// `overlay` on the UI side, which is what keeps display averaging off it: the raster is
/// an accumulator whose bins were each measured at a different moment, so folding
/// successive frames of it into a running mean would blend a bin's remembered value with
/// the interpolated fill that preceded it and lag every revisit behind the screen.
pub const CH_EXPLORER_PROFILE: u32 = 1103;

/// Acquired channels the flat per-slot layout carries: `1100..=1139`. `1140..=1149`
/// stays reserved (see [`CH_EXPLORER_BASE`]). A table with more enabled inputs than this
/// publishes the first 40 and SAYS so in the journal rather than wrapping into the
/// reserved tail - a silently wrapped id is two traces drawn on top of each other.
pub const EXPLORER_SPECTRUM_SLOTS: u32 = 40;

/// The daemon's Explorer host.
pub struct ExplorerHost {
    /// The run machinery: pump, arm, snapshot, watchdog, abort tail.
    run: RunHost,
    /// The SHARED display bus (one per daemon; the recorder and Random publish on it).
    tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    provenance: Mutex<Option<RunProvenance>>,
    /// 1-based table indices of the acquired channels in INTERLEAVE order: the map from
    /// a `SpectrumFrame`'s channel to its display trace.
    input_indices: Mutex<Vec<u32>>,
    /// The daemon's controller timeout, seconds - the budget the safe-stop ramp has to
    /// finish inside. See [`ExplorerHost::set_profile`] for the arithmetic.
    controller_timeout_s: f64,
    kind: BackendKind,
    ni_devices: Vec<String>,
}

impl ExplorerHost {
    /// A host bound to a station kind and the shared display bus. Builds nothing until
    /// [`ExplorerHost::set_profile`].
    pub fn new(
        kind: BackendKind,
        ni_devices: Vec<String>,
        tx: tokio::sync::broadcast::Sender<DisplayBatch>,
        controller_timeout_s: f64,
    ) -> Self {
        ExplorerHost {
            run: RunHost::new(),
            tx,
            provenance: Mutex::new(None),
            input_indices: Mutex::new(Vec::new()),
            controller_timeout_s,
            kind,
            ni_devices,
        }
    }

    /// The provenance of the configured run, if any.
    pub fn provenance(&self) -> Option<RunProvenance> {
        self.provenance.lock().expect("provenance poisoned").clone()
    }

    /// Is a run live (driving, or clocked and about to be able to drive) right now?
    /// `Armed` counts; see [`RunHost::is_live`].
    pub fn is_live(&self) -> bool {
        self.run.is_live()
    }

    /// Is anything configured at all? NEVER a blocking lock.
    pub fn is_configured(&self) -> bool {
        self.run.is_configured()
    }

    /// Translate the wire profile, derive the station spec from the channel table, build
    /// the station, and construct the service over it.
    ///
    /// **The CROSS-MODULE exclusion is NOT here**, exactly as in
    /// [`crate::random::RandomHost::set_profile`]: whether some OTHER module owns the
    /// core is `Core::live_module`'s question, asked by the caller. What stays here is
    /// the self-check - only Explorer knows what "already live" means for Explorer.
    pub fn set_profile(
        &self,
        profile: ExplorerProfile,
        table: eigen_channel_table::ChannelTable,
        lease_owner: &str,
    ) -> Result<(), ProtocolError> {
        if self.is_live() {
            return Err(ProtocolError::Refused {
                message: "an Explorer run is live on this core; Stop it before reconfiguring"
                    .into(),
            });
        }

        // ---- the arm posture is not negotiable over the wire ------------------
        //
        // The service arms at exactly 0.000 V and that is the STATED reason it has no
        // review hold: nothing moves until a human commands a level. A profile that
        // asked to arm at a non-zero drive would arm and immediately ramp, with no human
        // between the two - which is the hold the posture replaces, deleted. Refused by
        // name, pointing at the verb that raises the drive.
        if !(profile.amplitude_vpk.is_finite() && profile.amplitude_vpk == 0.0) {
            return Err(ProtocolError::InvalidArgument {
                message: format!(
                    "amplitude_vpk {} : the Explorer ARMS QUIET, at exactly 0.000 V. That \
                     posture is why the run has no review hold - nothing drives until a \
                     human commands a level - so a profile that armed into drive would \
                     delete the only gate in front of the armature. Send the profile with \
                     amplitude_vpk 0.0 and raise the drive with SetLevel(DriveVolts).",
                    profile.amplitude_vpk
                ),
            });
        }

        // ---- the drive row: EXACTLY one, and at least one input ----------------
        let inputs = acquired_inputs(&table).map_err(table_refusal)?;

        // ---- the wire profile -> the host config ------------------------------
        let cfg = ExplorerHostConfig {
            fs_hz: profile.fs_hz,
            band_hz: profile.band_hz,
            start_hz: profile.start_hz,
            max_drive_vpk: profile.max_drive_vpk,
            slew: SlewLimits {
                freq_oct_per_s: profile.slew.freq_oct_per_s,
                drive_db_per_s: profile.slew.drive_db_per_s,
            },
            ramp: AmplitudeRamp::default(),
            display_nfft: profile.display_nfft,
            ..ExplorerHostConfig::default()
        };

        // ---- THE SAFE-STOP BUDGET ---------------------------------------------
        //
        // A vanished controller is brought down by the RAMPED stop, and the ramp has to
        // finish inside the window the daemon gives it. The arithmetic, stated:
        //
        //     t_ramp = ramp_span_db / slew.drive_db_per_s          [seconds]
        //
        // The drive slew is a RATIO per frame, so it multiplies and can never reach zero
        // by multiplying; the ramp therefore spans a fixed `ramp_span_db` (40 dB, the
        // service's default and the only value this seam offers) below the drive it
        // started from, and steps the last 40 dB to exactly zero. So the span - not the
        // starting volts - sets the duration, and it is an UPPER bound: a run already at
        // 0 V stops at once.
        //
        //     at the shipped default 6 dB/s:  40 / 6   = 6.7 s
        //     against controller_timeout_s:   30 s     -> 23.3 s of margin
        //
        // The slew is the operator's, so the bound is CHECKED rather than asserted. A
        // profile whose ramp would outlast the timeout is refused with both numbers,
        // because the failure it produces otherwise is a daemon that declares the
        // controller gone and then rides a ramp past its own deadline.
        let ramp_span_db = cfg.ramp.ramp_span_db.abs();
        let t_ramp_s = if cfg.slew.drive_db_per_s > 0.0 {
            ramp_span_db / cfg.slew.drive_db_per_s
        } else {
            f64::INFINITY
        };
        // `!is_finite()` FIRST, and it is not belt-and-braces: a zero or negative slew
        // makes the ramp infinite and a NaN one makes every comparison false, so a
        // plain `>=` would let the worst two cases through as "fits".
        if !t_ramp_s.is_finite() || t_ramp_s >= self.controller_timeout_s {
            return Err(ProtocolError::InvalidArgument {
                message: format!(
                    "slew.drive_db_per_s {} would take {:.1} s to ramp the drive down over \
                     the {:.0} dB stop span, and this daemon's controller timeout is {:.1} \
                     s: a vanished controller's safe-stop would still be ramping when the \
                     run was supposed to be down. Raise the drive slew above {:.2} dB/s.",
                    cfg.slew.drive_db_per_s,
                    t_ramp_s,
                    ramp_span_db,
                    self.controller_timeout_s,
                    ramp_span_db / self.controller_timeout_s
                ),
            });
        }
        if t_ramp_s > 0.5 * self.controller_timeout_s {
            tracing::warn!(
                target: "eigen::backendd",
                ramp_s = format!("{t_ramp_s:.1}"),
                controller_timeout_s = self.controller_timeout_s,
                "explorer safe-stop ramp uses more than half the controller-timeout budget"
            );
        }

        // ---- table -> StationSpec (mirrors random.rs:544-571) ------------------
        let n_inputs = inputs.len();
        let (phys_in, phys_out) = eigen_station::physical_from_table(&table);
        let spec = StationSpec {
            // The EIGEN-SIM half is a WIRE, not a resonance plant: no resonators, unity
            // feedthrough, no noise. That is the software analogue of the bench this
            // module is aimed at (AO jumpered to AI, nothing downstream), and it is what
            // makes "measured RMS x sqrt(2) is the commanded volts peak" mean something
            // in the Sim tests. Random's Sim plant is a two-mode resonator because a
            // CLOSED loop needs something to converge against; an open-loop survey does
            // not, and borrowing that plant here would make every measured amplitude a
            // statement about a resonator's gain at whatever frequency the operator
            // happened to pick.
            sim: SimPlantSpec {
                sample_rate_hz: cfg.fs_hz,
                resonances: Vec::new(),
                noise_level: 0.0,
                feedthrough: 1.0,
                shared_clock: true,
                seed: 0x5A5A_1234,
                max_input_channels: n_inputs.max(1) as u32,
                max_output_channels: 1,
                ..SimPlantSpec::default()
            },
            ni: NiSelection {
                inputs: phys_in,
                outputs: phys_out,
                derive_inputs: n_inputs.max(1),
                derive_outputs: 1,
                iepe: false,
                restrict_devices: self.ni_devices.clone(),
            },
            ni_block_frames: eigen_hal::default_block_frames(cfg.fs_hz),
        };

        // THE ONE CONSTRUCTION POINT - a refusal is typed all the way to the remote
        // operator, never a Sim substitution.
        let station = try_build_station_backend(self.kind, &spec).map_err(|e| {
            tracing::warn!(target: "eigen::backendd", "explorer station refused: {e}");
            ProtocolError::Refused {
                message: e.to_string(),
            }
        })?;
        tracing::info!(target: "eigen::backendd", "{}", station.provenance.log_line());
        let station_class = station.station;

        let run_id = mint_run_id();
        let prov = RunProvenance::assemble("explorer", run_id, &station, lease_owner, now_unix());
        tracing::info!(target: "eigen::backendd", "{}", prov.log_line());

        let svc = ExplorerHostService::new(cfg, table, station.backend, station_class)
            .map_err(svc_err)?;

        *self.input_indices.lock().expect("indices poisoned") = inputs;
        self.run.install(
            svc,
            profile
                .sweep
                .map(|s| (sweep_direction(s.direction), s.rate_oct_per_min)),
        );
        *self.provenance.lock().expect("provenance poisoned") = Some(prov);
        Ok(())
    }

    /// Start the configured run.
    ///
    /// The CHEAP refusals are synchronous - not configured, wrong state - so the
    /// operator still gets an immediate, specific "no" for everything that can be
    /// decided without touching the driver. THE ARM IS NOT: it runs on the host's pump
    /// thread behind the priming flag, and the status path reports its progress - the
    /// same shape, and for the same reason, as Random's asynchronous pretest.
    pub fn start(&self) -> Result<(), ProtocolError> {
        let run_id = self
            .provenance()
            .and_then(|p| p.run_id.parse::<u64>().ok())
            .unwrap_or(0);
        let indices = self.input_indices.lock().expect("indices poisoned").clone();
        self.run
            .start(BusSink::new(self.tx.clone(), run_id, indices))
            .map_err(host_err)
    }

    /// Absolute frequency intent. Answers with the CLAMPED value actually adopted.
    ///
    /// The clamp is the STEP's, taken from its return value rather than recomputed here:
    /// a request that got clamped and a request that was honored must never look the
    /// same to the operator, and two implementations of one clamp is how they start to.
    pub fn go_to_frequency(&self, hz: f64) -> Result<f64, ProtocolError> {
        self.run.go_to_frequency(hz).map_err(host_err)
    }

    /// Relative frequency intent. Answers with the CLAMPED value actually adopted.
    pub fn adjust_frequency(&self, delta_hz: f64) -> Result<f64, ProtocolError> {
        self.run.adjust_frequency(delta_hz).map_err(host_err)
    }

    /// Absolute level intent, in the OPEN-LOOP basis. Answers with the clamped volts.
    ///
    /// `LevelTarget::Db` is refused by name: dB is relative to a full level, an
    /// open-loop survey has no full level to be relative to, and quietly reinterpreting
    /// one basis as the other is how a -6 becomes six volts.
    pub fn set_level(&self, level: LevelTarget) -> Result<f64, ProtocolError> {
        let vpk = match level {
            LevelTarget::DriveVolts(v) => v,
            LevelTarget::Db(_) => {
                return Err(ProtocolError::InvalidArgument {
                    message: "Explorer levels are VOLTS PEAK at the drive output; dB is \
                              relative to a closed-loop full level, which an open-loop \
                              survey does not have. Send SetLevel(DriveVolts)."
                        .into(),
                })
            }
        };
        self.run.set_amplitude_vpk(vpk).map_err(host_err)
    }

    /// Begin (or re-aim) a sweep toward a band edge.
    pub fn start_sweep(
        &self,
        direction: SweepDirection,
        rate_oct_per_min: f64,
    ) -> Result<(), ProtocolError> {
        self.run
            .start_sweep(direction, rate_oct_per_min)
            .map_err(host_err)
    }

    /// Stop sweeping; the tone HOLDS where the sweep reached.
    pub fn stop_sweep(&self) -> Result<(), ProtocolError> {
        self.run.stop_sweep().map_err(host_err)
    }

    /// The controlled stop (the operator's normal end of survey): the drive RAMPS to
    /// zero at the operator's drive slew, and only then are the tasks stopped.
    pub fn stop(&self) -> Result<(), ProtocolError> {
        self.run.stop();
        Ok(())
    }

    /// ABORT: immediate, no ramp. `emergency_stop` is issued by the service before this
    /// returns.
    ///
    /// The absence of a ramp is the point, and it is the service's stated tradeoff: a
    /// ramp is the right answer when a run is ENDING and the only question is how
    /// gracefully; an abort is the answer when something is already wrong, and the
    /// backend's own `RampDownThenAbort` taper is the backstop that keeps
    /// `emergency_stop` from being a cliff.
    pub fn abort(&self, reason: &str) -> Result<(), ProtocolError> {
        self.run.abort(reason).map_err(host_err)
    }

    /// Drop a BUILT-BUT-IDLE Explorer so this host stops holding the core.
    ///
    /// Called only by [`crate::core::Core::release_idle_claims`], which asks
    /// `holds(Explorer, Live)` first - so the live case cannot reach here. The host's
    /// own guard is still real, and it is the SECOND of two: `is_live` is the authority
    /// on what "idle" means for the Explorer, and a caller that got it wrong must not
    /// silently drop a clocked station. (Removing either guard alone leaves the
    /// behaviour correct; removing both reds
    /// `a_live_module_still_refuses_set_profile_and_keeps_its_config`.)
    ///
    /// Returns whether anything was actually released, so the caller can journal the
    /// names rather than a count of attempts.
    pub fn release_idle(&self) -> bool {
        if !self.run.release_idle() {
            return false;
        }
        if let Ok(mut p) = self.provenance.lock() {
            *p = None;
        }
        if let Ok(mut i) = self.input_indices.lock() {
            i.clear();
        }
        true
    }

    /// THE SAFE-STOP PATH, invoked when the controller times out. Same rule as Random's
    /// and the recorder's: "nobody is watching" is a reason to stop SAFELY - the
    /// controlled ramp-down - never to slam the drive. Never fails.
    ///
    /// Duration: the ramp is bounded by `ramp_span_db / slew.drive_db_per_s`, and
    /// [`ExplorerHost::set_profile`] REFUSES a profile whose bound is not under this
    /// daemon's `controller_timeout_s`. So by the time this is reached the arithmetic
    /// has already been checked against the budget that called it.
    pub fn safe_stop(&self, reason: &str) {
        if !self.is_live() {
            return;
        }
        tracing::warn!(
            target: "eigen::backendd",
            reason,
            "SAFE-STOP: ramping the Explorer drive down"
        );
        if let Err(e) = self.stop() {
            tracing::warn!(target: "eigen::backendd", "explorer safe-stop: {e}");
        }
    }

    /// Fold this module's truth into a protocol status report.
    ///
    /// **This path never takes the service mutex**: the host's snapshot reads the
    /// service's own published status, so the numbers are at most one analysis pass old,
    /// which is exactly what a display poll wants, and a driver stalled inside a DAQmx
    /// read can never wedge a status poll behind it.
    pub fn fill_status(&self, s: &mut eigen_protocol::StatusReport) {
        let snap = self.run.snapshot();
        // BEFORE anything else: the rate pair describes a run whose rates are already
        // decided, and a status that drops them while a run arms is exactly the window an
        // operator stares at. `None` is unknown, never a number standing in for one.
        s.requested_fs_hz = snap.requested_fs_hz;
        s.actual_fs_hz = snap.actual_fs_hz;
        match snap.view {
            HostView::Unconfigured => {}
            // An arm that REFUSED leaves the service at Idle with its backend consumed:
            // without this the wire would call that "configured" (the out-of-band fault
            // rule).
            HostView::Faulted(f) => {
                s.run = eigen_protocol::RunState::Faulted;
                s.module_state = Some(f);
            }
            HostView::Priming => {
                s.run = eigen_protocol::RunState::Armed;
                s.module_state = Some(
                    "priming (opening the station, configuring it, adopting the rate the \
                     driver programmed, consulting the arm gate, prefilling the drive and \
                     starting the clock - the run is ready when it reports armed)"
                        .into(),
                );
            }
            HostView::Stalled {
                stale_ms,
                period_ms,
            } => {
                s.run = eigen_protocol::RunState::Faulted;
                s.module_state = Some(format!(
                    "stalled_driver ({:.1} s without a pump iteration, expected one every \
                     {:.3} s - the NI runtime is not returning; restart the daemon, and \
                     the chassis if preflight then fails)",
                    stale_ms as f64 / 1e3,
                    period_ms as f64 / 1e3
                ));
            }
            // Configured, never armed: a panel that has never been told anything must say
            // so rather than render zeros.
            HostView::Configured => {
                s.run = eigen_protocol::RunState::Configured;
                s.module_state = Some("configured".into());
            }
            HostView::Status(st) => apply_status(&st, s),
        }
    }
}

/// Map one published service status onto the wire report.
fn apply_status(st: &ExplorerStatus, s: &mut eigen_protocol::StatusReport) {
    // The module's OWN state, verbatim - the front end renders this rather than guessing a
    // fine state from the coarse lifecycle. A TERMINAL state also carries what ended it
    // after a colon (a run that ends must name what ended it, and the wire has no other
    // field for that today). The token before the colon is always the bare state, so a
    // consumer that only wants the state splits once.
    let token = state_str(st.state);
    s.module_state = Some(match (&st.end, st.state.is_terminal()) {
        (Some(end), true) => format!("{token}: {}", end.reason),
        _ => token.to_string(),
    });
    s.run = match st.state {
        // Configured but not armed: no station is open, nothing is clocked.
        RunState::Idle => eigen_protocol::RunState::Configured,
        RunState::Armed => eigen_protocol::RunState::Armed,
        RunState::Running => eigen_protocol::RunState::Running,
        RunState::Stopping => eigen_protocol::RunState::Stopping,
        RunState::Stopped => eigen_protocol::RunState::Stopped,
        RunState::Aborted => eigen_protocol::RunState::Faulted,
    };
    // COMMANDED, not requested: the frequency the core is synthesizing right now, which
    // after a clamp is not the number the operator sent.
    s.frequency_hz = Some(st.commanded_freq_hz);
    s.drive_vpk = Some(st.commanded_vpk);
    // THREE-VALUED on the wire, and this module fills the two inner values: `Some(true)`
    // sweeping, `Some(false)` sweep-capable and parked. `None` is reserved for "no
    // sweep-capable module is live", which is not this module's answer to give. A sweep
    // that reached its edge is HALTED, and halted is parked.
    s.sweep_active = Some(st.sweep.map(|w| !w.halted).unwrap_or(false));
    // The Explorer has no soft alarm band: the volts ceiling clamps rather than warns,
    // and the abort monitors trip rather than warn.
    s.alarm_active = false;
    s.abort_active = st.observed_trip.is_some() || st.state == RunState::Aborted;
    // Run-relative seconds of synthesis, off the step's own frame clock rather than a
    // wall clock, so it means "seconds of drive generated" and survives a re-arm.
    s.elapsed_s = (st.actual_fs_hz > 0.0).then(|| st.frames_generated as f64 / st.actual_fs_hz);
    // THE DISPLAY RING'S LOSS, on the wire. The time-domain tap packs from the blocks this
    // ring hands over, so a drop is a hole in what the operator is watching; the count is
    // what says so, and a count that only reached the daemon's log would say it to nobody
    // at the screen. Reported unconditionally, including zero: on a configured run "0
    // dropped" is a measurement, and the wire's absent case is reserved for "no module
    // with such a ring is live".
    s.display_blocks_dropped = Some(st.blocks_dropped);
}

/// The service state, in the exact token the front end's Explorer state union speaks.
fn state_str(state: RunState) -> &'static str {
    match state {
        RunState::Idle => "idle",
        RunState::Armed => "armed",
        RunState::Running => "running",
        RunState::Stopping => "stopping",
        RunState::Stopped => "stopped",
        RunState::Aborted => "aborted",
    }
}

/// The daemon's half of one run: the pump's display output on the shared display bus,
/// and its journal under this daemon's tracing target. Built per run by
/// [`ExplorerHost::start`] and owned by the pump thread.
struct BusSink {
    tx: tokio::sync::broadcast::Sender<DisplayBatch>,
    run_id: u64,
    indices: Vec<u32>,
    /// The time-domain packer, built at arm from the rate the driver programmed.
    tap: Option<TimeTap>,
    /// The run-relative spectrum window counter.
    window_seq: u32,
    /// A spectrum batch packed under the service mutex, sent after it is released.
    pending: Option<SpectrumBatch>,
}

impl BusSink {
    fn new(
        tx: tokio::sync::broadcast::Sender<DisplayBatch>,
        run_id: u64,
        indices: Vec<u32>,
    ) -> Self {
        BusSink {
            tx,
            run_id,
            indices,
            tap: None,
            window_seq: 0,
            pending: None,
        }
    }

    fn send_pending(&mut self) {
        if let Some(b) = self.pending.take() {
            let _ = self.tx.send(DisplayBatch::Spectrum(b));
        }
    }
}

impl HostSink for BusSink {
    fn armed(&mut self, fs_hz: f64, _display_nfft: usize) {
        self.tap = Some(TimeTap::new(self.run_id, fs_hz));
        if self.indices.len() as u32 > EXPLORER_SPECTRUM_SLOTS {
            tracing::warn!(
                target: "eigen::backendd",
                channels = self.indices.len(),
                slots = EXPLORER_SPECTRUM_SLOTS,
                "more acquired channels than the Explorer trace block carries; the \
                 remainder is NOT published (an id wrapped into the reserved tail would \
                 draw two traces on top of each other)"
            );
        }
    }

    fn block(&mut self, samples: &[f32], stride: usize) {
        if let Some(tap) = self.tap.as_mut() {
            tap.push(samples, stride);
        }
    }

    fn dropped(&mut self, frames: u64) {
        if let Some(tap) = self.tap.as_mut() {
            tap.skip(frames);
        }
    }

    fn spectra(&mut self, frames: &[SpectrumFrame], profile: Option<&[f32]>) -> bool {
        match spectra_batch(frames, &self.indices, self.run_id, profile) {
            Some(b) => {
                self.window_seq = self.window_seq.wrapping_add(1);
                self.pending = Some(SpectrumBatch {
                    window_seq: self.window_seq,
                    ..b
                });
                true
            }
            None => false,
        }
    }

    fn publish(&mut self) {
        if let Some(tap) = self.tap.as_mut() {
            tap.publish(&self.tx);
        }
        self.send_pending();
    }

    fn finish(&mut self) {
        self.send_pending();
        if let Some(tap) = self.tap.as_mut() {
            tap.flush();
            tap.publish(&self.tx);
        }
    }

    fn note(&mut self, note: HostNote<'_>) {
        match note {
            HostNote::ArmRefused(e) => {
                tracing::warn!(target: "eigen::backendd", "explorer arm refused: {e}")
            }
            HostNote::StartRefused(e) => {
                tracing::warn!(target: "eigen::backendd", "explorer start refused: {e}")
            }
            HostNote::InitialSweepRefused(e) => {
                tracing::warn!(target: "eigen::backendd", "explorer profile sweep refused: {e}")
            }
            HostNote::LoopFaulted(e) => {
                tracing::warn!(target: "eigen::backendd", "explorer loop faulted: {e}")
            }
            // DRIVER diagnostics must reach the journal, or the first anyone hears of an
            // underrun is a symptom.
            HostNote::DriverWarning(w) => {
                tracing::warn!(target: "eigen::backendd", "explorer DRIVER diagnostic: {w}")
            }
            HostNote::RingDropping(dropped) => tracing::warn!(
                target: "eigen::backendd",
                dropped,
                "explorer analysis ring has dropped blocks - the pump is behind"
            ),
            HostNote::ProfileGridMismatch {
                profile_bins,
                spectrum_bins,
            } => tracing::warn!(
                target: "eigen::backendd",
                profile_bins,
                spectrum_bins,
                "explorer peak profile is on a DIFFERENT grid from the spectra; not \
                 published (a curve drawn against the wrong frequencies is worse than no \
                 curve)"
            ),
            // A run that ended for any reason but "the operator asked" has to say so in
            // the journal, or a tripped run reads as an unexplained thread exit (the
            // Random path's 2026-08-03 lesson: three deploy cycles blind).
            HostNote::EndedAborted { end, tail } => {
                tracing::warn!(
                    target: "eigen::backendd",
                    reason = %end.reason,
                    trip = format!("{:?}", end.trip),
                    "explorer run ended ABORTED"
                );
                if let Some(st) = tail {
                    tracing::warn!(
                        target: "eigen::backendd",
                        health_faults = st.health_faults,
                        ring_overruns = st.ring_overruns,
                        blocks_dropped = st.blocks_dropped,
                        worst_tick = format!("{:?}", st.worst_tick_compute),
                        budget = format!("{:?}", st.compute_budget),
                        "explorer abort tail"
                    );
                    for w in st.driver_warnings.iter().rev().take(6) {
                        tracing::warn!(target: "eigen::backendd", "  abort-tail driver: {w}");
                    }
                }
            }
            HostNote::Ended(end) => tracing::info!(
                target: "eigen::backendd",
                reason = %end.reason,
                "explorer run ended"
            ),
            HostNote::PumpExited { iters } => {
                tracing::info!(target: "eigen::backendd", iters, "explorer pump thread exited")
            }
            HostNote::PumpPanicked(what) => tracing::error!(
                target: "eigen::backendd",
                panic = %what,
                "EXPLORER PUMP PANICKED - issuing emergency_stop"
            ),
        }
    }

    fn panic_reason(&self, what: &str) -> String {
        format!("daemon pump panicked: {what}")
    }
}

/// One display batch from a new spectrum window, plus the peak-level sweep profile on its
/// reserved synthetic id. `None` when no frame is for a channel the table lists.
///
/// The profile rides THIS batch rather than one of its own for one reason: a batch carries
/// exactly one `(f0_hz, df_hz)` grid, and the profile has to be plotted against the same
/// x-axis as the spectra it is read beside. Sharing the batch makes that structural instead
/// of a coincidence two producers have to keep agreeing on. The host offers the profile
/// only when it is on the spectra's grid.
fn spectra_batch(
    frames: &[SpectrumFrame],
    indices: &[u32],
    run_id: u64,
    profile: Option<&[f32]>,
) -> Option<SpectrumBatch> {
    let first = frames.first()?;
    let mut traces: Vec<(u32, Option<u32>, Arc<Vec<f32>>)> = Vec::with_capacity(frames.len() + 1);
    for f in frames {
        if f.mags.is_empty() {
            continue;
        }
        // Per-channel spectra ride their REAL table index - the Random precedent
        // (its per-channel PSDs do the same), so the picker labels them from the
        // channel table and the same decoders route them. The 1100-1149 block is
        // reserved for SYNTHETIC Explorer traces only (drive/composite/markers -
        // ui/src/lib/explorer/traces.ts is the id map). A channel the table does
        // not list is skipped rather than guessed at.
        if !indices.contains(&f.ch) {
            continue;
        }
        traces.push((f.ch, None, Arc::new(f.mags.clone())));
    }
    if traces.is_empty() {
        return None;
    }
    // THE PEAK PROFILE, on the same grid. Offered only once something has been visited:
    // an all-NaN trace would put a permanently blank entry in the pane's channel list
    // before the first tone ever moved.
    if let Some(mags) = profile {
        traces.push((CH_EXPLORER_PROFILE, None, Arc::new(mags.to_vec())));
    }
    Some(SpectrumBatch {
        f0_hz: first.f0_hz,
        df_hz: first.df_hz,
        window_seq: 0, // stamped by the sink, which owns the run-relative counter
        run_id,
        traces,
    })
}

fn not_configured() -> ProtocolError {
    ProtocolError::Refused {
        message: "Explorer is not configured on this core; send SetProfile first".into(),
    }
}

/// A service error, typed onto the wire. `Config` is the operator's payload and answers
/// `InvalidArgument`; everything else is a refusal about the station or the run state.
fn svc_err(e: ServiceError) -> ProtocolError {
    match e {
        ServiceError::Config(m) => ProtocolError::InvalidArgument { message: m },
        other => ProtocolError::Refused {
            message: other.to_string(),
        },
    }
}

/// A host refusal, typed onto the wire.
fn host_err(e: HostError) -> ProtocolError {
    match e {
        HostError::NotConfigured => not_configured(),
        HostError::AlreadyRunning => ProtocolError::Refused {
            message: "the Explorer run is already under way on this core".into(),
        },
        HostError::CannotStart(state) => ProtocolError::Refused {
            message: format!(
                "Explorer cannot start from {state:?}; reconfigure (SetProfile) to reset"
            ),
        },
        HostError::Service(e) => svc_err(e),
    }
}

/// A table that cannot carry an Explorer run, refused with the fix named. Zero and two
/// Drive rows are different mistakes and get different sentences.
fn table_refusal(r: TableRefusal) -> ProtocolError {
    let message = match r {
        TableRefusal::NoDriveRow => "the channel table has no enabled Drive row: an Explorer \
                                     run synthesizes a tone and needs an output to put it \
                                     on. Edit the table (PutChannelTable) and enable \
                                     exactly one Drive row."
            .to_string(),
        TableRefusal::ExtraDriveRows(n_drive) => format!(
            "{n_drive} enabled Drive rows: the Explorer synthesizes ONE tone and \
             would leave the others silent, which is an output the operator \
             configured and nothing commands. Disable the extra Drive row(s) \
             (PutChannelTable)."
        ),
        TableRefusal::NoInputs => "the channel table has no enabled input channel: an \
                                   Explorer run with nothing to measure has no cross-check \
                                   on the tone it commands. Edit the table \
                                   (PutChannelTable) first."
            .to_string(),
    };
    ProtocolError::Refused { message }
}

/// The protocol's sweep direction, mirrored onto the module's own.
///
/// `eigen_protocol::SweepDirection` is a deliberate MIRROR of the module type (the
/// protocol crate may not depend on a module crate), so the two are structurally
/// identical and this is the one place that has to know it.
fn sweep_direction(d: eigen_protocol::SweepDirection) -> SweepDirection {
    match d {
        eigen_protocol::SweepDirection::Up => SweepDirection::Up,
        eigen_protocol::SweepDirection::Down => SweepDirection::Down,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eigen_channel_table::{ChannelRole, ChannelTable, Units};

    const FS: f64 = 8_192.0;

    /// Three inputs and ONE Drive row - the shape an Explorer run requires.
    fn table() -> ChannelTable {
        let mut t = ChannelTable::default_with_channels(4);
        t.channels[3].role = ChannelRole::Drive;
        t.channels[3].units = Units::VoltageV;
        t
    }

    /// The wire profile these tests run.
    ///
    /// The SLEWS are aggressive on purpose and the reason is arithmetic, not taste: an
    /// EIGEN-SIM station's natural block is ONE frame, so a batch advances the plant by
    /// two samples and the pump's release floor paces it at ~2000 frames/s - a quarter
    /// of real time at this rate. At the shipped 6 dB/s the 40 dB stop ramp alone is
    /// 6.7 s of signal, i.e. ~27 s of wall clock inside a unit test. At 600 dB/s it is
    /// 0.07 s of signal. Nothing else about the run changes, because the slew is a
    /// per-frame ratio derived from the adopted rate either way.
    fn profile() -> ExplorerProfile {
        ExplorerProfile {
            schema_version: eigen_protocol::PROFILE_SCHEMA,
            fs_hz: FS,
            band_hz: (20.0, 2_000.0),
            start_hz: 100.0,
            amplitude_vpk: 0.0,
            max_drive_vpk: 1.0,
            slew: eigen_protocol::ExplorerSlew {
                freq_oct_per_s: 100.0,
                drive_db_per_s: 600.0,
            },
            sweep: None,
            display_nfft: 512,
        }
    }

    fn host() -> ExplorerHost {
        let (tx, _rx) = tokio::sync::broadcast::channel(256);
        ExplorerHost::new(BackendKind::Sim, Vec::new(), tx, 30.0)
    }

    fn status(h: &ExplorerHost) -> eigen_protocol::StatusReport {
        let mut s = eigen_protocol::StatusReport::idle();
        h.fill_status(&mut s);
        s
    }

    /// Wait until `module_state` starts with one of `want`. Bounded, so a genuine
    /// failure fails the test instead of hanging.
    fn wait_state(h: &ExplorerHost, want: &[&str]) -> eigen_protocol::StatusReport {
        for _ in 0..2_000 {
            let s = status(h);
            if s.module_state
                .as_deref()
                .is_some_and(|m| want.iter().any(|w| m.starts_with(w)))
            {
                return s;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "never reached {want:?}; last state {:?}",
            status(h).module_state
        );
    }

    /// Wait until a status predicate holds. Bounded.
    fn wait_until(
        h: &ExplorerHost,
        what: &str,
        mut done: impl FnMut(&eigen_protocol::StatusReport) -> bool,
    ) -> eigen_protocol::StatusReport {
        for _ in 0..2_000 {
            let s = status(h);
            if done(&s) {
                return s;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("gave up waiting for {what}: {:?}", status(h));
    }

    /// The wire profile translates, the station builds, and the service configures - on
    /// the Sim station, hardware-free.
    #[test]
    fn a_wire_profile_configures_a_service() {
        let h = host();
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        assert!(h.is_configured());
        assert!(
            !h.is_live(),
            "configured is not live - nothing is clocked yet"
        );
        let p = h.provenance().expect("provenance recorded");
        assert_eq!(p.module, "explorer");
        assert!(!p.simulated_badge, "EIGEN-SIM is not the amber badge");
        // The REQUEST is known at configure; the actual is not, and nothing may stand
        // in for it before the station has answered.
        let s = status(&h);
        assert_eq!(s.requested_fs_hz, Some(FS));
        assert_eq!(
            s.actual_fs_hz, None,
            "the station answers at arm; a number here would be the request wearing the \
             actual's name"
        );
    }

    /// EXACTLY ONE Drive row, and zero and two are different mistakes with different
    /// sentences. Both refusals name the fix.
    #[test]
    fn a_table_without_exactly_one_drive_row_is_refused_by_name() {
        let h = host();
        // ZERO: the daemon's own seed table, which Random tolerates.
        let none = ChannelTable::default_with_channels(4);
        match h.set_profile(profile(), none, "test") {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains("no enabled Drive row"), "{message}");
                assert!(message.contains("PutChannelTable"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        // TWO: an output the operator configured that nothing would command.
        let mut two = table();
        two.channels[2].role = ChannelRole::Drive;
        two.channels[2].units = Units::VoltageV;
        match h.set_profile(profile(), two, "test") {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains("2 enabled Drive rows"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        assert!(!h.is_configured(), "a refused profile configures nothing");
    }

    /// THE ARM POSTURE IS NOT NEGOTIABLE OVER THE WIRE.
    ///
    /// A profile that armed into drive would arm and immediately ramp with no human
    /// between the two - deleting the very thing that makes a review hold unnecessary.
    #[test]
    fn a_profile_that_arms_into_drive_is_refused_with_the_posture_stated() {
        let h = host();
        let mut p = profile();
        p.amplitude_vpk = 0.1;
        match h.set_profile(p, table(), "test") {
            Err(ProtocolError::InvalidArgument { message }) => {
                assert!(message.contains("0.000 V"), "{message}");
                assert!(message.contains("review hold"), "{message}");
                assert!(message.contains("SetLevel(DriveVolts)"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// THE SAFE-STOP BUDGET, checked rather than asserted.
    ///
    /// 40 dB at 1 dB/s is a 40 s ramp against a 30 s controller timeout: a vanished
    /// controller's safe-stop would still be ramping when the run was supposed to be
    /// down. Anti-vacuity: the same profile at the shipped 6 dB/s (6.7 s) is ACCEPTED,
    /// so the refusal is about the arithmetic and not about the check being on.
    #[test]
    fn a_stop_ramp_that_outlasts_the_controller_timeout_is_refused_with_both_numbers() {
        let h = host();
        let mut p = profile();
        p.slew.drive_db_per_s = 1.0;
        match h.set_profile(p, table(), "test") {
            Err(ProtocolError::InvalidArgument { message }) => {
                assert!(message.contains("40.0 s"), "the ramp duration: {message}");
                assert!(message.contains("30.0"), "the timeout: {message}");
            }
            other => panic!("{other:?}"),
        }
        let mut ok = profile();
        ok.slew.drive_db_per_s = 6.0; // the shipped default: 40 / 6 = 6.7 s
        h.set_profile(ok, table(), "test")
            .expect("a 6.7 s ramp fits inside a 30 s budget");
    }

    /// Levels are VOLTS. dB is refused by name rather than reinterpreted, because the
    /// two bases are numerically indistinguishable and silently swapping them is how a
    /// -6 becomes six volts.
    #[test]
    fn a_db_level_is_refused_as_the_wrong_basis() {
        let h = host();
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        match h.set_level(LevelTarget::Db(-6.0)) {
            Err(ProtocolError::InvalidArgument { message }) => {
                assert!(message.contains("VOLTS PEAK"), "{message}");
                assert!(message.contains("DriveVolts"), "{message}");
            }
            other => panic!("{other:?}"),
        }
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

    /// **THE WHOLE RUN**, on the Sim station: configure -> start (arms on the pump
    /// thread) -> spectra on the RESERVED Explorer trace block -> clamped intents ->
    /// sweep -> ramped stop. The hardware-free proof that a remote peer's verb sequence
    /// drives a real service, not a mock.
    #[test]
    fn the_sim_station_runs_publishes_the_explorer_block_and_ramps_to_a_stop() {
        let h = host();
        let mut rx = h.tx.subscribe();
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        h.start().expect("start");
        assert!(h.is_live(), "started means live - the arm is under way");

        // ---- ARMED / RUNNING, and the rate the station actually programmed ---------
        let s = wait_state(&h, &["armed", "running"]);
        assert_eq!(
            s.actual_fs_hz,
            Some(FS),
            "the Sim programs what it is asked for, and BOTH rates are reported so a \
             station that coerced would be visible"
        );
        assert_eq!(s.drive_vpk, Some(0.0), "an armed Explorer commands 0.000 V");
        assert_eq!(s.frequency_hz, Some(100.0), "parked at the profile's start");
        assert_eq!(
            s.sweep_active,
            Some(false),
            "sweep-capable and parked is Some(false), not None"
        );

        // ---- INTENTS ANSWER WITH THE CLAMPED VALUE --------------------------------
        assert_eq!(
            h.go_to_frequency(9_000.0).expect("frequency intent"),
            2_000.0,
            "a frequency past the band edge is CLAMPED, and the answer says so"
        );
        assert_eq!(h.go_to_frequency(400.0).expect("retune"), 400.0);
        assert_eq!(
            h.set_level(LevelTarget::DriveVolts(50.0))
                .expect("level intent"),
            1.0,
            "a drive past the operator ceiling is CLAMPED to it"
        );
        assert_eq!(
            h.set_level(LevelTarget::DriveVolts(0.2)).expect("level"),
            0.2
        );

        // The COMMANDED drive walks to the target at the slew rather than jumping.
        let s = wait_until(&h, "the drive ramp to reach 0.2 Vpk", |s| {
            s.drive_vpk.is_some_and(|v| (v - 0.2).abs() < 1e-9)
        });
        assert!(
            s.frequency_hz.is_some_and(|f| (f - 400.0).abs() < 1e-9),
            "the tone arrived at the retune target: {:?}",
            s.frequency_hz
        );

        // ---- A SWEEP IS RUNNING, then parked where it reached ----------------------
        h.start_sweep(SweepDirection::Up, 600.0).expect("sweep");
        let swept = wait_until(&h, "the sweep to be reported active", |s| {
            s.sweep_active == Some(true)
        });
        let f_at_sweep = swept.frequency_hz.expect("a swept tone has a frequency");
        h.stop_sweep().expect("stop sweep");
        let parked = wait_until(&h, "the sweep to park", |s| s.sweep_active == Some(false));
        assert!(
            parked.frequency_hz.is_some_and(|f| f >= f_at_sweep),
            "an UP sweep travels toward the high edge and HOLDS where it stopped: \
             {f_at_sweep} -> {:?}",
            parked.frequency_hz
        );

        // ---- THE DISPLAY TRACES rode the RESERVED block ---------------------------
        let mut ids: Vec<u32> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline && ids.is_empty() {
            match rx.try_recv() {
                Ok(DisplayBatch::Spectrum(b)) => {
                    assert!(b.df_hz > 0.0, "a spectrum batch carries its grid");
                    assert!(b.run_id > 0, "an attributed run stamps its batches");
                    ids = b.traces.iter().map(|(ch, _, _)| *ch).collect();
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break,
            }
        }
        assert!(
            !ids.is_empty(),
            "the Explorer pump published NO spectrum batch - the producer is the problem"
        );
        // Three enabled inputs -> the REAL table indices 1, 2, 3. The 1100-1149 block
        // carries SYNTHETICS only, and the peak profile (1103) is the one that exists
        // today - so it is filtered out here rather than asserted absent, and pinned by
        // `the_peak_profile_rides_its_reserved_id_on_the_spectra_grid`.
        let real: Vec<u32> = ids
            .iter()
            .copied()
            .filter(|ch| *ch < CH_EXPLORER_BASE)
            .collect();
        assert_eq!(
            real,
            vec![1, 2, 3],
            "per-channel spectra ride their REAL table indices (Random precedent); \
             1100-1149 stays reserved for synthetics. Got {ids:?}"
        );
        assert!(
            ids.iter()
                .all(|ch| *ch < CH_EXPLORER_BASE || *ch == CH_EXPLORER_PROFILE),
            "no OTHER synthetic id may appear in the block yet: {ids:?}"
        );

        // ---- THE RAMPED STOP -------------------------------------------------------
        h.stop().expect("stop");
        let ended = wait_state(&h, &["stopped"]);
        assert!(!h.is_live(), "a stopped run is not live");
        assert_eq!(
            ended.drive_vpk,
            Some(0.0),
            "the stop RAMPED the drive to exactly zero before the tasks stopped"
        );
        assert_eq!(ended.run, eigen_protocol::RunState::Stopped);
        assert!(
            ended
                .module_state
                .as_deref()
                .is_some_and(|m| m.starts_with("stopped:")),
            "a run that ends must NAME what ended it: {:?}",
            ended.module_state
        );
        assert!(!ended.abort_active, "a controlled stop is not an abort");
    }

    /// RECONFIGURE while THIS module's own run is live is refused by name. The
    /// CROSS-module half is `Core::live_module`'s, asked by the caller.
    #[test]
    fn reconfigure_refuses_while_this_modules_own_run_is_live() {
        let h = host();
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        h.start().expect("start");
        wait_state(&h, &["armed", "running"]);
        match h.set_profile(profile(), table(), "test") {
            Err(ProtocolError::Refused { message }) => {
                assert!(message.contains("live"), "{message}");
                assert!(message.contains("Stop"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        h.safe_stop("test teardown");
        wait_state(&h, &["stopped"]);
    }

    /// ABORT is IMMEDIATE and says so; a controlled stop is not an abort.
    ///
    /// Anti-vacuity for the split in `Core::abort_module`: if `Abort` were still routed
    /// to the ramped stop, the run would land in `stopped` and both assertions here
    /// would red.
    #[test]
    fn an_abort_ends_the_run_immediately_and_names_itself() {
        let h = host();
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        h.start().expect("start");
        wait_state(&h, &["armed", "running"]);
        h.set_level(LevelTarget::DriveVolts(0.2)).expect("level");
        h.abort("test abort").expect("abort");
        let s = wait_state(&h, &["aborted"]);
        assert_eq!(s.run, eigen_protocol::RunState::Faulted);
        assert!(s.abort_active, "an aborted run says so on the wire");
        assert!(
            s.module_state
                .as_deref()
                .is_some_and(|m| m.contains("test abort")),
            "the abort must carry its reason: {:?}",
            s.module_state
        );
        assert!(!h.is_live());
    }

    /// **THE TIME-DOMAIN STREAMS** (operator: "Time is a must, highest priority").
    ///
    /// The Explorer daemon used to publish spectra and NOTHING ELSE, so every remote time
    /// pane sat blank for the whole run. This is the producer half, proved on the Sim
    /// station: envelope columns AND decimated samples reach the shared display bus, both
    /// carrying a RUN-RELATIVE clock that advances.
    ///
    /// RED-MUTATION: stamp `t_end_s` from the block length instead of the cumulative frame
    /// count (pass `frames` where `cum_frames` goes) and the monotonicity assertions red -
    /// which is the state where every wire frame carries the same `tEnd` and every remote
    /// follow window is frozen (48/48 frames measured on the Random path).
    #[test]
    fn the_time_domain_streams_reach_the_display_bus_with_an_advancing_clock() {
        let h = host();
        let mut rx = h.tx.subscribe();
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        h.start().expect("start");
        wait_state(&h, &["armed", "running"]);
        h.set_level(LevelTarget::DriveVolts(0.2)).expect("level");

        let mut scope_t: Vec<f64> = Vec::new();
        let mut wave_t: Vec<f64> = Vec::new();
        let mut scope_dt = 0.0f32;
        let mut wave_dt = 0.0f32;
        let mut run_ids: Vec<u64> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline && (scope_t.len() < 3 || wave_t.len() < 3) {
            match rx.try_recv() {
                Ok(DisplayBatch::Scope(b)) => {
                    scope_dt = b.dt_s;
                    run_ids.push(b.run_id);
                    scope_t.push(b.t_end_s);
                }
                Ok(DisplayBatch::Wave(b)) => {
                    wave_dt = b.dt_s;
                    run_ids.push(b.run_id);
                    wave_t.push(b.t_end_s);
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        h.safe_stop("test teardown");
        wait_state(&h, &["stopped"]);

        assert!(
            scope_t.len() >= 3,
            "the Explorer pump published no ENVELOPE stream: {} batches",
            scope_t.len()
        );
        assert!(
            wave_t.len() >= 3,
            "the Explorer pump published no WAVEFORM stream: {} batches",
            wave_t.len()
        );
        assert!(
            run_ids.iter().all(|&r| r > 0),
            "an attributed run stamps every time batch: {run_ids:?}"
        );
        // THE CLOCK CONTRACT, on both streams: run-relative and strictly advancing.
        for w in scope_t.windows(2) {
            assert!(w[1] > w[0], "scope t_end must ADVANCE: {scope_t:?}");
        }
        for w in wave_t.windows(2) {
            assert!(w[1] > w[0], "wave t_end must ADVANCE: {wave_t:?}");
        }
        assert!(
            scope_t[0] > 0.0 && wave_t[0] > 0.0,
            "run-relative, not zero: scope {} wave {}",
            scope_t[0],
            wave_t[0]
        );
        // The decimations the SHARED packers promise, at this rate: 240 envelope columns
        // per second and a ~2 kHz waveform. A per-module copy of either would drift from
        // these, which is exactly what sharing them prevents.
        let per_col = (FS / 240.0).round();
        assert!(
            (scope_dt as f64 - per_col / FS).abs() < 1e-6,
            "scope column period: {scope_dt}"
        );
        let dec = (FS / 2_000.0).round();
        assert!(
            (wave_dt as f64 - dec / FS).abs() < 1e-6,
            "wave sample period: {wave_dt}"
        );
    }

    /// **THE PEAK-LEVEL SWEEP PROFILE** on the wire: its reserved synthetic id, the
    /// SPECTRA'S OWN GRID, a visited span that is finite and an unvisited remainder that
    /// is not - and it SURVIVES the sweep stopping.
    ///
    /// The grid equality is the load-bearing assertion. The profile is rasterized from
    /// `display_nfft` and the adopted rate while the spectra come out of the FFT engine;
    /// if those two ever disagree the profile is a curve plotted against frequencies it
    /// was never measured at, and it would still LOOK like a profile.
    #[test]
    fn the_peak_profile_rides_its_reserved_id_on_the_spectra_grid() {
        let h = host();
        let mut rx = h.tx.subscribe();
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        h.start().expect("start");
        wait_state(&h, &["armed", "running"]);
        h.set_level(LevelTarget::DriveVolts(0.2)).expect("level");
        h.start_sweep(SweepDirection::Up, 600.0).expect("sweep");

        // A batch that carries BOTH a real channel and the profile: the grid comparison
        // only means something when the two rode the same batch.
        let mut grid: Option<(f64, f64, usize, usize)> = None;
        let mut profile_mags: Vec<f32> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(40);
        while Instant::now() < deadline && grid.is_none() {
            match rx.try_recv() {
                Ok(DisplayBatch::Spectrum(b)) => {
                    let prof = b
                        .traces
                        .iter()
                        .find(|(ch, _, _)| *ch == CH_EXPLORER_PROFILE);
                    let real = b.traces.iter().find(|(ch, _, _)| *ch == 1);
                    if let (Some((_, _, p)), Some((_, _, r))) = (prof, real) {
                        profile_mags = p.as_ref().clone();
                        grid = Some((b.f0_hz, b.df_hz, p.len(), r.len()));
                    }
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        let (f0, df, n_prof, n_real) =
            grid.expect("no spectrum batch carried the peak profile beside a real channel");
        assert_eq!(
            CH_EXPLORER_PROFILE, 1103,
            "the id is the UI census's (ui/src/lib/explorer/traces.ts EXPLORER_TRACE.profile)"
        );
        assert!(
            (CH_EXPLORER_BASE..CH_EXPLORER_BASE + 50).contains(&CH_EXPLORER_PROFILE),
            "inside the RESERVED Explorer block"
        );
        assert_eq!(n_prof, n_real, "the profile rides the SPECTRA'S grid");
        assert_eq!(f0, 0.0, "the display grid starts at DC");
        // 512-point display FFT at 8192 Hz -> 16 Hz bins, 257 of them.
        assert!((df - FS / 512.0).abs() < 1e-9, "df {df}");
        assert_eq!(n_prof, 512 / 2 + 1);
        // A VISITED span and an UNVISITED remainder. The sweep starts at 100 Hz and the
        // band tops out at 2 kHz, so the bins above wherever it reached have never been
        // the commanded tone and must read as absent, not as a measured floor.
        assert!(
            profile_mags.iter().any(|v| v.is_finite()),
            "nothing visited - the profile published an empty curve"
        );
        assert!(
            profile_mags.iter().any(|v| v.is_nan()),
            "an unvisited bin must be NaN, never a floor: {:?}",
            &profile_mags[..8]
        );
        assert!(
            profile_mags[0].is_nan(),
            "DC is never a survey tone: {}",
            profile_mags[0]
        );

        // PERSISTENCE: stopping the sweep parks the tone; the profile stands.
        h.stop_sweep().expect("stop sweep");
        wait_until(&h, "the sweep to park", |s| s.sweep_active == Some(false));
        let mut still_there = false;
        let deadline = Instant::now() + Duration::from_secs(40);
        while Instant::now() < deadline && !still_there {
            match rx.try_recv() {
                Ok(DisplayBatch::Spectrum(b)) => {
                    still_there = b.traces.iter().any(|(ch, _, m)| {
                        *ch == CH_EXPLORER_PROFILE && m.iter().any(|v| v.is_finite())
                    });
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        assert!(
            still_there,
            "the profile must SURVIVE the sweep stopping - remember-until-overwritten is \
             the whole feature"
        );
        h.safe_stop("test teardown");
        wait_state(&h, &["stopped"]);
    }

    /// The display ring's producer-side loss reaches the WIRE, not just the
    /// daemon's log. Zero is a measurement here and `None` is reserved for "no module with
    /// such a ring is live" - a configured Explorer run always answers with a number.
    #[test]
    fn the_display_rings_drop_count_reaches_the_status_report() {
        let h = host();
        assert_eq!(
            status(&h).display_blocks_dropped,
            None,
            "nothing configured has no ring to report on"
        );
        h.set_profile(profile(), table(), "test")
            .expect("configure");
        h.start().expect("start");
        wait_state(&h, &["armed", "running"]);
        assert!(
            status(&h).display_blocks_dropped.is_some(),
            "a live run reports its drop count, even at zero"
        );
        h.safe_stop("test teardown");
        wait_state(&h, &["stopped"]);
    }

    /// The pump-iteration period the stall watchdog scales to, and the 3 s floor that
    /// actually decides at these rates. A test rather than a comment because the
    /// arithmetic is what keeps a healthy fast loop from reading as a dead driver.
    #[test]
    fn the_stall_threshold_is_dominated_by_its_floor_at_realistic_block_rates() {
        // 256 frames at 8533 Hz (the 9234's coerced rate for a requested 8192) is a
        // 30 ms block; one iteration is two of them.
        let ms = iter_period_ms(8_533.33, 256);
        assert_eq!(ms, 60, "two 30 ms blocks per iteration");
        assert_eq!(
            (3 * ms).max(3_000),
            3_000,
            "three missed iterations is 180 ms - the 3 s floor is what saves a fast loop \
             from a scheduling hiccup reading as a stalled driver"
        );
        // And a rate that does not exist yields no cadence rather than a fabricated one.
        assert_eq!(iter_period_ms(0.0, 256), 0);
    }
}
