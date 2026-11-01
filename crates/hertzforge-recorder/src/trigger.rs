//! Trigger engine + pre-trigger ring, frame-domain, EU samples.
//!
//! [`TriggerEngine::feed`] consumes interleaved EU frames, maintains the
//! pre-trigger ring, and fires [`TriggerEvent`]s from a hysteresis-ARMED level
//! detector: for a Rising slope the detector arms when the signal goes BELOW
//! `threshold - hysteresis` and fires on crossing `threshold` (mirror-image for
//! Falling). A signal chattering inside the hysteresis band around the threshold
//! can therefore fire at most once - no noise re-triggering.
//!
//! # Rearm holdoff
//!
//! After a fire, the detector cannot re-arm until BOTH the configured holdoff
//! AND at least one full ring depth of frames have elapsed. The ring-depth floor
//! is load-bearing: the next fire's pre-trigger window is the ring's contents,
//! and if a fire were allowed before the ring fully refilled, that pre-window
//! would duplicate the tail of the PRIOR recording (frames already written to
//! the previous run). Holdoff >= ring depth guarantees every pre-window is made
//! of frames that belong to no earlier run.

use serde::{Deserialize, Serialize};

/// Default pre-trigger depth in seconds.
pub const DEFAULT_PRETRIGGER_S: f64 = 1.0;

/// Maximum pre-trigger depth in seconds; larger requests clamp here.
pub const MAX_PRETRIGGER_S: f64 = 10.0;

/// Level-trigger slope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Slope {
    /// Fire when the signal crosses the threshold going up.
    Rising,
    /// Fire when the signal crosses the threshold going down.
    Falling,
}

/// Level-trigger parameters. All values are in the trigger channel's EU.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LevelTrigger {
    /// 0-based channel index into the interleaved frame.
    pub channel_index: u32,
    /// The firing threshold in EU.
    pub threshold_eu: f64,
    /// Crossing direction.
    pub slope: Slope,
    /// Hysteresis band in EU: the detector must retreat past
    /// `threshold -/+ hysteresis` (Rising/Falling) before it arms. Default is
    /// 1% of the channel range - see [`LevelTrigger::default_hysteresis_eu`].
    pub hysteresis_eu: f64,
    /// Minimum seconds between a fire and the next allowed re-arm. The engine
    /// additionally floors this at one ring depth (see module docs).
    pub holdoff_s: f64,
}

impl LevelTrigger {
    /// The default hysteresis: 1% of the caller-supplied channel range (EU).
    /// The range comes from the channel table (`range_max`); this crate does not
    /// assume one.
    pub fn default_hysteresis_eu(range_eu: f64) -> f64 {
        0.01 * range_eu
    }

    /// A level trigger with the default 1%-of-range hysteresis and no extra
    /// holdoff (the engine's ring-depth floor still applies).
    pub fn new(channel_index: u32, threshold_eu: f64, slope: Slope, range_eu: f64) -> Self {
        Self {
            channel_index,
            threshold_eu,
            slope,
            hysteresis_eu: Self::default_hysteresis_eu(range_eu),
            holdoff_s: 0.0,
        }
    }
}

/// How recordings start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TriggerMode {
    /// No trigger: the recording runs whenever the operator starts it.
    FreeRun,
    /// Operator-initiated: fires only via [`TriggerEngine::manual_fire`].
    Manual,
    /// Hysteresis-armed level detection on one channel.
    Level(LevelTrigger),
}

/// A fired trigger: where it fired (global frame counter across all feeds) and
/// the pre-trigger window captured at that instant.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerEvent {
    /// The global index (frames since the engine was created) of the trigger
    /// frame - the first frame of the "live" part of the recording. The
    /// pre-trigger window contains frames strictly BEFORE this index.
    pub trigger_frame_global: u64,
    /// Interleaved pre-trigger frames, oldest to newest, ending at the frame
    /// just before `trigger_frame_global`. Joins gap-free with the live frames.
    pub pretrigger: Vec<f32>,
    /// Frame count of `pretrigger` (`pretrigger.len() / n_channels`).
    pub pretrigger_frames: u64,
}

/// A fixed-size interleaved f32 ring holding the most recent
/// `capacity_frames` frames.
#[derive(Debug, Clone)]
pub struct PreTriggerRing {
    n_channels: usize,
    capacity_frames: usize,
    /// Interleaved storage, `capacity_frames * n_channels` long once touched.
    buf: Vec<f32>,
    /// Next write position in FRAMES (wraps at `capacity_frames`).
    write_frame: usize,
    /// Frames currently held (saturates at `capacity_frames`).
    len_frames: usize,
}

