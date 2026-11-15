//! The level rule per mode: a drop term and a rise term on each mode of the jointed
//! plant, and the proof that a plant using only the structure-wide joint law plays
//! exactly what it played before the rule existed.

mod common;

use common::mag;
use hertzforge_hal::{AcquisitionConfig, Backend};
use hertzforge_hal_sim::jointed::MAX_DAMPING;
use hertzforge_hal_sim::plants::ShockPlant;
use hertzforge_hal_sim::{JointLaw, JointedMode, JointedPlant, LevelRule, PlantModel, SimConfig};
use std::f64::consts::TAU;

const FS: f64 = 12_800.0;

// ---------------------------------------------------------------------------------------
// The golden record.
// ---------------------------------------------------------------------------------------

/// FNV-1a over a stream of f64 bit patterns.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn f64(&mut self, v: f64) {
        for byte in v.to_bits().to_le_bytes() {
            self.0 ^= byte as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

fn xorshift(state: &mut u64) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 11) as f64 / (1u64 << 53) as f64 - 0.5
}

/// A seeded random drive, `n_d` records of `n` samples, at `scale` volts peak to peak.
fn random_drive(n_d: usize, n: usize, scale: f64, seed: u64) -> Vec<Vec<f64>> {
    let mut s = seed;
    (0..n_d)
        .map(|_| (0..n).map(|_| scale * xorshift(&mut s)).collect())
        .collect()
}

/// A sine at `f_hz` on every drive, `amp` volts, with a two-sided taper.
fn sine_drive(n_d: usize, n: usize, f_hz: f64, amp: f64) -> Vec<Vec<f64>> {
    (0..n_d)
        .map(|j| {
            (0..n)
                .map(|k| {
                    let t = k as f64 / FS;
                    let taper = (k as f64 / 512.0).min(1.0) * ((n - k) as f64 / 512.0).min(1.0);
                    amp * taper * (TAU * f_hz * t + 0.7 * j as f64).sin()
                })
                .collect()
        })
        .collect()
}

/// A plant with a hand-made law whose exponents differ from the standard plants'.
fn hand_plant() -> JointedPlant {
    let mode = |f_hz: f64, zeta: f64, a: [f64; 2], b: [f64; 3], r: f64| JointedMode {
        f_hz,
        zeta,
        input_shape: a.to_vec(),
        output_shape: b.to_vec(),
        reference_amplitude: r,
        level: None,
    };
    JointedPlant {
        joint: Some(JointLaw {
            delta: 0.07,
            damping_gain: 1.5,
            freq_exponent: 0.5,
            damping_exponent: 1.3,
            decay_cycles: 5.0,
        }),
        feedthrough: 0.01,
        residual: vec![0.1, 0.0, 0.0, 0.2, 0.05, 0.0],
        transport_delay_samples: 3,
        output_delays_s: vec![0.0, 1.3e-4, 2.7e-4],
        ..JointedPlant::new(
            3,
            2,
            vec![
                mode(150.0, 0.02, [1.0, 0.3], [1.0, 0.5, -0.2], 2e-6),
                mode(420.0, 0.015, [-0.4, 0.9], [0.3, 1.0, 0.6], 5e-7),
                mode(1300.0, 0.01, [0.2, 0.2], [-0.5, 0.4, 1.0], 1e-7),
            ],
        )
    }
}

/// The fingerprint of every sample and peak envelope `plant` plays for a set of drives
/// from small to far past its reference amplitudes, and of its FRF frozen at three
/// envelopes.
fn plant_fingerprint(plant: &JointedPlant) -> u64 {
    let mut h = Fnv::new();
    let n = 8192;
    let nd = plant.n_drives;
    let f1 = plant.modes[0].f_hz;
    let mut runs = Vec::new();
    for (k, scale) in [0.01, 1.0, 30.0, 1000.0].into_iter().enumerate() {
        runs.push(random_drive(nd, n, scale, 0x5EED + k as u64));
    }
    for amp in [0.05, 5.0, 500.0] {
        runs.push(sine_drive(nd, n, 0.97 * f1, amp));
    }
    for d in &runs {
        let s = plant.simulate(FS, d).expect("runs");
        for o in &s.outputs {
            o.iter().for_each(|&v| h.f64(v));
        }
        s.peak_envelope.iter().for_each(|&v| h.f64(v));
    }
    let freqs: Vec<f64> = (1..400).map(|k| 5.0 * k as f64).collect();
    for ratio in [0.0, 0.5, 3.0] {
        let frf = plant.frf_at_reference_ratio(&freqs, FS, ratio);
        for k in 0..freqs.len() {
            for i in 0..plant.n_inputs {
                for j in 0..plant.n_drives {
                    let c = frf.get(k, i, j);
                    h.f64(c.re);
                    h.f64(c.im);
                }
            }
        }
    }
    h.0
}

