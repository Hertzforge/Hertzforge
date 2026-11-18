//! A complex tone tracker: synchronous demodulation of a response against the
//! phase of the tone that produced it.
//!
//! The baseband low-pass is a cascade of [`TRACKER_POLES`] identical one-pole
//! sections, sized so the tracker's noise bandwidth (the full width of the
//! equivalent band-pass) is the configured tracking width `B_n`. Settling to
//! within about 3 % of a step takes [`SETTLE_WIDTHS`] over `B_n`.
//!
//! Phase convention: for a response `A sin(phi + theta)` read against the
//! reference phase `phi`, [`ToneTracker::phase`] reads `theta`.

/// Sections in the tracker's baseband low-pass.
pub const TRACKER_POLES: usize = 4;
/// A tracker's reading is usable this many `1 / B_n` after it starts (or
/// after its tone's frequency last jumped): a step has settled to within
/// about 3 %.
pub const SETTLE_WIDTHS: f64 = 1.3;

/// The noise gain `sum h^2` of [`TRACKER_POLES`] cascaded one-pole sections
/// over `alpha`, as `alpha` goes to zero: `(2n - 2)! / ((n - 1)!^2 2^(2n - 1))`.
const NOISE_GAIN_PER_ALPHA: f64 = 720.0 / (36.0 * 128.0);

/// The tracker's group delay times its noise bandwidth, as the bandwidth goes
/// small against the rate: each of the [`TRACKER_POLES`] sections delays by
/// `(1 - alpha) / alpha` samples and `alpha = B_n / (g fs)` with `g` the
/// noise gain per alpha, so the delay is `TRACKER_POLES g / B_n` seconds:
/// `4 x 0.15625 = 0.625` over the bandwidth.
pub const TRACKER_DELAY_FACTOR: f64 = TRACKER_POLES as f64 * NOISE_GAIN_PER_ALPHA;

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

    /// The tracked complex amplitude against the reference, `(re, im)`: the
    /// amplitude at the phase [`Self::phase`] reads.
    pub fn complex(&self) -> (f64, f64) {
        (self.i[TRACKER_POLES - 1], self.q[TRACKER_POLES - 1])
    }

    /// Change the noise bandwidth, keeping the state (a tone tracked along a
    /// sweep whose bandwidth follows its frequency). The settle count is kept.
    pub fn set_bn_hz(&mut self, bn_hz: f64) {
        self.alpha = (bn_hz / (NOISE_GAIN_PER_ALPHA * self.fs_hz)).clamp(1e-12, 0.5);
        self.bn_hz = bn_hz;
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
