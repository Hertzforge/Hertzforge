//! The run record: one folder per run, written while the run goes, that the analysis
//! side converts into a project file (`docs/run-folder.md`; the reader is
//! `hertzforge.io.run_folder`).
//!
//! A [`RunRecord`] owns ONE writer thread, and every file operation of the record
//! happens on it: claiming the folder, the small files (written whole and
//! atomically), the log and the streams (appended, and synced on a cadence), the
//! results and the end. Everything else hands the writer messages:
//!
//! - the run thread opens the record at arm and waits for the answer, because the
//!   arm is refused when `run.json`, `table.json` and `setup.json` cannot be
//!   written ([`RunRecord::open`]); nothing has been driven then;
//! - after that, every handle sends and returns at once: [`LogHandle::append`],
//!   [`RunRecord::put_result`], [`RunRecord::patch_manifest`],
//!   [`RunRecord::finish`];
//! - the streams are fed from the engine's thread through [`tap::StreamTap`], which
//!   takes a buffer made before the run, fills it, and sends it; when the writer has
//!   fallen behind and no buffer is free, the block is dropped, counted, and its
//!   place in the stream kept as a gap. It never waits, never locks and never
//!   touches a file.
//!
//! A write that fails is never swallowed: it reaches the run through the problem
//! callback given at open (the run puts it in its log, kind `record`, and in its
//! state). A stream that fails stops being written and says why in its
//! `stream.json`; the run goes on. Nothing here can stop, stall or abort a run.
//!
//! A run that stops any way, a killed service included, leaves a folder the reader
//! takes whole: `run.json` says `complete: false` until [`RunRecord::finish`] has
//! written `end.json`.
//!
//! A pulse module's stream is a run of shots. A stream opened with a `shots` member
//! in its description keeps a list of them: the tap marks where each begins and
//! ends ([`tap::StreamTap::begin_shot`]), in order with its blocks, so each shot's
//! `first_frame` and `frames` are its place in the file as written, and the run
//! names what each shot was ([`RunRecord::label_shot`]).

pub mod tap;

use serde_json::{json, Map, Value};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The schema every manifest names.
pub const SCHEMA: &str = "hertzforge-run/1";

/// How often the streams and the log are synced to the disk, and the streams'
/// descriptions rewritten with what they hold.
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// The free space below which the streams stop being written, beyond what the
/// streams already hold (the converted file needs about as much again): 512 MiB.
pub const FREE_SPACE_FLOOR_BYTES: u64 = 512 * 1024 * 1024;

/// Seconds of samples the streams' buffers hold by default.
pub const STREAM_BUFFER_S: f64 = 4.0;

/// How a record is written.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordOptions {
    /// How often streams and the log are synced.
    pub flush_interval: Duration,
    /// The free space kept beyond the streams' own size.
    pub free_space_floor_bytes: u64,
    /// A pause before each write. Zero in use; a test sets it to show that a slow
    /// disk never reaches the run.
    pub write_delay: Duration,
    /// Seconds of samples the streams' buffers hold between them: how far the
    /// writer may fall behind before blocks are dropped. A simulator run unpaced
    /// goes many times faster than real time and needs more.
    pub stream_buffer_s: f64,
}

impl Default for RecordOptions {
    fn default() -> Self {
        RecordOptions {
            flush_interval: FLUSH_INTERVAL,
            free_space_floor_bytes: FREE_SPACE_FLOOR_BYTES,
            write_delay: Duration::ZERO,
            stream_buffer_s: STREAM_BUFFER_S,
        }
    }
}

/// Called on the writer thread with a problem in words and its structured facts
/// (`stream` and `failed`), for the run's log and state.
pub type ProblemFn = Box<dyn Fn(String, Value) + Send + 'static>;

/// A stream the record opens: its name, its `stream.json` as at the start (without
/// `name`, `frames`, `gaps`, `complete` and `fault`, which the writer keeps), and
/// the link to the tap that feeds it.
pub struct StreamSpec {
    pub name: String,
    pub description: Map<String, Value>,
    pub link: tap::TapLink,
}

/// A law file's name and bytes.
pub type LawFile = (String, Vec<u8>);

/// What a record holds before the first sample.
pub struct Begin {
    /// Where run folders go.
    pub root: PathBuf,
    /// The run's name: the folder is `<name>` or the first free `<name>-<n>`.
    pub name: String,
    /// `run.json` without `streams` and `complete`, which the writer keeps.
    pub manifest: Map<String, Value>,
    pub table: Value,
    pub setup: Value,
    /// `law/law.json`, and the law file's name and bytes when the law is a file.
    pub law: Option<(Value, Option<LawFile>)>,
    /// The log entries made before the record opened, in order.
    pub log: Vec<Value>,
    pub streams: Vec<StreamSpec>,
}

/// Why a record could not be opened: the path and the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenError {
    pub path: PathBuf,
    pub reason: String,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.reason)
    }
}

/// One named array of a result.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultArray {
    pub name: String,
    pub shape: Vec<usize>,
    pub unit: String,
    pub values: ArrayValues,
}

/// An array's values: real, or complex as (real, imaginary).
#[derive(Debug, Clone, PartialEq)]
pub enum ArrayValues {
    F64(Vec<f64>),
    C128(Vec<(f64, f64)>),
}

