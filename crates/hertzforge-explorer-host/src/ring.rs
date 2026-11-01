//! The bounded acquisition ring that gets the ANALYSIS off the RT tick.
//!
//! # Why this exists
//!
//! `analysis.rs` is reusable verbatim, and its reuse came with one condition
//! attached: its FFT runs synchronously in whatever thread pushes samples. On the delegated
//! path that thread is a telemetry drain and the cost is nobody's deadline. On a
//! host-closes-loop station the pushing thread would be [`hertzforge_engine::ProcessStep`],
//! i.e. the tick that also has to produce the next block of drive. The measured display
//! FFT is ~24.4 ms; the block period on the chassis is ~15 ms. Running one inside the
//! other is not a performance concern, it is a `SchedulingFault` and an
//! `emergency_stop` on every display update.
//!
//! So the process step does the cheapest possible thing with its acquired block - copies
//! it into this ring - and the pump thread drains the ring into the spectrum / derived /
//! profile engines BETWEEN tick batches, where a 24 ms pass costs latency and nothing
//! else.
//!
//! # The three properties the RT side needs
//!
//! 1. **Never blocks.** The lock is held for a `Vec` swap and a `memcpy`, never across
//!    any analysis. The only contender is the pump thread's drain, which holds it for a
//!    `VecDeque` swap.
//! 2. **Never allocates in steady state.** [`ProcessStep`](hertzforge_engine::ProcessStep)'s
//!    contract. Buffers are pre-allocated at construction and RECYCLED: the consumer
//!    hands them back after it has read them, so the steady state is a fixed set of
//!    `Vec<f32>`s circulating between the two sides.
//! 3. **Drops the OLDEST, and counts what it dropped.** A full ring means the pump fell
//!    behind. Dropping the NEWEST would hold a stale window on the display while live
//!    data went in the bin; dropping the oldest keeps the display current. Either way
//!    the count is published ([`SampleRing::dropped`]) rather than absorbed, because a
//!    display that has silently lost blocks looks exactly like one that has not.
//!
//! The ring is a DISPLAY/ANALYSIS tap, not a safety tap. The engine's own
//! [`AbortWatchdog`](hertzforge_engine::AbortWatchdog) reads the raw ring inside the tick and
//! is the authoritative limit monitor; see [`crate::service`] for the statement of which
//! monitor owns which decision.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// A bounded, drop-oldest, recycling ring of acquired blocks.
#[derive(Debug)]
pub struct SampleRing {
    inner: Mutex<Inner>,
    dropped: AtomicU64,
    stride: usize,
}

#[derive(Debug)]
struct Inner {
    /// Filled blocks, oldest first.
    queue: VecDeque<Vec<f32>>,
    /// Buffers the consumer has handed back, ready to be filled again.
    pool: Vec<Vec<f32>>,
    /// Maximum number of filled blocks held before the oldest is dropped.
    capacity: usize,
}

impl SampleRing {
    /// A ring holding at most `capacity` blocks of `block_frames` frames at `stride`
    /// interleaved channels.
    ///
    /// Both the queue and the recycling pool are sized at construction: `capacity + 2`
    /// buffers exist from the start (`capacity` in flight plus one being filled and one
    /// being read), so the steady state never reaches the allocator.
    pub fn new(capacity: usize, block_frames: usize, stride: usize) -> Self {
        let capacity = capacity.max(1);
        let samples = block_frames.max(1) * stride.max(1);
        let pool = (0..capacity + 2)
            .map(|_| Vec::with_capacity(samples))
            .collect();
        Self {
            inner: Mutex::new(Inner {
                queue: VecDeque::with_capacity(capacity + 1),
                pool,
                capacity,
            }),
            dropped: AtomicU64::new(0),
            stride: stride.max(1),
        }
    }

    /// The interleave width the blocks in this ring carry.
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// Blocks dropped because the consumer fell behind, since construction.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// PRODUCER side (the RT tick): copy one interleaved block into the ring.
    ///
    /// A poisoned lock is treated as "the consumer died": the block is discarded and
    /// counted as dropped. That is deliberately not a panic - this runs on the tick that
    /// owns the drive, and a dead display must not take the drive path with it.
    pub fn push(&self, samples: &[f32]) {
        let Ok(mut inner) = self.inner.lock() else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if inner.queue.len() >= inner.capacity {
            if let Some(old) = inner.queue.pop_front() {
                inner.pool.push(old);
            }
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        let mut buf = inner.pool.pop().unwrap_or_default();
        buf.clear();
        buf.extend_from_slice(samples);
        inner.queue.push_back(buf);
    }

    /// CONSUMER side: move every filled block out of the ring into `out`, appending in
    /// acquisition order. The lock is released before the caller touches the data.
    ///
    /// The caller MUST hand the buffers back with [`SampleRing::recycle`] when it is
    /// done, or the producer falls back to allocating on the tick.
    pub fn drain_into(&self, out: &mut Vec<Vec<f32>>) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        while let Some(buf) = inner.queue.pop_front() {
            out.push(buf);
        }
    }

    /// CONSUMER side: return drained buffers for re-use, emptying `used`.
    pub fn recycle(&self, used: &mut Vec<Vec<f32>>) {
        let Ok(mut inner) = self.inner.lock() else {
            used.clear();
            return;
        };
        inner.pool.append(used);
    }

    /// Discard everything held (a run stop / re-arm), recycling the buffers. Does NOT
    /// clear the drop counter: it is a per-run total.
    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            while let Some(buf) = inner.queue.pop_front() {
                inner.pool.push(buf);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_come_back_in_acquisition_order() {
        let ring = SampleRing::new(4, 2, 1);
        ring.push(&[1.0, 2.0]);
        ring.push(&[3.0, 4.0]);
        let mut out = Vec::new();
        ring.drain_into(&mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], vec![1.0, 2.0]);
        assert_eq!(out[1], vec![3.0, 4.0]);
        assert_eq!(ring.dropped(), 0);
    }

    /// The OLDEST goes, the newest survives, and the loss is COUNTED. A ring that
    /// silently absorbed the overflow would make a stalled pump indistinguishable from
    /// a healthy one.
    #[test]
    fn overflow_drops_the_oldest_and_counts_it() {
        let ring = SampleRing::new(2, 1, 1);
        for v in 0..5 {
            ring.push(&[v as f32]);
        }
        assert_eq!(ring.dropped(), 3);
        let mut out = Vec::new();
        ring.drain_into(&mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], vec![3.0]);
        assert_eq!(out[1], vec![4.0]);
    }

    /// The steady state must not reach the allocator. Recycled buffers keep their
    /// capacity, so a push into a recycled buffer never grows it.
    #[test]
    fn recycled_buffers_keep_their_capacity() {
        let ring = SampleRing::new(2, 64, 2);
        ring.push(&vec![0.5f32; 128]);
        let mut out = Vec::new();
        ring.drain_into(&mut out);
        let cap_before = out[0].capacity();
        assert!(cap_before >= 128);
        ring.recycle(&mut out);
        assert!(out.is_empty());
        ring.push(&vec![0.25f32; 128]);
        let mut again = Vec::new();
        ring.drain_into(&mut again);
        assert_eq!(again[0].capacity(), cap_before);
    }
}