/// The fingerprint of what the backend reads back, noise included.
fn backend_fingerprint(plant: JointedPlant, scale: f32) -> u64 {
    use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
    let (n_in, n_d) = (plant.n_inputs, plant.n_drives);
    let mut t = ChannelTable::new();
    let roles = std::iter::repeat_n(ChannelRole::Control, n_in)
        .chain(std::iter::repeat_n(ChannelRole::Drive, n_d));
    for (k, role) in roles.enumerate() {
        let mut c = Channel::new(k as u32 + 1);
        c.role = role;
        c.range_max = 1e9;
        t.channels.push(c);
    }
    let mut b = SimConfig::new()
        .sample_rate_hz(FS)
        .plant(PlantModel::Jointed(plant))
        .noise_level(1e-3)
        .shared_clock(true)
        .build();
    b.open().unwrap();
    b.configure(&t, &AcquisitionConfig::new(FS, 256)).unwrap();
    b.arm().unwrap();
    b.start().unwrap();
    let mut s: u64 = 0x5EED_0F51_10C4;
    let mut h = Fnv::new();
    let mut frame = vec![0.0f32; n_d];
    let mut buf = vec![0.0f32; n_in];
    for _ in 0..4096 {
        for d in frame.iter_mut() {
            *d = scale * xorshift(&mut s) as f32;
        }
        b.write_drive(&frame).unwrap();
        b.read_input(&mut buf).unwrap();
        buf.iter().for_each(|v| h.f64(*v as f64));
    }
    h.0
}

fn golden_set() -> Vec<(String, u64)> {
    let mut got = Vec::new();
    for sp in ShockPlant::ALL {
        got.push((sp.name().to_string(), plant_fingerprint(&sp.plant())));
        got.push((
            format!("{}-backend", sp.name()),
            backend_fingerprint(sp.plant(), 8.0),
        ));
    }
    let hand = hand_plant();
    got.push(("hand".into(), plant_fingerprint(&hand)));
    got.push((
        "hand-single-1".into(),
        plant_fingerprint(&hand.single_mode(1)),
    ));
    got.push(("hand-linear".into(), plant_fingerprint(&hand.linear())));
    got.push(("hand-backend".into(), backend_fingerprint(hand, 8.0)));
    got
}

/// A plant that uses only the structure-wide joint law plays bit for bit what it played
/// before modes could carry their own rule: the values below were recorded at the
/// commit the rule started from (main `acfcb970`), on macOS aarch64, before any change.
#[test]
fn level_rule_old_fields_play_what_they_played() {
    let got = golden_set();
    for (name, h) in &got {
        println!("(\"{name}\", 0x{h:016x}),");
    }
    assert_eq!(GOLDEN.len(), got.len(), "every case has a recorded value");
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        for (name, h) in GOLDEN {
            let g = got
                .iter()
                .find(|(n, _)| n == name)
                .expect("a recorded case");
            assert_eq!(g.1, *h, "{name} no longer plays what it played");
        }
    } else {
        assert_eq!(golden_set(), got, "the same cases twice");
    }
}

/// Recorded on macOS (aarch64) at main `acfcb970`, before the level rule.
const GOLDEN: &[(&str, u64)] = &[
    ("shock-linear-4x4", 0xb942117dc42dbe4b),
    ("shock-linear-4x4-backend", 0x6ef6780a39177df1),
    ("shock-jointed-4x4-moderate", 0xce7214609ab74e0c),
    ("shock-jointed-4x4-moderate-backend", 0xb2c5a4bfcbf4c78a),
    ("shock-jointed-4x4-severe", 0x193edc77738ad028),
    ("shock-jointed-4x4-severe-backend", 0xfc832c148a635771),
    ("shock-jointed-6x3", 0xa776bfe9426b09f9),
    ("shock-jointed-6x3-backend", 0xf9983aa0f8b0e761),
    ("hand", 0x06871470d2127dec),
    ("hand-single-1", 0x5edf8e6a02c61b81),
    ("hand-linear", 0x2518cf32140d6ad5),
    ("hand-backend", 0xcd09577c497996b4),
];

