//! The dependency firewall, at two levels.
//!
//! - **The repository** holds only the controller crates and modules imported and
//!   reviewed when they came in. Any other controller crate in the lockfile, from this
//!   tree or from a registry, fails, and so does any package built from this
//!   repository's source that is not on a list here. No package is built from source
//!   outside the repository.
//! - **This crate** reaches only the controller crates on its own, narrower reviewed
//!   allowlist, and never the controller's desktop app, directly or transitively,
//!   through any dependency kind of its own (its dependencies' own dev-dependencies
//!   are never built for it, so the walk does not follow them).
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

/// The only controller crates this crate may reach, each reviewed for license and
/// hardware coupling. A new one is a change to review before this list grows.
///
/// Hardware comes in through one door: `hertzforge-station`, the controller's station
/// factory, which alone names the NI-DAQmx backend (`hertzforge-hal-ni`, loaded at run
/// time from NI's public driver; deny.toml allows no other crate to name it) and the
/// build stamp every run's provenance carries (`hertzforge-buildinfo`).
const REVIEWED_CONTROLLER_CRATES: &[&str] = &[
    "hertzforge-breakpoints",
    "hertzforge-buildinfo",
    "hertzforge-capture",
    "hertzforge-channel-table",
    "hertzforge-dsp",
    "hertzforge-engine",
    "hertzforge-hal",
    "hertzforge-hal-ni",
    "hertzforge-hal-sim",
    "hertzforge-module-abi",
    "hertzforge-random",
    "hertzforge-sdm",
    "hertzforge-shaker-db",
    "hertzforge-station",
    // The law catalog on the wire (`laws.list`, `laws.import`) and the MIMO setup's law
    // choice, reviewed 2026-11-03: `hertzforge-random-laws` names no backend (its tests
    // use the Sim as a fixture, which the Sim-leak lint already allows), and it brings
    // the law host (the sandbox law files run in), the shared ratio-law math and the
    // built-in MIMO laws (with the law SDK's plain types they are written against), all
    // pure computation on the numbers they are handed. These are
    // on LAW_CRATES, not IMPORTED_CONTROLLER_CRATES: written here, not imported.
    "hertzforge-law-host",
    "hertzforge-law-ratio-math",
    "hertzforge-law-sdk",
    "hertzforge-mimo-laws",
    "hertzforge-random-laws",
    // The MIMO Random run service and the reference reader behind the wire's MIMO
    // driver (src/mimo/adapter.rs), reviewed 2026-11-03: `hertzforge-mimo-random`
    // reaches hardware only through the station backend the control service hands it
    // (built by `hertzforge-station`, wrapped for the operator's stop and abort) and
    // holds the Sim only as a dev-dependency; `hertzforge-reference` reads and checks
    // reference files (sha2 and zip, both MIT) and names no backend.
    "hertzforge-mimo-random",
    "hertzforge-reference",
    // Classical shock behind the wire's `shock.*` methods, reviewed 2026-11-07: the
    // classical run service (`hertzforge-shock-host`), the pulse run infrastructure it
    // shares with SRS (`hertzforge-shock-common`) and time waveform replication's
    // identification and inverse (`hertzforge-twr`, whose pure modules the closed loop
    // uses) reach hardware only through the station backend the control service hands
    // them, and hold the Sim, if at all, only as a dev-dependency. All three are MIT and
    // name no backend in their `[dependencies]`.
    "hertzforge-shock-common",
    "hertzforge-shock-host",
    "hertzforge-twr",
    // The classical shock laws, reviewed 2026-11-07, as the Random law crates were:
    // the built-in law's math (`hertzforge-shock-law-math`, on `hertzforge-dsp` alone)
    // and the choice and check of a law (`hertzforge-shock-laws`, which runs a check
    // case on the backend it is handed and names none). Written here, so on LAW_CRATES.
    "hertzforge-shock-law-math",
    "hertzforge-shock-laws",
    // SRS shock behind the wire's `srs.*` methods, reviewed 2026-11-07: the SRS
    // synthesis engines, the predictive limit notch and the SRS run service
    // (`hertzforge-srs`, MIT), reached first through the SRS law choice and check in
    // `hertzforge-shock-laws`. Its `[dependencies]` are crates already on this list and
    // name no backend: it reaches hardware only through the station backend it is
    // handed, and holds the Sim only as a dev-dependency. Its delegated fire path is
    // never constructed by this crate.
    "hertzforge-srs",
    // Sine behind the wire's `sine.*` methods, reviewed 2026-11-08: the Sine profile
    // grammar, schedulers, control core and run service (`hertzforge-sine`, MIT,
    // imported), reached first through the Sine law choice and check in
    // `hertzforge-sine-laws`. Its `[dependencies]` are crates already on this list
    // and name no backend: the run service is handed a `Box<dyn Backend>` by the
    // station factory, and the Sim is a dev-dependency only. Its law seam calls a
    // factory the control service installs and holds no law host of its own.
    "hertzforge-sine",
    // The Sine laws, reviewed 2026-11-08 as the shock law crates were: the choice
    // and check of a law (`hertzforge-sine-laws`), which runs a check case on the
    // backend it is handed and names none. Written here, so on LAW_CRATES.
    "hertzforge-sine-laws",
    // The TWR laws, reviewed 2026-11-08 as the Sine and shock law crates were: the
    // choice and check of a law (`hertzforge-twr-laws`), which runs a check case on
    // the backend it is handed and names none. Its `[dependencies]` are
    // `hertzforge-twr`, `hertzforge-random-laws` and `hertzforge-hal`, all on this
    // list. Written here, so on LAW_CRATES.
    "hertzforge-twr-laws",
];

