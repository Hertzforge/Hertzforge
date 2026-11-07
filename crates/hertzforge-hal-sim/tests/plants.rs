//! The plant models and the standard plant set: the exact FRF against the time-domain
//! plant, the conditioning claims, the rigid plate's six-degree-of-freedom
//! transformation, per-input noise, and what `configure` refuses.

mod common;

use common::{arg, cdiv, condition_number, inverse, mag, Rng};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal::{AcquisitionConfig, Backend, HalError};
use hertzforge_hal_sim::plants::{Conditioning, Placement, StandardPlant, RIGID_DOFS};
use hertzforge_hal_sim::{
    ModalPlant, Mode, ModeResponse, PlantModel, Resonator, SimBackend, SimConfig, C64,
};
use std::f64::consts::TAU;

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

/// Drive one drive with a sine on bin `k` of an `n`-point grid, wait `settle` samples,
/// then return each input's complex amplitude over `n` samples (textbook DFT at that
/// bin) divided by the drive's: the measured FRF column at `k * fs / n`.
fn sine_column(
    b: &mut SimBackend,
    n_in: usize,
    n_d: usize,
    j: usize,
    k: usize,
    n: usize,
    settle: usize,
) -> Vec<C64> {
    let w = TAU * k as f64 / n as f64;
    let mut frame = vec![0.0f32; n_d];
    let mut buf = vec![0.0f32; n_in];
    let mut acc = vec![C64::new(0.0, 0.0); n_in];
    let mut drv = C64::new(0.0, 0.0);
    for s in 0..settle + n {
        let x = (w * s as f64).cos();
        frame[j] = x as f32;
        b.write_drive(&frame).unwrap();
        b.read_input(&mut buf).unwrap();
        if s >= settle {
            let m = (s - settle) as f64;
            let e = C64::new((w * m).cos(), -(w * m).sin());
            drv += e * (frame[j] as f64);
            for (a, &y) in acc.iter_mut().zip(&buf) {
                *a += e * (y as f64);
            }
        }
    }
    // The drive started at phase zero at s = 0; the window starts at s = settle, so
    // both are referred to the same instant.
    acc.into_iter().map(|a| cdiv(a, drv)).collect()
}

/// The exact FRF is the FRF of the plant the backend steps: a sine on each drive, read
/// back on every input, matches `analytic_frf` to the f32 output rounding, transport
/// delay and the phase sign included.
#[test]
fn the_modal_plant_in_the_time_domain_matches_its_analytic_frf() {
    let fs = 4096.0;
    let n = 1024;
    let delay = 3;
    let modes = vec![
        Mode::new(180.0, 8.0, vec![1.0, 0.4, -0.3], vec![0.9, -0.5]),
        Mode {
            response: ModeResponse::Acceleration,
            ..Mode::new(520.0, 12.0, vec![-0.2, 1.0, 0.6], vec![0.3, 1.1])
        },
    ];
    let model = PlantModel::Modal(ModalPlant {
        transport_delay_samples: delay,
        residual: vec![0.2, 0.0, 0.05, 0.0, 0.3, 0.0],
        ..ModalPlant::new(2, 3, modes)
    });
    let mut b = running(model.clone(), fs, 0.0, &table(2, 0, 3));
    let truth = b.analytic_frf(n / 2 + 1, fs / n as f64);
    for (j, k) in [(0usize, 45usize), (1, 130), (2, 300)] {
        let col = sine_column(&mut b, 2, 3, j, k, n, 4 * n);
        for (i, h) in col.iter().enumerate() {
            let t = truth.get(k, i, j);
            let r = cdiv(*h, t);
            assert!(
                (mag(r) - 1.0).abs() < 1e-5 && arg(r).abs() < 1e-5,
                "drive {j} input {i} at bin {k}: measured {h} vs analytic {t}"
            );
        }
    }
}