/// The golden drives exercise the law: the largest drives take every mode of every
/// jointed plant past its reference amplitude, so a change to the law's arithmetic
/// would show in the record.
#[test]
fn level_rule_golden_drives_reach_past_the_reference() {
    let mut plants: Vec<JointedPlant> = ShockPlant::ALL[1..].iter().map(|p| p.plant()).collect();
    plants.push(hand_plant());
    for p in &plants {
        let d = random_drive(p.n_drives, 8192, 1000.0, 0x5EED + 3);
        let r = p.simulate(FS, &d).unwrap().peak_ratio(p);
        assert!(r.iter().all(|&v| v > 1.0), "peak ratios {r:?}");
    }
}

// ---------------------------------------------------------------------------------------
// The rule's shape and its fields.
// ---------------------------------------------------------------------------------------

/// The rule as written in the spec: `1 - drop + rise` at the reference, the drop flat
/// above it, the rise flat above its cap, the damping as the joint law's.
#[test]
fn level_rule_shape() {
    let l = LevelRule {
        drop: 0.02,
        drop_exponent: 1.0,
        rise: 0.05,
        rise_exponent: 2.0,
        rise_cap: 2.0,
        damping_gain: 0.5,
        damping_exponent: 1.0,
    };
    assert_eq!(l.frequency_factor(0.0), 1.0);
    for r in [0.0, 0.3, 1.0, 1.7, 2.0, 50.0] {
        assert_eq!(l.frequency_factor(r), spec_factor(&l, r));
    }
    assert!((l.frequency_factor(1.0) - 1.03).abs() < 1e-15);
    assert!((l.frequency_factor(0.5) - (1.0 - 0.01 + 0.0125)).abs() < 1e-15);
    assert!((l.frequency_factor(2.0) - (1.0 - 0.02 + 0.2)).abs() < 1e-15);
    assert_eq!(l.frequency_factor(2.0), l.frequency_factor(1e9));
    assert!((l.max_frequency_factor() - 1.2).abs() < 1e-15);
    assert_eq!(l.damping(0.01, 0.0), 0.01);
    assert!((l.damping(0.01, 1.0) - 0.015).abs() < 1e-15);
    assert_eq!(l.damping(0.01, 1e12), MAX_DAMPING);
    // No damping gain: no damping change, however far past the reference.
    assert_eq!(LevelRule::hardening(0.05, 2.0).damping(0.01, 1e300), 0.01);
    // The rule of a joint law gives the law's numbers.
    let j = JointLaw::new(0.05, 1.0);
    let lj = LevelRule::from_joint(&j);
    for r in [0.0, 0.1, 0.7, 1.0, 3.0] {
        assert_eq!(lj.frequency_factor(r), j.frequency_factor(r));
        assert_eq!(lj.damping(0.02, r), j.damping(0.02, r));
    }
}

