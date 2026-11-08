//! Records (design 11, Q38): a run writes its raw stream, its identification set,
//! per-loop diagonals and summaries, spectral-matrix snapshots, the law record and
//! the report, and everything reads back as written. An identification set loads
//! into a later run on the same setup, and is refused on another.
mod common;

use common::*;
use hertzforge_hal_sim::plants::StandardPlant;
use hertzforge_mimo_random::records::{
    read_identification, read_snapshot, DiagonalSidecar, LoopSummary, IDENTIFICATION,
    LOOPS_DIAGONAL, LOOPS_SUMMARY,
};
use hertzforge_mimo_random::{LawLine, MimoError, MimoReport, MimoState};
use std::path::{Path, PathBuf};

fn run_dir(root: &Path) -> PathBuf {
    std::fs::read_dir(root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with("mimo_random_"))
                .unwrap_or(false)
        })
        .expect("a run folder")
}

#[test]
fn a_run_records_everything_and_it_reads_back_as_written() {
    let grid = Grid::standard();
    let p = StandardPlant::Modal4x4.build();
    let (mut cfg, table) = physical_config(&p, &grid, 0.5);
    cfg.records.enabled = true;
    cfg.records.snapshot_every_s = 20.0;
    cfg.full_level_duration_s = Some(40.0);
    let mut svc = service(cfg.clone(), table.clone(), &p, &grid, "records-run");
    let root = svc.data_root().to_path_buf();
    assert_eq!(svc.run_auto(40).unwrap(), MimoState::Stopped);
    let dir = run_dir(&root);
    let size = |name: &str| {
        std::fs::metadata(dir.join(name))
            .map(|m| m.len())
            .unwrap_or(0)
    };

    // The raw stream.
    assert!(size("telemetry.f32") > 0);
    assert!(size("capture.json") > 0);

    // The identification set, bit for bit.
    let set = read_identification(&dir).unwrap();
    let mine = svc.identification().unwrap();
    // The condition numbers are NaN outside the band; compare them bit for bit.
    let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&set.condition), bits(&mine.condition));
    let mut same = set.clone();
    same.condition = mine.condition.clone();
    let mut mine_c = mine.clone();
    mine_c.condition = mine.condition.clone();
    assert!(same
        .condition
        .iter()
        .zip(&mine_c.condition)
        .all(|(a, b)| a.to_bits() == b.to_bits()));
    same.condition.clear();
    mine_c.condition.clear();
    assert_eq!(same, mine_c);
    assert!(size(&format!("{IDENTIFICATION}.bin")) > 0);

    // Per-loop diagonals: one chunk per judged loop, of the documented size.
    let side: DiagonalSidecar =
        serde_json::from_slice(&std::fs::read(dir.join(format!("{LOOPS_DIAGONAL}.json"))).unwrap())
            .unwrap();
    assert!(side.loops >= 3, "{side:?}");
    assert_eq!(side.loop_index.len() as u64, side.loops);
    let per = side.lines * (2 * side.controls + side.drives) * 8;
    assert_eq!(
        size(&format!("{LOOPS_DIAGONAL}.bin")),
        side.loops * per as u64
    );

    // Per-loop summaries, one line each, readable.
    let text = std::fs::read_to_string(dir.join(LOOPS_SUMMARY)).unwrap();
    let rows: Vec<LoopSummary> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(rows.len() as u64 >= side.loops);
    assert!(rows.iter().any(|r| r.state == MimoState::FullLevel));

    // Snapshots: the first loop at full level, the cadence, and the last loop at
    // full level, which equals the run's final state exactly.
    let snaps: Vec<u64> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.strip_prefix("sdm_")
                .and_then(|s| s.strip_suffix(".bin"))
                .and_then(|s| s.parse().ok())
        })
        .collect();
    assert!(snaps.len() >= 2, "{snaps:?}");
    let last = *snaps.iter().max().unwrap();
    let (resp, drive) = read_snapshot(&dir, last).unwrap();
    assert_eq!(&resp, svc.response_full_level().unwrap());
    assert_eq!(&drive, svc.drive_full_level());

    // The law record and the report.
    let law: LawLine =
        serde_json::from_slice(&std::fs::read(dir.join("law.json")).unwrap()).unwrap();
    assert_eq!(law.id, "pinv_closed");
    let report: MimoReport =
        serde_json::from_slice(&std::fs::read(dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(report.final_state, MimoState::Stopped);
    assert_eq!(report.identification.unwrap().setup_hash, set.setup_hash);

    // The set loads into a later run on the same setup, which skips straight to
    // the gate and passes it the same way.
    let gate = svc.gate().unwrap().clone();
    let mut again = service(cfg.clone(), table.clone(), &p, &grid, "records-load");
    again.start().unwrap();
    again.load_identification(set.clone()).unwrap();
    assert_eq!(again.state(), MimoState::Gate);
    assert_eq!(again.gate().unwrap(), &gate);
    assert!(again.report().identification.unwrap().loaded);
    again.proceed().unwrap();
    assert_eq!(again.state(), MimoState::Predict);

    // Another setup (one control's sensitivity changed) refuses it, in words.
    let mut other_table = table;
    other_table.channels[1].sensitivity_mv_per_eu *= 2.0;
    let mut other = service(cfg, other_table, &p, &grid, "records-other");
    other.start().unwrap();
    match other.load_identification(set) {
        Err(MimoError::Blocked(m)) => assert!(m.contains("another setup"), "{m}"),
        r => panic!("expected a refusal, got {r:?}"),
    }
    assert_eq!(other.state(), MimoState::NoiseFloor);
}
