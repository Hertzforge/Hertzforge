//! The Recorder run: the core's `RecorderService` on the station the run starts on, a
//! pump thread that reads the station one batch at a time, and the run's own thread.
//!
//! The pump thread is the station's reading thread. It holds the service for one batch
//! and one analysis pass, hands the batch to the display ring, and does nothing else:
//! no file, no free-space query, no join. The run's thread composes the view ten times
//! a second, reads the free space once a second, logs, and hands the run's record its
//! log, its streams and its end. The shared record's writer thread writes the run
//! folder's own files (`run.json`, `table.json`, `setup.json`, `log.jsonl`, `end.json`)
//! and the core's record thread writes the recordings under `streams/`. Verbs take the
//! service lock, so the server runs them on the blocking pool with a bounded wait.

use super::config::{frames_for, round, Derived, Mode, RecorderStation, WireRecorder};
use super::records::{self, StartFacts};
use super::sim_signal::{Signal, Tap, Warnings};
use super::subscribe::{FrfData, RowSpectrum};
use super::view::{Disk, End, Health, LiveRow, RecordRef, RecorderView, Recording, State, Trigger};
use crate::protocol::{num, ErrorCode, MethodError};
use crate::pulse::rails::Shared;
use crate::random::record as run_ids;
use crate::record::{ProblemFn, RecordOptions, RecordStatus, RunRecord};
use crate::station::{self, NiScanner, StationPlan};
use hertzforge_channel_table::ChannelTable;
use hertzforge_recorder::service::{EndCause, Mark, RecState, RecStatus};
use hertzforge_recorder::RecorderService;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How often the run's thread composes and publishes the view.
pub const PUBLISH_PERIOD: Duration = Duration::from_millis(100);
/// How often the run's thread reads the data volume's free space.
pub const FREE_SPACE_PERIOD: Duration = Duration::from_secs(1);
/// Seconds of every recorded row the display ring keeps.
pub const DISPLAY_S: f64 = 2.0;
/// The longest the run's thread waits, at the end, for the record to write its end.
const RECORD_FINISH_WAIT: Duration = Duration::from_secs(10);
/// The longest a wall-clock-paced pump sleeps in one go, so a stop is never left
/// waiting behind a long pause.
const MAX_PACE: Duration = Duration::from_millis(50);

/// The overload indicator's definition, for `run.json`.
pub const OVERLOAD_DEFINITION: &str = "A row's overload indicator latches the first time a \
     sample's magnitude reaches 0.9 of the row's range_max, and holds for the run.";

/// How long a start waits for this service's own previous run to let the NI hardware go.
const OWN_RUN_RELEASE: Duration = Duration::from_secs(3);

