//! The streaming source: a reader thread fills a bounded ring from `drive.bin`, and the
//! loop thread pops it without waiting.
//!
//! The file holds `f64` samples; the drive path is `f32`, so the reader rounds each
//! sample to `f32` as it pushes it, and nothing before that point rounds.
//!
//! The ring is the engine's lock-free single-producer, single-consumer ring
//! ([`hertzforge_engine::ring`]), made before the run and never grown. The reader
//! thread is the only one that touches the file. It pushes a frame only when the ring
//! has room for it (it counts what it pushed against what the loop popped), so no
//! frame is ever dropped; when the ring is full it sleeps a millisecond and looks
//! again. The loop thread's side ([`RingReader`]) pops one frame at a time and never
//! waits: an empty ring before the file's end is an underrun, which the run treats as
//! an abort ([`crate::step`]).
//!
//! A failure on the reader thread (a read error, a file shorter than its sidecar, a
//! panic) is never a stall: it sets [`SourceShared::failed`], which the loop thread
//! reads on every block and answers with an abort to zero and the emergency stop.

use hertzforge_engine::ring::{self, Consumer};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Frames the reader reads from the file at a time.
const READ_FRAMES: usize = 4096;

/// What the reader thread and the loop share.
#[derive(Debug, Default)]
pub struct SourceShared {
    /// Frames pushed into the ring.
    pub pushed: AtomicU64,
    /// Frames the loop has popped.
    pub popped: AtomicU64,
    /// Every frame of the file has been pushed.
    pub eof: AtomicBool,
    /// The reader failed; [`SourceShared::failure`] says why.
    pub failed: AtomicBool,
    failure: Mutex<Option<String>>,
    /// Asks the reader to stop.
    pub cancel: AtomicBool,
}