/// Bad fields are refused, each with its name in the message; a plant whose mode could
/// rise to the Nyquist frequency is refused with the bound stated, and one just under
/// it runs.
#[test]
fn level_rule_bad_fields_and_the_frequency_bound_are_refused() {
    let base = LevelRule {
        drop: 0.01,
        drop_exponent: 1.0,
        rise: 0.05,
        rise_exponent: 2.0,
        rise_cap: 2.0,
        damping_gain: 0.5,
        damping_exponent: 1.0,
    };
    assert!(base.validate().is_ok());
    let bad: Vec<(&str, LevelRule)> = vec![
        ("drop", LevelRule { drop: 1.0, ..base }),
        ("drop", LevelRule { drop: -0.1, ..base }),
        (
            "drop",
            LevelRule {
                drop: f64::NAN,
                ..base
            },
        ),
        (
            "drop_exponent",
            LevelRule {
                drop_exponent: 0.0,
                ..base
            },
        ),
        (
            "drop_exponent",
            LevelRule {
                drop_exponent: f64::INFINITY,
                ..base
            },
        ),
        (
            "rise",
            LevelRule {
                rise: -1e-9,
                ..base
            },
        ),
        (
            "rise",
            LevelRule {
                rise: f64::INFINITY,
                ..base
            },
        ),
        (
            "rise_exponent",
            LevelRule {
                rise_exponent: -2.0,
                ..base
            },
        ),
        (
            "rise_cap",
            LevelRule {
                rise_cap: 0.0,
                ..base
            },
        ),
        (
            "rise_cap",
            LevelRule {
                rise_cap: f64::INFINITY,
                ..base
            },
        ),
        (
            "rise_cap",
            LevelRule {
                rise_cap: f64::NAN,
                ..base
            },
        ),
        (
            "damping_gain",
            LevelRule {
                damping_gain: -0.5,
                ..base
            },
        ),
        (
            "damping_exponent",
            LevelRule {
                damping_exponent: 0.0,
                ..base
            },
        ),
        (
            "highest frequency",
            LevelRule {
                rise: 1e300,
                rise_cap: 1e10,
                rise_exponent: 3.0,
                ..base
            },
        ),
    ];
    for (field, l) in bad {
        let e = l.validate().expect_err(field);
        assert!(e.contains(field), "{field}: {e}");
        let p = one(1000.0, 0.01, Some(l));
        let e = p.validate(FS).expect_err(field);
        assert!(
            e.contains("mode 0's level rule") && e.contains(field),
            "{e}"
        );
    }
    // The bound: 5000 Hz with a rise of 0.28 at a cap of 1 reaches 6400 Hz, the Nyquist
    // frequency at 12 800 Hz.
    let reach = |rise| LevelRule {
        rise,
        rise_cap: 1.0,
        ..LevelRule::linear()
    };
    let p = one(5000.0, 0.01, Some(reach(0.28)));
    assert_eq!(p.highest_frequencies_hz(), vec![6400.0]);
    assert_eq!(JointedPlant::frequency_bound_hz(FS), 6400.0);
    let e = p.validate(FS).expect_err("at the bound");
    assert!(e.contains("6400 Hz") && e.contains("Nyquist"), "{e}");
    assert!(PlantModel::Jointed(p).validate(FS).is_err());
    let p = one(5000.0, 0.01, Some(reach(0.279)));
    assert!(p.validate(FS).is_ok());
    let d = sine_drive(1, 4096, 5000.0, 1e3);
    assert!(p.simulate(FS, &d).unwrap().outputs[0]
        .iter()
        .all(|v| v.is_finite()));
}

// ---------------------------------------------------------------------------------------
// Measured from the plant's own response.
// ---------------------------------------------------------------------------------------

/// The frequency factor as `Level_Rule_Spec.md` writes it, apart from the code under
/// test, so a measurement is checked against the spec and not against the code itself.
fn spec_factor(l: &LevelRule, r: f64) -> f64 {
    if r <= 0.0 {
        return 1.0;
    }
    1.0 - l.drop * r.min(1.0).powf(l.drop_exponent)
        + l.rise * r.min(l.rise_cap).powf(l.rise_exponent)
}

fn one(f_hz: f64, zeta: f64, level: Option<LevelRule>) -> JointedPlant {
    JointedPlant::new(
        1,
        1,
        vec![JointedMode {
            f_hz,
            zeta,
            input_shape: vec![1.0],
            output_shape: vec![1.0],
            reference_amplitude: 1e-4,
            level,
        }],
    )
}

