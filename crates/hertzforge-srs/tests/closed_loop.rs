//! The SRS closed loop with the built-in law: identification in volts, the
//! review hold, the seed from the identified gain, the drive in volts, the
//! time between pulses, the records and the report line.

mod common;

use common::*;
use hertzforge_srs::service::config::ShockCaptureTrigger;
use hertzforge_srs::service::report::RunState;

#[test]
fn the_built_in_law_closes_the_loop_with_the_drive_in_volts() {
    // The same target on a 1 g/V and a 2 g/V article: the run completes on
    // both, and the second needs half the volts.
    let mut drives = Vec::new();
    for gain in [1.0, 2.0] {
        let p = Linear::new(gain);
        let fired = p.fired.clone();
        let mut s = service(closed_cfg(1.0), p);
        assert_eq!(run(&mut s), RunState::Stopped, "trip: {:?}", s.trip());
        let st = s.status();
        let id = st.identification.clone().expect("identified");
        assert!(id.passed && id.coherence_mean > 0.99, "{id:?}");
        let rows = s.report_data().pulses;
        assert_eq!(rows.len(), 4, "two passing pulses at each of two levels");
        for r in &rows {
            assert_eq!(r.passed, Some(true));
            assert!(r.law.as_ref().unwrap().used);
            assert!(r.worst_err_db.unwrap().abs() < 1.5, "{r:?}");
        }
        // Every row's drive peak is the drive the plant was handed.
        let f = fired.lock().unwrap();
        assert_eq!(f.len(), 5, "the identification burst and four pulses");
        for (r, d) in rows.iter().zip(&f[1..]) {
            assert!((r.drive_vpk.unwrap() - peak(d)).abs() < 1e-12);
        }
        drives.push(rows[3].drive_vpk.unwrap());
        let law = st.law.expect("a law status");
        assert_eq!(law.calls, 5, "the seed and one call a pulse");
        assert_eq!(law.misses, 0);
        let report = s.report_data().law.expect("the report's law line");
        assert_eq!(report.line, "SRS ratio 0.1.0 (built in)");
        assert_eq!(report.budget_ms, 250.0);
    }
    let ratio = drives[1] / drives[0];
    assert!(
        (ratio - 0.5).abs() < 0.01,
        "2 g/V needs half the volts: {ratio}"
    );
}

#[test]
fn the_core_holds_for_review_whatever_the_review_setting_says() {
    for review in [true, false] {
        let mut cfg = closed_cfg(1.0);
        cfg.review_after_sysid = review;
        let mut s = service(cfg, Linear::new(1.0));
        assert_eq!(identify(&mut s), RunState::AwaitingReview);
        assert!(s.fire_pulse().is_err(), "nothing fires at the hold");
        assert!(s.identification_view().is_some());
        s.proceed_after_review().unwrap();
        assert_eq!(s.state(), RunState::Armed);
    }
}

#[test]
fn every_law_call_is_recorded_and_the_run_folder_gets_them() {
    // Without capture the records wait for the host to drain them.
    let mut s = service(closed_cfg(1.0), Linear::new(1.0));
    run(&mut s);
    let recs = s.drain_law_records();
    assert_eq!(recs.len(), 5);
    assert_eq!(recs[0].pulse_index, 0);
    assert!(recs.iter().all(|r| r.miss.is_none()));
    assert!(s.drain_law_records().is_empty(), "a drain empties the ring");

    // With capture the run writes them into its folder between pulses.
    let root = std::env::temp_dir().join(format!("hertzforge-srs-records-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut cfg = closed_cfg(1.0);
    cfg.capture.trigger = ShockCaptureTrigger::EveryPulse;
    let mut s = hertzforge_srs::SrsShockService::new(
        cfg,
        siso_table(),
        hertzforge_srs::FirePath::Host(Box::new(Linear::new(1.0))),
        Some(root.clone()),
    )
    .unwrap();
    s.set_pacer(Box::new(hertzforge_srs::service::pacer::VirtualPacer::new(
        0.01,
    )));
    run(&mut s);
    assert!(s.drain_law_records().is_empty(), "already written");
    let run_dir = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.join("law.json").exists())
        .expect("a run folder with law.json");
    let law: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("law.json")).unwrap()).unwrap();
    assert_eq!(law["law"]["id"], "srs_ratio");
    assert!(run_dir.join("law_pulses.bin").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_identification_error_scales_the_gain_the_seed_divides_by() {
    // +3 dB on the identified gain makes the seed 3 dB low; on a linear plant
    // the first pulse's SRS is exactly that much lower in every band.
    let mut first = Vec::new();
    for db in [0.0, 3.0] {
        let mut s = service(closed_cfg(1.0), Linear::new(1.0)).with_identification_scale_db(db);
        identify(&mut s);
        s.proceed_after_review().unwrap();
        s.fire_pulse().unwrap();
        first.push(s.last_measured_srs().unwrap().to_vec());
    }
    let want = 10f64.powf(-3.0 / 20.0);
    for (a, b) in first[0].iter().zip(&first[1]) {
        assert!((b / a - want).abs() < 1e-9, "{} against {want}", b / a);
    }
}
