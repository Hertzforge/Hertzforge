//! Drive peak limiting tests (the honesty split): predictive
//! peak-accel scale-down pre-fire; between-pulse V and I measured enforcement;
//! Path A V/I inertness gated by `PathCapabilities.vi_limiting_observable`.

use crate::drive_limit::{
    predict_peak_accel_g, DriveLimit, DriveMonitor, DriveMonitorKind, MeasuredPeak,
    MonitorEnforcement,
};
use crate::path::DrivePath;

fn v_monitor(ceiling: f64) -> DriveMonitor {
    DriveMonitor {
        channel: "amp_v_drop".into(),
        eu: "V".into(),
        peak_ceiling: ceiling,
        kind: DriveMonitorKind::BetweenPulse,
    }
}

fn i_monitor(ceiling: f64) -> DriveMonitor {
    DriveMonitor {
        channel: "amp_i_drop".into(),
        eu: "A".into(),
        peak_ceiling: ceiling,
        kind: DriveMonitorKind::BetweenPulse,
    }
}

fn accel_monitor(ceiling: f64) -> DriveMonitor {
    DriveMonitor {
        channel: "table_accel".into(),
        eu: "g".into(),
        peak_ceiling: ceiling,
        kind: DriveMonitorKind::BetweenPulse,
    }
}

// ---------------------------------------------------------------------------
// Predictive peak-accel (pre-fire: the ONE honestly predictable quantity)
// ---------------------------------------------------------------------------

/// A flat FRF magnitude of 2.0 doubles the drive exactly (zero-phase filtering of a
/// flat magnitude is a pure gain), so the predicted peak is exactly 2x the drive
/// peak.
#[test]
fn predict_peak_accel_flat_frf_is_exact_gain() {
    let fs = 8192.0;
    // A 10 g half-sine-ish bump.
    let n = 512;
    let drive: Vec<f64> = (0..n)
        .map(|i| 10.0 * (std::f64::consts::PI * i as f64 / n as f64).sin())
        .collect();
    let frf_freqs = vec![0.0, 4096.0];
    let frf_mag = vec![2.0, 2.0];
    let pred = predict_peak_accel_g(&drive, fs, &frf_freqs, &frf_mag);
    assert!(
        (pred - 20.0).abs() < 1e-9,
        "flat |H|=2 must predict exactly 2x the 10 g peak, got {pred}"
    );
}

/// Predictive scale-down: a pulse that would over-g gets `allowed = false` and the
/// exact `ceiling / predicted` pre-fire scale.
#[test]
fn pre_fire_accel_scales_down_an_over_g_pulse() {
    let limit = DriveLimit {
        peak_accel_g: Some(15.0),
        monitors: vec![],
    };
    let decision = limit.pre_fire_accel(20.0);
    assert!(
        !decision.allowed,
        "20 g predicted vs 15 g ceiling: not as-is"
    );
    assert!(
        (decision.scale_factor - 0.75).abs() < 1e-12,
        "scale = 15/20 = 0.75, got {}",
        decision.scale_factor
    );
    // The scaled prediction fits exactly (linearity).
    let rescored = limit.pre_fire_accel(20.0 * decision.scale_factor);
    assert!(rescored.allowed, "the scaled pulse must fit the ceiling");
    assert_eq!(rescored.scale_factor, 1.0);
}

/// Under the ceiling: fire as-is.
#[test]
fn pre_fire_accel_under_ceiling_is_allowed() {
    let limit = DriveLimit {
        peak_accel_g: Some(15.0),
        monitors: vec![],
    };
    let d = limit.pre_fire_accel(10.0);
    assert!(d.allowed);
    assert_eq!(d.scale_factor, 1.0);
}

/// No ceiling configured: trivially allowed.
#[test]
fn pre_fire_accel_without_ceiling_is_noop() {
    let limit = DriveLimit {
        peak_accel_g: None,
        monitors: vec![],
    };
    let d = limit.pre_fire_accel(1e6);
    assert!(d.allowed);
    assert_eq!(d.scale_factor, 1.0);
}

/// A non-finite prediction refuses to fire (garbage in must not fire a pulse).
#[test]
fn pre_fire_accel_refuses_on_invalid_prediction() {
    let limit = DriveLimit {
        peak_accel_g: Some(15.0),
        monitors: vec![],
    };
    let d = limit.pre_fire_accel(f64::NAN);
    assert!(!d.allowed);
    assert_eq!(d.scale_factor, 0.0);
}

/// End-to-end predictive path: predict through an FRF, decide, scale.
#[test]
fn predictive_path_end_to_end() {
    let fs = 8192.0;
    let n = 512;
    let drive: Vec<f64> = (0..n)
        .map(|i| 10.0 * (std::f64::consts::PI * i as f64 / n as f64).sin())
        .collect();
    // |H| = 3 flat -> predicted 30 g against a 12 g ceiling.
    let pred = predict_peak_accel_g(&drive, fs, &[0.0, 4096.0], &[3.0, 3.0]);
    let limit = DriveLimit {
        peak_accel_g: Some(12.0),
        monitors: vec![],
    };
    let d = limit.pre_fire_accel(pred);
    assert!(!d.allowed);
    assert!(
        (d.scale_factor - 12.0 / 30.0).abs() < 1e-9,
        "scale = 12/30, got {}",
        d.scale_factor
    );
}

