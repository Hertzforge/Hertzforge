//! The reference the analysis service's converter wrote
//! (`testdata/mimo_reference/three_channel`, from
//! `testdata/generate_mimo_reference.py`) reads here, and this crate's encoding
//! of it is byte for byte the one the converter wrote.

use hertzforge_reference::{native, Method};
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/mimo_reference/three_channel")
}

#[test]
fn the_python_written_reference_reads_and_hashes_the_same() {
    let r = native::read(&fixture()).unwrap();
    assert_eq!(r.labels, vec!["front", "rear", "top"]);
    assert_eq!((r.lines(), r.n(), r.df_hz, r.band), (41, 3, 5.0, (4, 21)));
    assert_eq!(r.unit, "g");
    assert_eq!(r.provenance.method, Method::SpecificationMat);
    assert_eq!(r.provenance.conjugated_on_import, Some(true));

    // rear lags front by 30 degrees: +30 at (front, rear) in E[X X^H].
    let g = r.sdm.get(10, 0, 1);
    assert!((g.arg().to_degrees() - 30.0).abs() < 1e-9, "{g}");
    assert!((hertzforge_reference::coherence(&r.sdm, 10, 0, 1) - 0.5).abs() < 1e-12);
    assert!((hertzforge_reference::coherence(&r.sdm, 10, 0, 2) - 0.25).abs() < 1e-12);
    assert_eq!(r.sdm.get(10, 1, 2).norm(), 0.0);
    assert_eq!(r.sdm.get(3, 0, 0).re, 0.0);
    assert_eq!(r.sdm.get(20, 2, 2).re, 0.04);

    // Tolerances: no upper warning line for top at 20 Hz, twice its PSD after.
    assert!(r.tolerances.alarm_upper[4 * 3 + 2].is_nan());
    assert!((r.tolerances.alarm_upper[5 * 3 + 2] - 0.08).abs() < 1e-15);
    assert!((r.tolerances.abort_lower[5 * 3] - 0.0025).abs() < 1e-15);

    let side: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture().join(native::SIDECAR_FILE)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        r.content_hash(),
        side["payload"]["content_hash"].as_str().unwrap()
    );
}
