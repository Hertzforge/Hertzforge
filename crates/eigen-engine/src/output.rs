//! Output-delivery mechanism with a lookahead buffer honoring the [`OutputBufferContract`].
//!
//! Safety model: the drive side must never run dry into a DC step or a
//! zero-cliff, either of which injects a broadband transient into a running shaker.
//! The contract, in order of safety:
//!
//! 1. The engine maintains a **lookahead of at least `min_lookahead_frames`** pre-queued
//!    drive frames, so a bounded scheduling stall cannot drain the buffer.
//! 2. On a **low-water event** (queued drive falls below `min_lookahead_frames`) the
//!    configured [`UnderrunPolicy`] is applied:
//!    * `RampDownThenAbort` ramps the drive smoothly to zero over the remaining queued
//!      frames, then aborts.
//!    * `HoldLastThenAbort` holds the last commanded frame (no step), then aborts.
//! 3. An **actual underrun** (nothing left to send when the DAC needs a frame) is
//!    treated as an ABORT event, never a recoverable gap.
//!
//! This module is the queue + policy state machine; a real backend thread drives it by
//! calling [`OutputDelivery::next_frame`] whenever the DAC needs the next block and
//! [`OutputDelivery::enqueue`] as the processing step produces drive frames. It does
//! not itself own a thread, so it is deterministically testable.

use eigen_hal::{OutputBufferContract, UnderrunPolicy};
use std::collections::VecDeque;

/// The outcome of asking the delivery buffer for the next drive frame.
#[derive(Debug, Clone, PartialEq)]
pub enum DeliveryOutcome {
    /// A normal queued frame. Send it to the DAC.
    Frame(Vec<f32>),
    /// Low-water: we are honoring the policy. Under `RampDownThenAbort` this frame is a
    /// scaled-down copy heading to zero; under `HoldLastThenAbort` it is the held last
    /// frame. Send it, but know an abort is imminent.
    PolicyFrame(Vec<f32>),
    /// Actual underrun: nothing to send. This IS an abort event. The caller must invoke
    /// `emergency_stop()`; delivering silence/step here is forbidden.
    Underrun,
}

/// A drive-underrun abort reason, surfaced to the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderrunAbort {
    /// The ramp-down (or hold) completed and the policy demands an abort now.
    PolicyComplete,
    /// The queue hit true empty with no policy frames left. Hard underrun.
    HardUnderrun,
}

/// Lookahead drive queue + underrun policy state machine.
pub struct OutputDelivery {
    contract: OutputBufferContract,
    frame_len: usize,
    queue: VecDeque<Vec<f32>>,
    /// Last frame actually delivered, for `HoldLastThenAbort` and as the ramp seed.
    last_frame: Vec<f32>,
    /// When a low-water event starts a `RampDownThenAbort`, we ramp over this many
    /// steps. Set to the lookahead depth so the ramp is smooth and bounded.
    ramp_total: u32,
    ramp_step: u32,
    /// True once we have entered the policy (ramp/hold) phase; latched.
    in_policy: bool,
    /// Set once the policy has produced its final frame and an abort is owed.
    abort_pending: Option<UnderrunAbort>,
    /// Count of low-water events observed (test observability / health telemetry).
    low_water_events: u32,
}

impl OutputDelivery {
    pub fn new(contract: OutputBufferContract, frame_len: usize) -> Self {
        let ramp_total = contract.min_lookahead_frames.max(1);
        OutputDelivery {
            contract,
            frame_len,
            queue: VecDeque::new(),
            last_frame: vec![0.0; frame_len],
            ramp_total,
            ramp_step: 0,
            in_policy: false,
            abort_pending: None,
            low_water_events: 0,
        }
    }

    pub fn frame_len(&self) -> usize {
        self.frame_len
    }

    /// Frames currently queued ahead of the DAC.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Whether the queue is at or above the contracted lookahead (healthy).
    pub fn has_lookahead(&self) -> bool {
        self.queue.len() >= self.contract.min_lookahead_frames as usize
    }

    pub fn low_water_events(&self) -> u32 {
        self.low_water_events
    }

    /// If the policy has run to completion, the abort that is now owed.
    pub fn abort_pending(&self) -> Option<UnderrunAbort> {
        self.abort_pending
    }

    /// Enqueue one produced drive frame. `frame.len()` must equal `frame_len`. Enqueuing
    /// is refused once the policy phase has begun (we are shutting down, not resuming):
    /// returns `false` in that case.
    pub fn enqueue(&mut self, frame: &[f32]) -> bool {
        debug_assert_eq!(frame.len(), self.frame_len);
        if self.in_policy {
            return false;
        }
        self.queue.push_back(frame.to_vec());
        true
    }