/// A predictive-kind monitor channel uses the same pre-fire math; an unknown or
/// between-pulse channel yields None.
#[test]
fn pre_fire_monitor_matches_predictive_kind_only() {
    let limit = DriveLimit {
        peak_accel_g: None,
        monitors: vec![
            DriveMonitor {
                channel: "resp_accel".into(),
                eu: "g".into(),
                peak_ceiling: 50.0,
                kind: DriveMonitorKind::Predictive,
            },
            v_monitor(100.0),
        ],
    };
    let d = limit.pre_fire_monitor("resp_accel", 60.0).unwrap();
    assert!(!d.allowed);
    assert!((d.scale_factor - 50.0 / 60.0).abs() < 1e-12);
    // BetweenPulse monitors are not pre-fire; unknown channels are None.
    assert!(limit.pre_fire_monitor("amp_v_drop", 60.0).is_none());
    assert!(limit.pre_fire_monitor("nope", 60.0).is_none());
}

// ---------------------------------------------------------------------------
// Between-pulse measured enforcement (scale the NEXT pulse; not pre-fire)
// ---------------------------------------------------------------------------

/// Measured over-ceiling VOLTAGE scales the next pulse by ceiling/measured.
#[test]
fn between_pulse_voltage_scale_down() {
    let limit = DriveLimit {
        peak_accel_g: None,
        monitors: vec![v_monitor(100.0)],
    };
    let caps = DrivePath::HostDac.capabilities();
    let d = limit.between_pulse(
        &caps,
        &[MeasuredPeak {
            channel: "amp_v_drop".into(),
            peak_eu: 125.0,
        }],
    );
    assert!(
        (d.scale_factor - 0.8).abs() < 1e-12,
        "scale = 100/125 = 0.8, got {}",
        d.scale_factor
    );
    assert_eq!(d.actions.len(), 1);
    assert_eq!(d.actions[0].enforcement, MonitorEnforcement::Enforced);
    assert_eq!(d.actions[0].measured_peak_eu, Some(125.0));
}

/// Measured over-ceiling CURRENT scales the next pulse; with both V and I breached
/// the TIGHTEST scale wins (safety-monotone arbitration).
#[test]
fn between_pulse_current_and_tightest_wins() {
    let limit = DriveLimit {
        peak_accel_g: None,
        monitors: vec![v_monitor(100.0), i_monitor(40.0)],
    };
    let caps = DrivePath::HostDac.capabilities();
    let d = limit.between_pulse(
        &caps,
        &[
            MeasuredPeak {
                channel: "amp_v_drop".into(),
                peak_eu: 110.0, // scale 100/110 ~ 0.909
            },
            MeasuredPeak {
                channel: "amp_i_drop".into(),
                peak_eu: 50.0, // scale 40/50 = 0.8 (tighter)
            },
        ],
    );
    assert!(
        (d.scale_factor - 0.8).abs() < 1e-12,
        "the tighter current limit must win: got {}",
        d.scale_factor
    );
    assert_eq!(d.actions.len(), 2);
    assert!(d
        .actions
        .iter()
        .all(|a| a.enforcement == MonitorEnforcement::Enforced));
}

/// Measurements at/under the ceiling do not scale.
#[test]
fn between_pulse_under_ceiling_is_noop() {
    let limit = DriveLimit {
        peak_accel_g: None,
        monitors: vec![v_monitor(100.0)],
    };
    let caps = DrivePath::HostDac.capabilities();
    let d = limit.between_pulse(
        &caps,
        &[MeasuredPeak {
            channel: "amp_v_drop".into(),
            peak_eu: 99.0,
        }],
    );
    assert_eq!(d.scale_factor, 1.0);
    assert_eq!(d.actions[0].enforcement, MonitorEnforcement::Enforced);
}

/// A monitor with no measured peak reports NoMeasurement and does not scale.
#[test]
fn between_pulse_missing_measurement_reported() {
    let limit = DriveLimit {
        peak_accel_g: None,
        monitors: vec![v_monitor(100.0)],
    };
    let caps = DrivePath::HostDac.capabilities();
    let d = limit.between_pulse(&caps, &[]);
    assert_eq!(d.scale_factor, 1.0);
    assert_eq!(d.actions[0].enforcement, MonitorEnforcement::NoMeasurement);
}

/// Predictive-kind monitors are NOT evaluated between pulses (they are pre-fire).
#[test]
fn between_pulse_skips_predictive_monitors() {
    let limit = DriveLimit {
        peak_accel_g: None,
        monitors: vec![DriveMonitor {
            channel: "resp_accel".into(),
            eu: "g".into(),
            peak_ceiling: 10.0,
            kind: DriveMonitorKind::Predictive,
        }],
    };
    let caps = DrivePath::HostDac.capabilities();
    let d = limit.between_pulse(
        &caps,
        &[MeasuredPeak {
            channel: "resp_accel".into(),
            peak_eu: 100.0,
        }],
    );
    assert_eq!(d.scale_factor, 1.0);
    assert!(
        d.actions.is_empty(),
        "predictive monitors are pre-fire only"
    );
}

