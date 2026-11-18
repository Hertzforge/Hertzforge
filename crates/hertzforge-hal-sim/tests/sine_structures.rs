//! The standard sine structures: names and sizes, the small-signal response a tone
//! reads, the calibration of the full level, the movement of each level-dependent
//! resonance at full level, the near-singular frequency, the article's reading, and
//! reproducibility.
//!
//! # The phase convention
//!
//! Every complex amplitude here is read by direct correlation over a whole number of
//! cycles: a record `x[n]` at `f` Hz gives `X = (2 / N) sum_n x[n] exp(-j 2 pi f n / fs)`
//! over `N` samples holding a whole number of cycles, so `x[n] = Re(X exp(j 2 pi f n /
//! fs))`. A drive `D` and the responses `C` it makes are read the same way over the same
//! samples, and the response is `C = H D`: the phase of `H[i][j]` is the phase of
//! response `i` measured against the phase of drive `j`, positive when the response
//! leads. `PlantModel::analytic_frf_at` gives `H` in this convention.

mod common;

use common::{arg, cdiv, mag};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal::{AcquisitionConfig, Backend};
use hertzforge_hal_sim::plants::{
    Conditioning, SineStructure, SINE_BAND_HZ, SINE_CALIBRATION_RATE_HZ, SINE_NEAR_SINGULAR_HZ,
};
use hertzforge_hal_sim::{JointedPlant, MatrixStack, SimBackend, SimConfig, C64};
use hertzforge_sdm::linalg;
use std::f64::consts::TAU;

const FS: f64 = SINE_CALIBRATION_RATE_HZ;

/// One second at the calibration rate: every whole-hertz tone fits it in whole cycles.
const N: usize = 16_384;

/// The table a structure runs on: its controls, then its other responses as monitors,
/// then its drives.
fn table(s: SineStructure) -> ChannelTable {
    let (n_out, n_d) = s.dims();
    let mut t = ChannelTable::new();
    for k in 0..n_out + n_d {
        let mut c = Channel::new(k as u32 + 1);
        c.role = if k < s.n_controls() {
            ChannelRole::Control
        } else if k < n_out {
            ChannelRole::Monitor
        } else {
            ChannelRole::Drive
        };
        c.range_max = 1e9;
        t.channels.push(c);
    }
    t
}

fn running(s: SineStructure, noise: f64, seed: u64) -> SimBackend {
    let mut b = SimConfig::new()
        .sample_rate_hz(FS)
        .plant(s.build().model)
        .noise_level(noise)
        .seed(seed)
        .shared_clock(true)
        .build();
    b.open().unwrap();
    b.configure(&table(s), &AcquisitionConfig::new(FS, 256))
        .unwrap();
    b.arm().unwrap();
    b.start().unwrap();
    b
}

/// The complex amplitude of `x` at `f_hz` over its whole length (see the module docs).
fn demodulate(x: &[f64], f_hz: f64, first_sample: usize) -> C64 {
    let n = x.len() as f64;
    x.iter()
        .enumerate()
        .fold(C64::new(0.0, 0.0), |acc, (k, &v)| {
            let ph = TAU * f_hz * (first_sample + k) as f64 / FS;
            acc + C64::new(ph.cos(), -ph.sin()) * v
        })
        * (2.0 / n)
}

/// The complex amplitude of `x` at `f_hz` by a least-squares fit of a cosine and a sine
/// (any length, in the convention of [`demodulate`]): for the held stepped sine, whose
/// windows are not whole cycles.
fn fit(x: &[f64], f_hz: f64, first_sample: usize) -> C64 {
    let (mut cc, mut ss, mut cs, mut yc, mut ys) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (k, &v) in x.iter().enumerate() {
        let (s, c) = (TAU * f_hz * (first_sample + k) as f64 / FS).sin_cos();
        cc += c * c;
        ss += s * s;
        cs += c * s;
        yc += v * c;
        ys += v * s;
    }
    let det = cc * ss - cs * cs;
    let a = (yc * ss - ys * cs) / det;
    let b = (ys * cc - yc * cs) / det;
    // x = a cos + b sin = Re((a - j b) exp(j w t)).
    C64::new(a, -b)
}

