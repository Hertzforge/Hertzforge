//! How long `matrix` takes to be built, natively and as a law file, on the
//! research testbed's linear four-by-four structure with its 2 percent scatter
//! over 40 to 400 Hz (the case the law mathematics' own timings use). Ignored
//! by default: run with `--ignored`, optimized build. It prints the two times
//! and checks the law file builds within the default build budget.

use hertzforge_law_host::{HostConfig, LawHost};
use hertzforge_law_sdk::sine_mimo::builtin;
use hertzforge_mimo_sine::{MimoSineLawFactory, TestType};
use hertzforge_mimo_sine_law_math::reference_structure::ModalStructure;
use hertzforge_mimo_sine_laws::parity::bench::law_setup;
use hertzforge_mimo_sine_laws::{
    load_law_file, BuiltinFactory, WasmSineMimoLawFactory, DEFAULT_BUILD_BUDGET_S,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn law_file(name: &str) -> PathBuf {
    let file = format!("{name}.wasm");
    let exe = std::env::current_exe().expect("the test binary knows its own path");
    for dir in exe.ancestors().skip(1).take(4) {
        let p = dir.join("wasm32-wasip2").join("release").join(&file);
        if p.is_file() {
            return p;
        }
    }
    panic!("the law file {file} is not built. Run `bash scripts/build_laws.sh`.");
}

fn lin4x4() -> ModalStructure {
    let path = format!(
        "{}/../hertzforge-mimo-sine-law-math/tests/data/matrix_parity.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
    let c = &v["lin4x4"];
    let f = |k: &str| -> Vec<f64> {
        c[k].as_array()
            .unwrap()
            .iter()
            .flat_map(|x| match x.as_array() {
                Some(row) => row.iter().map(|y| y.as_f64().unwrap()).collect(),
                None => vec![x.as_f64().unwrap()],
            })
            .collect()
    };
    ModalStructure {
        f_hz: f("f_hz"),
        zeta: f("zeta"),
        drive_shape: f("drive_shape"),
        output_shape: f("output_shape"),
        drives: 4,
        controls: 4,
    }
}

#[test]
#[ignore = "a measurement; run with --ignored on an optimized build"]
fn mimo_sine_matrix_build_time_native_and_as_a_law_file() {
    let st = lin4x4();
    let setup = law_setup(&st.setup((40.0, 400.0), true), TestType::Sweep);
    let native =
        BuiltinFactory::new(&builtin::MATRIX, builtin::defaults(&builtin::MATRIX)).unwrap();
    let t = Instant::now();
    native.build(&setup).unwrap();
    let native_s = t.elapsed().as_secs_f64();
    let host = LawHost::new(HostConfig::default()).unwrap();
    let file = WasmSineMimoLawFactory::new(
        load_law_file(&host, &law_file("hertzforge_law_mimo_sine_matrix")).unwrap(),
        Vec::new(),
        Duration::from_millis(10),
    );
    let t = Instant::now();
    file.build(&setup).unwrap();
    let file_s = t.elapsed().as_secs_f64();
    eprintln!(
        "matrix built on {} lines: natively {native_s:.2} s, as a law file {file_s:.2} s",
        setup.identification.frequency_hz.len()
    );
    assert!(file_s < DEFAULT_BUILD_BUDGET_S);
}
