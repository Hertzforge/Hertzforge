//! One simulated random run: the controller's random service closing its loop
//! around the Sim plant, on a dedicated thread.
//!
//! The control loop never waits on the network or the shell. The run thread owns
//! the service outright; after every control loop it publishes a [`Snapshot`] into
//! a slot the display side reads, and the display side never blocks it for longer
//! than one `Arc` clone. A stop request is an atomic flag the thread checks between
//! control loops (and the pacer checks between engine ticks so a paced run winds
//! down at once).
//!
//! The run follows the controller's own state machine: feasibility, arm, system
//! identification, coherence gate, soft start, an automatic level ladder up to the
//! requested level, then the full-level hold for the requested duration. Every way a
//! run can end (duration complete, operator stop, protection abort, coherence hold,
//! loop fault) is reported as a [`Finish`] with an [`Outcome`] and a reason in
//! words; an abort is never folded into a plain "stopped".
//!
//! Everything here is simulation. The plant is `eigen_hal_sim`, the station class
//! is EIGEN-SIM, and every frame this crate builds from a run says `source: "SIM"`.

use crate::codec::Arrays;
use crate::profiles::{PlantSpec, Profile};
use crate::views::{self, Axis, Role, Row, Scale, Series, Status};
use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use eigen_random::service::config::{LevelSchedule, RunMode};
use eigen_random::service::engine::BlockObserver;
use eigen_random::{LoopConfig, RandomService, RandomTestConfig, RunState, TripKind, TripRecord};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Plant sample rate, Hz. Nyquist leaves room for a 2000 Hz test band.
pub const SAMPLE_RATE_HZ: f64 = 8192.0;
/// FFT length of the control spectra (8 Hz lines at 8192 Hz).
pub const NFFT: usize = 1024;
/// FFT blocks per control loop. Two blocks make a 0.25 s loop at 8192 Hz, so the
/// spectrum refreshes about four times a second; the controller's own per-line
/// averaging of the control estimate supplies the statistical degrees of freedom.
pub const BLOCKS_PER_WINDOW: usize = 2;
/// System identification length, in control loops.
pub const PRETEST_SEGMENTS: usize = 8;
/// Accepted `level_db` range (relative to the profile's full level).
pub const LEVEL_DB_RANGE: (f64, f64) = (-30.0, 6.0);
/// Accepted `speed` range. 0 = unpaced; 1 = real time; 2 = twice real time.
pub const SPEED_RANGE: (f64, f64) = (0.0, 1000.0);
/// Full-level hold when the caller names none, s.
pub const DEFAULT_DURATION_S: f64 = 300.0;
/// Longest accepted full-level hold, s.
pub const MAX_DURATION_S: f64 = 24.0 * 3600.0;
/// Ladder rungs below the requested level, and their spacing.
const LADDER_RUNGS_BELOW: usize = 4;
const LADDER_STEP_DB: f64 = 3.0;
/// Dwell on each rung below the requested level, s.
const LADDER_DWELL_S: f64 = 2.0;
/// Kinematic abort on the control channel, g peak.
const CONTROL_ABORT_G: f64 = 150.0;
/// Control channel range, g (the controller's overload proxy keys off it).
const CONTROL_RANGE_G: f64 = 250.0;

/// A request to run a profile.
#[derive(Debug, Clone)]
pub struct RunRequest {
    pub profile: Profile,
    /// Test level relative to the profile's full level, dB.
    pub level_db: f64,
    /// Simulated seconds per wall second; 0 runs as fast as the engine can.
    pub speed: f64,
    /// Hold at the requested level for this long, then stop cleanly, s.
    pub duration_s: f64,
}