/// The same for the resonator bank: its exact FRF (one path, `lines x 1 x 1`) is what a
/// sine reads back.
#[test]
fn the_resonator_bank_in_the_time_domain_matches_its_analytic_frf() {
    let fs = 4096.0;
    let n = 1024;
    let cfg = SimConfig::new()
        .sample_rate_hz(fs)
        .resonances(vec![(200.0, 6.0), (700.0, 15.0)])
        .feedthrough(0.05)
        .transport_delay_samples(5)
        .noise_level(0.0)
        .shared_clock(true);
    let model = cfg.plant.clone();
    let mut b = cfg.build();
    b.open().unwrap();
    b.configure(&table(1, 0, 1), &AcquisitionConfig::new(fs, 256))
        .unwrap();
    b.arm().unwrap();
    b.start().unwrap();
    let truth = model.analytic_frf(n / 2 + 1, fs / n as f64, fs);
    assert_eq!(truth.dims(), (n / 2 + 1, 1, 1));
    for k in [50usize, 175, 400] {
        let h = sine_column(&mut b, 1, 1, 0, k, n, 4 * n)[0];
        let r = cdiv(h, truth.get(k, 0, 0));
        assert!(
            (mag(r) - 1.0).abs() < 1e-5 && arg(r).abs() < 1e-5,
            "bin {k}: {h}"
        );
    }
}

/// A bandpass mode has a non-negative real part at every frequency (the property the
/// standard set's conditioning rests on); an acceleration mode does not.
#[test]
fn a_bandpass_mode_is_passive_and_an_acceleration_mode_is_not() {
    let fs = 8192.0;
    let bp = Resonator::with_response(300.0, 30.0, fs, ModeResponse::Bandpass);
    let acc = Resonator::with_response(300.0, 30.0, fs, ModeResponse::Acceleration);
    let mut min_bp = f64::INFINITY;
    let mut min_acc = f64::INFINITY;
    for k in 1..4096 {
        let f = k as f64;
        min_bp = min_bp.min(bp.response(f, fs).re);
        min_acc = min_acc.min(acc.response(f, fs).re);
    }
    assert!(min_bp > -1e-12, "bandpass real part reached {min_bp}");
    assert!(
        min_acc < -1.0,
        "acceleration real part never went negative ({min_acc})"
    );
    // Peaks of about Q at f_n; the acceleration mode sits at one far above.
    assert!((mag(bp.response(300.0, fs)) - 30.0).abs() < 1e-6);
    assert!((mag(acc.response(300.0, fs)) / 30.0 - 1.0).abs() < 0.01);
    assert!((mag(acc.response(4000.0, fs)) - 1.0).abs() < 0.01);
}

#[test]
fn every_standard_plant_name_round_trips_and_matches_its_model() {
    for p in StandardPlant::ALL {
        assert_eq!(StandardPlant::from_name(p.name()), Some(p));
        let built = p.build();
        assert_eq!(built.name, p.name());
        assert_eq!(built.model.dims(), Some(p.dims()));
        let (n_in, n_d) = p.dims();
        assert_eq!(built.geometry.inputs.len(), n_in);
        assert_eq!(built.geometry.drives.len(), n_d);
        built
            .model
            .validate(51_200.0)
            .expect("valid at the station rate");
        built.model.validate(8192.0).expect("valid at 8192 Hz");
    }
    assert_eq!(StandardPlant::from_name("modal-5x5"), None);
    // Same seed, same plant.
    assert_eq!(
        StandardPlant::Modal24x24.build(),
        StandardPlant::Modal24x24.build()
    );
}

/// The lines to check a plant's claim on: a uniform grid over the band plus, for every
/// mode in the band, its natural frequency and its two half-power points (where the
/// peaks are, which a coarse grid could step over).
fn claim_lines(model: &PlantModel, band: (f64, f64), step_hz: f64) -> Vec<f64> {
    let mut f: Vec<f64> = (0..)
        .map(|k| band.0 + k as f64 * step_hz)
        .take_while(|&f| f <= band.1)
        .collect();
    if let PlantModel::Modal(m) = model {
        for mode in &m.modes {
            for g in [1.0 - 0.5 / mode.q, 1.0, 1.0 + 0.5 / mode.q] {
                let x = mode.f_hz * g;
                if x >= band.0 && x <= band.1 {
                    f.push(x);
                }
            }
        }
    }
    f
}