// ---------------------------------------------------------------------------
// Path A: V/I unobservable -> inert (gated by PathCapabilities)
// ---------------------------------------------------------------------------

/// On the delegated path (`vi_limiting_observable == false`), V and I monitors
/// are INERT: no scale-down even with a wildly over-ceiling "measurement", and the
/// action says why. An accel-EU between-pulse monitor on the same path is still
/// enforced (pulse capture exists on Path A).
#[test]
fn path_a_vi_monitors_are_inert_but_accel_still_enforced() {
    let limit = DriveLimit {
        peak_accel_g: None,
        monitors: vec![v_monitor(100.0), i_monitor(40.0), accel_monitor(20.0)],
    };
    let caps = DrivePath::Delegated.capabilities();
    assert!(!caps.vi_limiting_observable, "Path A V/I unobservable");
    let d = limit.between_pulse(
        &caps,
        &[
            MeasuredPeak {
                channel: "amp_v_drop".into(),
                peak_eu: 1000.0,
            },
            MeasuredPeak {
                channel: "amp_i_drop".into(),
                peak_eu: 1000.0,
            },
            MeasuredPeak {
                channel: "table_accel".into(),
                peak_eu: 25.0, // over the 20 g ceiling -> scale 0.8
            },
        ],
    );
    let by_channel = |ch: &str| d.actions.iter().find(|a| a.channel == ch).unwrap();
    assert_eq!(
        by_channel("amp_v_drop").enforcement,
        MonitorEnforcement::InertPathA
    );
    assert_eq!(
        by_channel("amp_i_drop").enforcement,
        MonitorEnforcement::InertPathA
    );
    assert_eq!(by_channel("amp_v_drop").scale_factor, 1.0);
    assert_eq!(by_channel("amp_i_drop").scale_factor, 1.0);
    assert_eq!(
        by_channel("table_accel").enforcement,
        MonitorEnforcement::Enforced
    );
    assert!(
        (d.scale_factor - 0.8).abs() < 1e-12,
        "only the accel monitor may scale on Path A: got {}",
        d.scale_factor
    );
}

/// On Path B the same V/I monitors enforce normally (the capability boolean is the
/// one source of truth, not the monitor type).
#[test]
fn path_b_same_monitors_enforce() {
    let limit = DriveLimit {
        peak_accel_g: None,
        monitors: vec![v_monitor(100.0)],
    };
    let d = limit.between_pulse(
        &DrivePath::HostDac.capabilities(),
        &[MeasuredPeak {
            channel: "amp_v_drop".into(),
            peak_eu: 200.0,
        }],
    );
    assert!((d.scale_factor - 0.5).abs() < 1e-12);
}

// ---------------------------------------------------------------------------
// Config validation + EU classification
// ---------------------------------------------------------------------------

#[test]
fn validation_rejects_bad_config() {
    assert!(DriveLimit {
        peak_accel_g: Some(0.0),
        monitors: vec![]
    }
    .validate()
    .is_err());
    assert!(DriveLimit {
        peak_accel_g: Some(f64::NAN),
        monitors: vec![]
    }
    .validate()
    .is_err());
    assert!(DriveLimit {
        peak_accel_g: None,
        monitors: vec![v_monitor(-5.0)]
    }
    .validate()
    .is_err());
    assert!(DriveLimit {
        peak_accel_g: None,
        monitors: vec![DriveMonitor {
            channel: "  ".into(),
            eu: "V".into(),
            peak_ceiling: 10.0,
            kind: DriveMonitorKind::BetweenPulse,
        }]
    }
    .validate()
    .is_err());
    assert!(DriveLimit {
        peak_accel_g: Some(20.0),
        monitors: vec![v_monitor(100.0), i_monitor(40.0)]
    }
    .validate()
    .is_ok());
}

#[test]
fn vi_classification_from_eu_string() {
    assert!(v_monitor(1.0).is_vi());
    assert!(i_monitor(1.0).is_vi());
    let volts = DriveMonitor {
        channel: "c".into(),
        eu: "Volts".into(),
        peak_ceiling: 1.0,
        kind: DriveMonitorKind::BetweenPulse,
    };
    assert!(volts.is_vi());
    assert!(!accel_monitor(1.0).is_vi());
}

#[test]
fn predict_peak_accel_degenerate_inputs_are_zero() {
    assert_eq!(predict_peak_accel_g(&[], 100.0, &[0.0], &[1.0]), 0.0);
    assert_eq!(predict_peak_accel_g(&[1.0], 0.0, &[0.0], &[1.0]), 0.0);
    assert_eq!(predict_peak_accel_g(&[1.0], 100.0, &[], &[]), 0.0);
    assert_eq!(
        predict_peak_accel_g(&[1.0], 100.0, &[0.0, 1.0], &[1.0]),
        0.0
    );
}