    /// Pre-fill the queue with `n` zero frames (a benign starting drive) so the very
    /// first `next_frame` calls already have the contracted lookahead. Used at `arm()`
    /// before `start()`.
    pub fn prime_with_zeros(&mut self, n: usize) {
        for _ in 0..n {
            self.queue.push_back(vec![0.0; self.frame_len]);
        }
    }

    /// Replace the queue with `frames`.
    ///
    /// Used by a caller that scores one continuous acquisition in segments and
    /// builds a fresh engine per segment: the constructor's zero priming is
    /// correct for the START of a run and wrong at a segment boundary, where
    /// the previous segment's undelivered REAL drive is what should follow.
    /// Empty input keeps the benign priming rather than leaving the queue dry.
    pub fn adopt_queue(&mut self, frames: Vec<Vec<f32>>) {
        if frames.is_empty() {
            return;
        }
        self.queue.clear();
        for f in frames {
            if f.len() == self.frame_len {
                self.queue.push_back(f);
            }
        }
    }

    /// Take the undelivered frames, leaving the queue empty.
    pub fn take_queue(&mut self) -> Vec<Vec<f32>> {
        self.queue.drain(..).collect()
    }

    /// The DAC needs the next drive frame. Returns a [`DeliveryOutcome`]. This is where
    /// the lookahead contract and underrun policy are enforced.
    pub fn next_frame(&mut self) -> DeliveryOutcome {
        // Already committed to the policy phase: keep producing policy frames until the
        // ramp/hold completes, then signal the abort.
        if self.in_policy {
            return self.policy_next();
        }

        // Healthy path: we have more than the lookahead threshold queued.
        if self.queue.len() > self.contract.min_lookahead_frames as usize {
            let f = self.queue.pop_front().unwrap();
            self.last_frame.copy_from_slice(&f);
            return DeliveryOutcome::Frame(f);
        }

        // Low-water: queued <= min_lookahead. The bounded stall protection is exhausted;
        // enter the policy phase rather than risk a run-dry.
        self.low_water_events += 1;
        self.in_policy = true;
        self.ramp_step = 0;
        // Snapshot the remaining real frames as the ramp length denominator so a partly
        // drained queue still ramps smoothly to zero.
        self.ramp_total = (self.queue.len() as u32).max(1);
        self.policy_next()
    }

    fn policy_next(&mut self) -> DeliveryOutcome {
        match self.contract.underrun_policy {
            UnderrunPolicy::RampDownThenAbort => {
                if self.ramp_step >= self.ramp_total {
                    // Ramp finished; final zero already delivered on the last step.
                    self.abort_pending = Some(UnderrunAbort::PolicyComplete);
                    return DeliveryOutcome::Underrun;
                }
                // Linear gain from just-below-1 down to 0 across ramp_total steps.
                self.ramp_step += 1;
                let gain = 1.0 - (self.ramp_step as f32 / self.ramp_total as f32);
                // Base the ramp on the next real queued frame if present, else the last
                // delivered frame, so the shape follows the intended drive down to zero.
                let base = self
                    .queue
                    .pop_front()
                    .unwrap_or_else(|| self.last_frame.clone());
                let mut out = base;
                for s in out.iter_mut() {
                    *s *= gain;
                }
                self.last_frame.copy_from_slice(&out);
                if self.ramp_step >= self.ramp_total {
                    // This was the final (zero-gain) frame; abort owed after it is sent.
                    self.abort_pending = Some(UnderrunAbort::PolicyComplete);
                }
                DeliveryOutcome::PolicyFrame(out)
            }
            UnderrunPolicy::HoldLastThenAbort => {
                if self.ramp_step >= self.ramp_total {
                    self.abort_pending = Some(UnderrunAbort::PolicyComplete);
                    return DeliveryOutcome::Underrun;
                }
                self.ramp_step += 1;
                let out = self.last_frame.clone();
                if self.ramp_step >= self.ramp_total {
                    self.abort_pending = Some(UnderrunAbort::PolicyComplete);
                }
                DeliveryOutcome::PolicyFrame(out)
            }
        }
    }

