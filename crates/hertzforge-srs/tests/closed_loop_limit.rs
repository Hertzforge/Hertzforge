//! A limit channel under the closed loop: the prediction for every pulse's
//! drive stays under the channel's bound at every level of the schedule, and
//! a band the notch holds low does not fail the pulse.

mod common;

use common::*;
use hertzforge_srs::service::config::{FrfSpec, LimitChannelSpec, LimitSet, LimitTarget};
use hertzforge_srs::service::report::RunState;
use hertzforge_srs::SrsTestConfig;

const TARGET_G: f64 = 1.0;
const BOUND_G: f64 = 2.5 * TARGET_G;
const HEADROOM_DB: f64 = 1.0;

/// A 1 g/V wire for control, AUTO -12, -6 and 0 dB, and one limit channel
/// whose supplied gain is 4 at 500 Hz (1 far from it) under a flat bound.
fn limited() -> SrsTestConfig {
    let mut cfg = closed_cfg(TARGET_G);
    cfg.schedule.table_db = vec![-12.0, -6.0, 0.0];
    cfg.schedule.start_db = -12.0;
    cfg.schedule.pulses_per_level = vec![2, 2, 3];
    let freqs: Vec<f64> = (0..=400).map(|i| i as f64 * 25.0).collect();
    let mag: Vec<f64> = freqs
        .iter()
        .map(|&f| 1.0 + 3.0 / (1.0 + ((f - 500.0) / 100.0).powi(2)))
        .collect();
    cfg.limits = Some(LimitSet {
        channels: vec![LimitChannelSpec {
            name: "limit".into(),
            limit: LimitTarget::Flat { accel_g: BOUND_G },
            frf: Some(FrfSpec {
                freqs_hz: freqs,
                mag,
            }),
        }],
        headroom_db: HEADROOM_DB,
    });
    cfg
}

#[test]
fn the_limit_prediction_stays_under_its_bound_on_every_pulse_at_every_level() {
    let mut s = service(limited(), Linear::new(1.0));
    run(&mut s);
    let rows = s.report_data().pulses;
    assert!(
        rows.iter().any(|r| r.level_db == 0.0),
        "it reached full level"
    );
    for r in &rows {
        // `worst_limit_db` is over the allowed level, the bound less the
        // headroom.
        let over_bound = r.worst_limit_db.unwrap() - HEADROOM_DB;
        assert!(
            over_bound <= 0.5,
            "pulse {} at {} dB: the prediction is {over_bound:+.2} dB over the bound",
            r.seq,
            r.level_db
        );
    }
}

#[test]
fn a_band_the_notch_holds_low_does_not_fail_the_pulse() {
    let mut s = service(limited(), Linear::new(1.0));
    assert_eq!(run(&mut s), RunState::Stopped, "trip: {:?}", s.trip());
    let notched: Vec<usize> = s
        .arm_notch_report()
        .unwrap()
        .notched_bands
        .iter()
        .map(|b| b.band_index)
        .collect();
    assert!(!notched.is_empty());
    let r = s.report_data();
    let last = r.pulses.last().unwrap();
    assert_eq!(last.level_db, 0.0);
    assert_eq!(last.passed, Some(true));
    // The notch is real: the notched bands read low at full level, and every
    // other band is inside the alarm band.
    let worst_notched = notched
        .iter()
        .map(|&b| r.band_results[b].err_db.unwrap())
        .fold(0.0f64, f64::min);
    assert!(worst_notched < -2.0, "{worst_notched}");
    for (b, br) in r.band_results.iter().enumerate() {
        if !notched.contains(&b) {
            assert!(br.err_db.unwrap().abs() <= 3.0, "band {b}: {:?}", br.err_db);
        }
    }
}
