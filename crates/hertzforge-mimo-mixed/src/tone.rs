//! The multi-drive tone bank and the residual (design D3, D5, D13).
//!
//! `K` phase-continuous oscillators, each played on every drive with a
//! complex amplitude: drive `j` plays `Re{ D_j exp(j phi) } = |D_j|
//! cos(phi + arg D_j)` (the `sine-mimo` world's convention). A follower's
//! phase is its leader's times the ratio (theory 4.5, MV14), so harmonics
//! stay locked; the phase is kept as whole turns plus a fraction so the
//! product does not lose precision over a long run. Each drive's amplitude
//! and phase move through a ramp (MIMO Sine's drive ramp, the shorter way
//! round in phase), and each tone's part level through single-output Mixed
//! Mode's [`Ramp`].
//!
//! A tracker per tone per channel (controls then limits) demodulates the
//! response against the tone's phase played `lag` frames before, with the
//! phase advanced a quarter turn so the tracked complex amplitude `C` is
//! defined by `y = Re{ C exp(j phi) }`: on a linear structure in steady
//! state `C = H D` (MIMO Sine's core does the same). Each tracker's passed
//! waveform is subtracted from its channel to give the residual.
//!
//! [`ToneBank::drive_frame`] and [`ToneBank::track_frame`] do not allocate.

use hertzforge_dsp::tone::ToneTracker;
pub use hertzforge_mixed::tone::{
    tone_judging, tone_loop_gain_bound, tone_readable, Ramp, ToneJudging, MIN_TONE_SNR,
    TONE_JUDGE_CAP_S, TONE_JUDGE_SHARE, TONE_JUDGE_SIGMAS,
};
use hertzforge_sdm::C64;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

/// One drive's amplitude and phase on its way to a target over a number of
/// frames, the phase the shorter way round (MIMO Sine's drive ramp, copied:
/// that one is private to MIMO Sine's service).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriveRamp {
    mag: f64,
    mag_step: f64,
    ph: f64,
    ph_step: f64,
    left: u64,
    target: C64,
}

impl DriveRamp {
    /// A ramp standing at `d`.
    pub fn at(d: C64) -> Self {
        DriveRamp {
            mag: d.norm(),
            mag_step: 0.0,
            ph: d.arg(),
            ph_step: 0.0,
            left: 0,
            target: d,
        }
    }

    /// Go to `to` over `frames` frames (at once for 0).
    pub fn go(&mut self, to: C64, frames: u64) {
        self.target = to;
        if frames == 0 {
            self.land();
            return;
        }
        let to_mag = to.norm();
        // A zero end keeps the phase it has; a zero start takes the target's.
        let to_ph = if to_mag > 0.0 { to.arg() } else { self.ph };
        if self.mag == 0.0 {
            self.ph = to_ph;
        }
        let mut d = (to_ph - self.ph).rem_euclid(TAU);
        if d > PI {
            d -= TAU;
        }
        self.mag_step = (to_mag - self.mag) / frames as f64;
        self.ph_step = d / frames as f64;
        self.left = frames;
    }

    fn land(&mut self) {
        self.mag = self.target.norm();
        if self.mag > 0.0 {
            self.ph = self.target.arg();
        }
        self.mag_step = 0.0;
        self.ph_step = 0.0;
        self.left = 0;
    }

    /// Advance one frame; the amplitude and phase for it.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> (f64, f64) {
        if self.left > 0 {
            self.left -= 1;
            if self.left == 0 {
                self.land();
            } else {
                self.mag += self.mag_step;
                self.ph += self.ph_step;
            }
        }
        (self.mag, self.ph)
    }

    /// The value now.
    pub fn value(&self) -> C64 {
        C64::from_polar(self.mag, self.ph)
    }

    /// The target.
    pub fn target(&self) -> C64 {
        self.target
    }

    /// Whether it has arrived.
    pub fn settled(&self) -> bool {
        self.left == 0
    }
}

/// How a tone's phase is made.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToneLock {
    /// Integrated from its own frequency.
    Free,
    /// Its leader's phase (tone index, earlier in the bank) times `ratio`.
    Follow {
        /// The leader's index in the bank.
        leader: usize,
        /// The ratio.
        ratio: f64,
    },
}

/// One oscillator.
#[derive(Debug, Clone)]
struct Osc {
    lock: ToneLock,
    freq_hz: f64,
    /// Whole turns (free oscillators only).
    turns: i64,
    /// Fraction of a turn in `[0, 1)`.
    frac: f64,
    /// Total phase in turns as `(whole, fraction)` this frame.
    now: (i64, f64),
    part: Ramp,
    drives: Vec<DriveRamp>,
}

/// The tone bank.
#[derive(Debug, Clone)]
pub struct ToneBank {
    fs_hz: f64,
    drives: usize,
    channels: usize,
    lag: usize,
    bn_hz: f64,
    tones: Vec<Osc>,
    trackers: Vec<ToneTracker>,
    /// Phases played, radians wrapped, `(lag + 1)` per tone, a ring.
    ring: Vec<f64>,
    head: usize,
    frames: u64,
}