impl ArrayValues {
    fn dtype(&self) -> &'static str {
        match self {
            ArrayValues::F64(_) => "f8",
            ArrayValues::C128(_) => "c16",
        }
    }

    fn len(&self) -> usize {
        match self {
            ArrayValues::F64(v) => v.len(),
            ArrayValues::C128(v) => v.len(),
        }
    }

    fn item_bytes(&self) -> usize {
        match self {
            ArrayValues::F64(_) => 8,
            ArrayValues::C128(_) => 16,
        }
    }

    fn put(&self, out: &mut Vec<u8>) {
        match self {
            ArrayValues::F64(v) => v
                .iter()
                .for_each(|x| out.extend_from_slice(&x.to_le_bytes())),
            ArrayValues::C128(v) => v.iter().for_each(|(re, im)| {
                out.extend_from_slice(&re.to_le_bytes());
                out.extend_from_slice(&im.to_le_bytes());
            }),
        }
    }
}

/// What the run thread and the writer share: the folder, the latest problem and
/// whether the record has finished.
#[derive(Debug)]
pub struct RecordStatus {
    pub folder: PathBuf,
    problem: Mutex<Option<String>>,
    finished: AtomicBool,
}

impl RecordStatus {
    /// The latest problem, in words.
    pub fn problem(&self) -> Option<String> {
        match self.problem.lock() {
            Ok(p) => p.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    /// True once the writer has written everything it was given after `finish` or
    /// after its handles were dropped.
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }
}

/// The writer's answer to an open: the status, and which streams were made.
type Answer = (Arc<RecordStatus>, Vec<bool>);

enum Msg {
    Log(Value),
    Manifest(Map<String, Value>),
    Result {
        name: String,
        description: Map<String, Value>,
        arrays: Vec<ResultArray>,
        record: Option<Value>,
    },
    Block {
        stream: usize,
        buf: Vec<f32>,
        frames: usize,
        dropped_before: u64,
    },
    StreamPatch {
        stream: usize,
        patch: Map<String, Value>,
    },
    /// A shot begins (`facts` set) or ends (`None`) in a stream of shots, after
    /// the frames already sent; `dropped_before` frames were lost just before it.
    Shot {
        stream: usize,
        begin: Option<Map<String, Value>>,
        dropped_before: u64,
    },
    /// What the shot `ordinal` (from 0, in the order the shots began) was.
    ShotLabel {
        stream: usize,
        ordinal: usize,
        facts: Map<String, Value>,
    },
    Finish(Map<String, Value>),
}

/// The run's record: the writer thread's handle.
pub struct RunRecord {
    tx: Sender<Msg>,
    status: Arc<RecordStatus>,
}

/// A handle that appends log entries; it never blocks.
#[derive(Clone)]
pub struct LogHandle {
    tx: Sender<Msg>,
}

impl LogHandle {
    /// Append one entry (a JSON object) to `log.jsonl`.
    pub fn append(&self, entry: Value) {
        let _ = self.tx.send(Msg::Log(entry));
    }
}

impl RunRecord {
    /// Claim the run folder and write everything a run has before its first
    /// sample, on the record's own thread, waiting for it to be done. `Err` when
    /// the folder, `run.json`, `table.json`, `setup.json` or the law cannot be
    /// written: the caller refuses the arm with the path and the reason. A stream
    /// whose files cannot be made is a problem reported through `problem`, and the
    /// record goes on without it.
    pub fn open(
        begin: Begin,
        options: &RecordOptions,
        problem: ProblemFn,
    ) -> Result<RunRecord, OpenError> {
        let (tx, rx) = mpsc::channel::<Msg>();
        let (answer_tx, answer_rx) = mpsc::channel::<Result<Answer, OpenError>>();
        let options = options.clone();
        let root = begin.root.clone();
        let links: Vec<tap::TapLink> = begin.streams.iter().map(|s| s.link.clone()).collect();
        std::thread::Builder::new()
            .name("run-record".into())
            .spawn(move || {
                let mut w = match Writer::open(begin, options, problem) {
                    Ok(w) => w,
                    Err(e) => {
                        let _ = answer_tx.send(Err(e));
                        return;
                    }
                };
                let opened = w.streams.iter().map(|s| s.file.is_some()).collect();
                let _ = answer_tx.send(Ok((w.status.clone(), opened)));
                w.run(rx);
            })
            .map_err(|e| OpenError {
                path: root.clone(),
                reason: format!("the record's writer could not start: {e}"),
            })?;
        let (status, opened): Answer = answer_rx.recv().map_err(|_| OpenError {
            path: root,
            reason: "the record's writer stopped before it answered".into(),
        })??;
        // Only now is a stream fed: its files exist and its writer is listening.
        for (id, (link, open)) in links.iter().zip(opened).enumerate() {
            if open {
                link.attach(tx.clone(), id);
            }
        }
        Ok(RunRecord { tx, status })
    }

    /// The run folder.
    pub fn folder(&self) -> &Path {
        &self.status.folder
    }

    pub fn status(&self) -> Arc<RecordStatus> {
        self.status.clone()
    }

    pub fn log(&self) -> LogHandle {
        LogHandle {
            tx: self.tx.clone(),
        }
    }

    /// Set members of `run.json` (each replaces the member of its name) and
    /// rewrite it.
    pub fn patch_manifest(&self, patch: Map<String, Value>) {
        let _ = self.tx.send(Msg::Manifest(patch));
    }

