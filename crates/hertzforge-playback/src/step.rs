//! The process step the engine ticks: pre-roll at zero, the file at the level, post-roll
//! at zero; the stop ramp multiplying the streamed drive; the abort to zero at once.
//!
//! Every sample the step writes is judged against the drive ceiling before it leaves:
//! a sample over it zeroes the drive from that sample on and raises
//! [`StepShared::ceiling_trip`]. The ring empty before the file's end
//! ([`StepShared::underrun`]), the reader failed ([`crate::source::SourceShared::failed`])
//! or an abort asked ([`StepShared::abort`]) zero the drive at once, in the same block.
//! The run host answers each with the emergency stop between batches. The step does
//! no file work, takes no lock and allocates nothing after it is built.

use crate::source::RingReader;
use hertzforge_engine::ProcessStep;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

/// Where the play is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    PreRoll = 0,
    Playing = 1,
    PostRoll = 2,
    /// The post-roll has been generated: the drive is zero for good.
    Done = 3,
    /// The stop ramp is running.
    Stopping = 4,
    /// The stop ramp has ended: the drive is zero for good.
    Stopped = 5,
    /// Zeroed at once: an abort, an underrun, a ceiling trip or the reader's failure.
    Zeroed = 6,
}

impl Phase {
    pub fn from_u8(x: u8) -> Phase {
        match x {
            0 => Phase::PreRoll,
            1 => Phase::Playing,
            2 => Phase::PostRoll,
            3 => Phase::Done,
            4 => Phase::Stopping,
            5 => Phase::Stopped,
            _ => Phase::Zeroed,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Phase::PreRoll => "pre_roll",
            Phase::Playing => "playing",
            Phase::PostRoll => "post_roll",
            Phase::Done => "done",
            Phase::Stopping => "stopping",
            Phase::Stopped => "stopped",
            Phase::Zeroed => "zeroed",
        }
    }

    /// The drive is zero from here on.
    pub fn is_quiet(self) -> bool {
        matches!(self, Phase::Done | Phase::Stopped | Phase::Zeroed)
    }
}

/// What the step and the run host share: the asks in, the facts out.
#[derive(Debug, Default)]
pub struct StepShared {
    pub stop: AtomicBool,
    pub abort: AtomicBool,
    pub underrun: AtomicBool,
    pub ceiling_trip: AtomicBool,
    /// The file channel and the value (f64 bits) of a ceiling trip.
    pub trip_channel: AtomicU32,
    pub trip_value: AtomicU64,
    /// Frames generated.
    pub generated: AtomicU64,
    /// File frames played.
    pub played: AtomicU64,
    /// The generated frame from which the drive is zero for good.
    pub quiet_from: AtomicU64,
    /// The generated frame the stop ramp began at.
    pub stop_from: AtomicU64,
    phase: AtomicU8,
}

impl StepShared {
    pub fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Ordering::Acquire))
    }
}

/// How the step is built.
#[derive(Debug, Clone, PartialEq)]
pub struct StepBuild {
    pub out_channels: usize,
    /// The output slot each file channel plays on.
    pub slots: Vec<usize>,
    /// The file's frames.
    pub samples: u64,
    pub gain: f64,
    pub ceiling_v: f64,
    pub pre_roll_frames: u64,
    pub post_roll_frames: u64,
    pub stop_ramp_frames: u64,
    pub stop_ramp_depth_db: f64,
    /// Every this many frames a frame of inputs and drives goes to the display ring.
    pub display_every: u64,
}

/// The step.
pub struct PlaybackStep {
    b: StepBuild,
    ring: RingReader,
    shared: Arc<StepShared>,
    display: Option<hertzforge_engine::ring::Producer>,
    phase: Phase,
    pre_left: u64,
    post_left: u64,
    ramp_at: u64,
    generated: u64,
    played: u64,
    frame: Vec<f32>,
    out: Vec<f32>,
    shown: Vec<f32>,
}

impl PlaybackStep {
    pub fn new(
        b: StepBuild,
        ring: RingReader,
        display: Option<hertzforge_engine::ring::Producer>,
    ) -> (PlaybackStep, Arc<StepShared>) {
        let shared = Arc::new(StepShared::default());
        let width = b.slots.len();
        let out = b.out_channels.max(1);
        let shown = display.as_ref().map_or(0, |d| d.frame_len());
        let step = PlaybackStep {
            pre_left: b.pre_roll_frames,
            post_left: b.post_roll_frames,
            b,
            ring,
            shared: shared.clone(),
            display,
            phase: Phase::PreRoll,
            ramp_at: 0,
            generated: 0,
            played: 0,
            frame: vec![0.0; width],
            out: vec![0.0; out],
            shown: vec![0.0; shown],
        };
        step.set_phase(Phase::PreRoll);
        (step, shared)
    }