impl RunRequest {
    /// Check the knobs, naming the one that is out of range.
    pub fn validate(&self) -> Result<(), String> {
        let (lo, hi) = LEVEL_DB_RANGE;
        if !(lo..=hi).contains(&self.level_db) {
            return Err(format!(
                "level_db {} is outside {lo} to {hi} dB (0 dB is the profile's full level)",
                self.level_db
            ));
        }
        let (lo, hi) = SPEED_RANGE;
        if !(lo..=hi).contains(&self.speed) {
            return Err(format!(
                "speed {} is outside {lo} to {hi} (1 = real time, 0 = as fast as possible)",
                self.speed
            ));
        }
        if !(self.duration_s > 0.0 && self.duration_s <= MAX_DURATION_S) {
            return Err(format!(
                "duration_s {} must be above 0 and at most {MAX_DURATION_S} s",
                self.duration_s
            ));
        }
        Ok(())
    }
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The full-level duration elapsed and the run stopped cleanly.
    Completed,
    /// The operator stopped it.
    OperatorStop,
    /// A protection engine tripped.
    Aborted,
    /// The coherence gate held; the run never drove at test level.
    Held,
    /// The loop faulted (or could not start driving).
    Fault,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Completed => "completed",
            Outcome::OperatorStop => "operator_stop",
            Outcome::Aborted => "aborted",
            Outcome::Held => "held",
            Outcome::Fault => "fault",
        }
    }

    fn status(self) -> Status {
        match self {
            Outcome::Completed | Outcome::OperatorStop => Status::Info,
            Outcome::Held => Status::Warn,
            Outcome::Aborted | Outcome::Fault => Status::Fail,
        }
    }
}

/// The end of a run.
#[derive(Debug, Clone, PartialEq)]
pub struct Finish {
    pub outcome: Outcome,
    pub reason: String,
}

/// What the run thread publishes after each control loop.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The controller's own state.
    pub phase: RunState,
    /// The level being driven, dB relative to the profile's full level (the system
    /// identification level until the loop engages).
    pub level_db: f64,
    /// Measured control rms over the control band, g.
    pub control_rms: Option<f64>,
    /// Reference rms at `level_db`, g.
    pub reference_rms: f64,
    /// Drive rms, V.
    pub drive_rms: Option<f64>,
    /// Control lines outside the alarm band at the last loop.
    pub alarm_lines: usize,
    /// Measured control PSD on the display lines, g²/Hz.
    pub control_psd: Option<Vec<f64>>,
    /// Drive PSD on the display lines, V²/Hz.
    pub drive_psd: Option<Vec<f64>>,
    /// Set once, when the run has ended.
    pub finish: Option<Finish>,
}

/// The display line grid: the lines the profile spans, and the reference on them.
#[derive(Debug, Clone)]
struct DisplayGrid {
    /// First and last display line index on the control grid (inclusive).
    lo: usize,
    hi: usize,
    freq_hz: Vec<f64>,
    /// The reference at full level (0 dB) on the display lines.
    reference_full: Vec<f64>,
}

impl DisplayGrid {
    fn from_reference(psd: &[f64], df_hz: f64) -> Option<DisplayGrid> {
        let lo = psd.iter().position(|&p| p > 0.0)?;
        let hi = psd.iter().rposition(|&p| p > 0.0)?;
        Some(DisplayGrid {
            lo,
            hi,
            freq_hz: (lo..=hi).map(|k| k as f64 * df_hz).collect(),
            reference_full: psd[lo..=hi].to_vec(),
        })
    }

    fn slice(&self, psd: &[f64]) -> Option<Vec<f64>> {
        psd.get(self.lo..=self.hi)
            .map(|s| s.iter().map(|&p| p.max(0.0)).collect())
    }
}

/// State shared between the run thread and its readers.
struct Shared {
    stop: AtomicBool,
    finished: AtomicBool,
    /// Plant frames the engine has run (the simulated clock).
    frames: AtomicU64,
    latest: Mutex<Arc<Snapshot>>,
}

