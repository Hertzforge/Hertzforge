//! The frequency response update mode on the simulator's 4 x 4 modal structure.
//!
//! With `FrfMode::Update { blend: 1.0 }` the response handed to the law after a
//! loop is the loop's own H1 estimate, so on a linear structure measured with
//! little noise it should agree with the identification's. These tests read the
//! response the law sees (the `Frf` quantity) at engage, when it is the
//! identification's, and again after loops at full level, and compare the two
//! per band line.
mod common;

use common::*;
use hertzforge_hal_sim::plants::StandardPlant;
use hertzforge_mimo_random::{FrfMode, MimoConfig, MimoQuantity, MimoRandomService, MimoState};
use hertzforge_sdm::C64;

/// The response the law sees now, rebuilt from the `Frf` quantity's magnitude and
/// phase: band lines by controls by drives.
fn law_frf(svc: &MimoRandomService, n_c: usize, n_d: usize) -> Vec<Vec<C64>> {
    let (lo, hi) = svc.band();
    let mut out = vec![vec![C64::new(0.0, 0.0); n_c * n_d]; hi - lo];
    for i in 0..n_c {
        for k in 0..n_d {
            let s = svc
                .quantity(&MimoQuantity::Frf(i, k))
                .expect("the response the law sees");
            let (mag, ph) = (&s.traces[0].y, &s.traces[1].y);
            for (l, row) in out.iter_mut().enumerate() {
                row[i * n_d + k] = C64::from_polar(mag[l], ph[l].to_radians());
            }
        }
    }
    out
}

/// Per band line, `||a - b||_F / ||b||_F`, sorted.
fn relative_distances(a: &[Vec<C64>], b: &[Vec<C64>]) -> Vec<f64> {
    let mut d: Vec<f64> = a
        .iter()
        .zip(b)
        .map(|(x, y)| {
            let n: f64 = x.iter().zip(y).map(|(p, q)| (p - q).norm_sqr()).sum();
            let s: f64 = y.iter().map(|q| q.norm_sqr()).sum();
            (n / s).sqrt()
        })
        .collect();
    d.sort_by(f64::total_cmp);
    d
}

/// Engage, then run loops until `full_loops` loops at full level have updated the
/// response. Returns the identification's response (read at engage) and the
/// response in force at the end.
fn engage_and_update(
    svc: &mut MimoRandomService,
    n_c: usize,
    n_d: usize,
    full_loops: u64,
) -> (Vec<Vec<C64>>, Vec<Vec<C64>>) {
    let st = svc.run_auto(0).expect("the run engages");
    assert_eq!(st, MimoState::Engaged, "{:?}", svc.log().events());
    let ident = law_frf(svc, n_c, n_d);
    let mut at_full = 0;
    while svc.state().is_running() && at_full < full_loops {
        svc.loop_tick().expect("a loop");
        if svc.state() == MimoState::FullLevel {
            at_full += 1;
        }
        assert!(svc.loop_count() < full_loops + 30, "no full level reached");
    }
    assert!(
        svc.state().is_running(),
        "the run stopped: {:?}",
        svc.log().events()
    );
    (ident, law_frf(svc, n_c, n_d))
}

fn update_config(cfg: &mut MimoConfig) {
    cfg.frf_mode = FrfMode::Update { blend: 1.0 };
}

/// Engaging suspends the backend and restarts it cold. The lag the drive records
/// are aligned by is measured once, at identification; if the restart changes
/// the drive-to-response timing, every loop's response carries a pure delay
/// against the identification's, and a response updated at blend 1 differs
/// from the identification by a phase that grows with frequency (an 8-sample
/// delay on the simulator before its stop discarded the queued drive).
#[test]
fn an_updated_response_keeps_the_identifications_timing_after_engaging() {
    let grid = Grid::standard();
    let p = StandardPlant::Modal4x4.build();
    let (mut cfg, table) = physical_config(&p, &grid, 0.5);
    update_config(&mut cfg);
    let mut svc = service(cfg, table, &p, &grid, "frf-update-timing");
    let (ident, updated) = engage_and_update(&mut svc, 4, 4, 4);
    let d = relative_distances(&updated, &ident);
    let median = d[d.len() / 2];
    let p90 = d[d.len() * 9 / 10];
    eprintln!(
        "updated against identified, {} band lines: median {median:.4}, 90th percentile \
         {p90:.4}, max {:.4}",
        d.len(),
        d[d.len() - 1]
    );
    assert!(median < 0.05, "median relative distance {median}");
    assert!(p90 < 0.15, "90th percentile relative distance {p90}");
}
