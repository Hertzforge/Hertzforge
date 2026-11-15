//! The streams' side on the engine's thread: [`StreamTap`] hands blocks of samples to
//! the record's writer, and [`Recording`] is a station backend that feeds two taps,
//! the inputs as read and the drive as the device accepted it.
//!
//! This code runs inside every engine block, so it does no file work, takes no lock
//! and never waits. Its buffers are made before the run ([`pair`]); a block is copied
//! into a free one and sent. When none is free (the writer has fallen behind) or the
//! writer is gone, the block is dropped and its frames counted; the next block that
//! reaches the writer carries the count, so the gap is kept where it happened. A tap
//! that has not been attached (the record has no streams) records nothing.
//!
//! [`Recording`] only watches: every call goes to the station unchanged and its
//! answer comes back unchanged, and no recording outcome can fail a call. Any other
//! module's backend wrapper can feed the same taps.

use super::Msg;
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, OnceLock};

/// The record's side of a tap: where the tap sends, how buffers come back, and the
/// frames dropped since the last block the writer received.
#[derive(Clone)]
pub struct TapLink {
    sink: Arc<OnceLock<(Sender<Msg>, usize)>>,
    free: SyncSender<Vec<f32>>,
    unsent: Arc<AtomicU64>,
}

impl TapLink {
    /// Start feeding the writer: blocks go to `tx` as stream `id`.
    pub(super) fn attach(&self, tx: Sender<Msg>, id: usize) {
        let _ = self.sink.set((tx, id));
    }

    /// The writer's side: buffers back, and the frames dropped. It holds no way to
    /// send to the writer, so the writer ends when the run's handles are gone.
    pub(super) fn returns(&self) -> Returns {
        Returns {
            free: self.free.clone(),
            unsent: self.unsent.clone(),
        }
    }
}

/// How the writer returns a tap's buffers and learns the frames it dropped.
pub(super) struct Returns {
    free: SyncSender<Vec<f32>>,
    unsent: Arc<AtomicU64>,
}

impl Returns {
    /// Return a buffer to the tap.
    pub(super) fn give_back(&self, buf: Vec<f32>) {
        let _ = self.free.try_send(buf);
    }

    /// The frames dropped that no block has carried to the writer yet.
    pub(super) fn take_unsent(&self) -> u64 {
        self.unsent.swap(0, Ordering::Relaxed)
    }
}

/// The engine's side of a stream. Frames gather in one buffer until it is full,
/// then go to the writer whole; what is left is sent when the tap is dropped (the
/// station is released at the end of the run).
pub struct StreamTap {
    link: TapLink,
    free: Receiver<Vec<f32>>,
    width: usize,
    block_frames: usize,
    /// The buffer being filled, and the frames dropped before its first frame.
    current: Option<(Vec<f32>, u64)>,
}

/// A tap for frames `width` samples wide, with `buffers` buffers of
/// `block_frames` frames each, all made now.
pub fn pair(width: usize, block_frames: usize, buffers: usize) -> (StreamTap, TapLink) {
    let block_frames = block_frames.max(1);
    let buffers = buffers.max(1);
    let (free_tx, free_rx) = mpsc::sync_channel(buffers);
    for _ in 0..buffers {
        let _ = free_tx.try_send(Vec::with_capacity(block_frames * width.max(1)));
    }
    let link = TapLink {
        sink: Arc::new(OnceLock::new()),
        free: free_tx,
        unsent: Arc::new(AtomicU64::new(0)),
    };
    (
        StreamTap {
            link: link.clone(),
            free: free_rx,
            width,
            block_frames,
            current: None,
        },
        link,
    )
}

