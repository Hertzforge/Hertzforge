//! The Playback run: the core's run host on the station the run arms on, the run's own
//! thread that composes the view from the host's status ten times a second, logs every
//! change of phase, takes the display frames off the loop's display ring and writes the
//! record. The record ([`crate::record`]) is written on its own writer thread: the run's
//! thread hands it entries and results with sends that never wait, and the streams are
//! taken where the station's blocks pass ([`crate::record::tap`]). Nothing here touches
//! a file on the loop thread.

use super::config::WirePlayback;
use super::records::{self, StartFacts};
use crate::protocol::{num, ErrorCode, MethodError};
use crate::pulse::rails::{now_unix, Shared};
use crate::random::record as rrec;
use crate::record::tap::{self, Recording};
use crate::record::{ProblemFn, RecordOptions, RecordStatus, RunRecord};
use crate::station::{self, NiScanner, StationPlan};
use hertzforge_channel_table::ChannelTable;
use hertzforge_playback::{CheckedDrive, HostError, HostOptions, Pacing, Phase, PlaybackHost};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How often the run's thread composes and publishes the view.
pub const PUBLISH_PERIOD: Duration = Duration::from_millis(50);
/// Display points kept per channel for the live time plots.
pub const TIME_POINTS: usize = 1024;
/// How long a start waits for the read-ahead and the station.
pub const START_WAIT: Duration = Duration::from_secs(120);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

pub fn refuse(reason: &'static str, message: impl Into<String>) -> MethodError {
    MethodError::named(ErrorCode::Refused, reason, message)
}

/// The run's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Configured,
    Starting,
    Running,
    Stopping,
    Finished,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Configured => "configured",
            State::Starting => "starting",
            State::Running => "running",
            State::Stopping => "stopping",
            State::Finished => "finished",
        }
    }

    pub fn is_live(self) -> bool {
        matches!(self, State::Starting | State::Running | State::Stopping)
    }
}

/// How the run ended.
#[derive(Debug, Clone, PartialEq)]
pub struct End {
    pub outcome: &'static str,
    pub message: String,
    /// Station seconds from the clock's start.
    pub at_s: Option<f64>,
    pub played: u64,
    pub emergency_stop: bool,
    pub trip: Option<Value>,
}

impl End {
    pub fn to_json(&self) -> Value {
        json!({"outcome": self.outcome, "message": self.message, "at_s": self.at_s.map(num),
               "played": self.played, "emergency_stop": self.emergency_stop, "trip": self.trip})
    }
}

/// Where the run records.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordRef {
    pub folder: String,
    pub streams: bool,
    pub problem: Option<String>,
}

/// One input or drive row as the state shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub index: u32,
    pub role: &'static str,
    pub unit: String,
    pub description: String,
}

/// What `playback.state` shows.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackView {
    pub run_id: String,
    pub state: State,
    pub phase: &'static str,
    pub title: String,
    pub source: &'static str,
    pub station_class: &'static str,
    pub fs_hz: f64,
    pub samples: u64,
    pub sha256: String,
    pub level_db: f64,
    pub ceiling_v: f64,
    pub pre_roll_frames: u64,
    pub post_roll_frames: u64,
    pub played: u64,
    pub generated: u64,
    pub buffered_frames: u64,
    pub inputs: Vec<Row>,
    pub drives: Vec<Row>,
    /// Each file channel's drive row.
    pub map: Vec<u32>,
    pub overloads: Vec<bool>,
    pub record: Option<RecordRef>,
    pub end: Option<End>,
}

fn row(c: &hertzforge_channel_table::Channel, role: &'static str) -> Row {
    Row {
        index: c.index,
        role,
        unit: crate::table::unit_name(&c.units).to_owned(),
        description: c.description.clone(),
    }
}

