//! A mode of a [`crate::ModalPlant`] as a filter fitted to its continuous response
//! ([`crate::plant::Discretization::Fitted`]), the modes' level-dependent softening,
//! and the plant's recording of what it plays.
//!
//! A fitted mode keeps the continuous mode's poles exactly (`z = exp(s T)`) and fits
//! its numerator, `taps` coefficients, by least squares to the continuous response
//! delayed by `delay` samples, on lines from 0 to the band's top. The common delay is
//! what lets a short numerator match the response: without it the bilinear and hold
//! designs miss by several dB near a quarter of the sample rate, where a light mode's
//! acceleration nearly cancels against its mass line. On the RC plate's 48 x 12 plant
//! at 16 384 Hz, 9 taps and 4 samples match the continuous FRF to 0.0001 dB in every
//! sixth-octave band from 20 to 4000 Hz. Above the fitted band nothing is held.

use crate::plant::ModeResponse;
use hertzforge_sdm::C64;
use std::f64::consts::TAU;
use std::path::PathBuf;

/// Lines in a mode's fit per numerator coefficient.
const LINES_PER_TAP: usize = 16;
/// Bytes a recording holds before it is written out.
const RECORD_BUFFER: usize = 1 << 20;

/// The continuous response of one mode at `f` Hz: Bandpass `w s / (s^2 + s w / Q +
/// w^2)`, Acceleration `s^2 / (...)`, and the modal displacement `w^2 / (...)`
/// scaled to one at DC (`displacement`), which [`FittedMode::displacement`] unscales.
fn continuous(f_n: f64, q: f64, response: ModeResponse, displacement: bool, f: f64) -> C64 {
    let w = TAU * f_n;
    let s = C64::new(0.0, TAU * f);
    let den = s * s + s * (w / q) + w * w;
    let num = if displacement {
        C64::new(w * w, 0.0)
    } else {
        match response {
            ModeResponse::Bandpass => s * w,
            ModeResponse::Acceleration => s * s,
        }
    };
    num / den
}

/// The denominator `1 + a1 z^-1 + a2 z^-2` of the continuous mode's exact poles.
fn poles(f_n: f64, q: f64, fs_hz: f64) -> (f64, f64) {
    let w = TAU * f_n;
    let half = 1.0 / (2.0 * q);
    let root = C64::new(half * half - 1.0, 0.0).sqrt();
    let z1 = (C64::new(-half, 0.0) + root).scale(w / fs_hz).exp();
    let z2 = (C64::new(-half, 0.0) - root).scale(w / fs_hz).exp();
    (-(z1 + z2).re, (z1 * z2).re)
}

/// The least-squares solution of `a x = b`, `a` row-major `rows x cols`, by
/// Householder reflections.
fn least_squares(mut a: Vec<f64>, mut b: Vec<f64>, rows: usize, cols: usize) -> Vec<f64> {
    for k in 0..cols {
        let norm = (k..rows)
            .map(|i| a[i * cols + k].powi(2))
            .sum::<f64>()
            .sqrt();
        if norm == 0.0 {
            continue;
        }
        let alpha = if a[k * cols + k] > 0.0 { -norm } else { norm };
        let mut v: Vec<f64> = (k..rows).map(|i| a[i * cols + k]).collect();
        v[0] -= alpha;
        let vv: f64 = v.iter().map(|x| x * x).sum();
        if vv == 0.0 {
            continue;
        }
        for j in k..cols {
            let dot: f64 = (k..rows).map(|i| v[i - k] * a[i * cols + j]).sum::<f64>() * 2.0 / vv;
            for i in k..rows {
                a[i * cols + j] -= dot * v[i - k];
            }
        }
        let dot: f64 = (k..rows).map(|i| v[i - k] * b[i]).sum::<f64>() * 2.0 / vv;
        for i in k..rows {
            b[i] -= dot * v[i - k];
        }
    }
    let mut x = vec![0.0; cols];
    for k in (0..cols).rev() {
        let sum: f64 = (k + 1..cols).map(|j| a[k * cols + j] * x[j]).sum();
        let d = a[k * cols + k];
        x[k] = if d == 0.0 { 0.0 } else { (b[k] - sum) / d };
    }
    x
}

/// The numerator fitted to `target` behind the denominator `(a1, a2)`.
fn fit(
    a1: f64,
    a2: f64,
    taps: usize,
    delay: usize,
    band_hz: f64,
    fs_hz: f64,
    target: impl Fn(f64) -> C64,
) -> Vec<f64> {
    let lines = LINES_PER_TAP * taps;
    let mut a = vec![0.0; 2 * lines * taps];
    let mut b = vec![0.0; 2 * lines];
    for line in 0..lines {
        let f = band_hz * line as f64 / (lines - 1) as f64;
        let zi = C64::from_polar(1.0, -TAU * f / fs_hz);
        let den = C64::new(1.0, 0.0) + zi * a1 + zi * zi * a2;
        let want = target(f) * zi.powu(delay as u32);
        let mut basis = C64::new(1.0, 0.0) / den;
        for k in 0..taps {
            a[2 * line * taps + k] = basis.re;
            a[(2 * line + 1) * taps + k] = basis.im;
            basis *= zi;
        }
        b[2 * line] = want.re;
        b[2 * line + 1] = want.im;
    }
    least_squares(a, b, 2 * lines, taps)
}

