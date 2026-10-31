//! The dependency firewall: this crate reaches only the controller crates on a
//! reviewed allowlist, never any other controller crate (the private hardware backends
//! among them) and never the controller's desktop app, directly or transitively,
//! through any dependency kind. The allowlist is the whole rule, so the tree never has
//! to name what it keeps out.
//!
//! Two views of the graph: the workspace lockfile, which lists every resolved
//! package for every platform, and `cargo metadata` (resolved, all features, dev and
//! build dependencies included, filtered to the host platform so it runs offline),
//! walked from this package.

use serde_json::Value;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::process::Command;

/// The only controller crates this crate may reach: open, hardware-neutral, reviewed.
/// A new one is a change to review (license, hardware coupling) before this list grows.
const REVIEWED_CONTROLLER_CRATES: &[&str] = &[
    "eigen-breakpoints",
    "eigen-capture",
    "eigen-channel-table",
    "eigen-dsp",
    "eigen-engine",
    "eigen-hal",
    "eigen-hal-sim",
    "eigen-module-abi",
    "eigen-random",
    "eigen-shaker-db",
];

struct Package {
    name: String,
    manifest_path: String,
}

/// The host target triple, from `rustc -vV`.
fn host_triple() -> String {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc)
        .arg("-vV")
        .output()
        .expect("rustc -vV runs");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_owned))
        .expect("rustc -vV names the host")
}

/// Every package reachable from this one on the host platform.
fn reachable() -> Vec<Package> {
    let out = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--offline",
            "--filter-platform",
        ])
        .arg(host_triple())
        .arg("--manifest-path")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .output()
        .expect("cargo metadata runs");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: Value = serde_json::from_slice(&out.stdout).expect("metadata is JSON");

    let packages: HashMap<&str, Package> = meta["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["id"].as_str().unwrap(),
                Package {
                    name: p["name"].as_str().unwrap().to_owned(),
                    manifest_path: p["manifest_path"].as_str().unwrap().to_owned(),
                },
            )
        })
        .collect();
    let edges: HashMap<&str, Vec<&str>> = meta["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            (
                n["id"].as_str().unwrap(),
                n["deps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|d| d["pkg"].as_str().unwrap())
                    .collect(),
            )
        })
        .collect();
    let root = packages
        .iter()
        .find(|(_, p)| p.name == env!("CARGO_PKG_NAME"))
        .map(|(id, _)| *id)
        .expect("this package is in the metadata");

    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([root]);
    while let Some(id) = queue.pop_front() {
        if seen.insert(id) {
            queue.extend(edges.get(id).into_iter().flatten().copied());
        }
    }
    seen.remove(root);
    seen.into_iter()
        .map(|id| {
            let p = &packages[id];
            Package {
                name: p.name.clone(),
                manifest_path: p.manifest_path.clone(),
            }
        })
        .collect()
}

#[test]
fn the_lockfile_holds_only_reviewed_controller_crates_for_any_platform() {
    let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
        .expect("the workspace lockfile exists (build once first)");
    let names: BTreeSet<&str> = lock
        .lines()
        .filter_map(|l| l.strip_prefix("name = \"")?.strip_suffix('"'))
        .collect();
    assert!(
        names.contains("eigen-random") && names.contains("tokio"),
        "the lockfile scan found none of the expected packages; the check cannot be trusted"
    );
    let unreviewed: Vec<&&str> = names
        .iter()
        .filter(|n| n.starts_with("eigen-") && !REVIEWED_CONTROLLER_CRATES.contains(*n))
        .collect();
    assert!(
        unreviewed.is_empty(),
        "controller crates in Cargo.lock outside the reviewed set: {unreviewed:?}"
    );
    // The controller's desktop app is a package named `ui` with a `ui_lib` library.
    assert!(
        !names.contains("ui_lib"),
        "the controller's desktop app is in Cargo.lock"
    );
}

#[test]
fn the_desktop_app_is_not_reachable() {
    let deps = reachable();
    // The walk must be able to find something, or a pass would prove nothing.
    for known in ["eigen-random", "eigen-hal-sim", "tokio"] {
        assert!(
            deps.iter().any(|p| p.name == known),
            "the dependency walk did not find {known}; the check cannot be trusted"
        );
    }
    let hits: Vec<String> = deps
        .iter()
        .filter(|p| p.name == "ui" || p.manifest_path.contains("src-tauri"))
        .map(|p| format!("{} ({})", p.name, p.manifest_path))
        .collect();
    assert!(
        hits.is_empty(),
        "the controller's desktop app is in the dependency graph: {hits:?}"
    );
}

#[test]
fn controller_crates_are_the_reviewed_set() {
    let mut controller: Vec<String> = reachable()
        .into_iter()
        .filter(|p| p.name.starts_with("eigen-"))
        .map(|p| p.name)
        .collect();
    controller.sort();
    controller.dedup();
    let unreviewed: Vec<&String> = controller
        .iter()
        .filter(|n| !REVIEWED_CONTROLLER_CRATES.contains(&n.as_str()))
        .collect();
    assert!(
        unreviewed.is_empty(),
        "controller crates not yet reviewed for this crate: {unreviewed:?} (review, then add \
         them to REVIEWED_CONTROLLER_CRATES)"
    );
}