/// A started run. Dropping it does not stop the thread; call [`SimRun::request_stop`].
pub struct SimRun {
    run_id: String,
    request: RunRequest,
    fs_hz: f64,
    sysid_level_db: f64,
    grid: DisplayGrid,
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

struct Ready {
    fs_hz: f64,
    sysid_level_db: f64,
    grid: DisplayGrid,
}

impl SimRun {
    /// Build, check and arm the run, then hand it to its own thread. Returns once the
    /// service is armed (system identification is then under way), or with the
    /// controller's reason for refusing. Blocking; call it off the async runtime.
    pub fn start(run_id: String, request: RunRequest) -> Result<SimRun, String> {
        request.validate()?;
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            latest: Mutex::new(Arc::new(Snapshot {
                phase: RunState::Idle,
                level_db: 0.0,
                control_rms: None,
                reference_rms: 0.0,
                drive_rms: None,
                alarm_lines: 0,
                control_psd: None,
                drive_psd: None,
                finish: None,
            })),
        });
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Ready, String>>();
        let thread_shared = shared.clone();
        let thread_request = request.clone();
        let thread = std::thread::Builder::new()
            .name(format!("sds-sim-{run_id}"))
            .spawn(move || run_thread(thread_request, thread_shared, ready_tx))
            .map_err(|e| format!("could not start the run thread: {e}"))?;
        let ready = match ready_rx.recv_timeout(Duration::from_secs(60)) {
            Ok(Ok(ready)) => ready,
            Ok(Err(reason)) => {
                let _ = thread.join();
                return Err(reason);
            }
            Err(_) => {
                shared.stop.store(true, Ordering::SeqCst);
                return Err("the controller did not arm within 60 s".into());
            }
        };
        Ok(SimRun {
            run_id,
            request,
            fs_hz: ready.fs_hz,
            sysid_level_db: ready.sysid_level_db,
            grid: ready.grid,
            shared,
            thread: Mutex::new(Some(thread)),
        })
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn request(&self) -> &RunRequest {
        &self.request
    }

    /// The plant sample rate the station programmed, Hz.
    pub fn fs_hz(&self) -> f64 {
        self.fs_hz
    }

    /// The level system identification drives at, dB.
    pub fn sysid_level_db(&self) -> f64 {
        self.sysid_level_db
    }

    /// Ask the run to stop. Idempotent.
    pub fn request_stop(&self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }

