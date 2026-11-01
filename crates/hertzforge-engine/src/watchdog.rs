//! Abort watchdog: tri-field per-channel limit monitor on the RAW input ring.
//!
//! Safety model:
//!
//! * The watchdog reads the **raw acquisition frames before any decimation** and MUST
//!   NEVER read the same decimated buffer the scope reads. In `HostClosesLoop` mode it
//!   is the PRIMARY abort. It is a **peak-detecting sustained-over-level monitor whose
//!   temporal resolution equals the raw sample rate**; it protects against sustained
//!   over-level, and (because it taps the raw ring) can also catch a sub-frame spike
//!   that a decimated stream would average away. It is documented as NOT a guaranteed
//!   sub-microsecond transient catcher; the hardware interlock / physical E-stop remain
//!   the boundary for the sub-frame-transient-destruction class.
//! * Limits are the tri-field [`AbortLimits`] from the [`ChannelTable`]: acceleration
//!   (g), velocity (in/s), displacement (mil). `None` on an axis means no limit there.
//! * On a trip the watchdog calls `backend.emergency_stop()`.
//!
//! ## Per-limit-type latency budget, made explicit
//!
//! The three limit types do NOT have the same trip latency:
//!
//! * **Acceleration (g):** near-zero added delay. The raw sample is compared directly.
//! * **Velocity (in/s) and displacement (mil):** require real-time single/double
//!   integration whose high-pass + integrator group delay ADDS to the trip latency for
//!   those limit types. [`LatencyBudget`] states the per-type budget in frames; the
//!   engine surfaces it at `arm()` so the operator knows the enforceable band. The
//!   integrator here mirrors `hertzforge_dsp::integrate` (high-pass the acceleration BEFORE
//!   integrating, so a DC offset never becomes an unbounded ramp) so the watchdog and
//!   the analysis path agree on what "velocity" means.
//!
//! ## Sustained-over-level (debounce)
//!
//! A single stray sample should not abort a live test; a genuine over-level persists.
//! [`AbortWatchdog`] trips when a channel/axis stays over its limit for
//! `sustain_frames` consecutive raw frames (set `sustain_frames = 1` for the
//! spike-sensitive configuration used by the single-sample-spike test).

use crate::limits::ChannelLimiter;
use crate::ring::Consumer;
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_hal::Backend;

// The tri-field limit types are defined once in `crate::limits` and re-exported here
// so the pinned `watchdog::{AbortAxis, AbortTrip}` public path (and the crate-root
// re-export) is unchanged while the limit math is shared with the SMART run loop.
pub use crate::limits::{AbortAxis, AbortTrip};

/// Per-limit-type trip-latency budget, in raw frames, surfaced at `arm()`.
///
/// `accel_frames` is the near-zero g-path budget. `vel_frames` / `disp_frames` are the
/// integrator-group-delay budgets for the vel/disp paths; they are strictly larger
/// because a single/double integration cannot report a settled value instantly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatencyBudget {
    pub accel_frames: u32,
    pub vel_frames: u32,
    pub disp_frames: u32,
}

impl LatencyBudget {
    /// Default budget derived from the integrator high-pass settling. The g path is a
    /// direct compare (1 frame). The vel/disp paths add the high-pass transient +
    /// integration settling; stated conservatively here and CI-tracked against the
    /// measured value in the wider system.
    pub const fn conservative() -> Self {
        LatencyBudget {
            accel_frames: 1,
            vel_frames: 8,
            disp_frames: 16,
        }
    }

    /// Worst-case across all three (the number the arm() gate compares against a
    /// backend's `abort_latency_bound`).
    pub const fn worst_case_frames(&self) -> u32 {
        let m = if self.accel_frames > self.vel_frames {
            self.accel_frames
        } else {
            self.vel_frames
        };
        if m > self.disp_frames {
            m
        } else {
            self.disp_frames
        }
    }
}

// `AbortTrip` and `AbortAxis` now live in `crate::limits` (re-exported above) so the
// SMART run loop shares the exact same tri-field limit types and math.

/// Per-monitored-channel state: its position in the interleaved frame plus the shared
/// [`ChannelLimiter`] (integrator + limits + per-axis sustained-breach counters).
struct MonitoredChannel {
    frame_offset: usize,
    limiter: ChannelLimiter,
}