/// One mode as a fitted filter: the exact poles, then the numerator, in direct form II
/// on the history of `w[n] = u[n] - a1 w[n-1] - a2 w[n-2]`.
#[derive(Debug, Clone)]
pub(crate) struct FittedMode {
    f_n: f64,
    q: f64,
    response: ModeResponse,
    a1: f64,
    a2: f64,
    b: Vec<f64>,
    /// The modal displacement's numerator, for a softening mode.
    c: Option<Vec<f64>>,
    /// `w[n - k]` at `history[(head + k) % len]`.
    history: Vec<f64>,
    head: usize,
}

impl FittedMode {
    pub(crate) fn new(
        f_n: f64,
        q: f64,
        response: ModeResponse,
        displacement: bool,
        taps: usize,
    ) -> Self {
        FittedMode {
            f_n,
            q: q.max(0.5),
            response,
            a1: 0.0,
            a2: 0.0,
            b: vec![0.0; taps],
            c: displacement.then(|| vec![0.0; taps]),
            history: vec![0.0; taps.max(3)],
            head: 0,
        }
    }

    /// Set the mode's natural frequency and fit its filter for it.
    pub(crate) fn tune(&mut self, f_n: f64, fs_hz: f64, delay: usize, band_hz: f64) {
        self.f_n = f_n;
        let (q, response, taps) = (self.q, self.response, self.b.len());
        (self.a1, self.a2) = poles(f_n, q, fs_hz);
        self.b = fit(self.a1, self.a2, taps, delay, band_hz, fs_hz, |f| {
            continuous(f_n, q, response, false, f)
        });
        if self.c.is_some() {
            self.c = Some(fit(self.a1, self.a2, taps, delay, band_hz, fs_hz, |f| {
                continuous(f_n, q, response, true, f)
            }));
        }
    }

    pub(crate) fn f_n(&self) -> f64 {
        self.f_n
    }

    #[inline]
    pub(crate) fn process(&mut self, u: f64) -> f64 {
        let len = self.history.len();
        let w1 = self.history[self.head];
        let w2 = self.history[(self.head + 1) % len];
        self.head = (self.head + len - 1) % len;
        self.history[self.head] = u - self.a1 * w1 - self.a2 * w2;
        self.b
            .iter()
            .enumerate()
            .map(|(k, b)| b * self.history[(self.head + k) % len])
            .sum()
    }

    /// The modal displacement at the last sample, for a mode fitted with it.
    pub(crate) fn displacement(&self) -> f64 {
        let Some(c) = &self.c else { return 0.0 };
        let len = self.history.len();
        let scaled: f64 = c
            .iter()
            .enumerate()
            .map(|(k, c)| c * self.history[(self.head + k) % len])
            .sum();
        scaled / (TAU * self.f_n).powi(2)
    }

    /// The filter's own response at `f` Hz.
    pub(crate) fn response(&self, f_hz: f64, fs_hz: f64) -> C64 {
        let zi = C64::from_polar(1.0, -TAU * f_hz / fs_hz);
        let mut num = C64::new(0.0, 0.0);
        let mut power = C64::new(1.0, 0.0);
        for b in &self.b {
            num += power * *b;
            power *= zi;
        }
        num / (C64::new(1.0, 0.0) + zi * self.a1 + zi * zi * self.a2)
    }

    pub(crate) fn reset(&mut self) {
        self.history.fill(0.0);
        self.head = 0;
    }
}

/// A running mean square of each softening mode's displacement, and the frequencies
/// it gives at the end of each read.
#[derive(Debug, Clone)]
pub(crate) struct SofteningRun {
    pub(crate) spec: crate::plant::ModalSoftening,
    f0: Vec<f64>,
    square_sum: Vec<f64>,
    mean_square: Vec<f64>,
    in_read: usize,
    reads: usize,
}

impl SofteningRun {
    pub(crate) fn new(spec: &crate::plant::ModalSoftening, f0: Vec<f64>) -> Self {
        let n = spec.modes.len();
        if let Some(path) = &spec.log {
            let _ = std::fs::write(path, "");
        }
        SofteningRun {
            spec: spec.clone(),
            f0,
            square_sum: vec![0.0; n],
            mean_square: vec![0.0; n],
            in_read: 0,
            reads: 0,
        }
    }

