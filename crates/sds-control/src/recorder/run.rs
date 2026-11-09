//! The Recorder run: the core's `RecorderService` on the station the run starts on, a
//! pump thread that reads the station one batch at a time, and the run's own thread.
//!
//! The pump thread is the station's reading thread. It holds the service for one batch
//! and one analysis pass, hands the batch to the display ring, and does nothing else:
//! no file, no free-space query, no join. The run's thread composes the view ten times
//! a second, reads the free space once a second, logs, and writes the run folder's
//! records. The core's record thread writes the recordings. Verbs take the service
//! lock, so the server runs them on the blocking pool with a bounded wait.

use super::config::{frames_for, round, Derived, Mode, RecorderStation, WireRecorder};
use super::records::{self, Records};
use super::sim_signal::{Signal, Tap, Warnings};
use super::subscribe::{FrfData, RowSpectrum};
use super::view::{Disk, End, Health, LiveRow, RecorderView, Recording, State, Trigger};
use crate::protocol::{num, ErrorCode, MethodError};
use crate::pulse::rails::Shared;
use crate::station::{self, NiScanner, StationPlan};
use hertzforge_channel_table::ChannelTable;
use hertzforge_recorder::service::{EndCause, Mark, RecState, RecStatus};
use hertzforge_recorder::RecorderService;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How often the run's thread composes and publishes the view.
pub const PUBLISH_PERIOD: Duration = Duration::from_millis(100);
/// How often the run's thread reads the data volume's free space.
pub const FREE_SPACE_PERIOD: Duration = Duration::from_secs(1);
/// Seconds of every recorded row the display ring keeps.
pub const DISPLAY_S: f64 = 2.0;
/// The longest a wall-clock-paced pump sleeps in one go, so a stop is never left
/// waiting behind a long pause.
const MAX_PACE: Duration = Duration::from_millis(50);

