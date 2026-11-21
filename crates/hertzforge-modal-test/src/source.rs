//! The shaker sources: random, burst random, pseudo random, periodic random, periodic
//! chirp and burst chirp. Each is band-limited to the setup's band, seeded, independent
//! per drive, and has its crest bounded by construction: a random source is clipped at
//! [`crate::setup::RANDOM_CREST`] times its RMS level, and a chirp is a sine of the
//! level's amplitude. Frames are made on the feeder thread ([`Feeder`]), never on the
//! loop thread.
//!
//! Definitions:
//!
//! * **Burst random.** Random for the burst fraction of the frame, with tapered ends,
//!   and zero for the rest. Here the random part is a band-limited Gaussian frame,
//!   scaled to the level's RMS over the burst, clipped, then given a half-cosine taper of
//!   [`BURST_TAPER_FRACTION`] of the burst at each end.
//! * **Pseudo random.** One band-limited multisine frame (flat magnitude, random phases),
//!   repeated every frame.
//! * **Periodic random.** A new multisine frame every `delay + 1` frames, each played
//!   `delay + 1` times: the first `delay` let the transient die, the last is averaged.
//! * **Periodic chirp.** A linear sweep from the band's bottom to its top over one
//!   frame, with its end lines chosen so the frame holds whole cycles.
//! * **Burst chirp.** The same sweep over the burst, tapered, zero for the rest.

use crate::setup::{SourceKind, RANDOM_CREST};
use hertzforge_engine::ring::Producer;
use std::f64::consts::PI;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// The part of the burst each tapered end takes.
pub const BURST_TAPER_FRACTION: f64 = 0.1;

/// SplitMix64, then Box-Muller: a small seeded generator, one per drive.
#[derive(Debug, Clone)]
pub struct Rng {
    s: u64,
    spare: Option<f64>,
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng {
            s: seed ^ 0x6A09_E667_F3BC_C909,
            spare: None,
        }
    }

    /// The generator of drive `k` under `seed`.
    pub fn for_drive(seed: u64, k: usize) -> Rng {
        let mut r = Rng::new(seed.wrapping_add((k as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)));
        let _ = r.next_u64();
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        self.s = self.s.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.s;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `(0, 1]`.
    pub fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 1.0) / (1u64 << 53) as f64
    }

    pub fn gauss(&mut self) -> f64 {
        if let Some(s) = self.spare.take() {
            return s;
        }
        let (u, v) = (self.uniform(), self.uniform());
        let r = (-2.0 * u.ln()).sqrt();
        self.spare = Some(r * (2.0 * PI * v).sin());
        r * (2.0 * PI * v).cos()
    }
}

/// What a source is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceBuild {
    pub kind: SourceKind,
    pub fs_hz: f64,
    /// The frame, samples.
    pub block: usize,
    /// The band's lines (of the frame's grid), inclusive.
    pub lo_line: usize,
    pub hi_line: usize,
    /// Volts RMS per drive (over the burst for a burst).
    pub levels: Vec<f64>,
    pub burst_pct: f64,
    pub delay_frames: u32,
    pub seed: u64,
}

/// A source: [`Source::next_frame`] gives the next frame of every drive.
pub struct Source {
    b: SourceBuild,
    rngs: Vec<Rng>,
    /// Pseudo and periodic random: the current multisine frame per drive.
    held: Vec<Vec<f64>>,
    frame: u64,
    /// Continuous random: the synthesis per drive.
    cola: Vec<hertzforge_dsp::cola::ColaSynth>,
}

