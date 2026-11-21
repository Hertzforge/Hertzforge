//! The run thread of one MIMO Mixed Mode run: it owns the controller's
//! `MimoMixedService` and its station from arm to the end, takes the requests the
//! socket's tasks queue, keeps the record, and publishes the view the state and
//! the frames are encoded from.
//!
//! The pattern is Mixed Mode's (`crate::mixed::run`): a request is answered
//! between chunks, a stop or abort asked from the socket's thread reaches the
//! station's own stop at once ([`RunShared::signal_end`]), and the run ends by
//! releasing the service and the station before it says how it ended.

use super::config::{round, Derived, WireMimoMixed};
use super::record::{self as mrec, ArmFacts, LevelPoint, Names};
use super::view::{
    self, BandHistory, End, IdentView, MimoMixedView, Spectra, State, StationRef, ToneHistory,
    HISTORY_POINTS,
};
use super::{refuse, wrong_state};
use crate::protocol::{num, MethodError};
use crate::random::backend::{Interruptible, ABORT, NONE};
use crate::random::record as rrec;
use crate::random::run::{LevelTarget, LogEntry, NO_FRAME};
use crate::random::view::RecordRef;
use crate::record::tap::{self, Recording};
use crate::record::{LogHandle, ProblemFn, RecordOptions, RecordStatus, RunRecord};
use crate::sine::record::Blocks;
use crate::station::NiScanner;
use crate::station::{self, class_name, StationPlan};
use hertzforge_channel_table::ChannelTable;
use hertzforge_mimo_mixed::service::records::LawRecord;
use hertzforge_mimo_mixed::service::{
    MimoMixedLaws, MimoMixedPhase, MimoMixedService, MimoMixedState, MimoMixedStatus,
};
use hertzforge_mimo_mixed::MimoMixedConfig;
use hertzforge_random::RunMode;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// How often a running test publishes its view.
pub const PUBLISH_PERIOD: Duration = Duration::from_millis(200);
pub const SHUTDOWN_BY: &str = crate::random::run::SHUTDOWN_BY;
/// Chunks the held drive peak is taken over (about a second).
const PEAK_CHUNKS: usize = 4;
/// Frames the composite's moving average is taken over.
const COMPOSITE_AVERAGES: f64 = 8.0;

/// A publish, as the service hears it: the run id and the view published.
pub type Notice = (String, Arc<MimoMixedView>);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// What the run thread and the service share.
pub struct RunShared {
    view: Mutex<Arc<MimoMixedView>>,
    log: Mutex<Vec<LogEntry>>,
    /// The station signal: [`ABORT`] takes the drive down with the station's own
    /// stop at the next engine block.
    pub signal: Arc<AtomicU8>,
    /// A stop asked from the socket's thread while the identification plays: the
    /// run ends once it returns.
    pub stop_asked: AtomicBool,
    /// Who sent the stop or abort.
    pub ended_by: Mutex<Option<String>>,
    notify: tokio::sync::mpsc::UnboundedSender<Notice>,
    /// The `mimo_mixed.live` task has been started for this run.
    pub live_started: AtomicBool,
    /// The run's record from arm on: every log entry is appended to it as it is made.
    pub record: Mutex<Option<LogHandle>>,
    /// The input frames the station has delivered since it started
    /// ([`NO_FRAME`] before the first), for the record's entries.
    pub station_frame: Arc<AtomicU64>,
}

impl RunShared {
    pub fn new(
        initial: MimoMixedView,
        notify: tokio::sync::mpsc::UnboundedSender<Notice>,
    ) -> RunShared {
        RunShared {
            view: Mutex::new(Arc::new(initial)),
            log: Mutex::new(Vec::new()),
            signal: Arc::new(AtomicU8::new(NONE)),
            stop_asked: AtomicBool::new(false),
            ended_by: Mutex::new(None),
            notify,
            live_started: AtomicBool::new(false),
            record: Mutex::new(None),
            station_frame: Arc::new(AtomicU64::new(NO_FRAME)),
        }
    }

    pub fn view(&self) -> Arc<MimoMixedView> {
        lock(&self.view).clone()
    }

    pub fn log_since(&self, since: u64) -> (Vec<LogEntry>, u64) {
        let log = lock(&self.log);
        let next = log.len() as u64;
        let from = (since as usize).min(log.len());
        (log[from..].to_vec(), next)
    }

    pub fn add_log(&self, kind: &'static str, text: impl Into<String>) {
        self.add_log_data(kind, text, None);
    }

    /// A log entry with structured facts for the record (`data`).
    pub fn add_log_data(&self, kind: &'static str, text: impl Into<String>, data: Option<Value>) {
        let mut log = lock(&self.log);
        let seq = log.len() as u64;
        let entry = LogEntry {
            seq,
            at_unix: now_unix(),
            kind,
            text: text.into(),
        };
        if let Some(r) = lock(&self.record).as_ref() {
            r.append(self.record_entry(&entry, data));
        }
        log.push(entry);
    }

    /// An entry as the record holds it.
    pub fn record_entry(&self, e: &LogEntry, data: Option<Value>) -> Value {
        let frame = self.station_frame.load(Ordering::Relaxed);
        let mut v = json!({"seq": e.seq, "at_unix": e.at_unix,
                           "frame": if frame == NO_FRAME { Value::Null } else { Value::from(frame) },
                           "kind": e.kind, "text": e.text});
        if let (Some(d), Value::Object(m)) = (data, &mut v) {
            m.insert("data".into(), d);
        }
        v
    }

    /// The entries so far into the record, and from now on each as it is made.
    pub fn attach_record(&self, handle: LogHandle) {
        let log = lock(&self.log);
        for e in log.iter() {
            handle.append(self.record_entry(e, None));
        }
        *lock(&self.record) = Some(handle);
    }

    pub fn detach_record(&self) {
        let _log = lock(&self.log);
        *lock(&self.record) = None;
    }

    pub fn publish(&self, view: MimoMixedView) {
        let id = view.run_id.clone();
        let view = Arc::new(view);
        *lock(&self.view) = view.clone();
        let _ = self.notify.send((id, view));
    }

    /// Ask the run to stop or abort from the socket's thread, before the request is
    /// queued. An abort reaches the station's own stop at the next block.
    pub fn signal_end(&self, abort: bool, by: &str) {
        {
            let mut b = lock(&self.ended_by);
            if b.is_none() {
                *b = Some(by.to_owned());
            }
        }
        if abort {
            self.signal.store(ABORT, Ordering::SeqCst);
        }
        self.stop_asked.store(true, Ordering::SeqCst);
    }
}

/// What a run arms on, taken from the service when the request arrives.
pub struct ArmContext {
    pub table: ChannelTable,
    pub station: StationPlan,
    pub scanner: Arc<dyn NiScanner>,
    pub data_root: PathBuf,
    pub bind: String,
    /// The run's laws, resolved from the catalog for the station's class; `Err`
    /// is why they could not be (`law_not_checked` on hardware).
    pub laws: Result<MimoMixedLaws, MethodError>,
    pub revision: u64,
    pub devices: Vec<hertzforge_hal::DiscoveredDevice>,
}

