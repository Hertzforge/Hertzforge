//! The last input samples a MIMO Sine run read, for the `time` plot: a
//! station wrapper that copies each block it reads into a fixed ring, and
//! hands the ring to the run thread under a lock it only ever tries, so the
//! loop never waits and never allocates for it.

use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use std::sync::{Arc, Mutex};

/// The most frames the ring holds, per input.
pub const RING_FRAMES: usize = 8192;

/// The last frames read: every enabled input, interleaved, oldest first once
/// [`Ring::frames`] is called.
#[derive(Debug, Clone, Default)]
pub struct Ring {
    width: usize,
    data: Vec<f32>,
    head: usize,
    filled: usize,
}

impl Ring {
    fn push(&mut self, block: &[f32], width: usize) {
        if width == 0 {
            return;
        }
        if self.width != width || self.data.len() != width * RING_FRAMES {
            // Sized once, before the first block: the station's width does not
            // change during a run.
            self.width = width;
            self.data = vec![0.0; width * RING_FRAMES];
            self.head = 0;
            self.filled = 0;
        }
        for frame in block.chunks_exact(width) {
            let at = self.head * width;
            self.data[at..at + width].copy_from_slice(frame);
            self.head = (self.head + 1) % RING_FRAMES;
            self.filled = (self.filled + 1).min(RING_FRAMES);
        }
    }

    /// The last `n` frames of input `slot`, oldest first.
    pub fn last(&self, slot: usize, n: usize) -> Vec<f32> {
        if self.width == 0 || slot >= self.width {
            return Vec::new();
        }
        let n = n.min(self.filled);
        (0..n)
            .map(|k| {
                let i = (self.head + RING_FRAMES - n + k) % RING_FRAMES;
                self.data[i * self.width + slot]
            })
            .collect()
    }
}

/// The run thread's side.
pub type Shared = Arc<Mutex<Ring>>;

/// A station whose reads are copied into `ring`.
pub struct TimeTap {
    inner: Box<dyn Backend>,
    ring: Shared,
    width: usize,
    local: Ring,
}

impl TimeTap {
    pub fn wrap(inner: Box<dyn Backend>, ring: Shared, width: usize) -> Box<dyn Backend> {
        let mut local = Ring::default();
        local.push(&[], width);
        Box::new(TimeTap {
            inner,
            ring,
            width,
            local,
        })
    }
}

impl Backend for TimeTap {
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
            let w = self.width;
            self.local.push(&out[..(n * w).min(out.len())], w);
            // Never waits: a block the run thread is reading is skipped.
            if let Ok(mut g) = self.ring.try_lock() {
                g.clone_from(&self.local);
            }
        }
        Ok(r)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.inner.write_drive(drive)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_sine_the_ring_keeps_the_last_frames_of_each_input_oldest_first() {
        let mut r = Ring::default();
        let block: Vec<f32> = (0..10).flat_map(|k| [k as f32, -(k as f32)]).collect();
        r.push(&block, 2);
        assert_eq!(r.last(0, 3), vec![7.0, 8.0, 9.0]);
        assert_eq!(r.last(1, 2), vec![-8.0, -9.0]);
        assert_eq!(r.last(0, 100).len(), 10);
    }
}
