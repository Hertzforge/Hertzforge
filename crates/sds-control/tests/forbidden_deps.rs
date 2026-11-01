//! The dependency firewall, at two levels.
//!
//! - **The repository** holds only the controller crates and modules imported and
//!   reviewed when they came in. Any other controller crate in the lockfile, from this
//!   tree or from a registry, fails, and so does any package built from this
//!   repository's source that is not on a list here. No package is built from source
//!   outside the repository.
//! - **This crate** reaches only the controller crates on its own, narrower reviewed
//!   allowlist, and never the controller's desktop app, directly or transitively,
//!   through any dependency kind.
//!
//! The allowlists are the whole rule, so the tree never has to name what it keeps out.
//!
//! Two views of the graph: the workspace lockfile, which lists every resolved package
//! for every platform, and `cargo metadata` (resolved, all features, dev and build
//! dependencies included, filtered to the host platform so it runs offline).

use serde_json::Value;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::Path;
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

/// Every controller crate the repository holds (`crates/eigen-*`), each reviewed for
/// license and hardware coupling when it was imported. A controller crate not on this
/// list fails the lockfile check; adding one is a review, then a line here.
const IMPORTED_CONTROLLER_CRATES: &[&str] = &[
    "eigen-automation",
    "eigen-backendd",
    "eigen-breakpoints",
    "eigen-buildinfo",
    "eigen-capture",
    "eigen-channel-table",
    "eigen-data",
    "eigen-demo",
    "eigen-dsp",
    "eigen-engine",
    "eigen-explorer-host",
    "eigen-formats",
    "eigen-hal",
    "eigen-hal-ni",
    "eigen-hal-sim",
    "eigen-host",
    "eigen-insight",
    "eigen-ipc",
    "eigen-module-abi",
    "eigen-protocol",
    "eigen-random",
    "eigen-recorder",
    "eigen-shaker-db",
    "eigen-shock-common",
    "eigen-shock-host",
    "eigen-sine",
    "eigen-srs",
    "eigen-station",
    "eigen-twr",
];

/// The controller's modules (`modules/`), imported and reviewed with the crates.
const IMPORTED_MODULES: &[&str] = &["mimo_compile_probe", "random_sim_slice", "smart"];

/// The repository's own packages that are not controller code.
const OWN_PACKAGES: &[&str] = &["sds-control"];

struct Package {
    name: String,
    manifest_path: String,
}

/// One `[[package]]` entry of the lockfile.
struct Locked {
    name: String,
    /// `None` for a package built from source in this workspace.
    source: Option<String>,
}

/// Every package the workspace lockfile lists.
fn locked() -> Vec<Locked> {
    let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
        .expect("the workspace lockfile exists (build once first)");
    lock.split("[[package]]")
        .skip(1)
        .map(|entry| {
            let field = |key: &str| {
                entry.lines().find_map(|l| {
                    l.strip_prefix(key)?
                        .strip_prefix(" = \"")?
                        .strip_suffix('"')
                        .map(str::to_owned)
                })
            };
            Locked {
                name: field("name").expect("every lockfile entry has a name"),
                source: field("source"),
            }
        })
        .collect()
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

/// The resolved workspace graph on the host platform.
fn metadata() -> Value {
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
    serde_json::from_slice(&out.stdout).expect("metadata is JSON")
}

/// Every package reachable from this one on the host platform.
fn reachable() -> Vec<Package> {
    let meta = metadata();

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
    let entries = locked();
    let names: BTreeSet<&str> = entries.iter().map(|p| p.name.as_str()).collect();
    assert!(
        names.contains("eigen-random") && names.contains("tokio"),
        "the lockfile scan found none of the expected packages; the check cannot be trusted"
    );
    let unreviewed: Vec<&&str> = names
        .iter()
        .filter(|n| n.starts_with("eigen-") && !IMPORTED_CONTROLLER_CRATES.contains(*n))
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
fn every_package_built_from_this_repository_is_on_a_reviewed_list() {
    let own: BTreeSet<String> = locked()
        .into_iter()
        .filter(|p| p.source.is_none())
        .map(|p| p.name)
        .collect();
    // The scan must see the workspace, or a pass would prove nothing.
    for known in ["sds-control", "eigen-random", "smart"] {
        assert!(
            own.contains(known),
            "the lockfile scan did not find {known} among this repository's packages; the \
             check cannot be trusted"
        );
    }
    let unreviewed: Vec<&String> = own
        .iter()
        .filter(|n| {
            let n = n.as_str();
            !(IMPORTED_CONTROLLER_CRATES.contains(&n)
                || IMPORTED_MODULES.contains(&n)
                || OWN_PACKAGES.contains(&n))
        })
        .collect();
    assert!(
        unreviewed.is_empty(),
        "packages built from this repository's source that are on no reviewed list: \
         {unreviewed:?} (review, then add them to IMPORTED_CONTROLLER_CRATES, IMPORTED_MODULES \
         or OWN_PACKAGES)"
    );
    // This crate's allowlist is a subset of what the repository holds.
    let stray: Vec<&&str> = REVIEWED_CONTROLLER_CRATES
        .iter()
        .filter(|n| !IMPORTED_CONTROLLER_CRATES.contains(*n))
        .collect();
    assert!(
        stray.is_empty(),
        "this crate's allowlist names controller crates the repository does not hold: {stray:?}"
    );
}

#[test]
fn no_package_is_built_from_source_outside_the_repository() {
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .canonicalize()
        .expect("the repository root exists");
    let meta = metadata();
    let local: Vec<(String, std::path::PathBuf)> = meta["packages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["source"].is_null())
        .map(|p| {
            let manifest = Path::new(p["manifest_path"].as_str().unwrap());
            (
                p["name"].as_str().unwrap().to_owned(),
                manifest.canonicalize().unwrap_or_else(|_| manifest.into()),
            )
        })
        .collect();
    assert!(
        local.iter().any(|(n, _)| n == "eigen-random"),
        "the metadata scan found no controller crate built from source; the check cannot be \
         trusted"
    );
    let outside: Vec<String> = local
        .iter()
        .filter(|(_, m)| !m.starts_with(&root))
        .map(|(n, m)| format!("{n} ({})", m.display()))
        .collect();
    assert!(
        outside.is_empty(),
        "packages built from source outside the repository: {outside:?}"
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
