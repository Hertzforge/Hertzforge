//! A1: convergence on the simulator's standard modal plants with the default law.
//!
//! Each plant is controlled to a reference it can reach (`H X H^H` from its exact
//! FRF and a random partially coherent drive), 64 Welch frames per loop, the
//! estimate averaged over loops (memory 8). The bar: every control PSD within
//! +/- 1 dB of the reference on every band line, within 20 loops of reaching full
//! level, and with the default law the mean coherence error of every pair under
//! 0.1. The loops it took and the coherence error are printed for the record.
//!
//! The normal suite runs on 720 band lines (2 Hz, 160 Hz to 1.6 kHz) so it stays
//! quick in a debug build; the 72 x 24 run, the design's full scale, is ignored
//! and run in release with the timing test.
mod common;

use common::*;
use hertzforge_hal_sim::plants::StandardPlant;

fn run(plant: StandardPlant, grid: Grid, tag: &str, max_loops: u64) -> Convergence {
    let p = plant.build();
    let (cfg, table) = physical_config(&p, &grid, 0.5);
    let mut svc = service(cfg, table, &p, &grid, tag);
    let c = converge(&mut svc, 1.0, max_loops);
    eprintln!(
        "A1 {}: {} band lines, loops to converge {:?}, worst {:.3} dB, mean rms {:.3} dB, mean \
         coherence error {:.4}, loops run {}, gate passed {:?} (worst condition {:.1}), timing {:?}",
        p.name,
        svc.band().1 - svc.band().0,
        c.loops_to_converge,
        c.final_worst_db,
        c.final_rms_db,
        c.mean_coherence_error,
        c.loops,
        svc.gate().map(|g| g.passed),
        svc.gate().map(|g| g.worst_condition).unwrap_or(f64::NAN),
        svc.timing()
    );
    c
}

fn check(c: &Convergence) {
    assert!(
        c.loops_to_converge.is_some_and(|n| n <= 20),
        "not within 1 dB in 20 loops at full level: {c:?}"
    );
    assert!(c.mean_coherence_error < 0.1, "{c:?}");
}

#[test]
fn a1_modal_4x4_converges_within_20_loops_of_full_level() {
    check(&run(
        StandardPlant::Modal4x4,
        Grid::standard(),
        "a1-4x4",
        20,
    ));
}

#[test]
fn a1_modal_12x6_converges_within_20_loops_of_full_level() {
    check(&run(
        StandardPlant::Modal12x6,
        Grid::standard(),
        "a1-12x6",
        20,
    ));
}

#[test]
fn a1_modal_24x24_converges_within_20_loops_of_full_level() {
    check(&run(
        StandardPlant::Modal24x24,
        Grid::standard(),
        "a1-24x24",
        20,
    ));
}

/// The design's full scale: 72 controls, 24 drives, 1,600 band lines. Release
/// only; run with `cargo test --release -- --ignored`.
#[test]
#[ignore]
fn a1_modal_72x24_converges_at_1600_lines() {
    check(&run(
        StandardPlant::Modal72x24,
        Grid::full_scale(),
        "a1-72x24",
        20,
    ));
}
