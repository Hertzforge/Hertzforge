//! The matrix floor estimate: a per-line `n` by `n` Hermitian running
//! average of the residual's periodogram matrices, in which a line pauses
//! for the frames it is excluded and resumes with the average it held
//! (design D7, single-output design D18).
//!
//! Conventions are MIMO Random's (`hertzforge-mimo-random/src/estimate.rs`):
//! `G = E[X X^H]` per line with `X` under the negative-exponent transform
//! (the workspace transform's positive-exponent output conjugated), entry
//! `(r, c) = X_r conj(X_c)`, one-sided `2 / (fs sum(w^2))` on interior lines
//! and 1 at DC and Nyquist, mean removed per frame. The loop-to-loop average
//! is MIMO Random's `loop_tick`'s, per line: a loop's raw mean of its clean
//! frames, then `est += alpha (raw - est)` with
//! `alpha = max(2 / (memory + 1), 1 / (loops + 1))` (the first loop takes the
//! raw mean), and degrees of freedom `2 min(frames, 2 / alpha * raw_frames)`.
//! Level referral is the caller's ([`MatrixFloorEstimator::scale`]).
//!
//! A paused line keeps its average for at most the age limit, in frames
//! since its last clean frame; past it the line is stale and starts afresh
//! when it resumes.

use hertzforge_dsp::cola;
use hertzforge_dsp::fft;
use hertzforge_mimo_random::config::{Estimation, EstimationWindow};
use hertzforge_sdm::{HermitianStack, C64};

/// One frame's spectra of every channel, scaled so that `X X^H` is the
/// frame's one-sided periodogram matrix. Preallocated: [`Self::transform`]
/// does not allocate.
#[derive(Debug, Clone)]
pub struct FrameSpectra {
    nfft: usize,
    channels: usize,
    lines: usize,
    window: Vec<f64>,
    /// `sqrt(fold * norm)` per line.
    scale: Vec<f64>,
    conjugate: bool,
    scratch: Vec<(f64, f64)>,
    spectra: Vec<C64>,
}

impl FrameSpectra {
    /// For frames of `nfft` samples (a power of two) at `fs_hz` on
    /// `channels` channels, under `window`; the first `lines` lines kept
    /// (at most `nfft / 2 + 1`).
    pub fn new(
        nfft: usize,
        fs_hz: f64,
        channels: usize,
        lines: usize,
        window: EstimationWindow,
    ) -> Self {
        let win: Vec<f64> = match window {
            EstimationWindow::Hann => hertzforge_dsp::window::hann(nfft),
            EstimationWindow::Rectangular => vec![1.0; nfft],
        };
        let s2: f64 = win.iter().map(|w| w * w).sum();
        let norm = 1.0 / (fs_hz * s2);
        let lines = lines.min(nfft / 2 + 1);
        let scale = (0..lines)
            .map(|k| {
                let fold = if k == 0 || k == nfft / 2 { 1.0 } else { 2.0 };
                (fold * norm).sqrt()
            })
            .collect();
        FrameSpectra {
            nfft,
            channels,
            lines,
            window: win,
            scale,
            conjugate: cola::forward_exponent_is_positive(),
            scratch: vec![(0.0, 0.0); nfft],
            spectra: vec![C64::new(0.0, 0.0); lines * channels],
        }
    }

    /// Lines kept.
    pub fn lines(&self) -> usize {
        self.lines
    }

    /// Channels.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frame length, samples.
    pub fn nfft(&self) -> usize {
        self.nfft
    }

    /// Transform one frame: `frame(c)` gives channel `c`'s `nfft` samples.
    pub fn transform<'a>(&mut self, frame: impl Fn(usize) -> &'a [f64]) {
        let n = self.nfft;
        for c in 0..self.channels {
            let x = frame(c);
            let mean = x[..n].iter().sum::<f64>() / n as f64;
            for (i, s) in self.scratch.iter_mut().enumerate() {
                *s = ((x[i] - mean) * self.window[i], 0.0);
            }
            fft::fft_in_place(&mut self.scratch);
            for k in 0..self.lines {
                let (re, im) = self.scratch[k];
                let v = if self.conjugate {
                    C64::new(re, -im)
                } else {
                    C64::new(re, im)
                };
                self.spectra[k * self.channels + c] = v * self.scale[k];
            }
        }
    }

    /// The scaled spectra, line-major (`channels` per line).
    pub fn spectra(&self) -> &[C64] {
        &self.spectra
    }
}

