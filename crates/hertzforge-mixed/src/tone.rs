//! The tones: N phase-continuous oscillators, N trackers per control channel,
//! and the ramps every change of a part's level goes through.
//!
//! **Tracker.** Synchronous demodulation of the response against the tone's
//! own drive phase (order tracking along a known frequency history), retarded
//! by the measured pipeline lag so the reference is the phase that produced
//! the sample being read. The baseband low-pass is a cascade of
//! [`TRACKER_POLES`] identical one-pole sections, sized so the tracker's
//! **noise bandwidth** (the full width of the equivalent band-pass) is the
//! configured tracking width `B_n`: one reading of a tone of peak `A` in a
//! floor of one-sided density `G` scatters by `sqrt(G B_n)` and reads high by
//! `G B_n / (2 A)` (Rice; the brief's section 2.2). A second tone `D` away
//! passes at `1 / (1 + (D / f_c)^2)^2` with `f_c` the section corner, about
//! -27 dB at `D = 2 B_n`. Settling to within about 3 % of a step takes
//! [`SETTLE_WIDTHS`] over `B_n`.
//!
//! **Passed waveform.** The tracker's baseband phasor put back on the same
//! retarded phase is the tone as it stands in the response; subtracting it
//! leaves the floor and bands (design D5).
//!
//! **Oscillators** are Sine's [`SweepOscillator`]: phase-continuous for the
//! whole run, never rebuilt, every frequency change a change of increment only.
//! Each tone's amplitude moves sample by sample: toward the law's newest
//! amplitude linearly over one control tick (the slew), times its part ramp,
//! times the run's level ramp. Nothing in this module steps an amplitude.

use hertzforge_dsp::sweep::SweepOscillator;

/// Sections in the tracker's baseband low-pass.
pub const TRACKER_POLES: usize = 4;
/// A tracker's reading is usable this many `1 / B_n` after it starts (or
/// after its tone's frequency last jumped): a step has settled to within
/// about 3 %.
pub const SETTLE_WIDTHS: f64 = 1.3;
/// A tone's reading is used only when the tone stands this far above the
/// floor in its tracker's band: `A^2 / 2` over `G B_n` (the brief's 2.2 and 7).
pub const MIN_TONE_SNR: f64 = 10.0;

/// The noise gain `sum h^2` of [`TRACKER_POLES`] cascaded one-pole sections
/// over `alpha`, as `alpha` goes to zero: `(2n - 2)! / ((n - 1)!^2 2^(2n - 1))`.
const NOISE_GAIN_PER_ALPHA: f64 = 720.0 / (36.0 * 128.0);

/// One tone's tracker on one control channel.
#[derive(Debug, Clone)]
pub struct ToneTracker {
    alpha: f64,
    bn_hz: f64,
    fs_hz: f64,
    i: [f64; TRACKER_POLES],
    q: [f64; TRACKER_POLES],
    /// Samples fed since the last restart.
    fed: u64,
}

impl ToneTracker {
    /// A tracker at `fs_hz` of noise bandwidth `bn_hz` (full width).
    pub fn new(fs_hz: f64, bn_hz: f64) -> Self {
        let alpha = (bn_hz / (NOISE_GAIN_PER_ALPHA * fs_hz)).clamp(1e-12, 0.5);
        ToneTracker {
            alpha,
            bn_hz,
            fs_hz,
            i: [0.0; TRACKER_POLES],
            q: [0.0; TRACKER_POLES],
            fed: 0,
        }
    }

    /// The noise bandwidth this tracker was built for, Hz.
    pub fn bn_hz(&self) -> f64 {
        self.bn_hz
    }

    /// The exact noise bandwidth of the discrete filter, `fs sum h^2`, Hz,
    /// from its impulse response (for checks; not used per sample).
    pub fn exact_noise_bandwidth_hz(&self) -> f64 {
        let mut s = [0.0; TRACKER_POLES];
        let mut sum = 0.0;
        let n = (60.0 / self.alpha) as usize;
        for k in 0..n {
            let mut x = if k == 0 { 1.0 } else { 0.0 };
            for st in s.iter_mut() {
                *st += self.alpha * (x - *st);
                x = *st;
            }
            sum += x * x;
        }
        self.fs_hz * sum
    }

