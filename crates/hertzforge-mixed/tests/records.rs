//! Capture, the law records with a tone index, a snapshot and the report's
//! line per part, on the lagged resonant plant of `common`.

mod common;
use common::*;
use hertzforge_mixed::{service::law_records, PartKind};

#[test]
fn a_capture_writes_the_response_and_every_law_call_with_its_tone() {
    let root =
        std::env::temp_dir().join(format!("hertzforge-mixed-records-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut c = config(vec![
        floor(0.01),
        tone("t1", fixed(250.0), 2.0),
        tone("t2", fixed(400.0), 1.0),
    ]);
    c.soft_start_db = 0.0;
    c.floor_settle_s = 3.0;
    let mut s = hertzforge_mixed::MixedService::new(
        c,
        table(),
        station(DELAY),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        1,
        Some(root.clone()),
    )
    .unwrap()
    .with_block_frames(BLOCK);
    s.arm().unwrap();
    s.run_sysid(0).unwrap();
    assert!(s.coherence_gate().unwrap());
    s.proceed_after_review().unwrap();
    let dir = s.start_capture("run1").unwrap();
    assert!(s.live_status().capturing);
    run_for(&mut s, 14.0);
    let snap = s.save_snapshot(Vec::new()).unwrap();
    s.stop_capture().unwrap();
    s.stop().unwrap();
    assert!(snap.exists());
    let law: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(law_records::LAW_JSON_FILE)).unwrap())
            .unwrap();
    assert_eq!(law["tones"], serde_json::json!(["t1", "t2"]));
    let bin = std::fs::read(dir.join(law_records::LAW_TICKS_FILE)).unwrap();
    assert!(!bin.is_empty() && bin.len().is_multiple_of(law_records::MixedLawRecord::BYTES));
    let (mut floor, mut tone) = ([0usize; 1], [0usize; 2]);
    for r in bin.chunks(law_records::MixedLawRecord::BYTES) {
        match (r[28], r[29]) {
            (0, 0) => floor[0] += 1,
            (1, t) => tone[t as usize] += 1,
            other => panic!("{other:?}"),
        }
    }
    eprintln!("records: floor {} t1 {} t2 {}", floor[0], tone[0], tone[1]);
    assert!(floor[0] >= 5 && tone[0] > 100 && tone[1] > 100);
    assert!(std::fs::read_dir(&dir).unwrap().count() >= 3);
    let r = s.report_data();
    assert_eq!(r.parts.len(), 3);
    let t1 = r.parts.iter().find(|p| p.id == "t1").unwrap();
    assert_eq!(t1.kind, PartKind::Tone);
    assert!(t1.worst_error_db.is_some() && t1.measured.is_some());
    let _ = std::fs::remove_dir_all(&root);
}