/// The conditioning claims, checked on the exact FRF at 8192 Hz: each well-conditioned
/// plant stays under its bound on every checked line in its band, and the
/// ill-conditioned one exceeds 1,000 somewhere.
#[test]
fn the_conditioning_claims_hold_on_the_exact_frf() {
    let fs = 8192.0;
    for p in StandardPlant::ALL {
        let plant = p.build();
        let (n_in, n_d) = p.dims();
        let (band, step) = match plant.conditioning {
            Conditioning::Well { band_hz, .. } | Conditioning::Ill { band_hz, .. } => {
                (band_hz, if n_d > 6 { 4.0 } else { 1.0 })
            }
        };
        let freqs = claim_lines(&plant.model, band, step);
        let frf = plant.model.analytic_frf_at(&freqs, fs);
        let (mut worst, mut worst_f, mut best) = (0.0f64, 0.0f64, f64::INFINITY);
        for (k, &f) in freqs.iter().enumerate() {
            let c = condition_number(frf.line(k), n_in, n_d);
            if c > worst {
                worst = c;
                worst_f = f;
            }
            best = best.min(c);
        }
        println!(
            "{}: condition number {best:.1} to {worst:.1} (worst at {worst_f:.1} Hz) over {} lines, {:.0} to {:.0} Hz",
            p.name(),
            freqs.len(),
            band.0,
            band.1
        );
        match plant.conditioning {
            Conditioning::Well { max, .. } => assert!(
                worst < max,
                "{}: condition number {worst:.1} at {worst_f:.1} Hz breaks its claim of {max}",
                p.name()
            ),
            Conditioning::Ill { exceeds, .. } => assert!(
                worst > exceeds,
                "{}: condition number only reached {worst:.1}, not above {exceeds}",
                p.name()
            ),
        }
    }
}

/// The rigid plate's geometry alone gives a six-degree-of-freedom transformation that
/// controls it: with `T_s` the pseudo-inverse of the sensors' rigid-body kinematics and
/// `T_d` the inverse of the drives' generalized forces, `T_s H T_d` is diagonal in the
/// rigid-body band and sits on `1 / mass` and `1 / inertia` (the mass line).
#[test]
fn the_rigid_plate_is_controlled_by_a_transformation_built_from_its_geometry() {
    let plant = StandardPlant::RigidPlate12x6.build();
    let body = plant
        .rigid_body
        .expect("the rigid plate carries its rigid body");
    let g = &plant.geometry;
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let six = |p: &Placement| -> [f64; 6] {
        let r = [
            p.position_m[0] - g.reference_point_m[0],
            p.position_m[1] - g.reference_point_m[1],
            p.position_m[2] - g.reference_point_m[2],
        ];
        let m = cross(r, p.direction);
        [
            p.direction[0],
            p.direction[1],
            p.direction[2],
            m[0],
            m[1],
            m[2],
        ]
    };
    let real = |v: f64| C64::new(v, 0.0);
    // Sensor kinematics S (12 x 6): input i reads S[i] . (a, alpha).
    let s: Vec<C64> = g.inputs.iter().flat_map(|p| six(p).map(real)).collect();
    // T_s = (S^T S)^-1 S^T (6 x 12).
    let mut sts = vec![C64::new(0.0, 0.0); 36];
    for a in 0..6 {
        for c in 0..6 {
            sts[a * 6 + c] = (0..12).map(|i| s[i * 6 + a] * s[i * 6 + c]).sum();
        }
    }
    let sts_inv = inverse(&sts, 6).unwrap();
    let mut ts = vec![C64::new(0.0, 0.0); 6 * 12];
    for a in 0..6 {
        for i in 0..12 {
            ts[a * 12 + i] = (0..6).map(|c| sts_inv[a * 6 + c] * s[i * 6 + c]).sum();
        }
    }
    // Drive generalized forces B (6 x 6): drive j applies gain * six(drive j).
    let mut bmat = vec![C64::new(0.0, 0.0); 36];
    for (j, d) in g.drives.iter().enumerate() {
        for (a, v) in six(d).into_iter().enumerate() {
            bmat[a * 6 + j] = real(body.drive_gain_n * v);
        }
    }
    let td = inverse(&bmat, 6).expect("six independent drive axes");
    let want = [
        1.0 / body.mass_kg,
        1.0 / body.mass_kg,
        1.0 / body.mass_kg,
        1.0 / body.inertia_kg_m2[0],
        1.0 / body.inertia_kg_m2[1],
        1.0 / body.inertia_kg_m2[2],
    ];
    let fs = 8192.0;
    let band = match plant.conditioning {
        Conditioning::Well { band_hz, .. } | Conditioning::Ill { band_hz, .. } => band_hz,
    };
    let freqs: Vec<f64> = (band.0 as usize..=band.1 as usize)
        .map(|f| f as f64)
        .collect();
    let frf = plant.model.analytic_frf_at(&freqs, fs);
    let rigid: Vec<Resonator> = body
        .dof_hz
        .iter()
        .map(|&f| Resonator::with_response(f, body.q, fs, ModeResponse::Acceleration))
        .collect();
    let (mut worst_diag, mut worst_mass_line, mut worst_off) = (0.0f64, 0.0f64, 0.0f64);
    for (k, &f) in freqs.iter().enumerate() {
        let h = frf.line(k);
        let mut ht = vec![C64::new(0.0, 0.0); 12 * 6];
        for i in 0..12 {
            for c in 0..6 {
                ht[i * 6 + c] = (0..6).map(|j| h[i * 6 + j] * td[j * 6 + c]).sum();
            }
        }
        let mut d = [[C64::new(0.0, 0.0); 6]; 6];
        for (a, row) in d.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v = (0..12).map(|i| ts[a * 12 + i] * ht[i * 6 + c]).sum();
            }
        }
        for a in 0..6 {
            // The diagonal is the degree of freedom's own rigid-body response.
            let own = rigid[a].response(f, fs) * want[a];
            worst_diag = worst_diag.max(mag(d[a][a] - own) / mag(own));
            worst_mass_line = worst_mass_line.max((mag(d[a][a]) / want[a] - 1.0).abs());
            for c in 0..6 {
                if a != c {
                    // Scale-free: a cross term against the geometric mean of the two
                    // diagonals it couples (translation and rotation differ in units).
                    let rel = mag(d[a][c]) / (mag(d[a][a]) * mag(d[c][c])).sqrt();
                    worst_off = worst_off.max(rel);
                }
            }
        }
    }
    println!(
        "rigid plate, {:.0} to {:.0} Hz, DOFs {RIGID_DOFS:?}: diagonal within {:.2} % of each \
         degree of freedom's rigid-body response ({:.1} % of the mass line), largest cross term \
         {:.2} % of its diagonals",
        band.0,
        band.1,
        100.0 * worst_diag,
        100.0 * worst_mass_line,
        100.0 * worst_off
    );
    assert!(
        worst_diag < 0.10,
        "diagonal off the rigid-body response by {worst_diag}"
    );
    assert!(
        worst_off < 0.10,
        "cross terms reach {worst_off} of the diagonal"
    );
}

