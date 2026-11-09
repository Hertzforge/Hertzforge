//! The record thread: the one thread that touches a recording's files.
//!
//! The thread that reads the station never starts, writes, finishes or waits on a
//! recording. It hands each batch to this thread through a queue that holds a stated
//! number of frames (2 s of the stream in the service) and goes back to the station.
//! Starting a recording (a folder, a sidecar, a free-space check, a writer thread) and
//! finishing one (draining its writer, the final sidecars) take as long as the disk
//! takes, here, while acquisition goes on.
//!
//! What the reading side needs to know comes back without a wait: the frames on disk,
//! the store's fault in its own words, the frames still queued, and each finished
//! recording's summary, read from shared cells.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::recording::{GapRecord, RecordingSummary};
use crate::store::{RecordStore, RecordingSpec};

/// What the record thread is asked to do, in order.
enum Cmd {
    Start(Box<RecordingSpec>),
    Frames(Vec<f32>, usize),
    Gap(GapRecord),
    Marks(Vec<serde_json::Value>),
    Finish,
    Quit,
}

/// A finished recording, as the record thread reports it.
#[derive(Debug, Clone)]
pub struct Finished {
    /// The name the service gave the recording.
    pub name: String,
    /// What the store wrote, or why it could not finish.
    pub result: Result<RecordingSummary, String>,
}

#[derive(Default)]
struct Shared {
    /// Frames handed over and not yet given to the store.
    queued: AtomicU64,
    /// Commands sent and not yet done.
    pending: AtomicU64,
    /// The store's frames on disk after its last call.
    on_disk: AtomicU64,
    /// The store's fault after its last call, or a start that failed.
    fault: Mutex<Option<String>>,
    /// Recordings finished and not yet collected.
    finished: Mutex<VecDeque<Finished>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// The record thread and the queue to it.
pub struct RecordThread {
    tx: Sender<Cmd>,
    shared: Arc<Shared>,
    join: Option<JoinHandle<Box<dyn RecordStore>>>,
    capacity_frames: u64,
}

impl RecordThread {
    /// Start the thread with `store`, behind a queue of `capacity_frames` frames.
    pub fn spawn(store: Box<dyn RecordStore>, capacity_frames: u64) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        let join = std::thread::Builder::new()
            .name("hertzforge-recorder-record".into())
            .spawn(move || run(store, rx, thread_shared))?;
        Ok(Self {
            tx,
            shared,
            join: Some(join),
            capacity_frames,
        })
    }

    fn send(&self, cmd: Cmd) {
        self.shared.pending.fetch_add(1, Ordering::SeqCst);
        if self.tx.send(cmd).is_err() {
            self.shared.pending.fetch_sub(1, Ordering::SeqCst);
            *lock(&self.shared.fault) = Some("the record thread has stopped".into());
        }
    }

    /// Begin a recording. Returns at once.
    pub fn start(&self, spec: RecordingSpec) {
        self.send(Cmd::Start(Box::new(spec)));
    }

    /// Hand frames to the recording in progress, or answer `false` when the queue
    /// cannot take them: the disk is that far behind. Never waits.
    pub fn push(&self, frames: &[f32], n_frames: usize) -> bool {
        let queued = self.shared.queued.load(Ordering::SeqCst);
        if queued + n_frames as u64 > self.capacity_frames {
            return false;
        }
        self.shared
            .queued
            .fetch_add(n_frames as u64, Ordering::SeqCst);
        self.send(Cmd::Frames(frames.to_vec(), n_frames));
        true
    }

    /// Note frames the recording in progress never received. Returns at once.
    pub fn gap(&self, gap: GapRecord) {
        self.send(Cmd::Gap(gap));
    }

    /// Note marks inside the recording in progress. Returns at once.
    pub fn marks(&self, marks: Vec<serde_json::Value>) {
        if !marks.is_empty() {
            self.send(Cmd::Marks(marks));
        }
    }