    /// True once the run thread has published its final snapshot.
    pub fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::SeqCst)
    }

    /// Block until the run has finished, at most `timeout`. True if it finished.
    pub fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !self.is_finished() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if let Ok(mut t) = self.thread.lock() {
            if let Some(handle) = t.take() {
                let _ = handle.join();
            }
        }
        true
    }

    /// Simulated seconds the plant has run.
    pub fn elapsed_s(&self) -> f64 {
        self.shared.frames.load(Ordering::Relaxed) as f64 / self.fs_hz
    }

    /// The latest published snapshot.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        match self.shared.latest.lock() {
            Ok(s) => s.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// The protocol state: `running` until the thread has finished, then `stopped`.
    pub fn protocol_state(&self) -> &'static str {
        if self.is_finished() {
            "stopped"
        } else {
            "running"
        }
    }

    /// The `random.display` event data for the latest snapshot. Series data is
    /// attached to `arrays`.
    pub fn display_data(&self, arrays: &mut Arrays) -> Value {
        let snap = self.snapshot();
        let finish = snap.finish.as_ref();
        let state = if finish.is_some() {
            "stopped"
        } else {
            "running"
        };
        let level = snap.level_db;
        let p = &self.request.profile;
        let scale = |db: f64| -> Vec<f64> {
            let k = 10f64.powf((level + db) / 10.0);
            self.grid.reference_full.iter().map(|&r| r * k).collect()
        };
        let f = || self.grid.freq_hz.clone();

        // Every curve is per line of the control grid, so each is drawn flat across
        // its own bin, the way it was computed.
        let mut series = Vec::with_capacity(7);
        if let Some(c) = &snap.control_psd {
            series.push(Series::new("Control", Role::Measured, f(), c.clone()).stepped());
        }
        series.push(Series::new("Reference", Role::Reference, f(), scale(0.0)).stepped());
        series.push(Series::new("Alarm high", Role::Tolerance, f(), scale(p.alarm_db.1)).stepped());
        series.push(Series::new("Alarm low", Role::Tolerance, f(), scale(-p.alarm_db.0)).stepped());
        series.push(Series::new("Abort high", Role::Abort, f(), scale(p.abort_db.1)).stepped());
        series.push(Series::new("Abort low", Role::Abort, f(), scale(-p.abort_db.0)).stepped());
        if let Some(d) = &snap.drive_psd {
            series.push(Series::new("Drive (V²/Hz)", Role::Drive, f(), d.clone()).stepped());
        }
        let spectrum = views::spectrum(
            arrays,
            "Control Spectrum (SIM)",
            &Axis::new("Frequency", Some("Hz"), Scale::Log),
            &Axis::new("PSD", Some("g²/Hz"), Scale::Log),
            series,
            Some("frequency"),
        );

        let elapsed = self.elapsed_s();
        let control_status = match (finish, snap.control_rms) {
            (Some(fin), _) if fin.outcome == Outcome::Aborted => Status::Fail,
            (_, None) => Status::Info,
            _ if snap.alarm_lines > 0 => Status::Warn,
            _ => Status::Pass,
        };
        let mut rows = vec![
            match snap.control_rms {
                Some(g) => Row::number(
                    "Control rms",
                    views::round_to(g, 3),
                    Some("g"),
                    control_status,
                ),
                None => Row::text("Control rms", "pending", Status::Info),
            },
            Row::number(
                "Reference rms",
                views::round_to(snap.reference_rms, 3),
                Some("g"),
                Status::Info,
            ),
            match snap.drive_rms {
                Some(v) => Row::number("Drive rms", views::round_to(v, 3), Some("V"), Status::Info),
                None => Row::text("Drive rms", "pending", Status::Info),
            },
            Row::number("Level", views::round_to(level, 2), Some("dB"), Status::Info),
            Row::number(
                "Elapsed",
                views::round_to(elapsed, 2),
                Some("s"),
                Status::Info,
            ),
            Row::number(
                "Lines in alarm",
                snap.alarm_lines as f64,
                None,
                if snap.alarm_lines > 0 {
                    Status::Warn
                } else {
                    Status::Pass
                },
            ),
            Row::text(
                "Controller",
                phase_label(snap.phase),
                finish.map_or(Status::Info, |fin| fin.outcome.status()),
            ),
        ];
        if let Some(fin) = finish {
            rows.push(Row::text("Stop reason", &fin.reason, fin.outcome.status()));
        }
        let reading = views::reading("Run Readings (SIM)", rows);

        json!({
            "run_id": self.run_id,
            "source": "SIM",
            "state": state,
            "elapsed_s": views::round_to(elapsed, 3),
            "level_db": views::round_to(level, 2),
            "views": [spectrum, reading],
            "phase": phase_name(snap.phase),
            "outcome": finish.map(|fin| fin.outcome.as_str()),
            "reason": finish.map(|fin| fin.reason.clone()),
        })
    }
}

/// A stable machine name for a controller state.
pub fn phase_name(s: RunState) -> &'static str {
    match s {
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
}

fn phase_label(s: RunState) -> &'static str {
    match s {
        RunState::Idle => "Idle",
        RunState::Feasibility => "Feasibility check",
        RunState::FeasibilityBlocked => "Feasibility blocked",
        RunState::SysId => "System identification",
        RunState::CoherenceGate => "Coherence gate",
        RunState::CoherenceHold => "Coherence hold",
        RunState::AwaitingReview => "Awaiting review",
        RunState::Engaged => "Level ladder",
        RunState::FullLevel => "At test level",
        RunState::Stopped => "Stopped",
        RunState::Aborted => "Aborted",
    }
}

