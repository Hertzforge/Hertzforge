//! Device-counter timebase with EPOCH and GAP handling.
//!
//! The engine's clock is the device sample counter from [`BackendStatus`], never host
//! wall-clock. Three hazards the naive "order by counter" approach gets wrong, all
//! handled here:
//!
//! * **Reset vs wrap.** A re-enumeration/reconnect resets the device counter to zero.
//!   A 64-bit wrap-extender cannot tell that apart from a genuine wrap and would
//!   violently misorder. The backend surfaces a reset by bumping [`BackendStatus::epoch`];
//!   this module treats an epoch bump as a NEW epoch, resetting the counter origin, and
//!   never lets the wrap-extender absorb a reset.
//! * **Counter wrap.** Within one epoch the raw counter is monotonic and may wrap the
//!   64-bit field. A decrease WITHOUT an epoch bump is a wrap; the high extension is
//!   incremented so the extended counter keeps rising.
//! * **Dropped samples (gap).** A forward jump larger than the frames actually
//!   delivered marks a time-gap. Downstream PSD/COLA math must not integrate across a
//!   gap or an epoch boundary; this module surfaces both as [`TimebaseEvent`]s so the
//!   processing step can reset overlap-add state.
//!
//! Counter -> seconds uses the backend sample rate: `t = extended_frames / fs`.

use eigen_hal::BackendStatus;

/// A discontinuity detected on the counter timebase. The scheduler forwards these so
/// framed DSP (PSD/COLA) can reset state across the boundary rather than integrating
/// across a time discontinuity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimebaseEvent {
    /// The device reset (epoch bumped). Carries the new epoch. A hard boundary: no
    /// framed transform may span it.
    Reset { new_epoch: u32 },
    /// The counter jumped forward by more than the frames delivered: `missing` frames
    /// were dropped between the previous update and this one.
    Gap { missing: u64 },
    /// The raw 64-bit counter wrapped within one epoch (informational; continuity is
    /// preserved by the extended counter, so NOT a boundary for framed DSP).
    Wrap,
}

/// Tracks the extended (wrap-free) frame position across resets, wraps, and gaps.
#[derive(Debug, Clone)]
pub struct Timebase {
    sample_rate_hz: f64,
    /// Current epoch we are tracking; `None` until the first update.
    epoch: Option<u32>,
    /// Raw device counter seen on the previous update (within the current epoch).
    last_raw: u64,
    /// Sum of full 2^64 wraps observed within the current epoch.
    wrap_high: u128,
    /// Extended counter value (wrap_high + raw) at the START of the current epoch. The
    /// epoch frame index is `current_extended - epoch_origin`, so a counter that starts
    /// at 5000 in a new epoch still reports epoch_frame 0 at its first sample.
    epoch_origin: u128,
    /// Extended frame count SINCE the start of the current epoch (wrap-free).
    epoch_frames: u128,
    /// Extended frame count accumulated in all PRIOR epochs, so the global timeline is
    /// monotonic across resets (a reset does not rewind wall-time on the record axis).
    prior_epoch_frames: u128,
}

impl Timebase {
    pub fn new(sample_rate_hz: f64) -> Self {
        Timebase {
            sample_rate_hz,
            epoch: None,
            last_raw: 0,
            wrap_high: 0,
            epoch_origin: 0,
            epoch_frames: 0,
            prior_epoch_frames: 0,
        }
    }

    /// Extended, wrap-free frame index within the CURRENT epoch (starts at the first
    /// counter value seen in the epoch).
    #[inline]
    pub fn epoch_frame(&self) -> u128 {
        self.epoch_frames
    }

    /// Global monotonic frame index across all epochs (never rewinds on a reset).
    #[inline]
    pub fn global_frame(&self) -> u128 {
        self.prior_epoch_frames + self.epoch_frames
    }

    /// Current epoch, if any update has been seen.
    #[inline]
    pub fn epoch(&self) -> Option<u32> {
        self.epoch
    }

    /// Seconds since the start of the current epoch.
    #[inline]
    pub fn epoch_seconds(&self) -> f64 {
        if self.sample_rate_hz <= 0.0 {
            return 0.0;
        }
        self.epoch_frames as f64 / self.sample_rate_hz
    }

    /// Global seconds across the whole record (monotonic across resets).
    #[inline]
    pub fn global_seconds(&self) -> f64 {
        if self.sample_rate_hz <= 0.0 {
            return 0.0;
        }
        self.global_frame() as f64 / self.sample_rate_hz
    }