    /// Finish the recording in progress. Returns at once; its summary comes back
    /// through [`RecordThread::take_finished`].
    pub fn finish(&self) {
        self.send(Cmd::Finish);
    }

    /// The recordings finished since the last call.
    pub fn take_finished(&self) -> Vec<Finished> {
        lock(&self.shared.finished).drain(..).collect()
    }

    /// Frames of the recording in progress on disk.
    pub fn frames_on_disk(&self) -> u64 {
        self.shared.on_disk.load(Ordering::SeqCst)
    }

    /// Frames handed over and not yet given to the store.
    pub fn queued_frames(&self) -> u64 {
        self.shared.queued.load(Ordering::SeqCst)
    }

    /// The queue's depth, frames.
    pub fn capacity_frames(&self) -> u64 {
        self.capacity_frames
    }

    /// The store's fault in its own words, if writing has failed.
    pub fn fault(&self) -> Option<String> {
        lock(&self.shared.fault).clone()
    }

    /// Whether every command sent has been done.
    pub fn idle(&self) -> bool {
        self.shared.pending.load(Ordering::SeqCst) == 0
    }

    /// Finish everything queued, stop the thread, and give back the store and the
    /// recordings finished and not yet collected. Waits for the disk: never called on
    /// the thread that reads the station while it reads.
    pub fn close(mut self) -> (Option<Box<dyn RecordStore>>, Vec<Finished>) {
        let _ = self.tx.send(Cmd::Quit);
        let store = self.join.take().and_then(|j| j.join().ok());
        (store, self.take_finished())
    }
}

impl Drop for RecordThread {
    fn drop(&mut self) {
        if let Some(j) = self.join.take() {
            let _ = self.tx.send(Cmd::Quit);
            let _ = j.join();
        }
    }
}

/// How often an idle record thread reads the store's frames on disk and fault again:
/// the store's own writer moves them between commands.
const REFRESH: Duration = Duration::from_millis(20);

fn refresh(store: &dyn RecordStore, shared: &Shared) {
    shared
        .on_disk
        .store(store.frames_on_disk(), Ordering::SeqCst);
    if let Some(f) = store.fault() {
        *lock(&shared.fault) = Some(f);
    }
}

fn run(
    mut store: Box<dyn RecordStore>,
    rx: Receiver<Cmd>,
    shared: Arc<Shared>,
) -> Box<dyn RecordStore> {
    let mut name: Option<String> = None;
    loop {
        let cmd = match rx.recv_timeout(REFRESH) {
            Ok(cmd) => cmd,
            Err(RecvTimeoutError::Timeout) => {
                refresh(store.as_ref(), &shared);
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match cmd {
            Cmd::Start(spec) => {
                let asked = spec.name.clone();
                match store.start(*spec) {
                    Ok(_) => name = Some(asked),
                    Err(e) => {
                        *lock(&shared.fault) = Some(format!("could not begin {asked}: {e}"));
                        name = None;
                    }
                }
            }
            Cmd::Frames(frames, n) => {
                if name.is_some() {
                    store.push(&frames, n);
                }
                shared.queued.fetch_sub(n as u64, Ordering::SeqCst);
            }
            Cmd::Gap(gap) => {
                if name.is_some() {
                    store.gap(gap);
                }
            }
            Cmd::Marks(marks) => {
                if name.is_some() {
                    store.marks(marks);
                }
            }
            Cmd::Finish => {
                if let Some(asked) = name.take() {
                    let result = store.finish();
                    lock(&shared.finished).push_back(Finished {
                        name: asked,
                        result,
                    });
                }
            }
            Cmd::Quit => {
                shared.pending.fetch_sub(1, Ordering::SeqCst);
                break;
            }
        }
        refresh(store.as_ref(), &shared);
        shared.pending.fetch_sub(1, Ordering::SeqCst);
    }
    store
}
