//! The cross-coupled plant identified through the backend's own drive and input path,
//! checked against its exact FRF.
//!
//! Every drive plays independent periodic random noise (uniform magnitude, a random
//! phase per line per drive, new phases for each realization). After the plant has
//! settled into the periodic steady state, one period of every drive and every input is
//! kept, so each kept period is exactly periodic: with a rectangular window and no
//! overlap there is no leakage, and the only differences from the truth are the noise
//! floor and whatever transient is left. The FRF matrix is the H1 estimate per line,
//! `H1 = G_yx G_xx^-1`, from the workspace's cross-spectral estimator, with the multiple
//! coherence of each input `gamma2_i = (H1 G_xy)_ii / G_yy,ii` (standard; Bendat and
//! Piersol, Random Data). It is compared, element by element, with
//! `SimBackend::analytic_frf` on the in-band lines where the multiple coherence is high.
//!
//! Spectral convention: `G = E[X X^H]` with `X` the transform with the NEGATIVE
//! exponent (the convention the plant's analytic FRF uses), so
//! `G_xx[j][l] = E[X_j conj(X_l)]` and `G_yx[i][j] = E[Y_i conj(X_j)]`.
//! `cross_welch_with(x, y)` returns `<X conj(Y)>` in the workspace FFT's own sign. That
//! FFT's forward transform is documented as negative-exponent but its butterflies use
//! the positive exponent (`fft_exponent_sign` below measures it), which is why its
//! `<X conj(Y)>` FRF convention comes out with the right phase. This test measures the
//! sign rather than trusting either, and maps: for a negative-exponent FFT
//! `G_xx[j][l] = cw(x_j, x_l)` and `G_yx[i][j] = conj(cw(x_j, y_i))`; for a
//! positive-exponent one, the conjugates of both.
//!
//! Against the single-path plant this replaced (one response from the first drive,
//! copied to every input) the estimate is rank one and every off-diagonal element and
//! every drive after the first is wrong by tens of dB; the test was run that way first
//! and failed.

mod common;

use common::{arg, cdiv, inverse, mag, Rng};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_dsp::fft;
use hertzforge_dsp::frf_ext::cross_welch_with;
use hertzforge_dsp::spectrum_offline::Detrend;
use hertzforge_hal::{AcquisitionConfig, Backend};
use hertzforge_hal_sim::plants::{Conditioning, NamedPlant, StandardPlant};
use hertzforge_hal_sim::{FrfPlant, PlantModel, SimBackend, SimConfig, C64};

/// What one identification run measured.
#[derive(Debug)]
struct Identified {
    /// Worst magnitude error over the compared elements, dB.
    max_db: f64,
    /// Worst phase error over the compared elements, degrees.
    max_deg: f64,
    /// Root-mean-square magnitude error, dB.
    rms_db: f64,
    /// Elements compared, and elements skipped as more than 40 dB below their row.
    compared_elements: usize,
    excluded_elements: usize,
    /// Lines compared (high coherence, in band) and lines in band.
    compared_lines: usize,
    band_lines: usize,
    /// Lowest multiple coherence over the compared lines and inputs.
    min_coherence: f64,
    realizations: usize,
    settle_periods: usize,
}

/// An element more than 40 dB below the largest element of its row (a transfer zero
/// crossing a line, say) is not compared: the row's noise, which the multiple coherence
/// measures, is spread over its elements in absolute terms, so the relative error of an
/// element that deep is set by that noise and says nothing about the plant.
const ELEMENT_FLOOR: f64 = 0.01;

fn table(n_in: usize, n_d: usize) -> ChannelTable {
    let mut t = ChannelTable::new();
    for i in 0..n_in {
        let mut c = Channel::new(i as u32 + 1);
        c.role = ChannelRole::Control;
        c.range_max = 1e9;
        t.channels.push(c);
    }
    for j in 0..n_d {
        let mut d = Channel::new((n_in + j) as u32 + 1);
        d.role = ChannelRole::Drive;
        t.channels.push(d);
    }
    t
}