/// The per-line state of the matrix floor estimate.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct LineState {
    /// Loops averaged into the estimate.
    loops: u32,
    /// Clean frames behind the estimate (summed over loops).
    frames: f64,
    /// Degrees of freedom of the estimate.
    dof: f64,
    /// Clean frames in the loop being gathered.
    loop_frames: u32,
    /// Frames since the line's last clean frame.
    since_clean: u64,
    /// Whether the line took no frame in the last finished loop.
    paused: bool,
}

/// The matrix floor estimate with per-line pause and resume.
#[derive(Debug, Clone)]
pub struct MatrixFloorEstimator {
    n: usize,
    lines: usize,
    memory_loops: f64,
    age_limit_frames: u64,
    /// The running estimate, packed per line.
    est: Vec<C64>,
    /// The loop being gathered, packed per line.
    loop_sum: Vec<C64>,
    state: Vec<LineState>,
}

impl MatrixFloorEstimator {
    /// An empty estimate of `n` channels on `lines` lines, averaging over
    /// `memory_loops` loops (MIMO Random's `Estimation::memory_loops`), a
    /// paused line kept for at most `age_limit_frames` frames (0: forever).
    pub fn new(n: usize, lines: usize, memory_loops: f64, age_limit_frames: u64) -> Self {
        let pl = HermitianStack::packed_len(n);
        MatrixFloorEstimator {
            n,
            lines,
            memory_loops: memory_loops.max(1.0),
            age_limit_frames,
            est: vec![C64::new(0.0, 0.0); lines * pl],
            loop_sum: vec![C64::new(0.0, 0.0); lines * pl],
            state: vec![LineState::default(); lines],
        }
    }

    /// For MIMO Random's estimation settings, with the default age limit
    /// ([`default_age_limit_frames`]).
    pub fn for_estimation(n: usize, lines: usize, e: &Estimation) -> Self {
        Self::new(n, lines, e.memory_loops, default_age_limit_frames(e))
    }

    /// Channels.
    pub fn n(&self) -> usize {
        self.n
    }

    /// Lines.
    pub fn lines(&self) -> usize {
        self.lines
    }

    /// The age limit, frames (0: none).
    pub fn age_limit_frames(&self) -> u64 {
        self.age_limit_frames
    }

    /// Forget everything.
    pub fn reset(&mut self) {
        self.est.fill(C64::new(0.0, 0.0));
        self.loop_sum.fill(C64::new(0.0, 0.0));
        self.state.fill(LineState::default());
    }

    /// Add one frame: `spectra` its scaled spectra, line-major (`n` per
    /// line, [`FrameSpectra::spectra`]); a line marked in `excluded` takes
    /// no part of this frame. Does not allocate.
    pub fn add_frame(&mut self, spectra: &[C64], excluded: &[bool]) {
        let n = self.n;
        let pl = HermitianStack::packed_len(n);
        for l in 0..self.lines {
            let st = &mut self.state[l];
            if excluded.get(l).copied().unwrap_or(false) {
                st.since_clean = st.since_clean.saturating_add(1);
                continue;
            }
            let x = &spectra[l * n..(l + 1) * n];
            let s = &mut self.loop_sum[l * pl..(l + 1) * pl];
            let mut k = 0;
            for r in 0..n {
                for c in r..n {
                    s[k] += x[r] * x[c].conj();
                    k += 1;
                }
            }
            st.loop_frames += 1;
            if self.age_limit_frames > 0 && st.since_clean > self.age_limit_frames {
                // The line resumes after its age limit: what it held is stale.
                st.loops = 0;
                st.frames = 0.0;
                st.dof = 0.0;
            }
            st.since_clean = 0;
        }
    }

