//! Replay (acceptance A11): a run recorded on the simulator replays. Every law
//! call's recorded inputs, fed through the recorded law, reproduce the recorded
//! outputs within 1e-12; a law changed by 1% in one parameter does not. The law
//! file case runs through the law host when the guest law is built.
mod common;

use common::*;
use hertzforge_hal_sim::plants::StandardPlant;
use hertzforge_law_host::{Param, ParamValue};
use hertzforge_mimo_random::records::{
    param_from_json, read_law_calls, read_law_record, read_reference, LAW_CALLS,
};
use hertzforge_mimo_random::replay::{replay, ReplayOptions, REPLAY_TOLERANCE};
use hertzforge_mimo_random::{FrfMode, MimoState};
use std::path::{Path, PathBuf};

fn run_dir(root: &Path) -> PathBuf {
    std::fs::read_dir(root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.join(format!("{LAW_CALLS}.jsonl")).is_file())
        .expect("a run folder with a law-input record")
}

/// A recorded run on the 4 by 4 plant with the FRF updated every loop (so the law
/// sees a different FRF each call) and the prediction iterated once.
fn recorded_run(tag: &str) -> PathBuf {
    let grid = Grid::standard();
    let p = StandardPlant::Modal4x4.build();
    let (mut cfg, table) = physical_config(&p, &grid, 0.5);
    cfg.records.enabled = true;
    cfg.frf_mode = FrfMode::Update { blend: 0.5 };
    cfg.full_level_duration_s = Some(30.0);
    let mut svc = service(cfg, table, &p, &grid, tag);
    let root = svc.data_root().to_path_buf();
    svc.start().unwrap();
    svc.run_noise_floor().unwrap();
    svc.run_identification().unwrap();
    svc.proceed().unwrap();
    svc.iterate_prediction().unwrap();
    assert_eq!(svc.run_auto(30).unwrap(), MimoState::Stopped);
    run_dir(&root)
}

#[test]
fn a_recorded_run_replays_through_its_law_within_1e_12() {
    let dir = recorded_run("replay");
    // The record: a seed, the iterated prediction, then one call per judged loop.
    let calls = read_law_calls(&dir).unwrap();
    assert!(calls.len() >= 4, "{} calls", calls.len());
    assert!(calls[0].seed && calls[0].loop_index == 0);
    assert!(calls.iter().skip(1).all(|c| !c.seed));
    // The FRF changed between loop calls and was recorded each time it did.
    let distinct = calls
        .windows(2)
        .filter(|w| w[0].inputs.frf != w[1].inputs.frf)
        .count();
    assert!(distinct >= 2, "the FRF changed {distinct} times");
    // The reference record agrees with what the law was given.
    let (sdm, _, band, _) = read_reference(&dir).unwrap();
    assert_eq!(sdm, calls[0].inputs.reference);
    assert_eq!(band, calls[0].inputs.band);
    let law = read_law_record(&dir).unwrap();
    assert!(law.builtin && law.id == "pinv_closed" && !law.params.is_empty());

    let r = replay(&dir, &ReplayOptions::default()).unwrap();
    assert!(r.reproduced(), "{r:?}");
    assert_eq!(r.compared + r.missed_alike, r.calls);
    assert!(r.compared >= 4);
    assert!(r.worst_relative <= REPLAY_TOLERANCE, "{r:?}");
    eprintln!(
        "replay: {} calls, {} compared, worst relative difference {:.3e}",
        r.calls, r.compared, r.worst_relative
    );

    // Falsified: the loop gain 1% off, and the replay says so. (The truncation
    // amount, the first numeric parameter, changes nothing on a well-conditioned
    // plant, so the parameter is named.)
    let mut params: Vec<Param> = law
        .params
        .iter()
        .map(|p| param_from_json(p).unwrap())
        .collect();
    let k = params
        .iter()
        .position(|p| p.key == "convergence")
        .expect("the convergence parameter");
    if let ParamValue::Number(x) = &mut params[k].value {
        *x *= 1.01;
    }
    let off = replay(
        &dir,
        &ReplayOptions {
            params_override: Some(params),
            ..ReplayOptions::default()
        },
    )
    .unwrap();
    assert!(!off.reproduced(), "a 1% change went unnoticed: {off:?}");
    assert!(off.worst_relative > 1e-6, "{off:?}");
}

/// The guest build of the default law, when it has been built
/// (`scripts/build_laws.sh`, or `HERTZFORGE_LAW_WASM_DIR`).
fn guest_pinv() -> Option<PathBuf> {
    let file = "hertzforge_law_mimo_pinv.wasm";
    if let Some(d) = std::env::var_os("HERTZFORGE_LAW_WASM_DIR") {
        let p = PathBuf::from(d).join(file);
        return p.is_file().then_some(p);
    }
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .skip(1)
        .take(4)
        .map(|d| d.join("wasm32-wasip2").join("release").join(file))
        .find(|p| p.is_file())
}

#[test]
#[ignore = "needs the guest law built (scripts/build_laws.sh or HERTZFORGE_LAW_WASM_DIR)"]
fn a_run_on_a_law_file_replays_through_the_law_host() {
    let wasm = guest_pinv().expect("the guest law is built");
    let grid = Grid::standard();
    let p = StandardPlant::Modal4x4.build();
    let (mut cfg, table) = physical_config(&p, &grid, 0.5);
    cfg.records.enabled = true;
    cfg.full_level_duration_s = Some(20.0);
    // The law file through the catalog, as the control service resolves it.
    let svc0 = service(cfg.clone(), table.clone(), &p, &grid, "replay-file");
    let root = svc0.data_root().to_path_buf();
    drop(svc0);
    let mut cat = hertzforge_random_laws::LawCatalog::open(root.join("catalog")).unwrap();
    let entry = cat.import(&wasm).unwrap();
    cfg.law = hertzforge_random_laws::LawChoice::new(entry.id.clone(), Vec::new());
    let factory = cat.resolve_mimo(&cfg.law, 4).unwrap();
    let mut svc = hertzforge_mimo_random::MimoRandomService::new(
        cfg,
        table,
        grid.loop_config(),
        backend(&p, &grid, 0x51_4D_00, 1e-4),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        &root,
    )
    .unwrap()
    .with_law_factory(factory);
    svc.set_law_file(Some(cat.laws_dir().join(format!("{}.wasm", entry.id))));
    assert_eq!(svc.run_auto(30).unwrap(), MimoState::Stopped);
    let dir = run_dir(&root);
    let law = read_law_record(&dir).unwrap();
    assert!(!law.builtin && law.file.as_deref() == Some("law_file.wasm"));
    let r = replay(&dir, &ReplayOptions::default()).unwrap();
    assert!(r.law.starts_with("law file"), "{r:?}");
    assert!(r.reproduced(), "{r:?}");
    eprintln!(
        "law-file replay: {} calls, {} compared, worst {:.3e}",
        r.calls, r.compared, r.worst_relative
    );
}