    /// Seconds after a restart before a reading is usable.
    pub fn settle_s(&self) -> f64 {
        SETTLE_WIDTHS / self.bn_hz
    }

    /// Feed one response sample with the reference phase that produced it.
    #[inline]
    pub fn update(&mut self, sample: f64, phase: f64) {
        let (s, c) = phase.sin_cos();
        let (mut xi, mut xq) = (2.0 * sample * s, 2.0 * sample * c);
        for k in 0..TRACKER_POLES {
            self.i[k] += self.alpha * (xi - self.i[k]);
            self.q[k] += self.alpha * (xq - self.q[k]);
            xi = self.i[k];
            xq = self.q[k];
        }
        self.fed += 1;
    }

    /// The tracked amplitude, peak.
    pub fn amplitude(&self) -> f64 {
        self.i[TRACKER_POLES - 1].hypot(self.q[TRACKER_POLES - 1])
    }

    /// The tracked phase against the reference, radians.
    pub fn phase(&self) -> f64 {
        self.q[TRACKER_POLES - 1].atan2(self.i[TRACKER_POLES - 1])
    }

    /// The tone as it stands in the response at reference phase `phase`.
    #[inline]
    pub fn passed(&self, phase: f64) -> f64 {
        let (s, c) = phase.sin_cos();
        self.i[TRACKER_POLES - 1] * s + self.q[TRACKER_POLES - 1] * c
    }

    /// Whether the tracker has run its settle time since its last restart.
    pub fn settled(&self) -> bool {
        self.fed as f64 >= self.settle_s() * self.fs_hz
    }

    /// Forget the settle count (the state is kept, so the passed waveform
    /// does not jump): used when a tone's frequency jumps.
    pub fn restart_settle(&mut self) {
        self.fed = 0;
    }
}

/// A linear ramp of a gain toward a target at a set rate, per sample.
/// Every change of a part's level goes through one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ramp {
    now: f64,
    target: f64,
    step: f64,
}

impl Ramp {
    /// A ramp standing at `value`.
    pub fn at(value: f64) -> Self {
        Ramp {
            now: value,
            target: value,
            step: 0.0,
        }
    }

    /// Go to `target` over `frames` samples (at once for 0 frames, which no
    /// caller in the run uses for a level that is playing).
    pub fn go(&mut self, target: f64, frames: f64) {
        self.target = target;
        self.step = if frames >= 1.0 {
            (target - self.now).abs() / frames
        } else {
            f64::INFINITY
        };
    }

    /// Advance one sample and return the value for it.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f64 {
        if self.now != self.target {
            let d = self.target - self.now;
            self.now = if d.abs() <= self.step {
                self.target
            } else {
                self.now + self.step.copysign(d)
            };
        }
        self.now
    }

    /// The value now.
    pub fn value(&self) -> f64 {
        self.now
    }

    /// The value it is going to.
    pub fn target(&self) -> f64 {
        self.target
    }

    /// The value it will have `frames` samples from now.
    pub fn ahead(&self, frames: f64) -> f64 {
        let d = self.target - self.now;
        if d.abs() <= self.step * frames.max(0.0) {
            self.target
        } else {
            self.now + (self.step * frames.max(0.0)).copysign(d)
        }
    }

    /// Whether it has arrived.
    pub fn settled(&self) -> bool {
        self.now == self.target
    }
}

/// One tone's voice: its oscillator, its amplitude slew and its part ramp.
#[derive(Debug, Clone)]
pub struct ToneVoice {
    osc: SweepOscillator,
    /// The amplitude, drive units peak at full level, slewed per sample.
    amp: Ramp,
    /// The part's ramp, 0 to 1.
    part: Ramp,
}

impl ToneVoice {
    /// A voice at `fs_hz` starting at `f0_hz`, silent and ramped out.
    pub fn new(fs_hz: f64, f0_hz: f64) -> Self {
        ToneVoice {
            osc: SweepOscillator::new(fs_hz, f0_hz.clamp(0.0, fs_hz * 0.499)),
            amp: Ramp::at(0.0),
            part: Ramp::at(0.0),
        }
    }

