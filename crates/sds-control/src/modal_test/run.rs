//! One configured Modal Test: its view, its start, and the run's own thread, which
//! owns the session (trigger, hits, frames, averages), the host and the record. The
//! loop thread only captures; every analysis, every result and every file write is on
//! the run's thread or the record's.

use super::config::WireModal;
use super::records::{self, StartFacts};
use super::sim_source::{SimShared, SimSource};
use crate::protocol::{num, ErrorCode, MethodError};
use crate::pulse::rails::{now_unix, Shared};
use crate::random::record as rrec;
use crate::record::tap::{self, Recording};
use crate::record::{ProblemFn, RecordOptions, RecordStatus, RunRecord};
use crate::station::{self, NiScanner, StationPlan};
use hertzforge_channel_table::ChannelTable;
use hertzforge_modal_test::host::{
    capture_only_table, stop_ramp_frames, DriveBuild, DrivePhase, HostBuild, HostError, ModalHost,
    Pacing,
};
use hertzforge_modal_test::plan::Plan;
use hertzforge_modal_test::session::{Event, Session, State as SState};
use hertzforge_modal_test::setup::Mode;
use hertzforge_modal_test::source::SourceBuild;
use hertzforge_modal_test::structure::HitKind;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How often the run's thread looks.
pub const POLL: Duration = Duration::from_millis(10);
/// How long a start may take.
pub const START_WAIT: Duration = Duration::from_secs(120);
/// How long a call to the run's thread may wait for its answer.
pub const CALL_WAIT: Duration = Duration::from_secs(10);
/// Points of the time display.
pub const TIME_POINTS: usize = 1024;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

pub fn refuse(reason: &'static str, message: impl Into<String>) -> MethodError {
    MethodError::named(ErrorCode::Refused, reason, message.into())
}

/// How a test ended.
#[derive(Debug, Clone, PartialEq)]
pub struct End {
    /// `ended` (the operator ended it), `aborted`, `response_abort`, `drive_underrun`,
    /// `source_failed`, `drive_ceiling`, `input_overload`, `loop_faulted`,
    /// `loop_panicked`, `run_panicked` (the run's own thread failed), `run_not_started`
    /// (its thread could not start) or `arm_refused`.
    pub outcome: &'static str,
    pub message: String,
    pub emergency_stop: bool,
    pub at_s: Option<f64>,
}

impl End {
    pub fn to_json(&self) -> Value {
        json!({"outcome": self.outcome, "message": self.message,
               "emergency_stop": self.emergency_stop, "at_s": self.at_s.map(num)})
    }
}

/// Where the record is.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordRef {
    pub folder: String,
    pub streams: bool,
    pub problem: Option<String>,
}

/// What the state shows.
#[derive(Debug, Clone, PartialEq)]
pub struct ModalView {
    pub run_id: String,
    /// `configured`, `starting`, the session's state (`idle`, `armed`, `captured`,
    /// `measuring`, `point_done`), `stopping`, or `finished`.
    pub state: &'static str,
    pub mode: &'static str,
    pub source: &'static str,
    pub station_class: &'static str,
    pub title: String,
    pub drive_phase: &'static str,
    /// The session's counts, the selected point's hits and the latest hit.
    pub counts: Value,
    pub hits: Value,
    pub latest: Value,
    pub point: usize,
    pub point_dofs: Value,
    pub record: Option<RecordRef>,
    pub end: Option<End>,
}

impl ModalView {
    pub fn is_live(&self) -> bool {
        !matches!(self.state, "configured" | "finished")
    }
}

/// The view a configured test starts from.
pub fn configured_view(
    run_id: &str,
    w: &WireModal,
    session: &Session,
    source: &'static str,
    class: &'static str,
) -> ModalView {
    ModalView {
        run_id: run_id.to_owned(),
        state: "configured",
        mode: w.setup.mode.name(),
        source,
        station_class: class,
        title: w.title.clone(),
        drive_phase: "idle",
        counts: session.counts_json(),
        hits: Value::Array(Vec::new()),
        latest: Value::Null,
        point: 0,
        point_dofs: dofs_json(session, 0),
        record: None,
        end: None,
    }
}

fn dofs_json(s: &Session, p: usize) -> Value {
    let m = &s.plan.measurements[p];
    let (refs, resps) = s.plan.rows_json(m);
    json!({"references": refs, "responses": resps})
}