impl StreamTap {
    /// Hand `frames` frames of `samples` (frame-interleaved, `width` wide) to the
    /// writer, or count them dropped. Never blocks.
    pub fn push(&mut self, samples: &[f32], frames: usize) {
        if self.width == 0 || self.link.sink.get().is_none() {
            return;
        }
        let w = self.width;
        let frames = frames.min(samples.len() / w);
        let mut at = 0;
        while at < frames {
            if self.current.is_none() {
                match self.free.try_recv() {
                    Ok(mut buf) => {
                        buf.clear();
                        // the frames dropped before this buffer's first frame
                        let before = self.link.unsent.swap(0, Ordering::Relaxed);
                        self.current = Some((buf, before));
                    }
                    Err(_) => {
                        // the writer is behind: these frames are lost, and counted
                        self.link
                            .unsent
                            .fetch_add((frames - at) as u64, Ordering::Relaxed);
                        return;
                    }
                }
            }
            let Some((buf, _)) = self.current.as_mut() else {
                return;
            };
            let room = self.block_frames - buf.len() / w;
            let n = room.min(frames - at);
            buf.extend_from_slice(&samples[at * w..(at + n) * w]);
            at += n;
            if buf.len() / w >= self.block_frames {
                self.send();
            }
        }
    }

    /// Mark a shot beginning at the next frame pushed, with `facts` for its
    /// members (a stream of shots, `docs/run-folder.md`): what was pushed before
    /// it goes to the writer first. Never blocks; nothing when not attached.
    pub fn begin_shot(&mut self, facts: serde_json::Map<String, serde_json::Value>) {
        self.shot(Some(facts));
    }

    /// Mark the end of the shot open: its frames are those pushed since it began.
    pub fn end_shot(&mut self) {
        self.shot(None);
    }

    fn shot(&mut self, begin: Option<serde_json::Map<String, serde_json::Value>>) {
        if self.width == 0 || self.link.sink.get().is_none() {
            return;
        }
        self.send();
        let Some((tx, id)) = self.link.sink.get() else {
            return;
        };
        let dropped_before = self.link.unsent.swap(0, Ordering::Relaxed);
        let mark = Msg::Shot {
            stream: *id,
            begin,
            dropped_before,
        };
        if tx.send(mark).is_err() {
            self.link
                .unsent
                .fetch_add(dropped_before, Ordering::Relaxed);
        }
    }

    /// Send the buffer being filled, if it holds any frames.
    fn send(&mut self) {
        let Some((buf, before)) = self.current.take() else {
            return;
        };
        let Some((tx, id)) = self.link.sink.get() else {
            return;
        };
        let frames = buf.len() / self.width.max(1);
        if frames == 0 && before == 0 {
            let _ = self.link.free.try_send(buf);
            return;
        }
        let block = Msg::Block {
            stream: *id,
            buf,
            frames,
            dropped_before: before,
        };
        if tx.send(block).is_err() {
            self.link
                .unsent
                .fetch_add(before + frames as u64, Ordering::Relaxed);
        }
    }
}

impl Drop for StreamTap {
    fn drop(&mut self) {
        self.send();
    }
}

/// A station backend that records what passes through it: the inputs as read and
/// the drive as the device accepted it, and the count of input frames read since
/// the station started (the station frame).
pub struct Recording {
    inner: Box<dyn Backend>,
    inputs: StreamTap,
    drives: StreamTap,
    frames: u64,
    station_frame: Arc<AtomicU64>,
}

impl Recording {
    /// Wrap `inner`. `station_frame` is set to the frames read so far after every
    /// read.
    pub fn wrap(
        inner: Box<dyn Backend>,
        inputs: StreamTap,
        drives: StreamTap,
        station_frame: Arc<AtomicU64>,
    ) -> Box<dyn Backend> {
        Box::new(Recording {
            inner,
            inputs,
            drives,
            frames: 0,
            station_frame,
        })
    }
}

impl Backend for Recording {
    fn caps(&self) -> Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
    }
    fn take_warnings(&mut self) -> Vec<String> {
        self.inner.take_warnings()
    }
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        self.inner.configure(table, acq)
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let r = self.inner.read_input(out)?;
        if let InputRead::Frames(n) = r {
            self.frames += n as u64;
            self.station_frame.store(self.frames, Ordering::Relaxed);
            self.inputs.push(out, n);
        }
        Ok(r)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let n = self.inner.write_drive(drive)?;
        self.drives.push(drive, n);
        Ok(n)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.inner.output_space()
    }
    fn input_overloads(&mut self) -> Vec<bool> {
        self.inner.input_overloads()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.inner.emergency_stop()
    }
}
