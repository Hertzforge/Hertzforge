//! Tests of the reference object, its validation, its repair and the native file.

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) fn c(re: f64, im: f64) -> C64 {
    C64::new(re, im)
}

/// A fresh empty folder under the system temporary directory.
pub(crate) fn scratch(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "hertzforge-reference-{}-{}-{name}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub(crate) fn labels(n: usize) -> Vec<String> {
    (1..=n).map(|k| format!("c{k}")).collect()
}

/// A valid 3 by 3 reference on 16 lines at 2 Hz, band [4, 12), PSDs 1, 2, 3 with
/// pair coherence 0.25 and a 30 degree phase on (0, 1).
pub(crate) fn parts3() -> ReferenceParts {
    let (lines, n) = (16, 3);
    let mut sdm = HermitianStack::zeros(lines, n);
    for line in 4..12 {
        for i in 0..n {
            sdm.set(line, i, i, c(1.0 + i as f64, 0.0));
        }
        for i in 0..n {
            for j in i + 1..n {
                let mag = 0.5 * ((1.0 + i as f64) * (1.0 + j as f64)).sqrt();
                let ph = if (i, j) == (0, 1) {
                    30f64.to_radians()
                } else {
                    0.0
                };
                sdm.set(line, i, j, C64::from_polar(mag, ph));
            }
        }
    }
    let mut tolerances = Tolerances::none(lines, n);
    for line in 4..12 {
        for i in 0..n {
            let g = 1.0 + i as f64;
            let k = line * n + i;
            tolerances.alarm_upper[k] = g * 2.0;
            tolerances.alarm_lower[k] = g * 0.5;
            tolerances.abort_upper[k] = g * 4.0;
            tolerances.abort_lower[k] = g * 0.25;
        }
    }
    ReferenceParts {
        sdm,
        df_hz: 2.0,
        band: (4, 12),
        labels: labels(n),
        unit: "g".into(),
        tolerances,
        response_transformation: None,
        drive_transformation: None,
        provenance: Provenance::new(Method::Direct, "test"),
    }
}

fn build(p: ReferenceParts) -> Result<Reference, ReferenceError> {
    Reference::from_parts(p, None).map(|(r, _)| r)
}

#[test]
fn a_consistent_reference_validates() {
    let r = build(parts3()).unwrap();
    let rep = r.validate().unwrap();
    assert_eq!(rep.lines_in_band, 8);
    assert!(rep.min_relative_eigenvalue > -1e-12);
    assert!(!r.is_virtual());
    assert!(r.content_hash().starts_with("sha256:"));
    assert_eq!(r.content_hash().len(), 7 + 64);
}

#[test]
fn a_non_hermitian_full_matrix_is_refused_with_its_line() {
    let r = build(parts3()).unwrap();
    let mut full = r.sdm.to_full();
    full.set(7, 2, 0, c(9.0, 0.0));
    match Reference::sdm_from_full(&full) {
        Err(ReferenceError::NotHermitian { line, .. }) => assert_eq!(line, 7),
        other => panic!("{other:?}"),
    }
    let mut nan = r.sdm.to_full();
    nan.set(5, 0, 0, c(f64::NAN, 0.0));
    assert!(matches!(
        Reference::sdm_from_full(&nan),
        Err(ReferenceError::NonFinite { line: 5, .. })
    ));
    assert!(Reference::sdm_from_full(&r.sdm.to_full()).is_ok());
}

/// Unit pairwise coherence on three channels with phases that do not add up:
/// theta_01 + theta_12 != theta_02.
fn inconsistent_parts(gamma2: f64) -> ReferenceParts {
    let mut p = parts3();
    let g = gamma2.sqrt();
    for line in 4..12 {
        let d: Vec<f64> = p.sdm.diagonal(line);
        p.sdm
            .set(line, 0, 1, C64::from_polar(g * (d[0] * d[1]).sqrt(), 0.0));
        p.sdm
            .set(line, 1, 2, C64::from_polar(g * (d[1] * d[2]).sqrt(), 0.0));
        p.sdm.set(
            line,
            0,
            2,
            C64::from_polar(g * (d[0] * d[2]).sqrt(), std::f64::consts::PI),
        );
    }
    p
}