/// `modal_test.state`.
pub fn state_json(v: Option<&ModalView>) -> Value {
    match v {
        None => json!({"run_id": null, "state": "none"}),
        Some(v) => json!({
            "run_id": v.run_id,
            "state": v.state,
            "mode": v.mode,
            "source": v.source,
            "station_class": v.station_class,
            "title": v.title,
            "drive_phase": v.drive_phase,
            "point": v.point,
            "point_dofs": v.point_dofs,
            "counts": v.counts,
            "hits": v.hits,
            "latest": v.latest,
            "record": v.record.as_ref().map(|r| json!({"folder": r.folder, "streams": r.streams, "problem": r.problem})),
            "end": v.end.as_ref().map(End::to_json),
        }),
    }
}

/// What makes a new `modal_test.state` worth sending.
pub fn state_key(v: Option<&ModalView>) -> String {
    state_json(v).to_string()
}

/// The display: the latest frames of every measured channel, and the selected point's
/// current estimate.
#[derive(Debug, Clone, Default)]
pub struct Display {
    pub rate_hz: f64,
    pub channels: Vec<Vec<f64>>,
    pub freqs: Vec<f64>,
    /// `[response][reference]` magnitude and phase (degrees) per line.
    pub mag: Vec<Vec<Vec<f64>>>,
    pub phase_deg: Vec<Vec<Vec<f64>>>,
    /// `[response]` per line: the ordinary coherence (one reference) or the multiple.
    pub coherence: Vec<Vec<f64>>,
    pub averages: usize,
}

/// `modal_test.live`.
pub fn live_json(v: &ModalView, d: &Display, arrays: &mut crate::codec::Arrays) -> Value {
    use crate::codec::{Array, ArrayData};
    let mut put = |id: String, x: &[f64]| {
        arrays.insert(
            id,
            Array::vector(ArrayData::F32(x.iter().map(|v| *v as f32).collect())),
        )
    };
    let channels: Vec<Value> = d
        .channels
        .iter()
        .enumerate()
        .map(|(k, x)| json!({"values": put(format!("t{k}"), x)}))
        .collect();
    let freqs = put("f".into(), &d.freqs);
    let mut frf = Vec::new();
    for (i, row) in d.mag.iter().enumerate() {
        for (j, m) in row.iter().enumerate() {
            frf.push(json!({"response": i, "reference": j,
                "magnitude": put(format!("m{i}_{j}"), m),
                "phase_deg": put(format!("p{i}_{j}"), &d.phase_deg[i][j])}));
        }
    }
    let coh: Vec<Value> = d
        .coherence
        .iter()
        .enumerate()
        .map(|(i, c)| json!({"response": i, "values": put(format!("c{i}"), c)}))
        .collect();
    json!({
        "run_id": v.run_id,
        "state": v.state,
        "point": v.point,
        "time": {"rate_hz": num(d.rate_hz), "channels": channels},
        "frf": {"frequency_hz": freqs, "averages": d.averages, "pairs": frf, "coherence": coh},
    })
}

/// What a start needs from the service.
pub struct StartContext {
    pub table: ChannelTable,
    pub station: StationPlan,
    pub scanner: Arc<dyn NiScanner>,
    pub data_root: PathBuf,
    pub records_root: PathBuf,
    pub bind: String,
    pub revision: u64,
    pub devices: Vec<hertzforge_hal::DiscoveredDevice>,
    pub record: RecordOptions,
    pub sim_speed: f64,
}

/// A call to the run's thread.
#[derive(Debug, Clone)]
pub enum Cmd {
    Select(usize),
    Step(i64),
    Arm,
    Disarm,
    Accept,
    Reject,
    Undo,
    TargetFromHit,
    DriveStart,
    DriveStop,
    End,
    SimHit(HitKind),
}

type Reply = mpsc::Sender<Result<Value, MethodError>>;

/// The service's handle on one configured test.
pub struct ModalRun {
    pub id: String,
    pub shared: Arc<Shared<ModalView>>,
    pub wire: Arc<WireModal>,
    pub plan: Plan,
    /// The configure report and its warnings.
    pub report: Value,
    pub config: Value,
    pub display: Mutex<Display>,
    pub sim: Arc<SimShared>,
    session: Mutex<Option<Session>>,
    host: Mutex<Option<Arc<ModalHost>>>,
    cmds: Mutex<Option<mpsc::Sender<(Cmd, Reply)>>>,
    started: AtomicBool,
    operator_abort: AtomicBool,
}

impl ModalRun {
    pub fn new(
        id: String,
        shared: Arc<Shared<ModalView>>,
        wire: WireModal,
        plan: Plan,
        session: Session,
        report: Value,
    ) -> ModalRun {
        ModalRun {
            id,
            shared,
            config: wire.raw.clone(),
            wire: Arc::new(wire),
            plan,
            report,
            display: Mutex::new(Display::default()),
            sim: Arc::new(SimShared::new()),
            session: Mutex::new(Some(session)),
            host: Mutex::new(None),
            cmds: Mutex::new(None),
            started: AtomicBool::new(false),
            operator_abort: AtomicBool::new(false),
        }
    }

