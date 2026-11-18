//! The law host's `sine-mimo` world: the world is named, and a law file of
//! another world offered as a MIMO Sine law is refused, naming the world it
//! is.

use hertzforge_law_host::{AnyLaw, HostConfig, LawError, LawHost, SINE_MIMO_WORLD};
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

fn refusal(r: Result<hertzforge_law_host::LoadedSineMimoLaw, LawError>) -> String {
    match r {
        Err(LawError::NotALaw(m)) => m,
        Err(e) => panic!("refused, but not as a law of another world: {e}"),
        Ok(_) => panic!("a law of another world loaded as a MIMO Sine law"),
    }
}

#[test]
fn mimo_sine_the_world_is_named_sine_mimo() {
    assert_eq!(SINE_MIMO_WORLD, "sine-mimo");
}

#[test]
fn mimo_sine_a_law_of_another_world_is_refused_by_name() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let m = refusal(host.load_sine_mimo(&law_bytes("hertzforge_law_mimo_shock_paired")));
    assert!(m.contains("MIMO Shock law (the shock-mimo world)"), "{m}");
    assert!(m.contains("not a MIMO Sine law"), "{m}");
    let m = refusal(host.load_sine_mimo(&law_bytes("hertzforge_law_sine_servo")));
    assert!(
        m.contains("single-output Sine law (the sine-single world)"),
        "{m}"
    );
    let m = refusal(host.load_sine_mimo(&law_bytes("hertzforge_law_mimo_pinv")));
    assert!(m.contains("multiple-output Random law"), "{m}");
    let m = refusal(host.load_sine_mimo(&law_bytes("hertzforge_law_shock_srs_ratio")));
    assert!(m.contains("another test type"), "{m}");
}

#[test]
fn mimo_sine_a_law_of_another_world_still_loads_as_its_own() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let any = host
        .load_any(&law_bytes("hertzforge_law_mimo_shock_paired"))
        .unwrap();
    assert!(matches!(any, AnyLaw::ShockMimo(_)));
    assert_ne!(any.world(), SINE_MIMO_WORLD);
}