    /// Take one sample of the softening modes' displacements, in `spec.modes` order;
    /// at the end of a read, the modes' new natural frequencies.
    pub(crate) fn sample(
        &mut self,
        displacement: impl Iterator<Item = f64>,
        fs_hz: f64,
    ) -> Option<Vec<f64>> {
        for (sum, x) in self.square_sum.iter_mut().zip(displacement) {
            *sum += x * x;
        }
        self.in_read += 1;
        if self.in_read < self.spec.read_samples {
            return None;
        }
        self.in_read = 0;
        let mut f = Vec::with_capacity(self.f0.len());
        for (s, sum) in self.square_sum.iter_mut().enumerate() {
            let block = *sum / self.spec.read_samples as f64;
            *sum = 0.0;
            if self.reads == 0 {
                self.mean_square[s] = block;
            } else {
                self.mean_square[s] += self.spec.smoothing * (block - self.mean_square[s]);
            }
            let r2 = self.mean_square[s] / self.spec.reference_rms[s].powi(2);
            f.push(self.f0[s] * (1.0 - self.spec.fraction * r2 / (1.0 + r2)));
        }
        self.reads += 1;
        if let Some(path) = &self.spec.log {
            use std::fmt::Write as _;
            use std::io::Write as _;
            let seconds = (self.reads * self.spec.read_samples) as f64 / fs_hz;
            let mut line = format!("{seconds:.3}");
            for v in &f {
                let _ = write!(line, ",{v:.4}");
            }
            line.push('\n');
            if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(path) {
                let _ = file.write_all(line.as_bytes());
            }
        }
        Some(f)
    }

    /// The natural frequencies before any read.
    pub(crate) fn reset(&mut self) -> Vec<f64> {
        self.square_sum.fill(0.0);
        self.mean_square.fill(0.0);
        self.in_read = 0;
        self.reads = 0;
        self.f0.clone()
    }
}

/// Every input of every sample, as played, noise included, appended to a file as
/// little-endian `f32`, sample-major.
#[derive(Debug, Clone)]
pub(crate) struct Recorder {
    path: PathBuf,
    buffered: Vec<u8>,
}

impl Recorder {
    pub(crate) fn new(path: PathBuf) -> Self {
        let _ = std::fs::write(&path, b"");
        Recorder {
            path,
            buffered: Vec::with_capacity(RECORD_BUFFER),
        }
    }

    #[inline]
    pub(crate) fn record(&mut self, out: &[f64]) {
        for v in out {
            self.buffered.extend_from_slice(&(*v as f32).to_le_bytes());
        }
        if self.buffered.len() >= RECORD_BUFFER {
            self.flush();
        }
    }

    pub(crate) fn flush(&mut self) {
        use std::io::Write as _;
        if self.buffered.is_empty() {
            return;
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&self.path) {
            let _ = f.write_all(&self.buffered);
        }
        self.buffered.clear();
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A light mode and one well above the band, fitted at 16 384 Hz to 4400 Hz:
    /// each filter matches its continuous response, delayed, within 1e-4 of its peak
    /// on every line to 4 kHz. The bilinear biquad misses the high mode's skirt there
    /// by far more.
    #[test]
    fn a_fitted_mode_matches_the_continuous_mode_across_the_band() {
        let fs = 16384.0;
        for (f_n, q) in [(418.0, 50.0), (7900.0, 50.0), (1.0, 50.0)] {
            let mut mode = FittedMode::new(f_n, q, ModeResponse::Acceleration, true, 9);
            mode.tune(f_n, fs, 4, 4400.0);
            let mut worst: f64 = 0.0;
            for k in 1..400 {
                let f = 10.0 * k as f64;
                let want = continuous(f_n, q, ModeResponse::Acceleration, false, f)
                    * C64::from_polar(1.0, -TAU * f * 4.0 / fs);
                worst = worst.max((mode.response(f, fs) - want).norm());
            }
            assert!(worst < 1e-4 * q, "mode at {f_n} Hz: worst error {worst}");
        }
    }

    /// Stepped sample by sample, a fitted mode's output is its own response: a tone in
    /// the band comes out at the gain and phase `response` gives, and the displacement
    /// at the continuous displacement's.
    #[test]
    fn stepping_a_fitted_mode_plays_its_response() {
        let fs = 16384.0;
        let (f_n, q, f) = (1125.0, 50.0, 1000.0);
        let mut mode = FittedMode::new(f_n, q, ModeResponse::Acceleration, true, 9);
        mode.tune(f_n, fs, 4, 4400.0);
        let n = 1 << 15;
        let (mut y, mut x) = (C64::new(0.0, 0.0), C64::new(0.0, 0.0));
        for t in 0..n {
            let phase = TAU * f * t as f64 / fs;
            let out = mode.process(phase.cos());
            if t >= n / 2 {
                y += C64::from_polar(out, -phase);
                x += C64::from_polar(mode.displacement(), -phase);
            }
        }
        let scale = 2.0 / (n / 2) as f64;
        let want = mode.response(f, fs);
        assert!(
            ((y * scale) - want).norm() < 1e-3 * want.norm(),
            "{} against {want}",
            y * scale
        );
        let w = TAU * f_n;
        let disp = continuous(f_n, q, ModeResponse::Acceleration, true, f) / (w * w)
            * C64::from_polar(1.0, -TAU * f * 4.0 / fs);
        assert!(
            ((x * scale) - disp).norm() < 1e-3 * disp.norm(),
            "{} against {disp}",
            x * scale
        );
    }
}