#[test]
fn an_indefinite_reference_names_its_lines_and_the_repair_reports_coherence() {
    let err = build(inconsistent_parts(0.9)).unwrap_err();
    match &err {
        ReferenceError::NotPositiveSemiDefinite { violations } => {
            assert_eq!(violations.len(), 8);
            assert_eq!(violations[0].line, 4);
            assert!(violations[0].smallest_eigenvalue < 0.0);
        }
        other => panic!("{other:?}"),
    }
    assert!(err.to_string().contains("accept the repair"), "{err}");

    // The standard policy refuses a real inconsistency rather than hiding it.
    let refused = Reference::from_parts(inconsistent_parts(0.9), Some(RepairPolicy::STANDARD));
    assert!(
        matches!(refused, Err(ReferenceError::RepairRefused { line: 4, .. })),
        "{refused:?}"
    );

    let (r, rep) =
        Reference::from_parts(inconsistent_parts(0.9), Some(RepairPolicy::any_depth())).unwrap();
    let rep = rep.unwrap();
    assert_eq!(rep.lines.len(), 8);
    assert_eq!(rep.summary.lines_repaired, 8);
    // The PSDs are exactly what was entered.
    for line in 4..12 {
        for i in 0..3 {
            assert!((r.sdm.get(line, i, i).re - (1.0 + i as f64)).abs() < 1e-12);
        }
    }
    // Every pair's coherence was 0.9 before and dropped after.
    let l = &rep.lines[0];
    assert_eq!(l.pairs.len(), 3);
    for p in &l.pairs {
        assert!((p.before - 0.9).abs() < 1e-12, "{p:?}");
        assert!(p.after < p.before, "{p:?}");
    }
    assert!(rep.summary.max_coherence_change > 0.05);
    assert_eq!(r.provenance.repair.as_ref(), Some(&rep.summary));
    assert!(rep.to_string().contains("8 line(s) repaired"));
}

#[test]
fn zero_diagonal_in_band_and_energy_out_of_band_are_refused() {
    let mut p = parts3();
    p.sdm.set(9, 1, 1, c(0.0, 0.0));
    p.sdm.set(9, 0, 1, c(0.0, 0.0));
    p.sdm.set(9, 1, 2, c(0.0, 0.0));
    match build(p) {
        Err(ReferenceError::ZeroDiagonalInBand { line, channel, .. }) => {
            assert_eq!((line, channel.as_str()), (9, "c2"));
        }
        other => panic!("{other:?}"),
    }
    let mut p = parts3();
    p.sdm.set(13, 0, 2, c(0.0, 1e-9));
    assert!(matches!(
        build(p),
        Err(ReferenceError::NonzeroOutOfBand { line: 13, .. })
    ));
}

#[test]
fn inconsistent_tolerances_are_refused() {
    let k = 6 * 3 + 1;
    let mut p = parts3();
    p.tolerances.alarm_lower[k] = p.tolerances.alarm_upper[k] * 1.5;
    let e = build(p).unwrap_err();
    assert!(e.to_string().contains("alarm lower"), "{e}");

    let mut p = parts3();
    p.tolerances.abort_upper[k] = p.tolerances.alarm_upper[k] * 0.9;
    let e = build(p).unwrap_err();
    assert!(e.to_string().contains("abort upper"), "{e}");

    let mut p = parts3();
    p.tolerances.alarm_upper[k] = 1.0; // the reference there is 2.0
    p.tolerances.abort_upper[k] = 1.5;
    let e = build(p).unwrap_err();
    assert!(e.to_string().contains("above its own alarm upper"), "{e}");

    let mut p = parts3();
    p.tolerances.alarm_upper[k] = f64::INFINITY;
    assert!(matches!(build(p), Err(ReferenceError::Tolerance(_))));
}

#[test]
fn labels_must_be_unique_and_present() {
    let mut p = parts3();
    p.labels[2] = "c1".into();
    assert!(matches!(build(p), Err(ReferenceError::Labels(_))));
    let mut p = parts3();
    p.labels.pop();
    assert!(matches!(build(p), Err(ReferenceError::Labels(_))));
}

