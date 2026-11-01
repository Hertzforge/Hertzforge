//! Path-honesty seam tests.

use crate::path::{DrivePath, PathCapabilities, ReferenceError, ReferenceForPath};
use eigen_dsp::shock::pulse;

#[test]
fn delegated_path_greys_all_host_features_f1_f7_f12() {
    let caps = PathCapabilities::for_path(DrivePath::Delegated);
    // Everything the host computes is unavailable on the delegated path.
    assert!(!caps.host_compensation, "device compensates on-device");
    assert!(!caps.shaped_sysid, "device owns sysID at arm");
    assert!(!caps.pulse_trains_at_rate, "at-rate is HostDac-only");
    assert!(!caps.vi_limiting_observable, "V/I unobservable on Path A");
    assert!(!caps.eigen_control_law, "device closes the loop");
}

#[test]
fn host_path_enables_all_features() {
    let caps = PathCapabilities::for_path(DrivePath::HostDac);
    assert!(caps.host_compensation);
    assert!(caps.shaped_sysid);
    assert!(caps.pulse_trains_at_rate);
    assert!(caps.vi_limiting_observable);
    assert!(caps.eigen_control_law);
}

#[test]
fn capabilities_come_from_one_source() {
    // DrivePath::capabilities() and PathCapabilities::for_path() must agree - there is
    // exactly one table.
    for p in [DrivePath::Delegated, DrivePath::HostDac] {
        assert_eq!(p.capabilities(), PathCapabilities::for_path(p));
    }
    assert!(DrivePath::Delegated.is_delegated());
    assert!(!DrivePath::Delegated.is_host());
    assert!(DrivePath::HostDac.is_host());
}

#[test]
fn hard_invariant_delegated_reference_is_uncompensated_plain_pulse_f1() {
    // Fixture: a delegated-path reference download MUST be the UNCOMPENSATED,
    // one-signed pulse. A plain nonnegative half-sine is accepted.
    let half_sine = pulse::half_sine(20.0, 11.0, 5120.0);
    assert!(!half_sine.is_empty());
    let reference =
        ReferenceForPath::delegated(&half_sine).expect("plain half-sine is legal on Path A");
    assert_eq!(reference.path(), DrivePath::Delegated);
    // The stored waveform is the plain pulse (nonnegative).
    let most_neg = reference
        .waveform_g()
        .iter()
        .fold(0.0_f64, |m, &v| m.min(v));
    assert!(
        most_neg >= -0.4,
        "plain half-sine stays nonnegative (bar float noise)"
    );
    match &reference {
        ReferenceForPath::Delegated(u) => assert_eq!(u.pulse_g(), half_sine.as_slice()),
        _ => panic!("expected a delegated reference"),
    }
}

#[test]
fn hard_invariant_compensated_waveform_rejected_on_delegated_path_f1() {
    // A COMPENSATED pulse (negative shoulders) handed to the delegated constructor is
    // the double-compensation bug the seam prevents: it must be rejected.
    let half_sine = pulse::half_sine(20.0, 11.0, 5120.0);
    // Build a crude compensated waveform: a -24% trough shoulder before the pulse.
    let mut compensated = vec![-4.8f64; 10]; // -24% of 20 g, the decoded OEM trough
    compensated.extend_from_slice(&half_sine);
    let err = ReferenceForPath::delegated(&compensated)
        .expect_err("compensated waveform must be rejected on Path A");
    match err {
        ReferenceError::CompensatedOnDelegatedPath {
            most_negative_g,
            peak_g,
        } => {
            assert!(most_negative_g < 0.0);
            assert!(peak_g > 0.0);
        }
        other => panic!("expected CompensatedOnDelegatedPath, got {other:?}"),
    }
    // The same compensated waveform is fine on the HOST path (it may be compensated).
    let host_ref = ReferenceForPath::host(&compensated).expect("host reference may be compensated");
    assert_eq!(host_ref.path(), DrivePath::HostDac);
}

#[test]
fn empty_reference_rejected_on_both_paths() {
    assert_eq!(
        ReferenceForPath::delegated(&[]).unwrap_err(),
        ReferenceError::EmptyReference
    );
    assert_eq!(
        ReferenceForPath::host(&[]).unwrap_err(),
        ReferenceError::EmptyReference
    );
    // All-zero also rejected on the delegated path (no pulse).
    assert_eq!(
        ReferenceForPath::delegated(&[0.0, 0.0, 0.0]).unwrap_err(),
        ReferenceError::EmptyReference
    );
}

#[test]
fn capabilities_and_path_round_trip_through_json() {
    let p = DrivePath::HostDac;
    let js = serde_json::to_string(&p).unwrap();
    let back: DrivePath = serde_json::from_str(&js).unwrap();
    assert_eq!(p, back);

    let reference = ReferenceForPath::host(&[1.0, 2.0, 1.0]).unwrap();
    let js = serde_json::to_string(&reference).unwrap();
    let back: ReferenceForPath = serde_json::from_str(&js).unwrap();
    assert_eq!(reference, back);
}