/// The abort watchdog. Owns the RAW-ring consumer and a `Box<dyn Backend>` handle it
/// calls `emergency_stop()` on. Constructed from the channel table so the frame layout
/// and per-channel limits are fixed at `arm()`.
pub struct AbortWatchdog {
    raw: Consumer,
    frame_len: usize,
    monitored: Vec<MonitoredChannel>,
    sustain_frames: u32,
    budget: LatencyBudget,
    frames_seen: u64,
    tripped: Option<AbortTrip>,
    estop_calls: u32,
    /// ARM BLANKING. While true the watchdog OBSERVES - integrators keep running,
    /// breaches keep being detected - but does NOT trip: no latch, no `emergency_stop`.
    /// Would-have-tripped events are counted and the first one is kept as a warning.
    blanked: bool,
    /// Breaches seen while blanked (never acted on).
    blanked_trips: u32,
    /// The FIRST breach seen while blanked, for the operator-facing warning.
    first_blanked_trip: Option<AbortTrip>,
    /// `poll()` calls since construction. Observability for the "the watchdog was
    /// checked every block" gate: a poll count below the tick count means a code
    /// path skipped the safety check.
    polls: u64,
}

impl AbortWatchdog {
    /// Build a watchdog from the channel table. `raw` is the consumer end of the RAW
    /// input ring (frame layout = interleaved enabled channels, one f32 per channel).
    /// `hp_hz` is the integrator high-pass corner (choose a decade below the lowest
    /// band of interest). `sustain_frames` is the debounce (1 = spike-sensitive).
    ///
    /// Only channels with at least one non-`None` abort axis are monitored; drive
    /// outputs are skipped. The frame offset for each channel is its position among the
    /// enabled channels, matching the backend's interleave.
    pub fn new(
        table: &ChannelTable,
        raw: Consumer,
        fs_hz: f64,
        hp_hz: f64,
        sustain_frames: u32,
    ) -> Self {
        let frame_len = raw.frame_len();
        let mut monitored = Vec::new();
        // Frame offset = position among ENABLED **INPUT** channels, because that is what
        // the backend interleaves into the input frame. `SimBackend::configure` states the
        // rule ("Control/Monitor/Reference are measurement inputs; Drive is an output") and
        // only pushes non-Drive rows into `input_channels`; a delegated backend interleaves
        // its measurement channels the same way.
        //
        // This loop used to increment `offset` BEFORE skipping Drive rows, so a Drive row
        // consumed an input-frame slot it does not occupy and every input row after it was
        // monitored one slot too high. With a table of [Control, Drive, Monitor] the
        // Monitor was read at offset 2 of a 2-wide frame. The out-of-bounds panic is the
        // LUCKY failure: where the shift lands inside the frame instead, the watchdog
        // silently compares one channel's samples against another channel's abort limits.
        // Latent until 2026-07-16 because the scheduler's frames-vs-samples defect meant
        // multi-channel runs ended on tick 1 before the watchdog ever drained a frame.
        let mut offset = 0usize;
        for ch in &table.channels {
            if !ch.enabled {
                continue;
            }
            if ch.role == ChannelRole::Drive {
                continue; // an output: not present in the input frame at all
            }
            // Every enabled INPUT channel occupies a slot, limits or not.
            let this_offset = offset;
            offset += 1;
            if !ChannelLimiter::has_limit(&ch.abort) {
                continue;
            }
            monitored.push(MonitoredChannel {
                frame_offset: this_offset,
                limiter: ChannelLimiter::new(
                    ch.index,
                    ch.abort,
                    fs_hz,
                    hp_hz,
                    sustain_frames.max(1),
                ),
            });
        }
        AbortWatchdog {
            raw,
            frame_len,
            monitored,
            sustain_frames: sustain_frames.max(1),
            budget: LatencyBudget::conservative(),
            frames_seen: 0,
            tripped: None,
            estop_calls: 0,
            blanked: false,
            blanked_trips: 0,
            first_blanked_trip: None,
            polls: 0,
        }
    }

    /// Whether ANY channel is monitored, i.e. whether this run has limits armed.
    ///
    /// A run with limits armed is one where a dropped raw frame could be the frame
    /// that carried a breach, which is why the ring-overrun rule binds exactly
    /// this case.
    pub fn limits_armed(&self) -> bool {
        !self.monitored.is_empty()
    }

    /// Number of monitored channels.
    pub fn monitored_channels(&self) -> usize {
        self.monitored.len()
    }

    /// `poll()` calls since construction.
    pub fn polls(&self) -> u64 {
        self.polls
    }

    /// Enter / leave the ARM BLANKING window.
    ///
    /// While blanked the watchdog still drains the ring and still runs every
    /// integrator - the HP filter's re-arm transient is exactly what the window
    /// exists to let settle, and it can only settle if the samples keep flowing -
    /// but a breach does not latch and does not call `emergency_stop`. It is
    /// counted, and the first one is kept for the operator.
    ///
    /// The window's LENGTH is not this type's business: the engine owns it and
    /// caps it (a blanking window that never ends is a disabled watchdog).
    pub fn set_blanked(&mut self, blanked: bool) {
        self.blanked = blanked;
    }