#[test]
fn transformations_need_full_rank() {
    let base = build(parts3()).unwrap();
    // Three virtual rows over four controls, the third the sum of the first two.
    let rows = labels(3);
    let cols: Vec<String> = (1..=4).map(|k| format!("a{k}")).collect();
    #[rustfmt::skip]
    let dependent = [
        1.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        1.0, 1.0, 0.0, 0.0,
    ];
    let t = Transformation::real(3, 4, &dependent, rows.clone(), cols.clone()).unwrap();
    match base.clone().with_response_transformation(t) {
        Err(ReferenceError::TransformationRank {
            which,
            rank,
            needed,
            ..
        }) => assert_eq!((which, rank, needed), ("response", 2, 3)),
        other => panic!("{other:?}"),
    }
    #[rustfmt::skip]
    let good = [
        0.5, 0.5, 0.0, 0.0,
        0.0, 0.0, 0.5, 0.5,
        0.5, -0.5, 0.5, -0.5,
    ];
    let t = Transformation::real(3, 4, &good, rows.clone(), cols).unwrap();
    let v = base.clone().with_response_transformation(t).unwrap();
    assert!(v.is_virtual());
    assert!(v.validate().unwrap().response_condition.unwrap() < 10.0);

    // More virtual rows than controls.
    let t = Transformation::real(3, 2, &[1.0; 6], rows, vec!["a".into(), "b".into()]).unwrap();
    assert!(matches!(
        base.clone().with_response_transformation(t),
        Err(ReferenceError::Transformation(_))
    ));

    // A drive transformation: two virtual drives ganged onto four outputs, and one
    // whose columns are equal.
    let outs: Vec<String> = (1..=4).map(|k| format!("d{k}")).collect();
    let virt = vec!["u1".to_string(), "u2".to_string()];
    let gang = [1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0];
    let t = Transformation::real(4, 2, &gang, outs.clone(), virt.clone()).unwrap();
    assert!(base.clone().with_drive_transformation(t).is_ok());
    let same = [1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0];
    let t = Transformation::real(4, 2, &same, outs, virt).unwrap();
    assert!(matches!(
        base.with_drive_transformation(t),
        Err(ReferenceError::TransformationRank {
            which: "drive",
            rank: 1,
            needed: 2,
            ..
        })
    ));
}

fn same_reference(a: &Reference, b: &Reference) {
    // NaN tolerances make PartialEq false; compare the encodings bit for bit.
    assert_eq!(native::encode(a), native::encode(b));
    assert_eq!(a.provenance, b.provenance);
}

#[test]
fn the_native_file_round_trips_and_carries_the_content_hash() {
    let mut p = parts3();
    let cols: Vec<String> = (1..=4).map(|k| format!("a{k}")).collect();
    #[rustfmt::skip]
    let tr = [
        0.5, 0.5, 0.0, 0.0,
        0.0, 0.0, 0.5, 0.5,
        0.5, -0.5, 0.5, -0.5,
    ];
    p.response_transformation = Some(Transformation::real(3, 4, &tr, labels(3), cols).unwrap());
    p.drive_transformation = Some(
        Transformation::real(
            2,
            2,
            &[1.0, 1.0, 1.0, -1.0],
            vec!["d1".into(), "d2".into()],
            vec!["sum".into(), "difference".into()],
        )
        .unwrap(),
    );
    p.provenance.assumptions.push("entered by hand".into());
    p.unit = "m/s^2".into();
    let r = build(p).unwrap();
    let dir = scratch("roundtrip");
    let hash = native::write(&r, &dir).unwrap();
    assert_eq!(hash, r.content_hash());
    let back = native::read(&dir).unwrap();
    same_reference(&r, &back);
    assert_eq!(back.content_hash(), hash);

    // The hash ignores provenance: the same content made another way hashes the same.
    let mut other = r.clone();
    other.provenance = Provenance::new(Method::Csv, "elsewhere");
    assert_eq!(other.content_hash(), hash);

    // The sidecar is a capture snapshot envelope with the reference payload.
    let side: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(native::SIDECAR_FILE)).unwrap())
            .unwrap();
    assert_eq!(side["payload"]["format"], native::FORMAT);
    assert_eq!(side["payload"]["content_hash"], hash.as_str());
    assert_eq!(side["payload"]["space"], "virtual");
    assert!(side["marks"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_changed_binary_or_sidecar_is_refused() {
    let r = build(parts3()).unwrap();
    let dir = scratch("tamper");
    native::write(&r, &dir).unwrap();
    let bin = dir.join(native::BINARY_FILE);
    let mut bytes = std::fs::read(&bin).unwrap();
    let last = bytes.len() - 3;
    bytes[last] ^= 0x40;
    std::fs::write(&bin, &bytes).unwrap();
    let e = native::read(&dir).unwrap_err();
    assert!(e.to_string().contains("content hash"), "{e}");

    native::write(&r, &dir).unwrap();
    let side_path = dir.join(native::SIDECAR_FILE);
    let text = std::fs::read_to_string(&side_path).unwrap();
    std::fs::write(&side_path, text.replace("\"c3\"", "\"c9\"")).unwrap();
    let e = native::read(&dir).unwrap_err();
    assert!(e.to_string().contains("does not describe"), "{e}");

    // Truncation is caught by the decoder even with a matching hash.
    let short = &native::encode(&r)[..200];
    assert!(native::decode(short, r.provenance.clone()).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn band_of_finds_the_nonzero_diagonal_range() {
    let r = build(parts3()).unwrap();
    assert_eq!(band_of(&r.sdm), Some((4, 12)));
    assert_eq!(band_of(&HermitianStack::zeros(4, 2)), None);
}