/// A resonance dwell by relay feedback: a force of `force` volts, its sign switched to
/// follow the mode's velocity (read from the output with the force taken out, which is
/// minus the mode's stiffness and damping force), so the mode oscillates at its own
/// resonance at the amplitude where the force feeds what the damping takes. Returns the
/// measured `(frequency Hz, amplitude ratio)` over the last second of three.
fn relay_dwell(p: &JointedPlant, force: f64) -> (f64, f64) {
    let mut st = p.stepper(FS).expect("a plant");
    let n = 3 * FS as usize;
    let keep = FS as usize;
    let mut out = [0.0];
    let (mut z_prev, mut u) = (0.0, force);
    let mut z = Vec::with_capacity(keep);
    for k in 0..n {
        st.step(&[u], &mut out);
        // The output is the mode's acceleration: the force less the restoring force.
        let zk = out[0] - u;
        u = if zk - z_prev < 0.0 { force } else { -force };
        z_prev = zk;
        if k >= n - keep {
            z.push(zk);
        }
    }
    let ups: Vec<f64> = (1..z.len())
        .filter(|&k| z[k - 1] < 0.0 && z[k] >= 0.0)
        .map(|k| (k - 1) as f64 + z[k - 1] / (z[k - 1] - z[k]))
        .collect();
    let f = FS * (ups.len() - 1) as f64 / (ups[ups.len() - 1] - ups[0]);
    let peak = z.iter().fold(0.0f64, |a, v| a.max(v.abs()));
    (f, peak / (TAU * f).powi(2) / p.modes[0].reference_amplitude)
}

/// A hardening mode driven at its resonance at rising level (a relay-feedback dwell at
/// six force levels, the last past the cap): its measured resonance over its small-signal resonance rises as
/// the rule says at the measured amplitude, within a tenth of the rule's rise (or 0.05 %
/// of the frequency where the rise is smaller than that), and stops rising past the
/// cap. A softening mode under its own rule, dwelt the same way, falls as its rule says.
#[test]
fn level_rule_rise_measured_at_rising_level() {
    let harden = LevelRule::hardening(0.04, 2.0);
    let soften = LevelRule {
        drop: 0.04,
        drop_exponent: 1.0,
        ..LevelRule::linear()
    };
    for (rule, sign) in [(harden, 1.0), (soften, -1.0)] {
        let p = one(200.0, 0.002, Some(rule));
        let (f_small, r_small) = relay_dwell(&p, 1e-4);
        assert!(r_small < 1e-3, "small signal at r = {r_small}");
        assert!(
            (f_small / 200.0 - 1.0).abs() < 2e-3,
            "small signal {f_small} Hz"
        );
        let mut last = 1.0;
        for force in [0.15, 0.35, 0.5, 0.75, 1.25, 2.0] {
            let (f, r) = relay_dwell(&p, force);
            let got = f / f_small;
            let want = spec_factor(&rule, r);
            let shift = (want - 1.0).abs();
            println!(
                "{}: force {force} V, r = {r:.3}: {f:.3} Hz, factor {got:.5}, rule {want:.5}",
                if sign > 0.0 { "rise" } else { "drop" }
            );
            assert!(
                (got - want).abs() < (0.1 * shift).max(5e-4),
                "at r = {r:.3} the factor is {got}, the rule says {want}"
            );
            assert!(
                (got - last) * sign > -5e-4,
                "moved the wrong way at r = {r:.3}"
            );
            last = got;
        }
    }
}