    /// Finish a loop: every line that took a frame blends its loop's raw
    /// mean into its estimate (MIMO Random's rule); a line that took none
    /// keeps its estimate and is paused.
    pub fn end_loop(&mut self) {
        let pl = HermitianStack::packed_len(self.n);
        for l in 0..self.lines {
            let st = &mut self.state[l];
            let s = &mut self.loop_sum[l * pl..(l + 1) * pl];
            let e = &mut self.est[l * pl..(l + 1) * pl];
            if st.loop_frames == 0 {
                st.paused = true;
                continue;
            }
            st.paused = false;
            let raw_frames = f64::from(st.loop_frames);
            let inv = 1.0 / raw_frames;
            if st.loops == 0 {
                for (ev, sv) in e.iter_mut().zip(s.iter()) {
                    *ev = *sv * inv;
                }
                st.frames = raw_frames;
                st.loops = 1;
                st.dof = 2.0 * raw_frames;
            } else {
                let alpha = (2.0 / (self.memory_loops + 1.0)).max(1.0 / f64::from(st.loops + 1));
                for (ev, sv) in e.iter_mut().zip(s.iter()) {
                    *ev += (*sv * inv - *ev) * alpha;
                }
                st.frames += raw_frames;
                st.loops += 1;
                st.dof = 2.0 * st.frames.min(2.0 / alpha * raw_frames);
            }
            s.fill(C64::new(0.0, 0.0));
            st.loop_frames = 0;
        }
    }

    /// Scale every line's estimate and the loop being gathered by a power
    /// ratio (a level change, or referral to full level).
    pub fn scale(&mut self, power: f64) {
        for v in self.est.iter_mut().chain(self.loop_sum.iter_mut()) {
            *v *= power;
        }
    }

    /// Whether line `l` took no frame in the last finished loop.
    pub fn paused(&self, l: usize) -> bool {
        self.state[l].paused
    }

    /// Whether line `l` has an estimate that can be read: averaged at least
    /// once, not paused, and not past its age limit.
    pub fn clean(&self, l: usize) -> bool {
        let st = &self.state[l];
        st.loops > 0 && !st.paused && !self.stale(l)
    }

    /// Whether line `l` has been paused past its age limit.
    pub fn stale(&self, l: usize) -> bool {
        self.age_limit_frames > 0 && self.state[l].since_clean > self.age_limit_frames
    }

    /// Loops averaged on line `l`.
    pub fn loops(&self, l: usize) -> u32 {
        self.state[l].loops
    }

    /// Clean frames behind line `l`'s estimate.
    pub fn frames(&self, l: usize) -> f64 {
        self.state[l].frames
    }

    /// Degrees of freedom of line `l`'s estimate.
    pub fn dof(&self, l: usize) -> f64 {
        self.state[l].dof
    }

    /// The smallest degrees of freedom over the clean lines of `band`
    /// (`[lo, hi)`), the one value the frozen `random-mimo` world passes
    /// (theory 2.6, limit 1); 0 when no line in the band is clean.
    pub fn min_clean_dof(&self, band: (usize, usize)) -> f64 {
        let m = (band.0..band.1.min(self.lines))
            .filter(|&l| self.clean(l))
            .map(|l| self.state[l].dof)
            .fold(f64::INFINITY, f64::min);
        if m.is_finite() {
            m
        } else {
            0.0
        }
    }

    /// Line `l`'s estimate, packed.
    pub fn line(&self, l: usize) -> &[C64] {
        let pl = HermitianStack::packed_len(self.n);
        &self.est[l * pl..(l + 1) * pl]
    }

    /// Entry `(r, c)` of line `l`'s estimate.
    pub fn get(&self, l: usize, r: usize, c: usize) -> C64 {
        let (a, b) = if r <= c { (r, c) } else { (c, r) };
        let v = self.line(l)[HermitianStack::packed_index(self.n, a, b)];
        if r <= c {
            v
        } else {
            v.conj()
        }
    }

    /// The estimate as a stack (allocates).
    pub fn to_stack(&self) -> HermitianStack {
        HermitianStack::from_packed(self.lines, self.n, self.est.clone()).expect("packed shape")
    }

    /// The clean mask: `true` where [`Self::clean`].
    pub fn clean_mask(&self) -> Vec<bool> {
        (0..self.lines).map(|l| self.clean(l)).collect()
    }
}

/// The default age limit of a paused line, frames: the frames the memory
/// spans, `ceil(memory_loops) * frames_per_loop`. Past it the average held
/// is older than the average would be on a clean line.
pub fn default_age_limit_frames(e: &Estimation) -> u64 {
    (e.memory_loops.max(1.0).ceil() as u64) * e.frames_per_loop.max(1) as u64
}