/// Log-spaced frequencies over the band.
fn band_grid(n: usize) -> Vec<f64> {
    let (lo, hi) = SINE_BAND_HZ;
    (0..n)
        .map(|k| lo * (hi / lo).powf(k as f64 / (n - 1) as f64))
        .collect()
}

/// The controlled rows of a stack.
fn controlled(s: SineStructure, h: &MatrixStack) -> MatrixStack {
    let (lines, _, n_d) = h.dims();
    let nc = s.n_controls();
    let mut out = MatrixStack::zeros(lines, nc, n_d);
    for k in 0..lines {
        for i in 0..nc {
            for j in 0..n_d {
                out.set(k, i, j, h.get(k, i, j));
            }
        }
    }
    out
}

/// Names, sizes, descriptions and conditioning claims; every structure validates at the
/// calibration rate and states a positive full level.
#[test]
fn mimo_sine_structures_by_name() {
    let names: Vec<&str> = SineStructure::ALL.iter().map(|s| s.name()).collect();
    assert_eq!(
        names,
        [
            "sine-linear-4x4",
            "sine-softening-4x4-mild",
            "sine-softening-4x4-moderate",
            "sine-hardening-4x4",
            "sine-tall-6x3",
            "sine-wide-3x6",
            "sine-base-article-6x4",
            "sine-near-singular-4x4",
        ]
    );
    let grid = band_grid(1200);
    for s in SineStructure::ALL {
        assert_eq!(SineStructure::from_name(s.name()), Some(s));
        let d = s.description();
        assert!(!d.is_empty() && !d.contains('\n'), "{}", s.name());
        let named = s.build();
        assert_eq!(named.name, s.name());
        let (n_out, n_d) = s.dims();
        assert_eq!(named.model.dims(), Some((n_out, n_d)));
        assert_eq!(named.geometry.inputs.len(), n_out);
        assert_eq!(named.geometry.drives.len(), n_d);
        named
            .model
            .validate(FS)
            .expect("validates at the calibration rate");
        assert!(s.full_level_g() > 0.0);
        assert_eq!(s.full_level_reference().len(), s.n_controls());
        assert_eq!(
            s.structure().is_level_dependent(),
            s.level_design().is_some()
        );
        // Every elastic mode's Q is between 16 and 50, and one sits near 40 Hz at 1 %.
        let p = s.structure();
        for m in &p.modes {
            let q = 0.5 / m.zeta;
            assert!(
                (16.0..=50.0).contains(&q),
                "{} mode at {} Hz has Q {q}",
                s.name(),
                m.f_hz
            );
        }
        assert!(p
            .modes
            .iter()
            .any(|m| (m.f_hz - 40.0).abs() < 5.0 && m.zeta == 0.01));
        // The claim, on the controlled rows over the band.
        let h = controlled(s, &named.model.analytic_frf_at(&grid, FS));
        let cond = linalg::condition_numbers(&h).expect("finite");
        let worst = cond.iter().copied().fold(0.0, f64::max);
        match named.conditioning {
            Conditioning::Well { band_hz, max } => {
                assert_eq!(band_hz, SINE_BAND_HZ);
                assert!(worst < max, "{}: condition {worst}", s.name());
                assert!(worst < 20.0, "{}: condition {worst}", s.name());
            }
            Conditioning::Ill { band_hz, exceeds } => {
                assert_eq!(band_hz, SINE_BAND_HZ);
                assert!(worst > exceeds, "{}: condition {worst}", s.name());
            }
        }
    }
    assert_eq!(SineStructure::from_name("sine-linear-5x5"), None);
}