    /// Whether the blanking window is currently suppressing trips.
    pub fn is_blanked(&self) -> bool {
        self.blanked
    }

    /// Breaches observed while blanked, which would have tripped had the window
    /// already closed. A non-zero count is a WARNING the operator must see: it
    /// says the run was over a limit during arm settling.
    pub fn blanked_trips(&self) -> u32 {
        self.blanked_trips
    }

    /// The first breach observed while blanked.
    pub fn first_blanked_trip(&self) -> Option<AbortTrip> {
        self.first_blanked_trip
    }

    /// The per-limit-type latency budget this watchdog enforces (surface at `arm()`).
    pub fn latency_budget(&self) -> LatencyBudget {
        self.budget
    }

    /// Whether a trip has already fired (latched).
    pub fn tripped(&self) -> Option<AbortTrip> {
        self.tripped
    }

    /// How many times `emergency_stop()` was invoked (test observability).
    pub fn estop_calls(&self) -> u32 {
        self.estop_calls
    }

    /// Reset all integrators (call across an epoch boundary or marked gap so integration
    /// never spans a discontinuity). Does NOT clear a latched trip.
    pub fn reset_integrators(&mut self) {
        for m in &mut self.monitored {
            m.limiter.reset();
        }
    }

    /// Drain every available RAW frame, update the tri-field limits, and on a sustained
    /// breach call `backend.emergency_stop()` and latch the trip. Returns the trip if
    /// one fired on this poll. Idempotent once latched. This is the safety hot path:
    /// it visits the raw ring with a zero-copy drain and never allocates.
    pub fn poll(&mut self, backend: &mut dyn Backend) -> Option<AbortTrip> {
        self.polls += 1;
        if self.tripped.is_some() {
            // Latched: still drain so the ring does not overflow, but do not re-trip.
            self.raw.drain_each(|_| {});
            return self.tripped;
        }

        // Collect the first trip found in this drain (a mutable capture cannot escape
        // the closure cleanly, so record into locals and act after the drain).
        let mut trip: Option<AbortTrip> = None;
        let frame_len = self.frame_len;
        let mut frames_seen = self.frames_seen;
        let monitored = &mut self.monitored;

        self.raw.drain_each(|frame| {
            debug_assert_eq!(frame.len(), frame_len);
            frames_seen += 1;
            if trip.is_some() {
                return; // stop evaluating once a trip is found this drain
            }
            for m in monitored.iter_mut() {
                let accel = frame[m.frame_offset] as f64;
                // One shared tri-field check: accel direct-compare, then integrated
                // velocity, then displacement, with per-axis sustained-breach debounce.
                if let Some(t) = m.limiter.push(accel, frames_seen) {
                    trip = Some(t);
                    break;
                }
            }
        });

        self.frames_seen = frames_seen;

        if let Some(t) = trip {
            if self.blanked {
                // ARM BLANKING: observe, warn, do NOT act. The integrators above
                // already ran, so the settling this window exists for is happening;
                // the breach is recorded as a would-have-tripped warning and the
                // run continues. Nothing latches, so the watchdog arms fully the
                // instant the engine clears the flag.
                self.blanked_trips += 1;
                if self.first_blanked_trip.is_none() {
                    self.first_blanked_trip = Some(t);
                }
                return None;
            }
            // Fire the stop. We deliberately ignore a stop error's Ok/Err distinction
            // for the latch (the trip stands regardless); a failed stop is itself a
            // condition the caller escalates to the physical E-stop.
            let _ = backend.emergency_stop();
            self.estop_calls += 1;
            self.tripped = Some(t);
            return trip;
        }
        None
    }
}