    pub fn view(&self) -> Arc<ModalView> {
        self.shared.view()
    }

    pub fn started(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    pub fn host(&self) -> Option<Arc<ModalHost>> {
        lock(&self.host).clone()
    }

    pub fn mark_starting(&self) {
        self.started.store(true, Ordering::SeqCst);
        let mut v = (*self.view()).clone();
        v.state = "starting";
        self.shared.publish(&self.id, v);
    }

    /// Ask the run's thread, and wait for its answer.
    pub fn call(&self, cmd: Cmd) -> Result<Value, MethodError> {
        let tx = lock(&self.cmds)
            .clone()
            .ok_or_else(|| refuse("not_started", "The test has not started; start it first."))?;
        let (rtx, rrx) = mpsc::channel();
        tx.send((cmd, rtx))
            .map_err(|_| refuse("not_live", "The test has ended."))?;
        rrx.recv_timeout(CALL_WAIT)
            .map_err(|_| MethodError::internal("the test's thread did not answer in time"))?
    }

    /// The abort, named for `by`: the drive (if any) to zero at once, then the test ends.
    pub fn abort(&self, by: &str) {
        self.shared.set_ended_by(by);
        self.operator_abort.store(true, Ordering::SeqCst);
        self.shared.add_log_data(
            "operator",
            format!("abort asked by {by}"),
            Some(json!({"action": "abort", "by": by})),
        );
        if let Some(h) = self.host() {
            h.abort();
        }
    }

    /// End the test for `by` (the controlled way: the drive ramps down first).
    /// It does not wait: the run's thread takes the end on its next look.
    pub fn end(&self, by: &str) {
        self.shared.set_ended_by(by);
        let sent = lock(&self.cmds).as_ref().is_some_and(|tx| {
            let (rtx, _rrx) = mpsc::channel();
            tx.send((Cmd::End, rtx)).is_ok()
        });
        if !sent {
            if let Some(h) = self.host() {
                h.finish();
            } else {
                self.operator_abort.store(true, Ordering::SeqCst);
            }
        }
    }

    pub fn start(self: &Arc<Self>, ctx: StartContext) -> Result<(), MethodError> {
        match self.start_inner(ctx) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.end_before_start(&e);
                Err(e)
            }
        }
    }

    fn end_before_start(&self, e: &MethodError) {
        let mut v = (*self.view()).clone();
        if v.state == "finished" {
            return;
        }
        v.state = "finished";
        v.end = Some(End {
            outcome: "arm_refused",
            message: e.message.clone(),
            emergency_stop: false,
            at_s: None,
        });
        self.shared.add_log_data(
            "state",
            format!("start refused: {}", e.message),
            Some(json!({"event": "refused", "reason": e.reason})),
        );
        self.shared.publish(&self.id, v);
    }

    fn streams_off(&self, ctx: &StartContext) -> Option<String> {
        if !self.wire.record_streams {
            return Some("switched off in the setup".to_owned());
        }
        match hertzforge_capture::free_space_bytes_for(&ctx.records_root) {
            Ok(free) if free < ctx.record.free_space_floor_bytes => Some(format!(
                "the disk had {} MB free, under the {} MB kept free",
                free / 1_000_000,
                ctx.record.free_space_floor_bytes / 1_000_000
            )),
            _ => None,
        }
    }

    fn start_inner(self: &Arc<Self>, ctx: StartContext) -> Result<(), MethodError> {
        let s = &self.wire.setup;
        let built = station::build(&ctx.station, &*ctx.scanner)?;
        let hardware = built.station.is_hardware();
        let guard = if hardware {
            Some(
                hertzforge_station::lock::acquire(
                    &ctx.data_root,
                    &ctx.bind,
                    &built.provenance.station_label,
                )
                .map_err(|e| match e {
                    hertzforge_station::lock::LockError::Held(h) => refuse(
                        "hardware_busy",
                        format!("Another program holds the NI hardware: {}", h.advice()),
                    ),
                    hertzforge_station::lock::LockError::Io(m) => {
                        refuse("arm_refused", format!("The station refused to arm: {m}"))
                    }
                })?,
            )
        } else {
            None
        };
        let caps = built.backend.caps();
        let fs = s.fs_hz;
        let engine_table = match s.mode {
            Mode::Impact => capture_only_table(&ctx.table),
            Mode::Shaker => ctx.table.clone(),
        };
        let streams_off = self.streams_off(&ctx);
        let buffers = if streams_off.is_none() {
            rrec::tap_buffers(fs, ctx.record.stream_buffer_s)
        } else {
            1
        };
        let (inputs_tap, inputs_link) = tap::pair(
            rrec::input_rows(&engine_table).len(),
            rrec::TAP_BLOCK_FRAMES,
            buffers,
        );
        let n_drives = rrec::drive_rows(&engine_table).len();
        let (drives_tap, drives_link) = tap::pair(n_drives, rrec::TAP_BLOCK_FRAMES, buffers);
        let class = built.station;
        let provenance = built.provenance.clone();
        let record_until = Arc::new(AtomicU64::new(u64::MAX));
        // On the simulator the structure is written over the measured rows, beneath the
        // record's taps, so the streams hold what the test measured.
        let inner_backend: Box<dyn hertzforge_hal::Backend> = if hardware {
            built.backend
        } else {
            Box::new(SimSource::new(
                built.backend,
                &self.plan,
                s.mode,
                fs,
                &engine_table,
                self.sim.clone(),
            ))
        };
        let backend = Recording::wrap_until(
            inner_backend,
            inputs_tap,
            drives_tap,
            self.shared.station_frame.clone(),
            Some(record_until.clone()),
        );
        let started_unix = now_unix();
        let run_uid = rrec::run_uid(started_unix, &self.id);
        let begin = records::begin(
            &StartFacts {
                run: self,
                run_uid: &run_uid,
                table: &ctx.table,
                engine_table: &engine_table,
                revision: ctx.revision,
                plan: &ctx.station,
                class,
                provenance: &provenance,
                devices: &ctx.devices,
                caps: &caps,
                started_unix,
                utc_offset_s: rrec::utc_offset_s(),
                streams_off: streams_off.clone(),
                windows: lock(&self.session)
                    .as_ref()
                    .map_or(Value::Null, |s| s.windows.to_json()),
            },
            ctx.records_root.clone(),
            (inputs_link, drives_link),
        );
        let shared = self.shared.clone();
        let problem: ProblemFn = Box::new(move |words: String, data: Value| {
            shared.add_log_data("record", words, Some(data));
        });
        let record = RunRecord::open(begin, &ctx.record, problem).map_err(|e| {
            refuse(
                "record_unwritable",
                format!(
                    "The test's record could not be written, so the test did not start: {} ({}). Choose a data folder that can be written.",
                    e.path.display(),
                    e.reason
                ),
            )
            .with_detail(json!({"path": e.path.display().to_string(), "reason": e.reason}))
        })?;
        self.shared.attach_record(record.log());
        let folder = record.folder().display().to_string();
        if let Some(why) = &streams_off {
            self.shared.add_log_data(
                "record",
                format!("the test's streams are not recorded: {why}"),
                Some(json!({"stream": null, "failed": why})),
            );
        }
        self.shared.add_log("note", format!("records in {folder}"));
        let mut v = (*self.view()).clone();
        v.record = Some(RecordRef {
            folder,
            streams: streams_off.is_none(),
            problem: None,
        });
        self.shared.publish(&self.id, v);
        let session = lock(&self.session)
            .take()
            .ok_or_else(|| MethodError::internal("the test's session is gone"))?;
        let finish_early = |record: RunRecord, session: &Session, e: MethodError| {
            self.shared.detach_record();
            record.finish(records::end_map(
                &run_uid,
                &self.view(),
                now_unix(),
                None,
                session,
            ));
            Err(e)
        };
        if self.operator_abort.load(Ordering::SeqCst) {
            return finish_early(
                record,
                &session,
                refuse(
                    "arm_refused",
                    "The start was stopped before the station opened.",
                ),
            );
        }
        let pacing = if hardware {
            Pacing::Station
        } else {
            Pacing::WallClock {
                speed: ctx.sim_speed.max(1e-3),
            }
        };
        let drive = (s.mode == Mode::Shaker).then(|| {
            let lines = s.lines();
            (
                SourceBuild {
                    kind: s.shaker.source,
                    fs_hz: fs,
                    block: s.block,
                    lo_line: *lines.start(),
                    hi_line: *lines.end(),
                    levels: self.plan.drives.iter().map(|d| d.level_vrms).collect(),
                    burst_pct: s.shaker.burst_pct,
                    delay_frames: s.shaker.delay_frames,
                    seed: s.shaker.seed,
                },
                DriveBuild {
                    outputs: self.plan.outputs,
                    slots: self.plan.drives.iter().map(|d| d.slot).collect(),
                    block: s.block,
                    ramp_frames: s.ramp_frames(),
                    ceiling_v: s.shaker.ceiling_v,
                    stop_ramp_frames: stop_ramp_frames(fs),
                },
            )
        });
        let host = match ModalHost::start(
            HostBuild {
                fs_hz: fs,
                block: s.block,
                drive,
                pacing,
                record_until: Some(record_until),
            },
            engine_table,
            backend,
            class,
        ) {
            Ok(h) => Arc::new(h),
            Err(e) => {
                let err = match e {
                    HostError::Rate { asked, got } => refuse("rate_mismatch", e.to_string())
                        .with_detail(json!({"asked_hz": num(asked), "programmed_hz": num(got)})),
                    HostError::Arm(m) => {
                        refuse("arm_refused", format!("The station refused to arm: {m}"))
                    }
                };
                self.shared
                    .add_log("check", format!("start refused: {}", err.message));
                return finish_early(record, &session, err);
            }
        };
        *lock(&self.host) = Some(host.clone());
        if self.operator_abort.load(Ordering::SeqCst) {
            host.abort();
        }
        self.shared.add_log_data(
            "state",
            format!(
                "started on {}: {} mode, {} Hz, block {}, {} point{}",
                provenance.station_label,
                s.mode.name(),
                num(fs),
                s.block,
                self.plan.measurements.len(),
                if self.plan.measurements.len() == 1 { "" } else { "s" }
            ),
            Some(json!({"event": "started", "mode": s.mode.name(), "fs_hz": num(fs), "block": s.block})),
        );
        let (tx, rx) = mpsc::channel();
        *lock(&self.cmds) = Some(tx);
        let run = self.clone();
        let facts = Facts {
            status: record.status(),
            record: Some(record),
            run_uid,
            hardware,
            _guard: guard,
        };
        // The thread's parts wait in a slot it takes them from, so that, when it cannot
        // start, they come back here to stop the station and close the record.
        let slot = Arc::new(Mutex::new(Some((session, facts, rx))));
        let taken = slot.clone();
        let h = host.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("modal-test {}", self.id))
            .spawn(move || {
                let Some((mut session, mut facts, rx)) = lock(&taken).take() else {
                    return;
                };
                let r = h.guard(|| run.watch(&h, &mut session, rx, &mut facts));
                if let Err(why) = r {
                    run.watch_failed(&h, &mut session, &mut facts, &why);
                }
            });
        if let Err(e) = spawned {
            let message =
                format!("the test's thread could not start ({e}); the station was stopped");
            host.abort_for("run_not_started", message.clone());
            host.join(Duration::from_secs(10));
            if let Some((session, mut facts, _)) = lock(&slot).take() {
                let mut v = (*self.view()).clone();
                v.state = "finished";
                v.end = Some(End {
                    outcome: "run_not_started",
                    message: message.clone(),
                    emergency_stop: true,
                    at_s: None,
                });
                self.shared.publish(&self.id, v);
                self.shared.add_log_data(
                    "outcome",
                    format!("ended (run_not_started): {message}"),
                    Some(json!({"outcome": "run_not_started", "emergency_stop": true})),
                );
                self.finish_record(&mut facts, &session);
            }
            return Err(MethodError::internal(message));
        }
        let asked = Instant::now();
        loop {
            let v = self.view();
            if v.state != "starting" {
                return Ok(());
            }
            if asked.elapsed() > START_WAIT {
                return Err(MethodError::internal("the test did not start in time"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The run's thread failed: the host has taken the drive to zero with the
    /// emergency stop; the end is published and the record closed.
    fn watch_failed(&self, host: &ModalHost, session: &mut Session, facts: &mut Facts, why: &str) {
        let st = host.status();
        let message = st.end.as_ref().map_or_else(
            || format!("the run's thread panicked ({why})"),
            |o| o.message.clone(),
        );
        self.shared.add_log_data(
            "abort",
            format!("abort: {message}"),
            Some(json!({"outcome": "run_panicked"})),
        );
        self.shared.add_log_data(
            "outcome",
            format!("ended (run_panicked): {message}"),
            Some(json!({"outcome": "run_panicked", "emergency_stop": true})),
        );
        let end = End {
            outcome: "run_panicked",
            message,
            emergency_stop: true,
            at_s: Some(st.generated as f64 / session.setup.fs_hz),
        };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.publish(session, host, Some(end.clone()));
            self.finish_record(facts, session);
        }));
        if r.is_err() {
            let mut v = (*self.view()).clone();
            v.state = "finished";
            v.end = Some(end);
            self.shared.publish(&self.id, v);
        }
    }

    /// The run's own thread.
    fn watch(
        &self,
        host: &ModalHost,
        session: &mut Session,
        rx: mpsc::Receiver<(Cmd, Reply)>,
        facts: &mut Facts,
    ) {
        let mut capture = lock(&host.capture).take();
        let width = host.inputs + host.outputs;
        let mut frame = vec![0f32; width.max(1)];
        let fs = session.setup.fs_hz;
        let shaker = session.setup.mode == Mode::Shaker;
        let mut want_measure = false;
        let mut seen_ramp: Option<u64> = None;
        let mut ending = false;
        let mut last_display = Instant::now() - Duration::from_secs(1);
        let st0 = host.status();
        let input_delay = if facts.hardware {
            u64::from(st0.group_delay_frames.unwrap_or(0))
        } else {
            st0.block_frames as u64
        };
        let lead_in = st0.lead_in_frames;
        {
            let mut d = lock(&self.display);
            d.rate_hz = fs;
            d.freqs = session.freqs().to_vec();
        }
        self.publish(session, host, None);
        loop {
            // The operator's calls.
            while let Ok((cmd, reply)) = rx.try_recv() {
                let r = self.handle(session, host, facts, cmd, &mut want_measure, &mut ending);
                let _ = reply.send(r);
            }
            // The captured frames.
            let mut events = Vec::new();
            if let Some(c) = capture.as_mut() {
                while c.pop(&mut frame) {
                    events.extend(session.feed(&frame[..host.inputs]));
                }
            }
            // Shaker mode: averaging begins once the ramp has begun on a frame's start.
            if shaker && want_measure {
                let sh = host.shared();
                let ph = sh.phase();
                if matches!(ph, DrivePhase::Ramping | DrivePhase::Driving) {
                    let from = sh.ramp_from.load(Ordering::Acquire);
                    if seen_ramp != Some(from) {
                        seen_ramp = Some(from);
                        want_measure = false;
                        let n = session.setup.block as u64;
                        let src_frame = from.saturating_sub(lead_in) / n;
                        match session.begin_measuring(
                            from + input_delay,
                            src_frame,
                            session.setup.ramp_frames(),
                        ) {
                            Ok(ev) => events.extend(ev),
                            Err(e) => self.shared.add_log("check", e.message),
                        }
                    }
                }
            }
            let changed = !events.is_empty();
            self.on_events(session, host, facts, events);
            let st = host.status();
            if let Some(o) = st.end.clone().filter(|_| host.finished()) {
                let ev = session.end();
                self.on_events(session, host, facts, ev);
                let by = self.shared.ended_by();
                let outcome = if o.outcome == "ended" {
                    "ended"
                } else {
                    o.outcome
                };
                let message = match by {
                    Some(b) => format!("{} (by {b})", o.message),
                    None => o.message.clone(),
                };
                if o.emergency_stop {
                    self.shared.add_log_data(
                        "abort",
                        format!("abort: {message}"),
                        Some(json!({"outcome": o.outcome})),
                    );
                }
                self.shared.add_log_data(
                    "outcome",
                    format!("ended ({outcome}): {message}"),
                    Some(json!({"outcome": outcome, "emergency_stop": o.emergency_stop})),
                );
                let end = End {
                    outcome,
                    message,
                    emergency_stop: o.emergency_stop,
                    at_s: Some(o.generated as f64 / fs),
                };
                self.publish(session, host, Some(end));
                self.finish_record(facts, session);
                self.publish(session, host, self.view().end.clone());
                break;
            }
            if changed || last_display.elapsed() > Duration::from_millis(100) {
                last_display = Instant::now();
                self.update_display(session);
                self.publish(session, host, None);
            }
            std::thread::sleep(POLL);
        }
        if let Some(c) = capture.take() {
            drop(c);
        }
    }

    fn handle(
        &self,
        s: &mut Session,
        host: &ModalHost,
        facts: &mut Facts,
        cmd: Cmd,
        want: &mut bool,
        ending: &mut bool,
    ) -> Result<Value, MethodError> {
        let as_err = |e: hertzforge_modal_test::session::Refused| refuse(e.reason, e.message);
        let shaker = s.setup.mode == Mode::Shaker;
        // Once an end or an abort is asked, nothing new starts: no drive, no arm, no hit
        // taken. The host refuses a drive start on its own too.
        if (*ending || host.closed())
            && matches!(
                cmd,
                Cmd::DriveStart | Cmd::Arm | Cmd::Accept | Cmd::SimHit(_)
            )
        {
            return Err(refuse(
                "ending",
                "The test is ending: nothing new starts after an end or an abort.",
            ));
        }
        let ev = match cmd.clone() {
            Cmd::Select(k) => s.select(k).map_err(as_err)?,
            Cmd::Step(d) => {
                let n = s.points.len() as i64;
                let k = (s.point as i64 + d).rem_euclid(n.max(1)) as usize;
                s.select(k).map_err(as_err)?
            }
            Cmd::Arm => s.arm().map_err(as_err)?,
            Cmd::Disarm => s.disarm(),
            Cmd::Accept => s.accept().map_err(as_err)?,
            Cmd::Reject => s.reject().map_err(as_err)?,
            Cmd::Undo => s.undo().map_err(as_err)?,
            Cmd::TargetFromHit => {
                let peak = s.target_from_latest().map_err(as_err)?;
                self.shared.add_log_data(
                    "operator",
                    format!("force target set from the latest hit: {}", num(peak)),
                    Some(json!({"action": "target_from_hit", "peak": num(peak)})),
                );
                Vec::new()
            }
            Cmd::DriveStart => {
                if !shaker {
                    return Err(refuse(
                        "not_in_mode",
                        "Impact mode drives nothing; arm for hits.",
                    ));
                }
                if s.points[s.point].measured {
                    return Err(refuse(
                        "point_done",
                        format!(
                            "Point {} is measured: undo to take it again, or select another.",
                            s.point
                        ),
                    ));
                }
                if !matches!(host.shared().phase(), DrivePhase::Idle) {
                    return Err(refuse(
                        "wrong_state",
                        format!("The drive is {}.", host.shared().phase().name()),
                    ));
                }
                if !host.start_drive() {
                    return Err(refuse(
                        "ending",
                        "The test is ending: nothing new starts after an end or an abort.",
                    ));
                }
                *want = true;
                self.shared.add_log_data(
                    "state",
                    "drive start: the ramp begins on the next frame",
                    Some(json!({"event": "drive_start", "point": s.point})),
                );
                Vec::new()
            }
            Cmd::DriveStop => {
                if !shaker {
                    return Err(refuse("not_in_mode", "Impact mode drives nothing."));
                }
                host.stop_drive();
                *want = false;
                self.shared.add_log_data(
                    "state",
                    "drive stop: the drive ramps down",
                    Some(json!({"event": "drive_stop", "point": s.point})),
                );
                s.stop_measuring()
            }
            Cmd::End => {
                *ending = true;
                *want = false;
                self.sim.armed.store(false, Ordering::Release);
                self.shared.add_log_data(
                    "operator",
                    "end of test asked",
                    Some(json!({"action": "end"})),
                );
                host.finish();
                let mut ev = s.disarm();
                ev.extend(s.stop_measuring());
                ev
            }
            Cmd::SimHit(k) => {
                if facts.hardware {
                    return Err(refuse(
                        "not_simulated",
                        "Simulated hits are for the simulator.",
                    ));
                }
                if shaker {
                    return Err(refuse("not_in_mode", "Shaker mode takes no hits."));
                }
                if !self.sim.queue(k) {
                    return Err(refuse("busy", "The simulated hit queue is full."));
                }
                Vec::new()
            }
        };
        self.on_events(s, host, facts, ev);
        self.update_display(s);
        self.publish(s, host, None);
        Ok(json!({"ok": true, "counts": s.counts_json()}))
    }

    fn on_events(&self, s: &mut Session, host: &ModalHost, facts: &mut Facts, events: Vec<Event>) {
        for e in events {
            match e {
                Event::Taken { point, at } => {
                    let t = &s.points[point].taken[at];
                    let m = &s.plan.measurements[point];
                    let words = match &t.report {
                        Some(r) => format!(
                            "hit {} at point {point}: peak {}, {}",
                            t.n,
                            num(r.peak),
                            match t.reason {
                                Some(x) => format!("rejected ({x})"),
                                None => t.status.to_owned(),
                            }
                        ),
                        None => format!("frame {} at point {point}: {}", t.n, t.status),
                    };
                    self.shared.add_log_data("hit", words, Some(json!({"point": point, "seq": t.seq, "status": t.status, "reason": t.reason})));
                    if let Some(rec) = facts.record.as_ref() {
                        let (name, d, arrays, line) = records::frame_line(s, point, at, m, "taken");
                        rec.put_result(&name, d, arrays, Some(line));
                    }
                }
                Event::Status { point, at } => {
                    let t = &s.points[point].taken[at];
                    self.shared.add_log_data(
                        "hit",
                        format!("{} {} at point {point} is now {}", if s.setup.mode == Mode::Impact { "hit" } else { "frame" }, t.n, t.status),
                        Some(json!({"point": point, "seq": t.seq, "status": t.status, "reason": t.reason, "event": "status"})),
                    );
                    if let Some(rec) = facts.record.as_ref() {
                        let m = &s.plan.measurements[point];
                        let (name, d, arrays, line) =
                            records::frame_line(s, point, at, m, "status");
                        rec.put_result(&name, d, arrays, Some(line));
                    }
                }
                Event::Done {
                    point,
                    measurement,
                    replaces,
                    estimates,
                } => {
                    self.shared.add_log_data(
                        "result",
                        format!("point {point} measured: {} averages (measurement {measurement})", estimates.averages),
                        Some(json!({"point": point, "measurement": measurement, "replaces": replaces})),
                    );
                    if let Some(rec) = facts.record.as_ref() {
                        for (name, d, arrays, line) in records::measurement_results(
                            s,
                            point,
                            measurement,
                            replaces,
                            &estimates,
                        ) {
                            rec.put_result(&name, d, arrays, Some(line));
                        }
                    }
                    if s.setup.mode == Mode::Shaker {
                        host.stop_drive();
                    }
                }
                Event::Reopened { point } => {
                    self.shared.add_log_data(
                        "result",
                        format!("point {point} is open again"),
                        Some(json!({"point": point, "event": "reopened"})),
                    );
                }
                Event::Selected { point } => {
                    self.sim.point.store(point, Ordering::Release);
                    self.shared.add_log_data(
                        "state",
                        format!("point {point} selected"),
                        Some(json!({"event": "selected", "point": point})),
                    );
                }
                Event::State(st) => {
                    self.sim.armed.store(st == SState::Armed, Ordering::Release);
                    self.shared.add_log_data(
                        "state",
                        format!("state {}", st.name()),
                        Some(json!({"event": "state", "state": st.name()})),
                    );
                }
            }
        }
    }

    fn update_display(&self, s: &Session) {
        let mut d = lock(&self.display);
        d.channels = s.recent(TIME_POINTS.min(s.setup.block));
        match s.estimate(s.point) {
            Some(e) => {
                d.mag = e
                    .frf
                    .iter()
                    .map(|r| {
                        r.iter()
                            .map(|h| h.iter().map(|c| c.norm()).collect())
                            .collect()
                    })
                    .collect();
                d.phase_deg = e
                    .frf
                    .iter()
                    .map(|r| {
                        r.iter()
                            .map(|h| h.iter().map(|c| c.arg().to_degrees()).collect())
                            .collect()
                    })
                    .collect();
                d.coherence = e
                    .coherence
                    .clone()
                    .or(e.multiple_coherence.clone())
                    .unwrap_or_default();
                d.averages = e.averages;
            }
            None => {
                d.mag.clear();
                d.phase_deg.clear();
                d.coherence.clear();
                d.averages = 0;
            }
        }
    }

    fn publish(&self, s: &Session, host: &ModalHost, end: Option<End>) {
        let mut v = (*self.view()).clone();
        let ph = host.shared().phase();
        v.drive_phase = ph.name();
        v.state = if end.is_some() || v.end.is_some() {
            "finished"
        } else if s.setup.mode == Mode::Shaker && ph == DrivePhase::Stopping {
            "stopping"
        } else {
            s.state.name()
        };
        if end.is_some() {
            v.end = end;
        }
        v.point = s.point;
        v.point_dofs = dofs_json(s, s.point);
        v.counts = s.counts_json();
        v.hits = Value::Array(s.taken_json(s.point));
        v.latest = s.latest().map_or(Value::Null, |(p, t)| {
            let mut j = t.to_json();
            if let (Some(o), Some(r)) = (j.as_object_mut(), &t.report) {
                o.insert("point".into(), json!(p));
                o.insert("report".into(), r.to_json());
            }
            j
        });
        if let Some(r) = v.record.as_mut() {
            r.problem = None;
        }
        self.shared.publish(&self.id, v);
    }

    fn finish_record(&self, facts: &mut Facts, s: &Session) {
        let Some(record) = facts.record.take() else {
            return;
        };
        let (name, d, arrays) = records::outcome_result(&self.view(), s);
        record.put_result(&name, d, arrays, None);
        self.shared.detach_record();
        let frames = self.shared.station_frame.load(Ordering::Relaxed);
        let frames = (frames != crate::random::run::NO_FRAME).then_some(frames);
        record.finish(records::end_map(
            &facts.run_uid,
            &self.view(),
            now_unix(),
            frames,
            s,
        ));
        let _ = facts.status.problem();
    }
}

struct Facts {
    status: Arc<RecordStatus>,
    record: Option<RunRecord>,
    run_uid: String,
    hardware: bool,
    _guard: Option<hertzforge_station::lock::LockGuard>,
}
