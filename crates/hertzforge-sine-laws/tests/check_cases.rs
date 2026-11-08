//! The Sine check's seven cases on the simulator: the built-in law at its
//! default parameters passes every one, and the core's rise cap is at least
//! twice the largest rise the built-in asked for on any of them.

mod common;

use common::station_for;
use hertzforge_sine::law::servo_blend::{ServoBlendFactory, ServoBlendParams};
use hertzforge_sine::law::LawGuards;
use hertzforge_sine_laws::{judge_sine, run_check_case, SineCheckCase, SINE_CHECK_TICK_S};
use std::sync::Arc;

#[test]
fn the_built_in_passes_every_case() {
    let mut failed = Vec::new();
    let mut worst_rise = 0f64;
    for case in SineCheckCase::ALL {
        let f = Arc::new(ServoBlendFactory {
            params: ServoBlendParams::default(),
        });
        let m = run_check_case(case, f, station_for(case)).expect("the case runs");
        let j = judge_sine(case, &m);
        println!(
            "{:<20} {} {} | worst rise {:.3} dB, max drive {:.4} V, limit over {:?} dB, \
             release overshoot {:?} dB",
            case.name(),
            if j.passed { "PASS" } else { "FAIL" },
            j.detail,
            m.worst_rise_db,
            m.max_drive_vpk,
            m.worst_limit_over_db,
            m.worst_release_overshoot_db
        );
        worst_rise = worst_rise.max(m.worst_rise_db);
        if !j.passed {
            failed.push(case.name());
        }
    }
    let cap = LawGuards::default().max_rise_db_per_s * SINE_CHECK_TICK_S;
    println!("worst rise over every case: {worst_rise:.3} dB in a tick; the rise cap {cap:.2} dB in a tick");
    assert!(failed.is_empty(), "failed: {failed:?}");
    assert!(
        cap >= 2.0 * worst_rise,
        "the rise cap {cap} dB a tick is under twice the built-in's worst rise {worst_rise}"
    );
}
