//! Reading, normalizing and comparing a run folder against a golden one: the
//! pieces every golden run folder's test shares.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Every file under `root`, relative to it, sorted; dot files left out.
pub fn files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if !p.file_name().unwrap().to_string_lossy().starts_with('.') {
                out.push(p.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    out.sort();
    out
}

/// Set the member at `path` when it is there.
pub fn set(v: &mut Value, path: &[&str], to: Value) {
    let mut at = v;
    for k in &path[..path.len() - 1] {
        match at.get_mut(*k) {
            Some(next) => at = next,
            None => return,
        }
    }
    if let Some(m) = at.as_object_mut() {
        if m.contains_key(path[path.len() - 1]) {
            m.insert(path[path.len() - 1].to_owned(), to);
        }
    }
}

pub fn pretty(v: &Value) -> Vec<u8> {
    let mut out = serde_json::to_vec_pretty(v).unwrap();
    out.push(b'\n');
    out
}

/// Each line of a JSON-lines file with `at_unix` set to `start + line`.
pub fn lines_at(bytes: Vec<u8>, start: f64) -> Vec<u8> {
    let mut text = Vec::new();
    for (k, line) in String::from_utf8(bytes).unwrap().lines().enumerate() {
        let mut v: Value = serde_json::from_str(line).unwrap();
        set(&mut v, &["at_unix"], json!(start + k as f64));
        text.extend(serde_json::to_vec(&v).unwrap());
        text.push(b'\n');
    }
    text
}

/// Numbers within a part in a million (or 1e-9 near zero): the platform's maths
/// library may differ in its last bits.
fn close(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= 1e-6 * a.abs().max(b.abs()) + 1e-9 || (a.is_nan() && b.is_nan())
}

pub fn same_json(a: &Value, b: &Value, path: &str) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => assert!(
            close(x.as_f64().unwrap(), y.as_f64().unwrap()),
            "{path}: {x} against the golden {y}"
        ),
        (Value::Object(x), Value::Object(y)) => {
            let mut kx: Vec<&String> = x.keys().collect();
            let mut ky: Vec<&String> = y.keys().collect();
            kx.sort();
            ky.sort();
            assert_eq!(kx, ky, "{path}: members differ");
            for (k, v) in x {
                same_json(v, &y[k], &format!("{path}.{k}"));
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "{path}: lengths differ");
            for (i, (u, v)) in x.iter().zip(y).enumerate() {
                same_json(u, v, &format!("{path}[{i}]"));
            }
        }
        _ => assert_eq!(a, b, "{path}: differs"),
    }
}

fn same_numbers(name: &str, got: &[u8], want: &[u8], width: usize) {
    assert_eq!(
        got.len(),
        want.len(),
        "{name}: {} bytes against the golden {}",
        got.len(),
        want.len()
    );
    for (k, (a, b)) in got.chunks(width).zip(want.chunks(width)).enumerate() {
        let (x, y) = if width == 4 {
            (
                f64::from(f32::from_le_bytes(a.try_into().unwrap())),
                f64::from(f32::from_le_bytes(b.try_into().unwrap())),
            )
        } else {
            (
                f64::from_le_bytes(a.try_into().unwrap()),
                f64::from_le_bytes(b.try_into().unwrap()),
            )
        };
        // a float32 sample to its own precision
        let tol = if width == 4 { 1e-5 } else { 1e-6 };
        assert!(
            x == y
                || (x - y).abs() <= tol * x.abs().max(y.abs()) + 1e-9
                || (x.is_nan() && y.is_nan()),
            "{name}: value {k} is {x}, the golden {y}"
        );
    }
}

/// The folder's files against the golden folder's: the same files, JSON member
/// for member, numbers within [`close`], samples to their own precision.
pub fn compare(got: &BTreeMap<String, Vec<u8>>, want: &BTreeMap<String, Vec<u8>>) {
    let a: Vec<&String> = got.keys().collect();
    let b: Vec<&String> = want.keys().collect();
    assert_eq!(a, b, "the folder's files differ from the golden folder's");
    for (name, bytes) in got {
        let gold = &want[name];
        if name.ends_with(".json") {
            same_json(
                &serde_json::from_slice(bytes).unwrap(),
                &serde_json::from_slice(gold).unwrap(),
                name,
            );
        } else if name.ends_with(".jsonl") {
            let (x, y) = (
                String::from_utf8(bytes.clone()).unwrap(),
                String::from_utf8(gold.clone()).unwrap(),
            );
            let (x, y): (Vec<&str>, Vec<&str>) = (x.lines().collect(), y.lines().collect());
            assert_eq!(x.len(), y.len(), "{name}: lines differ");
            for (k, (u, v)) in x.iter().zip(&y).enumerate() {
                same_json(
                    &serde_json::from_str(u).unwrap(),
                    &serde_json::from_str(v).unwrap(),
                    &format!("{name}:{}", k + 1),
                );
            }
        } else if name.ends_with(".f32") {
            same_numbers(name, bytes, gold, 4);
        } else if name.ends_with(".bin") {
            same_numbers(name, bytes, gold, 8);
        } else {
            assert_eq!(bytes, gold, "{name} differs");
        }
    }
}

/// Write `files` as the folder `dir`, replacing what is there.
pub fn write_golden(dir: &Path, files: &BTreeMap<String, Vec<u8>>) {
    let _ = std::fs::remove_dir_all(dir);
    for (name, bytes) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}