impl SourceShared {
    pub fn failure(&self) -> Option<String> {
        match self.failure.lock() {
            Ok(f) => f.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    fn fail(&self, why: String) {
        match self.failure.lock() {
            Ok(mut f) => *f = Some(why),
            Err(p) => *p.into_inner() = Some(why),
        }
        self.failed.store(true, Ordering::Release);
    }
}

/// The reader thread's handle.
pub struct DriveSource {
    pub shared: Arc<SourceShared>,
    pub samples: u64,
    pub capacity_frames: usize,
    thread: Option<JoinHandle<()>>,
}

/// The loop thread's side of the ring. Pops never wait.
pub struct RingReader {
    consumer: Consumer,
    shared: Arc<SourceShared>,
    pending: u64,
}

impl RingReader {
    /// Pop one frame into `out` (one sample per file channel). `false` when the ring
    /// is empty.
    pub fn pop(&mut self, out: &mut [f32]) -> bool {
        let ok = self.consumer.pop(out);
        if ok {
            self.pending += 1;
        }
        ok
    }

    /// Tell the reader how many frames were taken since the last call (once a block).
    pub fn release(&mut self) {
        if self.pending > 0 {
            self.shared
                .popped
                .fetch_add(self.pending, Ordering::Release);
            self.pending = 0;
        }
    }

    pub fn shared(&self) -> &Arc<SourceShared> {
        &self.shared
    }
}

impl DriveSource {
    /// Start the reader on `reader` (positioned at the first sample): `samples` frames of
    /// `channels` little-endian `f64`s, into a ring of `capacity_frames` (as `f32`).
    pub fn spawn<R: Read + Send + 'static>(
        reader: R,
        channels: usize,
        samples: u64,
        capacity_frames: usize,
    ) -> std::io::Result<(DriveSource, RingReader)> {
        let capacity_frames = capacity_frames.max(1);
        let (producer, consumer) = ring::channel(capacity_frames, channels);
        let capacity = producer.capacity() as u64;
        let shared = Arc::new(SourceShared::default());
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("playback-reader".into())
            .spawn(move || {
                let s2 = s.clone();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                    read_all(reader, channels, samples, capacity, &producer, &s2)
                }));
                match r {
                    Ok(Ok(())) => {}
                    Ok(Err(why)) => s.fail(why),
                    Err(p) => {
                        let why = p
                            .downcast_ref::<&str>()
                            .map(|x| (*x).to_owned())
                            .or_else(|| p.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "the reader thread panicked".into());
                        s.fail(format!("the reader thread failed: {why}"));
                    }
                }
            })?;
        Ok((
            DriveSource {
                shared: shared.clone(),
                samples,
                capacity_frames: capacity as usize,
                thread: Some(thread),
            },
            RingReader {
                consumer,
                shared,
                pending: 0,
            },
        ))
    }

    /// Wait until the ring holds `frames` (or the whole file), the reader fails, or
    /// `wait` passes. Called before the station is opened, never on the loop thread.
    pub fn wait_prefill(&self, frames: u64, wait: Duration) -> Result<(), String> {
        let want = frames.min(self.samples);
        let began = Instant::now();
        loop {
            if self.shared.failed.load(Ordering::Acquire) {
                return Err(self
                    .shared
                    .failure()
                    .unwrap_or_else(|| "the reader failed".into()));
            }
            if self.shared.pushed.load(Ordering::Acquire) >= want {
                return Ok(());
            }
            if began.elapsed() > wait {
                return Err(format!(
                    "the reader filled {} of the {want} frames it reads ahead in {:.1} s",
                    self.shared.pushed.load(Ordering::Acquire),
                    wait.as_secs_f64()
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// How long a dropped source waits for its reader to see the cancel before leaving it.
const READER_JOIN_WAIT: Duration = Duration::from_secs(1);

impl Drop for DriveSource {
    /// Cancels the reader and waits a bounded time for it. A reader held inside one
    /// read (a stalled disk or share) is left to finish that read and exit on its own:
    /// the run it fed has ended, the drive is already zero, and its end must not wait
    /// on the disk.
    fn drop(&mut self) {
        self.shared.cancel.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let began = Instant::now();
            while !t.is_finished() && began.elapsed() < READER_JOIN_WAIT {
                std::thread::sleep(Duration::from_millis(2));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

fn read_all<R: Read>(
    mut reader: R,
    channels: usize,
    samples: u64,
    capacity: u64,
    producer: &ring::Producer,
    s: &SourceShared,
) -> Result<(), String> {
    let width = channels.max(1);
    let sample = crate::file::SAMPLE_BYTES as usize;
    let mut bytes = vec![0u8; READ_FRAMES * width * sample];
    let mut frame = vec![0f32; width];
    let mut at: u64 = 0;
    while at < samples {
        let n = ((samples - at) as usize).min(READ_FRAMES);
        let piece = &mut bytes[..n * width * sample];
        // One read at a time, not `read_exact`: a slow source fills a piece over many
        // reads, and a cancelled run must not wait for the whole piece.
        let mut filled = 0;
        while filled < piece.len() {
            if s.cancel.load(Ordering::Acquire) {
                return Ok(());
            }
            match reader.read(&mut piece[filled..]) {
                Ok(0) => {
                    return Err(format!(
                        "drive.bin ended before sample {samples}, near sample {at}"
                    ))
                }
                Ok(k) => filled += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("drive.bin could not be read near sample {at}: {e}")),
            }
        }
        for f in piece.chunks_exact(width * sample) {
            for (k, b) in f.chunks_exact(sample).enumerate() {
                // The drive path is f32: the one rounding, at the source's output.
                frame[k] =
                    f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32;
            }
            // Room for one more frame: what was pushed less what the loop took.
            while at - s.popped.load(Ordering::Acquire) >= capacity {
                if s.cancel.load(Ordering::Acquire) {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            if !producer.push(&frame) {
                return Err(format!(
                    "the read-ahead ring refused sample {at} though it had room"
                ));
            }
            at += 1;
            s.pushed.store(at, Ordering::Release);
        }
        if s.cancel.load(Ordering::Acquire) {
            return Ok(());
        }
    }
    s.eof.store(true, Ordering::Release);
    Ok(())
}