/// The sign of the workspace FFT's forward exponent, measured: the transform of a unit
/// sample at `n = 1` (length 4) is `exp(-j pi/2) = -j` at `k = 1` for the textbook
/// negative exponent, `+j` for the positive one.
fn fft_exponent_sign() -> f64 {
    let x = fft::fft(&[(0.0, 0.0), (1.0, 0.0), (0.0, 0.0), (0.0, 0.0)]);
    x[1].1.signum()
}

/// What the workspace FFT's forward sign is today, so the day it changes is noticed
/// (and this file's mapping, which follows the measured sign either way, keeps working).
#[test]
fn the_workspace_fft_forward_transform_uses_the_positive_exponent() {
    assert_eq!(
        fft_exponent_sign(),
        1.0,
        "the forward FFT's sign changed; the dsp crate's cross-spectrum phase convention \
         depends on it"
    );
}

/// One period of periodic random noise: magnitude `amp` on lines 1 to `n/2 - 1`, a
/// random phase on each, nothing at DC or Nyquist.
fn periodic_random(n: usize, amp: f64, rng: &mut Rng) -> Vec<f64> {
    let mut spec: Vec<fft::Cplx> = vec![(0.0, 0.0); n];
    for k in 1..n / 2 {
        let ph = std::f64::consts::TAU * rng.unit();
        let a = amp * n as f64 / 2.0;
        spec[k] = (a * ph.cos(), a * ph.sin());
        spec[n - k] = (a * ph.cos(), -a * ph.sin());
    }
    fft::ifft(&spec).into_iter().map(|c| c.0).collect()
}

/// The slowest mode's decay time constant `Q / (pi f)`, seconds.
fn slowest_decay_s(model: &PlantModel) -> f64 {
    match model {
        PlantModel::Modal(m) => m
            .modes
            .iter()
            .map(|mode| mode.q / (std::f64::consts::PI * mode.f_hz))
            .fold(0.0, f64::max),
        _ => 0.0,
    }
}

/// How long after a new realization starts the inputs are in the periodic steady state:
/// seven decay times of the slowest mode (a 0.1 % remainder) for a modal plant, the
/// FIR's whole memory (exact) for an FRF plant.
fn settle_s(model: &PlantModel, fs: f64) -> f64 {
    match model {
        PlantModel::Frf(p) => {
            let t = p.impulse_responses(fs).expect("a playable stack");
            (t.taps + t.block + p.transport_delay_samples) as f64 / fs
        }
        _ => 7.0 * slowest_decay_s(model),
    }
}