/// A low-level tone on each drive in turn, played sample by sample through the
/// simulator's backend, gives a steady complex response equal to the exact small-signal
/// response at that frequency: magnitude within 0.01 % and phase within 0.1 mrad (the f32
/// samples and, on the level-dependent structures, the tone's own small envelope are
/// what is left). The phase convention is the module docs'.
#[test]
fn mimo_sine_low_level_tone_reads_the_small_signal_response() {
    const TONES: [f64; 11] = [
        5.0, 40.0, 96.0, 165.0, 230.0, 381.0, 430.0, 690.0, 1080.0, 1620.0, 2000.0,
    ];
    // Four seconds settle the 40 Hz mode (time constant 0.4 s) to well under 1e-4.
    let settle = 4 * N;
    let mut worst = (0.0f64, 0.0f64);
    for (si, s) in SineStructure::ALL.into_iter().enumerate() {
        let (n_out, n_d) = s.dims();
        let model = s.build().model;
        for j in 0..n_d {
            for t in 0..2 {
                let f = TONES[(si + 2 * j + 5 * t) % TONES.len()];
                let truth = model.analytic_frf_at(&[f], FS);
                // Ten nanovolts: every mode far under its reference amplitude, whatever the
                // pattern of the drive (the level rules grow linearly from zero, so the
                // tone must be small for its own movement to stay under the tolerance).
                let amp = 1e-8;
                let mut frame = vec![0.0f32; n_d];
                let mut buf = vec![0.0f32; n_out];
                let mut drive = Vec::with_capacity(N);
                let mut resp = vec![Vec::with_capacity(N); n_out];
                let mut b = running(s, 0.0, 1);
                for k in 0..settle + N {
                    frame[j] = (amp * (TAU * f * k as f64 / FS).cos()) as f32;
                    b.write_drive(&frame).unwrap();
                    b.read_input(&mut buf).unwrap();
                    if k >= settle {
                        drive.push(frame[j] as f64);
                        for (r, &y) in resp.iter_mut().zip(&buf) {
                            r.push(y as f64);
                        }
                    }
                }
                let d = demodulate(&drive, f, settle);
                for (i, r) in resp.iter().enumerate() {
                    let h = cdiv(demodulate(r, f, settle), d);
                    let e = cdiv(h, truth.get(0, i, j));
                    let (em, ep) = ((mag(e) - 1.0).abs(), arg(e).abs());
                    worst = (worst.0.max(em), worst.1.max(ep));
                    assert!(
                        em < 1e-4 && ep < 1e-4,
                        "{} drive {j} response {i} at {f} Hz: read {h}, exact {}",
                        s.name(),
                        truth.get(0, i, j)
                    );
                }
            }
        }
    }
    println!(
        "largest magnitude error {:.2e}, phase error {:.2e} rad",
        worst.0, worst.1
    );
}

/// The calibration: the drives that hold every control at full level (by the
/// small-signal response) at a mode's own resonance bring that mode, played sample by
/// sample through the linear structure, to its reference amplitude within 3 %.
#[test]
fn mimo_sine_full_level_brings_each_mode_to_its_reference_amplitude() {
    for s in SineStructure::ALL {
        let lin = s.linear_structure();
        let n_d = s.dims().1;
        for (m, mode) in lin.modes.iter().enumerate() {
            let f = mode.f_hz;
            let d = s.full_level_drive(f);
            let largest = d.iter().map(|v| v.norm()).fold(0.0, f64::max);
            assert!(largest < 0.5, "{} at {f} Hz needs {largest} V", s.name());
            let mut st = lin.stepper(FS).expect("a structure");
            let mut out = vec![0.0; s.dims().0];
            let mut frame = vec![0.0; n_d];
            let settle = ((12.0 / (mode.zeta * TAU * f)) * FS) as usize;
            for k in 0..settle {
                let ph = TAU * f * k as f64 / FS;
                for (x, dj) in frame.iter_mut().zip(&d) {
                    *x = (*dj * C64::new(ph.cos(), ph.sin())).re;
                }
                st.step(&frame, &mut out);
            }
            let ratio = st.envelope()[m] / mode.reference_amplitude;
            assert!(
                (ratio - 1.0).abs() < 0.03,
                "{} mode {m} at {f} Hz reaches {ratio} of its reference",
                s.name()
            );
        }
    }
}