/// The trip record in words.
pub fn describe_trip(t: &TripRecord) -> String {
    let at = format!("at {:.1} dB, control loop {}", t.at_level_db, t.at_loop);
    match &t.kind {
        TripKind::Envelope { abort_lines, worst_db } => format!(
            "envelope abort: {abort_lines} lines outside the abort limits (worst {worst_db:+.1} dB) {at}"
        ),
        TripKind::Kinematic { channel_index, axis, value, limit, .. } => format!(
            "kinematic abort on channel {channel_index}: {axis} {value:.2} exceeds the limit {limit:.2} {at}"
        ),
        TripKind::DriveVrms { realized_vrms, limit_vrms } => format!(
            "drive abort: {realized_vrms:.2} Vrms exceeds the {limit_vrms:.2} Vrms limit {at}"
        ),
        TripKind::Divergence { consecutive_loops, divergent_bins } => format!(
            "divergence abort: {divergent_bins} lines diverging for {consecutive_loops} consecutive loops {at}"
        ),
    }
}

/// The loop settings for a profile: the controller's demo loop on this crate's line
/// grid, with the control band set from the breakpoints (as the controller's own
/// daemon derives it).
pub fn loop_config(profile: &Profile) -> LoopConfig {
    let mut cfg = LoopConfig::demo_default();
    cfg.fs_hz = SAMPLE_RATE_HZ;
    cfg.nfft = NFFT;
    cfg.blocks_per_window = BLOCKS_PER_WINDOW;
    let (lo, hi) = profile.band_hz();
    let lo = lo.max(cfg.df());
    let hi = hi.min(cfg.fs_hz / 2.0 * 0.95);
    if hi > lo {
        cfg.band_lo_hz = lo;
        cfg.band_hi_hz = hi;
    }
    cfg
}

/// The test configuration: an automatic ladder of 3 dB rungs up to the requested
/// level, then the full-level hold.
pub fn test_config(request: &RunRequest) -> RandomTestConfig {
    let p = &request.profile;
    let mut cfg = RandomTestConfig::siso(p.breakpoints.clone(), 1);
    let table_db: Vec<f64> = (0..=LADDER_RUNGS_BELOW)
        .rev()
        .map(|i| request.level_db - LADDER_STEP_DB * i as f64)
        .collect();
    let mut dwell_s = vec![LADDER_DWELL_S; LADDER_RUNGS_BELOW];
    dwell_s.push(0.0);
    cfg.schedule = LevelSchedule {
        mode: RunMode::Auto,
        start_db: table_db[0],
        table_db,
        dwell_s,
    };
    cfg.full_level_duration_s = request.duration_s;
    cfg.alarm_db = p.alarm_db;
    cfg.abort_db = p.abort_db;
    cfg.drive_ceiling_vrms = p.drive_ceiling_vrms;
    cfg
}

/// Control on channel 1 (acceleration), drive on channel 2.
pub fn channel_table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = CONTROL_RANGE_G;
    ctrl.abort.accel_g = Some(CONTROL_ABORT_G);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(ctrl);
    t.channels.push(drv);
    t
}

/// The Sim plant as a controller backend.
pub fn sim_backend(plant: &PlantSpec, fs_hz: f64) -> Box<dyn eigen_hal::Backend> {
    Box::new(
        eigen_hal_sim::SimConfig::new()
            .sample_rate_hz(fs_hz)
            .resonances(plant.modes.clone())
            .noise_level(plant.noise_level)
            .feedthrough(plant.feedthrough)
            // A software plant advances input and output on one sample index; the
            // controller's arm gate requires the claim to be stated.
            .shared_clock(true)
            .seed(plant.seed)
            .max_input_channels(1)
            .max_output_channels(1)
            .build(),
    )
}