fn identify(plant: &NamedPlant, fs: f64, n: usize, realizations: usize, noise: f64) -> Identified {
    let (n_in, n_d) = plant.model.dims().expect("a sized plant");
    let mut b: SimBackend = SimConfig::new()
        .sample_rate_hz(fs)
        .plant(plant.model.clone())
        .noise_level(noise)
        .shared_clock(true)
        .seed(0x1D_E471_F1ED)
        .output_buffer_frames(2048)
        .build();
    b.open().unwrap();
    b.configure(&table(n_in, n_d), &AcquisitionConfig::new(fs, 1024))
        .expect("the table matches the plant");
    b.arm().unwrap();
    b.start().unwrap();

    let period_s = n as f64 / fs;
    let settle_periods = (settle_s(&plant.model, fs) / period_s).ceil() as usize;
    let amp = 1.0 / ((n / 2 - 1) as f64 / 2.0).sqrt(); // about 1 V rms per drive
    let mut rng = Rng::new(0xF00D_5EED);
    let mut x: Vec<Vec<f64>> = vec![Vec::with_capacity(realizations * n); n_d];
    let mut y: Vec<Vec<f64>> = vec![Vec::with_capacity(realizations * n); n_in];
    let chunk = 1024usize.min(n);
    let mut wbuf = vec![0.0f32; chunk * n_d];
    let mut rbuf = vec![0.0f32; chunk * n_in];
    for _ in 0..realizations {
        let sig: Vec<Vec<f32>> = (0..n_d)
            .map(|_| {
                periodic_random(n, amp, &mut rng)
                    .into_iter()
                    .map(|v| v as f32)
                    .collect()
            })
            .collect();
        for period in 0..=settle_periods {
            let keep = period == settle_periods;
            for start in (0..n).step_by(chunk) {
                for f in 0..chunk {
                    for j in 0..n_d {
                        wbuf[f * n_d + j] = sig[j][start + f];
                    }
                }
                assert_eq!(b.write_drive(&wbuf).unwrap(), chunk);
                b.read_input(&mut rbuf).unwrap();
                if keep {
                    for f in 0..chunk {
                        for (j, xj) in x.iter_mut().enumerate() {
                            xj.push(sig[j][start + f] as f64);
                        }
                        for (i, yi) in y.iter_mut().enumerate() {
                            yi.push(rbuf[f * n_in + i] as f64);
                        }
                    }
                }
            }
        }
    }

    let window = vec![1.0f64; n];
    let cw =
        |a: &[f64], b: &[f64]| cross_welch_with(a, b, fs, &window, 0.0, Detrend::None).unwrap();
    // `<X conj(Y)>` in the textbook (negative-exponent) sign, whatever the FFT's is.
    let positive_exponent = fft_exponent_sign() > 0.0;
    let textbook = |re: f64, im: f64| {
        let v = C64::new(re, im);
        if positive_exponent {
            v.conj()
        } else {
            v
        }
    };
    let lines = n / 2 + 1;
    let mut gxx = vec![C64::new(0.0, 0.0); lines * n_d * n_d];
    for j in 0..n_d {
        for l in j..n_d {
            let s = cw(&x[j], &x[l]);
            assert_eq!(s.segments, realizations);
            for k in 0..lines {
                let v = textbook(s.gxy[k].0, s.gxy[k].1);
                gxx[(k * n_d + j) * n_d + l] = v;
                gxx[(k * n_d + l) * n_d + j] = v.conj();
            }
        }
    }
    let mut gyx = vec![C64::new(0.0, 0.0); lines * n_in * n_d];
    let mut gyy = vec![0.0f64; lines * n_in];
    for i in 0..n_in {
        for j in 0..n_d {
            let s = cw(&x[j], &y[i]);
            for k in 0..lines {
                gyx[(k * n_in + i) * n_d + j] = textbook(s.gxy[k].0, s.gxy[k].1).conj();
                if j == 0 {
                    gyy[k * n_in + i] = s.gyy[k];
                }
            }
        }
    }

    let df = fs / n as f64;
    let truth = b.analytic_frf(lines, df);
    let band = match plant.conditioning {
        Conditioning::Well { band_hz, .. } | Conditioning::Ill { band_hz, .. } => band_hz,
    };
    let (mut max_db, mut max_deg, mut sum_sq, mut count) = (0.0f64, 0.0f64, 0.0f64, 0usize);
    let (mut compared_lines, mut band_lines, mut min_coh) = (0usize, 0usize, 1.0f64);
    let mut excluded = 0usize;
    for k in 1..lines - 1 {
        let f = k as f64 * df;
        if f < band.0 || f > band.1 {
            continue;
        }
        band_lines += 1;
        let gx = &gxx[k * n_d * n_d..(k + 1) * n_d * n_d];
        let inv = inverse(gx, n_d).expect("independent drives give an invertible G_xx");
        let mut line_ok = true;
        let mut h1 = vec![C64::new(0.0, 0.0); n_in * n_d];
        let mut coh = vec![0.0f64; n_in];
        for i in 0..n_in {
            for j in 0..n_d {
                let mut s = C64::new(0.0, 0.0);
                for l in 0..n_d {
                    s += gyx[(k * n_in + i) * n_d + l] * inv[l * n_d + j];
                }
                h1[i * n_d + j] = s;
            }
            let num: f64 = (0..n_d)
                .map(|j| (h1[i * n_d + j] * gyx[(k * n_in + i) * n_d + j].conj()).re)
                .sum();
            coh[i] = num / gyy[k * n_in + i];
            if coh[i] < 0.99 {
                line_ok = false;
            }
        }
        if !line_ok {
            continue;
        }
        compared_lines += 1;
        for i in 0..n_in {
            min_coh = min_coh.min(coh[i]);
            let row_max = (0..n_d)
                .map(|j| mag(truth.get(k, i, j)))
                .fold(0.0, f64::max);
            for j in 0..n_d {
                let t = truth.get(k, i, j);
                if mag(t) < ELEMENT_FLOOR * row_max {
                    excluded += 1;
                    continue;
                }
                let r = cdiv(h1[i * n_d + j], t);
                let db = 20.0 * mag(r).log10();
                let deg = arg(r).to_degrees();
                max_db = max_db.max(db.abs());
                max_deg = max_deg.max(deg.abs());
                sum_sq += db * db;
                count += 1;
            }
        }
    }
    Identified {
        max_db,
        max_deg,
        rms_db: (sum_sq / count.max(1) as f64).sqrt(),
        compared_elements: count,
        excluded_elements: excluded,
        compared_lines,
        band_lines,
        min_coherence: min_coh,
        realizations,
        settle_periods,
    }
}