impl PreTriggerRing {
    /// A ring holding up to `capacity_frames` frames of `n_channels` channels.
    /// `n_channels` must be nonzero; a zero-capacity ring is valid (holds
    /// nothing, snapshots empty).
    pub fn new(n_channels: usize, capacity_frames: usize) -> Self {
        assert!(n_channels > 0, "PreTriggerRing needs at least one channel");
        Self {
            n_channels,
            capacity_frames,
            buf: vec![0.0; capacity_frames * n_channels],
            write_frame: 0,
            len_frames: 0,
        }
    }

    /// Push interleaved frames (`frames.len()` must be a whole number of
    /// frames). Older frames fall out once the ring is full; pushing a slice
    /// longer than the capacity keeps only its newest `capacity_frames` frames.
    pub fn push_frames(&mut self, frames: &[f32]) {
        debug_assert_eq!(
            frames.len() % self.n_channels,
            0,
            "push_frames slice must be a whole number of interleaved frames"
        );
        if self.capacity_frames == 0 {
            return;
        }
        let n_frames = frames.len() / self.n_channels;
        // Only the newest `capacity_frames` frames of the input can survive.
        let keep = n_frames.min(self.capacity_frames);
        let skip_frames = n_frames - keep;
        let src = &frames[skip_frames * self.n_channels..];
        for f in 0..keep {
            let dst = self.write_frame * self.n_channels;
            self.buf[dst..dst + self.n_channels]
                .copy_from_slice(&src[f * self.n_channels..(f + 1) * self.n_channels]);
            self.write_frame = (self.write_frame + 1) % self.capacity_frames;
        }
        self.len_frames = (self.len_frames + n_frames).min(self.capacity_frames);
    }

    /// The ring contents oldest to newest as one contiguous interleaved vector.
    pub fn snapshot(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.len_frames * self.n_channels);
        if self.len_frames == 0 {
            return out;
        }
        // Oldest frame position: write_frame - len_frames (mod capacity).
        let start =
            (self.write_frame + self.capacity_frames - self.len_frames) % self.capacity_frames;
        for f in 0..self.len_frames {
            let src = ((start + f) % self.capacity_frames) * self.n_channels;
            out.extend_from_slice(&self.buf[src..src + self.n_channels]);
        }
        out
    }

    /// Frames currently held.
    pub fn len_frames(&self) -> usize {
        self.len_frames
    }

    /// The fixed capacity in frames.
    pub fn capacity_frames(&self) -> usize {
        self.capacity_frames
    }
}

/// Detector arming state for the level trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArmState {
    /// Waiting for the signal to retreat past the hysteresis bound.
    Disarmed,
    /// Armed: the next threshold crossing fires.
    Armed,
    /// Fired with no rearms remaining; only `manual_fire` or `rearm` can fire again.
    Exhausted,
}

/// The trigger engine: pre-trigger ring + hysteresis-armed level detection +
/// holdoff + auto-rearm, over a global frame counter that runs across feeds.
#[derive(Debug, Clone)]
pub struct TriggerEngine {
    mode: TriggerMode,
    n_channels: usize,
    ring: PreTriggerRing,
    /// Global frames consumed so far (index of the NEXT frame to arrive).
    global_frame: u64,
    arm: ArmState,
    /// Re-arm suppressed until the global counter reaches this frame.
    holdoff_until_frame: u64,
    /// Holdoff in frames = max(holdoff_s * fs, ring depth). See module docs.
    holdoff_frames: u64,
    /// Automatic re-arms remaining after a fire.
    rearms_remaining: u32,
}

impl TriggerEngine {
    /// Create an engine.
    ///
    /// * `pretrigger_s` - pre-trigger depth in seconds; values above
    ///   [`MAX_PRETRIGGER_S`] clamp to it, negatives/NaN clamp to zero
    ///   ([`DEFAULT_PRETRIGGER_S`] is the conventional default).
    /// * `auto_rearm` - how many times the engine automatically re-arms AFTER
    ///   its first fire (so `auto_rearm + 1` level fires are possible in total).
    ///   Each fire counts one down; [`TriggerEngine::rearm`] resets the budget.
    pub fn new(
        mode: TriggerMode,
        n_channels: usize,
        fs_hz: f64,
        pretrigger_s: f64,
        auto_rearm: u32,
    ) -> Self {
        assert!(n_channels > 0, "TriggerEngine needs at least one channel");
        assert!(
            fs_hz > 0.0 && !fs_hz.is_nan(),
            "TriggerEngine needs a positive sample rate"
        );
        let pretrigger_s = if pretrigger_s.is_nan() || pretrigger_s < 0.0 {
            0.0
        } else {
            pretrigger_s.min(MAX_PRETRIGGER_S)
        };
        let capacity_frames = (pretrigger_s * fs_hz).round() as usize;
        let holdoff_s = match &mode {
            TriggerMode::Level(lt) if lt.holdoff_s > 0.0 => lt.holdoff_s,
            _ => 0.0,
        };
        // Rearm holdoff >= ring depth so a pre-window can never
        // duplicate a prior run's tail.
        let holdoff_frames = ((holdoff_s * fs_hz).ceil() as u64).max(capacity_frames as u64);
        Self {
            mode,
            n_channels,
            ring: PreTriggerRing::new(n_channels, capacity_frames),
            global_frame: 0,
            arm: ArmState::Disarmed,
            holdoff_until_frame: 0,
            holdoff_frames,
            rearms_remaining: auto_rearm,
        }
    }

