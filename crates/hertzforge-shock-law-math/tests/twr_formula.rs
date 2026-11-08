//! `fd_correct` is time waveform replication's iteration: its seed and its
//! correction equal `hertzforge_twr::law::{initial_drive, correct}` bit for
//! bit, up to the transform convention.
//!
//! Time waveform replication forms its spectra with `hertzforge_dsp`'s
//! positive-exponent transform and builds its inverse for that convention; the
//! `shock-classical` world is negative-exponent. For a real record the two
//! spectra are conjugates, so the law is handed the conjugate of the inverse
//! time waveform replication would use, and its drive spectrum must be the
//! conjugate of time waveform replication's (the imaginary parts negated,
//! nothing else) and its drive record the same samples. Zeros may differ in
//! sign; every other value is compared by its bits.
//!
//! Two fixtures: time waveform replication's own flat plant (`H = 2`, its
//! Tikhonov inverse, coherence 0.99) iterated against its own response, and
//! random spectra, masks and responses at several parameter sets, the growth
//! clamp engaged.

use hertzforge_shock_law_math::classical::{FdCorrect, FdCorrectParams};
use hertzforge_shock_law_math::Cplx;
use hertzforge_twr::grid::{irfft_half, rfft_half, RecordGrid};
use hertzforge_twr::inverse::GridInverse;
use hertzforge_twr::law::{correct, initial_drive, LawConfig};

/// A small deterministic generator, so the fixture is the same on every run.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
    fn signed(&mut self) -> f64 {
        2.0 * self.next() - 1.0
    }
}

fn same_number(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a == 0.0 && b == 0.0)
}

fn assert_conjugates(law: &[Cplx], twr: &[Cplx], what: &str) {
    assert_eq!(law.len(), twr.len(), "{what}");
    for (k, (a, b)) in law.iter().zip(twr).enumerate() {
        assert!(
            same_number(a.0, b.0) && same_number(a.1, -b.1),
            "{what}: bin {k}: law {a:?}, time waveform replication {b:?}"
        );
    }
}

fn assert_same_record(law: &[f64], twr: &[f64], what: &str) {
    assert_eq!(law.len(), twr.len(), "{what}");
    for (i, (a, b)) in law.iter().zip(twr).enumerate() {
        assert!(same_number(*a, *b), "{what}: sample {i}: {a} against {b}");
    }
}

/// What both laws are built from: the grid length, the reference, time
/// waveform replication's inverse and the trusted bins.
struct Fixture<'a> {
    n: usize,
    reference: &'a [f64],
    g_twr: &'a [Cplx],
    mask: &'a [bool],
}

/// Run both laws from the same inputs: the seed, then a correction per
/// response. `respond` makes each response from the drive just played.
fn compare(
    f: &Fixture<'_>,
    params: FdCorrectParams,
    pulses: usize,
    mut respond: impl FnMut(&[f64]) -> Vec<f64>,
    what: &str,
) -> usize {
    let Fixture {
        n,
        reference,
        g_twr,
        mask,
    } = *f;
    let inv = GridInverse {
        g: g_twr.to_vec(),
        coherence: vec![0.99; g_twr.len()],
        eps: 0.01,
        hmax_inband: 2.0,
    };
    let cfg = LawConfig {
        beta: params.beta,
        max_db_per_iter: params.max_db_per_pulse,
        ..LawConfig::default()
    };
    let r_twr = rfft_half(reference, n);

    // Time waveform replication's seed, scaled as its service scales it.
    let scale = 10f64.powf(-params.initial_backoff_db / 20.0);
    let mut d_twr = initial_drive(&r_twr, &inv, mask);
    for c in d_twr.iter_mut() {
        c.0 *= scale;
        c.1 *= scale;
    }

    let g_world: Vec<Cplx> = g_twr.iter().map(|&(re, im)| (re, -im)).collect();
    let mut law = FdCorrect::new(params, n, reference, &g_world, mask).unwrap();
    let seed = law.seed().unwrap();
    assert_conjugates(law.drive_spectrum(), &d_twr, &format!("{what}: seed"));
    let mut drive_twr = irfft_half(&d_twr, n);
    assert_same_record(&seed.drive, &drive_twr, &format!("{what}: seed drive"));

    let mut clamped_total = 0usize;
    let mut drive = seed.drive;
    for p in 0..pulses {
        let y = respond(&drive);
        let y_twr = rfft_half(&y, n);
        let clamped = correct(&mut d_twr, &r_twr, &y_twr, &inv, mask, &cfg);
        let step = law.iterate(&y).unwrap();
        assert_conjugates(law.drive_spectrum(), &d_twr, &format!("{what}: pulse {p}"));
        drive_twr = irfft_half(&d_twr, n);
        assert_same_record(&step.drive, &drive_twr, &format!("{what}: pulse {p} drive"));
        assert_eq!(
            step.report.clamped_bins, clamped,
            "{what}: pulse {p} clamps"
        );
        clamped_total += clamped;
        drive = step.drive;
    }
    clamped_total
}