    /// Set members of a stream's `stream.json`.
    pub fn patch_stream(&self, stream: usize, patch: Map<String, Value>) {
        let _ = self.tx.send(Msg::StreamPatch { stream, patch });
    }

    /// Name what a stream's shot `ordinal` (from 0, in the order they began) was:
    /// its members are set on the shot in `stream.json`. A label may come before
    /// its shot has begun; it is kept until it has.
    pub fn label_shot(&self, stream: usize, ordinal: usize, facts: Map<String, Value>) {
        let _ = self.tx.send(Msg::ShotLabel {
            stream,
            ordinal,
            facts,
        });
    }

    /// Write a result. `record` is `None` for a result written once (its `.json`
    /// and `.bin` whole), or the line of `<name>.jsonl` for a result that repeats
    /// (each call appends its arrays to `<name>.bin`; the arrays of every record
    /// have the shapes of the first). `description` holds `kind`, `abscissa`,
    /// `channels` and the rest; the writer adds `name` and `arrays`. A result is
    /// declared in `run.json` when it is first written.
    pub fn put_result(
        &self,
        name: &str,
        description: Map<String, Value>,
        arrays: Vec<ResultArray>,
        record: Option<Value>,
    ) {
        let _ = self.tx.send(Msg::Result {
            name: name.to_owned(),
            description,
            arrays,
            record,
        });
    }

    /// End the record: `end.json` from `end` (with each stream's frames, drops and
    /// gaps added), then `run.json` with `complete: true`. The writer finishes on
    /// its own; this returns at once.
    pub fn finish(self, end: Map<String, Value>) {
        let _ = self.tx.send(Msg::Finish(end));
    }
}

// ---- the writer -------------------------------------------------------------------

struct StreamState {
    name: String,
    dir: PathBuf,
    file: Option<File>,
    description: Map<String, Value>,
    frames: u64,
    width: usize,
    gaps: Vec<(u64, u64)>,
    dropped: u64,
    fault: Option<String>,
    link: tap::Returns,
    bytes: Vec<u8>,
    /// A stream of shots: each shot's members, the one open, and labels for
    /// shots not yet begun. `None` for a continuous stream.
    shots: Option<Shots>,
}

/// A stream's shots as the writer keeps them.
#[derive(Default)]
struct Shots {
    list: Vec<Map<String, Value>>,
    open: Option<usize>,
    waiting: Vec<(usize, Map<String, Value>)>,
}

impl Shots {
    fn label(&mut self, ordinal: usize, facts: Map<String, Value>) {
        match self.list.get_mut(ordinal) {
            Some(shot) => shot.extend(facts),
            None => self.waiting.push((ordinal, facts)),
        }
    }

    /// Every shot in the reader's form: a shot labelled with `pre_frames` is
    /// fired at its first station frame plus them (`station_frame`).
    fn to_json(&self) -> Value {
        Value::Array(
            self.list
                .iter()
                .map(|shot| {
                    let mut s = shot.clone();
                    let first = s.get("first_station_frame").and_then(Value::as_u64);
                    let pre = s.get("pre_frames").and_then(Value::as_u64);
                    if let (Some(first), Some(pre)) = (first, pre) {
                        s.insert("station_frame".into(), Value::from(first + pre));
                    }
                    Value::Object(s)
                })
                .collect(),
        )
    }
}

impl StreamState {
    fn sidecar(&self, complete: bool) -> Value {
        let mut d = self.description.clone();
        d.insert("name".into(), Value::from(self.name.as_str()));
        d.insert("frames".into(), Value::from(self.frames));
        d.insert(
            "gaps".into(),
            Value::Array(
                self.gaps
                    .iter()
                    .map(|(at, n)| json!({"at_frame": at, "dropped_frames": n}))
                    .collect(),
            ),
        );
        d.insert("complete".into(), Value::from(complete));
        d.insert(
            "fault".into(),
            self.fault.as_deref().map_or(Value::Null, Value::from),
        );
        if let Some(shots) = &self.shots {
            d.insert("shots".into(), shots.to_json());
        }
        Value::Object(d)
    }

    /// A shot begins at the next frame the file holds, or the open one ends with
    /// the frames the file holds for it.
    fn shot(&mut self, begin: Option<Map<String, Value>>, dropped_before: u64) {
        self.lost(dropped_before);
        let at = self.frames;
        let Some(shots) = self.shots.as_mut() else {
            return;
        };
        if let Some(k) = shots.open.take() {
            if let Some(shot) = shots.list.get_mut(k) {
                let first = shot
                    .get("first_frame")
                    .and_then(Value::as_u64)
                    .unwrap_or(at);
                shot.insert("frames".into(), Value::from(at.saturating_sub(first)));
            }
        }
        if let Some(mut facts) = begin {
            facts.insert("first_frame".into(), Value::from(at));
            facts.insert("frames".into(), Value::from(0));
            shots.list.push(facts);
            let k = shots.list.len() - 1;
            shots.open = Some(k);
            let waiting = std::mem::take(&mut shots.waiting);
            for (ordinal, facts) in waiting {
                shots.label(ordinal, facts);
            }
        }
    }