    /// The pre-trigger ring depth in frames (after clamping).
    pub fn ring_capacity_frames(&self) -> usize {
        self.ring.capacity_frames()
    }

    /// Frames consumed so far across all feeds.
    pub fn global_frame(&self) -> u64 {
        self.global_frame
    }

    /// Automatic re-arms remaining.
    pub fn rearms_remaining(&self) -> u32 {
        self.rearms_remaining
    }

    /// Manually reset the auto-rearm budget and disarm the detector (it must
    /// re-arm through the hysteresis bound as usual). Clears any exhaustion.
    pub fn rearm(&mut self, auto_rearm: u32) {
        self.rearms_remaining = auto_rearm;
        self.arm = ArmState::Disarmed;
    }

    /// Fire NOW regardless of mode: snapshot the ring as the pre-window and
    /// stamp the trigger at the current global frame (the next frame to be fed
    /// is the first live frame). This is the `Manual` mode path, and also an
    /// operator override for `Level` mode.
    pub fn manual_fire(&mut self) -> TriggerEvent {
        let pretrigger = self.ring.snapshot();
        let pretrigger_frames = (pretrigger.len() / self.n_channels) as u64;
        self.begin_holdoff();
        TriggerEvent {
            trigger_frame_global: self.global_frame,
            pretrigger,
            pretrigger_frames,
        }
    }

    /// Feed interleaved EU frames (`frames[..n_frames * n_channels]` is
    /// consumed). Updates the ring and the global counter; in `Level` mode runs
    /// the hysteresis-armed detector per frame and returns the FIRST fire in
    /// the block, with the exact trigger frame identified.
    ///
    /// At most one event per call: after a fire the holdoff (>= one ring depth)
    /// suppresses re-arming, so a second fire cannot occur within any block
    /// shorter than the ring depth.
    pub fn feed(&mut self, frames: &[f32], n_frames: usize) -> Option<TriggerEvent> {
        debug_assert!(
            frames.len() >= n_frames * self.n_channels,
            "feed slice shorter than n_frames * n_channels"
        );
        let level = match &self.mode {
            TriggerMode::Level(lt) => Some(*lt),
            TriggerMode::FreeRun | TriggerMode::Manual => None,
        };
        let mut event = None;
        for f in 0..n_frames {
            let frame = &frames[f * self.n_channels..(f + 1) * self.n_channels];
            if event.is_none() {
                if let Some(lt) = &level {
                    let sample = f64::from(frame[lt.channel_index as usize]);
                    if self.level_step(lt, sample) {
                        // Fire BEFORE pushing this frame: the ring holds frames
                        // strictly before the trigger frame, so the pre-window
                        // + live-from-trigger join is gap-free.
                        let pretrigger = self.ring.snapshot();
                        let pretrigger_frames = (pretrigger.len() / self.n_channels) as u64;
                        event = Some(TriggerEvent {
                            trigger_frame_global: self.global_frame,
                            pretrigger,
                            pretrigger_frames,
                        });
                    }
                }
            }
            self.ring.push_frames(frame);
            self.global_frame += 1;
        }
        event
    }

    /// One detector step on the trigger-channel sample. Returns true on fire.
    fn level_step(&mut self, lt: &LevelTrigger, sample: f64) -> bool {
        match self.arm {
            ArmState::Exhausted => false,
            ArmState::Disarmed => {
                // Holdoff gate: no re-arming until the ring has refilled AND
                // the configured holdoff has elapsed.
                if self.global_frame < self.holdoff_until_frame {
                    return false;
                }
                let arms = match lt.slope {
                    Slope::Rising => sample < lt.threshold_eu - lt.hysteresis_eu,
                    Slope::Falling => sample > lt.threshold_eu + lt.hysteresis_eu,
                };
                if arms {
                    self.arm = ArmState::Armed;
                }
                false
            }
            ArmState::Armed => {
                let fires = match lt.slope {
                    Slope::Rising => sample >= lt.threshold_eu,
                    Slope::Falling => sample <= lt.threshold_eu,
                };
                if fires {
                    self.begin_holdoff();
                }
                fires
            }
        }
    }

    /// Post-fire bookkeeping: start the holdoff window and count the rearm
    /// budget down (or exhaust).
    fn begin_holdoff(&mut self) {
        self.holdoff_until_frame = self.global_frame + self.holdoff_frames;
        if self.rearms_remaining > 0 {
            self.rearms_remaining -= 1;
            self.arm = ArmState::Disarmed;
        } else {
            self.arm = ArmState::Exhausted;
        }
    }
}
