//! Pulse-train scheduler + rate model tests.

use crate::path::DrivePath;
use crate::train::{gunfire_spacing, RateModel, Spacing, TrainError, TrainSpec};

#[test]
fn conservative_rate_model_floors_are_nontrivial() {
    let m = RateModel::conservative_default();
    // Host floor: max of cooling/settling/compute (no device recompute).
    let host_floor = m.effective_min_spacing(DrivePath::HostDac);
    assert_eq!(host_floor, 1.0, "host floor = max(1.0, 0.5, 0.25)");
    // Delegated floor: the device-recompute floor dominates.
    let del_floor = m.effective_min_spacing(DrivePath::Delegated);
    assert_eq!(
        del_floor, 20.0,
        "delegated floor = device recompute (dominant)"
    );
}

#[test]
fn rate_model_refuses_everything_when_read_cadence_fails() {
    let m = RateModel {
        read_cadence_ok: false,
        ..RateModel::conservative_default()
    };
    assert!(m.effective_min_spacing(DrivePath::HostDac).is_infinite());
}

#[test]
fn single_pulse_is_allowed_on_any_path() {
    let m = RateModel::conservative_default();
    // A single pulse is not a train; it plans on both paths, no hardware-block marker.
    for p in [DrivePath::Delegated, DrivePath::HostDac] {
        let plan = TrainSpec::single().plan(p, &m).unwrap();
        assert_eq!(plan.count, 1);
        assert_eq!(plan.total_duration_s, 0.0);
        assert!(!plan.hardware_blocked, "a single pulse is not a train");
    }
}

#[test]
fn at_rate_train_is_host_only_f7() {
    let m = RateModel::conservative_default();
    // A multi-pulse (at-rate) train on the DELEGATED path is refused.
    let spec = TrainSpec::delayed(5, 30.0); // 30 s > delegated floor 20 s
    let err = spec.plan(DrivePath::Delegated, &m).unwrap_err();
    assert!(matches!(err, TrainError::AtRateNotOnPath { .. }));
    // The same train is fine on the HOST path.
    let plan = spec.plan(DrivePath::HostDac, &m).unwrap();
    assert_eq!(plan.count, 5);
    assert_eq!(plan.realized_spacing_s, 30.0);
    assert_eq!(plan.total_duration_s, 30.0 * 4.0);
    assert!(
        !plan.hardware_blocked,
        "the write-trigger is a DELEGATED-path unknown; a HOST train writes its own DAC samples \
         and never had a trigger semantic to decode"
    );
}

/// The hardware-blocked marker is PATH-AWARE (2026-08-08). It used to be `count > 1` on every
/// path, which shipped a delegated-device caveat on every host train.
///
/// RED-MUTATION: restore `hardware_blocked: self.count > 1` in `TrainSpec::plan`
/// and the HostDac rows below red.
///
/// The DELEGATED half of the property cannot be asserted through `plan()` at all,
/// and that is stated here rather than faked: `pulse_trains_at_rate` is `false` on
/// Path A, so a multi-pulse delegated train is refused as `AtRateNotOnPath` one
/// check EARLIER and never reaches the marker. Asserting the marker on a path that
/// cannot produce it would be a result that cannot fail. What is
/// asserted instead is exactly that refusal, so the day Path A gains at-rate trains
/// this test says where the marker has to come back.
#[test]
fn the_f8_marker_is_delegated_only_and_currently_unreachable() {
    let m = RateModel::conservative_default();
    for count in [1u32, 2, 5] {
        let plan = TrainSpec::delayed(count, 30.0)
            .plan(DrivePath::HostDac, &m)
            .unwrap();
        assert!(
            !plan.hardware_blocked,
            "host train of {count} must not carry the delegated caveat"
        );
    }
    // A single DELEGATED pulse plans (not a train) and carries no marker.
    let single = TrainSpec::single().plan(DrivePath::Delegated, &m).unwrap();
    assert!(!single.hardware_blocked);
    // A multi-pulse DELEGATED train never reaches the marker: the at-rate check refuses
    // it first.
    assert!(matches!(
        TrainSpec::delayed(2, 30.0).plan(DrivePath::Delegated, &m),
        Err(TrainError::AtRateNotOnPath { .. })
    ));
}

