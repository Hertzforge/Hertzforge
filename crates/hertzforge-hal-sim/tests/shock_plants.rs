//! The shock plants: the jointed modal plant (frequency down and damping up with a
//! running amplitude envelope, a delay per output) and the standard shock set, and the
//! proof that every plant that existed before them plays exactly what it played.

mod common;

use common::{arg, cdiv, condition_number, mag, Rng};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_dsp::fft;
use hertzforge_hal::{AcquisitionConfig, Backend, HalError};
use hertzforge_hal_sim::bench_network::BenchNetwork;
use hertzforge_hal_sim::jointed::MAX_DAMPING;
use hertzforge_hal_sim::plants::{
    reference_pulse, Conditioning, ShockPlant, StandardPlant, SHOCK_4X4_REFERENCE_VELOCITY,
    SHOCK_6X3_REFERENCE_VELOCITY, SHOCK_CALIBRATION_RATE_HZ, SHOCK_CALIBRATION_SAMPLES,
};
use hertzforge_hal_sim::{
    Discretization, JointLaw, JointedMode, JointedPlant, ModalPlant, ModalSoftening, Mode,
    ModeResponse, PlantModel, ResonatorBank, SimBackend, SimConfig, C64,
};
use std::f64::consts::TAU;

const FS: f64 = SHOCK_CALIBRATION_RATE_HZ;
const N: usize = SHOCK_CALIBRATION_SAMPLES;

fn table(n_control: usize, n_reference: usize, n_drive: usize) -> ChannelTable {
    let mut t = ChannelTable::new();
    let roles = std::iter::repeat_n(ChannelRole::Control, n_control)
        .chain(std::iter::repeat_n(ChannelRole::Reference, n_reference))
        .chain(std::iter::repeat_n(ChannelRole::Drive, n_drive));
    for (k, role) in roles.enumerate() {
        let mut c = Channel::new(k as u32 + 1);
        c.role = role;
        c.range_max = 1e9;
        t.channels.push(c);
    }
    t
}

fn running(model: PlantModel, fs: f64, noise: f64, t: &ChannelTable) -> SimBackend {
    let mut b = SimConfig::new()
        .sample_rate_hz(fs)
        .plant(model)
        .noise_level(noise)
        .shared_clock(true)
        .build();
    b.open().unwrap();
    b.configure(t, &AcquisitionConfig::new(fs, 256)).unwrap();
    b.arm().unwrap();
    b.start().unwrap();
    b
}

