//! A5: compute per loop at the design's full scale, 72 controls by 24 drives on
//! 1,600 band lines, with the per-line math on the thread pool.
//!
//! Release builds only (a debug build of the linear algebra measures nothing):
//! `cargo test --release -p hertzforge-mimo-random --test timing`. Listed in
//! `scripts/rust_timing_tests.txt`. It runs the modal 72 x 24 plant through the
//! service to full level and reports the milliseconds each loop spent estimating,
//! in the law, in the guards and in synthesis, against the loop's period.
mod common;

use common::*;
use hertzforge_hal_sim::plants::StandardPlant;
use hertzforge_mimo_random::MimoState;

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn a5_compute_per_loop_at_72_by_24_by_1600_lines() {
    let grid = Grid::full_scale();
    let p = StandardPlant::Modal72x24.build();
    let (cfg, table) = physical_config(&p, &grid, 0.5);
    let mut svc = service(cfg, table, &p, &grid, "a5-timing");
    let lines = svc.band().1 - svc.band().0;
    assert_eq!(lines, 1600);
    let st = svc.run_auto(0).expect("engages");
    assert_eq!(st, MimoState::Engaged);
    let period_s = svc.loop_config().segment_seconds();
    let mut rows = Vec::new();
    while svc.state().is_running() && svc.loops_at_full_level() < 4 {
        svc.loop_tick().expect("a loop");
        if svc.state() == MimoState::FullLevel {
            rows.push(svc.timing());
        }
    }
    assert!(
        !rows.is_empty(),
        "never reached full level: {:?}",
        svc.trip()
    );
    let n = rows.len() as f64;
    let mean = |f: &dyn Fn(&hertzforge_mimo_random::report::LoopTiming) -> f64| {
        rows.iter().map(f).sum::<f64>() / n
    };
    let est = mean(&|t| t.estimate_ms);
    let law = mean(&|t| t.law_ms);
    let guards = mean(&|t| t.guards_ms);
    let synth = mean(&|t| t.synth_ms);
    let plant = mean(&|t| t.plant_ms);
    let compute = est + law + guards + synth;
    eprintln!(
        "A5 72 x 24 x {lines} lines, {} threads, {} loops at full level, loop period {:.2} s: \
         estimate {est:.1} ms, law {law:.1} ms, guards {guards:.1} ms, synthesis {synth:.1} ms, \
         compute {compute:.1} ms ({:.1} % of the period); simulated plant {plant:.1} ms; worst \
         error {:?} dB",
        hertzforge_mimo_random::pool::threads(),
        rows.len(),
        period_s,
        100.0 * compute / (1e3 * period_s),
        svc.worst_error_db()
            .iter()
            .fold(0.0f64, |a, b| a.max(b.abs()))
    );
    assert!(
        compute < 0.25 * 1e3 * period_s,
        "compute {compute:.1} ms is over a quarter of the {period_s:.2} s loop"
    );
}