    /// The oscillator's frequency, Hz.
    pub fn frequency(&self) -> f64 {
        self.osc.frequency()
    }

    /// The commanded amplitude (where the slew is going), drive units peak.
    pub fn amplitude_cmd(&self) -> f64 {
        self.amp.target()
    }

    /// The amplitude now, drive units peak.
    pub fn amplitude_now(&self) -> f64 {
        self.amp.value()
    }

    /// The part ramp now, 0 to 1.
    pub fn scale(&self) -> f64 {
        self.part.value()
    }

    /// The part ramp `frames` samples from now.
    pub fn scale_ahead(&self, frames: f64) -> f64 {
        self.part.ahead(frames)
    }

    /// Whether the part ramp is at its target.
    pub fn ramp_settled(&self) -> bool {
        self.part.settled()
    }

    /// The part ramp's target.
    pub fn ramp_target(&self) -> f64 {
        self.part.target()
    }

    /// Move the amplitude to `a` (drive units peak) over `frames` samples.
    pub fn slew_to(&mut self, a: f64, frames: f64) {
        self.amp.go(a.max(0.0), frames);
    }

    /// Set the amplitude at once (before the voice plays).
    pub fn set_amplitude(&mut self, a: f64) {
        self.amp = Ramp::at(a.max(0.0));
    }

    /// Ramp the part in (1) or out (0) over `frames` samples.
    pub fn ramp_to(&mut self, scale: f64, frames: f64) {
        self.part.go(scale.clamp(0.0, 1.0), frames);
    }

    /// Set the frequency; refused (the voice keeps its frequency) outside the
    /// oscillator's range. Phase-continuous.
    pub fn set_frequency(&mut self, f_hz: f64) -> bool {
        self.osc.try_set_frequency(f_hz).is_ok()
    }

    /// One sample: `(value without the level gain, phase used)`.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> (f64, f64) {
        let (s, _, phase) = self.osc.next();
        let a = self.amp.next();
        let k = self.part.next();
        (a * k * s, phase)
    }
}

/// N tone voices, and N trackers per control channel reading the response
/// against each voice's retarded phase.
///
/// Per frame: [`Self::drive_frame`] steps every voice and records its phase;
/// [`Self::track_frame`] feeds each tracker the control sample with the phase
/// that was played `lag` frames before, and writes each channel's residual
/// (the sample less every tracker's passed waveform). Allocation-free after
/// construction; the work is linear in tones times channels.
#[derive(Debug, Clone)]
pub struct ToneBank {
    bn_hz: f64,
    voices: Vec<ToneVoice>,
    channels: usize,
    trackers: Vec<ToneTracker>,
    lag: usize,
    /// Phases, `(lag + 1)` per tone, a ring.
    ring: Vec<f64>,
    head: usize,
    frames: u64,
}

impl ToneBank {
    /// A bank at `fs_hz` for tones starting at `f0_hz`, with `channels`
    /// control channels, trackers of noise bandwidth `bn_hz`, reading the
    /// response `lag` frames behind the drive.
    pub fn new(fs_hz: f64, f0_hz: &[f64], channels: usize, bn_hz: f64, lag: usize) -> Self {
        let n = f0_hz.len();
        ToneBank {
            bn_hz,
            voices: f0_hz.iter().map(|&f| ToneVoice::new(fs_hz, f)).collect(),
            channels,
            trackers: vec![ToneTracker::new(fs_hz, bn_hz); n * channels],
            lag,
            ring: vec![0.0; n * (lag + 1)],
            head: 0,
            frames: 0,
        }
    }

    /// Tones.
    pub fn len(&self) -> usize {
        self.voices.len()
    }

    /// Whether there are no tones.
    pub fn is_empty(&self) -> bool {
        self.voices.is_empty()
    }

    /// The trackers' noise bandwidth, Hz.
    pub fn bn_hz(&self) -> f64 {
        self.bn_hz
    }