impl Source {
    pub fn new(b: SourceBuild) -> Source {
        let n = b.levels.len();
        let rngs = (0..n).map(|k| Rng::for_drive(b.seed, k)).collect();
        let cola = if b.kind == SourceKind::Random {
            (0..n)
                .map(|k| {
                    hertzforge_dsp::cola::ColaSynth::with_seed(
                        b.block,
                        0.5,
                        b.fs_hz,
                        Rng::for_drive(b.seed, k).next_u64(),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        Source {
            b,
            rngs,
            held: vec![Vec::new(); n],
            frame: 0,
            cola,
        }
    }

    pub fn build(&self) -> &SourceBuild {
        &self.b
    }

    /// The burst's samples.
    pub fn burst_len(&self) -> usize {
        ((self.b.burst_pct / 100.0) * self.b.block as f64)
            .round()
            .max(2.0) as usize
    }

    /// A band-limited Gaussian frame: a random complex value on every band line.
    fn gaussian_frame(&mut self, k: usize) -> Vec<f64> {
        let n = self.b.block;
        let mut spec = vec![(0.0, 0.0); n];
        for l in self.b.lo_line.max(1)..=self.b.hi_line.min(n / 2 - 1) {
            let (a, c) = (self.rngs[k].gauss(), self.rngs[k].gauss());
            spec[l] = (a, c);
            spec[n - l] = (a, -c);
        }
        real(hertzforge_dsp::fft::ifft(&spec))
    }

    /// A multisine frame: flat magnitude, random phase on every band line.
    fn multisine_frame(&mut self, k: usize) -> Vec<f64> {
        let n = self.b.block;
        let mut spec = vec![(0.0, 0.0); n];
        for l in self.b.lo_line.max(1)..=self.b.hi_line.min(n / 2 - 1) {
            let ph = 2.0 * PI * self.rngs[k].uniform();
            spec[l] = (ph.cos(), ph.sin());
            spec[n - l] = (ph.cos(), -ph.sin());
        }
        real(hertzforge_dsp::fft::ifft(&spec))
    }

    /// The chirp's end lines: the band's, with the top moved down one line when
    /// needed so `lo + hi` is even (the frame then holds whole cycles).
    pub fn chirp_lines(&self) -> (usize, usize) {
        let lo = self.b.lo_line.max(1);
        let mut hi = self.b.hi_line.max(lo + 1);
        if (lo + hi) % 2 == 1 {
            hi -= 1;
        }
        (lo, hi)
    }

    /// A linear sweep of `len` samples from line `lo` to line `hi` of a `len`-sample grid.
    fn sweep(len: usize, lo: f64, hi: f64) -> Vec<f64> {
        let n = len as f64;
        (0..len)
            .map(|i| {
                let x = i as f64 / n;
                (2.0 * PI * n * (lo * x / n + (hi - lo) * x * x / (2.0 * n))).sin()
            })
            .collect()
    }

    /// The next frame of every drive, volts.
    pub fn next_frame(&mut self) -> Vec<Vec<f64>> {
        let n = self.b.block;
        let drives = self.b.levels.len();
        let mut out = Vec::with_capacity(drives);
        for k in 0..drives {
            let level = self.b.levels[k];
            let x = match self.b.kind {
                SourceKind::Random => {
                    let bw = (self.b.hi_line + 1 - self.b.lo_line) as f64 * self.b.fs_hz / n as f64;
                    let s = level * level / bw;
                    let psd: Vec<f64> = (0..=n / 2)
                        .map(|l| {
                            if (self.b.lo_line..=self.b.hi_line).contains(&l) && l > 0 && l < n / 2
                            {
                                s
                            } else {
                                0.0
                            }
                        })
                        .collect();
                    let mut x = Vec::with_capacity(n);
                    while x.len() < n {
                        x.extend(self.cola[k].next_frame(&psd));
                    }
                    x.truncate(n);
                    clip(&mut x, RANDOM_CREST * level);
                    x
                }
                SourceKind::BurstRandom => {
                    let m = self.burst_len();
                    let g = self.gaussian_frame(k);
                    let mut x = vec![0.0; n];
                    let rms = (g[..m].iter().map(|v| v * v).sum::<f64>() / m as f64)
                        .sqrt()
                        .max(1e-300);
                    for i in 0..m {
                        x[i] = g[i] * level / rms;
                    }
                    clip(&mut x[..m], RANDOM_CREST * level);
                    taper(&mut x[..m]);
                    x
                }
                SourceKind::PseudoRandom | SourceKind::PeriodicRandom => {
                    let every = if self.b.kind == SourceKind::PseudoRandom {
                        u64::MAX
                    } else {
                        u64::from(self.b.delay_frames) + 1
                    };
                    if self.held[k].is_empty()
                        || (every != u64::MAX && self.frame.is_multiple_of(every))
                    {
                        let mut g = self.multisine_frame(k);
                        let rms = (g.iter().map(|v| v * v).sum::<f64>() / n as f64)
                            .sqrt()
                            .max(1e-300);
                        g.iter_mut().for_each(|v| *v *= level / rms);
                        clip(&mut g, RANDOM_CREST * level);
                        self.held[k] = g;
                    }
                    self.held[k].clone()
                }
                SourceKind::PeriodicChirp => {
                    let (lo, hi) = self.chirp_lines();
                    let a = level * std::f64::consts::SQRT_2;
                    Self::sweep(n, lo as f64, hi as f64)
                        .into_iter()
                        .map(|v| a * v)
                        .collect()
                }
                SourceKind::BurstChirp => {
                    let m = self.burst_len();
                    let (lo, hi) = self.chirp_lines();
                    // The same frequencies over the burst's shorter length.
                    let scale = m as f64 / n as f64;
                    let a = level * std::f64::consts::SQRT_2;
                    let mut x = vec![0.0; n];
                    for (i, v) in Self::sweep(m, lo as f64 * scale, hi as f64 * scale)
                        .into_iter()
                        .enumerate()
                    {
                        x[i] = a * v;
                    }
                    taper(&mut x[..m]);
                    x
                }
            };
            out.push(x);
        }
        self.frame += 1;
        out
    }
}

fn real(x: Vec<(f64, f64)>) -> Vec<f64> {
    x.into_iter().map(|c| c.0).collect()
}

fn clip(x: &mut [f64], at: f64) {
    for v in x.iter_mut() {
        *v = v.clamp(-at, at);
    }
}

/// A half-cosine rise and fall of [`BURST_TAPER_FRACTION`] of the length at each end.
fn taper(x: &mut [f64]) {
    let m = x.len();
    let t = ((m as f64) * BURST_TAPER_FRACTION).round() as usize;
    for i in 0..t.min(m) {
        let w = 0.5 * (1.0 - (PI * (i as f64 + 0.5) / t as f64).cos());
        x[i] *= w;
        x[m - 1 - i] *= w;
    }
}

/// What the feeder and the step share.
#[derive(Debug, Default)]
pub struct FeedShared {
    /// Sample frames pushed.
    pub pushed: AtomicU64,
    /// Sample frames the step has taken.
    pub popped: AtomicU64,
    pub cancel: AtomicBool,
    /// Set when the feeder thread failed (its source panicked): the loop ends the run
    /// with the emergency stop rather than wait for the ring to run dry.
    pub failed: AtomicBool,
    /// The failure's words.
    pub why: Mutex<Option<String>>,
}

/// What a feeder fills its ring from: frames of `block` samples for each of `width`
/// drives, one after another. [`Source`] is the one the run uses.
pub trait Frames: Send + 'static {
    fn block(&self) -> usize;
    fn width(&self) -> usize;
    /// The next frame, per drive.
    fn next_frame(&mut self) -> Vec<Vec<f64>>;
}

impl Frames for Source {
    fn block(&self) -> usize {
        self.build().block
    }
    fn width(&self) -> usize {
        self.build().levels.len()
    }
    fn next_frame(&mut self) -> Vec<Vec<f64>> {
        Source::next_frame(self)
    }
}

/// A caught panic's words.
fn panic_words(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|x| (*x).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no words".to_owned())
}

/// The feeder thread: fills the ring ahead of the loop, frame by frame.
pub struct Feeder {
    pub shared: Arc<FeedShared>,
    thread: Option<JoinHandle<()>>,
}

impl Feeder {
    /// Fill `ring` (one ring frame per sample, every drive) until it holds
    /// `ahead_frames`, before returning; then keep it filled on its own thread.
    /// A panic in the source is caught: before the thread starts it is an error, after
    /// it [`FeedShared::failed`] is set and the thread stops filling.
    pub fn spawn<F: Frames>(
        mut source: F,
        ring: Producer,
        ahead_frames: u64,
    ) -> std::io::Result<Feeder> {
        let shared = Arc::new(FeedShared::default());
        let block = source.block() as u64;
        let width = source.width();
        let fill = {
            let shared = shared.clone();
            move |source: &mut F, ring: &Producer| -> bool {
                let mut pushed_any = false;
                while shared.pushed.load(Ordering::Acquire) - shared.popped.load(Ordering::Acquire)
                    + block
                    <= ahead_frames
                {
                    let f = source.next_frame();
                    let mut s = vec![0f32; width];
                    for i in 0..block as usize {
                        for (k, d) in f.iter().enumerate() {
                            s[k] = d[i] as f32;
                        }
                        if !ring.push(&s) {
                            return pushed_any;
                        }
                        shared.pushed.fetch_add(1, Ordering::AcqRel);
                    }
                    pushed_any = true;
                }
                pushed_any
            }
        };
        let first = std::panic::catch_unwind(AssertUnwindSafe(|| fill(&mut source, &ring)));
        if let Err(p) = first {
            return Err(std::io::Error::other(format!(
                "the source failed: {}",
                panic_words(p.as_ref())
            )));
        }
        let sh = shared.clone();
        let thread = std::thread::Builder::new()
            .name("modal-test-source".into())
            .spawn(move || {
                while !sh.cancel.load(Ordering::Acquire) {
                    match std::panic::catch_unwind(AssertUnwindSafe(|| fill(&mut source, &ring))) {
                        Ok(true) => {}
                        Ok(false) => std::thread::sleep(Duration::from_millis(2)),
                        Err(p) => {
                            if let Ok(mut w) = sh.why.lock() {
                                *w = Some(panic_words(p.as_ref()));
                            }
                            sh.failed.store(true, Ordering::Release);
                            break;
                        }
                    }
                }
            })?;
        Ok(Feeder {
            shared,
            thread: Some(thread),
        })
    }

    pub fn cancel(&mut self) {
        self.shared.cancel.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Feeder {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(kind: SourceKind, drives: usize) -> SourceBuild {
        SourceBuild {
            kind,
            fs_hz: 1024.0,
            block: 1024,
            lo_line: 10,
            hi_line: 200,
            levels: vec![0.1; drives],
            burst_pct: 50.0,
            delay_frames: 1,
            seed: 7,
        }
    }

    fn rms(x: &[f64]) -> f64 {
        (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt()
    }

    /// The fraction of the power of `x` (a whole frame) on lines `lo..=hi`.
    fn in_band(x: &[f64], lo: usize, hi: usize) -> f64 {
        let s = hertzforge_dsp::fft::rfft(x);
        let p = |k: usize| s[k].0 * s[k].0 + s[k].1 * s[k].1;
        let all: f64 = (1..x.len() / 2).map(p).sum();
        let band: f64 = (lo..=hi).map(p).sum();
        band / all
    }

    #[test]
    fn modal_test_every_source_is_in_band_and_crest_bounded() {
        for kind in [
            SourceKind::Random,
            SourceKind::BurstRandom,
            SourceKind::PseudoRandom,
            SourceKind::PeriodicRandom,
            SourceKind::PeriodicChirp,
            SourceKind::BurstChirp,
        ] {
            let mut s = Source::new(build(kind, 1));
            let bound = kind.crest() * 0.1 + 1e-12;
            let mut frames = Vec::new();
            for _ in 0..8 {
                let f = s.next_frame().remove(0);
                assert!(f.iter().all(|v| v.abs() <= bound), "{kind:?} crest");
                frames.push(f);
            }
            let f = &frames[4];
            // A burst spreads a little past the band by its taper; every source keeps at
            // least 95 percent of its power in the band.
            let frac = in_band(f, 10, 200);
            assert!(frac > 0.95, "{kind:?} in band {frac}");
        }
    }

    #[test]
    fn modal_test_source_rms_meets_the_level() {
        for kind in [
            SourceKind::Random,
            SourceKind::PseudoRandom,
            SourceKind::PeriodicRandom,
            SourceKind::PeriodicChirp,
        ] {
            let mut s = Source::new(build(kind, 1));
            let x: Vec<f64> = (0..16).flat_map(|_| s.next_frame().remove(0)).collect();
            let r = rms(&x);
            assert!((r - 0.1).abs() < 0.006, "{kind:?} rms {r}");
        }
        let mut b = Source::new(build(SourceKind::BurstRandom, 1));
        let f = b.next_frame().remove(0);
        // Over the burst before the taper, the level; the taper takes a little.
        let r = rms(&f[..512]);
        assert!(r > 0.085 && r <= 0.1 + 1e-9, "burst rms {r}");
    }

    #[test]
    fn modal_test_burst_timing_is_on_then_zero() {
        for kind in [SourceKind::BurstRandom, SourceKind::BurstChirp] {
            let mut s = Source::new(build(kind, 1));
            let f = s.next_frame().remove(0);
            assert!(
                f[512..].iter().all(|v| *v == 0.0),
                "{kind:?} quiet after the burst"
            );
            assert!(
                f[100..400].iter().any(|v| v.abs() > 0.05),
                "{kind:?} on in the burst"
            );
            // Tapered ends: the first and last samples of the burst are small.
            assert!(f[0].abs() < 0.01 && f[511].abs() < 0.01, "{kind:?} tapered");
        }
    }

    #[test]
    fn modal_test_drives_are_independent_and_seeded() {
        for kind in [
            SourceKind::Random,
            SourceKind::BurstRandom,
            SourceKind::PeriodicRandom,
        ] {
            let mut s = Source::new(build(kind, 2));
            let mut a = Vec::new();
            let mut b = Vec::new();
            for _ in 0..16 {
                let f = s.next_frame();
                a.extend(f[0].clone());
                b.extend(f[1].clone());
            }
            let ab: f64 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
            let rho = ab / (rms(&a) * rms(&b) * a.len() as f64);
            assert!(rho.abs() < 0.08, "{kind:?} correlation {rho}");
            // The same seed gives the same frames; another seed does not.
            let mut s2 = Source::new(build(kind, 2));
            assert_eq!(s2.next_frame()[0][..64], a[..64]);
            let mut other = build(kind, 2);
            other.seed = 8;
            assert_ne!(Source::new(other).next_frame()[0][..64], a[..64]);
        }
    }

    #[test]
    fn modal_test_periodic_sources_repeat_as_stated() {
        let mut p = Source::new(build(SourceKind::PseudoRandom, 1));
        let f0 = p.next_frame().remove(0);
        for _ in 0..4 {
            assert_eq!(p.next_frame().remove(0), f0);
        }
        // Periodic random with one delay frame: each frame twice, then a new one.
        let mut q = Source::new(build(SourceKind::PeriodicRandom, 1));
        let a0 = q.next_frame().remove(0);
        let a1 = q.next_frame().remove(0);
        let b0 = q.next_frame().remove(0);
        let b1 = q.next_frame().remove(0);
        assert_eq!(a0, a1);
        assert_eq!(b0, b1);
        assert_ne!(a0, b0);
        // A periodic chirp holds whole cycles: the frame joins its repeat smoothly.
        let mut c = Source::new(build(SourceKind::PeriodicChirp, 1));
        let (lo, hi) = c.chirp_lines();
        assert_eq!((lo + hi) % 2, 0);
        let f = c.next_frame().remove(0);
        assert_eq!(f, c.next_frame().remove(0));
        // The sample after the last is the first: the phase at n = N is a whole cycle.
        let n = f.len() as f64;
        let ph_end = 2.0 * PI * (lo as f64 + (hi - lo) as f64 / 2.0);
        assert!(((ph_end / (2.0 * PI)) - (ph_end / (2.0 * PI)).round()).abs() < 1e-9);
        assert!(f[0].abs() < 1e-12 && n > 0.0);
    }

    #[test]
    fn modal_test_feeder_fills_ahead_off_the_loop() {
        let (tx, rx) = hertzforge_engine::ring::channel(8192, 2);
        let mut fd =
            Feeder::spawn(Source::new(build(SourceKind::BurstRandom, 2)), tx, 4096).unwrap();
        assert!(fd.shared.pushed.load(Ordering::Acquire) >= 3072);
        let mut s = [0f32; 2];
        let mut n = 0u64;
        while rx.pop(&mut s) {
            n += 1;
        }
        fd.shared.popped.store(n, Ordering::Release);
        std::thread::sleep(Duration::from_millis(50));
        assert!(fd.shared.pushed.load(Ordering::Acquire) > n);
        fd.cancel();
    }

    /// A source that panics after `after` frames.
    struct Breaks {
        after: usize,
        made: usize,
    }

    impl Frames for Breaks {
        fn block(&self) -> usize {
            1024
        }
        fn width(&self) -> usize {
            1
        }
        fn next_frame(&mut self) -> Vec<Vec<f64>> {
            self.made += 1;
            assert!(self.made <= self.after, "the source broke");
            vec![vec![0.0; 1024]]
        }
    }

    #[test]
    fn modal_test_a_source_that_fails_is_caught_before_and_after_the_feeder_starts() {
        // before: the start is refused with the words, not a panic on the caller
        let (tx, _rx) = hertzforge_engine::ring::channel(8192, 1);
        let e = Feeder::spawn(Breaks { after: 1, made: 0 }, tx, 4096).err();
        let e = e.expect("refused");
        assert!(e.to_string().contains("the source broke"), "{e}");
        // after: the thread says it failed
        let (tx, rx) = hertzforge_engine::ring::channel(8192, 1);
        let fd = Feeder::spawn(Breaks { after: 4, made: 0 }, tx, 4096).unwrap();
        let mut s = [0f32; 1];
        let mut n = 0u64;
        while rx.pop(&mut s) {
            n += 1;
        }
        fd.shared.popped.store(n, Ordering::Release);
        let t = std::time::Instant::now();
        while !fd.shared.failed.load(Ordering::Acquire) {
            assert!(
                t.elapsed() < Duration::from_secs(5),
                "the failure was not seen"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let why = fd.shared.why.lock().unwrap().clone().unwrap();
        assert!(why.contains("the source broke"), "{why}");
    }
}
