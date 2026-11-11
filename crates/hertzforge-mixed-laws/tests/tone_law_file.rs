//! A tone law FILE in the Mixed role: the built-in's own law file, imported,
//! is built for every tone (its identification grid starts above 0 Hz, as the
//! Sine core hands its own) and passes all nine cases of the mixed check with
//! the role's start values, which reach it because it declares the same
//! parameters. Started from Sine's own defaults instead, it fails cases: a law
//! tuned for Sine alone is not tuned for a tone read through a narrow tracker.

use hertzforge_hal_sim::SimConfig;
use hertzforge_mixed::{LawPick, MixedLawChoice};
use hertzforge_mixed_laws::{
    mixed_check_station, resolve_mixed, run_mixed_check_case, MixedCheckCase,
};
use hertzforge_random_laws::{LawCatalog, LawWorld};
use std::path::PathBuf;

fn law_file(name: &str) -> PathBuf {
    let file = format!("{name}.wasm");
    let exe = std::env::current_exe().expect("the test binary has a path");
    for dir in exe.ancestors().skip(1).take(4) {
        let p = dir.join("wasm32-wasip2").join("release").join(&file);
        if p.is_file() {
            return p;
        }
    }
    panic!("the law file {file} is not built. Run `bash scripts/build_laws.sh`.");
}

fn station(case: MixedCheckCase) -> Box<dyn hertzforge_hal::Backend> {
    let st = mixed_check_station(case);
    Box::new(
        SimConfig::new()
            .sample_rate_hz(st.fs_hz)
            .resonances(st.modes)
            .feedthrough(st.feedthrough)
            .noise_level(st.noise_rms_g)
            .transport_delay_samples(st.transport_delay_samples)
            .shared_clock(true)
            .seed(st.seed)
            .build(),
    )
}

/// Every case with the law file in the tone role, `params` its pick; the
/// names of the cases that fail, and the first failure's words.
fn run_all(
    tag: &str,
    params: serde_json::Map<String, serde_json::Value>,
) -> (Vec<&'static str>, String) {
    let root =
        std::env::temp_dir().join(format!("hf-mixed-tone-file-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut cat = LawCatalog::open(&root).unwrap();
    std::fs::create_dir_all(cat.laws_dir()).unwrap();
    let bytes = std::fs::read(law_file("hertzforge_law_sine_servo")).unwrap();
    let sha = hertzforge_law_host::law_file_sha256(&bytes);
    std::fs::write(cat.laws_dir().join(format!("{sha}.wasm")), &bytes).unwrap();
    cat.rescan().unwrap();
    let id = cat
        .entries_for(LawWorld::Sine)
        .into_iter()
        .find(|e| !e.builtin)
        .unwrap_or_else(|| panic!("the law file is not in the catalog: {:?}", cat.problems()))
        .id;
    let choice = MixedLawChoice {
        tones: LawPick { id, params },
        ..MixedLawChoice::default()
    };
    let laws = resolve_mixed(&cat, &choice, 1).unwrap();
    let (mut failed, mut first) = (Vec::new(), String::new());
    for case in MixedCheckCase::ALL {
        let r = run_mixed_check_case(case, &laws, station(case)).unwrap();
        if !r.passed {
            if first.is_empty() {
                first = format!("{}: {}", r.name, r.detail);
            }
            failed.push(case.name());
        }
    }
    let _ = std::fs::remove_dir_all(&root);
    (failed, first)
}

#[test]
fn a_tone_law_file_passes_the_mixed_check_from_the_role_s_start_values() {
    let (failed, first) = run_all("role", serde_json::Map::new());
    eprintln!(
        "law file, role start values: {} of 9 fail {failed:?} {first}",
        failed.len()
    );
    assert!(failed.is_empty(), "{failed:?}: {first}");
}

#[test]
fn a_tone_law_file_started_from_sine_s_own_defaults_fails_cases() {
    let mut p = serde_json::Map::new();
    p.insert("compression".into(), "slow".into());
    let (failed, first) = run_all("sine", p);
    eprintln!(
        "law file, Sine's defaults: {} of 9 fail {failed:?}",
        failed.len()
    );
    assert!(
        !failed.is_empty() && !first.contains("positive and ascending"),
        "{first}"
    );
}