fn run_thread(
    request: RunRequest,
    shared: Arc<Shared>,
    ready: mpsc::Sender<Result<Ready, String>>,
) {
    let mut svc = match arm(&request) {
        Ok(svc) => svc,
        Err(reason) => {
            let _ = ready.send(Err(reason));
            return;
        }
    };
    let fs_hz = svc.loop_fs_hz();
    let report = svc.report_data();
    let Some(grid) = DisplayGrid::from_reference(&report.target_psd.psd, report.target_psd.df_hz)
    else {
        let _ = ready.send(Err(
            "the profile has no energy on the controller's line grid".into(),
        ));
        return;
    };
    let sysid_level_db = svc.config().sysid_level_db;
    svc.set_block_observer(Some(pacer(shared.clone(), fs_hz, request.speed)));
    let _ = ready.send(Ok(Ready {
        fs_hz,
        sysid_level_db,
        grid: grid.clone(),
    }));

    let mut ctx = Ctx {
        shared: &shared,
        grid: &grid,
        sysid_level_db,
        engaged: false,
    };
    ctx.publish(&svc, None);
    let finish = drive(&mut svc, &mut ctx);
    ctx.publish(&svc, Some(finish));
    shared.finished.store(true, Ordering::SeqCst);
}

/// Build the service and take it through feasibility and arm.
fn arm(request: &RunRequest) -> Result<RandomService, String> {
    let loop_cfg = loop_config(&request.profile);
    let backend = sim_backend(&request.profile.plant, loop_cfg.fs_hz);
    let mut svc = RandomService::new(
        test_config(request),
        channel_table(),
        loop_cfg,
        backend,
        eigen_module_abi::StationClass::EigenSim,
        // Capture is off; the root is only ever written if a capture is started.
        Some(std::env::temp_dir().join("sds-control")),
    )
    .map_err(|e| format!("the controller refused the configuration: {e}"))?;
    // The protocol has no review verb yet, so the post-identification review hold
    // is off; the coherence gate and every protection still apply.
    svc.set_review_after_sysid(false);
    svc.assess_feasibility()
        .map_err(|e| format!("feasibility check failed: {e}"))?;
    if svc.state() == RunState::FeasibilityBlocked {
        return Err(
            "feasibility exceeded: the profile is beyond the configured shaker's ratings".into(),
        );
    }
    svc.arm()
        .map_err(|e| format!("the controller refused to arm: {e}"))?;
    Ok(svc)
}

/// The per-tick display tap: it counts plant frames (the simulated clock) and, for a
/// paced run, sleeps whenever simulated time runs ahead of wall time.
fn pacer(shared: Arc<Shared>, fs_hz: f64, speed: f64) -> BlockObserver {
    let t0 = Instant::now();
    Arc::new(move |_input: &[f32], n_frames: usize| {
        let frames = shared.frames.fetch_add(n_frames as u64, Ordering::Relaxed) + n_frames as u64;
        if speed <= 0.0 || shared.stop.load(Ordering::Relaxed) {
            return;
        }
        let due_s = frames as f64 / fs_hz / speed;
        let ahead_s = due_s - t0.elapsed().as_secs_f64();
        if ahead_s > 0.005 {
            std::thread::sleep(Duration::from_secs_f64(ahead_s));
        }
    })
}

struct Ctx<'a> {
    shared: &'a Shared,
    grid: &'a DisplayGrid,
    sysid_level_db: f64,
    engaged: bool,
}

impl Ctx<'_> {
    fn stop_requested(&self) -> bool {
        self.shared.stop.load(Ordering::SeqCst)
    }

    fn publish(&self, svc: &RandomService, finish: Option<Finish>) {
        let status = svc.status();
        let report = svc.report_data();
        let level_db = if self.engaged {
            status.current_level_db
        } else {
            self.sysid_level_db
        };
        let snap = Snapshot {
            phase: status.state,
            level_db,
            control_rms: status.measured_grms,
            reference_rms: svc.full_target_grms() * 10f64.powf(level_db / 20.0),
            drive_rms: report.final_drive_psd.as_ref().map(|d| {
                (d.psd.iter().filter(|&&p| p > 0.0).sum::<f64>() * d.df_hz)
                    .max(0.0)
                    .sqrt()
            }),
            alarm_lines: report.final_alarm_lines,
            control_psd: report
                .final_composite_psd
                .as_ref()
                .and_then(|c| self.grid.slice(&c.psd)),
            drive_psd: report
                .final_drive_psd
                .as_ref()
                .and_then(|d| self.grid.slice(&d.psd)),
            finish,
        };
        let snap = Arc::new(snap);
        match self.shared.latest.lock() {
            Ok(mut slot) => *slot = snap,
            Err(poisoned) => *poisoned.into_inner() = snap,
        }
    }
}