    fn lost(&mut self, frames: u64) {
        if frames == 0 {
            return;
        }
        self.dropped += frames;
        match self.gaps.last_mut() {
            Some((at, n)) if *at == self.frames => *n += frames,
            _ => self.gaps.push((self.frames, frames)),
        }
    }
}

struct ResultState {
    record_bytes: usize,
    repeating: bool,
}

struct Writer {
    status: Arc<RecordStatus>,
    options: RecordOptions,
    problem: ProblemFn,
    manifest: Map<String, Value>,
    log: Option<File>,
    streams: Vec<StreamState>,
    results: Vec<(String, ResultState)>,
    finished: bool,
    last_flush: Instant,
}

fn reason(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::PermissionDenied => format!("it cannot be written ({e})"),
        io::ErrorKind::NotFound => format!("it does not exist ({e})"),
        _ => e.to_string(),
    }
}

fn pretty(v: &Value) -> Vec<u8> {
    let mut out = serde_json::to_vec_pretty(v).unwrap_or_default();
    out.push(b'\n');
    out
}

impl Writer {
    fn pause(&self) {
        if !self.options.write_delay.is_zero() {
            std::thread::sleep(self.options.write_delay);
        }
    }

    fn open(begin: Begin, options: RecordOptions, problem: ProblemFn) -> Result<Writer, OpenError> {
        // The arm waits for these writes by design, so the pause a test asks for
        // (`write_delay`) starts after them: it is the run that must not wait.
        let folder =
            hertzforge_capture::claim_run_dir(&begin.root, &begin.name).map_err(|e| OpenError {
                path: begin.root.clone(),
                reason: reason(&e),
            })?;
        let small = |name: &str, v: &Value| -> Result<(), OpenError> {
            let path = folder.join(name);
            hertzforge_capture::write_atomic(&path, &pretty(v)).map_err(|e| OpenError {
                path,
                reason: reason(&e),
            })
        };
        small("table.json", &begin.table)?;
        small("setup.json", &begin.setup)?;
        if let Some((law, file)) = &begin.law {
            small("law/law.json", law)?;
            if let Some((name, bytes)) = file {
                let path = folder.join("law").join(name);
                hertzforge_capture::write_atomic(&path, bytes).map_err(|e| OpenError {
                    path,
                    reason: reason(&e),
                })?;
            }
        }
        let mut manifest = begin.manifest;
        manifest.insert("schema".into(), Value::from(SCHEMA));
        manifest.insert(
            "streams".into(),
            Value::Array(
                begin
                    .streams
                    .iter()
                    .map(|s| {
                        json!({"name": s.name, "kind": s.description.get("kind").cloned().unwrap_or(Value::Null),
                               "channels": s.description.get("channels").and_then(Value::as_array)
                                   .map(|cs| cs.iter().map(|c| c["index"].clone()).collect::<Vec<_>>())
                                   .unwrap_or_default()})
                    })
                    .collect(),
            ),
        );
        if !manifest.contains_key("results") {
            manifest.insert("results".into(), Value::Array(Vec::new()));
        }
        manifest.insert("complete".into(), Value::from(false));
        small("run.json", &Value::Object(manifest.clone()))?;
        let log_path = folder.join("log.jsonl");
        let mut log = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| OpenError {
                path: log_path.clone(),
                reason: reason(&e),
            })?;
        let mut lines = Vec::new();
        for entry in &begin.log {
            lines.extend(serde_json::to_vec(entry).unwrap_or_default());
            lines.push(b'\n');
        }
        log.write_all(&lines).map_err(|e| OpenError {
            path: log_path,
            reason: reason(&e),
        })?;
        let status = Arc::new(RecordStatus {
            folder: folder.clone(),
            problem: Mutex::new(None),
            finished: AtomicBool::new(false),
        });
        let delay = options.write_delay;
        let mut w = Writer {
            status,
            options: RecordOptions {
                write_delay: Duration::ZERO,
                ..options
            },
            problem,
            manifest,
            log: Some(log),
            streams: Vec::new(),
            results: Vec::new(),
            finished: false,
            last_flush: Instant::now(),
        };
        for mut spec in begin.streams {
            let width = spec
                .description
                .get("channels")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let shots = spec.description.remove("shots").map(|_| Shots::default());
            let mut s = StreamState {
                dir: folder.join("streams").join(&spec.name),
                name: spec.name,
                file: None,
                description: spec.description,
                frames: 0,
                width,
                gaps: Vec::new(),
                dropped: 0,
                fault: None,
                link: spec.link.returns(),
                bytes: Vec::new(),
                shots,
            };
            let made = std::fs::create_dir_all(&s.dir).and_then(|_| {
                OpenOptions::new()
                    .create_new(true)
                    .append(true)
                    .open(s.dir.join("telemetry.f32"))
            });
            match made {
                Ok(f) => s.file = Some(f),
                Err(e) => s.fault = Some(format!("its file could not be made: {}", reason(&e))),
            }
            w.streams.push(s);
            let n = w.streams.len() - 1;
            w.write_sidecar(n, false);
            if let Some(why) = w.streams[n].fault.clone() {
                let name = w.streams[n].name.clone();
                w.report(
                    format!("The {name} stream is not recorded: {why}. The run goes on."),
                    Some(&name),
                    &why,
                );
            }
        }
        w.options.write_delay = delay;
        Ok(w)
    }

    fn run(&mut self, rx: Receiver<Msg>) {
        loop {
            let wait = self
                .options
                .flush_interval
                .saturating_sub(self.last_flush.elapsed());
            match rx.recv_timeout(wait) {
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => self.flush(false),
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if self.last_flush.elapsed() >= self.options.flush_interval {
                self.flush(false);
            }
            if self.finished {
                break;
            }
        }
        if !self.finished {
            // Every handle is gone and the run did not end: keep what was written,
            // complete: false throughout.
            self.take_unsent();
            self.flush(false);
        }
        self.status.finished.store(true, Ordering::SeqCst);
    }

    fn report(&mut self, words: String, stream: Option<&str>, failed: &str) {
        if let Ok(mut p) = self.status.problem.lock() {
            *p = Some(words.clone());
        }
        (self.problem)(words, json!({"stream": stream, "failed": failed}));
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Log(entry) => self.append_log(&entry),
            Msg::Manifest(patch) => {
                for (k, v) in patch {
                    self.manifest.insert(k, v);
                }
                self.write_manifest();
            }
            Msg::Result {
                name,
                description,
                arrays,
                record,
            } => self.put_result(name, description, arrays, record),
            Msg::Block {
                stream,
                buf,
                frames,
                dropped_before,
            } => self.block(stream, buf, frames, dropped_before),
            Msg::StreamPatch { stream, patch } => {
                if let Some(s) = self.streams.get_mut(stream) {
                    for (k, v) in patch {
                        s.description.insert(k, v);
                    }
                    self.write_sidecar(stream, false);
                }
            }
            Msg::Shot {
                stream,
                begin,
                dropped_before,
            } => {
                let ended = begin.is_none();
                if let Some(s) = self.streams.get_mut(stream) {
                    s.shot(begin, dropped_before);
                    if ended {
                        self.write_sidecar(stream, false);
                    }
                }
            }
            Msg::ShotLabel {
                stream,
                ordinal,
                facts,
            } => {
                if let Some(shots) = self.streams.get_mut(stream).and_then(|s| s.shots.as_mut()) {
                    shots.label(ordinal, facts);
                    self.write_sidecar(stream, false);
                }
            }
            Msg::Finish(end) => self.finish(end),
        }
    }

    fn append_log(&mut self, entry: &Value) {
        let Some(f) = self.log.as_mut() else { return };
        let mut line = serde_json::to_vec(entry).unwrap_or_default();
        line.push(b'\n');
        if !self.options.write_delay.is_zero() {
            std::thread::sleep(self.options.write_delay);
        }
        if let Err(e) = f.write_all(&line) {
            self.log = None;
            let words = format!(
                "The run's log could not be written to {}: {}. The log goes on on screen.",
                self.status.folder.join("log.jsonl").display(),
                reason(&e)
            );
            self.report(words, None, "log");
        }
    }

    fn write_manifest(&mut self) {
        self.pause();
        let path = self.status.folder.join("run.json");
        if let Err(e) =
            hertzforge_capture::write_atomic(&path, &pretty(&Value::Object(self.manifest.clone())))
        {
            let words = format!("{} could not be rewritten: {}", path.display(), reason(&e));
            self.report(words, None, "run.json");
        }
    }

    fn write_sidecar(&mut self, stream: usize, complete: bool) {
        let Some(s) = self.streams.get(stream) else {
            return;
        };
        let path = s.dir.join("stream.json");
        let body = pretty(&s.sidecar(complete));
        self.pause();
        if let Err(e) = hertzforge_capture::write_atomic(&path, &body) {
            let name = s.name.clone();
            let words = format!("{} could not be written: {}", path.display(), reason(&e));
            self.report(words, Some(&name), "stream.json");
        }
    }

    fn block(&mut self, stream: usize, mut buf: Vec<f32>, frames: usize, dropped_before: u64) {
        let pause = self.options.write_delay;
        let Some(s) = self.streams.get_mut(stream) else {
            return;
        };
        s.lost(dropped_before);
        let mut failed = None;
        match s.file.as_mut() {
            Some(f) if s.fault.is_none() => {
                s.bytes.clear();
                for x in buf.iter().take(frames * s.width) {
                    s.bytes.extend_from_slice(&x.to_le_bytes());
                }
                if !pause.is_zero() {
                    std::thread::sleep(pause);
                }
                match f.write_all(&s.bytes) {
                    Ok(()) => s.frames += frames as u64,
                    Err(e) => failed = Some(reason(&e)),
                }
            }
            _ => s.lost(frames as u64),
        }
        buf.clear();
        s.link.give_back(buf);
        if let Some(why) = failed {
            self.fault_stream(stream, format!("a write failed: {why}"));
        }
    }

    /// Stop writing a stream, say why in its description, and tell the run.
    fn fault_stream(&mut self, stream: usize, why: String) {
        let Some(s) = self.streams.get_mut(stream) else {
            return;
        };
        if s.fault.is_some() {
            return;
        }
        s.fault = Some(why.clone());
        s.file = None;
        let name = s.name.clone();
        self.write_sidecar(stream, false);
        self.report(
            format!("Recording of the {name} stream stopped: {why}. The run goes on."),
            Some(&name),
            &why,
        );
    }

    fn put_result(
        &mut self,
        name: String,
        description: Map<String, Value>,
        arrays: Vec<ResultArray>,
        record: Option<Value>,
    ) {
        if let Err((path, e)) = self.try_result(&name, description, &arrays, record) {
            let words = format!(
                "The result {name} could not be written to {}: {}",
                path.display(),
                reason(&e)
            );
            self.report(words, None, &format!("results/{name}"));
        }
    }

    fn try_result(
        &mut self,
        name: &str,
        description: Map<String, Value>,
        arrays: &[ResultArray],
        record: Option<Value>,
    ) -> Result<(), (PathBuf, io::Error)> {
        let dir = self.status.folder.join("results");
        let mut layout = Vec::new();
        let mut offset = 0usize;
        for a in arrays {
            layout.push(
                json!({"name": a.name, "dtype": a.values.dtype(), "shape": a.shape,
                               "unit": a.unit, "offset": offset}),
            );
            offset += a.values.len() * a.values.item_bytes();
        }
        let mut bytes = Vec::with_capacity(offset);
        for a in arrays {
            a.values.put(&mut bytes);
        }
        let bin = dir.join(format!("{name}.bin"));
        let lines = dir.join(format!("{name}.jsonl"));
        if let Some((_, state)) = self.results.iter().find(|(n, _)| n == name) {
            if !state.repeating || record.is_none() || state.record_bytes != bytes.len() {
                return Err((
                    bin,
                    io::Error::other("a record does not have the shape of the result's first"),
                ));
            }
            self.pause();
            OpenOptions::new()
                .append(true)
                .open(&bin)
                .and_then(|mut f| f.write_all(&bytes))
                .map_err(|e| (bin.clone(), e))?;
            if let Some(r) = &record {
                self.append_line(&lines, r)?;
            }
            return Ok(());
        }
        let kind = description.get("kind").cloned().unwrap_or(Value::Null);
        let mut side = Map::new();
        side.insert("name".into(), Value::from(name));
        for (k, v) in description {
            side.insert(k, v);
        }
        side.insert("arrays".into(), Value::Array(layout));
        self.pause();
        if record.is_some() {
            std::fs::create_dir_all(&dir).map_err(|e| (dir.clone(), e))?;
            OpenOptions::new()
                .create_new(true)
                .append(true)
                .open(&bin)
                .and_then(|mut f| f.write_all(&bytes))
                .map_err(|e| (bin.clone(), e))?;
        } else {
            hertzforge_capture::write_atomic(&bin, &bytes).map_err(|e| (bin.clone(), e))?;
        }
        let side_path = dir.join(format!("{name}.json"));
        self.pause();
        hertzforge_capture::write_atomic(&side_path, &pretty(&Value::Object(side)))
            .map_err(|e| (side_path.clone(), e))?;
        if let Some(r) = &record {
            self.append_line(&lines, r)?;
        }
        self.results.push((
            name.to_owned(),
            ResultState {
                record_bytes: bytes.len(),
                repeating: record.is_some(),
            },
        ));
        if let Some(Value::Array(list)) = self.manifest.get_mut("results") {
            list.push(json!({"name": name, "kind": kind}));
        }
        self.write_manifest();
        Ok(())
    }

    fn append_line(&self, path: &Path, v: &Value) -> Result<(), (PathBuf, io::Error)> {
        self.pause();
        let mut line = serde_json::to_vec(v).unwrap_or_default();
        line.push(b'\n');
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| f.write_all(&line))
            .map_err(|e| (path.to_path_buf(), e))
    }

    /// Sync the log and the streams, rewrite the streams' descriptions, and check
    /// the free space.
    fn flush(&mut self, complete: bool) {
        self.last_flush = Instant::now();
        if let Some(f) = self.log.as_mut() {
            let _ = f.sync_data();
        }
        let mut written = 0u64;
        let mut syncs = Vec::new();
        for (k, s) in self.streams.iter_mut().enumerate() {
            written += s.frames * s.width as u64 * 4;
            if let Some(f) = s.file.as_mut() {
                if let Err(e) = f.sync_data() {
                    syncs.push((k, format!("a sync failed: {}", reason(&e))));
                }
            }
        }
        for (k, why) in syncs {
            self.fault_stream(k, why);
        }
        let floor = self.options.free_space_floor_bytes.saturating_add(written);
        if self
            .streams
            .iter()
            .any(|s| s.fault.is_none() && s.file.is_some())
        {
            if let Ok(free) = hertzforge_capture::free_space_bytes(&self.status.folder) {
                if free < floor {
                    for k in 0..self.streams.len() {
                        self.fault_stream(
                            k,
                            format!(
                                "the free space fell to {} MB, under the {} MB the run's files \
                                 need kept free",
                                free / 1_000_000,
                                floor / 1_000_000
                            ),
                        );
                    }
                }
            }
        }
        for k in 0..self.streams.len() {
            self.write_sidecar(k, complete);
        }
    }

    /// Drops the taps counted after their last block reached the writer: a gap at
    /// the end of each stream. Taken only once the run's blocks have all arrived,
    /// so the gap is where it happened.
    fn take_unsent(&mut self) {
        for s in self.streams.iter_mut() {
            let n = s.link.take_unsent();
            s.lost(n);
            // a shot still open when the run ended ends with what the file holds
            if s.shots.as_ref().is_some_and(|x| x.open.is_some()) {
                s.shot(None, 0);
            }
        }
    }

    fn finish(&mut self, mut end: Map<String, Value>) {
        self.take_unsent();
        self.flush(true);
        let streams: Map<String, Value> = self
            .streams
            .iter()
            .map(|s| {
                (
                    s.name.clone(),
                    json!({"frames": s.frames, "drops": s.dropped, "gaps": s.gaps.len(),
                           "fault": s.fault}),
                )
            })
            .collect();
        // A run whose streams are written elsewhere (the Recorder's recordings) states
        // them itself; the writer's own join them.
        match end.get_mut("streams") {
            Some(Value::Object(given)) => given.extend(streams),
            _ => {
                end.insert("streams".into(), Value::Object(streams));
            }
        }
        self.pause();
        let path = self.status.folder.join("end.json");
        match hertzforge_capture::write_atomic(&path, &pretty(&Value::Object(end))) {
            Ok(()) => {
                self.manifest.insert("complete".into(), Value::from(true));
                self.write_manifest();
            }
            Err(e) => {
                let words = format!(
                    "{} could not be written: {}; the run's folder says it did not end \
                     cleanly",
                    path.display(),
                    reason(&e)
                );
                self.report(words, None, "end.json");
            }
        }
        if let Some(f) = self.log.as_mut() {
            let _ = f.sync_data();
        }
        self.finished = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;

    fn scratch(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "run-record-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn read(p: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
    }

    /// A record with one two-channel stream, and the problems it reports.
    fn opened(
        root: &Path,
        options: RecordOptions,
    ) -> (RunRecord, tap::StreamTap, Receiver<(String, Value)>) {
        let (stream, link) = tap::pair(2, 64, 4);
        let mut d = Map::new();
        d.insert("kind".into(), json!("inputs"));
        d.insert("fs_hz".into(), json!(1024));
        d.insert(
            "channels".into(),
            json!([{"index": 1, "unit": "g"}, {"index": 2, "unit": "g"}]),
        );
        let mut manifest = Map::new();
        manifest.insert("test".into(), json!("random"));
        manifest.insert("source".into(), json!("SIM"));
        let (ptx, prx) = mpsc::channel();
        let ptx = Mutex::new(ptx);
        let record = RunRecord::open(
            Begin {
                root: root.to_path_buf(),
                name: "run".into(),
                manifest,
                table: json!({"rows": []}),
                setup: json!({"config": {}}),
                law: None,
                log: vec![json!({"seq": 0, "kind": "state", "text": "configured"})],
                streams: vec![StreamSpec {
                    name: "inputs".into(),
                    description: d,
                    link,
                }],
            },
            &options,
            Box::new(move |words, data| {
                let _ = ptx.lock().unwrap().send((words, data));
            }),
        )
        .unwrap();
        (record, stream, prx)
    }

    fn wait_finished(status: &RecordStatus) {
        let t0 = Instant::now();
        while !status.finished() {
            assert!(
                t0.elapsed() < Duration::from_secs(20),
                "the writer did not finish"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A run that stops without ending (a killed service drops every handle) leaves
    /// a folder a reader takes whole: `complete: false`, no `end.json`, and a stream
    /// whose description matches its file. Red if the writer wrote the end.
    #[test]
    fn a_run_cut_off_leaves_a_valid_folder_that_says_it_did_not_complete() {
        let root = scratch("cut");
        let (record, mut stream, _p) = opened(&root, RecordOptions::default());
        let status = record.status();
        let samples: Vec<f32> = (0..200).map(|k| k as f32).collect();
        stream.push(&samples, 100);
        record
            .log()
            .append(json!({"seq": 1, "kind": "level", "text": "level 0 dB"}));
        drop(stream);
        drop(record);
        wait_finished(&status);
        let folder = &status.folder;
        assert_eq!(read(&folder.join("run.json"))["complete"], false);
        assert!(!folder.join("end.json").exists());
        let side = read(&folder.join("streams/inputs/stream.json"));
        assert_eq!(side["frames"], 100);
        assert_eq!(side["complete"], false);
        let bytes = std::fs::read(folder.join("streams/inputs/telemetry.f32")).unwrap();
        assert_eq!(bytes.len(), 100 * 2 * 4);
        assert_eq!(f32::from_le_bytes(bytes[4..8].try_into().unwrap()), 1.0);
        let log = std::fs::read_to_string(folder.join("log.jsonl")).unwrap();
        assert_eq!(log.lines().count(), 2, "{log}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A clean end writes `end.json` with each stream's counts, then `complete`.
    #[test]
    fn finish_writes_the_end_then_says_complete() {
        let root = scratch("end");
        let (record, mut stream, _p) = opened(&root, RecordOptions::default());
        let status = record.status();
        stream.push(&[0.5; 20], 10);
        drop(stream);
        let mut end = Map::new();
        end.insert("end".into(), json!({"outcome": "completed"}));
        record.finish(end);
        wait_finished(&status);
        let folder = &status.folder;
        let e = read(&folder.join("end.json"));
        assert_eq!(e["end"]["outcome"], "completed");
        assert_eq!(e["streams"]["inputs"]["frames"], 10);
        assert_eq!(read(&folder.join("run.json"))["complete"], true);
        assert_eq!(
            read(&folder.join("streams/inputs/stream.json"))["complete"],
            true
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A stalled writer costs frames, never a wait: the tap drops what it cannot
    /// queue and the gap is kept where it happened. Red if `push` waited.
    #[test]
    fn a_tap_never_waits_for_a_stalled_writer_and_keeps_the_gap_in_place() {
        let root = scratch("stall");
        let options = RecordOptions {
            write_delay: Duration::from_millis(300),
            ..RecordOptions::default()
        };
        let (record, mut stream, _p) = opened(&root, options);
        let status = record.status();
        let block: Vec<f32> = vec![1.0; 128];
        let t0 = Instant::now();
        for _ in 0..100 {
            stream.push(&block, 64);
        }
        let pushed = t0.elapsed();
        assert!(
            pushed < Duration::from_millis(100),
            "push waited {pushed:?}"
        );
        drop(stream);
        record.finish(Map::new());
        wait_finished(&status);
        let side = read(&status.folder.join("streams/inputs/stream.json"));
        let frames = side["frames"].as_u64().unwrap();
        let lost: u64 = side["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["dropped_frames"].as_u64().unwrap())
            .sum();
        assert_eq!(frames + lost, 6400, "{side}");
        assert!(lost > 0, "{side}");
        // the four buffers made before the run reached the file, then the rest was lost
        assert_eq!(side["gaps"][0]["at_frame"], 4 * 64, "{side}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A stream of shots says where each shot is in its file and what it was: the
    /// marks travel in order with the blocks, so a shot's first frame and length
    /// are the file's own, and a label sent before its shot began still lands on
    /// it. Red if a mark overtook the frames before it (the second shot would
    /// begin at 0) or if an early label were lost.
    #[test]
    fn a_stream_of_shots_places_each_shot_in_its_file() {
        let root = scratch("shots");
        let (stream, link) = tap::pair(1, 8, 64);
        let mut d = Map::new();
        d.insert("kind".into(), json!("inputs"));
        d.insert("fs_hz".into(), json!(1000));
        d.insert("channels".into(), json!([{"index": 1, "unit": "g"}]));
        d.insert("shots".into(), json!([]));
        let mut manifest = Map::new();
        manifest.insert("test".into(), json!("classical_shock"));
        manifest.insert("source".into(), json!("SIM"));
        let record = RunRecord::open(
            Begin {
                root: root.clone(),
                name: "run".into(),
                manifest,
                table: json!({"rows": []}),
                setup: json!({"config": {}}),
                law: None,
                log: Vec::new(),
                streams: vec![StreamSpec {
                    name: "inputs".into(),
                    description: d,
                    link,
                }],
            },
            &RecordOptions::default(),
            Box::new(|_, _| {}),
        )
        .unwrap();
        let mut stream = stream;
        let status = record.status();
        let mut label = Map::new();
        label.insert("level_db".into(), json!(-6.0));
        label.insert("pre_frames".into(), json!(3));
        record.label_shot(0, 1, label);
        let mut first = Map::new();
        first.insert("first_station_frame".into(), json!(0));
        stream.begin_shot(first);
        stream.push(&[1.0; 5], 5);
        stream.end_shot();
        let mut second = Map::new();
        second.insert("first_station_frame".into(), json!(100));
        stream.begin_shot(second);
        stream.push(&[2.0; 7], 7);
        stream.end_shot();
        drop(stream);
        record.finish(Map::new());
        wait_finished(&status);
        let side = read(&status.folder.join("streams/inputs/stream.json"));
        assert_eq!(side["frames"], 12, "{side}");
        let shots = side["shots"].as_array().unwrap();
        assert_eq!(shots.len(), 2, "{side}");
        assert_eq!(shots[0]["first_frame"], 0);
        assert_eq!(shots[0]["frames"], 5);
        assert_eq!(shots[1]["first_frame"], 5);
        assert_eq!(shots[1]["frames"], 7);
        assert_eq!(shots[1]["level_db"], -6.0);
        assert_eq!(shots[1]["station_frame"], 103, "{side}");
        assert!(shots[0].get("station_frame").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Crossing the free-space floor stops the stream, says so in its description,
    /// and tells the run; the record goes on. Red if the fault were swallowed.
    #[test]
    fn crossing_the_free_space_floor_stops_the_stream_and_tells_the_run() {
        let root = scratch("floor");
        let free = hertzforge_capture::free_space_bytes(&root).unwrap();
        let options = RecordOptions {
            flush_interval: Duration::from_millis(20),
            // the floor is crossed once the stream holds more than about 1 MB
            free_space_floor_bytes: free.saturating_sub(1_000_000),
            ..RecordOptions::default()
        };
        let (record, mut stream, problems) = opened(&root, options);
        let status = record.status();
        let block: Vec<f32> = vec![0.25; 128];
        // about 5 MB, sent a buffer at a time
        for _ in 0..10_000 {
            stream.push(&block, 64);
            std::thread::sleep(Duration::from_micros(100));
        }
        drop(stream);
        let (words, data) = problems
            .recv_timeout(Duration::from_secs(10))
            .expect("the run was told");
        assert!(words.contains("free space"), "{words}");
        assert_eq!(data["stream"], "inputs");
        record
            .log()
            .append(json!({"seq": 1, "kind": "note", "text": "after"}));
        record.finish(Map::new());
        wait_finished(&status);
        let side = read(&status.folder.join("streams/inputs/stream.json"));
        assert!(
            side["fault"].as_str().unwrap().contains("free space"),
            "{side}"
        );
        assert_eq!(status.problem().as_deref(), Some(words.as_str()));
        let log = std::fs::read_to_string(status.folder.join("log.jsonl")).unwrap();
        assert!(log.contains("after"), "{log}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
