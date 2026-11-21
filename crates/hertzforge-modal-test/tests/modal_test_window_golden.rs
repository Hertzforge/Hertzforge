//! The force and exponential windows against the golden vectors the analysis side writes
//! (`tests/data/modal_test_windows.json`, from the workspace root): every case's samples,
//! made here from the numbers the case states, agree to the file's tolerance.

use hertzforge_modal_test::window::{ExpWindow, ForceWindow};
use serde_json::Value;
use std::path::Path;

fn golden() -> Value {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/modal_test_windows.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("the golden windows are JSON")
}

fn whole(case: &Value, key: &str) -> usize {
    case[key]
        .as_u64()
        .unwrap_or_else(|| panic!("{key} is a whole number of samples in {case}")) as usize
}

fn number(case: &Value, key: &str) -> f64 {
    match &case[key] {
        Value::String(s) if s == "inf" => f64::INFINITY,
        v => v
            .as_f64()
            .unwrap_or_else(|| panic!("{key} is a number in {case}")),
    }
}

#[test]
fn the_windows_give_the_golden_samples() {
    let golden = golden();
    let tolerance = golden["tolerance"].as_f64().expect("a tolerance");
    let cases = golden["cases"].as_array().expect("cases");
    let (mut force, mut exponential) = (0, 0);
    for case in cases {
        let block = whole(case, "block");
        let made = match case["window"].as_str() {
            Some("force") => {
                force += 1;
                ForceWindow {
                    pre: whole(case, "pre"),
                    width: whole(case, "width"),
                    fall: whole(case, "fall"),
                    block,
                }
                .values()
            }
            Some("exponential") => {
                exponential += 1;
                ExpWindow {
                    tau_s: number(case, "tau_s"),
                    end: 0.0,
                    block,
                    fs_hz: number(case, "rate_hz"),
                }
                .values()
            }
            other => panic!("a window the golden file does not define: {other:?}"),
        };
        let want: Vec<f64> = case["samples"]
            .as_array()
            .expect("samples")
            .iter()
            .map(|v| v.as_f64().expect("a sample"))
            .collect();
        assert_eq!(made.len(), want.len(), "{case}");
        for (n, (a, b)) in made.iter().zip(&want).enumerate() {
            assert!(
                (a - b).abs() <= tolerance,
                "sample {n}: {a} against {b} in {}",
                case["window"]
            );
        }
    }
    assert!(
        force > 0 && exponential > 0,
        "both windows are in the golden file"
    );
}