    /// Force a hard underrun (nothing was ever queued and the DAC needs a frame now).
    /// Distinct from the graceful low-water path: there is no queued drive to ramp from,
    /// so this is an immediate abort.
    pub fn force_underrun(&mut self) -> UnderrunAbort {
        self.abort_pending = Some(UnderrunAbort::HardUnderrun);
        UnderrunAbort::HardUnderrun
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract(min: u32, policy: UnderrunPolicy) -> OutputBufferContract {
        OutputBufferContract {
            min_lookahead_frames: min,
            underrun_policy: policy,
        }
    }

    #[test]
    fn healthy_queue_delivers_real_frames() {
        let mut od = OutputDelivery::new(contract(2, UnderrunPolicy::RampDownThenAbort), 1);
        for i in 0..5 {
            assert!(od.enqueue(&[i as f32]));
        }
        assert!(od.has_lookahead());
        // With 5 queued and min 2, the first frames are real.
        assert_eq!(od.next_frame(), DeliveryOutcome::Frame(vec![0.0]));
        assert_eq!(od.next_frame(), DeliveryOutcome::Frame(vec![1.0]));
    }

    #[test]
    fn ramp_down_on_low_water_reaches_zero_then_aborts() {
        // min_lookahead 2: with exactly 2 queued we are at low-water immediately.
        let mut od = OutputDelivery::new(contract(2, UnderrunPolicy::RampDownThenAbort), 1);
        od.enqueue(&[10.0]);
        od.enqueue(&[10.0]);
        // First next_frame: queue.len()==2 is NOT > 2, so we enter the ramp.
        let mut last = f32::INFINITY;
        let mut saw_abort = false;
        for _ in 0..8 {
            match od.next_frame() {
                DeliveryOutcome::PolicyFrame(f) => {
                    // Monotonically non-increasing magnitude heading to zero.
                    assert!(f[0].abs() <= last + 1e-6, "ramp should not increase");
                    last = f[0].abs();
                }
                DeliveryOutcome::Underrun => {
                    saw_abort = true;
                    break;
                }
                DeliveryOutcome::Frame(_) => panic!("should be in policy phase"),
            }
        }
        assert!(saw_abort, "ramp must terminate in an underrun/abort");
        assert_eq!(od.abort_pending(), Some(UnderrunAbort::PolicyComplete));
        assert!(last < 1e-6, "final ramp frame reaches ~zero, got {last}");
        assert_eq!(od.low_water_events(), 1);
    }

    #[test]
    fn ramp_never_steps_or_cliffs() {
        // The whole point: no single frame jumps from a value straight to zero.
        let mut od = OutputDelivery::new(contract(4, UnderrunPolicy::RampDownThenAbort), 1);
        for _ in 0..4 {
            od.enqueue(&[8.0]);
        }
        let mut values = Vec::new();
        loop {
            match od.next_frame() {
                DeliveryOutcome::PolicyFrame(f) => values.push(f[0]),
                DeliveryOutcome::Underrun => break,
                DeliveryOutcome::Frame(f) => values.push(f[0]),
            }
            if values.len() > 20 {
                break;
            }
        }
        // Consecutive step never exceeds the initial amplitude / ramp_total.
        for w in values.windows(2) {
            assert!((w[0] - w[1]).abs() <= 8.0, "step too large: {:?}", w);
        }
        assert!(values.last().unwrap().abs() < 1e-6);
    }

    #[test]
    fn hold_last_policy_holds_then_aborts() {
        let mut od = OutputDelivery::new(contract(2, UnderrunPolicy::HoldLastThenAbort), 1);
        od.enqueue(&[3.0]);
        od.enqueue(&[7.0]);
        od.enqueue(&[9.0]);
        // Drain the one real frame above lookahead.
        assert_eq!(od.next_frame(), DeliveryOutcome::Frame(vec![3.0]));
        // Now at low-water (2 queued). Enter hold policy: frames repeat the last real.
        let mut held = None;
        loop {
            match od.next_frame() {
                DeliveryOutcome::PolicyFrame(f) => held = Some(f[0]),
                DeliveryOutcome::Underrun => break,
                DeliveryOutcome::Frame(_) => panic!("should be in policy"),
            }
        }
        assert_eq!(held, Some(3.0), "hold repeats the last delivered frame");
        assert_eq!(od.abort_pending(), Some(UnderrunAbort::PolicyComplete));
    }

    #[test]
    fn hard_underrun_is_immediate_abort() {
        let mut od = OutputDelivery::new(contract(2, UnderrunPolicy::RampDownThenAbort), 1);
        assert_eq!(od.force_underrun(), UnderrunAbort::HardUnderrun);
        assert_eq!(od.abort_pending(), Some(UnderrunAbort::HardUnderrun));
    }

    #[test]
    fn enqueue_refused_once_in_policy() {
        let mut od = OutputDelivery::new(contract(1, UnderrunPolicy::RampDownThenAbort), 1);
        od.enqueue(&[1.0]);
        od.next_frame(); // enters policy (1 queued, not > 1)
        assert!(!od.enqueue(&[2.0]), "no resume after policy starts");
    }
}