/// What a stepped sine measures of one mode with its modal amplitude held: its
/// equivalent-linear natural frequency and damping.
#[derive(Debug, Clone, Copy)]
struct Resonance {
    f_hz: f64,
    zeta: f64,
}

/// Mode `k` of `p` alone (unit shapes), driven by a sine at `f_hz` whose modal amplitude
/// is held at `amplitude`, as a stepped sine with amplitude control holds it: every
/// decay time the drive is corrected toward the amplitude read from the output (the
/// acceleration over the frequency squared). Returns output over drive.
fn held_response(one: &JointedPlant, f_hz: f64, amplitude: f64) -> C64 {
    let m = &one.modes[0];
    let (w, w0) = (TAU * f_hz, TAU * m.f_hz);
    let mut st = one.stepper(FS).expect("a structure");
    let mut u = amplitude * C64::new(w0 * w0 - w * w, 2.0 * m.zeta * w0 * w).norm();
    // Three decay times: a correction's transient has fallen to 5 % by the next.
    let cyc = ((3.0 / (m.zeta * w0)) * f_hz).ceil().max(4.0);
    let window = (cyc * FS / f_hz).round() as usize;
    let mut out = [0.0];
    let mut k = 0usize;
    let mut run = |st: &mut hertzforge_hal_sim::Stepper, k: &mut usize, len: usize, u: f64| {
        let first = *k;
        let mut y = Vec::with_capacity(len);
        for _ in 0..len {
            st.step(&[u * (w * *k as f64 / FS).cos()], &mut out);
            y.push(out[0]);
            *k += 1;
        }
        fit(&y, f_hz, first)
    };
    let (mut held, mut gain, mut last) = (0, 0.5, 0.0f64);
    for _ in 0..600 {
        let y = run(&mut st, &mut k, window, u);
        let err = amplitude / (mag(y) / (w * w));
        if (err - 1.0).abs() < 5e-4 {
            held += 1;
            if held >= 3 {
                break;
            }
        } else {
            held = 0;
        }
        // Halve the correction each time it overshoots: a hardening mode near its
        // moved resonance answers more than in proportion.
        if (err - 1.0) * last < 0.0 {
            gain = (gain * 0.5f64).max(0.02);
        }
        last = err - 1.0;
        u *= err.powf(gain);
    }
    assert!(
        held >= 3,
        "the amplitude did not hold at {f_hz} Hz: last error {last}"
    );
    run(&mut st, &mut k, window, u) / u
}

/// A stepped sine of five steps a quarter to one and a quarter half-power bandwidths to
/// one `side` of `centre_hz` (+1 above, -1 below), each with the modal amplitude held:
/// at each step the equivalent-linear natural frequency and damping,
/// `w_n^2 - w^2 + 2 j zeta w_n w = -w^2 F / A` (`A / F` the step's response with the
/// structure's two-sample step delay taken out), and their mean. The steps agree with
/// each other (one resonance whose place depends on its amplitude only), which the
/// function checks. The side is the one where a held amplitude is stable: above a
/// softening resonance, below a hardening one (on the other side the held amplitude can
/// sit on the branch a fixed force jumps away from).
fn stepped_resonance(one: &JointedPlant, centre_hz: f64, side: f64, amplitude: f64) -> Resonance {
    let m = &one.modes[0];
    let bw = 2.0 * m.zeta * m.f_hz;
    let delay = one.step_delay_samples() as f64;
    let steps: Vec<Resonance> = (0..5)
        .map(|i| {
            let f = centre_hz + side * bw * (0.25 + 0.25 * i as f64);
            let w = TAU * f;
            let ph = w * delay / FS;
            let g = held_response(one, f, amplitude) * C64::new(ph.cos(), ph.sin());
            let dd = cdiv(C64::new(-w * w, 0.0), g);
            let wn = (dd.re + w * w).sqrt();
            Resonance {
                f_hz: wn / TAU,
                zeta: dd.im / (2.0 * wn * w),
            }
        })
        .collect();
    let mean = Resonance {
        f_hz: steps.iter().map(|r| r.f_hz).sum::<f64>() / 5.0,
        zeta: steps.iter().map(|r| r.zeta).sum::<f64>() / 5.0,
    };
    for r in &steps {
        assert!(
            (r.f_hz - mean.f_hz).abs() < 0.02 * bw && (r.zeta / mean.zeta - 1.0).abs() < 0.02,
            "the steps disagree: {steps:?}"
        );
    }
    mean
}