/// The view of a run just configured.
pub fn configured_view(
    run_id: &str,
    w: &WirePlayback,
    drive: &CheckedDrive,
    table: &ChannelTable,
    source: &'static str,
    station_class: &'static str,
) -> PlaybackView {
    let fs = drive.file.rate_hz;
    PlaybackView {
        run_id: run_id.to_owned(),
        state: State::Configured,
        phase: "idle",
        title: w.title.clone(),
        source,
        station_class,
        fs_hz: fs,
        samples: drive.file.samples,
        sha256: drive.file.sha256.clone(),
        level_db: w.level_db,
        ceiling_v: w.ceiling_v,
        pre_roll_frames: (w.pre_roll_s * fs).round() as u64,
        post_roll_frames: (w.post_roll_s * fs).round() as u64,
        played: 0,
        generated: 0,
        buffered_frames: 0,
        inputs: rrec::input_rows(table)
            .into_iter()
            .map(|c| row(c, "input"))
            .collect(),
        drives: rrec::drive_rows(table)
            .into_iter()
            .map(|c| row(c, "drive"))
            .collect(),
        map: w.map.clone(),
        overloads: vec![false; rrec::input_rows(table).len()],
        record: None,
        end: None,
    }
}

fn rows_json(rows: &[Row]) -> Vec<Value> {
    rows.iter()
        .map(|r| {
            json!({"index": r.index, "role": r.role, "unit": r.unit,
                        "description": r.description})
        })
        .collect()
}

/// `playback.state`: the run, or `unconfigured`.
pub fn state_json(v: Option<&PlaybackView>) -> Value {
    let Some(v) = v else {
        return json!({"run_id": null, "state": "unconfigured"});
    };
    let fs = v.fs_hz.max(1.0);
    json!({
        "run_id": v.run_id,
        "state": v.state.name(),
        "phase": v.phase,
        "title": v.title,
        "source": v.source,
        "class": v.station_class,
        "fs_hz": num(v.fs_hz),
        "drive": {"samples": v.samples, "duration_s": num(v.samples as f64 / fs),
                  "sha256": v.sha256, "map": v.map},
        "level_db": num(v.level_db),
        "ceiling_v": num(v.ceiling_v),
        "rolls": {"pre_frames": v.pre_roll_frames, "post_frames": v.post_roll_frames},
        "progress": progress_json(v),
        "inputs": rows_json(&v.inputs),
        "drives": rows_json(&v.drives),
        "overloads": v.inputs.iter().zip(&v.overloads)
            .map(|(r, o)| json!({"channel": r.index, "overload": o})).collect::<Vec<_>>(),
        "record": v.record.as_ref().map_or(Value::Null, |r| json!({"folder": r.folder,
            "streams": if r.streams { "on" } else { "off" }, "problem": r.problem})),
        "end": v.end.as_ref().map_or(Value::Null, End::to_json),
    })
}

fn progress_json(v: &PlaybackView) -> Value {
    let fs = v.fs_hz.max(1.0);
    json!({"played": v.played, "samples": v.samples,
           "played_s": num(crate::explorer::config::round(v.played as f64 / fs, 3)),
           "fraction": num(crate::explorer::config::round(
               if v.samples == 0 { 0.0 } else { v.played as f64 / v.samples as f64 }, 4)),
           "elapsed_s": num(crate::explorer::config::round(v.generated as f64 / fs, 3)),
           "buffered_s": num(crate::explorer::config::round(v.buffered_frames as f64 / fs, 3))})
}

/// What changes `playback.state` is sent for: everything but the progress.
pub fn state_key(v: Option<&PlaybackView>) -> String {
    match v {
        None => "none".into(),
        Some(v) => {
            let mut s = state_json(Some(v));
            if let Value::Object(m) = &mut s {
                m.remove("progress");
            }
            s.to_string()
        }
    }
}

/// The display points the loop handed on: per channel, inputs then drives.
#[derive(Default)]
pub struct Display {
    /// Display points per second.
    pub rate_hz: f64,
    /// The station time of the first point kept.
    pub first_s: f64,
    pub taken: u64,
    pub channels: Vec<VecDeque<f32>>,
}

