//! The shock plants: the jointed modal plant (frequency down and damping up with a
//! running amplitude envelope, a delay per output) and the standard shock set, and the
//! proof that every plant that existed before them plays exactly what it played.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal::{AcquisitionConfig, Backend};
use hertzforge_hal_sim::bench_network::BenchNetwork;
use hertzforge_hal_sim::plants::StandardPlant;
use hertzforge_hal_sim::{
    Discretization, ModalPlant, ModalSoftening, Mode, ModeResponse, PlantModel, ResonatorBank,
    SimBackend, SimConfig,
};

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
    let mut state: u64 = 0x5EED_0F_5110C4;
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
    assert_eq!(RECORDED.len(), got.len(), "every plant has a recorded fingerprint");
    // The plants' coefficients come through the platform's `sin`, `cos` and `powf`,
    // whose last bit differs between math libraries, so the recorded values are the
    // recording platform's. Elsewhere the run is checked for repeatability only.
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        for (name, h) in RECORDED {
            let g = got.iter().find(|(n, _)| n == name).expect("a recorded plant");
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
