//! The law host's `shock-mimo` world: a MIMO Shock law file is recognized by its
//! world and refused by name, since this version runs none.

use hertzforge_law_host::shock_mimo::SHOCK_MIMO_NOT_RUN;
use hertzforge_law_host::{HostConfig, LawError, LawHost, SHOCK_MIMO_WORLD};
use std::path::PathBuf;

fn law_bytes(name: &str) -> Vec<u8> {
    let file = format!("{name}.wasm");
    let exe = std::env::current_exe().expect("the test binary knows its own path");
    for dir in exe.ancestors().skip(1).take(4) {
        let p: PathBuf = dir.join("wasm32-wasip2").join("release").join(&file);
        if p.is_file() {
            return std::fs::read(&p).unwrap();
        }
    }
    panic!("the law file {file} is not built. Run `bash scripts/build_laws.sh`.");
}

#[test]
fn mimo_shock_the_world_is_named_shock_mimo() {
    assert_eq!(SHOCK_MIMO_WORLD, "shock-mimo");
}

#[test]
fn mimo_shock_a_law_file_of_the_world_is_recognized_and_refused_by_name() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    match host.load_any(&law_bytes("law_fixture_shock_mimo_minimal")) {
        Err(LawError::NotALaw(m)) => assert_eq!(m, SHOCK_MIMO_NOT_RUN),
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("a MIMO Shock law file loaded"),
    }
}

#[test]
fn mimo_shock_a_law_of_another_world_is_not_taken_for_one() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host.load_any(&law_bytes("hertzforge_law_shock_srs_ratio"));
    assert!(law.is_ok(), "{:?}", law.err().map(|e| e.to_string()));
}