/// Why a bank cannot be built.
#[derive(Debug, Clone, PartialEq)]
pub struct ToneBankError(pub String);

impl std::fmt::Display for ToneBankError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ToneBankError {}

impl ToneBank {
    /// A bank at `fs_hz` of tones starting at `f0_hz` with `locks` (a
    /// follower's leader comes earlier in the list), on `drives` drives,
    /// tracked on `channels` channels (controls then limits) with trackers
    /// of noise bandwidth `bn_hz` reading `lag` frames behind the drive.
    /// Every tone starts silent (part level 0, zero drive).
    pub fn new(
        fs_hz: f64,
        f0_hz: &[f64],
        locks: &[ToneLock],
        drives: usize,
        channels: usize,
        bn_hz: f64,
        lag: usize,
    ) -> Result<Self, ToneBankError> {
        if locks.len() != f0_hz.len() {
            return Err(ToneBankError(format!(
                "{} locks for {} tones",
                locks.len(),
                f0_hz.len()
            )));
        }
        for (k, l) in locks.iter().enumerate() {
            if let ToneLock::Follow { leader, ratio } = *l {
                if leader >= k || !(ratio.is_finite() && ratio > 0.0) {
                    return Err(ToneBankError(format!(
                        "tone {k} follows tone {leader} at {ratio}: a leader comes first and a ratio is positive"
                    )));
                }
            }
        }
        let k = f0_hz.len();
        Ok(ToneBank {
            fs_hz,
            drives,
            channels,
            lag,
            bn_hz,
            tones: f0_hz
                .iter()
                .zip(locks)
                .map(|(&f, &lock)| Osc {
                    lock,
                    freq_hz: f,
                    turns: 0,
                    frac: 0.0,
                    now: (0, 0.0),
                    part: Ramp::at(0.0),
                    drives: vec![DriveRamp::at(C64::new(0.0, 0.0)); drives],
                })
                .collect(),
            trackers: vec![ToneTracker::new(fs_hz, bn_hz); k * channels],
            ring: vec![0.0; k * (lag + 1)],
            head: 0,
            frames: 0,
        })
    }

    /// Tones.
    pub fn len(&self) -> usize {
        self.tones.len()
    }

    /// Whether there are no tones.
    pub fn is_empty(&self) -> bool {
        self.tones.is_empty()
    }

    /// Drives.
    pub fn drives(&self) -> usize {
        self.drives
    }

    /// Tracked channels.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The pipeline lag, frames.
    pub fn lag(&self) -> usize {
        self.lag
    }

    /// The trackers' noise bandwidth, Hz.
    pub fn bn_hz(&self) -> f64 {
        self.bn_hz
    }

    /// Frames played.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Set a free tone's frequency, Hz (a follower's is its leader's times
    /// the ratio and is set through its leader).
    pub fn set_frequency(&mut self, k: usize, f_hz: f64) {
        self.tones[k].freq_hz = f_hz;
    }

    /// Tone `k`'s frequency now, Hz.
    pub fn frequency(&self, k: usize) -> f64 {
        match self.tones[k].lock {
            ToneLock::Free => self.tones[k].freq_hz,
            ToneLock::Follow { leader, ratio } => ratio * self.frequency(leader),
        }
    }

    /// Ramp tone `k`'s drive vector (one complex amplitude per drive, peak
    /// volts at full level) to `d` over `frames` frames.
    pub fn set_drive(&mut self, k: usize, d: &[C64], frames: u64) {
        for (r, &v) in self.tones[k].drives.iter_mut().zip(d) {
            r.go(v, frames);
        }
    }

    /// Tone `k`'s drive vector in force, one per drive (into `out`).
    pub fn drive_in_force(&self, k: usize, out: &mut [C64]) {
        for (o, r) in out.iter_mut().zip(&self.tones[k].drives) {
            *o = r.value();
        }
    }

    /// Ramp tone `k`'s part level (0 off, 1 on) to `target` over `frames`.
    pub fn set_part_level(&mut self, k: usize, target: f64, frames: f64) {
        self.tones[k].part.go(target, frames);
    }

    /// Tone `k`'s part level now.
    pub fn part_level(&self, k: usize) -> f64 {
        self.tones[k].part.value()
    }

    /// Tone `k`'s phase this frame, radians in `[0, 2 pi)`.
    pub fn phase(&self, k: usize) -> f64 {
        TAU * self.tones[k].now.1
    }

    /// Tone `k`'s phase this frame as whole turns plus a fraction.
    pub fn phase_turns(&self, k: usize) -> (i64, f64) {
        self.tones[k].now
    }

