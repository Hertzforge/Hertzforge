//! Lock-free single-producer / single-consumer ring buffer for sample frames.
//!
//! Pure `std` (a preallocated `Vec` plus two `AtomicUsize` cursors). No external
//! crate, no allocation after construction, no locks. One producer thread (the
//! backend read side) and one consumer thread (processing / watchdog) only. Using it
//! from more than one producer or more than one consumer is a contract violation and
//! is not defended against.
//!
//! # Memory model
//!
//! Capacity is stored as `cap + 1` slots so a full ring is distinguishable from an
//! empty one without a separate count. The producer publishes with a `Release` store
//! on `head`; the consumer observes with an `Acquire` load, which establishes the
//! happens-before edge that makes the written frame visible. Symmetrically for the
//! consumer's `tail`. This is the standard Lamport SPSC queue.
//!
//! A "frame" here is `frame_len` interleaved `f32` samples (one sample per active
//! channel). The ring stores whole frames; partial frames are never enqueued.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// The shared storage behind a `Producer`/`Consumer` pair.
struct Shared {
    /// `(cap + 1) * frame_len` f32s. Interior mutability: only the producer writes a
    /// slot before publishing it, only the consumer reads a slot before releasing it,
    /// and the atomics fence the handoff, so there is never concurrent access to the
    /// same slot.
    buf: UnsafeCell<Vec<f32>>,
    /// Number of samples per frame (channel count).
    frame_len: usize,
    /// Total slot count = usable_frames + 1.
    slots: usize,
    /// Next frame index the producer will write (published with Release).
    head: AtomicUsize,
    /// Next frame index the consumer will read (published with Release).
    tail: AtomicUsize,
    /// Frames the producer had to drop because the ring was full. Monotonic.
    /// Overrun is a real event the caller must surface (a dropped-sample counter
    /// feeds the watchdog / capture-gap machinery in the wider system).
    overruns: AtomicUsize,
}

// SAFETY: the atomics fence every access to `buf`; a slot is only touched by one side
// at a time. The `f32` payload is `Send`. The pair is split into a `Producer` and a
// `Consumer`, each of which is `!Sync` in practice by construction (moved to one
// thread). We mark `Shared` `Sync` so the two halves can live on different threads.
unsafe impl Sync for Shared {}
unsafe impl Send for Shared {}

/// The write end. Move this to the producer thread. `!Clone` on purpose: a second
/// producer would break the SPSC invariant.
pub struct Producer {
    shared: Arc<Shared>,
}

/// The read end. Move this to the consumer thread.
pub struct Consumer {
    shared: Arc<Shared>,
}

/// Create an SPSC ring holding up to `capacity_frames` frames of `frame_len` samples
/// each. All storage is allocated here; nothing allocates afterward.
///
/// Panics if `capacity_frames == 0` or `frame_len == 0` (a zero-width ring is a
/// programming error, caught at construction rather than silently deadlocking).
pub fn channel(capacity_frames: usize, frame_len: usize) -> (Producer, Consumer) {
    assert!(capacity_frames > 0, "ring capacity must be > 0");
    assert!(frame_len > 0, "frame_len must be > 0");
    let slots = capacity_frames + 1;
    let shared = Arc::new(Shared {
        buf: UnsafeCell::new(vec![0.0; slots * frame_len]),
        frame_len,
        slots,
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        overruns: AtomicUsize::new(0),
    });
    (
        Producer {
            shared: Arc::clone(&shared),
        },
        Consumer { shared },
    )
}

impl Shared {
    /// Write `frame` into the slot at `frame_idx`. The producer owns this slot
    /// exclusively until it publishes `head`, so the mutable access is sound.
    #[inline]
    fn write_slot(&self, frame_idx: usize, frame: &[f32]) {
        let base = frame_idx * self.frame_len;
        // SAFETY: the producer holds sole access to `[base, base+frame_len)` until it
        // Release-stores `head`; no consumer can observe this slot before that store.
        let v = unsafe { &mut *self.buf.get() };
        v[base..base + self.frame_len].copy_from_slice(frame);
    }

    /// Immutable view of the slot at `frame_idx`. The consumer owns this slot until it
    /// publishes `tail`, so the shared read is sound.
    #[inline]
    fn read_slot(&self, frame_idx: usize) -> &[f32] {
        let base = frame_idx * self.frame_len;
        // SAFETY: the consumer holds sole read access to this slot until it
        // Release-stores `tail`; the producer will not reuse it before that store.
        let v = unsafe { &*self.buf.get() };
        &v[base..base + self.frame_len]
    }
}

impl Producer {
    #[inline]
    pub fn frame_len(&self) -> usize {
        self.shared.frame_len
    }

