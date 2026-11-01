//! Offline alignment discriminator (2026-08-08): does the sweep's alignment row
//! reproduce its chassis result (-2370, r 0.9725) in PURE MATH, with no
//! hardware at all?
//!
//! The chassis measured a bit-stable residual of -2049 frames across two runs
//! AND across the greedy-writer change (which refuted the underrun
//! hypothesis), while the tolerance row - which only inspects the +/-0.4 T
//! window around the main pulse - passes at its expected position. This
//! example synthesizes the reference and an IDEAL record (quiet + composite +
//! quiet, delayed by the 39-frame ADC group delay) and runs the row's EXACT
//! `xcorr_peak` on the pair.
//!
//! If this prints -2370-ish: the row's measurement is an artifact of the
//! 4.5x-shoulder geometry in the xcorr itself, and the chassis was never
//! wrong. If it prints -321: the artifact is physical and lives on the wire.

use eigen_shock_host::spec::{ClassicalPulseSpec, ClassicalShape};

const FS: f64 = 51_200.0;
const T_MS: f64 = 11.0;
const AMPLITUDE: f64 = 0.5;
const QUIET: usize = 282; // max(0.5 T, 5 ms) at FS
const GROUP_DELAY: usize = 39;

// Byte-for-byte copy of ni_module_sweep.rs's xcorr_peak (its doc says the two
// must stay the same measurement; this example is the offline third leg).
fn xcorr_peak(a: &[f64], b: &[f64]) -> (isize, f64) {
    if a.is_empty() || b.is_empty() {
        return (0, 0.0);
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let (ma, mb) = (mean(a), mean(b));
    let ea: f64 = a.iter().map(|x| (x - ma) * (x - ma)).sum::<f64>().sqrt();
    let eb: f64 = b.iter().map(|x| (x - mb) * (x - mb)).sum::<f64>().sqrt();
    if ea <= 0.0 || eb <= 0.0 {
        return (0, 0.0);
    }
    let (n, m) = (a.len() as isize, b.len() as isize);
    let mut best_lag = 0isize;
    let mut best = 0.0f64;
    for lag in -(m - 1)..n {
        let mut acc = 0.0;
        let jlo = (-lag).max(0);
        let jhi = (n - lag).min(m);
        for j in jlo..jhi {
            acc += (a[(j + lag) as usize] - ma) * (b[j as usize] - mb);
        }
        let r = acc / (ea * eb);
        if r.abs() > best.abs() {
            best = r;
            best_lag = lag;
        }
    }
    (best_lag, best)
}

fn main() {
    let mut spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, AMPLITUDE, T_MS);
    spec.compensation.pre_frac = 4.5;
    spec.compensation.post_frac = 4.5;
    let reference = spec.synthesize(FS).waveform_g;

    // The IDEAL record: what a perfect wire hands back.
    let mut record = vec![0.0f64; QUIET + GROUP_DELAY];
    record.extend(reference.iter().copied());
    record.extend(std::iter::repeat_n(0.0, QUIET));

    let (lag, r) = xcorr_peak(&reference, &record);
    println!(
        "reference {} samples, ideal record {} samples",
        reference.len(),
        record.len()
    );
    println!("IDEAL-record xcorr: lag {lag} (expected -321), peak r {r:+.6}");

    // And the r the TRUE lag scores, for comparison with the best.
    let true_lag = -((QUIET + GROUP_DELAY) as isize);
    let r_at_true = {
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let (ma, mb) = (mean(&reference), mean(&record));
        let ea: f64 = reference
            .iter()
            .map(|x| (x - ma) * (x - ma))
            .sum::<f64>()
            .sqrt();
        let eb: f64 = record
            .iter()
            .map(|x| (x - mb) * (x - mb))
            .sum::<f64>()
            .sqrt();
        let (n, m) = (reference.len() as isize, record.len() as isize);
        let (jlo, jhi) = ((-true_lag).max(0), (n - true_lag).min(m));
        let mut acc = 0.0;
        for j in jlo..jhi {
            acc += (reference[(j + true_lag) as usize] - ma) * (record[j as usize] - mb);
        }
        acc / (ea * eb)
    };
    println!("r at the TRUE lag {true_lag}: {r_at_true:+.6}");
}