type Reply<T> = oneshot::Sender<Result<T, MethodError>>;

/// A request to the run thread.
pub enum Cmd {
    Arm {
        ctx: ArmContext,
        reply: Reply<()>,
    },
    Start {
        ctx: ArmContext,
        reply: Reply<()>,
    },
    Proceed {
        reply: Reply<()>,
    },
    Level {
        target: LevelTarget,
        reply: Reply<f64>,
    },
    Hold {
        on: bool,
        reply: Reply<()>,
    },
    Part {
        id: String,
        on: bool,
        by: String,
        reply: Reply<bool>,
    },
    Stop {
        outcome: &'static str,
        by: String,
        reply: Option<Reply<()>>,
    },
    Abort {
        by: String,
        reply: Reply<()>,
    },
}

/// The fixed facts of one run.
pub struct RunPlan {
    pub run_id: String,
    pub wire: Arc<WireMimoMixed>,
    pub core: Arc<MimoMixedConfig>,
    pub facts: Arc<Derived>,
    /// Simulated seconds per wall second on the simulator; 0 runs unpaced (tests).
    pub sim_speed: f64,
    pub capture_root: Option<PathBuf>,
    pub timeout_s: f64,
    pub record: RecordOptions,
}

/// What the record is kept with while the run goes.
struct Keeping {
    record: RunRecord,
    status: Arc<RecordStatus>,
    run_uid: String,
    names: Names,
    levels: Blocks<LevelPoint>,
    trace: Blocks<LawRecord>,
    /// The last spectra, the level they are at and the station frame then.
    spectra: Option<(Spectra, f64, u64)>,
    spectra_kept: u64,
    /// The level the last spectra record was kept at.
    kept_level: Option<f64>,
    last_t: f64,
    identified: bool,
}

/// What the timeline compares one chunk's status against.
#[derive(Default)]
struct Seen {
    level_db: Option<f64>,
    steady_level: Option<f64>,
    tone_on: Vec<bool>,
    tone_limiting: Vec<bool>,
    tone_alarm: Vec<bool>,
    band_on: Vec<bool>,
    band_alarm: Vec<bool>,
    floor_alarm: bool,
    equalized: bool,
}

struct Actor {
    plan: RunPlan,
    shared: Arc<RunShared>,
    svc: Option<MimoMixedService>,
    lock: Option<hertzforge_station::lock::LockGuard>,
    view: MimoMixedView,
    forwarded: usize,
    stop: Option<(&'static str, String)>,
    stop_at_t: Option<f64>,
    last_publish: Option<Instant>,
    pace_origin: Option<(Instant, f64)>,
    driving_level: Option<f64>,
    keep: Option<Keeping>,
    seen: Seen,
    peaks: Vec<Vec<f64>>,
    composite: Option<(Vec<Vec<f64>>, f64)>,
    tones: Vec<ToneHistory>,
    bands: Vec<BandHistory>,
}

fn arm_refused(words: impl std::fmt::Display) -> MethodError {
    refuse("arm_refused", format!("The run cannot arm: {words}"))
}

/// Start the run thread. Returns once the thread exists; the run is `configured`.
pub fn spawn(
    plan: RunPlan,
    initial: MimoMixedView,
    shared: Arc<RunShared>,
    rx: Receiver<Cmd>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let name = format!("mimo-mixed-{}", plan.run_id);
    std::thread::Builder::new().name(name).spawn(move || {
        let mut a = Actor {
            plan,
            shared,
            svc: None,
            lock: None,
            view: initial,
            forwarded: 0,
            stop: None,
            stop_at_t: None,
            last_publish: None,
            pace_origin: None,
            driving_level: None,
            keep: None,
            seen: Seen::default(),
            peaks: Vec::new(),
            composite: None,
            tones: Vec::new(),
            bands: Vec::new(),
        };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| a.run(rx)));
        if r.is_err() {
            a.shared.signal.store(ABORT, Ordering::SeqCst);
            if let Some(svc) = a.svc.as_mut() {
                let _ = svc.abort("the run thread failed");
            }
            a.svc = None;
            a.lock = None;
            let msg = "The run stopped on a fault: its thread failed; the drive was taken \
                       down with the station's own stop."
                .to_owned();
            a.view.end = Some(End {
                outcome: "fault",
                name: None,
                message: msg.clone(),
                at_s: a.view.t_s,
                level_db: a.view.level_db,
                part: None,
                control: None,
                drive: None,
                ramp_s: None,
            });
            a.view.state = State::Finished;
            a.view.phase = "aborted";
            a.shared
                .add_log_data("note", msg, Some(json!({"outcome": "fault"})));
            a.finish_record();
            a.shared.publish(a.view.clone());
        }
    })
}

impl Actor {
    fn run(&mut self, rx: Receiver<Cmd>) {
        loop {
            match (self.view.state, self.view.phase) {
                (State::Running, "sysid") => self.identify(),
                (State::Running, _) | (State::Stopping, _) => {
                    loop {
                        match rx.try_recv() {
                            Ok(cmd) => self.handle(cmd),
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => {
                                self.abandon();
                                return;
                            }
                        }
                    }
                    if matches!(self.view.state, State::Running | State::Stopping) {
                        self.pump();
                    }
                }
                _ => match rx.recv() {
                    Ok(cmd) => self.handle(cmd),
                    Err(_) => {
                        self.abandon();
                        return;
                    }
                },
            }
        }
    }

    /// The service dropped this run: release anything held, the drive first.
    fn abandon(&mut self) {
        if let Some(svc) = self.svc.as_mut() {
            self.shared.signal.store(ABORT, Ordering::SeqCst);
            let _ = svc.abort("the service released the run");
        }
        self.svc = None;
        self.lock = None;
        self.shared.detach_record();
        self.keep = None;
    }

    fn publish(&mut self) {
        if let (Some(k), Some(r)) = (self.keep.as_ref(), self.view.record.as_mut()) {
            r.problem = k.status.problem();
        }
        self.view.tones = Arc::new(self.tones.clone());
        self.view.bands = Arc::new(self.bands.clone());
        self.last_publish = Some(Instant::now());
        self.shared.publish(self.view.clone());
    }