/// `(movement in half-power bandwidths, damping at full level over damping at low)` of
/// mode `k`, each mode alone with its modal amplitude held at its reference amplitude
/// (full level) and at a thousandth of it (low level). The full-level steps are placed
/// beside where the rule puts the resonance; with no rule, beside the low-level one.
fn measured_movement(p: &JointedPlant, k: usize) -> (f64, f64) {
    let one = p.single_mode(k);
    let m = &one.modes[0];
    let r = m.reference_amplitude;
    let (expect, side) = m.level.map_or((1.0, 1.0), |l| {
        let f = l.frequency_factor(1.0);
        (f, if f > 1.0 { -1.0 } else { 1.0 })
    });
    let low = stepped_resonance(&one, m.f_hz, side, 1e-3 * r);
    let full = stepped_resonance(&one, m.f_hz * expect, side, r);
    let bw = 2.0 * m.zeta * m.f_hz;
    ((full.f_hz - low.f_hz) / bw, full.zeta / low.zeta)
}

/// The calibration's falsifying test: a stepped sine beside each level-dependent
/// resonance, at low level and at full level, finds it moved by the stated number of
/// half-power bandwidths (mild: under a quarter; moderate: a half to one; hardening: up
/// a half to one) and its damping grown as stated. The same structure with its level
/// rules switched off (`JointedPlant::linear`) moves under a twentieth of a bandwidth,
/// which is outside every one of those bands.
#[test]
fn mimo_sine_level_rule_moves_each_resonance_as_stated() {
    for s in [
        SineStructure::SofteningMild4x4,
        SineStructure::SofteningModerate4x4,
        SineStructure::Hardening4x4,
    ] {
        let d = s.level_design().expect("level dependent");
        let p = s.structure();
        let (band, damping) = match s {
            SineStructure::SofteningMild4x4 => ((-0.25, -0.08), 1.2),
            SineStructure::SofteningModerate4x4 => ((-1.0, -0.5), 1.75),
            _ => ((0.5, 1.0), 1.25),
        };
        for &k in d.modes {
            let (moved, grown) = measured_movement(&p, k);
            println!(
                "{} mode {k} at {} Hz: moved {moved:+.3} bandwidths, damping x {grown:.3}",
                s.name(),
                p.modes[k].f_hz
            );
            assert!(
                moved > band.0 && moved < band.1,
                "{} mode {k} moved {moved} bandwidths, outside {band:?}",
                s.name()
            );
            assert!(
                (moved - d.bandwidths).abs() < 0.02,
                "{} mode {k} moved {moved}, designed {}",
                s.name(),
                d.bandwidths
            );
            assert!(
                (grown / damping - 1.0).abs() < 0.01,
                "{} mode {k} damping x {grown}, stated x {damping}",
                s.name()
            );
        }
        // Switched off, nothing moves.
        let off = p.linear();
        let k = d.modes[0];
        let (moved, grown) = measured_movement(&off, k);
        assert!(
            moved.abs() < 0.05 && (grown - 1.0).abs() < 0.02,
            "{moved} {grown}"
        );
    }
}