/// Every controller crate the repository holds (`crates/hertzforge-*`), each reviewed for
/// license and hardware coupling when it was imported. A controller crate not on this
/// list fails the lockfile check; adding one is a review, then a line here.
///
/// `hertzforge-mimo-random` was written here as well: the multiple-output Random
/// run service (identification, the loop, the guards, records). Like the
/// single-output service it reaches hardware only through the station factory
/// and builds the Sim only as a test fixture. The control service reaches it
/// through the wire adapter (`src/mimo/adapter.rs`).
///
/// `hertzforge-reference` was written here too: reference SDM builders, validation
/// and file readers (the `zip` and `sha2` crates, both MIT). It names no backend and
/// reaches no hardware. The control service reads a setup's reference file with it, and
/// reaches it for `mimo_random.reference.*`.
///
/// `hertzforge-sdm` was written here, not imported: spectral density matrix stacks,
/// the multiple-output law boundary and per-line linear algebra (faer, MIT). It
/// names no backend and reaches no hardware; it builds for WebAssembly.
const IMPORTED_CONTROLLER_CRATES: &[&str] = &[
    "hertzforge-automation",
    "hertzforge-backendd",
    "hertzforge-breakpoints",
    "hertzforge-buildinfo",
    "hertzforge-capture",
    "hertzforge-channel-table",
    "hertzforge-data",
    "hertzforge-demo",
    "hertzforge-dsp",
    "hertzforge-engine",
    "hertzforge-explorer-host",
    "hertzforge-formats",
    "hertzforge-hal",
    "hertzforge-hal-ni",
    "hertzforge-hal-sim",
    "hertzforge-host",
    "hertzforge-insight",
    "hertzforge-ipc",
    "hertzforge-mimo-random",
    "hertzforge-module-abi",
    "hertzforge-protocol",
    "hertzforge-random",
    "hertzforge-recorder",
    "hertzforge-reference",
    "hertzforge-shaker-db",
    "hertzforge-shock-common",
    "hertzforge-shock-host",
    "hertzforge-sdm",
    "hertzforge-sine",
    "hertzforge-srs",
    "hertzforge-station",
    "hertzforge-twr",
];

/// The controller's modules (`modules/`), imported and reviewed with the crates.
const IMPORTED_MODULES: &[&str] = &["mimo_compile_probe", "random_sim_slice", "smart"];

/// The repository's own packages that are not controller code.
const OWN_PACKAGES: &[&str] = &["sds-control"];

