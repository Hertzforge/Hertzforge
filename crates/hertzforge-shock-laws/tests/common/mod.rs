//! What the classical law tests share: the simulated station for each check
//! case, and where the built law files are.

#![allow(dead_code)]

use hertzforge_hal::Backend;
use hertzforge_hal_sim::plant::{FrfPlant, PlantModel};
use hertzforge_hal_sim::SimConfig;
use hertzforge_sdm::{MatrixStack, C64};
use hertzforge_shock_laws::{ClassicalCheckCase, ClassicalCheckPlant, CHECK_PULSE_G};
use std::path::PathBuf;

/// The tests' station rate, Hz.
pub const FS: f64 = 5120.0;

/// A wire with a notch of `depth_db` at `f_hz`, as an FRF plant: a peaking
/// biquad (Q 5) cut by `depth_db`, causal, so the plant's impulse response is
/// short, and a short convolution block, so it adds little delay.
fn notch(f_hz: f64, depth_db: f64) -> PlantModel {
    let df = 1.0;
    let lines = (FS / 2.0 / df) as usize + 1;
    let a = 10f64.powf(-depth_db / 40.0);
    let w0 = std::f64::consts::TAU * f_hz / FS;
    let alpha = w0.sin() / (2.0 * 5.0);
    let (b0, b1, b2) = (1.0 + alpha * a, -2.0 * w0.cos(), 1.0 - alpha * a);
    let (a0, a1, a2) = (1.0 + alpha / a, -2.0 * w0.cos(), 1.0 - alpha / a);
    let mut stack = MatrixStack::zeros(lines, 1, 1);
    for k in 0..lines {
        let w = std::f64::consts::TAU * k as f64 * df / FS;
        let z1 = C64::new(w.cos(), -w.sin());
        let z2 = z1 * z1;
        let num = C64::new(b0, 0.0) + z1 * b1 + z2 * b2;
        let den = C64::new(a0, 0.0) + z1 * a1 + z2 * a2;
        stack.set(k, 0, 0, num / den);
    }
    PlantModel::Frf(FrfPlant {
        block_frames: 16,
        ..FrfPlant::new(stack, df)
    })
}

/// The simulated station a check case names.
pub fn station_for(case: ClassicalCheckCase) -> Box<dyn Backend> {
    let cfg = SimConfig::new()
        .sample_rate_hz(FS)
        .shared_clock(true)
        .noise_level(0.0);
    let cfg = match case.plant() {
        ClassicalCheckPlant::Wire => cfg.resonances(Vec::new()).feedthrough(1.0),
        ClassicalCheckPlant::OneMode { f_hz, q } => cfg.resonance(f_hz, q).feedthrough(1.0),
        ClassicalCheckPlant::Station { f_hz, q } => cfg
            .resonances(vec![(f_hz[0], q), (f_hz[1], q)])
            .feedthrough(1.0),
        ClassicalCheckPlant::DelayedMode { f_hz, q, delay_s } => cfg
            .resonance(f_hz, q)
            .feedthrough(1.0)
            .transport_delay_samples((delay_s * FS).round() as usize),
        // Noise, so the notch's bins identify under the coherence floor.
        ClassicalCheckPlant::Notch { f_hz, depth_db } => {
            cfg.plant(notch(f_hz, depth_db)).noise_level(2e-3)
        }
    };
    let cfg = match case.noise_below_db() {
        Some(db) => {
            let main_rms = CHECK_PULSE_G / 2f64.sqrt() * 10f64.powf(-12.0 / 20.0);
            cfg.noise_level(main_rms * 10f64.powf(-db / 20.0))
        }
        None => cfg,
    };
    Box::new(cfg.build())
}

/// A built law file, `<target>/wasm32-wasip2/release/<name>.wasm`, the target
/// directory found from this test binary's own path.
pub fn law_file(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary has a path");
    // <target>/debug/deps/<test> -> <target>
    let target = exe
        .ancestors()
        .nth(3)
        .expect("the test binary sits under <target>/<profile>/deps")
        .to_path_buf();
    let path = target
        .join("wasm32-wasip2")
        .join("release")
        .join(format!("{name}.wasm"));
    assert!(
        path.is_file(),
        "{} is missing: run scripts/build_laws.sh (it builds every guest law into \
         <target>/wasm32-wasip2/release/)",
        path.display()
    );
    path
}

/// The SRS tests' station rate, Hz.
pub const SRS_FS: f64 = 20_480.0;

/// The simulated station an SRS check case names: a feedthrough of 1 with the
/// case's modes, and its measurement noise.
pub fn srs_station_for(case: hertzforge_shock_laws::SrsCheckCase) -> Box<dyn Backend> {
    use hertzforge_shock_laws::SrsCheckPlant;
    let cfg = SimConfig::new()
        .sample_rate_hz(SRS_FS)
        .shared_clock(true)
        .noise_level(case.noise_rms_g(SRS_FS).unwrap_or(0.0));
    let cfg = match case.plant() {
        SrsCheckPlant::Wire => cfg.resonances(Vec::new()).feedthrough(1.0),
        SrsCheckPlant::OneMode { f_hz, q } => cfg.resonance(f_hz, q).feedthrough(1.0),
        SrsCheckPlant::TwoModes { f_hz, q } => cfg
            .resonances(vec![(f_hz[0], q), (f_hz[1], q)])
            .feedthrough(1.0),
    };
    Box::new(cfg.build())
}