    /// Seconds a tracker needs to settle.
    pub fn tracker_settle_s(&self) -> f64 {
        SETTLE_WIDTHS / self.bn_hz
    }

    /// The pipeline lag the trackers read behind the drive, frames.
    pub fn lag(&self) -> usize {
        self.lag
    }

    /// Tone `i`'s voice.
    pub fn voice(&self, i: usize) -> &ToneVoice {
        &self.voices[i]
    }

    /// Tone `i`'s voice, to command.
    pub fn voice_mut(&mut self, i: usize) -> &mut ToneVoice {
        &mut self.voices[i]
    }

    /// Tone `i`'s tracker on control channel `c`.
    pub fn tracker(&self, i: usize, c: usize) -> &ToneTracker {
        &self.trackers[i * self.channels + c]
    }

    /// Restart tone `i`'s trackers' settle count (its frequency jumped).
    pub fn restart_settle(&mut self, i: usize) {
        for c in 0..self.channels {
            self.trackers[i * self.channels + c].restart_settle();
        }
    }

    /// One drive frame: every voice steps (at `level` times its own
    /// amplitude and ramp); returns the tones' sum.
    #[inline]
    pub fn drive_frame(&mut self, level: f64) -> f64 {
        let w = self.lag + 1;
        let mut sum = 0.0;
        for (i, v) in self.voices.iter_mut().enumerate() {
            let (x, phase) = v.next();
            self.ring[i * w + self.head] = phase;
            sum += x;
        }
        self.head = (self.head + 1) % w;
        self.frames += 1;
        level * sum
    }

    /// One response frame (`input[c]` per control channel), after
    /// [`Self::drive_frame`] for the same frame: feed every tracker and write
    /// each channel's residual into `residual`. Until `lag` frames have been
    /// played the trackers are not fed and the residual is the input.
    #[inline]
    pub fn track_frame(&mut self, input: &[f64], residual: &mut [f64]) {
        let w = self.lag + 1;
        residual[..self.channels].copy_from_slice(&input[..self.channels]);
        if self.frames <= self.lag as u64 {
            return;
        }
        // `head` is where the next phase goes, which is the oldest held: the
        // phase played `lag` frames before the one just written.
        let at = self.head;
        for i in 0..self.voices.len() {
            let phase = self.ring[i * w + at];
            for c in 0..self.channels {
                let t = &mut self.trackers[i * self.channels + c];
                t.update(input[c], phase);
                residual[c] -= t.passed(phase);
            }
        }
    }

    /// Tone `i`'s composite tracked amplitude over the channels with
    /// `weights` (weighted average; Random's default composite), peak, and
    /// whether every channel's tracker has settled.
    pub fn composite(&self, i: usize, weights: &[f64]) -> (f64, bool) {
        let (mut num, mut den, mut settled) = (0.0, 0.0, true);
        for c in 0..self.channels {
            let t = &self.trackers[i * self.channels + c];
            let w = weights.get(c).copied().unwrap_or(1.0);
            num += w * t.amplitude();
            den += w;
            settled &= t.settled();
        }
        (if den > 0.0 { num / den } else { 0.0 }, settled)
    }
}

/// Whether a tone of peak `a` stands far enough above a floor of one-sided
/// density `g` in a tracker band of `bn_hz` to be read ([`MIN_TONE_SNR`]).
pub fn tone_readable(a: f64, g: f64, bn_hz: f64) -> bool {
    a * a / 2.0 > MIN_TONE_SNR * g.max(0.0) * bn_hz
}

/// The tone loop's stability bound on its compression gain `c` with a
/// reading `d` loops late, for a log-proportional update on a pure-gain plant
/// (`e[n+1] = e[n] - c e[n-d]`): `2 sin(pi / (2 (2d + 1)))` (the brief's
/// 4.2, from the Jury test of `z^(d+1) - z^d + c`).
pub fn tone_loop_gain_bound(d: usize) -> f64 {
    2.0 * (std::f64::consts::PI / (2.0 * (2.0 * d as f64 + 1.0))).sin()
}

#[cfg(test)]
mod tests;