/// FNV-1a over the bits of every sample read back while `frames` frames of a seeded
/// random drive (one independent value per drive row) are played.
fn fingerprint(model: PlantModel, fs: f64, n_in: usize, n_d: usize, frames: usize) -> u64 {
    let mut b = running(model, fs, 1e-3, &table(n_in, 0, n_d));
    let mut state: u64 = 0x5EED_0F51_10C4;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    };
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut frame = vec![0.0f32; n_d];
    let mut buf = vec![0.0f32; n_in];
    for _ in 0..frames {
        for d in frame.iter_mut() {
            *d = next() as f32;
        }
        b.write_drive(&frame).unwrap();
        b.read_input(&mut buf).unwrap();
        for v in &buf {
            for byte in v.to_bits().to_le_bytes() {
                h ^= byte as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    h
}

/// A small fitted modal plant with one softening mode, as the one caller of the
/// softening builds it.
fn softening_plant() -> PlantModel {
    let modes = vec![
        Mode::new(180.0, 8.0, vec![1.0, 0.4, -0.3], vec![0.9, -0.5]),
        Mode {
            response: ModeResponse::Acceleration,
            ..Mode::new(900.0, 12.0, vec![-0.2, 1.0, 0.6], vec![0.3, 1.1])
        },
    ];
    PlantModel::Modal(ModalPlant {
        transport_delay_samples: 2,
        residual: vec![0.2, 0.0, 0.05, 0.0, 0.3, 0.0],
        discretization: Discretization::fitted(1500.0),
        softening: Some(ModalSoftening {
            modes: vec![0],
            reference_rms: vec![1e-5],
            fraction: 0.05,
            smoothing: 0.2,
            read_samples: 64,
            log: None,
        }),
        ..ModalPlant::new(2, 3, modes)
    })
}

/// Every plant that existed before the shock plants plays bit for bit what it played:
/// the fingerprints below were recorded at the commit the shock plants started from,
/// noise included, through the backend's own read path.
#[test]
fn mimo_shock_plant_existing_plants_unchanged() {
    let fs = 8192.0;
    let mut got: Vec<(String, u64)> = Vec::new();
    for p in StandardPlant::ALL {
        let (n_in, n_d) = p.dims();
        got.push((
            p.name().to_string(),
            fingerprint(p.build().model, fs, n_in, n_d, 2048),
        ));
    }
    got.push((
        "resonators-default".into(),
        fingerprint(PlantModel::default(), fs, 2, 1, 4096),
    ));
    got.push((
        "resonators-two-delayed".into(),
        fingerprint(
            PlantModel::Resonators(ResonatorBank {
                resonances: vec![(200.0, 2.5), (550.0, 2.5)],
                feedthrough: 0.3,
                transport_delay_samples: 5,
            }),
            fs,
            1,
            2,
            4096,
        ),
    ));
    got.push((
        "modal-softening".into(),
        fingerprint(softening_plant(), fs, 2, 3, 4096),
    ));
    got.push((
        "bench-network".into(),
        fingerprint(
            BenchNetwork::nominal().plant(2049, fs / 4096.0).model,
            fs,
            4,
            4,
            2048,
        ),
    ));
    for (name, h) in &got {
        println!("(\"{name}\", 0x{h:016x}),");
    }
    assert_eq!(
        RECORDED.len(),
        got.len(),
        "every plant has a recorded fingerprint"
    );
    // The plants' coefficients come through the platform's `sin`, `cos` and `powf`,
    // whose last bit differs between math libraries, so the recorded values are the
    // recording platform's. Elsewhere the run is checked for repeatability only.
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        for (name, h) in RECORDED {
            let g = got
                .iter()
                .find(|(n, _)| n == name)
                .expect("a recorded plant");
            assert_eq!(g.1, *h, "{name} no longer plays what it played");
        }
    } else {
        let again = fingerprint(StandardPlant::Modal4x4.build().model, fs, 4, 4, 2048);
        assert_eq!(again, got[0].1, "the same plant twice");
    }
}

/// Recorded on macOS (aarch64) at the commit the shock plants started from.
const RECORDED: &[(&str, u64)] = &[
    ("modal-4x4", 0xf6cfd70710d30c2e),
    ("modal-12x6", 0xd22f0ceb212b7f04),
    ("modal-12x6-ill", 0x1f2540ca49281d46),
    ("modal-24x24", 0x73c624995f271840),
    ("modal-72x24", 0xb5546d1ea4013704),
    ("rigid-plate-12x6", 0xc8af26196a91f997),
    ("resonators-default", 0x857deabecb5d38f1),
    ("resonators-two-delayed", 0x296be6aa49d3c81c),
    ("modal-softening", 0xe3335a63f354bff3),
    ("bench-network", 0xe54e7fb63a1c5b7b),
];

// ---------------------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------------------

/// The reference pulse's drives at `level_db` and the plant's response to them.
fn pulse_run(plant: &JointedPlant, level_db: f64) -> hertzforge_hal_sim::Simulated {
    let target = reference_pulse(FS, N, level_db);
    let drives = plant.least_drive_for(0, &target, FS).expect("a drive");
    plant.simulate(FS, &drives).expect("runs")
}

fn rms(v: &[f64]) -> f64 {
    (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt()
}

/// The steady response of a one-by-one plant to a sine at `f_hz` whose amplitude is
/// held, as a stepped-sine test with amplitude control holds it: every decay time the
/// drive is corrected toward the modal displacement amplitude `amplitude`, read from the
/// output (the acceleration over the frequency squared), until it holds within 0.2 %
/// over a window; then the complex output over drive over twenty cycles.
fn held_amplitude_response(p: &JointedPlant, f_hz: f64, amplitude: f64) -> C64 {
    let m = &p.modes[0];
    let w = TAU * f_hz;
    let w0 = TAU * m.f_hz;
    let mut st = p.stepper(FS).expect("a plant");
    let mut u = amplitude * mag(C64::new(w0 * w0 - w * w, 2.0 * m.zeta * w0 * w));
    let per = FS / f_hz;
    // A window of a decay time, and at least four cycles: a faster correction chases
    // the mode's own transient and cycles.
    let window = ((1.0 / (m.zeta * w0) * FS).max(4.0 * per)).round() as usize;
    let mut out = [0.0];
    let mut k = 0usize;
    // Least-squares cos and sin over `len` samples from sample `k` at drive `u`.
    let mut fit = |st: &mut hertzforge_hal_sim::Stepper, k: &mut usize, len: usize, u: f64| {
        let (mut cc, mut ss, mut cs, mut yc, mut ys) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for _ in 0..len {
            let (s, c) = (w * *k as f64 / FS).sin_cos();
            st.step(&[u * c], &mut out);
            let v = out[0];
            cc += c * c;
            ss += s * s;
            cs += c * s;
            yc += v * c;
            ys += v * s;
            *k += 1;
        }
        let det = cc * ss - cs * cs;
        let a = (yc * ss - ys * cs) / det;
        let b = (ys * cc - yc * cs) / det;
        // y = a cos + b sin = Re((a - j b) exp(j w t)).
        C64::new(a, -b)
    };
    let mut held = 0;
    let mut errs = Vec::new();
    for _ in 0..2000 {
        let y = fit(&mut st, &mut k, window, u);
        let q = mag(y) / (w * w);
        let err = amplitude / q;
        errs.push(err);
        if (err - 1.0).abs() < 2e-3 {
            held += 1;
            if held >= 8 {
                break;
            }
        } else {
            held = 0;
        }
        u *= err.powf(0.4);
    }
    assert!(
        held >= 8,
        "the amplitude did not hold at {f_hz} Hz: {:?}",
        &errs[errs.len() - 12..]
    );
    let y = fit(&mut st, &mut k, (20.0 * per).round() as usize, u);
    y / u
}

/// The resonance of a one-by-one plant held at `amplitude`: the peak of its response
/// magnitude and the half-power damping, `(f_peak, zeta)`.
fn held_amplitude_resonance(p: &JointedPlant, amplitude: f64, lo: f64, hi: f64) -> (f64, f64) {
    let gain = |f: f64| mag(held_amplitude_response(p, f, amplitude));
    // Golden section for the peak.
    let g = 0.5 * (5f64.sqrt() - 1.0);
    let (mut a, mut b) = (lo, hi);
    let (mut c, mut d) = (b - g * (b - a), a + g * (b - a));
    let (mut gc, mut gd) = (gain(c), gain(d));
    while b - a > 1e-4 * hi {
        if gc > gd {
            b = d;
            d = c;
            gd = gc;
            c = b - g * (b - a);
            gc = gain(c);
        } else {
            a = c;
            c = d;
            gc = gd;
            d = a + g * (b - a);
            gd = gain(d);
        }
    }
    let fp = 0.5 * (a + b);
    let half = gain(fp) / 2f64.sqrt();
    let edge = |mut inside: f64, mut outside: f64| {
        for _ in 0..30 {
            let mid = 0.5 * (inside + outside);
            if gain(mid) > half {
                inside = mid;
            } else {
                outside = mid;
            }
        }
        0.5 * (inside + outside)
    };
    let zeta0 = p.modes[0].zeta;
    let f1 = edge(fp, fp * (1.0 - 8.0 * zeta0));
    let f2 = edge(fp, fp * (1.0 + 8.0 * zeta0));
    (fp, (f2 - f1) / (2.0 * fp))
}

/// The sign of the workspace FFT's forward exponent (`-1` textbook).
fn fwd_sign() -> f64 {
    let x = fft::fft(&[(0.0, 0.0), (1.0, 0.0), (0.0, 0.0), (0.0, 0.0)]);
    if x[1].1 > 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// The lag of `del` behind `rf`, samples: the peak of their band-limited
/// cross-correlation, found on the integer grid and refined between samples.
fn xcorr_delay(rf: &[f64], del: &[f64]) -> f64 {
    let n = rf.len();
    let tr = |x: &[f64]| fft::fft(&x.iter().map(|&v| (v, 0.0)).collect::<Vec<_>>());
    let (a, b) = (tr(rf), tr(del));
    let sg = fwd_sign();
    // Textbook spectra: conjugate when the transform uses the positive exponent.
    let spec = |x: &(f64, f64)| C64::new(x.0, if sg > 0.0 { -x.1 } else { x.1 });
    let g: Vec<C64> = (0..n / 2)
        .map(|k| spec(&a[k]).conj() * spec(&b[k]))
        .collect();
    let corr = |tau: f64| -> f64 {
        g.iter()
            .enumerate()
            .skip(1)
            .map(|(k, v)| {
                let ph = TAU * k as f64 * tau / n as f64;
                (v * C64::new(ph.cos(), ph.sin())).re
            })
            .sum()
    };
    let coarse = (0..64)
        .map(|t| (t as f64, corr(t as f64)))
        .fold((0.0, f64::MIN), |b, x| if x.1 > b.1 { x } else { b })
        .0;
    let (mut lo, mut hi) = (coarse - 1.0, coarse + 1.0);
    for _ in 0..60 {
        let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
        if corr(m1) < corr(m2) {
            lo = m1;
        } else {
            hi = m2;
        }
    }
    0.5 * (lo + hi)
}

/// A drive signal: sample index and drive index to volts per volt of range.
type Signal = Box<dyn Fn(usize, usize) -> f64>;

fn one_mode(f_hz: f64, zeta: f64, joint: Option<JointLaw>) -> JointedPlant {
    JointedPlant {
        joint,
        ..JointedPlant::new(
            1,
            1,
            vec![JointedMode {
                f_hz,
                zeta,
                input_shape: vec![1.0],
                output_shape: vec![1.0],
                reference_amplitude: 1e-4,
            }],
        )
    }
}

// ---------------------------------------------------------------------------------------
// The jointed plant.
// ---------------------------------------------------------------------------------------

/// The FRF a jointed plant reports is the FRF of the plant the backend steps: a sine on
/// each drive of a linear jointed plant (mass line, transport delay, fractional output
/// delays), read back through the backend, matches `analytic_frf` to the f32 rounding.
#[test]
fn mimo_shock_plant_analytic_frf_is_what_the_backend_plays() {
    let fs = 4096.0;
    let n = 1024;
    let plant = JointedPlant {
        transport_delay_samples: 3,
        residual: vec![0.5, 0.1, 0.0, 0.05, 0.6, 0.2],
        feedthrough: 0.01,
        output_delays_s: vec![0.37 / fs, 2.6 / fs],
        ..JointedPlant::new(
            2,
            3,
            vec![
                JointedMode {
                    f_hz: 180.0,
                    zeta: 0.04,
                    input_shape: vec![1.0, 0.4, -0.3],
                    output_shape: vec![0.9, -0.5],
                    reference_amplitude: 1.0,
                },
                JointedMode {
                    f_hz: 1300.0,
                    zeta: 0.03,
                    input_shape: vec![-0.2, 1.0, 0.6],
                    output_shape: vec![0.3, 1.1],
                    reference_amplitude: 1.0,
                },
            ],
        )
    };
    let model = PlantModel::Jointed(plant);
    let mut b = running(model, fs, 0.0, &table(2, 0, 3));
    let truth = b.analytic_frf(n / 2 + 1, fs / n as f64);
    for (j, k) in [(0usize, 45usize), (1, 130), (2, 325), (0, 470)] {
        let w = TAU * k as f64 / n as f64;
        let (settle, mut frame, mut buf) = (4 * n, vec![0.0f32; 3], vec![0.0f32; 2]);
        let mut acc = [C64::new(0.0, 0.0); 2];
        let mut drv = C64::new(0.0, 0.0);
        for s in 0..settle + n {
            frame[j] = (w * s as f64).cos() as f32;
            b.write_drive(&frame).unwrap();
            b.read_input(&mut buf).unwrap();
            if s >= settle {
                let m = (s - settle) as f64;
                let e = C64::new((w * m).cos(), -(w * m).sin());
                drv += e * frame[j] as f64;
                for (a, &y) in acc.iter_mut().zip(&buf) {
                    *a += e * y as f64;
                }
            }
        }
        frame[j] = 0.0;
        for (i, a) in acc.iter().enumerate() {
            let h = cdiv(*a, drv);
            let t = truth.get(k, i, j);
            let r = cdiv(h, t);
            assert!(
                (mag(r) - 1.0).abs() < 1e-5 && arg(r).abs() < 1e-5,
                "drive {j} output {i} at bin {k}: measured {h} vs analytic {t}"
            );
        }
    }
}

/// `simulate` is the backend's step: the same drive through both gives the same
/// samples (to the backend's f32 output).
#[test]
fn mimo_shock_plant_simulate_is_the_backend_step() {
    let sp = ShockPlant::Jointed4x4Moderate;
    let plant = sp.plant();
    let target = reference_pulse(FS, 4096, 0.0);
    let drives = plant.least_drive_for(0, &target, FS).unwrap();
    // The drive as the backend receives it, in f32.
    let drives: Vec<Vec<f64>> = drives
        .iter()
        .map(|d| d.iter().map(|&v| v as f32 as f64).collect())
        .collect();
    let sim = plant.simulate(FS, &drives).unwrap();
    let mut b = running(sp.build().model, FS, 0.0, &table(4, 0, 4));
    let mut buf = [0.0f32; 4];
    for k in 0..4096 {
        let frame: Vec<f32> = drives.iter().map(|d| d[k] as f32).collect();
        b.write_drive(&frame).unwrap();
        b.read_input(&mut buf).unwrap();
        for (i, &v) in buf.iter().enumerate() {
            assert_eq!(v, sim.outputs[i][k] as f32, "output {i} sample {k}");
        }
    }
}

/// The same seed gives the same samples, noise included; another seed gives others.
#[test]
fn mimo_shock_plant_same_seed_same_samples() {
    let run = |seed: u64| -> Vec<f32> {
        let mut b = SimConfig::new()
            .sample_rate_hz(FS)
            .plant(ShockPlant::Jointed6x3.build().model)
            .noise_level(1e-3)
            .seed(seed)
            .shared_clock(true)
            .build();
        b.open().unwrap();
        b.configure(&table(6, 0, 3), &AcquisitionConfig::new(FS, 256))
            .unwrap();
        b.arm().unwrap();
        b.start().unwrap();
        let mut rng = Rng::new(11);
        let mut out = Vec::new();
        let mut buf = [0.0f32; 6];
        for _ in 0..4096 {
            let f: Vec<f32> = (0..3).map(|_| (rng.unit() - 0.5) as f32).collect();
            b.write_drive(&f).unwrap();
            b.read_input(&mut buf).unwrap();
            out.extend_from_slice(&buf);
        }
        out
    };
    let (a, b, c) = (run(5), run(5), run(6));
    assert_eq!(a, b, "the same seed");
    assert_ne!(a, c, "another seed");
}

/// A jointed plant is sized: its table must be its controls by its drives (Reference
/// rows aside, which read the first drive), and a bad law is refused at `configure`.
#[test]
fn mimo_shock_plant_sized_plant_row_and_drive_count_rule_holds() {
    let model = ShockPlant::Jointed6x3.build().model;
    assert_eq!(model.dims(), Some((6, 3)));
    let mut b = SimConfig::new().plant(model.clone()).build();
    assert_eq!(
        (b.caps().max_input_channels, b.caps().max_output_channels),
        (8, 3)
    );
    b.open().unwrap();
    for (n_c, n_d) in [(4, 3), (6, 2), (5, 3)] {
        let err = b
            .configure(&table(n_c, 0, n_d), &AcquisitionConfig::new(FS, 64))
            .unwrap_err();
        assert!(
            matches!(&err, HalError::Config(m) if m.contains("jointed plant is 6 measurement")),
            "{n_c} x {n_d}: {err:?}"
        );
    }
    b.configure(&table(6, 1, 3), &AcquisitionConfig::new(FS, 64))
        .expect("six controls, a reference row, three drives");
    b.arm().unwrap();
    b.start().unwrap();
    let mut buf = [0.0f32; 7];
    b.write_drive(&[0.25, 0.5, 0.75]).unwrap();
    b.read_input(&mut buf).unwrap();
    assert_eq!(buf[6], 0.25, "the reference row reads the first drive");

    let mut bad = ShockPlant::Jointed6x3.plant();
    bad.joint = Some(JointLaw::new(1.2, 1.0));
    let mut b = SimConfig::new().plant(PlantModel::Jointed(bad)).build();
    b.open().unwrap();
    let err = b
        .configure(&table(6, 0, 3), &AcquisitionConfig::new(FS, 64))
        .unwrap_err();
    assert!(
        matches!(&err, HalError::Config(m) if m.contains("frequency drop")),
        "{err:?}"
    );
}

/// Each output's delay, measured by cross-correlation of the output against the same
/// plant's output with no delays, is the stated delay within a tenth of a sample.
#[test]
fn mimo_shock_plant_output_delays_measured_by_cross_correlation() {
    for sp in [ShockPlant::Jointed4x4Severe, ShockPlant::Jointed6x3] {
        let plant = sp.plant().linear();
        let undelayed = JointedPlant {
            output_delays_s: Vec::new(),
            ..plant.clone()
        };
        // A random drive band-limited to the shock band (2 kHz) and played twice; the
        // second period is read, so the record is periodic.
        let n = 8192;
        let mut rng = Rng::new(3);
        let drives: Vec<Vec<f64>> = (0..plant.n_drives)
            .map(|_| {
                let x: Vec<(f64, f64)> = (0..n).map(|_| (rng.unit() - 0.5, 0.0)).collect();
                let mut spec = fft::fft(&x);
                let cut = (2000.0 / FS * n as f64) as usize;
                for (k, v) in spec.iter_mut().enumerate() {
                    if k.min(n - k) > cut {
                        *v = (0.0, 0.0);
                    }
                }
                let y: Vec<f64> = fft::ifft(&spec).into_iter().map(|c| c.0).collect();
                y.iter().chain(&y).copied().collect()
            })
            .collect();
        let a = undelayed.simulate(FS, &drives).unwrap();
        let b = plant.simulate(FS, &drives).unwrap();
        for i in 0..plant.n_inputs {
            let want = plant.output_delay_samples(i, FS);
            let got = xcorr_delay(&a.outputs[i][n..], &b.outputs[i][n..]);
            assert!(
                (got - want).abs() < 0.1,
                "{} output {i}: measured {got:.3} samples, stated {want:.3}",
                sp.name()
            );
        }
    }
}

/// The step stays bounded at any envelope: every standard plant driven on every drive
/// at the full drive range (10 V) by random, by repeated pulses, and by sines at its
/// first mode's shifted resonance and at twice it (where a modulated stiffness would
/// pump), and at a hundred thousand times that range (every mode far past its reference
/// and its damping at the cap), never leaves the linear bound of its small-signal modes.
#[test]
fn mimo_shock_plant_step_is_stable_at_any_amplitude() {
    let n = 2 * FS as usize;
    for sp in ShockPlant::ALL {
        let plant = sp.plant();
        let delta = plant.joint.map_or(0.0, |j| j.delta);
        let f1 = plant.modes[0].f_hz * (1.0 - delta);
        // The response bound per volt on each output: mass line, feedthrough, and each
        // mode's impulse-response area (at most 1 + 1/zeta for the acceleration shape).
        let bound: Vec<f64> = (0..plant.n_inputs)
            .map(|i| {
                (0..plant.n_drives)
                    .map(|j| {
                        plant.residual[i * plant.n_drives + j].abs()
                            + plant.feedthrough.abs()
                            + plant
                                .modes
                                .iter()
                                .map(|m| {
                                    (m.output_shape[i] * m.input_shape[j]).abs()
                                        * (1.0 + 1.0 / m.zeta)
                                })
                                .sum::<f64>()
                    })
                    .sum()
            })
            .collect();
        // Each drive signed by the first mode's participation, so every signal reaches it.
        let sign: Vec<f64> = plant.modes[0]
            .input_shape
            .iter()
            .map(|v| v.signum())
            .collect();
        let s1 = sign.clone();
        let s2 = sign.clone();
        let s3 = sign.clone();
        let mut rng = Rng::new(17);
        let signals: Vec<(&str, Signal)> = vec![
            ("random", {
                let r: Vec<f64> = (0..n * plant.n_drives)
                    .map(|_| 2.0 * rng.unit() - 1.0)
                    .collect();
                let nd = plant.n_drives;
                Box::new(move |k, j| r[k * nd + j])
            }),
            (
                "pulses",
                Box::new(move |k, j| {
                    let t = (k % 1280) as f64 / FS;
                    if t < 0.011 {
                        s1[j] * (std::f64::consts::PI * t / 0.011).sin()
                    } else {
                        0.0
                    }
                }),
            ),
            (
                "sine at the shifted resonance",
                Box::new(move |k, j| s2[j] * (TAU * f1 * k as f64 / FS).sin()),
            ),
            (
                "sine at twice it",
                Box::new(move |k, j| s3[j] * (TAU * 2.0 * f1 * k as f64 / FS).sin()),
            ),
        ];
        for volts in [10.0, 1e6] {
            for (what, sig) in &signals {
                let drives: Vec<Vec<f64>> = (0..plant.n_drives)
                    .map(|j| (0..n).map(|k| volts * sig(k, j)).collect())
                    .collect();
                let sim = plant.simulate(FS, &drives).unwrap();
                for (i, y) in sim.outputs.iter().enumerate() {
                    let peak = y.iter().fold(0.0f64, |a, v| a.max(v.abs()));
                    assert!(
                        y.iter().all(|v| v.is_finite()) && peak <= volts * bound[i],
                        "{} {what} at {volts} V: output {i} peaks at {peak}, bound {}",
                        sp.name(),
                        volts * bound[i]
                    );
                }
                if volts > 10.0 && plant.joint.is_some() {
                    let r = sim.peak_ratio(&plant);
                    assert!(
                        r[0] > 10.0,
                        "{} {what}: driven far past the reference",
                        sp.name()
                    );
                }
            }
        }
    }
}

/// The law: the drop stops at the reference amplitude, the damping keeps rising to its
/// cap, and nothing moves at zero.
#[test]
fn mimo_shock_plant_law_shape() {
    let j = JointLaw::new(0.05, 1.0);
    assert_eq!(j.frequency_factor(0.0), 1.0);
    assert_eq!(j.damping(0.02, 0.0), 0.02);
    assert!((j.frequency_factor(1.0) - 0.95).abs() < 1e-15);
    assert_eq!(j.frequency_factor(1.0), j.frequency_factor(50.0));
    assert!((j.damping(0.02, 1.0) - 0.04).abs() < 1e-15);
    assert!(j.damping(0.02, 4.0) > j.damping(0.02, 1.0));
    assert_eq!(j.damping(0.02, 1e12), MAX_DAMPING);
}

// ---------------------------------------------------------------------------------------
// The standard shock plants.
// ---------------------------------------------------------------------------------------

/// Names, sizes and the conditioning claim of the standard shock plants; the standard
/// modal set is left as it was.
#[test]
fn mimo_shock_plant_standard_set_by_name() {
    for sp in ShockPlant::ALL {
        assert_eq!(ShockPlant::from_name(sp.name()), Some(sp));
        let p = sp.build();
        assert_eq!(p.name, sp.name());
        assert_eq!(p.model.dims(), Some(sp.dims()));
        assert_eq!(p.geometry.inputs.len(), sp.dims().0);
        assert_eq!(p.geometry.drives.len(), sp.dims().1);
        assert!(StandardPlant::from_name(sp.name()).is_none());
        let jp = sp.plant();
        assert!(jp.modes.iter().all(|m| (60.0..=1800.0).contains(&m.f_hz)));
        let delays: Vec<f64> = (0..jp.n_inputs).map(|i| jp.output_delays_s[i]).collect();
        assert_eq!(delays.iter().cloned().fold(f64::MAX, f64::min), 0.0);
        assert!((delays.iter().cloned().fold(0.0, f64::max) - 0.5e-3).abs() < 1e-12);
        let Conditioning::Well { band_hz, max } = p.conditioning else {
            panic!("{} claims to be well-conditioned", sp.name());
        };
        let freqs: Vec<f64> = (0..400)
            .map(|k| band_hz.0 * (band_hz.1 / band_hz.0).powf(k as f64 / 399.0))
            .collect();
        let h = p.model.analytic_frf_at(&freqs, FS);
        let (r, c) = sp.dims();
        for (k, f) in freqs.iter().enumerate() {
            let cn = condition_number(h.line(k), r, c);
            assert!(cn < max, "{} at {f} Hz: condition {cn}", sp.name());
        }
        // Cross-coupled: every drive reaches every control.
        let mid = h.line(200);
        assert!(mid.iter().all(|v| mag(*v) > 1e-3), "{}", sp.name());
    }
    assert_eq!(ShockPlant::from_name("shock-jointed-4x4"), None);
    assert_eq!(StandardPlant::ALL.len(), 6, "the modal set is unchanged");
}

/// The calibration in the plants' docs: the reference pulse at full level, played at the
/// first control by the least drives through the LINEAR plant, brings the dominant mode
/// (the one nearest its reference) to its reference amplitude, and no mode past it.
/// Through the jointed plants, whose damping rises, it comes within a fifth of it.
#[test]
fn mimo_shock_plant_calibration_reaches_the_reference_on_the_dominant_mode() {
    for (sp, v_ref) in [
        (ShockPlant::Jointed4x4Moderate, SHOCK_4X4_REFERENCE_VELOCITY),
        (ShockPlant::Jointed6x3, SHOCK_6X3_REFERENCE_VELOCITY),
    ] {
        let plant = sp.plant();
        let lin = pulse_run(&plant.linear(), 0.0);
        let ratio = lin.peak_ratio(&plant);
        let top = ratio.iter().cloned().fold(0.0, f64::max);
        assert!(
            (top - 1.0).abs() < 2e-3,
            "{}: the dominant mode reaches {top} of its reference (V_ref {v_ref})",
            sp.name()
        );
        assert_eq!(ratio[0], top, "{}: the first mode dominates", sp.name());
        let peak = lin.outputs[0].iter().fold(0.0f64, |a, v| a.max(v.abs()));
        assert!(
            (peak - 20.0).abs() < 0.05,
            "{}: {peak} g at the control",
            sp.name()
        );
    }
    for sp in [
        ShockPlant::Jointed4x4Moderate,
        ShockPlant::Jointed4x4Severe,
        ShockPlant::Jointed6x3,
    ] {
        let plant = sp.plant();
        let r = pulse_run(&plant, 0.0).peak_ratio(&plant)[0];
        assert!((0.8..1.1).contains(&r), "{}: {r}", sp.name());
    }
}

/// At a low level the jointed plants are the linear plant: the reference pulse 80 dB
/// down gives the linear plant's response within half a percent (RMS, each control),
/// 60 dB down within 1.5 % (the damping's square-root law still moves it there), and
/// 20 dB down every mode sits within 1 % of its small-signal frequency.
#[test]
fn mimo_shock_plant_low_level_equals_the_linear_plant() {
    let linear = ShockPlant::Linear4x4.plant();
    for sp in [ShockPlant::Jointed4x4Moderate, ShockPlant::Jointed4x4Severe] {
        let plant = sp.plant();
        for (level_db, within) in [(-80.0, 0.005), (-60.0, 0.015)] {
            let lin = pulse_run(&linear, level_db);
            let low = pulse_run(&plant, level_db);
            for i in 0..plant.n_inputs {
                let diff: Vec<f64> = low.outputs[i]
                    .iter()
                    .zip(&lin.outputs[i])
                    .map(|(a, b)| a - b)
                    .collect();
                let e = rms(&diff) / rms(&lin.outputs[i]);
                assert!(
                    e < within,
                    "{} at {level_db} dB, output {i}: {:.3} % from linear",
                    sp.name(),
                    100.0 * e
                );
            }
        }
        let at = pulse_run(&plant, -20.0);
        for ((f, _), m) in plant
            .modes_at_envelope(&at.peak_envelope)
            .iter()
            .zip(&plant.modes)
        {
            assert!(
                (f / m.f_hz - 1.0).abs() < 0.01,
                "{} at -20 dB: {f} Hz against {} Hz",
                sp.name(),
                m.f_hz
            );
        }
    }
}

/// The shift and the damping change, measured from the plant's own response: each
/// jointed standard plant's dominant mode, alone, swept with a sine whose modal
/// amplitude is held. Held just past the reference amplitude (`FULL` times it; the
/// reference pulse at full level takes the dominant mode to between 0.85 and 1.06 of
/// it) its resonance has dropped by the stated fraction within a fifth of it and its
/// damping has risen by the stated factor within a fifth; held at a tenth of it (20 dB
/// down) the resonance is within 1 % of its small-signal value. The FRF at the
/// reference envelope peaks where the measurement does, within a fifth of the drop.
///
/// Held exactly at the reference amplitude, a severe mode just below its resonance sits
/// on the unstable side of its softening (less amplitude, higher frequency, less
/// amplitude), which an amplitude control as slow as this one does not hold.
#[test]
fn mimo_shock_plant_shift_and_damping_measured_at_full_level() {
    const FULL: f64 = 1.1;
    for sp in [
        ShockPlant::Jointed4x4Moderate,
        ShockPlant::Jointed4x4Severe,
        ShockPlant::Jointed6x3,
    ] {
        let plant = sp.plant();
        let law = plant.joint.expect("jointed");
        let one = plant.single_mode(0);
        let m = &one.modes[0];
        let (f0, a_ref) = (m.f_hz, m.reference_amplitude);
        let (lo, hi) = (
            f0 * (1.0 - 1.5 * law.delta - 3.0 * m.zeta),
            f0 * (1.0 + 3.0 * m.zeta),
        );
        let (fs_small, z_small) = held_amplitude_resonance(&one, 1e-6 * a_ref, lo, hi);
        let (f_full, z_full) = held_amplitude_resonance(&one, FULL * a_ref, lo, hi);
        let (f_low, _) = held_amplitude_resonance(&one, 0.1 * a_ref, lo, hi);
        let shift = 1.0 - f_full / fs_small;
        let damp = z_full / z_small;
        println!(
            "{}: small {fs_small:.2} Hz z {z_small:.4}; full {f_full:.2} Hz z {z_full:.4} \
             (shift {:.2} %, damping x{damp:.2}); -20 dB {f_low:.2} Hz",
            sp.name(),
            100.0 * shift
        );
        assert!(
            (shift - law.delta).abs() < 0.2 * law.delta,
            "{}: dropped {:.2} %, stated {:.2} %",
            sp.name(),
            100.0 * shift,
            100.0 * law.delta
        );
        let want = law.damping(m.zeta, FULL) / m.zeta;
        assert!(
            (damp - want).abs() < 0.2 * want,
            "{}: damping x{damp:.3}, stated x{want}",
            sp.name()
        );
        assert!(
            (f_low / fs_small - 1.0).abs() < 0.01,
            "{}: {f_low} Hz 20 dB down against {fs_small} Hz",
            sp.name()
        );
        // The frozen FRF at the reference envelope.
        let freqs: Vec<f64> = (0..4000)
            .map(|k| lo + (hi - lo) * k as f64 / 3999.0)
            .collect();
        let h = one.frf_at_reference_ratio(&freqs, FS, 1.0);
        let k = (0..freqs.len())
            .max_by(|&a, &b| mag(h.get(a, 0, 0)).total_cmp(&mag(h.get(b, 0, 0))))
            .unwrap();
        assert!(
            (freqs[k] - f_full).abs() < 0.2 * law.delta * f0,
            "{}: the FRF at the reference peaks at {} Hz, measured {f_full} Hz",
            sp.name(),
            freqs[k]
        );
    }
}

/// A jointed plant with no law is linear: twice the drive, twice the response, to the
/// last bit's rounding; with a law it is not.
#[test]
fn mimo_shock_plant_linear_without_a_law() {
    let lin = one_mode(150.0, 0.02, None);
    let jnt = one_mode(150.0, 0.02, Some(JointLaw::new(0.05, 1.0)));
    let mut rng = Rng::new(9);
    let d: Vec<f64> = (0..8192).map(|_| rng.unit() - 0.5).collect();
    let d2: Vec<f64> = d.iter().map(|v| 2.0 * v).collect();
    for (p, linear) in [(&lin, true), (&jnt, false)] {
        let a = p
            .simulate(FS, std::slice::from_ref(&d))
            .unwrap()
            .outputs
            .remove(0);
        let b = p
            .simulate(FS, std::slice::from_ref(&d2))
            .unwrap()
            .outputs
            .remove(0);
        let e = a
            .iter()
            .zip(&b)
            .map(|(x, y)| (2.0 * x - y).abs())
            .fold(0.0, f64::max)
            / b.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        assert_eq!(e < 1e-12, linear, "relative error {e}");
    }
}