    fn set(&mut self, state: State, phase: &'static str) {
        if self.view.phase != phase {
            self.shared.add_log_data(
                "state",
                format!(
                    "{} to {}",
                    self.view.phase.replace('_', " "),
                    phase.replace('_', " ")
                ),
                Some(json!({"from": self.view.phase, "to": phase, "state": state.name()})),
            );
        }
        self.view.state = state;
        self.view.phase = phase;
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Arm { ctx, reply } => {
                let r = self.arm_cmd(ctx);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Start { ctx, reply } => {
                let r = self.start_cmd(ctx);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Proceed { reply } => {
                let r = self.proceed_cmd();
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Level { target, reply } => {
                let r = self.level_cmd(target);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Hold { on, reply } => {
                let r = self.hold_cmd(on);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Part { id, on, by, reply } => {
                let r = self.part_cmd(&id, on, &by);
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Stop { outcome, by, reply } => {
                self.stop_cmd(outcome, by);
                self.publish();
                if let Some(reply) = reply {
                    let _ = reply.send(Ok(()));
                }
            }
            Cmd::Abort { by, reply } => {
                if self.view.state.is_live() {
                    self.abort_now(&by);
                }
                self.publish();
                let _ = reply.send(Ok(()));
            }
        }
    }

    // ---- arm and start -------------------------------------------------------------

    fn finished_refusal(&self) -> MethodError {
        refuse(
            "run_finished",
            "This run has finished; reset or configure again to run.",
        )
    }

    fn arm_cmd(&mut self, ctx: ArmContext) -> Result<(), MethodError> {
        match self.view.state {
            State::Configured | State::Blocked => self.arm(ctx),
            State::Finished => Err(self.finished_refusal()),
            _ => Err(wrong_state(&self.view, "Arming")),
        }
    }

    fn start_cmd(&mut self, ctx: ArmContext) -> Result<(), MethodError> {
        match self.view.state {
            State::Configured => self.arm(ctx)?,
            State::Armed => {}
            State::Finished => return Err(self.finished_refusal()),
            _ => return Err(wrong_state(&self.view, "Starting")),
        }
        self.shared.stop_asked.store(false, Ordering::SeqCst);
        self.set(State::Running, "sysid");
        self.view.streaming = true;
        if let Some(k) = self.keep.as_ref() {
            let frame = self.shared.station_frame.load(Ordering::Relaxed);
            let mut patch = Map::new();
            patch.insert(
                "start".into(),
                rrec::start_json(
                    now_unix(),
                    rrec::utc_offset_s(),
                    if frame == NO_FRAME { 0 } else { frame },
                ),
            );
            k.record.patch_manifest(patch);
        }
        Ok(())
    }

    fn arm(&mut self, ctx: ArmContext) -> Result<(), MethodError> {
        let laws = ctx.laws?;
        let table = ctx.table.clone();
        let station = station::build(&ctx.station, &*ctx.scanner)?;
        let hardware = station.station.is_hardware();
        let lock = if hardware {
            Some(
                hertzforge_station::lock::acquire(
                    &ctx.data_root,
                    &ctx.bind,
                    &station.provenance.station_label,
                )
                .map_err(|e| match e {
                    hertzforge_station::lock::LockError::Held(h) => refuse(
                        "hardware_busy",
                        format!("Another program holds the NI hardware: {}", h.advice()),
                    )
                    .with_detail(json!({"holder": h.advice()})),
                    hertzforge_station::lock::LockError::Io(m) => arm_refused(m),
                })?,
            )
        } else {
            None
        };
        let class = station.station;
        let provenance = station.provenance.clone();
        let caps = station.backend.caps();
        let wire = self.plan.wire.clone();
        // The streams are taken where the station's blocks pass (record::tap), from
        // the first block the station plays, the identification included.
        let buffers = if wire.streams {
            rrec::tap_buffers(wire.fs_hz, self.plan.record.stream_buffer_s)
        } else {
            1
        };
        let (inputs_tap, inputs_link) = tap::pair(
            rrec::input_rows(&table).len(),
            rrec::TAP_BLOCK_FRAMES,
            buffers,
        );
        let (drives_tap, drives_link) = tap::pair(
            mrec::played_drives(&table).len(),
            rrec::TAP_BLOCK_FRAMES,
            buffers,
        );
        let recorded = Recording::wrap(
            station.backend,
            inputs_tap,
            drives_tap,
            self.shared.station_frame.clone(),
        );
        let backend = Interruptible::wrap(recorded, self.shared.signal.clone());
        let mut svc = MimoMixedService::new(
            (*self.plan.core).clone(),
            table.clone(),
            backend,
            class,
            laws,
        )
        .map_err(|e| arm_refused(&e).with_detail(json!({"reason": e.name()})))?;
        if !hardware {
            svc = svc.with_block_frames(hertzforge_hal::default_block_frames(wire.fs_hz));
        }
        // The laws' calls are kept for the record, taken between chunks.
        svc.keep_law_records(true);
        svc.arm()
            .map_err(|e| arm_refused(&e).with_detail(json!({"reason": e.name()})))?;
        self.forward_log(&svc);
        let fs = svc.sample_rate_hz();
        self.view.grid.fs_hz = fs;
        self.view.grid.df_hz = fs / self.view.grid.n_pts.max(1) as f64;
        self.view.grid.actual = true;
        self.view.station = StationRef {
            kind: if matches!(ctx.station, StationPlan::Sim { .. }) {
                "sim"
            } else {
                "ni"
            },
            class: class_name(class),
            devices: provenance
                .device_ids
                .iter()
                .cloned()
                .zip(provenance.serials.iter().cloned())
                .collect(),
        };

        // The run's record, before any sample: refused here, the arm is refused and
        // the station released (nothing has been driven).
        let armed_unix = now_unix();
        let utc_offset_s = rrec::utc_offset_s();
        let run_uid = rrec::run_uid(armed_unix, &self.plan.run_id);
        let root = self
            .plan
            .capture_root
            .clone()
            .unwrap_or_else(hertzforge_capture::default_data_root);
        let streams_off = if !wire.streams {
            Some("switched off in the setup".to_owned())
        } else {
            match hertzforge_capture::free_space_bytes_for(&root) {
                Ok(free) if free < self.plan.record.free_space_floor_bytes => Some(format!(
                    "the disk had {} MB free, under the {} MB kept free",
                    free / 1_000_000,
                    self.plan.record.free_space_floor_bytes / 1_000_000
                )),
                _ => None,
            }
        };
        let facts = ArmFacts {
            run_id: &self.plan.run_id,
            run_uid: &run_uid,
            cfg: &wire,
            controller: svc.config(),
            derived: &self.plan.facts,
            table: &table,
            revision: ctx.revision,
            plan: &ctx.station,
            class,
            provenance: &provenance,
            devices: &ctx.devices,
            caps: &caps,
            fs_hz: fs,
            armed_unix,
            utc_offset_s,
            streams_off: streams_off.clone(),
        };
        let begin = mrec::begin(&facts, root, (inputs_link, drives_link));
        let shared = self.shared.clone();
        let problem: ProblemFn = Box::new(move |words: String, data: Value| {
            shared.add_log_data("record", words.clone(), Some(data));
            let mut v = (*shared.view()).clone();
            if let Some(r) = v.record.as_mut() {
                r.problem = Some(words);
                shared.publish(v);
            }
        });
        let record = RunRecord::open(begin, &self.plan.record, problem).map_err(|e| {
            refuse(
                "record_unwritable",
                format!(
                    "The run's record could not be written, so the run did not arm: {} ({}). \
                     Choose a data folder that can be written.",
                    e.path.display(),
                    e.reason
                ),
            )
            .with_detail(json!({"path": e.path.display().to_string(), "reason": e.reason}))
        })?;
        self.shared.attach_record(record.log());
        if let Some(why) = &streams_off {
            self.shared.add_log_data(
                "record",
                format!("the run's streams are not recorded: {why}"),
                Some(json!({"stream": null, "failed": why})),
            );
        }
        let names = Names::of(svc.config(), &table);
        let (name, d, arrays) =
            mrec::reference_result(svc.config(), &names, &self.plan.facts.source);
        record.put_result(&name, d, arrays, None);
        self.view.record = Some(RecordRef {
            folder: record.folder().display().to_string(),
            streams: streams_off.is_none(),
            problem: None,
        });
        self.keep = Some(Keeping {
            status: record.status(),
            record,
            run_uid,
            names,
            levels: Blocks::new(mrec::LEVEL_BLOCK),
            trace: Blocks::new(mrec::TRACE_BLOCK),
            spectra: None,
            spectra_kept: 0,
            kept_level: None,
            last_t: f64::NEG_INFINITY,
            identified: false,
        });
        let cfg = svc.config();
        self.tones = cfg
            .tones()
            .map(|(_, p, _)| ToneHistory {
                id: p.id.clone(),
                ..Default::default()
            })
            .collect();
        self.bands = cfg
            .bands()
            .map(|(_, p, _)| BandHistory {
                id: p.id.clone(),
                ..Default::default()
            })
            .collect();
        self.svc = Some(svc);
        self.lock = lock;
        self.set(State::Armed, "armed");
        self.shared.add_log_data(
            "station",
            format!(
                "armed on {} at {} Hz, shared clock",
                class_name(class),
                num(round(fs, 3)),
            ),
            Some(json!({"class": class_name(class), "fs_hz": fs, "event": "arm"})),
        );
        Ok(())
    }

    // ---- identification and the gate --------------------------------------------------

    fn identify(&mut self) {
        let Some(svc) = self.svc.as_mut() else {
            self.fault("the run lost its station before identification");
            return;
        };
        self.shared.add_log_data(
            "identification",
            "identification started",
            Some(json!({"event": "identification_start"})),
        );
        let r = svc.identify().map(|_| ());
        self.forward_log_now();
        if self.shared.signal.load(Ordering::SeqCst) == ABORT {
            self.abort_now(&self.ender());
            return;
        }
        if let Err(e) = r {
            if self.svc.as_ref().map(MimoMixedService::state) == Some(MimoMixedState::Ended) {
                self.finish_from_controller();
            } else {
                self.fault(&format!("identification failed: {e}"));
            }
            return;
        }
        if self.shared.stop_asked.load(Ordering::SeqCst) {
            self.end_stopped(self.stop_outcome());
            return;
        }
        self.set(State::Running, "coherence_gate");
        self.publish();
        let Some(svc) = self.svc.as_mut() else { return };
        let gate = svc.coherence_gate();
        self.forward_log_now();
        self.ident_facts();
        self.keep_identification();
        match gate {
            Err(e) => self.fault(&format!("the coherence gate failed: {e}")),
            Ok(false) => {
                let c = self
                    .view
                    .identification
                    .as_ref()
                    .map(|i| i.band_coherence.clone())
                    .unwrap_or_default();
                let gate = self.plan.core.gates.coherence_threshold;
                self.end(
                    "held",
                    "coherence_hold",
                    None,
                    Some(format!(
                        "Coherence gate held: band coherence per control {:?} is below the gate \
                         of {gate:.2}; the run never drove at test level.",
                        c.iter().map(|x| round(*x, 3)).collect::<Vec<_>>()
                    )),
                    None,
                );
            }
            Ok(true) => {
                self.set(State::Review, "awaiting_review");
                self.publish();
            }
        }
    }

    fn ident_facts(&mut self) {
        let Some(id) = self.svc.as_ref().and_then(|s| s.identification()) else {
            return;
        };
        let gate = self.plan.core.gates.coherence_threshold;
        let v = IdentView {
            level_db: id.level_db,
            steps: id.steps,
            frames: id.frames,
            lag_frames: id.lag_frames,
            band_coherence: id.band_coherence.clone(),
            gate,
            passed: id.band_coherence.iter().all(|c| *c >= gate),
            drive_vrms: id.drive_vrms.clone(),
        };
        self.shared.add_log_data(
            "identification",
            format!(
                "identified at {} dB in {} steps: band coherence {:?}, gate {}",
                num(round(v.level_db, 2)),
                v.steps,
                v.band_coherence
                    .iter()
                    .map(|x| round(*x, 3))
                    .collect::<Vec<_>>(),
                if v.passed { "passed" } else { "failed" }
            ),
            Some(
                json!({"event": "identification", "level_db": v.level_db, "steps": v.steps,
                        "band_coherence": v.band_coherence, "passed": v.passed,
                        "lag_frames": v.lag_frames}),
            ),
        );
        self.view.identification = Some(v);
    }

    fn ender(&self) -> String {
        lock(&self.shared.ended_by)
            .clone()
            .unwrap_or_else(|| "the operator".into())
    }

    fn stop_outcome(&self) -> &'static str {
        self.stop.as_ref().map_or("operator_stop", |(o, _)| *o)
    }

    fn fault(&mut self, words: &str) {
        self.shared.signal.store(ABORT, Ordering::SeqCst);
        if let Some(svc) = self.svc.as_mut() {
            let _ = svc.abort(words);
        }
        self.forward_log_now();
        self.end(
            "fault",
            "aborted",
            None,
            Some(format!("The run stopped on a fault: {words}")),
            None,
        );
    }

    // ---- review and engage --------------------------------------------------------

    fn proceed_cmd(&mut self) -> Result<(), MethodError> {
        if self.view.state != State::Review {
            return Err(wrong_state(&self.view, "Proceeding"));
        }
        self.shared.add_log_data(
            "note",
            "the operator reviewed the identification and proceeded",
            Some(json!({"action": "proceed"})),
        );
        let svc = self
            .svc
            .as_mut()
            .ok_or_else(|| wrong_state(&self.view, "Proceeding"))?;
        let r = svc.proceed();
        let state = svc.state();
        self.forward_log_now();
        match r {
            Ok(()) => {
                self.shared.add_log_data(
                    "engage",
                    "engaged: the floor plays alone at the soft-start level",
                    Some(json!({"event": "engage"})),
                );
                self.set(State::Running, "floor_only");
                self.refresh();
                Ok(())
            }
            Err(_) if state == MimoMixedState::Ended => {
                self.finish_from_controller();
                Ok(())
            }
            Err(e) => Err(refuse("wrong_state", e.to_string())),
        }
    }

    // ---- the loop -----------------------------------------------------------------

    fn pump(&mut self) {
        if self.shared.signal.load(Ordering::SeqCst) == ABORT {
            self.abort_now(&self.ender());
            return;
        }
        let Some(svc) = self.svc.as_mut() else {
            self.fault("the run lost its station");
            return;
        };
        let r = svc.run_chunk();
        match r {
            Ok(true) => {
                self.forward_log_now();
                self.refresh();
                self.keep_results();
                self.pace();
                let due = self
                    .last_publish
                    .is_none_or(|t| t.elapsed() >= PUBLISH_PERIOD);
                let shown = self.shared.view();
                if due || view::state_key(Some(&shown)) != view::state_key(Some(&self.view)) {
                    self.publish();
                }
            }
            Ok(false) => {
                self.forward_log_now();
                self.refresh();
                self.keep_results();
                self.finish_from_controller();
            }
            Err(e) => {
                if self.shared.signal.load(Ordering::SeqCst) == ABORT {
                    self.abort_now(&self.ender());
                } else if self.svc.as_ref().map(MimoMixedService::state)
                    == Some(MimoMixedState::Ended)
                {
                    self.forward_log_now();
                    self.finish_from_controller();
                } else {
                    self.fault(&format!("the loop failed: {e}"));
                }
            }
        }
    }

    fn pace(&mut self) {
        if self.plan.sim_speed <= 0.0 || self.view.station.kind != "sim" {
            return;
        }
        let Some(t) = self.view.t_s else { return };
        let (w0, t0) = *self.pace_origin.get_or_insert((Instant::now(), t));
        let ahead = (t - t0) / self.plan.sim_speed - w0.elapsed().as_secs_f64();
        if ahead > 0.002 {
            std::thread::sleep(Duration::from_secs_f64(ahead.min(0.25)));
        }
    }

    /// Read the controller's status into the view, the spectra and histories,
    /// and the timeline's entries for what changed since the last chunk.
    fn refresh(&mut self) {
        let Some(svc) = self.svc.take() else { return };
        self.refresh_from(&svc);
        self.svc = Some(svc);
    }

    fn refresh_from(&mut self, svc: &MimoMixedService) {
        let st = svc.status();
        if st.state == MimoMixedState::Ended {
            self.view.status = Some(st);
            return;
        }
        let stopping = self.view.state == State::Stopping || st.phase == MimoMixedPhase::Stopping;
        if st.state == MimoMixedState::Running {
            let phase = view::phase_name(st.phase);
            if stopping {
                if self.stop_at_t.is_none() {
                    self.stop_at_t = Some(st.t_s);
                }
                self.set(State::Stopping, "ramping_down");
            } else {
                self.set(State::Running, phase);
                self.driving_level = Some(st.level_db);
            }
        }
        self.view.t_s = Some(st.t_s);
        self.view.level_db = Some(st.level_db);
        // The held peak: the largest of the last chunks.
        self.peaks
            .push(st.drives.iter().map(|d| d.peak_v).collect());
        if self.peaks.len() > PEAK_CHUNKS {
            self.peaks.remove(0);
        }
        let m = st.drives.len();
        self.view.held_peak_v = (0..m)
            .map(|j| {
                self.peaks
                    .iter()
                    .filter_map(|p| p.get(j).copied())
                    .fold(0.0f64, f64::max)
            })
            .collect();
        self.timeline(&st);
        self.spectra(svc, st.level_db);
        let phasors = svc.tone_phasors();
        self.histories(&st, &phasors);
        self.view.status = Some(st);
    }

    /// The timeline's entries for what changed: a level settled, a part in or
    /// out, a tone limiting or released, an alarm raised or cleared, the floor
    /// equalized.
    fn timeline(&mut self, st: &MimoMixedStatus) {
        let sh = self.shared.clone();
        let t = st.t_s;
        let s = &mut self.seen;
        let level = round(st.level_db, 2);
        if s.level_db.is_some_and(|l| (l - level).abs() < 0.01)
            && s.steady_level.is_none_or(|l| (l - level).abs() >= 0.01)
        {
            sh.add_log_data(
                "level",
                format!("level {} dB", num(level)),
                Some(json!({"event": "level", "level_db": level, "t_s": t})),
            );
            s.steady_level = Some(level);
        }
        s.level_db = Some(level);
        if st.floor_equalized && !s.equalized {
            sh.add_log_data(
                "floor",
                "the floor is equalized",
                Some(json!({"event": "floor_equalized", "t_s": t})),
            );
        }
        s.equalized = st.floor_equalized;
        let floor_alarm = view::alarm_floor(&self.view, st);
        if floor_alarm != s.floor_alarm {
            sh.add_log_data(
                "alarm",
                if floor_alarm {
                    "the floor is outside its alarm band".to_owned()
                } else {
                    "the floor is back inside its alarm band".to_owned()
                },
                Some(json!({"event": if floor_alarm { "alarm" } else { "alarm_cleared" },
                            "part": self.view.parts.iter().find(|p| p.kind == "floor").map(|p| p.id.clone()),
                            "t_s": t})),
            );
            s.floor_alarm = floor_alarm;
        }
        s.tone_on.resize(st.tones.len(), false);
        s.tone_limiting.resize(st.tones.len(), false);
        s.tone_alarm.resize(st.tones.len(), false);
        for (k, tone) in st.tones.iter().enumerate() {
            let on = tone.part_level > 0.0;
            if on != s.tone_on[k] {
                sh.add_log_data(
                    "part",
                    format!("tone {} {}", tone.id, if on { "in" } else { "out" }),
                    Some(json!({"event": if on { "part_in" } else { "part_out" },
                                "part": tone.id, "t_s": t})),
                );
                s.tone_on[k] = on;
            }
            if tone.limiting != s.tone_limiting[k] {
                sh.add_log_data(
                    "limit",
                    if tone.limiting {
                        format!(
                            "tone {} limiting: the shared ceiling scales it by {}",
                            tone.id,
                            num(round(tone.scale, 3))
                        )
                    } else {
                        format!("tone {} released", tone.id)
                    },
                    Some(
                        json!({"event": if tone.limiting { "limiting" } else { "released" },
                                "part": tone.id, "scale": tone.scale, "t_s": t}),
                    ),
                );
                s.tone_limiting[k] = tone.limiting;
            }
            let alarm: Vec<usize> = (0..tone.alarm.len()).filter(|i| tone.alarm[*i]).collect();
            let any = !alarm.is_empty();
            if any != s.tone_alarm[k] {
                let controls: Vec<u32> = alarm
                    .iter()
                    .filter_map(|i| self.view.controls.get(*i).copied())
                    .collect();
                sh.add_log_data(
                    "alarm",
                    if any {
                        format!(
                            "tone {} outside its alarm band at controls {controls:?}",
                            tone.id
                        )
                    } else {
                        format!("tone {} back inside its alarm band", tone.id)
                    },
                    Some(json!({"event": if any { "alarm" } else { "alarm_cleared" },
                                "part": tone.id, "controls": controls, "t_s": t})),
                );
                s.tone_alarm[k] = any;
            }
        }
        s.band_on.resize(st.bands.len(), false);
        s.band_alarm.resize(st.bands.len(), false);
        for (k, b) in st.bands.iter().enumerate() {
            let on = b.part_level > 0.0;
            if on != s.band_on[k] {
                sh.add_log_data(
                    "part",
                    format!("band {} {}", b.id, if on { "in" } else { "out" }),
                    Some(json!({"event": if on { "part_in" } else { "part_out" },
                                "part": b.id, "folded": b.folded, "t_s": t})),
                );
                s.band_on[k] = on;
            }
            let any = b.alarm.iter().any(|a| *a);
            if any != s.band_alarm[k] {
                let controls: Vec<u32> = (0..b.alarm.len())
                    .filter(|i| b.alarm[*i])
                    .filter_map(|i| self.view.controls.get(i).copied())
                    .collect();
                sh.add_log_data(
                    "alarm",
                    if any {
                        format!(
                            "band {} outside its alarm band at controls {controls:?}",
                            b.id
                        )
                    } else {
                        format!("band {} back inside its alarm band", b.id)
                    },
                    Some(json!({"event": if any { "alarm" } else { "alarm_cleared" },
                                "part": b.id, "controls": controls, "t_s": t})),
                );
                s.band_alarm[k] = any;
            }
        }
    }

    /// The spectra on the band at the level in force: the composite averaged from
    /// the floor loop's frames, the residual and the drive from the controller's
    /// full-level matrices.
    fn spectra(&mut self, svc: &MimoMixedService, level_db: f64) {
        let core = &self.plan.core;
        let band = self.view.grid.band;
        let n = core.n();
        let q = n + core.p();
        let g = 10f64.powf(level_db / 10.0);
        let (Some(resid), Some(drive)) = (svc.floor_measured(), svc.floor_drive()) else {
            return;
        };
        if let Some(x) = svc.frame_spectra() {
            let frame: Vec<Vec<f64>> = (0..n)
                .map(|i| {
                    (band.0..band.1)
                        .map(|l| x.get(l * q + i).map_or(f64::NAN, |z| z.norm_sqr()))
                        .collect()
                })
                .collect();
            let (avg, k) = self.composite.get_or_insert_with(|| (frame.clone(), 0.0));
            *k = (*k + 1.0).min(COMPOSITE_AVERAGES);
            for (a, f) in avg.iter_mut().zip(&frame) {
                for (a, f) in a.iter_mut().zip(f) {
                    *a += (f - *a) / *k;
                }
            }
        }
        let (g_res, _) = resid;
        let diag = |h: &hertzforge_sdm::HermitianStack, i: usize| -> Vec<f64> {
            (band.0..band.1)
                .map(|l| {
                    if l < h.lines() {
                        h.get(l, i, i).re * g
                    } else {
                        f64::NAN
                    }
                })
                .collect()
        };
        let (reference, alarm, abort) = view::reference_lines(core, band, level_db);
        let sp = Spectra {
            composite: self
                .composite
                .as_ref()
                .map(|c| c.0.clone())
                .unwrap_or_default(),
            residual: (0..n).map(|i| diag(&g_res, i)).collect(),
            cross: (0..n.saturating_sub(1))
                .map(|i| {
                    (
                        i,
                        i + 1,
                        (band.0..band.1)
                            .map(|l| {
                                if l < g_res.lines() {
                                    g_res.get(l, i, i + 1).norm() * g
                                } else {
                                    f64::NAN
                                }
                            })
                            .collect(),
                    )
                })
                .collect(),
            reference,
            alarm,
            abort,
            drive: (0..drive.n()).map(|j| diag(drive, j)).collect(),
        };
        let frame = self.shared.station_frame.load(Ordering::Relaxed);
        if let Some(k) = self.keep.as_mut() {
            k.spectra = Some((
                sp.clone(),
                level_db,
                if frame == NO_FRAME { 0 } else { frame },
            ));
        }
        self.view.spectra = Some(Arc::new(sp));
    }

    fn histories(
        &mut self,
        st: &MimoMixedStatus,
        phasors: &[hertzforge_mimo_mixed::service::TonePhasors],
    ) {
        if st.state != MimoMixedState::Running {
            return;
        }
        let trim = |v: &mut Vec<f64>| {
            if v.len() > HISTORY_POINTS {
                v.remove(0);
            }
        };
        for (k, h) in self.tones.iter_mut().enumerate() {
            let (Some(t), Some(p)) = (st.tones.get(k), phasors.get(k)) else {
                continue;
            };
            h.t.push(st.t_s);
            trim(&mut h.t);
            h.freq_hz.push(t.frequency_hz);
            trim(&mut h.freq_hz);
            let n = p.measured.len();
            h.amplitude.resize(n, Vec::new());
            h.phase_deg.resize(n, Vec::new());
            h.error_db.resize(n, Vec::new());
            for i in 0..n {
                h.amplitude[i].push(p.measured[i].norm());
                trim(&mut h.amplitude[i]);
                h.phase_deg[i].push(p.measured[i].arg().to_degrees());
                trim(&mut h.phase_deg[i]);
                h.error_db[i].push(t.error_db.get(i).copied().unwrap_or(f64::NAN));
                trim(&mut h.error_db[i]);
            }
        }
        for (k, h) in self.bands.iter_mut().enumerate() {
            let Some(b) = st.bands.get(k) else { continue };
            h.t.push(st.t_s);
            trim(&mut h.t);
            h.error_db.resize(b.error_db.len(), Vec::new());
            for (i, e) in b.error_db.iter().enumerate() {
                h.error_db[i].push(*e);
                trim(&mut h.error_db[i]);
            }
        }
    }

    fn driving(&self) -> bool {
        self.view.state == State::Running
            && matches!(self.view.phase, "floor_only" | "engaged" | "full_level")
    }

    // ---- verbs --------------------------------------------------------------------

    fn level_cmd(&mut self, target: LevelTarget) -> Result<f64, MethodError> {
        if !self.driving() {
            return Err(wrong_state(&self.view, "A level change"));
        }
        if self.plan.core.schedule.mode != RunMode::Manual {
            return Err(refuse(
                "manual_mode_required",
                "The level moves by hand only in manual mode; configure the schedule's mode as \
                 manual.",
            )
            .with_detail(json!({"mode": "auto"})));
        }
        let svc = self
            .svc
            .as_mut()
            .ok_or_else(|| wrong_state(&self.view, "A level change"))?;
        let r = match target {
            LevelTarget::Delta(d) => svc.step_level(d),
            LevelTarget::To(t) => svc.set_level_db(t),
        };
        let level = r.map_err(|e| refuse("wrong_state", e.to_string()))?;
        self.shared.add_log_data(
            "level",
            format!("level to {} dB asked", num(round(level, 2))),
            Some(json!({"action": "level", "level_db": level})),
        );
        Ok(level)
    }

    fn hold_cmd(&mut self, on: bool) -> Result<(), MethodError> {
        if !self.driving() {
            return Err(wrong_state(
                &self.view,
                if on { "Holding" } else { "Resuming" },
            ));
        }
        let svc = self
            .svc
            .as_mut()
            .ok_or_else(|| wrong_state(&self.view, "Holding"))?;
        if on { svc.hold() } else { svc.resume() }
            .map_err(|e| refuse("wrong_state", e.to_string()))?;
        self.view.held = on;
        self.shared.add_log_data(
            "hold",
            if on {
                "progress held"
            } else {
                "progress resumed"
            },
            Some(json!({"action": if on { "hold" } else { "resume" }, "held": on})),
        );
        Ok(())
    }

    fn part_refused(id: &str, why: &'static str, words: String) -> MethodError {
        refuse("part_refused", words).with_detail(json!({"id": id, "why": why}))
    }

    fn part_cmd(&mut self, id: &str, on: bool, by: &str) -> Result<bool, MethodError> {
        let Some(p) = self.plan.core.parts.iter().find(|p| p.id == id) else {
            return Err(Self::part_refused(
                id,
                "unknown_part",
                format!("The test has no part \"{id}\"."),
            ));
        };
        if p.kind() == "floor" && !on {
            return Err(Self::part_refused(
                id,
                "floor",
                "The floor plays for the whole run; it cannot be switched off.".into(),
            ));
        }
        if !self.driving() {
            return Err(wrong_state(&self.view, "Switching a part"));
        }
        if self.view.enabled.get(id) == Some(&on) {
            return Ok(on);
        }
        let svc = self
            .svc
            .as_mut()
            .ok_or_else(|| wrong_state(&self.view, "Switching a part"))?;
        svc.set_part_enabled(id, on).map_err(|e| match e {
            hertzforge_mimo_mixed::service::MimoMixedError::Refused { name, why, .. } => {
                Self::part_refused(id, name, format!("Part {id} cannot be switched: {why}."))
            }
            e => refuse("wrong_state", e.to_string()),
        })?;
        self.view.enabled.insert(id.to_owned(), on);
        self.shared.add_log_data(
            "part",
            format!(
                "part {id} switched {} by {by}",
                if on { "on" } else { "off" }
            ),
            Some(json!({"action": "part", "part": id, "on": on, "by": by})),
        );
        Ok(on)
    }

    // ---- the end ------------------------------------------------------------------

    fn stop_cmd(&mut self, outcome: &'static str, by: String) {
        if !self.view.state.is_live() || self.view.state == State::Stopping {
            return;
        }
        self.stop = Some((outcome, by.clone()));
        self.shared.add_log_data(
            "note",
            if outcome == "safe_stop" && by == SHUTDOWN_BY {
                "the control service is shutting down; stopping the run with its controlled stop"
                    .to_owned()
            } else if outcome == "safe_stop" {
                format!(
                    "nobody held control for {} s; stopping the run with its controlled stop",
                    num(self.plan.timeout_s)
                )
            } else {
                format!("stop requested by {by}")
            },
            Some(json!({"action": outcome, "by": by})),
        );
        match self.view.state {
            State::Armed | State::Review => self.end_stopped(outcome),
            State::Running if self.driving() => {
                let Some(svc) = self.svc.as_mut() else { return };
                let t = svc.status().t_s;
                let r = svc.stop();
                self.forward_log_now();
                if r.is_err() {
                    self.end_stopped(outcome);
                } else {
                    self.stop_at_t = Some(t);
                    self.set(State::Stopping, "ramping_down");
                }
            }
            _ => {}
        }
    }

    fn abort_now(&mut self, by: &str) {
        self.shared.signal.store(ABORT, Ordering::SeqCst);
        if let Some(svc) = self.svc.as_mut() {
            let _ = svc.abort(&format!("aborted by {by}"));
        }
        {
            let mut b = lock(&self.shared.ended_by);
            if b.is_none() {
                *b = Some(by.to_owned());
            }
        }
        self.end("operator_abort", "aborted", None, None, None);
    }

    /// End before or after the drive played (nothing ramps).
    fn end_stopped(&mut self, outcome: &'static str) {
        self.end(outcome, "stopped", None, None, None);
    }

    /// The controller ended the run: how, by its end's name.
    fn finish_from_controller(&mut self) {
        let Some(svc) = self.svc.as_ref() else { return };
        let st = svc.status();
        let t_end = st.t_s;
        let Some(e) = svc.end().cloned() else {
            self.fault("the loop ended without saying how");
            return;
        };
        if self.shared.signal.load(Ordering::SeqCst) == ABORT && self.stop.is_none() {
            self.end("operator_abort", "aborted", None, None, None);
            return;
        }
        let ramp = self.stop_at_t.map(|s| (t_end - s).max(0.0));
        match e.name {
            "completed" => self.end(
                "completed",
                "stopped",
                Some(&e),
                Some(
                    "Test complete: the schedule finished and every part ramped down with the \
                     controlled stop."
                        .into(),
                ),
                ramp,
            ),
            "stopped" => {
                let outcome = self.stop_outcome();
                self.end(outcome, "stopped", Some(&e), None, ramp)
            }
            "operator_abort" => self.end("operator_abort", "aborted", Some(&e), None, None),
            "ident_unreachable" => self.end(
                "sysid_level_unreachable",
                "aborted",
                Some(&e),
                Some(format!(
                    "The identification could not reach its level: {}.",
                    e.words
                )),
                None,
            ),
            "law_fault" => self.end(
                "law_fault",
                if e.aborted { "aborted" } else { "stopped" },
                Some(&e),
                Some(format!("The run stopped on a law fault: {}.", e.words)),
                ramp,
            ),
            _ => self.end(
                "aborted",
                "aborted",
                Some(&e),
                Some(format!("Aborted ({}): {}.", e.name, e.words)),
                None,
            ),
        }
    }

    /// End the run, release the service and the station, then say how it ended.
    fn end(
        &mut self,
        outcome: &'static str,
        phase: &'static str,
        run_end: Option<&hertzforge_mimo_mixed::service::RunEnd>,
        message: Option<String>,
        ramp_s: Option<f64>,
    ) {
        if self.view.state == State::Finished {
            return;
        }
        let by = self
            .stop
            .as_ref()
            .map(|(_, b)| b.clone())
            .unwrap_or_else(|| self.ender());
        let level = self.driving_level.or(self.view.level_db);
        let at = match level {
            Some(l) => format!(
                "{} dB after {} s",
                num(round(l, 2)),
                num(round(self.view.t_s.unwrap_or(0.0), 2))
            ),
            None => "before driving".to_owned(),
        };
        let message = message.unwrap_or_else(|| match outcome {
            "operator_stop" => format!("Stopped by {by} at {at}."),
            "operator_abort" => format!("Aborted by {by} at {at}."),
            "safe_stop" if by == SHUTDOWN_BY => format!(
                "The control service was shut down, so it stopped the run with its controlled \
                 stop, from {at}."
            ),
            "safe_stop" => format!(
                "Nobody held control for {} s, so the service stopped the run with its \
                 controlled stop, from {at}.",
                num(self.plan.timeout_s)
            ),
            _ => "The run ended.".to_owned(),
        });
        self.forward_log_now();
        if ramp_s.is_none() && matches!(outcome, "aborted" | "operator_abort" | "fault") {
            self.shared.signal.store(ABORT, Ordering::SeqCst);
        }
        // The last law calls before the service goes.
        if let (Some(k), Some(svc)) = (self.keep.as_mut(), self.svc.as_mut()) {
            for call in svc.take_law_records() {
                if let Some(block) = k.trace.push(call) {
                    Self::put_trace(k, &block);
                }
            }
        }
        // The release order: the service and the station go, then the end is said.
        self.svc = None;
        self.lock = None;
        let core = &self.plan.core;
        let control = run_end
            .and_then(|e| e.control)
            .and_then(|i| core.controls.get(i).copied());
        let drive = run_end
            .and_then(|e| e.drive)
            .and_then(|j| core.drives.get(j).map(|d| d.channel_index));
        self.view.end = Some(End {
            outcome,
            name: run_end.map(|e| e.name.to_owned()),
            message: message.clone(),
            at_s: self.view.t_s,
            level_db: level,
            part: run_end.and_then(|e| e.part.clone()),
            control,
            drive,
            ramp_s,
        });
        self.set(State::Finished, phase);
        self.view.held = false;
        let end = self.view.end.as_ref().map(view::end_json);
        self.shared.add_log_data(
            if phase == "aborted" { "abort" } else { "stop" },
            message,
            Some(
                json!({"event": if phase == "aborted" { "abort" } else { "stop" },
                        "outcome": outcome, "end": end}),
            ),
        );
        self.finish_record();
        self.publish();
    }

    // ---- the record ---------------------------------------------------------------

    fn keep_identification(&mut self) {
        let (Some(k), Some(svc)) = (self.keep.as_mut(), self.svc.as_ref()) else {
            return;
        };
        if k.identified {
            return;
        }
        let Some(id) = svc.identification() else {
            return;
        };
        for (name, d, arrays) in
            mrec::identification_results(id, self.view.grid.band, self.view.grid.df_hz, &k.names)
        {
            k.record.put_result(&name, d, arrays, None);
        }
        k.identified = true;
    }

    /// After a chunk: the laws' calls, the level point and, at a new steady
    /// level, the last level's spectra. Only copies and sends.
    fn keep_results(&mut self) {
        let (Some(k), Some(svc)) = (self.keep.as_mut(), self.svc.as_mut()) else {
            return;
        };
        for call in svc.take_law_records() {
            if let Some(block) = k.trace.push(call) {
                Self::put_trace(k, &block);
            }
        }
        let Some(st) = self.view.status.as_ref() else {
            return;
        };
        if st.state != MimoMixedState::Running || st.t_s <= k.last_t {
            return;
        }
        k.last_t = st.t_s;
        let point = LevelPoint {
            t_s: st.t_s,
            level_db: st.level_db,
            status: st.clone(),
            phasors: svc.tone_phasors(),
            held_peak_v: self.view.held_peak_v.clone(),
            peak_ceiling_v: self.plan.core.drives.iter().map(|d| d.peak_v).collect(),
        };
        if let Some(block) = k.levels.push(point) {
            Self::put_levels(k, &block);
        }
        if let Some(level) = self.seen.steady_level {
            if k.kept_level.is_some_and(|l| (l - level).abs() >= 0.01) {
                Self::put_spectra(k, "level");
            }
            if k.kept_level.is_none_or(|l| (l - level).abs() >= 0.01) {
                k.kept_level = Some(level);
            }
        }
    }

    fn put_spectra(k: &mut Keeping, why: &str) {
        let Some((sp, level, frame)) = k.spectra.take() else {
            return;
        };
        for (name, desc, arrays) in mrec::spectra_result(&sp, k.names.band, k.names.df_hz, &k.names)
        {
            let line = json!({"index": k.spectra_kept, "why": why, "level_db": level,
                              "frame": frame});
            k.record.put_result(&name, desc, arrays, Some(line));
        }
        k.spectra_kept += 1;
    }

    fn put_levels(k: &mut Keeping, block: &[LevelPoint]) {
        let index = k.levels.kept;
        for (name, d, arrays) in mrec::levels_results(block, &k.names) {
            let line = json!({"index": index, "points": block.len(),
                              "first_s": block.first().map(|p| p.t_s),
                              "last_s": block.last().map(|p| p.t_s)});
            k.record.put_result(&name, d, arrays, Some(line));
        }
        k.levels.kept += 1;
    }

    fn put_trace(k: &mut Keeping, block: &[LawRecord]) {
        let (name, d, arrays) = mrec::trace_result(block, &k.names);
        let line = json!({"index": k.trace.kept, "calls": block.len(),
                          "first_s": block.first().map(|r| r.t_s),
                          "last_s": block.last().map(|r| r.t_s)});
        k.record.put_result(&name, d, arrays, Some(line));
        k.trace.kept += 1;
    }

    /// End the record: the part-filled blocks, the last spectra, `end.json`, then
    /// `complete`.
    fn finish_record(&mut self) {
        let Some(mut k) = self.keep.take() else {
            return;
        };
        if let Some(block) = k.levels.rest() {
            Self::put_levels(&mut k, &block);
        }
        if let Some(block) = k.trace.rest() {
            Self::put_trace(&mut k, &block);
        }
        Self::put_spectra(&mut k, "end");
        self.shared.detach_record();
        let state = view::state_json(Some(&self.view));
        let frame = self.shared.station_frame.load(Ordering::Relaxed);
        let mut end = Map::new();
        end.insert("run_uid".into(), Value::from(k.run_uid.as_str()));
        end.insert("end".into(), state["end"].clone());
        end.insert("at_unix".into(), Value::from(now_unix()));
        end.insert(
            "station_frames".into(),
            Value::from(if frame == NO_FRAME { 0 } else { frame }),
        );
        end.insert(
            "summary".into(),
            json!({"t_s": self.view.t_s,
                   "level_records": k.levels.kept, "trace_records": k.trace.kept,
                   "spectra_records": k.spectra_kept}),
        );
        if let Some(r) = self.view.record.as_mut() {
            r.problem = k.status.problem();
        }
        k.record.finish(end);
    }

    // ---- the controller's log -------------------------------------------------------

    fn forward_log_now(&mut self) {
        if let Some(svc) = self.svc.take() {
            self.forward_log(&svc);
            self.svc = Some(svc);
        }
    }

    /// The controller's events since the last call, into the run log as notes.
    fn forward_log(&mut self, svc: &MimoMixedService) {
        let events = svc.events();
        for e in events.iter().skip(self.forwarded) {
            self.shared
                .add_log_data("note", e.clone(), Some(json!({"source": "controller"})));
        }
        self.forwarded = events.len();
    }
}