/// Each input of a modal plant has its own noise: with no drive, the inputs' noise is
/// uncorrelated and at the configured level; the same seed gives the same stream.
#[test]
fn a_modal_plant_gives_each_input_independent_seeded_noise() {
    let fs = 8192.0;
    let sigma = 0.01;
    let model = StandardPlant::Modal4x4.build().model;
    let collect = || {
        let mut b = running(model.clone(), fs, sigma, &table(4, 0, 4));
        let mut buf = vec![0.0f32; 4 * 1024];
        let mut out = Vec::new();
        for _ in 0..20 {
            b.write_drive(&vec![0.0f32; 4 * 1024]).unwrap();
            b.read_input(&mut buf).unwrap();
            out.extend_from_slice(&buf);
        }
        out
    };
    let a = collect();
    assert_eq!(a, collect(), "same seed, same noise");
    let n = a.len() / 4;
    let ch = |i: usize| -> Vec<f64> { (0..n).map(|f| a[f * 4 + i] as f64).collect() };
    let chans: Vec<Vec<f64>> = (0..4).map(ch).collect();
    let rms = |v: &[f64]| (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt();
    for (i, c) in chans.iter().enumerate() {
        let r = rms(c);
        assert!((r / sigma - 1.0).abs() < 0.05, "input {i} noise rms {r}");
        for (l, d) in chans.iter().enumerate().skip(i + 1) {
            let corr = c.iter().zip(d).map(|(x, y)| x * y).sum::<f64>() / (n as f64 * r * rms(d));
            assert!(
                corr.abs() < 0.03,
                "inputs {i} and {l} noise correlated: {corr}"
            );
        }
    }
}

/// A sized plant refuses a table of another size, by name and size; a Reference row
/// does not count as a measurement input and reads the commanded first drive.
#[test]
fn configure_sizes_the_plant_to_the_table_and_refuses_a_mismatch() {
    let fs = 8192.0;
    let model = StandardPlant::Modal4x4.build().model;
    let mut b = SimConfig::new()
        .sample_rate_hz(fs)
        .plant(model.clone())
        .build();
    b.open().unwrap();
    let err = b
        .configure(&table(3, 0, 4), &AcquisitionConfig::new(fs, 64))
        .unwrap_err();
    match err {
        HalError::Config(m) => assert!(
            m.contains("modal plant is 4 measurement inputs by 4 drives")
                && m.contains("3 measurement inputs"),
            "{m}"
        ),
        other => panic!("expected a configuration refusal, got {other:?}"),
    }
    // Four controls plus a reference: the reference is not a plant output.
    b.configure(&table(4, 1, 4), &AcquisitionConfig::new(fs, 64))
        .expect("4 measurement inputs and 4 drives fit");
    assert_eq!(b.n_input_channels(), 5);
    b.arm().unwrap();
    b.start().unwrap();
    b.write_drive(&[0.25, 0.5, 0.75, 1.0]).unwrap();
    let mut buf = vec![0.0f32; 5];
    b.read_input(&mut buf).unwrap();
    assert_eq!(
        buf[4], 0.25,
        "the reference row reads the commanded first drive"
    );

    // A mode at or above the Nyquist frequency of the configured rate is refused.
    let mut b = SimConfig::new().plant(model).build();
    b.open().unwrap();
    let err = b
        .configure(&table(4, 0, 4), &AcquisitionConfig::new(2048.0, 64))
        .unwrap_err();
    assert!(
        matches!(&err, HalError::Config(m) if m.contains("Nyquist")),
        "{err:?}"
    );
}

/// `.plant` raises the advertised channel maxima to the plant's size, never lowers them.
#[test]
fn choosing_a_plant_raises_the_advertised_channel_maxima() {
    let b = SimConfig::new()
        .plant(StandardPlant::Modal72x24.build().model)
        .build();
    let caps = b.caps();
    assert_eq!(
        (caps.max_input_channels, caps.max_output_channels),
        (72, 24)
    );
    let b = SimConfig::new()
        .max_input_channels(400)
        .plant(StandardPlant::Modal4x4.build().model)
        .build();
    assert_eq!(b.caps().max_input_channels, 400);
}

/// Every drive reaches every input: driving one drive of a modal plant moves every
/// input, and two drives superpose (the plant is linear across drives).
#[test]
fn every_drive_reaches_every_input_and_drives_superpose() {
    let fs = 8192.0;
    let model = StandardPlant::Modal12x6.build().model;
    let run = |frame: [f32; 6]| -> Vec<f32> {
        let mut b = running(model.clone(), fs, 0.0, &table(12, 0, 6));
        let mut rng = Rng::new(7);
        let mut out = Vec::new();
        let mut buf = vec![0.0f32; 12];
        for _ in 0..2048 {
            let s = (rng.unit() - 0.5) as f32;
            let f: Vec<f32> = frame.iter().map(|g| g * s).collect();
            b.write_drive(&f).unwrap();
            b.read_input(&mut buf).unwrap();
            out.extend_from_slice(&buf);
        }
        out
    };
    let a = run([1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let c = run([0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    let both = run([1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    for i in 0..12 {
        let energy = |v: &[f32]| {
            (0..2048)
                .map(|f| (v[f * 12 + i] as f64).powi(2))
                .sum::<f64>()
        };
        assert!(
            energy(&a) > 1e-6 && energy(&c) > 1e-6,
            "input {i} does not see both drives"
        );
    }
    let err = a
        .iter()
        .zip(&c)
        .zip(&both)
        .map(|((x, y), z)| ((x + y) - z).abs() as f64)
        .fold(0.0, f64::max);
    let scale = both.iter().map(|v| v.abs() as f64).fold(0.0, f64::max);
    assert!(err < 1e-5 * scale, "superposition error {err} of {scale}");
}