    /// Feed the latest device status plus how many input frames were actually
    /// delivered by the read that produced this status. Returns the discontinuities
    /// detected (usually empty). `frames_delivered` is the real payload count; the
    /// counter is expected to advance by exactly that much on a clean update.
    ///
    /// Order of precedence: an epoch bump is a Reset (checked first; a reset-to-zero
    /// must never be read as a wrap). Otherwise a counter decrease is a Wrap. A
    /// forward jump beyond `frames_delivered` is a Gap.
    pub fn update(&mut self, status: &BackendStatus, frames_delivered: u64) -> Vec<TimebaseEvent> {
        let mut events = Vec::new();
        let raw = status.sample_counter;

        // First-ever update: adopt the epoch and counter origin, no events. The origin
        // is this first counter value, so epoch_frame starts at 0 regardless of where
        // the device counter happens to be.
        let Some(cur_epoch) = self.epoch else {
            self.epoch = Some(status.epoch);
            self.last_raw = raw;
            self.wrap_high = 0;
            self.epoch_origin = raw as u128;
            self.epoch_frames = 0;
            return events;
        };

        // --- Reset (epoch bump) takes precedence over any counter interpretation. ---
        if status.epoch != cur_epoch {
            // Roll the current epoch's frame count into the prior total so the global
            // timeline stays monotonic, then start a fresh epoch anchored at this
            // counter (epoch_frame resets to 0, global_frame does NOT rewind).
            self.prior_epoch_frames += self.epoch_frames;
            self.epoch = Some(status.epoch);
            self.last_raw = raw;
            self.wrap_high = 0;
            self.epoch_origin = raw as u128;
            self.epoch_frames = 0;
            events.push(TimebaseEvent::Reset {
                new_epoch: status.epoch,
            });
            return events;
        }

        // --- Within the same epoch: wrap vs forward-advance. ---
        if raw < self.last_raw {
            // Counter decreased without an epoch bump => the 64-bit field wrapped.
            self.wrap_high += 1u128 << 64;
            events.push(TimebaseEvent::Wrap);
        }

        let extended = self.wrap_high + raw as u128;
        // Frames since the epoch origin (this is what epoch_frames tracks).
        let new_epoch_frames = extended.saturating_sub(self.epoch_origin);
        let advance = new_epoch_frames.saturating_sub(self.epoch_frames);

        // A clean update advances by exactly `frames_delivered`. More than that means
        // the device counted samples the host never received: a dropped-block gap.
        if advance > frames_delivered as u128 {
            let missing = advance - frames_delivered as u128;
            events.push(TimebaseEvent::Gap {
                missing: missing.min(u64::MAX as u128) as u64,
            });
        }

        self.epoch_frames = new_epoch_frames;
        self.last_raw = raw;
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(counter: u64, epoch: u32) -> BackendStatus {
        BackendStatus {
            connected: true,
            faulted: false,
            message: String::new(),
            sample_counter: counter,
            epoch,
        }
    }

    #[test]
    fn clean_advance_no_events() {
        let mut tb = Timebase::new(1000.0);
        assert!(tb.update(&status(0, 0), 0).is_empty()); // first update seeds origin
        assert!(tb.update(&status(64, 0), 64).is_empty());
        assert!(tb.update(&status(128, 0), 64).is_empty());
        assert_eq!(tb.epoch_frame(), 128);
        assert_eq!(tb.epoch_seconds(), 0.128);
    }

    #[test]
    fn epoch_bump_is_reset_not_wrap() {
        let mut tb = Timebase::new(1000.0);
        tb.update(&status(5000, 0), 0);
        tb.update(&status(5064, 0), 64);
        // Device re-enumerated: counter resets to a small value AND epoch bumps.
        let ev = tb.update(&status(0, 1), 0);
        assert_eq!(ev, vec![TimebaseEvent::Reset { new_epoch: 1 }]);
        assert_eq!(tb.epoch(), Some(1));
        assert_eq!(tb.epoch_frame(), 0);
        // Global timeline did NOT rewind: prior epoch's 64 frames are retained.
        assert_eq!(tb.global_frame(), 64);
    }

    #[test]
    fn counter_wrap_within_epoch_preserves_continuity() {
        let mut tb = Timebase::new(1000.0);
        let near = u64::MAX - 10;
        tb.update(&status(near, 0), 0); // seed origin near the top
                                        // Advance across the 64-bit boundary: raw decreases but epoch is unchanged.
        let ev = tb.update(&status(20, 0), 31); // 10 to reach MAX+1, then 21 more = 31
        assert!(ev.contains(&TimebaseEvent::Wrap));
        assert!(!ev.iter().any(|e| matches!(e, TimebaseEvent::Gap { .. })));
    }

    #[test]
    fn forward_jump_beyond_delivered_is_gap() {
        let mut tb = Timebase::new(1000.0);
        tb.update(&status(0, 0), 0);
        // Counter advanced 256 but only 64 frames were delivered: 192 dropped.
        let ev = tb.update(&status(256, 0), 64);
        assert_eq!(ev, vec![TimebaseEvent::Gap { missing: 192 }]);
        assert_eq!(tb.epoch_frame(), 256); // time axis still tracks the device
    }

    #[test]
    fn exact_delivery_is_not_a_gap() {
        let mut tb = Timebase::new(1000.0);
        tb.update(&status(0, 0), 0);
        let ev = tb.update(&status(64, 0), 64);
        assert!(ev.is_empty());
    }
}