#[test]
fn on_the_flat_plant_the_seed_and_corrections_are_time_waveform_replications() {
    // Time waveform replication's own law fixture: n = 4096 at 8192 Hz, H = 2,
    // G = 2 / (4 + 0.01) on every bin.
    let grid = RecordGrid::new(8192.0, 3000, 500, 500);
    let n = grid.n;
    let g = vec![(2.0 / 4.01, 0.0); grid.n_bins()];
    // A half-sine section with its quiet pads, the mask the band 20-2000 Hz.
    let section: Vec<f64> = (0..3000)
        .map(|i| {
            let t = i as f64 / 3000.0;
            if (0.4..0.6).contains(&t) {
                (std::f64::consts::PI * (t - 0.4) / 0.2).sin()
            } else {
                0.0
            }
        })
        .collect();
    let reference = grid.window_from_section(&section);
    let mask: Vec<bool> = (0..grid.n_bins())
        .map(|k| (20.0..=2000.0).contains(&grid.freq(k)))
        .collect();
    for params in [
        FdCorrectParams::default(),
        FdCorrectParams {
            beta: 0.25,
            initial_backoff_db: 3.0,
            ..FdCorrectParams::default()
        },
    ] {
        // The plant doubles the drive: H = 2.
        compare(
            &Fixture {
                n,
                reference: &reference,
                g_twr: &g,
                mask: &mask,
            },
            params,
            4,
            |d| d.iter().map(|v| 2.0 * v).collect(),
            &format!("flat plant at {params:?}"),
        );
    }
}

#[test]
fn on_random_spectra_the_seed_and_corrections_are_time_waveform_replications() {
    let mut rng = Lcg(0x005E_ED0F_C1A5_51CA);
    let n = 1024;
    let bins = n / 2 + 1;
    let mut clamped = 0usize;
    for case in 0..6 {
        let reference: Vec<f64> = (0..n).map(|_| rng.signed()).collect();
        let g: Vec<Cplx> = (0..bins)
            .map(|_| {
                let (m, ph) = (2.0 * rng.next(), std::f64::consts::TAU * rng.next());
                (m * ph.cos(), m * ph.sin())
            })
            .collect();
        let mask: Vec<bool> = (0..bins)
            .map(|k| k > 0 && k < bins - 1 && rng.next() < 0.7)
            .collect();
        let params = FdCorrectParams {
            beta: [0.5, 0.25, 1.0, 0.05, 0.75, 0.5][case],
            max_db_per_pulse: [6.0, 1.0, 9.0, 3.0, 1.5, 6.0][case],
            initial_backoff_db: [0.0, 3.0, 12.0, 0.0, 6.0, 1.0][case],
        };
        // Random responses, some far from the reference, so the clamp works.
        let mut gen = Lcg(rng.0 ^ 0xA5A5);
        clamped += compare(
            &Fixture {
                n,
                reference: &reference,
                g_twr: &g,
                mask: &mask,
            },
            params,
            5,
            |_| (0..n).map(|_| 3.0 * gen.signed()).collect(),
            &format!("random case {case}"),
        );
    }
    assert!(clamped > 0, "the fixture never engaged the growth clamp");
}