/// Run system identification, the gate, and the control loop to the end.
fn drive(svc: &mut RandomService, ctx: &mut Ctx<'_>) -> Finish {
    if let Err(e) = svc.run_sysid(PRETEST_SEGMENTS) {
        svc.fault_from_loop(&e.to_string());
        return Finish {
            outcome: Outcome::Fault,
            reason: format!("system identification failed: {e}"),
        };
    }
    ctx.publish(svc, None);
    if ctx.stop_requested() {
        let _ = svc.stop();
        return operator_stop(svc, ctx, "during system identification");
    }
    match svc.coherence_gate() {
        Ok(true) => {}
        Ok(false) => {
            return Finish {
                outcome: Outcome::Held,
                reason: format!(
                    "coherence gate held: band coherence {:.3} is below the floor {:.2}; \
                     the run never drove at test level",
                    svc.band_coherence(),
                    svc.config().coherence_floor
                ),
            }
        }
        Err(e) => {
            svc.fault_from_loop(&e.to_string());
            return Finish {
                outcome: Outcome::Fault,
                reason: format!("coherence gate failed: {e}"),
            };
        }
    }
    if svc.state() == RunState::AwaitingReview {
        if let Err(e) = svc.proceed_after_review() {
            svc.fault_from_loop(&e.to_string());
            return Finish {
                outcome: Outcome::Fault,
                reason: format!("could not engage after system identification: {e}"),
            };
        }
    }
    ctx.engaged = true;
    ctx.publish(svc, None);

    loop {
        if ctx.stop_requested() {
            let _ = svc.request_stop();
            return operator_stop(svc, ctx, "");
        }
        match svc.loop_tick() {
            Ok(true) => ctx.publish(svc, None),
            Ok(false) => return finish_from_state(svc),
            Err(e) => {
                svc.fault_from_loop(&e.to_string());
                return Finish {
                    outcome: Outcome::Fault,
                    reason: format!("control loop fault: {e}"),
                };
            }
        }
    }
}

fn operator_stop(svc: &RandomService, ctx: &Ctx<'_>, when: &str) -> Finish {
    // A trip that fired on the very loop the stop arrived on still wins.
    if svc.state() == RunState::Aborted {
        return finish_from_state(svc);
    }
    let level = if ctx.engaged {
        svc.status().current_level_db
    } else {
        ctx.sysid_level_db
    };
    let elapsed = ctx.shared.frames.load(Ordering::Relaxed) as f64 / svc.loop_fs_hz();
    let when = if when.is_empty() {
        format!("at {level:.1} dB")
    } else {
        when.to_owned()
    };
    Finish {
        outcome: Outcome::OperatorStop,
        reason: format!("stopped by the operator {when} after {elapsed:.1} s"),
    }
}

fn finish_from_state(svc: &RandomService) -> Finish {
    match svc.state() {
        RunState::Aborted => Finish {
            outcome: Outcome::Aborted,
            reason: svc.trip().map_or_else(
                || {
                    // A loop fault is recorded as a note, not a trip.
                    let note = svc.log().events().iter().rev().find_map(|e| {
                        let t = format!("{:?}", e.kind);
                        t.contains("LOOP FAULT").then_some(t)
                    });
                    format!(
                        "aborted without a trip record{}",
                        note.map(|n| format!(": {n}")).unwrap_or_default()
                    )
                },
                describe_trip,
            ),
        },
        RunState::Stopped => Finish {
            outcome: Outcome::Completed,
            reason: format!(
                "test complete: {:.0} s held at {:.1} dB",
                svc.config().full_level_duration_s,
                svc.status().current_level_db
            ),
        },
        other => Finish {
            outcome: Outcome::Fault,
            reason: format!("the control loop ended in state {}", phase_name(other)),
        },
    }
}