/// The near-singular structure: its controlled response's smallest singular value
/// falls about 30 dB (27 to 35) under its largest near [`SINE_NEAR_SINGULAR_HZ`], the
/// deepest in the band, while the linear 4 x 4 it is built on stays within 22 dB
/// everywhere.
#[test]
fn mimo_sine_near_singular_frequency() {
    let grid = band_grid(4000);
    let depth = |s: SineStructure| -> (f64, f64) {
        let h = s.build().model.analytic_frf_at(&grid, FS);
        let cond = linalg::condition_numbers(&controlled(s, &h)).expect("finite");
        let (i, c) = cond
            .iter()
            .copied()
            .enumerate()
            .fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a });
        (grid[i], 20.0 * c.log10())
    };
    let (f, db) = depth(SineStructure::NearSingular4x4);
    println!("deepest {db:.2} dB at {f:.1} Hz");
    assert!((27.0..35.0).contains(&db), "{db} dB");
    assert!((f / SINE_NEAR_SINGULAR_HZ - 1.0).abs() < 0.01, "at {f} Hz");
    let (_, lin_db) = depth(SineStructure::Linear4x4);
    assert!(lin_db < 22.0, "the linear 4 x 4 reaches {lin_db} dB");
}

/// The base and article: with the four base controls held at full level, the article's
/// responses read about four times the base (3.5 to 4.5) at the article's resonance,
/// and near 1 well away from it.
#[test]
fn mimo_sine_article_reads_about_four_times_the_base() {
    let s = SineStructure::BaseArticle6x4;
    let p = s.linear_structure();
    let article = p.modes.last().expect("the article's mode").f_hz;
    let read = |f: f64| -> f64 {
        let h = p.frf_at_envelope(&[f], FS, &vec![0.0; p.modes.len()]);
        let d = s.full_level_drive(f);
        (4..6)
            .map(|i| {
                (0..4)
                    .fold(C64::new(0.0, 0.0), |acc, j| acc + h.get(0, i, j) * d[j])
                    .norm()
                    / s.full_level_g()
            })
            .fold(0.0, f64::max)
    };
    let (f_peak, peak) = (0..800)
        .map(|k| article * (0.85 + 0.3 * k as f64 / 799.0))
        .map(|f| (f, read(f)))
        .fold((0.0, 0.0), |a, b| if b.1 > a.1 { b } else { a });
    println!("article reads {peak:.2} times the base at {f_peak:.1} Hz");
    assert!((3.5..4.5).contains(&peak), "{peak} at {f_peak} Hz");
    for f in [20.0, 60.0] {
        let r = read(f);
        assert!((0.7..1.3).contains(&r), "{r} at {f} Hz");
    }
}

/// Two builds of a structure give the same samples, noise included; the drive is a
/// seeded random one on every drive at a tenth of a volt.
#[test]
fn mimo_sine_structures_are_reproducible() {
    let play = |s: SineStructure| -> Vec<u32> {
        let (n_out, n_d) = s.dims();
        let mut b = running(s, 1e-3, 7);
        let mut state: u64 = 0x5EED_5151_0001;
        let mut frame = vec![0.0f32; n_d];
        let mut buf = vec![0.0f32; n_out];
        let mut bits = Vec::new();
        for _ in 0..4096 {
            for x in frame.iter_mut() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *x = 0.1 * ((state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32;
            }
            b.write_drive(&frame).unwrap();
            b.read_input(&mut buf).unwrap();
            bits.extend(buf.iter().map(|v| v.to_bits()));
        }
        bits
    };
    for s in SineStructure::ALL {
        assert_eq!(s.build(), s.build(), "{}", s.name());
        assert_eq!(play(s), play(s), "{}", s.name());
    }
}