/// The backbone in free decay: a mode struck past its reference glides along
/// its rule as it decays, each cycle's frequency within 0.3 % of the frequency the rule
/// gives at that cycle's amplitude. Each mode of a structure with one hardening mode
/// and one softening mode follows its own rule, in its own direction.
#[test]
fn level_rule_each_mode_follows_its_own_rule_in_free_decay() {
    let harden = LevelRule::hardening(0.06, 2.0);
    let soften = LevelRule {
        drop: 0.04,
        drop_exponent: 1.0,
        ..LevelRule::linear()
    };
    let a_ref = 1e-4;
    let mode = |f_hz: f64, out: [f64; 2], level| JointedMode {
        f_hz,
        zeta: 0.004,
        input_shape: vec![f_hz / 300.0],
        output_shape: out.to_vec(),
        reference_amplitude: a_ref,
        level: Some(level),
    };
    let plant = JointedPlant {
        joint: Some(JointLaw {
            decay_cycles: 3.0,
            ..JointLaw::new(0.0, 0.0)
        }),
        ..JointedPlant::new(
            2,
            1,
            vec![
                mode(300.0, [1.0, 0.0], harden),
                mode(700.0, [0.0, 1.0], soften),
            ],
        )
    };
    // Strike both modes with one sample's pulse: each starts its free decay at about
    // twice its reference amplitude.
    let n = 3 * FS as usize;
    let mut drive = vec![0.0; n];
    drive[1] = 2.0 * a_ref * TAU * 300.0 * FS;
    let sim = plant.simulate(FS, &[drive]).unwrap();
    let peak = sim.peak_ratio(&plant);
    println!("peak ratios {peak:?}");
    assert!(
        peak.iter().all(|&r| r > 1.5),
        "struck past the reference: {peak:?}"
    );
    for (m, (rule, sign)) in [(harden, 1.0), (soften, -1.0)].into_iter().enumerate() {
        let f0 = plant.modes[m].f_hz;
        let y = &sim.outputs[m][64..];
        // Upward zero crossings, interpolated.
        let ups: Vec<f64> = (1..y.len())
            .filter(|&k| y[k - 1] < 0.0 && y[k] >= 0.0)
            .map(|k| (k - 1) as f64 + y[k - 1] / (y[k - 1] - y[k]))
            .collect();
        let (mut checked, mut worst, mut first_shift) = (0, 0.0f64, None);
        for c in ups.windows(2) {
            let f = FS / (c[1] - c[0]);
            let (a, b) = (c[0].ceil() as usize, c[1].floor() as usize);
            let peak_a = y[a..=b].iter().fold(0.0f64, |s, v| s.max(v.abs()));
            let r = peak_a / (TAU * f).powi(2) / a_ref;
            if r < 0.05 {
                break;
            }
            let want = f0 * spec_factor(&rule, r);
            let err = (f - want) / want;
            worst = worst.max(err.abs());
            first_shift.get_or_insert(f / f0 - 1.0);
            checked += 1;
        }
        let first = first_shift.expect("cycles");
        println!(
            "mode {m}: {checked} cycles checked, worst {:.3} %, first cycle shift {:+.2} %",
            100.0 * worst,
            100.0 * first
        );
        assert!(checked > 20, "mode {m}: {checked} cycles");
        assert!(
            worst < 3e-3,
            "mode {m}: worst cycle {:.3} % off its rule",
            100.0 * worst
        );
        assert!(first * sign > 0.02, "mode {m} moved {first} the wrong way");
    }
}

/// A softening mode under its own rule plays exactly what it played under the plant's
/// joint law: the same samples to the bit, through the backend, noise included, and
/// in a structure whose other modes keep the joint law.
#[test]
fn level_rule_drop_alone_is_the_joint_law() {
    for sp in [ShockPlant::Jointed4x4Moderate, ShockPlant::Jointed6x3] {
        let joint = sp.plant();
        let law = joint.joint.unwrap();
        let mut own = joint.clone();
        own.modes
            .iter_mut()
            .step_by(2)
            .for_each(|m| m.level = Some(LevelRule::from_joint(&law)));
        assert_eq!(
            plant_fingerprint(&own),
            plant_fingerprint(&joint),
            "{}",
            sp.name()
        );
        assert_eq!(
            backend_fingerprint(own, 8.0),
            backend_fingerprint(joint, 8.0),
            "{}",
            sp.name()
        );
    }
}