// The per-axis debounced check (`check_axis`) and the streaming integrator now live in
// `crate::limits`, shared with the SMART run loop via [`ChannelLimiter`].

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring;
    use crate::test_backend::MockBackend;
    use hertzforge_channel_table::{AbortLimits, Channel};

    fn one_channel_table(limits: AbortLimits) -> ChannelTable {
        let mut t = ChannelTable::new();
        let mut c = Channel::new(1);
        c.abort = limits;
        c.role = ChannelRole::Control;
        t.channels.push(c);
        t
    }

    #[test]
    fn latency_budget_orders_by_integrator_depth() {
        let b = LatencyBudget::conservative();
        assert!(b.accel_frames < b.vel_frames);
        assert!(b.vel_frames < b.disp_frames);
        assert_eq!(b.worst_case_frames(), b.disp_frames);
    }

    #[test]
    fn accel_over_limit_trips_and_calls_estop() {
        let table = one_channel_table(AbortLimits {
            accel_g: Some(10.0),
            velocity_ips: None,
            displacement_mil: None,
        });
        let (p, c) = ring::channel(16, 1);
        let mut wd = AbortWatchdog::new(&table, c, 1000.0, 5.0, 1);
        let mut backend = MockBackend::new(1);

        // Below-limit frames: no trip.
        for _ in 0..5 {
            p.push(&[3.0]);
        }
        assert!(wd.poll(&mut backend).is_none());
        assert_eq!(backend.estop_count(), 0);

        // A spike above the limit: single-frame sustain trips immediately.
        p.push(&[42.0]);
        let trip = wd.poll(&mut backend).expect("should trip");
        assert_eq!(trip.axis, AbortAxis::Accel);
        assert_eq!(trip.channel_index, 1);
        assert_eq!(backend.estop_count(), 1);
        assert!(wd.tripped().is_some());
    }

    #[test]
    fn sustain_debounce_ignores_single_stray_sample() {
        let table = one_channel_table(AbortLimits {
            accel_g: Some(10.0),
            velocity_ips: None,
            displacement_mil: None,
        });
        let (p, c) = ring::channel(16, 1);
        let mut wd = AbortWatchdog::new(&table, c, 1000.0, 5.0, 3); // needs 3 consecutive
        let mut backend = MockBackend::new(1);

        p.push(&[50.0]); // one stray over-limit sample
        p.push(&[1.0]); // back under: counter resets
        assert!(wd.poll(&mut backend).is_none());
        assert_eq!(backend.estop_count(), 0);

        // Now three consecutive over-limit samples: trips on the third.
        p.push(&[50.0]);
        p.push(&[50.0]);
        p.push(&[50.0]);
        assert!(wd.poll(&mut backend).is_some());
        assert_eq!(backend.estop_count(), 1);
    }

    #[test]
    fn latched_trip_does_not_refire() {
        let table = one_channel_table(AbortLimits {
            accel_g: Some(10.0),
            velocity_ips: None,
            displacement_mil: None,
        });
        let (p, c) = ring::channel(16, 1);
        let mut wd = AbortWatchdog::new(&table, c, 1000.0, 5.0, 1);
        let mut backend = MockBackend::new(1);
        p.push(&[99.0]);
        wd.poll(&mut backend);
        p.push(&[99.0]);
        wd.poll(&mut backend);
        assert_eq!(backend.estop_count(), 1, "estop fires exactly once");
    }

    #[test]
    fn velocity_limit_trips_on_integrated_signal() {
        // A sinusoidal acceleration well above the HP corner integrates to a sinusoidal
        // velocity. A 2 g, 50 Hz sine integrates to a velocity of amplitude
        // ~ (2*386.09)/(2*pi*50) ~= 2.46 in/s per unit... peak measured ~4.7 in/s, so a
        // 4.0 in/s limit trips even though the per-sample accel (2 g) is under any
        // plausible g limit. This proves the vel PATH (integrator group delay budgeted
        // separately) is what catches this class, not the direct g compare.
        let table = one_channel_table(AbortLimits {
            accel_g: None,
            velocity_ips: Some(4.0),
            displacement_mil: None,
        });
        let fs = 1000.0;
        let (p, c) = ring::channel(4096, 1);
        let mut wd = AbortWatchdog::new(&table, c, fs, 1.0, 1);
        let mut backend = MockBackend::new(1);
        for n in 0..500 {
            let t = n as f64 / fs;
            let accel = 2.0 * (2.0 * std::f64::consts::PI * 50.0 * t).sin();
            p.push(&[accel as f32]);
        }
        let trip = wd.poll(&mut backend).expect("velocity should trip");
        assert_eq!(trip.axis, AbortAxis::Velocity);
        assert_eq!(backend.estop_count(), 1);
    }

    #[test]
    fn channel_without_limits_is_not_monitored() {
        let table = one_channel_table(AbortLimits::default()); // all None
        let (p, c) = ring::channel(16, 1);
        let mut wd = AbortWatchdog::new(&table, c, 1000.0, 5.0, 1);
        let mut backend = MockBackend::new(1);
        for _ in 0..10 {
            p.push(&[1e6]); // enormous, but no limit set
        }
        assert!(wd.poll(&mut backend).is_none());
        assert_eq!(backend.estop_count(), 0);
    }
}