/// Take the station's hardware lock; when this same service holds it, a run that has just
/// finished is still letting it go (another module's run publishes its end before its
/// thread releases the lock), so wait for it, up to [`OWN_RUN_RELEASE`], rather than refuse
/// the start as if another program held the hardware (the NI bench: a Recorder start sent
/// on an Explorer run's end).
fn acquire_after_own_run(
    data_root: &std::path::Path,
    bind: &str,
    station: &str,
) -> Result<hertzforge_station::lock::LockGuard, hertzforge_station::lock::LockError> {
    let t0 = Instant::now();
    loop {
        match hertzforge_station::lock::acquire(data_root, bind, station) {
            Err(hertzforge_station::lock::LockError::Held(h))
                if h.pid == std::process::id() && t0.elapsed() < OWN_RUN_RELEASE =>
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

/// The most the pump waits, after a batch, for the threads waiting on the service to take it:
/// a verb or a status read holds it for well under a millisecond.
const HANDOFF_MAX: Duration = Duration::from_millis(20);

/// On a hardware station the pump holds the service across each batch's read, the read being
/// the pacing, and takes it again at once; a lock that is not fair can then keep a verb or the
/// status thread out indefinitely (on the NI bench verbs went unanswered for 5 s and an abort
/// took 10 s to end acquisition). A thread that wants the service says so here while it waits,
/// and the pump gives way after each batch until it has had its turn.
#[derive(Default)]
struct Handoff {
    waiting: AtomicUsize,
}

impl Handoff {
    /// Take `m`, counted as waiting until it is held.
    fn take<'a, T>(&self, m: &'a Mutex<T>) -> MutexGuard<'a, T> {
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let g = lock(m);
        self.waiting.fetch_sub(1, Ordering::SeqCst);
        g
    }

    /// Wait, up to `max`, while anyone is waiting to take the service.
    fn give_way(&self, max: Duration) {
        let t0 = Instant::now();
        while self.waiting.load(Ordering::SeqCst) > 0 && t0.elapsed() < max {
            std::thread::sleep(Duration::from_micros(200));
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn refuse(reason: &'static str, message: impl Into<String>) -> MethodError {
    MethodError::named(ErrorCode::Refused, reason, message)
}

/// The refusal of a verb in a phase it does not apply in.
pub fn wrong_state(v: &RecorderView, verb: &str) -> MethodError {
    refuse(
        "wrong_state",
        format!(
            "{verb} does not apply while the run is {} ({}).",
            v.state.name(),
            v.phase.replace('_', " ")
        ),
    )
    .with_detail(json!({"state": v.state.name(), "phase": v.phase}))
}

/// The refusal of a verb in a trigger mode it does not apply in.
pub fn wrong_mode(v: &RecorderView, verb: &str) -> MethodError {
    refuse(
        "wrong_mode",
        format!("{verb} does not apply in {} mode.", v.mode.name()),
    )
    .with_detail(json!({"mode": v.mode.name()}))
}

/// What a start needs from the server.
pub struct StartContext {
    pub table: ChannelTable,
    pub station: StationPlan,
    pub scanner: Arc<dyn NiScanner>,
    /// Where the hardware lock lives.
    pub data_root: PathBuf,
    /// Where the run's folder is made.
    pub records_root: PathBuf,
    pub bind: String,
    /// What the simulator's inputs carry: the stated source, or a test's.
    pub signal: Option<Signal>,
    /// The signal in words, for the record of a simulated run.
    pub signal_words: Option<Value>,
    /// The table's revision, for the record.
    pub revision: u64,
    /// The devices of the last scan, for the record's models and serial numbers.
    pub devices: Vec<hertzforge_hal::DiscoveredDevice>,
    /// How the run's record is written.
    pub record: RecordOptions,
}

/// What the pump and the run's thread hand on for the display.
#[derive(Default)]
struct Display {
    /// The last [`DISPLAY_S`] of every recorded row.
    time: Vec<VecDeque<f32>>,
    /// The station frame after the last kept.
    time_end: u64,
    /// The largest magnitude of each row since start.
    peaks: Vec<f32>,
    spectra: Vec<RowSpectrum>,
    frf: Option<FrfData>,
}

/// The run's display data, shared with the server.
#[derive(Default)]
pub struct RunData {
    display: Mutex<Display>,
}

impl RunData {
    /// The kept samples: the station frame of the first, and one column a row.
    pub fn time(&self) -> (u64, Vec<Vec<f32>>) {
        let d = lock(&self.display);
        let n = d.time.first().map_or(0, |c| c.len()) as u64;
        (
            d.time_end.saturating_sub(n),
            d.time.iter().map(|c| c.iter().copied().collect()).collect(),
        )
    }

    pub fn with_spectra<T>(&self, f: impl FnOnce(&[RowSpectrum], Option<&FrfData>) -> T) -> T {
        let d = lock(&self.display);
        f(&d.spectra, d.frf.as_ref())
    }
}

/// A stop asked for: the cause and whether it is an abort.
const NONE: u8 = 0;
const STOP: u8 = 1;
const ABORT: u8 = 2;

/// The service's handle on one configured run.
pub struct RecorderRun {
    pub id: String,
    pub shared: Arc<Shared<RecorderView>>,
    pub data: Arc<RunData>,
    pub wire: Arc<WireRecorder>,
    pub config: Value,
    pub derived: Value,
    pub derived_figures: Derived,
    pub station_kind: RecorderStation,
    /// The core's service, once the station is up.
    svc: Mutex<Option<Arc<Mutex<RecorderService>>>>,
    started: AtomicBool,
    stop: AtomicU8,
    stop_cause: Mutex<Option<&'static str>>,
    /// The run folder, once made.
    folder: Mutex<Option<PathBuf>>,
    /// Recordings saved since the last arm.
    saved_at_arm: Mutex<u64>,
    /// The pump's way of letting a waiting verb or the status thread at the service.
    handoff: Handoff,
    /// The run's marks, kept when the service is released at the end.
    final_marks: Mutex<Vec<Mark>>,
    /// The run's record's status (its folder and its latest problem), once open.
    record_status: Mutex<Option<Arc<RecordStatus>>>,
    /// The run's identity, unique across restarts, once started.
    run_uid: Mutex<String>,
}

impl RecorderRun {
    pub fn new(
        id: String,
        shared: Arc<Shared<RecorderView>>,
        wire: Arc<WireRecorder>,
        derived_figures: Derived,
        station_kind: RecorderStation,
    ) -> RecorderRun {
        RecorderRun {
            id,
            shared,
            data: Arc::new(RunData::default()),
            config: wire.raw.clone(),
            derived: derived_figures.to_json(),
            wire,
            derived_figures,
            station_kind,
            svc: Mutex::new(None),
            started: AtomicBool::new(false),
            stop: AtomicU8::new(NONE),
            stop_cause: Mutex::new(None),
            folder: Mutex::new(None),
            saved_at_arm: Mutex::new(0),
            handoff: Handoff::default(),
            final_marks: Mutex::new(Vec::new()),
            record_status: Mutex::new(None),
            run_uid: Mutex::new(String::new()),
        }
    }

    pub fn view(&self) -> Arc<RecorderView> {
        self.shared.view()
    }

    pub fn started(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    /// The run folder, once the run has started.
    pub fn folder(&self) -> Option<PathBuf> {
        lock(&self.folder).clone()
    }

    fn service(&self) -> Option<Arc<Mutex<RecorderService>>> {
        lock(&self.svc).clone()
    }

    /// Drop the run's own handle on the core's service, so the station closes once the last
    /// handle goes.
    fn release_service(&self) {
        if let Some(svc) = self.service() {
            *lock(&self.final_marks) = self.handoff.take(&svc).marks().to_vec();
        }
        *lock(&self.svc) = None;
    }

    fn publish(&self, v: RecorderView) {
        self.shared.publish(&self.id, v);
    }

    /// The run is starting: the station is being opened.
    pub fn mark_starting(&self) {
        self.started.store(true, Ordering::SeqCst);
        let mut v = (*self.view()).clone();
        v.state = State::Starting;
        v.phase = "opening";
        self.publish(v);
    }

    /// A start refused once the station was being opened ends the run `arm_refused`.
    fn end_before_start(&self, e: &MethodError) {
        let mut v = (*self.view()).clone();
        v.state = State::Finished;
        v.end = Some(End {
            outcome: "arm_refused",
            message: e.message.clone(),
            at_s: None,
            recordings: 0,
        });
        v.phase = "faulted";
        self.shared
            .add_log("state", format!("ended arm_refused: {}", e.message));
        self.publish(v);
    }

    /// Open the station with no outputs, start acquisition and the run's threads.
    /// Answers once acquisition runs.
    pub fn start(self: &Arc<Self>, ctx: StartContext) -> Result<(), MethodError> {
        let w = self.wire.clone();
        let fs = w.fs_hz;
        let n_rows = self.derived_figures.rows.len();
        let warnings: Warnings = Arc::new(Mutex::new(Vec::new()));
        let built = (|| {
            let station = station::build(&ctx.station, &*ctx.scanner)?;
            let hardware = station.station.is_hardware();
            let guard = if hardware {
                Some(
                    acquire_after_own_run(
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
                        hertzforge_station::lock::LockError::Io(m) => {
                            refuse("arm_refused", format!("The station refused to start: {m}"))
                        }
                    })?,
                )
            } else {
                None
            };
            let clock = match station.backend.caps().clock {
                hertzforge_hal::ClockDomain::Shared => "shared",
                hertzforge_hal::ClockDomain::Independent => "independent",
                hertzforge_hal::ClockDomain::Unknown => "unknown",
            };
            let class = station.station;
            let provenance = station.provenance.clone();
            let caps = station.backend.caps();
            let signal = if hardware { None } else { ctx.signal.clone() };
            let backend: Box<dyn hertzforge_hal::Backend> =
                Box::new(Tap::new(station.backend, signal, n_rows, warnings.clone()));
            let mut svc = RecorderService::with_backend(
                w.build(&ctx.table),
                ctx.table.clone(),
                backend,
                station.station,
                // The recordings' place is the run folder's `streams/`, named once the
                // folder is made below.
                ctx.records_root.join(records::STREAMS),
            )
            .map_err(|e| refuse("arm_refused", format!("The station refused to start: {e}")))?
            .with_block_frames(hertzforge_hal::default_block_frames(fs))
            .with_group_delay_frames(self.station_kind.group_delay_frames(fs));
            svc.configure()
                .map_err(|e| refuse("arm_refused", format!("The station refused to start: {e}")))?;
            svc.start_monitor()
                .map_err(|e| refuse("arm_refused", format!("The station refused to start: {e}")))?;
            Ok::<_, MethodError>((svc, guard, clock, hardware, class, provenance, caps))
        })();
        let (mut svc, guard, clock, hardware, class, provenance, caps) = match built {
            Ok(b) => b,
            Err(e) => {
                self.end_before_start(&e);
                return Err(e);
            }
        };
        let programmed = svc.actual_rate_hz();
        if (programmed - fs).abs() > 1e-9 {
            self.shared.add_log(
                "station",
                format!(
                    "sample rate coerced: requested {} Hz, device programmed {} Hz",
                    num(fs),
                    num(programmed)
                ),
            );
        }
        self.shared.add_log(
            "station",
            format!(
                "the station programmed {} Hz (asked {} Hz)",
                num(round(programmed, 3)),
                num(fs)
            ),
        );
        let drives: Vec<String> = ctx
            .table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == hertzforge_channel_table::ChannelRole::Drive)
            .map(|c| c.index.to_string())
            .collect();
        self.shared.add_log(
            "note",
            if drives.is_empty() {
                "outputs: none".to_owned()
            } else {
                format!(
                    "outputs: none (drive rows {} not opened)",
                    drives.join(", ")
                )
            },
        );

        // The run's record, before the first batch is read: `run.json`, `table.json`,
        // `setup.json` and the log so far, on the record's own thread. Refused here,
        // the start is refused and the station released.
        let derived_now = {
            let mut d = self.wire.derive(
                &ctx.table,
                self.station_kind,
                programmed,
                self.derived_figures.free_bytes,
            );
            d.actual = true;
            d.block_frames = hertzforge_hal::default_block_frames(programmed);
            d.batch_frames = super::config::BLOCKS_PER_BATCH * d.block_frames;
            d
        };
        let started_unix = now_unix();
        let run_uid = run_ids::run_uid(started_unix, &self.id);
        *lock(&self.run_uid) = run_uid.clone();
        let rows = self.view().live_rows.clone();
        let (log_so_far, log_written) = self.shared.log_since(0);
        let facts = StartFacts {
            run_id: &self.id,
            run_uid: &run_uid,
            wire: &self.wire,
            table: &ctx.table,
            revision: ctx.revision,
            plan: &ctx.station,
            class,
            provenance: &provenance,
            devices: &ctx.devices,
            caps: &caps,
            derived: &derived_now,
            rows: &rows,
            sim_source: if hardware {
                None
            } else {
                ctx.signal_words.clone()
            },
            started_unix,
            utc_offset_s: run_ids::utc_offset_s(),
            overload: json!({"fraction": num(super::config::OVERLOAD_FRACTION),
                             "definition": OVERLOAD_DEFINITION}),
            log: log_so_far
                .iter()
                .map(|e| log_entry(e, None, None))
                .collect(),
        };
        let begin = records::begin(&facts, ctx.records_root.clone());
        let shared = self.shared.clone();
        let problem: ProblemFn = Box::new(move |words: String, _data: Value| {
            // In the log and, through the record's status, in the state's `record`.
            shared.add_log("record", words);
        });
        let record = match RunRecord::open(begin, &ctx.record, problem) {
            Ok(r) => r,
            Err(e) => {
                let _ = svc.stop_monitor();
                let err = refuse(
                    "arm_refused",
                    format!(
                        "The run's record could not be written, so the run did not start: {} \
                         ({}). Choose a data folder that can be written.",
                        e.path.display(),
                        e.reason
                    ),
                )
                .with_detail(json!({"path": e.path.display().to_string(), "reason": e.reason}));
                self.end_before_start(&err);
                return Err(err);
            }
        };
        let folder = record.folder().to_path_buf();
        let streams = folder.join(records::STREAMS);
        // Made here, on the start's thread, as the run folder is: the record thread
        // claims each recording's folder inside it.
        let made = std::fs::create_dir(&streams);
        svc.set_data_root(streams.clone());
        let begun = made
            .map_err(|e| format!("{} could not be made: {e}", streams.display()))
            .and_then(|()| {
                if w.trigger.mode == Mode::Start {
                    svc.record_from_start()
                        .map_err(|e| format!("The recording could not begin: {e}"))
                } else {
                    Ok(())
                }
            });
        if let Err(words) = begun {
            let _ = svc.stop_monitor();
            let err = refuse("arm_refused", words);
            self.end_before_start(&err);
            let mut end = serde_json::Map::new();
            end.insert("run_uid".into(), Value::from(run_uid.as_str()));
            end.insert("run_id".into(), Value::from(self.id.as_str()));
            end.insert("end".into(), view_end_json(&self.view()));
            end.insert("at_unix".into(), Value::from(now_unix()));
            end.insert("station_frames".into(), Value::from(0));
            for e in self.shared.log_since(log_written).0.iter() {
                record.log().append(log_entry(e, None, None));
            }
            record.finish(end);
            return Err(err);
        }
        *lock(&self.folder) = Some(folder.clone());
        *lock(&self.record_status) = Some(record.status());
        self.shared
            .add_log("note", format!("records in {}", folder.display()));

        let mut v = (*self.view()).clone();
        v.state = State::Running;
        v.phase = if w.trigger.mode == Mode::Start {
            "recording"
        } else {
            "monitoring"
        };
        v.grid.fs_hz = programmed;
        v.grid.block_frames = derived_now.block_frames;
        v.grid.batch_frames = derived_now.batch_frames;
        v.grid.actual = true;
        v.clock = Some(clock);
        v.t_s = Some(0.0);
        v.station_frames = Some(0);
        v.record = Some(RecordRef {
            folder: folder.display().to_string(),
            problem: None,
        });
        self.shared
            .add_log("state", format!("running, {}", v.phase));
        // The entries made before the first batch, to the record now: the station has
        // delivered no frame yet.
        let (entries, log_written) = self.shared.log_since(log_written);
        let to_record = record.log();
        for e in &entries {
            to_record.append(log_entry(e, None, None));
        }
        self.publish(v);

        {
            let mut d = lock(&self.data.display);
            d.time = vec![VecDeque::new(); n_rows];
            d.peaks = vec![0.0; n_rows];
        }
        let svc = Arc::new(Mutex::new(svc));
        *lock(&self.svc) = Some(svc.clone());
        let pump_done = Arc::new(AtomicBool::new(false));
        {
            let run = self.clone();
            let svc = svc.clone();
            let done = pump_done.clone();
            let _ = std::thread::Builder::new()
                .name("hertzforge-recorder-pump".into())
                .spawn(move || {
                    run.pump(&svc, programmed, hardware);
                    // This thread's handle on the service goes before it says it is done.
                    drop(svc);
                    done.store(true, Ordering::SeqCst);
                });
        }
        {
            let run = self.clone();
            let _ = std::thread::Builder::new()
                .name("hertzforge-recorder-run".into())
                .spawn(move || {
                    let last = run.watch(&svc, &pump_done, record, log_written, warnings);
                    // The run is finished only once the hardware is free: the service (and with
                    // it the station) goes, then the lock, then the finished state is published.
                    // Published first, a start sent on it found the lock still held and was
                    // refused as if another program held the NI hardware.
                    drop(svc);
                    run.release_service();
                    drop(guard);
                    if let Some(v) = last {
                        run.publish(v);
                    }
                });
        }
        Ok(())
    }

    /// The station's reading thread: one batch at a time until a stop or an end, then
    /// the stop (every recording finalized).
    fn pump(&self, svc: &Mutex<RecorderService>, fs: f64, hardware: bool) {
        let keep = (DISPLAY_S * fs).round().max(1.0) as usize;
        let began = Instant::now();
        let mut frames = 0u64;
        loop {
            if self.stop.load(Ordering::SeqCst) != NONE {
                break;
            }
            let ended = {
                let mut s = lock(svc);
                let r = s.pump();
                if let Ok(n) = r {
                    frames += n;
                }
                let (first, batch) = s.last_batch();
                let mut d = lock(&self.data.display);
                let rows = d.time.len().max(1);
                let n = batch.len() / rows;
                if n > 0 && d.time_end <= first {
                    for f in 0..n {
                        for c in 0..rows {
                            let x = batch[f * rows + c];
                            d.time[c].push_back(x);
                            if x.abs() > d.peaks[c] {
                                d.peaks[c] = x.abs();
                            }
                        }
                    }
                    for c in 0..rows {
                        let col = &mut d.time[c];
                        while col.len() > keep {
                            col.pop_front();
                        }
                    }
                    d.time_end = first + n as u64;
                }
                r.is_err()
            };
            if ended {
                break;
            }
            if !hardware && fs > 0.0 {
                let due = Duration::from_secs_f64(frames as f64 / fs);
                let pause = due.saturating_sub(began.elapsed()).min(MAX_PACE);
                if !pause.is_zero() {
                    std::thread::sleep(pause);
                }
            } else {
                // The read is the pacing here, inside the lock: give way to a verb or the
                // status thread waiting for the service, or it can starve (an unfair lock
                // re-taken at once; verbs went unanswered on the NI bench).
                self.handoff.give_way(HANDOFF_MAX);
            }
        }
        // Acquisition ends here; the record thread finishes the files.
        let _ = lock(svc).stop_monitor();
    }

    /// Ask for the run to stop (`abort` false) or abort, for `cause`.
    pub fn request_stop(&self, abort: bool, cause: &'static str) {
        if abort {
            self.stop.store(ABORT, Ordering::SeqCst);
        } else {
            let _ = self
                .stop
                .compare_exchange(NONE, STOP, Ordering::SeqCst, Ordering::SeqCst);
        }
        let mut c = lock(&self.stop_cause);
        if c.is_none() || abort {
            *c = Some(cause);
        }
        drop(c);
        // Before the station is up there is no pump to see it: end at once.
        if !self.started() || self.service().is_none() && self.view().state == State::Configured {
            return;
        }
        self.shared.add_log(
            "state",
            format!("stop began: {}", if abort { "abort" } else { cause }),
        );
        let mut v = (*self.view()).clone();
        if v.state == State::Running {
            v.stop = Some(cause);
            v.state = State::Stopping;
            v.phase = "closing";
            self.publish(v);
        }
    }

    // -----------------------------------------------------------------------------
    // The verbs. Each takes the service lock: the server runs them on the blocking pool.
    // -----------------------------------------------------------------------------

    /// Publish the phase a verb has just made, so a state read after its answer
    /// shows it.
    fn note_phase(&self, st: RecState) {
        let mut v = (*self.view()).clone();
        if v.state == State::Running {
            v.phase = match st {
                RecState::Armed => "armed",
                RecState::Recording => "recording",
                _ => "monitoring",
            };
            self.publish(v);
        }
    }

    fn running(&self, verb: &str) -> Result<Arc<Mutex<RecorderService>>, MethodError> {
        let v = self.view();
        if v.state != State::Running || self.stop.load(Ordering::SeqCst) != NONE {
            return Err(wrong_state(&v, verb));
        }
        self.service().ok_or_else(|| wrong_state(&v, verb))
    }

    pub fn arm(&self) -> Result<Value, MethodError> {
        let v = self.view();
        if v.mode == Mode::Start {
            return Err(wrong_mode(&v, "recorder.arm"));
        }
        let svc = self.running("recorder.arm")?;
        let mut s = self.handoff.take(&svc);
        if s.state() != RecState::Monitoring {
            return Err(wrong_state(&self.view(), "recorder.arm"));
        }
        s.arm()
            .map_err(|e| wrong_state(&self.view(), &format!("recorder.arm ({e})")))?;
        let st = s.status(0);
        *lock(&self.saved_at_arm) = st.recordings_saved;
        self.note_phase(s.state());
        Ok(json!({"pre_frames": st.pre_frames_asked, "held_frames": st.pre_frames_held}))
    }

    pub fn disarm(&self) -> Result<Value, MethodError> {
        let v = self.view();
        if v.mode == Mode::Start {
            return Err(wrong_mode(&v, "recorder.disarm"));
        }
        let svc = self.running("recorder.disarm")?;
        let mut s = self.handoff.take(&svc);
        if s.state() != RecState::Armed {
            return Err(wrong_state(&self.view(), "recorder.disarm"));
        }
        s.disarm()
            .map_err(|e| wrong_state(&self.view(), &format!("recorder.disarm ({e})")))?;
        self.note_phase(s.state());
        Ok(Value::Null)
    }

    pub fn record(&self) -> Result<Value, MethodError> {
        let v = self.view();
        if v.mode != Mode::Manual {
            return Err(wrong_mode(&v, "recorder.record"));
        }
        let svc = self.running("recorder.record")?;
        let mut s = self.handoff.take(&svc);
        if !matches!(s.state(), RecState::Monitoring | RecState::Armed) {
            return Err(wrong_state(&self.view(), "recorder.record"));
        }
        s.record_now()
            .map_err(|e| wrong_state(&self.view(), &format!("recorder.record ({e})")))?;
        let now = s
            .status(0)
            .recording
            .ok_or_else(|| MethodError::internal("the recording did not begin"))?;
        self.note_phase(s.state());
        Ok(json!({
            "name": now.name,
            "station_frame": now.trigger_station_frame,
            "pre_frames": now.pre_frames,
        }))
    }

    pub fn cut(&self) -> Result<Value, MethodError> {
        let v = self.view();
        if v.mode == Mode::Level {
            return Err(wrong_mode(&v, "recorder.cut"));
        }
        let svc = self.running("recorder.cut")?;
        let mut s = self.handoff.take(&svc);
        let st = s.status(0);
        let Some(now) = st.recording.filter(|_| s.state() == RecState::Recording) else {
            return Err(wrong_state(&self.view(), "recorder.cut"));
        };
        s.stop_recording()
            .map_err(|e| wrong_state(&self.view(), &format!("recorder.cut ({e})")))?;
        self.note_phase(s.state());
        Ok(json!({
            "name": now.name,
            "last_station_frame": st.station_frames.saturating_sub(1),
        }))
    }

    // -----------------------------------------------------------------------------
    // The run's thread.
    // -----------------------------------------------------------------------------

    fn watch(
        &self,
        svc: &Arc<Mutex<RecorderService>>,
        pump_done: &AtomicBool,
        record: RunRecord,
        mut log_written: u64,
        warnings: Warnings,
    ) -> Option<RecorderView> {
        let folder = record.folder().to_path_buf();
        let streams_dir = folder.join(records::STREAMS);
        let log = record.log();
        let rows = self.view().live_rows.clone();
        let mut free: Option<u64> = None;
        let mut free_read: Option<Instant> = None;
        let mut marks_sent = 0usize;
        // The facts of the log's mark entries, by their place in the log, until written.
        let mut mark_data: HashMap<u64, (Value, u64)> = HashMap::new();
        let mut declared: Vec<String> = Vec::new();
        let mut known_recording: Option<String> = None;
        let mut saved_logged = 0usize;
        let mut driver_warnings = 0u64;
        loop {
            let done = pump_done.load(Ordering::SeqCst);
            if free_read.is_none_or(|t| t.elapsed() >= FREE_SPACE_PERIOD) || done {
                free = hertzforge_capture::free_space_bytes_for(&folder).ok();
                free_read = Some(Instant::now());
            }
            for w in std::mem::take(&mut *lock(&warnings)) {
                driver_warnings += 1;
                self.shared.add_log("note", format!("driver: {w}"));
            }
            let (st, fs, floor, marks, spectra, frf, saved, stats) = {
                let mut s = self.handoff.take(svc);
                let st = s.status(free.unwrap_or(0));
                let fs = s.actual_rate_hz();
                let floor = s.floor_bytes();
                let marks: Vec<Mark> = s.marks()[marks_sent..].to_vec();
                let acq_df = fs
                    / f64::from(self.wire.analysis.decimation as u32)
                    / self.wire.analysis.block as f64;
                let spectra: Vec<(usize, Vec<f64>)> = s.spectra();
                let frf = s.frf();
                let saved = s.report_data().recordings;
                let stats = s.row_stats();
                (st, fs, floor, marks, (acq_df, spectra), frf, saved, stats)
            };
            // Marks to the log, each entry with the mark's own facts for the record.
            if !marks.is_empty() {
                marks_sent += marks.len();
                for m in &marks {
                    let (kind, text) = mark_words(m);
                    let seq = self.shared.add_log_seq(kind, text);
                    let data = serde_json::to_value(m).unwrap_or(Value::Null);
                    mark_data.insert(seq, (data, mark_frame(m)));
                }
            }
            let now_name = st.recording.as_ref().map(|r| r.name.clone());
            if now_name != known_recording {
                if let Some(r) = &st.recording {
                    self.shared.add_log(
                        "recording",
                        format!(
                            "{} began at station frame {}{}",
                            r.name,
                            r.trigger_station_frame,
                            if r.pre_frames > 0 {
                                format!(" (pre-window {} frames)", r.pre_frames)
                            } else {
                                String::new()
                            }
                        ),
                    );
                }
                known_recording = now_name;
            }
            for r in &saved[saved_logged..] {
                self.shared.add_log(
                    "recording",
                    format!(
                        "{} saved: {} frames, {} s, {}",
                        r.name,
                        r.frames,
                        num(round(r.frames as f64 / fs.max(1e-9), 3)),
                        if r.gaps.is_empty() {
                            "no gaps".to_owned()
                        } else {
                            format!("{} gap(s)", r.gaps.len())
                        }
                    ),
                );
            }
            saved_logged = saved.len();
            // Every recording begun is declared in the manifest as a stream, in the order
            // they began (one begun and saved between two passes included).
            let mut begun: Vec<String> = saved.iter().map(|r| r.name.clone()).collect();
            if let Some(r) = &st.recording {
                if !begun.contains(&r.name) {
                    begun.push(r.name.clone());
                }
            }
            if begun.len() > declared.len() {
                declared = begun;
                let mut patch = serde_json::Map::new();
                patch.insert("streams".into(), records::streams_member(&declared, &rows));
                record.patch_manifest(patch);
            }

            // The display's spectra.
            {
                let v0 = self.view();
                let mut d = lock(&self.data.display);
                d.spectra = spectra
                    .1
                    .iter()
                    .filter_map(|(slot, psd)| {
                        let row = v0.live_rows.get(*slot)?;
                        Some(RowSpectrum {
                            index: row.index,
                            description: row.description.clone(),
                            unit: row.unit.clone(),
                            df_hz: spectra.0,
                            psd: psd.clone(),
                        })
                    })
                    .collect();
                d.frf = frf.map(|f| {
                    let unit = self.frf_unit(&v0);
                    FrfData {
                        unit,
                        df_hz: f.df_hz,
                        mag: f.h1_mag,
                        phase_deg: f.h1_phase_deg,
                        coherence: f.coherence,
                        valid: f.valid,
                    }
                });
            }

            let v = self.compose(&st, fs, floor, free, driver_warnings, done);
            // The log to the record, each entry with the station frame now (a mark's
            // own), and a mark's facts.
            let frame = (st.batches > 0).then_some(st.station_frames);
            let forward = |log_written: &mut u64, mark_data: &mut HashMap<u64, (Value, u64)>| {
                let (entries, next) = self.shared.log_since(*log_written);
                for e in &entries {
                    let (data, at) = match mark_data.remove(&e.seq) {
                        Some((d, f)) => (Some(d), Some(f)),
                        None => (None, frame),
                    };
                    log.append(log_entry(e, at, data));
                }
                *log_written = next;
            };
            forward(&mut log_written, &mut mark_data);
            if v.state == State::Finished {
                // The log's last lines, then the end; the caller publishes the finished
                // state once the hardware is free. Every recording is final by now: the
                // pump closed the record thread before it said it was done.
                forward(&mut log_written, &mut mark_data);
                let mut end = serde_json::Map::new();
                end.insert("run_uid".into(), Value::from(lock(&self.run_uid).as_str()));
                end.insert("run_id".into(), Value::from(self.id.as_str()));
                end.insert("end".into(), view_end_json(&v));
                end.insert("at_unix".into(), Value::from(now_unix()));
                end.insert("station_frames".into(), json!(v.station_frames));
                end.insert(
                    "rows".into(),
                    Value::Array(
                        stats
                            .iter()
                            .zip(&v.live_rows)
                            .map(|(s, r)| {
                                json!({
                                    "index": r.index,
                                    "mean": num(round(s.mean, 6)),
                                    "rms": num(round(s.rms, 6)),
                                    "peak": num(round(s.peak, 6)),
                                    "crest": num(round(s.crest, 6)),
                                    "kurtosis": num(round(s.kurtosis, 6)),
                                })
                            })
                            .collect(),
                    ),
                );
                end.insert("streams".into(), records::end_streams(&streams_dir));
                let status = record.status();
                record.finish(end);
                // The finished state is published once the record says so: `end.json`
                // written and `run.json` complete. This thread reads no station; the
                // wait is bounded so a disk that hangs cannot hold the run's end.
                let t0 = Instant::now();
                while !status.finished() && t0.elapsed() < RECORD_FINISH_WAIT {
                    std::thread::sleep(Duration::from_millis(5));
                }
                return Some(v);
            }
            self.publish(v);
            std::thread::sleep(PUBLISH_PERIOD);
        }
    }

    fn frf_unit(&self, v: &RecorderView) -> String {
        let unit = |index: u32| {
            v.live_rows
                .iter()
                .find(|r| r.index == index)
                .map_or("g".to_owned(), |r| r.unit.clone())
        };
        match self.wire.analysis.frf {
            Some((x, y)) => format!("{}/{}", unit(y), unit(x)),
            None => String::new(),
        }
    }

    /// The view from the core's status.
    fn compose(
        &self,
        st: &RecStatus,
        fs: f64,
        floor: u64,
        free: Option<u64>,
        driver_warnings: u64,
        pump_done: bool,
    ) -> RecorderView {
        let mut v = (*self.view()).clone();
        let w = &self.wire;
        let bytes_per_s = self.derived_figures.bytes_per_s.max(1);
        v.grid.fs_hz = fs;
        v.station_frames = Some(st.station_frames);
        v.t_s = Some(st.station_frames as f64 / fs.max(1e-9));
        let saved_at_arm = *lock(&self.saved_at_arm);
        let in_progress = st.recording.is_some() as u64;
        v.trigger = (w.trigger.mode != Mode::Start).then(|| Trigger {
            armed: st.trigger_armed,
            pre_frames: if st.pre_frames_asked > 0 {
                st.pre_frames_asked
            } else {
                frames_for(w.pre_s(), fs)
            },
            held_frames: st.pre_frames_held,
            fires: st.trigger_fires,
            last_station_frame: st.last_fire_station_frame,
            saved: st.recordings_saved.saturating_sub(saved_at_arm),
            left: match (w.trigger.mode, w.trigger.count) {
                (Mode::Level, Some(n)) => Some(n.saturating_sub(
                    st.recordings_saved.saturating_sub(saved_at_arm) + in_progress,
                )),
                _ => None,
            },
            holdoff_left_s: 0.0,
        });
        v.recording = st.recording.as_ref().map(|r| Recording {
            name: r.name.clone(),
            trigger: r.trigger.clone(),
            first_station_frame: r.first_station_frame,
            trigger_station_frame: r.trigger_station_frame,
            pre_frames: r.pre_frames,
            pre_frames_asked: r.pre_frames_asked,
            frames_fed: r.frames_fed,
            frames_on_disk: r.frames_on_disk.min(r.frames_fed),
            target_frames: r.target_frames,
            seconds: r.frames_fed.saturating_sub(r.pre_frames) as f64 / fs.max(1e-9),
            length_s: w.trigger.record_s,
            backlog_s: r.frames_fed.saturating_sub(r.frames_on_disk) as f64 / fs.max(1e-9),
        });
        v.recordings = st.recordings_saved;
        v.marks = st.marks;
        v.disk = Some(Disk {
            free_bytes: free,
            floor_bytes: floor,
            seconds_above_floor: free.map(|f| f.saturating_sub(floor) / bytes_per_s),
            fault: st.write_fault.clone(),
        });
        v.health = Some(Health {
            batches: st.batches,
            blocks_dropped: 0,
            driver_warnings,
            station_errors: 0,
            pass_last_ms: st.pass_ms_last,
            pass_worst_ms: st.pass_ms_worst,
            batch_gap_ms: st.batch_gap_ms_max,
        });
        // The rows: rms over the last second, the peak since start, the overload
        // latch and the marks.
        let (time_first, cols) = self.data.time();
        let _ = time_first;
        let peaks = lock(&self.data.display).peaks.clone();
        let one_s = fs.round().max(1.0) as usize;
        for (i, row) in v.live_rows.iter_mut().enumerate() {
            if let Some(col) = cols.get(i).filter(|c| !c.is_empty()) {
                let tail = &col[col.len().saturating_sub(one_s)..];
                let ms = tail
                    .iter()
                    .map(|x| f64::from(*x) * f64::from(*x))
                    .sum::<f64>()
                    / tail.len() as f64;
                row.rms = Some(ms.sqrt());
                row.peak = peaks.get(i).map(|p| f64::from(*p));
            }
            row.overload = st.overload_latches.get(i).is_some_and(|(_, l)| *l);
        }
        // Per-row mark counts.
        {
            let svc = self.service();
            if let Some(svc) = svc {
                let s = self.handoff.take(&svc);
                for row in v.live_rows.iter_mut() {
                    row.marks = s
                        .marks()
                        .iter()
                        .filter(|m| matches!(m, Mark::Limit { row: r, .. } if *r == row.index))
                        .count() as u64;
                }
            }
        }

        let stop = self.stop.load(Ordering::SeqCst);
        if let Some(f) = &st.run_fault {
            // The core ended the run on its own.
            if pump_done {
                v.state = State::Finished;
                let (outcome, message) = match f.cause {
                    EndCause::WriteFailed => (
                        "write_failed",
                        match &st.recording {
                            Some(_) => f.message.clone(),
                            None => f.message.clone(),
                        },
                    ),
                    EndCause::WriteBehind => ("write_behind", f.message.clone()),
                    EndCause::Fault => ("fault", f.message.clone()),
                };
                if v.end.is_none() {
                    self.shared
                        .add_log("state", format!("ended {outcome}: {message}"));
                }
                v.end = Some(End {
                    outcome,
                    message,
                    at_s: Some(f.station_frame as f64 / fs.max(1e-9)),
                    recordings: st.recordings_saved,
                });
                v.phase = v.end.as_ref().map_or("faulted", End::phase);
            } else {
                v.state = State::Stopping;
                v.phase = "closing";
            }
        } else if stop != NONE {
            if pump_done {
                let cause = lock(&self.stop_cause).unwrap_or("operator");
                let by = self
                    .shared
                    .ended_by()
                    .unwrap_or_else(|| "the operator".into());
                let (outcome, message) = if stop == ABORT {
                    (
                        "operator_abort",
                        format!("Aborted by {by}: every recording is saved."),
                    )
                } else {
                    match cause {
                        "safe_stop" => (
                            "safe_stop",
                            "Nobody held control: the run stopped and every recording is saved."
                                .to_owned(),
                        ),
                        "shutdown" => (
                            "safe_stop",
                            "The service shut down: the run stopped and every recording is \
                             saved."
                                .to_owned(),
                        ),
                        _ => (
                            "operator_stop",
                            format!("Stopped by {by}: every recording is saved."),
                        ),
                    }
                };
                if v.end.is_none() {
                    self.shared
                        .add_log("state", format!("ended {outcome}: {message}"));
                }
                v.state = State::Finished;
                v.end = Some(End {
                    outcome,
                    message,
                    at_s: v.t_s,
                    recordings: st.recordings_saved,
                });
                v.phase = v.end.as_ref().map_or("stopped", End::phase);
                v.stop = Some(if stop == ABORT { "operator" } else { cause });
            } else {
                v.state = State::Stopping;
                v.phase = "closing";
                v.stop = Some(lock(&self.stop_cause).unwrap_or("operator"));
            }
        } else {
            v.state = State::Running;
            v.phase = match st.state {
                RecState::Armed => "armed",
                RecState::Recording => "recording",
                _ => "monitoring",
            };
        }
        if v.state == State::Finished {
            v.recording = None;
        }
        if let (Some(r), Some(status)) = (v.record.as_mut(), lock(&self.record_status).as_ref()) {
            r.problem = status.problem();
        }
        v
    }

    /// The run's recordings from `since`, the one in progress last.
    pub fn recordings(&self, since: u64) -> (Vec<Value>, u64) {
        let Some(folder) = self.folder() else {
            return (Vec::new(), 0);
        };
        let v = self.view();
        let in_progress = v.recording.as_ref().map(|r| r.name.clone());
        let all = records::list(&folder.join(records::STREAMS), in_progress.as_deref());
        let next = all.len() as u64;
        (all.into_iter().skip(since as usize).collect(), next)
    }

    /// The run's marks from `since`.
    pub fn marks(&self, since: u64) -> (Vec<Value>, u64) {
        // Once the run has ended its service is released: its marks were kept.
        let all: Vec<Mark> = match self.service() {
            Some(svc) => self.handoff.take(&svc).marks().to_vec(),
            None => lock(&self.final_marks).clone(),
        };
        let next = all.len() as u64;
        (
            all.iter()
                .skip(since as usize)
                .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
                .collect(),
            next,
        )
    }

    /// The live rows as the view holds them, for a new configuration of the table.
    pub fn rows(&self) -> Vec<LiveRow> {
        self.view().live_rows.clone()
    }
}

/// Seconds since 1970, from the wall clock.
fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// A log entry as the run's record holds it: the wire's members, the station frame
/// when it was handed to the record (null before the station has delivered one; a
/// mark's own), and `data`, a mark's facts, when there are any.
fn log_entry(e: &crate::random::run::LogEntry, frame: Option<u64>, data: Option<Value>) -> Value {
    let mut v = json!({"seq": e.seq, "at_unix": e.at_unix, "frame": frame,
                       "kind": e.kind, "text": e.text});
    if let (Some(d), Value::Object(m)) = (data, &mut v) {
        m.insert("data".into(), d);
    }
    v
}

/// The state's `end`, for `end.json`.
fn view_end_json(v: &RecorderView) -> Value {
    v.end.as_ref().map_or(Value::Null, End::to_json)
}

/// The station frame a mark was made at.
fn mark_frame(m: &Mark) -> u64 {
    match m {
        Mark::Limit { station_frame, .. } | Mark::Overload { station_frame, .. } => *station_frame,
    }
}

/// A mark in words, for the log: its kind and text.
pub fn mark_words(m: &Mark) -> (&'static str, String) {
    match m {
        Mark::Limit {
            row,
            axis,
            value,
            limit,
            station_frame,
            recording,
        } => (
            "mark",
            format!(
                "row {row} {axis} {} past its limit {} at station frame {station_frame}{}",
                num(round(*value, 4)),
                num(*limit),
                recording
                    .as_ref()
                    .map(|r| format!(", in {r}"))
                    .unwrap_or_default()
            ),
        ),
        Mark::Overload {
            row,
            value,
            level,
            station_frame,
            ..
        } => (
            "overload",
            format!(
                "row {row} overload: {} reached {} (0.9 of its range) at station frame \
                 {station_frame}",
                num(round(*value, 4)),
                num(round(*level, 4))
            ),
        ),
    }
}

#[cfg(test)]
mod recorder_handoff_tests {
    use super::*;

    /// The lock held by this same service (a run just finishing) is waited for, not refused.
    #[test]
    fn recorder_a_start_waits_for_this_services_own_run_to_free_the_hardware() {
        let dir = std::env::temp_dir().join(format!("hf_recorder_lock_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let held = hertzforge_station::lock::acquire(&dir, "127.0.0.1:0", "station").unwrap();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        let t0 = Instant::now();
        let got = acquire_after_own_run(&dir, "127.0.0.1:0", "station");
        let waited = t0.elapsed();
        t.join().unwrap();
        assert!(
            got.is_ok(),
            "refused while this service's own run let the hardware go"
        );
        assert!(
            waited >= Duration::from_millis(250),
            "it did not wait: {waited:?}"
        );
        drop(got);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recorder_handoff_gives_way_only_while_someone_waits() {
        let h = Handoff::default();
        let t0 = Instant::now();
        h.give_way(Duration::from_millis(200));
        assert!(
            t0.elapsed() < Duration::from_millis(50),
            "no one waiting: no wait"
        );
        h.waiting.fetch_add(1, Ordering::SeqCst);
        let t0 = Instant::now();
        h.give_way(Duration::from_millis(30));
        let w = t0.elapsed();
        assert!(
            w >= Duration::from_millis(30) && w < Duration::from_millis(500),
            "bounded: {w:?}"
        );
    }

    /// A pump that holds the lock almost all the time, as the hardware read does, and takes it
    /// again at once: a waiter through the handoff gets it every time, within a batch or two.
    #[test]
    fn recorder_a_waiter_gets_the_service_from_a_pump_that_holds_it() {
        let h = Arc::new(Handoff::default());
        let m = Arc::new(Mutex::new(0u64));
        let stop = Arc::new(AtomicBool::new(false));
        let pump = {
            let (h, m, stop) = (h.clone(), m.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    {
                        let mut g = lock(&m);
                        std::thread::sleep(Duration::from_millis(10));
                        *g += 1;
                    }
                    h.give_way(HANDOFF_MAX);
                }
            })
        };
        let mut worst = Duration::ZERO;
        for _ in 0..20 {
            let t0 = Instant::now();
            drop(h.take(&m));
            worst = worst.max(t0.elapsed());
            std::thread::sleep(Duration::from_millis(3));
        }
        stop.store(true, Ordering::SeqCst);
        pump.join().unwrap();
        assert!(
            worst < Duration::from_millis(500),
            "a waiter waited {worst:?}"
        );
    }
}