/// `playback.live`: the state's progress and the time plots of every input and drive.
pub fn live_json(v: &PlaybackView, d: &Display, arrays: &mut crate::codec::Arrays) -> Value {
    let rows: Vec<&Row> = v.inputs.iter().chain(v.drives.iter()).collect();
    let channels: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(k, r)| {
            let values: Vec<f32> = d
                .channels
                .get(k)
                .map(|q| q.iter().copied().collect())
                .unwrap_or_default();
            let id = format!("c{k}");
            let reference = arrays.insert(
                id,
                crate::codec::Array::vector(crate::codec::ArrayData::F32(values)),
            );
            json!({"index": r.index, "role": r.role, "unit": r.unit, "values": reference})
        })
        .collect();
    json!({
        "run_id": v.run_id,
        "state": v.state.name(),
        "phase": v.phase,
        "progress": progress_json(v),
        "time": {"rate_hz": num(d.rate_hz), "t0_s": num(crate::explorer::config::round(d.first_s, 6)),
                 "channels": channels},
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
    /// Station seconds per wall second on the simulator.
    pub sim_speed: f64,
}

/// The service's handle on one configured run.
pub struct PlaybackRun {
    pub id: String,
    pub shared: Arc<Shared<PlaybackView>>,
    pub wire: Arc<WirePlayback>,
    pub drive: CheckedDrive,
    /// The check report (with the previews), the configuration and the derived values.
    pub check: Value,
    pub config: Value,
    pub derived: Value,
    pub host: Mutex<Option<Arc<PlaybackHost>>>,
    pub display: Mutex<Display>,
    started: AtomicBool,
    operator_abort: AtomicBool,
}

impl PlaybackRun {
    pub fn new(
        id: String,
        shared: Arc<Shared<PlaybackView>>,
        wire: WirePlayback,
        drive: CheckedDrive,
        check: Value,
        derived: Value,
    ) -> PlaybackRun {
        PlaybackRun {
            id,
            shared,
            config: wire.raw.clone(),
            wire: Arc::new(wire),
            drive,
            check,
            derived,
            host: Mutex::new(None),
            display: Mutex::new(Display::default()),
            started: AtomicBool::new(false),
            operator_abort: AtomicBool::new(false),
        }
    }

    pub fn view(&self) -> Arc<PlaybackView> {
        self.shared.view()
    }

    pub fn started(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    pub fn host(&self) -> Option<Arc<PlaybackHost>> {
        lock(&self.host).clone()
    }

    /// Show the run starting, under the service's lock.
    pub fn mark_starting(&self) {
        self.started.store(true, Ordering::SeqCst);
        let mut v = (*self.view()).clone();
        v.state = State::Starting;
        v.phase = "reading_ahead";
        self.shared.publish(&self.id, v);
    }

    /// The controlled stop, named for `by`. Before the pump runs, the start is ended.
    pub fn stop(&self, by: &str) {
        self.shared.set_ended_by(by);
        match self.host() {
            Some(h) => {
                self.shared.add_log_data(
                    "operator",
                    format!("stop asked by {by}"),
                    Some(json!({"action": "stop", "by": by})),
                );
                h.stop();
            }
            None => self.operator_abort.store(true, Ordering::SeqCst),
        }
    }

    /// The abort, named for `by`: the drive to zero at once.
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

    /// Re-hash the file (this is the worker thread), read ahead, open the station, open
    /// the record, start the pump and the run's thread. Answers once the pre-roll runs.
    pub fn start(self: &Arc<Self>, ctx: StartContext) -> Result<(), MethodError> {
        match self.start_inner(ctx) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.end_before_start(&e);
                Err(e)
            }
        }
    }

    fn start_inner(self: &Arc<Self>, ctx: StartContext) -> Result<(), MethodError> {
        // The file as it is now, against the hash the check was taken on.
        let path = self.drive.file.bin_path();
        let now = hertzforge_playback::file::sha256_file(&path).map_err(|e| {
            refuse(
                "drive_unreadable",
                format!(
                    "The drive file could not be read again at start: {} ({e}).",
                    path.display()
                ),
            )
        })?;
        if now != self.drive.check.sha256 {
            self.shared.add_log(
                "check",
                "start refused: the drive file changed since it was checked",
            );
            return Err(refuse(
                "drive_changed",
                format!(
                    "drive.bin has changed since it was checked: its SHA-256 is now {now}, the \
                     check was taken on {}. Check the file again.",
                    self.drive.check.sha256
                ),
            )
            .with_detail(json!({"now": now, "checked": self.drive.check.sha256})));
        }
        self.shared.add_log_data(
            "check",
            "the drive file is unchanged since its check",
            Some(json!({"sha256": now})),
        );
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
        let fs = self.drive.file.rate_hz;
        let streams_off = self.streams_off(&ctx);
        let buffers = if streams_off.is_none() {
            rrec::tap_buffers(fs, ctx.record.stream_buffer_s)
        } else {
            1
        };
        let (inputs_tap, inputs_link) = tap::pair(
            rrec::input_rows(&ctx.table).len(),
            rrec::TAP_BLOCK_FRAMES,
            buffers,
        );
        let (drives_tap, drives_link) = tap::pair(
            rrec::drive_rows(&ctx.table).len(),
            rrec::TAP_BLOCK_FRAMES,
            buffers,
        );
        let class = built.station;
        let provenance = built.provenance.clone();
        // The drive record ends where the step says (the post-roll's end, or a block of
        // zeros past a stop ramp); the zeros played after it, while the inputs come back,
        // are not recorded.
        let record_until = Arc::new(AtomicU64::new(u64::MAX));
        let backend = Recording::wrap_until(
            built.backend,
            inputs_tap,
            drives_tap,
            self.shared.station_frame.clone(),
            Some(record_until.clone()),
        );
        // The record, before the station is opened: refused here, nothing has driven.
        let started_unix = now_unix();
        let run_uid = rrec::run_uid(started_unix, &self.id);
        let begin = records::begin(
            &StartFacts {
                run: self,
                run_uid: &run_uid,
                table: &ctx.table,
                revision: ctx.revision,
                plan: &ctx.station,
                class,
                provenance: &provenance,
                devices: &ctx.devices,
                caps: &caps,
                started_unix,
                utc_offset_s: rrec::utc_offset_s(),
                streams_off: streams_off.clone(),
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
                    "The run's record could not be written, so the run did not start: {} ({}). \
                     Choose a data folder that can be written.",
                    e.path.display(),
                    e.reason
                ),
            )
            .with_detail(json!({"path": e.path.display().to_string(), "reason": e.reason}))
        })?;
        self.shared.attach_record(record.log());
        // The check that let the run be configured, and its pass again at start, now
        // that the record holds the entries' facts.
        self.shared.add_log_data(
            "check",
            "the drive file's check passed, and the file is unchanged since",
            Some(
                json!({"event": "check_passed", "sha256": self.drive.check.sha256,
                        "pass": true}),
            ),
        );
        let folder = record.folder().display().to_string();
        if let Some(why) = &streams_off {
            self.shared.add_log_data(
                "record",
                format!("the run's streams are not recorded: {why}"),
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
        if self.operator_abort.load(Ordering::SeqCst) {
            let e = refuse(
                "arm_refused",
                "The start was stopped before the station opened.",
            );
            record.finish(records::end_map(
                &run_uid,
                &self.view(),
                now_unix(),
                None,
                &self.drive,
            ));
            return Err(e);
        }
        let pacing = if hardware {
            Pacing::Station
        } else {
            Pacing::WallClock {
                speed: ctx.sim_speed.max(1e-3),
            }
        };
        let opts = HostOptions {
            pacing,
            record_until: Some(record_until),
            ..HostOptions::default()
        };
        let host = match PlaybackHost::start(
            self.drive.clone(),
            ctx.table.clone(),
            backend,
            class,
            opts,
        ) {
            Ok(h) => Arc::new(h),
            Err(e) => {
                let err = match e {
                    HostError::Rate(r) => refuse("rate_mismatch", r.message).with_detail(r.detail),
                    HostError::Source(s) => refuse(
                        "drive_unreadable",
                        format!("The drive file could not be read ahead: {s}."),
                    ),
                    HostError::Arm(s) => {
                        refuse("arm_refused", format!("The station refused to arm: {s}"))
                    }
                };
                self.shared
                    .add_log("check", format!("start refused: {}", err.message));
                self.end_before_start(&err);
                self.shared.detach_record();
                record.finish(records::end_map(
                    &run_uid,
                    &self.view(),
                    now_unix(),
                    None,
                    &self.drive,
                ));
                return Err(err);
            }
        };
        *lock(&self.host) = Some(host.clone());
        if self.operator_abort.load(Ordering::SeqCst) {
            host.abort();
        }
        {
            let mut d = lock(&self.display);
            d.rate_hz = fs / host.display_every as f64;
            d.channels =
                vec![VecDeque::new(); rrec::input_rows(&ctx.table).len() + self.drive.rows.len()];
        }
        self.shared.add_log_data(
            "state",
            format!(
                "armed on {}: {} Hz, {} drive{}, {} dB, ceiling {} V",
                provenance.station_label,
                num(fs),
                self.drive.rows.len(),
                if self.drive.rows.len() == 1 { "" } else { "s" },
                num(self.wire.level_db),
                num(self.wire.ceiling_v)
            ),
            Some(json!({"event": "armed", "fs_hz": num(fs), "level_db": num(self.wire.level_db)})),
        );
        let run = self.clone();
        let facts = Facts {
            status: record.status(),
            record: Some(record),
            run_uid,
            _guard: guard,
        };
        std::thread::Builder::new()
            .name(format!("playback {}", self.id))
            .spawn(move || run.watch(host, facts))
            .map_err(|e| MethodError::internal(format!("could not start the run's thread: {e}")))?;
        let asked = Instant::now();
        loop {
            let v = self.view();
            if v.state != State::Starting {
                return Ok(());
            }
            if asked.elapsed() > START_WAIT {
                return Err(MethodError::internal("the run did not start in time"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
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

    fn end_before_start(&self, e: &MethodError) {
        let mut v = (*self.view()).clone();
        if v.state == State::Finished {
            return;
        }
        v.state = State::Finished;
        v.phase = "refused";
        v.end = Some(End {
            outcome: "arm_refused",
            message: e.message.clone(),
            at_s: None,
            played: 0,
            emergency_stop: false,
            trip: None,
        });
        self.shared.add_log_data(
            "state",
            format!("start refused: {}", e.message),
            Some(json!({"event": "refused", "reason": e.reason})),
        );
        self.shared.publish(&self.id, v);
    }

    /// The run's own thread.
    fn watch(self: Arc<Self>, host: Arc<PlaybackHost>, mut facts: Facts) {
        let mut last_phase: Option<Phase> = None;
        let mut display_rx = lock(&host.display).take();
        let width = {
            let d = lock(&self.display);
            d.channels.len()
        };
        let mut frame = vec![0f32; width.max(1)];
        loop {
            std::thread::sleep(PUBLISH_PERIOD);
            let st = host.status();
            // The display frames the loop handed on.
            if let Some(rx) = display_rx.as_mut() {
                let mut d = lock(&self.display);
                while width > 0 && rx.pop(&mut frame) {
                    d.taken += 1;
                    for (k, q) in d.channels.iter_mut().enumerate() {
                        q.push_back(frame[k]);
                        if q.len() > TIME_POINTS {
                            q.pop_front();
                        }
                    }
                }
                let kept = d.channels.first().map_or(0, VecDeque::len) as u64;
                d.first_s = (d.taken.saturating_sub(kept)) as f64 / d.rate_hz.max(1.0);
            }
            if last_phase != Some(st.phase) {
                // The play's own phases come in order; the ones that passed between two
                // looks are logged too, each at the frame it began.
                let order = [Phase::PreRoll, Phase::Playing, Phase::PostRoll, Phase::Done];
                let at = |p: Phase| order.iter().position(|q| *q == p);
                let from = last_phase.and_then(at).map_or(0, |k| k + 1);
                let steps: Vec<Phase> = match at(st.phase) {
                    Some(to) if to >= from => order[from..=to].to_vec(),
                    _ => vec![st.phase],
                };
                for p in steps {
                    self.log_phase(p, &st);
                }
                last_phase = Some(st.phase);
            }
            let prev = self.view();
            let mut v = (*prev).clone();
            v.played = st.played;
            v.generated = st.generated;
            v.buffered_frames = st.buffered;
            if !st.overloads.is_empty() {
                for (k, (now, was)) in st.overloads.iter().zip(prev.overloads.iter()).enumerate() {
                    if *now && !*was {
                        let ch = v.inputs.get(k).map(|r| r.index);
                        self.shared.add_log_data(
                            "overload",
                            format!("input row {} overloaded", ch.unwrap_or(0)),
                            Some(json!({"channel": ch})),
                        );
                    }
                }
                v.overloads = st.overloads.clone();
            }
            v.phase = st.phase.name();
            v.state = match st.phase {
                Phase::Stopping => State::Stopping,
                _ => State::Running,
            };
            if let Some(r) = v.record.as_mut() {
                r.problem = facts.status.problem();
            }
            let ended = st.end.clone().filter(|_| host.finished());
            if let Some(o) = &ended {
                v.state = State::Finished;
                let by = self.shared.ended_by();
                let message = match (&by, o.outcome) {
                    (Some(b), "aborted") => format!("{} (by {b})", o.message),
                    (Some(b), "stopped") => format!("{} (by {b})", o.message),
                    _ => o.message.clone(),
                };
                let trip = o.trip.as_ref().map(|t| {
                    json!({"channel": t.channel_index, "axis": format!("{:?}", t.axis).to_lowercase(),
                           "value": num(t.value), "limit": num(t.limit)})
                });
                if o.outcome == "response_abort"
                    || o.outcome == "drive_ceiling"
                    || o.outcome == "input_overload"
                {
                    self.shared.add_log_data(
                        "trip",
                        message.clone(),
                        Some(json!({"outcome": o.outcome, "trip": trip})),
                    );
                }
                if o.outcome == "drive_underrun" {
                    self.shared.add_log_data(
                        "underrun",
                        message.clone(),
                        Some(json!({"played": o.played})),
                    );
                }
                if o.emergency_stop {
                    self.shared.add_log_data(
                        "abort",
                        format!("abort: {message}"),
                        Some(json!({"outcome": o.outcome})),
                    );
                }
                v.end = Some(End {
                    outcome: o.outcome,
                    message: message.clone(),
                    at_s: Some(o.generated as f64 / st.fs_hz.max(1.0)),
                    played: o.played,
                    emergency_stop: o.emergency_stop,
                    trip,
                });
                self.shared.add_log_data(
                    "outcome",
                    format!("ended {}: {message}", o.outcome),
                    Some(
                        json!({"outcome": o.outcome, "played": o.played, "samples": st.samples,
                                "emergency_stop": o.emergency_stop}),
                    ),
                );
            }
            self.shared.publish(&self.id, v.clone());
            if ended.is_some() {
                self.finish_record(&mut facts, &v);
                self.shared.publish(&self.id, v);
                break;
            }
        }
    }

    /// A phase of the play, as a `state` entry with the generated frame it began at.
    fn log_phase(&self, p: Phase, st: &hertzforge_playback::HostStatus) {
        let fs = st.fs_hz.max(1.0);
        let (n, post) = (st.samples, st.post_roll_frames);
        // frames as the station plays them: the file starts at the pre-roll's end, or at
        // the lead-in's when the pre-roll is shorter
        let pre = st.pre_roll_frames.max(st.lead_in_frames);
        let (kind, words, frame): (&'static str, String, Option<u64>) = match p {
            Phase::PreRoll => (
                "pre_roll",
                format!("pre-roll: {} s at zero drive", num(pre as f64 / fs)),
                Some(0),
            ),
            Phase::Playing => (
                "playing",
                format!("playing the drive file at {} dB", num(self.wire.level_db)),
                Some(pre),
            ),
            Phase::PostRoll => (
                "post_roll",
                format!("post-roll: {} s at zero drive", num(post as f64 / fs)),
                Some(pre + n),
            ),
            Phase::Done => (
                "post_roll_done",
                "the post-roll has been played".into(),
                Some(pre + n + post),
            ),
            Phase::Stopping => ("stop", "stop: the drive ramps down".into(), st.stop_from),
            Phase::Stopped => ("stopped", "the stop ramp has reached zero".into(), None),
            Phase::Zeroed => ("zeroed", "the drive went to zero at once".into(), None),
        };
        self.shared.add_log_data(
            "state",
            words,
            Some(json!({"event": kind, "generated_frame": frame})),
        );
    }

    fn finish_record(&self, facts: &mut Facts, v: &PlaybackView) {
        let Some(record) = facts.record.take() else {
            return;
        };
        let (name, d, arrays) = records::check_result(&self.drive);
        record.put_result(&name, d, arrays, None);
        let (name, d, arrays) = records::outcome_result(v);
        record.put_result(&name, d, arrays, None);
        self.shared.detach_record();
        let frames = self.shared.station_frame.load(Ordering::Relaxed);
        let frames = (frames != crate::random::run::NO_FRAME).then_some(frames);
        record.finish(records::end_map(
            &facts.run_uid,
            v,
            now_unix(),
            frames,
            &self.drive,
        ));
    }
}

struct Facts {
    status: Arc<RecordStatus>,
    record: Option<RunRecord>,
    run_uid: String,
    _guard: Option<hertzforge_station::lock::LockGuard>,
}