    /// One drive frame: every tone advances, and `level` times each tone's
    /// part level times `Re{ D_j exp(j phi) }` is added into `out[j]` (one
    /// frame of `drives` values). Does not allocate.
    #[inline]
    pub fn drive_frame(&mut self, level: f64, out: &mut [f64]) {
        let w = self.lag + 1;
        for k in 0..self.tones.len() {
            let now = match self.tones[k].lock {
                ToneLock::Free => {
                    let o = &mut self.tones[k];
                    let now = (o.turns, o.frac);
                    let step = o.freq_hz / self.fs_hz;
                    let f = o.frac + step;
                    let whole = f.floor();
                    o.turns += whole as i64;
                    o.frac = f - whole;
                    now
                }
                ToneLock::Follow { leader, ratio } => {
                    let (t, f) = self.tones[leader].now;
                    let a = ratio * t as f64;
                    let b = ratio * f;
                    let x = (a - a.floor()) + (b - b.floor());
                    let whole = a.floor() + b.floor() + x.floor();
                    (whole as i64, x - x.floor())
                }
            };
            let o = &mut self.tones[k];
            o.now = now;
            let phase = TAU * now.1;
            self.ring[k * w + self.head] = phase;
            let g = level * o.part.next();
            for (j, r) in o.drives.iter_mut().enumerate() {
                let (mag, ph) = r.next();
                if g != 0.0 && mag != 0.0 {
                    out[j] += g * mag * (phase + ph).cos();
                }
            }
        }
        self.head = (self.head + 1) % w;
        self.frames += 1;
    }

    /// [`Self::drive_frame`] over a block: `out` holds whole frames of
    /// `drives` interleaved values; each frame's sum is added in.
    pub fn drive_block(&mut self, level: f64, out: &mut [f64]) {
        let m = self.drives.max(1);
        for frame in out.chunks_exact_mut(m) {
            self.drive_frame(level, frame);
        }
    }

    /// One response frame (`input[c]` per channel), after
    /// [`Self::drive_frame`] for the same frame: every tracker is fed the
    /// sample with its tone's phase played `lag` frames before, and each
    /// channel's residual (the sample less every tracker's passed waveform)
    /// is written into `residual`. Until `lag` frames have been played the
    /// trackers are not fed and the residual is the input. Does not allocate.
    #[inline]
    pub fn track_frame(&mut self, input: &[f64], residual: &mut [f64]) {
        let w = self.lag + 1;
        let ch = self.channels;
        residual[..ch].copy_from_slice(&input[..ch]);
        if self.frames <= self.lag as u64 {
            return;
        }
        let at = self.head;
        for k in 0..self.tones.len() {
            let phase = self.ring[k * w + at] + FRAC_PI_2;
            for c in 0..ch {
                let t = &mut self.trackers[k * ch + c];
                t.update(input[c], phase);
                residual[c] -= t.passed(phase);
            }
        }
    }

    /// Tone `k`'s tracked complex amplitude on channel `c`, peak:
    /// `y_c = Re{ C exp(j phi) }`.
    pub fn tracked(&self, k: usize, c: usize) -> C64 {
        let (re, im) = self.trackers[k * self.channels + c].complex();
        C64::new(re, im)
    }

    /// Tone `k`'s tracked vector over every channel (into `out`).
    pub fn tracked_vector(&self, k: usize, out: &mut [C64]) {
        for (c, o) in out.iter_mut().enumerate().take(self.channels) {
            *o = self.tracked(k, c);
        }
    }

    /// Whether every one of tone `k`'s trackers has settled.
    pub fn settled(&self, k: usize) -> bool {
        (0..self.channels).all(|c| self.trackers[k * self.channels + c].settled())
    }

    /// Restart tone `k`'s trackers' settle count (its frequency jumped).
    pub fn restart_settle(&mut self, k: usize) {
        for c in 0..self.channels {
            self.trackers[k * self.channels + c].restart_settle();
        }
    }

    /// Tone `k`'s tracker on channel `c`.
    pub fn tracker(&self, k: usize, c: usize) -> &ToneTracker {
        &self.trackers[k * self.channels + c]
    }
}

/// How each control's reading of a tone is judged: single-output Mixed
/// Mode's [`tone_judging`] per control, for a tone of peak `a[i]` over a
/// floor of density `g[i]` on control `i`.
pub fn judging_per_control(
    a: &[f64],
    g: &[f64],
    alarm_db: &[(f64, f64)],
    abort_db: &[(f64, f64)],
    bn_hz: f64,
) -> Vec<ToneJudging> {
    a.iter()
        .zip(g)
        .zip(alarm_db.iter().zip(abort_db))
        .map(|((&a, &g), (&al, &ab))| tone_judging(a, g, al, ab, bn_hz))
        .collect()
}

/// Whether a tone stands far enough over the floor to be read on every
/// control ([`tone_readable`] per control).
pub fn readable_on_every_control(a: &[f64], g: &[f64], bn_hz: f64) -> bool {
    a.iter().zip(g).all(|(&a, &g)| tone_readable(a, g, bn_hz))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_mixed_drive_ramp_takes_the_shorter_way() {
        let mut r = DriveRamp::at(C64::from_polar(1.0, 3.0));
        r.go(C64::from_polar(1.0, -3.0), 10);
        let (_, ph) = r.next();
        assert!(ph > 3.0, "{ph}");
        for _ in 0..9 {
            r.next();
        }
        assert!((r.value() - C64::from_polar(1.0, -3.0)).norm() < 1e-12);
        assert!(r.settled());
    }
}