#[test]
fn spec_below_floor_is_refused_f7() {
    let m = RateModel::conservative_default();
    // Host floor is 1.0 s; a 0.1 s delayed train is refused.
    let spec = TrainSpec::delayed(3, 0.1);
    let err = spec.plan(DrivePath::HostDac, &m).unwrap_err();
    match err {
        TrainError::SpacingBelowFloor {
            requested_s,
            floor_s,
        } => {
            assert_eq!(requested_s, 0.1);
            assert_eq!(floor_s, 1.0);
        }
        other => panic!("expected SpacingBelowFloor, got {other:?}"),
    }
}

#[test]
fn max_rate_realizes_exactly_the_floor() {
    let m = RateModel::conservative_default();
    let plan = TrainSpec::max_rate(4).plan(DrivePath::HostDac, &m).unwrap();
    assert_eq!(plan.realized_spacing_s, 1.0, "MaxRate == host floor");
    assert_eq!(plan.floor_s, 1.0);
}

#[test]
fn empty_and_bad_spacing_are_errors() {
    let m = RateModel::conservative_default();
    assert!(matches!(
        TrainSpec {
            count: 0,
            spacing: Spacing::Delay(1.0)
        }
        .plan(DrivePath::HostDac, &m),
        Err(TrainError::EmptyTrain)
    ));
    assert!(matches!(
        TrainSpec::delayed(3, -1.0).plan(DrivePath::HostDac, &m),
        Err(TrainError::BadSpacing(_))
    ));
    assert!(matches!(
        TrainSpec::delayed(3, f64::NAN).plan(DrivePath::HostDac, &m),
        Err(TrainError::BadSpacing(_))
    ));
}

#[test]
fn gunfire_helper_converts_rounds_per_min_to_delay_proc_iv() {
    // 600 rounds/min -> 0.1 s between pulse starts (Procedure IV, 60/rate).
    match gunfire_spacing(600.0).unwrap() {
        Spacing::Delay(d) => assert!((d - 0.1).abs() < 1e-12),
        other => panic!("expected a Delay, got {other:?}"),
    }
    // Bad rate rejected.
    assert!(matches!(
        gunfire_spacing(0.0),
        Err(TrainError::BadSpacing(_))
    ));
    assert!(matches!(
        gunfire_spacing(-5.0),
        Err(TrainError::BadSpacing(_))
    ));
}

#[test]
fn gunfire_train_at_rate_needs_host_path_f7() {
    // A realistic gunfire rate (600 rpm -> 0.1 s) is below the host floor here, so it
    // is refused on the host too until the rate model has faster-cooling numbers -
    // but on the DELEGATED path it is refused as AtRateNotOnPath first.
    let m = RateModel::conservative_default();
    let spacing = gunfire_spacing(600.0).unwrap();
    let spec = TrainSpec { count: 10, spacing };
    assert!(matches!(
        spec.plan(DrivePath::Delegated, &m),
        Err(TrainError::AtRateNotOnPath { .. })
    ));
    // On the host it is refused for being below the cooling floor (honest: this rate
    // is not sustainable with the conservative default cooling floor).
    assert!(matches!(
        spec.plan(DrivePath::HostDac, &m),
        Err(TrainError::SpacingBelowFloor { .. })
    ));
    // A slower gunfire rate (30 rpm -> 2 s) clears the host floor.
    let slow = TrainSpec {
        count: 10,
        spacing: gunfire_spacing(30.0).unwrap(),
    };
    let plan = slow.plan(DrivePath::HostDac, &m).unwrap();
    assert_eq!(plan.realized_spacing_s, 2.0);
}

#[test]
fn plan_round_trips_through_json() {
    let m = RateModel::conservative_default();
    let plan = TrainSpec::delayed(3, 5.0)
        .plan(DrivePath::HostDac, &m)
        .unwrap();
    let js = serde_json::to_string(&plan).unwrap();
    let back: crate::train::TrainPlan = serde_json::from_str(&js).unwrap();
    assert_eq!(plan, back);
}