    fn set_phase(&self, p: Phase) {
        if p.is_quiet() && !self.phase.is_quiet() {
            self.shared
                .quiet_from
                .store(self.generated, Ordering::Release);
        }
        self.shared.phase.store(p as u8, Ordering::Release);
    }

    fn enter(&mut self, p: Phase) {
        self.set_phase(p);
        self.phase = p;
    }

    /// Zero the drive at once.
    fn zero(&mut self) {
        self.enter(Phase::Zeroed);
    }

    /// The stop ramp's gain at frame `k` of the ramp.
    fn ramp_gain(&self, k: u64) -> f64 {
        let n = self.b.stop_ramp_frames.max(1);
        10f64.powf(-self.b.stop_ramp_depth_db * (k as f64 / n as f64) / 20.0)
    }

    /// One frame of the file into `self.out`, scaled by `gain`, or `false` with the
    /// drive zeroed (an underrun or a ceiling trip).
    fn play_frame(&mut self, gain: f64) -> bool {
        if !self.ring.pop(&mut self.frame) {
            if self.played < self.b.samples {
                self.shared.underrun.store(true, Ordering::Release);
                self.zero();
            }
            return false;
        }
        self.played += 1;
        for (k, slot) in self.b.slots.iter().enumerate() {
            let v = f64::from(self.frame[k]) * gain;
            if v.abs() > self.b.ceiling_v || !v.is_finite() {
                self.shared.trip_channel.store(k as u32, Ordering::Relaxed);
                self.shared.trip_value.store(v.to_bits(), Ordering::Relaxed);
                self.shared.ceiling_trip.store(true, Ordering::Release);
                self.zero();
                return false;
            }
            if let Some(o) = self.out.get_mut(*slot) {
                *o = v as f32;
            }
        }
        true
    }

    fn next_frame(&mut self) {
        self.out.iter_mut().for_each(|o| *o = 0.0);
        match self.phase {
            Phase::PreRoll => {
                if self.pre_left > 0 {
                    self.pre_left -= 1;
                } else {
                    self.enter(Phase::Playing);
                    self.next_frame();
                }
            }
            Phase::Playing => {
                if self.played >= self.b.samples {
                    self.enter(Phase::PostRoll);
                    self.next_frame();
                    return;
                }
                let _ = self.play_frame(self.b.gain);
            }
            Phase::Stopping => {
                if self.ramp_at >= self.b.stop_ramp_frames || self.played >= self.b.samples {
                    self.enter(Phase::Stopped);
                } else {
                    let g = self.b.gain * self.ramp_gain(self.ramp_at);
                    self.ramp_at += 1;
                    let _ = self.play_frame(g);
                }
            }
            Phase::PostRoll => {
                if self.post_left > 0 {
                    self.post_left -= 1;
                } else {
                    self.enter(Phase::Done);
                }
            }
            Phase::Done | Phase::Stopped | Phase::Zeroed => {}
        }
    }
}

impl ProcessStep for PlaybackStep {
    fn process(&mut self, input: &[f32], n_frames: usize, drive_out: &mut Vec<f32>) {
        // The engine hands the same buffer every tick: this block's drive only.
        drive_out.clear();
        let s = self.shared.clone();
        if s.abort.load(Ordering::Acquire) || self.ring.shared().failed.load(Ordering::Acquire) {
            if self.phase != Phase::Zeroed {
                self.zero();
            }
        } else if s.stop.load(Ordering::Acquire) {
            match self.phase {
                Phase::Playing => {
                    self.shared
                        .stop_from
                        .store(self.generated, Ordering::Release);
                    self.ramp_at = 0;
                    self.enter(Phase::Stopping);
                }
                Phase::PreRoll | Phase::PostRoll => self.enter(Phase::Stopped),
                _ => {}
            }
        }
        let in_w = input.len().checked_div(n_frames).unwrap_or(0);
        for f in 0..n_frames {
            self.next_frame();
            drive_out.extend_from_slice(&self.out);
            self.generated += 1;
            if let Some(d) = &self.display {
                if self.b.display_every > 0 && self.generated.is_multiple_of(self.b.display_every) {
                    let w = self.shown.len();
                    let row = &input[f * in_w..(f + 1) * in_w];
                    for (k, x) in row.iter().chain(self.out.iter()).take(w).enumerate() {
                        self.shown[k] = *x;
                    }
                    let _ = d.push(&self.shown);
                }
            }
        }
        self.ring.release();
        s.generated.store(self.generated, Ordering::Release);
        s.played.store(self.played, Ordering::Release);
    }
}