/// The control-law host and its guest SDK, the shared ratio-law math, and the
/// Random law catalog that joins the host to the Random core, written in this
/// repository rather than imported, and the guest crates built to WebAssembly:
/// the control laws under `laws/` and the law host's test fixtures. Reviewed
/// for license and hardware coupling when they came in: none names a backend
/// (the catalog's tests use the Sim), and the guests can reach nothing but the
/// numbers the host hands them. This crate reaches the catalog and what it needs
/// (see REVIEWED_CONTROLLER_CRATES), never a guest crate. The
/// multiple-output set (`hertzforge-mimo-laws` and the `mimo-*` guests) was
/// reviewed the same way: pure math on the stacks, no backend, synthetic plants
/// in its tests. So was the classical shock set (2026-11-07): the built-in law's
/// math, the choice and check of a law, the `shock-fdc` law file, and the
/// `shock-misbehave` and `shock-template` host fixtures (the latter the SDK
/// template's own law source, built in this workspace). The SRS shock set was
/// listed the same way when its contracts were written (2026-11-07), before the
/// crates exist: the `shock-srs-ratio` law file and the `srs-misbehave` and
/// `srs-template` host fixtures. So was the Sine set (2026-11-08): the choice and
/// check of a law (`hertzforge-sine-laws`, written with the contracts), and, before
/// they exist, the `sine-servo` law file and the `sine-misbehave` and
/// `sine-template` host fixtures. So was the TWR set (2026-11-08): the choice and
/// check of a law (`hertzforge-twr-laws`, written with the contracts), and, before
/// they exist, the `twr-correct` law file and the `twr-misbehave` and
/// `twr-template` host fixtures.
const LAW_CRATES: &[&str] = &[
    "hertzforge-law-hinv",
    "hertzforge-law-host",
    "hertzforge-law-mimo-buzz",
    "hertzforge-law-mimo-min-drive",
    "hertzforge-law-mimo-open-loop",
    "hertzforge-law-mimo-pinv",
    "hertzforge-law-mimo-shape",
    "hertzforge-law-mimo-zero",
    "hertzforge-law-ratio",
    "hertzforge-law-ratio-math",
    "hertzforge-law-sdk",
    "hertzforge-law-shock-fdc",
    "hertzforge-law-shock-srs-ratio",
    "hertzforge-law-sine-servo",
    "hertzforge-law-twr-correct",
    "hertzforge-mimo-laws",
    "hertzforge-random-laws",
    "hertzforge-shock-law-math",
    "hertzforge-shock-laws",
    "hertzforge-sine-laws",
    "hertzforge-twr-laws",
    "law-fixture-bad-manifest",
    "law-fixture-mimo-misbehave",
    "law-fixture-misbehave",
    "law-fixture-shock-misbehave",
    "law-fixture-shock-template",
    "law-fixture-sine-misbehave",
    "law-fixture-sine-template",
    "law-fixture-srs-misbehave",
    "law-fixture-srs-template",
    "law-fixture-twr-misbehave",
    "law-fixture-twr-template",
];

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
    // Each edge keeps whether it is ONLY a dev-dependency: a dependency's own
    // dev-dependencies (its tests and examples) are never built for this crate, so the
    // walk follows them from this crate alone.
    let edges: HashMap<&str, Vec<(&str, bool)>> = meta["resolve"]["nodes"]
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
                    .map(|d| {
                        let dev_only = d["dep_kinds"]
                            .as_array()
                            .is_some_and(|k| !k.is_empty() && k.iter().all(|k| k["kind"] == "dev"));
                        (d["pkg"].as_str().unwrap(), dev_only)
                    })
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
            let from_root = id == root;
            queue.extend(
                edges
                    .get(id)
                    .into_iter()
                    .flatten()
                    .filter(|(_, dev_only)| from_root || !dev_only)
                    .map(|(pkg, _)| *pkg),
            );
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
        names.contains("hertzforge-random") && names.contains("tokio"),
        "the lockfile scan found none of the expected packages; the check cannot be trusted"
    );
    let unreviewed: Vec<&&str> = names
        .iter()
        .filter(|n| {
            n.starts_with("hertzforge-")
                && !IMPORTED_CONTROLLER_CRATES.contains(*n)
                && !LAW_CRATES.contains(*n)
        })
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
    for known in ["sds-control", "hertzforge-random", "smart"] {
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
                || OWN_PACKAGES.contains(&n)
                || LAW_CRATES.contains(&n))
        })
        .collect();
    assert!(
        unreviewed.is_empty(),
        "packages built from this repository's source that are on no reviewed list: \
         {unreviewed:?} (review, then add them to IMPORTED_CONTROLLER_CRATES, IMPORTED_MODULES, \
         OWN_PACKAGES or LAW_CRATES)"
    );
    // This crate's allowlist is a subset of what the repository holds.
    let stray: Vec<&&str> = REVIEWED_CONTROLLER_CRATES
        .iter()
        .filter(|n| !IMPORTED_CONTROLLER_CRATES.contains(*n) && !LAW_CRATES.contains(*n))
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
        local.iter().any(|(n, _)| n == "hertzforge-random"),
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
    for known in ["hertzforge-random", "hertzforge-hal-sim", "tokio"] {
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
        .filter(|p| p.name.starts_with("hertzforge-"))
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