/// The cap holds: a hardening mode driven far past its cap never goes above
/// `f0 (1 + rise rise_cap^rise_exponent)`, its FRF frozen at any envelope peaks no
/// higher, and the step stays bounded and finite at a million volts.
#[test]
fn level_rule_cap_holds() {
    let rule = LevelRule::hardening(0.05, 2.0);
    let p = one(500.0, 0.01, Some(rule));
    let top = p.highest_frequencies_hz()[0];
    assert!((top - 600.0).abs() < 1e-9);
    for r in [2.0, 10.0, 1e6] {
        let (f, _) = p.modes_at_envelope(&[r * 1e-4])[0];
        assert!((f - top).abs() < 1e-9, "at r = {r}: {f} Hz");
    }
    let freqs: Vec<f64> = (0..3000).map(|k| 450.0 + 0.1 * k as f64).collect();
    let h = p.frf_at_reference_ratio(&freqs, FS, 1e4);
    let k = (0..freqs.len())
        .max_by(|&a, &b| mag(h.get(a, 0, 0)).total_cmp(&mag(h.get(b, 0, 0))))
        .unwrap();
    assert!(
        (freqs[k] - top).abs() < 1.0,
        "frozen FRF peaks at {} Hz",
        freqs[k]
    );
    // Driven far past the cap at its capped resonance and at twice it.
    let n = 2 * FS as usize;
    // The linear bound per volt: 1 + 1/zeta, times the energy the rise can add.
    let bound = (1.0 + 1.0 / 0.01) * rule.max_frequency_factor().powi(2);
    for volts in [10.0, 1e6] {
        for f in [top, 2.0 * top, 500.0] {
            let d = sine_drive(1, n, f, volts);
            let s = p.simulate(FS, &d).unwrap();
            let pk = s.outputs[0].iter().fold(0.0f64, |a, v| a.max(v.abs()));
            assert!(s.outputs[0].iter().all(|v| v.is_finite()));
            assert!(pk <= volts * bound, "{f} Hz at {volts} V: peak {pk}");
            if volts > 10.0 {
                assert!(s.peak_ratio(&p)[0] > 2.0, "{f} Hz: past the cap");
            }
        }
    }
    // A stepper's frequency in steady drive past the cap: the frozen step's own.
    let mut st = p.stepper(FS).unwrap();
    let mut out = [0.0];
    for k in 0..n {
        st.step(&[1e6 * (TAU * top * k as f64 / FS).sin()], &mut out);
    }
    let env = st.envelope();
    assert!(env[0] / 1e-4 > 2.0);
    assert!((p.modes_at_envelope(&env)[0].0 - top).abs() < 1e-9);
}

/// A cap under 1 is allowed (the reference amplitude is shared with the drop, and full
/// level can sit below it): the rise stops at the cap, the highest frequency is
/// `f0 (1 + rise rise_cap^rise_exponent)`, and the measured resonance, dwelt past the
/// cap, sits there.
#[test]
fn level_rule_cap_below_one() {
    let rule = LevelRule {
        rise: 0.08,
        rise_cap: 0.5,
        ..LevelRule::linear()
    };
    assert!(rule.validate().is_ok());
    assert!(LevelRule {
        rise_cap: 1e-3,
        ..rule
    }
    .validate()
    .is_ok());
    assert!(LevelRule {
        rise_cap: -0.5,
        ..rule
    }
    .validate()
    .is_err());
    let top = 1.0 + 0.08 * 0.25;
    assert!((rule.max_frequency_factor() - top).abs() < 1e-15);
    assert_eq!(rule.frequency_factor(0.5), rule.frequency_factor(1.0));
    assert_eq!(rule.frequency_factor(0.5), spec_factor(&rule, 7.0));
    assert!(rule.frequency_factor(0.3) < top);
    let p = one(200.0, 0.002, Some(rule));
    assert!(p.validate(FS).is_ok());
    assert!((p.highest_frequencies_hz()[0] - 200.0 * top).abs() < 1e-9);
    let (f_small, _) = relay_dwell(&p, 1e-4);
    for force in [0.08, 0.15, 0.75] {
        let (f, r) = relay_dwell(&p, force);
        let want = spec_factor(&rule, r);
        println!(
            "cap 0.5: force {force} V, r = {r:.3}: factor {:.5}, rule {want:.5}",
            f / f_small
        );
        assert!((f / f_small - want).abs() < (0.1 * (want - 1.0)).max(5e-4));
    }
    let (f, r) = relay_dwell(&p, 0.75);
    assert!(r > 0.5, "dwelt past the cap: r = {r}");
    assert!((f / f_small - top).abs() < 5e-4);
}

/// The linear plant drops every rule; `single_mode` keeps the mode's own.
#[test]
fn level_rule_linear_and_single_mode() {
    let mut p = hand_plant();
    p.modes[1].level = Some(LevelRule::hardening(0.03, 1.5));
    assert!(p.is_level_dependent());
    let lin = p.linear();
    assert!(!lin.is_level_dependent());
    assert!(lin.modes.iter().all(|m| m.level.is_none()));
    assert_eq!(p.single_mode(1).modes[0].level, p.modes[1].level);
    let f = p.modes_at_envelope(&[0.0, 1.5 * 5e-7, 0.0]);
    assert!((f[1].0 / 420.0 - 1.0675).abs() < 1e-12, "{:?}", f[1]);
}