    /// Usable capacity in frames.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.shared.slots - 1
    }

    /// Try to push one frame. Returns `true` on success. On a full ring the frame is
    /// dropped and the overrun counter is bumped (never blocks, never overwrites the
    /// consumer's unread data). `frame.len()` must equal `frame_len`.
    #[inline]
    pub fn push(&self, frame: &[f32]) -> bool {
        debug_assert_eq!(frame.len(), self.shared.frame_len);
        let head = self.shared.head.load(Ordering::Relaxed);
        let next = advance(head, self.shared.slots);
        // Full if advancing head would collide with the consumer's tail.
        if next == self.shared.tail.load(Ordering::Acquire) {
            self.shared.overruns.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.shared.write_slot(head, frame);
        // Publish: the Release makes the slot write visible to a consumer that
        // Acquire-loads this head.
        self.shared.head.store(next, Ordering::Release);
        true
    }

    /// Frames dropped due to a full ring since construction.
    #[inline]
    pub fn overruns(&self) -> usize {
        self.shared.overruns.load(Ordering::Relaxed)
    }
}

impl Consumer {
    #[inline]
    pub fn frame_len(&self) -> usize {
        self.shared.frame_len
    }

    /// Number of frames currently available to read.
    ///
    /// The distance is computed by CASE, not by `head.wrapping_sub(tail) % slots`.
    /// That older form is wrong whenever `head < tail` (i.e. after the cursors have
    /// wrapped past the end of the backing store): `wrapping_sub` produces
    /// `2^64 - (tail - head)`, and reducing THAT modulo `slots` only agrees with
    /// `(head - tail) mod slots` when `slots` divides `2^64`. For `slots = 5`,
    /// `head = 0`, `tail = 3` it answered 3 where the true length is 2 - it
    /// over-reported the readable frames, which for a consumer sizing a copy is
    /// the dangerous direction. Latent because nothing on the hot path used it
    /// (`pop` / `drain_each` compare cursors directly); `drain_chunks` does.
    #[inline]
    pub fn len(&self) -> usize {
        let head = self.shared.head.load(Ordering::Acquire);
        let tail = self.shared.tail.load(Ordering::Relaxed);
        frames_between(tail, head, self.shared.slots)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Pop one frame into `out` (which must be `frame_len` long). Returns `true` on
    /// success, `false` if the ring is empty. Never blocks.
    #[inline]
    pub fn pop(&self, out: &mut [f32]) -> bool {
        debug_assert_eq!(out.len(), self.shared.frame_len);
        let tail = self.shared.tail.load(Ordering::Relaxed);
        if tail == self.shared.head.load(Ordering::Acquire) {
            return false; // empty
        }
        out.copy_from_slice(self.shared.read_slot(tail));
        let next = advance(tail, self.shared.slots);
        self.shared.tail.store(next, Ordering::Release);
        true
    }

    /// Apply `f` to each available frame in order WITHOUT copying it out, then release
    /// them all. This is the zero-copy read path the abort watchdog uses on the RAW
    /// ring: it must inspect every raw frame (never a decimated view) with the least
    /// overhead. `f` sees an immutable borrow of each frame slice in FIFO order.
    /// Returns the number of frames visited.
    #[inline]
    pub fn drain_each<F: FnMut(&[f32])>(&self, mut f: F) -> usize {
        let head = self.shared.head.load(Ordering::Acquire);
        let mut tail = self.shared.tail.load(Ordering::Relaxed);
        let mut n = 0;
        while tail != head {
            f(self.shared.read_slot(tail));
            tail = advance(tail, self.shared.slots);
            n += 1;
        }
        if n > 0 {
            self.shared.tail.store(tail, Ordering::Release);
        }
        n
    }

    /// Copy out as many whole frames as `buf` holds, CONTIGUOUSLY, and release
    /// them. Returns the number of FRAMES copied (`0..=buf.len() / frame_len`).
    ///
    /// This is the block-shaped sibling of [`Consumer::drain_each`]: `drain_each`
    /// visits frames one closure call at a time (zero copy, per-frame dispatch),
    /// `drain_chunks` does at most two `copy_from_slice` calls (one per side of a
    /// wrap) and hands the caller one interleaved buffer it can pass straight to a
    /// block DSP routine. Both release exactly the frames they consumed.
    ///
    /// `buf` is truncated to a whole number of frames: a buffer of 3.5 frames'
    /// worth copies at most 3. Nothing partial is ever emitted, and nothing is
    /// consumed that is not copied - a short `buf` leaves the remainder queued.
    ///
    /// The abort watchdog is deliberately NOT rewritten onto this (deferred perf work);
    /// the API exists and is correct.
    #[inline]
    pub fn drain_chunks(&mut self, buf: &mut [f32]) -> usize {
        let frame_len = self.shared.frame_len;
        let want = buf.len() / frame_len;
        if want == 0 {
            return 0;
        }
        let head = self.shared.head.load(Ordering::Acquire);
        let tail = self.shared.tail.load(Ordering::Relaxed);
        let n = frames_between(tail, head, self.shared.slots).min(want);
        if n == 0 {
            return 0;
        }
        // SAFETY: the consumer holds sole read access to `[tail, tail + n)` (mod
        // slots) until it Release-stores the new tail below; the producer cannot
        // reuse any of those slots before that store.
        let v = unsafe { &*self.shared.buf.get() };
        // The run from `tail` to the end of the backing store, then the wrapped
        // remainder from slot 0. `first == n` in the common (non-wrapping) case.
        let first = n.min(self.shared.slots - tail);
        buf[..first * frame_len].copy_from_slice(&v[tail * frame_len..(tail + first) * frame_len]);
        if n > first {
            let rest = n - first;
            buf[first * frame_len..n * frame_len].copy_from_slice(&v[..rest * frame_len]);
        }
        let new_tail = {
            let t = tail + n;
            if t >= self.shared.slots {
                t - self.shared.slots
            } else {
                t
            }
        };
        self.shared.tail.store(new_tail, Ordering::Release);
        n
    }
}

/// Frames from `tail` to `head` on a ring of `slots` slots. Both cursors are
/// always `< slots`, so this is exact in both the wrapped and unwrapped case (see
/// the note on [`Consumer::len`] for why the modular-arithmetic shortcut is not).
#[inline]
fn frames_between(tail: usize, head: usize, slots: usize) -> usize {
    if head >= tail {
        head - tail
    } else {
        slots - tail + head
    }
}

#[inline]
fn advance(i: usize, slots: usize) -> usize {
    let n = i + 1;
    if n == slots {
        0
    } else {
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn push_pop_fifo_order() {
        let (p, c) = channel(4, 2);
        assert!(p.push(&[1.0, 2.0]));
        assert!(p.push(&[3.0, 4.0]));
        assert_eq!(c.len(), 2);
        let mut f = [0.0; 2];
        assert!(c.pop(&mut f));
        assert_eq!(f, [1.0, 2.0]);
        assert!(c.pop(&mut f));
        assert_eq!(f, [3.0, 4.0]);
        assert!(!c.pop(&mut f)); // empty
    }

    #[test]
    fn full_ring_drops_and_counts_overruns() {
        let (p, c) = channel(2, 1); // usable capacity 2
        assert!(p.push(&[1.0]));
        assert!(p.push(&[2.0]));
        assert!(!p.push(&[3.0])); // full: dropped
        assert_eq!(p.overruns(), 1);
        let mut f = [0.0; 1];
        assert!(c.pop(&mut f));
        assert_eq!(f, [1.0]); // oldest retained, newest dropped
    }

    #[test]
    fn wraps_around_capacity() {
        let (p, c) = channel(2, 1);
        let mut f = [0.0; 1];
        for round in 0..10 {
            assert!(p.push(&[round as f32]));
            assert!(c.pop(&mut f));
            assert_eq!(f[0], round as f32);
        }
    }

    #[test]
    fn drain_each_visits_all_in_order() {
        let (p, c) = channel(8, 1);
        for i in 0..5 {
            assert!(p.push(&[i as f32]));
        }
        let mut seen = Vec::new();
        let n = c.drain_each(|frame| seen.push(frame[0]));
        assert_eq!(n, 5);
        assert_eq!(seen, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
        assert!(c.is_empty());
    }

    /// RED-MUTATION: restore `len()`'s old body
    /// (`head.wrapping_sub(tail) % slots`) and this test reds - with `slots = 5`,
    /// `head = 0`, `tail = 3` the old form answers 3 where two frames are
    /// readable, over-reporting the queue depth to any consumer that sizes a copy
    /// from it.
    #[test]
    fn len_is_exact_after_the_cursors_wrap() {
        // capacity 4 -> slots 5. Advance both cursors to 3, then push 2 so head
        // wraps to 0 while tail stays at 3 (the head < tail case).
        let (p, mut c) = channel(4, 1);
        let mut f = [0.0; 1];
        for i in 0..3 {
            assert!(p.push(&[i as f32]));
            assert!(c.pop(&mut f));
        }
        assert_eq!(c.len(), 0);
        assert!(p.push(&[10.0]));
        assert!(p.push(&[11.0]));
        assert_eq!(c.len(), 2, "two frames queued across the wrap");
        // And the count agrees with what can actually be drained.
        let mut out = [0.0; 4];
        assert_eq!(c.drain_chunks(&mut out), 2);
        assert_eq!(&out[..2], &[10.0, 11.0]);
    }

    #[test]
    fn drain_chunks_copies_whole_frames_in_order() {
        let (p, mut c) = channel(8, 2);
        for i in 0..4 {
            assert!(p.push(&[i as f32, (i + 100) as f32]));
        }
        let mut out = [0.0f32; 8]; // 4 frames of 2
        assert_eq!(c.drain_chunks(&mut out), 4);
        assert_eq!(out, [0.0, 100.0, 1.0, 101.0, 2.0, 102.0, 3.0, 103.0]);
        assert!(c.is_empty(), "drained frames are released");
        assert_eq!(c.drain_chunks(&mut out), 0, "empty ring copies nothing");
    }

    #[test]
    fn drain_chunks_truncates_to_whole_frames_and_leaves_the_rest() {
        let (p, mut c) = channel(8, 2);
        for i in 0..4 {
            assert!(p.push(&[i as f32, -(i as f32)]));
        }
        // Buffer holds 2.5 frames: exactly 2 are copied, 2 stay queued.
        let mut out = [0.0f32; 5];
        assert_eq!(c.drain_chunks(&mut out), 2);
        assert_eq!(&out[..4], &[0.0, -0.0, 1.0, -1.0]);
        assert_eq!(c.len(), 2, "the unread remainder is NOT consumed");
        // A sub-frame buffer copies (and consumes) nothing at all.
        let mut tiny = [0.0f32; 1];
        assert_eq!(c.drain_chunks(&mut tiny), 0);
        assert_eq!(c.len(), 2);
    }

    /// The wrap-around case: the readable run straddles the end of the backing
    /// store, so the copy is two `copy_from_slice` calls and the order across the
    /// seam must still be FIFO.
    #[test]
    fn drain_chunks_handles_wrap_around() {
        // capacity 4 (slots 5), frame_len 2.
        let (p, mut c) = channel(4, 2);
        let mut scratch = [0.0f32; 8];
        // Push 3, drain 3: tail and head both sit at slot 3.
        for i in 0..3 {
            assert!(p.push(&[i as f32, i as f32]));
        }
        assert_eq!(c.drain_chunks(&mut scratch), 3);
        // Now push 4 more: slots 3, 4, then wrap to 0, 1.
        for i in 10..14 {
            assert!(p.push(&[i as f32, (i * 2) as f32]));
        }
        assert_eq!(c.len(), 4);
        let mut out = [0.0f32; 8];
        assert_eq!(c.drain_chunks(&mut out), 4);
        assert_eq!(
            out,
            [10.0, 20.0, 11.0, 22.0, 12.0, 24.0, 13.0, 26.0],
            "FIFO order must survive the seam between the two copies"
        );
        assert!(c.is_empty());
        // The ring is still usable after a wrapping drain.
        assert!(p.push(&[99.0, 98.0]));
        assert_eq!(c.drain_chunks(&mut out), 1);
        assert_eq!(&out[..2], &[99.0, 98.0]);
    }

    /// `drain_chunks` and `drain_each` must see the same frames in the same
    /// order: they are two shapes of one read path, not two answers.
    #[test]
    fn drain_chunks_agrees_with_drain_each() {
        let fill = |p: &Producer| {
            for i in 0..7 {
                assert!(p.push(&[i as f32, (i * 3) as f32]));
            }
        };
        let (p1, c1) = channel(16, 2);
        fill(&p1);
        let mut via_each = Vec::new();
        c1.drain_each(|f| via_each.extend_from_slice(f));

        let (p2, mut c2) = channel(16, 2);
        fill(&p2);
        let mut via_chunks = vec![0.0f32; 14];
        let n = c2.drain_chunks(&mut via_chunks);
        assert_eq!(n, 7);
        assert_eq!(via_each, via_chunks);
    }

    #[test]
    fn cross_thread_spsc_transfers_all() {
        let (p, c) = channel(64, 1);
        const N: usize = 100_000;
        let prod = thread::spawn(move || {
            let mut i = 0usize;
            while i < N {
                if p.push(&[i as f32]) {
                    i += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
        });
        let mut got = 0usize;
        let mut f = [0.0; 1];
        while got < N {
            if c.pop(&mut f) {
                assert_eq!(f[0], got as f32);
                got += 1;
            } else {
                std::hint::spin_loop();
            }
        }
        prod.join().unwrap();
        assert_eq!(got, N);
    }
}