/// The overload indicator's definition, for `run.json`.
pub const OVERLOAD_DEFINITION: &str = "A row's overload indicator latches the first time a \
     sample's magnitude reaches 0.9 of the row's range_max, and holds for the run.";

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
            let records = Records::open(&ctx.records_root, &w.run_name).map_err(|e| {
                refuse(
                    "arm_refused",
                    format!("The run folder could not be made: {e}"),
                )
            })?;
            let signal = if hardware { None } else { ctx.signal.clone() };
            let backend: Box<dyn hertzforge_hal::Backend> =
                Box::new(Tap::new(station.backend, signal, n_rows, warnings.clone()));
            let mut svc = RecorderService::with_backend(
                w.build(&ctx.table),
                ctx.table.clone(),
                backend,
                station.station,
                records.recordings(),
            )
            .map_err(|e| refuse("arm_refused", format!("The station refused to start: {e}")))?
            .with_block_frames(hertzforge_hal::default_block_frames(fs))
            .with_group_delay_frames(self.station_kind.group_delay_frames(fs));
            svc.configure()
                .map_err(|e| refuse("arm_refused", format!("The station refused to start: {e}")))?;
            svc.start_monitor()
                .map_err(|e| refuse("arm_refused", format!("The station refused to start: {e}")))?;
            if w.trigger.mode == Mode::Start {
                svc.record_from_start().map_err(|e| {
                    refuse("arm_refused", format!("The recording could not begin: {e}"))
                })?;
            }
            Ok::<_, MethodError>((svc, guard, clock, hardware, records))
        })();
        let (svc, guard, clock, hardware, records) = match built {
            Ok(b) => b,
            Err(e) => {
                self.end_before_start(&e);
                return Err(e);
            }
        };
        let programmed = svc.actual_rate_hz();
        let folder = records.folder().to_path_buf();
        *lock(&self.folder) = Some(folder.clone());
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
        self.shared
            .add_log("note", format!("records in {}", folder.display()));
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

        // run.json: the configuration, the station and the derived numbers at the
        // programmed rate.
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
        let v0 = self.view();
        records.write_run(&json!({
            "run_id": self.id,
            "title": self.wire.title,
            "source": v0.source.unwrap_or("SIM"),
            "station": v0.station.to_json(),
            "config": self.config,
            "requested_fs_hz": num(fs),
            "fs_hz": num(programmed),
            "group_delay_frames": num(derived_now.group_delay_frames),
            "outputs": "none",
            "rows": v0.live_rows.iter().map(|r| {
                let range = ctx.table.channels.iter().find(|c| c.index == r.index)
                    .map_or(0.0, |c| c.range_max);
                json!({"index": r.index, "description": r.description, "role": r.role,
                       "unit": r.unit, "range_max": num(range)})
            }).collect::<Vec<_>>(),
            "derived": derived_now.to_json(),
            "overload": {"fraction": num(super::config::OVERLOAD_FRACTION),
                         "definition": OVERLOAD_DEFINITION},
        }));

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
        self.shared
            .add_log("state", format!("running, {}", v.phase));
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
                    done.store(true, Ordering::SeqCst);
                });
        }
        {
            let run = self.clone();
            let _ = std::thread::Builder::new()
                .name("hertzforge-recorder-run".into())
                .spawn(move || {
                    let _guard = guard;
                    run.watch(&svc, &pump_done, records, warnings);
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
                // Let a parked verb take the lock between batches.
                std::thread::yield_now();
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
        let mut s = lock(&svc);
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
        let mut s = lock(&svc);
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
        let mut s = lock(&svc);
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
        let mut s = lock(&svc);
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
        records: Records,
        warnings: Warnings,
    ) {
        let fs_asked = self.wire.fs_hz;
        let mut free: Option<u64> = None;
        let mut free_read: Option<Instant> = None;
        let mut marks_sent = 0usize;
        let mut log_written = 0u64;
        let mut known_recording: Option<String> = None;
        let mut saved_logged = 0usize;
        let mut driver_warnings = 0u64;
        loop {
            let done = pump_done.load(Ordering::SeqCst);
            if free_read.is_none_or(|t| t.elapsed() >= FREE_SPACE_PERIOD) || done {
                free = hertzforge_capture::free_space_bytes_for(records.folder()).ok();
                free_read = Some(Instant::now());
            }
            for w in std::mem::take(&mut *lock(&warnings)) {
                driver_warnings += 1;
                self.shared.add_log("note", format!("driver: {w}"));
            }
            let (st, fs, floor, marks, spectra, frf, saved, stats) = {
                let mut s = lock(svc);
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
            // Marks and recordings, to the log and the records.
            if !marks.is_empty() {
                marks_sent += marks.len();
                let lines: Vec<Value> = marks
                    .iter()
                    .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
                    .collect();
                for m in &marks {
                    let (kind, text) = mark_words(m);
                    self.shared.add_log(kind, text);
                }
                records.marks(&lines);
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
            for e in self.shared.log_since(log_written).0.iter() {
                records.log(&[e.to_json()]);
            }
            log_written = self.shared.log_since(0).1;
            let finished = v.state == State::Finished;
            if finished {
                let end = v.end.as_ref().map_or(Value::Null, End::to_json);
                records.write_end(&json!({
                    "run_id": self.id,
                    "end": end,
                    "station_frames": v.station_frames,
                    "rows": stats.iter().zip(&v.live_rows).map(|(s, r)| json!({
                        "index": r.index,
                        "mean": num(round(s.mean, 6)),
                        "rms": num(round(s.rms, 6)),
                        "peak": num(round(s.peak, 6)),
                        "crest": num(round(s.crest, 6)),
                        "kurtosis": num(round(s.kurtosis, 6)),
                    })).collect::<Vec<_>>(),
                }));
            }
            self.publish(v);
            if finished {
                // The log's last lines.
                for e in self.shared.log_since(log_written).0.iter() {
                    records.log(&[e.to_json()]);
                }
                break;
            }
            let _ = fs_asked;
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
                let s = lock(&svc);
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
        v
    }

    /// The run's recordings from `since`, the one in progress last.
    pub fn recordings(&self, since: u64) -> (Vec<Value>, u64) {
        let Some(folder) = self.folder() else {
            return (Vec::new(), 0);
        };
        let v = self.view();
        let in_progress = v.recording.as_ref().map(|r| r.name.clone());
        let all = records::list(&folder.join(records::RECORDINGS), in_progress.as_deref());
        let next = all.len() as u64;
        (all.into_iter().skip(since as usize).collect(), next)
    }

    /// The run's marks from `since`.
    pub fn marks(&self, since: u64) -> (Vec<Value>, u64) {
        let Some(svc) = self.service() else {
            return (Vec::new(), 0);
        };
        let s = lock(&svc);
        let all = s.marks();
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