fn assert_identified(name: &str, r: &Identified) {
    println!(
        "{name}: worst {:.4} dB and {:.3} deg, rms {:.5} dB over {} elements ({} more than \
         40 dB below their row skipped) on {} of {} in-band lines; lowest multiple \
         coherence {:.7}; {} realizations after {} settling periods each",
        r.max_db,
        r.max_deg,
        r.rms_db,
        r.compared_elements,
        r.excluded_elements,
        r.compared_lines,
        r.band_lines,
        r.min_coherence,
        r.realizations,
        r.settle_periods
    );
    assert!(
        r.compared_lines * 10 >= r.band_lines * 9,
        "{name}: only {} of {} in-band lines had multiple coherence >= 0.99",
        r.compared_lines,
        r.band_lines
    );
    assert!(
        r.max_db <= 0.2 && r.max_deg <= 2.0,
        "{name}: identified FRF differs from the analytic one by up to {:.3} dB and {:.3} deg",
        r.max_db,
        r.max_deg
    );
}

/// 4 x 4: every element of the identified FRF matrix within 0.2 dB and 2 degrees of the
/// plant's exact FRF on every high-coherence line from 20 Hz to 2 kHz.
#[test]
fn h1_identification_of_the_4x4_modal_plant_matches_its_analytic_frf() {
    let plant = StandardPlant::Modal4x4.build();
    let r = identify(&plant, 8192.0, 4096, 8, 1e-5);
    assert_identified("modal-4x4", &r);
}

/// The FRF plant replaying the 4 x 4 plant's exact FRF (2 Hz lines) is identified back
/// to what it reports it plays (the stack behind its block latency), to the same bars.
#[test]
fn h1_identification_of_an_frf_plant_replaying_the_4x4_matches_its_analytic_frf() {
    let fs = 8192.0;
    let mut plant = StandardPlant::Modal4x4.build();
    let stack = plant.model.analytic_frf(2049, 2.0, fs);
    plant.model = PlantModel::Frf(FrfPlant::new(stack, 2.0));
    let r = identify(&plant, fs, 4096, 8, 1e-5);
    assert_identified("frf replay of modal-4x4", &r);
}

/// 12 x 6, the same check (longer; run with `--ignored`).
#[test]
#[ignore = "longer (about 6 s in a debug build); run with --ignored"]
fn h1_identification_of_the_12x6_modal_plant_matches_its_analytic_frf() {
    let plant = StandardPlant::Modal12x6.build();
    let r = identify(&plant, 8192.0, 4096, 18, 1e-5);
    assert_identified("modal-12x6", &r);
}
