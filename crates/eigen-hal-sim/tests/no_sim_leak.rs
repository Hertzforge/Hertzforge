//! THE SIM-LEAK LINT - the mechanical half of the no-Sim-in-production gate.
//!
//! # What it enforces
//!
//! 1. **No production code outside the allowlist names a Sim type.** `SimBackend`,
//!    `SimConfig` and `eigen_hal_sim::` must not appear in production Rust across
//!    the workspace crates, the module crates, or `ui/src-tauri` - with exactly two
//!    kinds of exception, both enumerated below.
//!
//!    **SCOPE STATEMENT (stated honestly).** The walk covers `crates/**`,
//!    `modules/**` and `ui/src-tauri/src/**`. The factory lift added TWO crates that sit outside
//!    `ui/src-tauri` - `eigen-station` (the lifted factory) and `eigen-backendd`
//!    (the headless daemon) - and both are under `crates/`, so both are ALREADY in
//!    scope with no widening required. What DID change is where the one legal
//!    construction lives: `ui/src-tauri/src/factory.rs` no longer names a Sim type
//!    at all (it is a thin wrapper now), so [`UI_SIM_FILES`] is empty and the
//!    construction anchor moved to `eigen-station`. The daemon is covered by the
//!    same walk as every other crate and has no exemption.
//! 2. **No crate outside the allowlist holds `eigen-hal-sim` as a PRODUCTION
//!    dependency.** This is the half `cargo deny check bans` cannot express:
//!    cargo-deny's ban graph includes dev-dependencies (verified on cargo-deny
//!    0.20.2, 2026-07-25), so its wrapper list has to name every crate that touches
//!    the Sim at all and cannot distinguish "builds a plant in production" from
//!    "drives a fixture in a test". This test draws that line.
//!
//! 3. **The delegated-control test double is never reachable in production.**
//!    `eigen_hal::delegated::testing::ScriptedDelegate` exists only under
//!    `cfg(test)` or `eigen-hal`'s `test-support` feature. No production code outside
//!    `eigen-hal` may name it, and no production `[dependencies]` entry may enable
//!    that feature; dev-dependencies may. Same scanner, same reasoning as the Sim: a
//!    double a factory can hand out is a backend that lies about the device.
//!
//! # Why it lives here
//!
//! In the workspace, so the existing CI `cargo test --workspace` runs it - a lint
//! that only fires when someone remembers to run it is not a gate. In THIS crate,
//! because the banned crate carrying the enumeration of where it may legally appear
//! is the one place a reader will think to look.
//!
//! # THE LINT IS THE ENUMERATION
//!
//! There are no hand-maintained counts here and no "six sites" written down as a
//! number. The allowlists below are the complete statement of what is permitted;
//! everything else is found by walking the tree. Adding a stray
//! `SimBackend::new()` to any module file fails this test without anyone having to
//! notice.
//!
//! RED-MUTATION (run 2026-07-25, reverted): add
//! `let _ = eigen_hal_sim::SimBackend::new();` to `ui/src-tauri/src/twr.rs` outside
//! any `#[cfg(test)]` block and `production_rust_never_names_a_sim_type` reds with
//! that file and line. Add `eigen-hal-sim` to `crates/eigen-srs/Cargo.toml`'s
//! `[dependencies]` and `no_crate_outside_the_allowlist_depends_on_the_sim_in_production`
//! reds.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// =====================================================================================
// The allowlists - the complete statement of what may name the Sim
// =====================================================================================

/// Crates whose PRODUCTION code may name a Sim type.
///
/// * `eigen-hal-sim` - it is the Sim.
/// * `eigen-demo` - a demo binary whose entire subject is the Sim plant; it has no
///   customer-facing surface and nothing downstream depends on it.
/// * `eigen-station` - THE station factory. It is the one construction point,
///   and it became a crate precisely so that adding the `eigen-backendd` daemon did
///   NOT add a second one. This list is what makes "one construction point" a
///   checkable claim rather than an intention.
/// * `sds-control` - the control service's simulated Random run. It runs only on the
///   Sim plant (`src/sim.rs`, every frame marked `source: "SIM"`) and has no hardware
///   path; when it gains one, its backend comes from the station factory and this
///   entry goes.
const SIM_CRATES: &[&str] = &[
    "eigen-hal-sim",
    "eigen-demo",
    "eigen-station",
    "sds-control",
];

/// Files in `ui/src-tauri/src` allowed to construct a Sim.
///
/// **EMPTY since the factory lift, and that is the interesting part.** Before the lift, this list
/// was `["factory.rs"]` - the Tauri layer had exactly one place that decided what
/// hardware a run is on. That decision now lives in `eigen-station`, so the Tauri
/// layer has ZERO places, and `factory.rs` is scanned like every other file in the
/// app. An empty allowlist is a stronger statement than a one-element one, and it is
/// enforced identically: the entry has to come back here, deliberately, before any
/// file under `ui/src-tauri/src` may name a Sim type again.
const UI_SIM_FILES: &[&str] = &[];

/// Identifiers whose presence in production code means a Sim was named.
///
/// `eigen_hal_sim::` carries the `::` deliberately - `eigen_hal_sim=debug` appears
/// inside the UI's `tracing` filter STRING, which is a log-level directive and not a
/// construction.
const SIM_TOKENS: &[&str] = &["SimBackend", "SimConfig", "eigen_hal_sim::"];

/// Crates whose production code may name the delegated-control test double: only the
/// crate that defines it (behind `cfg(any(test, feature = "test-support"))`).
const DOUBLE_CRATES: &[&str] = &["eigen-hal"];

/// Identifiers whose presence in production code means the double was named.
const DOUBLE_TOKENS: &[&str] = &["ScriptedDelegate", "delegated::testing"];

// =====================================================================================
// Tree walking
// =====================================================================================

/// Does this tree carry the Tauri app? The app-specific assertions below are about
/// `ui/src-tauri` and bind exactly when it is there.
///
/// The open crates are imported into a tree without the app: there the app's
/// factory-lift claims have no subject, and the walk simply has no app files to scan.
/// Keyed off the app's MANIFEST, not its `src/` directory, so a present app whose
/// factory moved or vanished still fails loudly below rather than being read as "no
/// app here".
fn app_present(root: &Path) -> bool {
    root.join("ui")
        .join("src-tauri")
        .join("Cargo.toml")
        .is_file()
}

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <repo>/crates/eigen-hal-sim.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate sits two levels under the repo root")
        .to_path_buf()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, ext: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            // Build output and VCS internals are not source.
            if name == "target" || name == ".git" || name == "node_modules" {
                continue;
            }
            walk(&p, out, ext);
        } else if p.extension().and_then(|s| s.to_str()) == Some(ext) {
            out.push(p);
        }
    }
}

/// Is this file TEST code by its path?
///
/// A `tests.rs`, anything under a `tests/` directory, and anything under a
/// `benches/`/`examples/` directory. Those are compiled with dev-dependencies and
/// are exactly where a Sim fixture belongs.
///
/// This is a deliberate, documented hole: a Sim construction hidden in a file named
/// `tests.rs` would not be caught here. The MANIFEST half of this lint closes the
/// consequence of that hole - such a file could only compile if its crate held
/// `eigen-hal-sim`, and if it held it in `[dependencies]` the second test fires.
fn is_test_path(p: &Path) -> bool {
    p.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some("tests") | Some("benches") | Some("examples")
        )
    }) || p.file_name().and_then(|s| s.to_str()) == Some("tests.rs")
}

// =====================================================================================
// The scanner
// =====================================================================================

/// Blank out comments, leaving everything else byte-for-byte in place.
///
/// STRING LITERALS ARE KEPT. That is the conservative direction for a firewall: a
/// Sim name inside a string is reported and a human decides, rather than being
/// silently exempt. Comments are removed because the tree is heavily commented and
/// most prose mentions of `SimBackend` are history, not construction.
fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while i < b.len() {
        // Raw string: r"..." / r#"..."# - copied verbatim, no escape processing.
        if b[i] == b'r' && i + 1 < b.len() && (b[i + 1] == b'"' || b[i + 1] == b'#') {
            let mut j = i + 1;
            let mut hashes = 0usize;
            while j < b.len() && b[j] == b'#' {
                hashes += 1;
                j += 1;
            }
            if j < b.len() && b[j] == b'"' {
                out.extend_from_slice(&b[i..=j]);
                j += 1;
                // Find the closing quote followed by `hashes` hashes.
                while j < b.len() {
                    if b[j] == b'"' && b[j + 1..].iter().take(hashes).all(|&c| c == b'#') {
                        let end = j + 1 + hashes;
                        out.extend_from_slice(&b[j..end.min(b.len())]);
                        j = end;
                        break;
                    }
                    out.push(b[j]);
                    j += 1;
                }
                i = j;
                continue;
            }
        }
        // Ordinary string literal.
        if b[i] == b'"' {
            out.push(b[i]);
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' && i + 1 < b.len() {
                    out.extend_from_slice(&b[i..i + 2]);
                    i += 2;
                    continue;
                }
                out.push(b[i]);
                let done = b[i] == b'"';
                i += 1;
                if done {
                    break;
                }
            }
            continue;
        }
        // Line comment (including `///` and `//!`).
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                out.push(b' ');
                i += 1;
            }
            continue;
        }
        // Block comment, nesting-aware (Rust allows nesting).
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            let mut depth = 1usize;
            out.extend_from_slice(b"  ");
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
                    depth += 1;
                    out.extend_from_slice(b"  ");
                    i += 2;
                } else if b[i] == b'*' && i + 1 < b.len() && b[i + 1] == b'/' {
                    depth -= 1;
                    out.extend_from_slice(b"  ");
                    i += 2;
                } else {
                    out.push(if b[i] == b'\n' { b'\n' } else { b' ' });
                    i += 1;
                }
            }
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Blank out every `#[cfg(test)]`-gated item (and `#[cfg(any(test, ...))]`), so a
/// test module inside a production file is not a leak.
///
/// Operates on already-comment-stripped source. NEWLINES ARE PRESERVED so the line
/// numbers this test reports are the real ones.
fn strip_cfg_test_items(src: &str) -> String {
    let mut out: Vec<u8> = src.as_bytes().to_vec();
    let hay = src.as_bytes();
    let mut i = 0usize;
    while i < hay.len() {
        let Some(rel) = find_cfg_test(&src[i..]) else {
            break;
        };
        let start = i + rel.0;
        let mut j = i + rel.1; // just past the attribute
                               // Skip further attributes / whitespace, then find the item's `{` or `;`.
        let mut end = None;
        while j < hay.len() {
            match hay[j] {
                b'{' => {
                    let mut depth = 1usize;
                    j += 1;
                    while j < hay.len() && depth > 0 {
                        match hay[j] {
                            b'{' => depth += 1,
                            b'}' => depth -= 1,
                            _ => {}
                        }
                        j += 1;
                    }
                    end = Some(j);
                    break;
                }
                b';' => {
                    end = Some(j + 1);
                    break;
                }
                _ => j += 1,
            }
        }
        let end = end.unwrap_or(hay.len());
        for c in out.iter_mut().take(end).skip(start) {
            if *c != b'\n' {
                *c = b' ';
            }
        }
        i = end;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Byte range of the next `#[cfg(test)]`-style attribute in `s`, if any.
fn find_cfg_test(s: &str) -> Option<(usize, usize)> {
    let mut from = 0usize;
    while let Some(rel) = s[from..].find("#[cfg(") {
        let start = from + rel;
        // The attribute ends at the matching `]`.
        let end = match s[start..].find(")]") {
            Some(e) => start + e + 2,
            None => return None,
        };
        let attr = &s[start..end];
        // `test` as a whole word inside the predicate.
        let has_test = attr
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|w| w == "test");
        if has_test {
            return Some((start, end));
        }
        from = end;
    }
    None
}

/// Every `(line_number, line)` in `path` whose PRODUCTION code names a Sim token.
fn sim_hits(path: &Path) -> Vec<(usize, String)> {
    token_hits(path, SIM_TOKENS)
}

/// Every `(line_number, line)` in `path` whose PRODUCTION code names one of `tokens`.
fn token_hits(path: &Path, tokens: &[&str]) -> Vec<(usize, String)> {
    let Ok(src) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let scrubbed = strip_cfg_test_items(&strip_comments(&src));
    scrubbed
        .lines()
        .enumerate()
        .filter(|(_, l)| tokens.iter().any(|t| l.contains(t)))
        .map(|(n, l)| (n + 1, l.trim().to_string()))
        .collect()
}

/// Every production `[dependencies]` entry in one manifest that enables
/// `eigen-hal`'s `test-support` feature, in either the inline form
/// (`eigen-hal = { path = .., features = ["test-support"] }`) or the table form
/// (`[dependencies.eigen-hal]` then `features = [..]`). Dev-dependency sections do
/// not count: that is where the double belongs.
fn test_support_in_production(src: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut section = String::new();
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t.to_string();
            continue;
        }
        if !t.contains("test-support") {
            continue;
        }
        let prod_table = section == "[dependencies]" || section.ends_with(".dependencies]");
        let inline_hal = t.starts_with("eigen-hal ") || t.starts_with("eigen-hal=");
        let hal_table = section == "[dependencies.eigen-hal]"
            || (section.ends_with(".dependencies.eigen-hal]") && !section.contains("dev-"));
        if (prod_table && inline_hal) || hal_table {
            found.push(format!("{section} {t}"));
        }
    }
    found
}

/// The crate directory name for a source path under `crates/` or `modules/`.
fn owning_crate_dir(root: &Path, p: &Path) -> Option<String> {
    let rel = p.strip_prefix(root).ok()?;
    let mut it = rel.components();
    let first = it.next()?.as_os_str().to_str()?.to_string();
    if first != "crates" && first != "modules" {
        return None;
    }
    Some(it.next()?.as_os_str().to_str()?.to_string())
}

// =====================================================================================
// The tests
// =====================================================================================

/// THE LINT. No production Rust outside the allowlists names a Sim type.
///
/// RED-MUTATION: add `let _ = eigen_hal_sim::SimBackend::new();` to any module file
/// (`ui/src-tauri/src/twr.rs`, `crates/eigen-random/src/service/engine.rs`, ...)
/// outside a `#[cfg(test)]` block and this reds, naming the file and line.
#[test]
fn production_rust_never_names_a_sim_type() {
    let root = repo_root();
    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files, "rs");
    walk(&root.join("modules"), &mut files, "rs");
    walk(
        &root.join("ui").join("src-tauri").join("src"),
        &mut files,
        "rs",
    );

    let mut scanned = 0usize;
    let mut leaks: Vec<String> = Vec::new();
    for f in &files {
        if is_test_path(f) {
            continue;
        }
        // Allowed: the Sim's own crate, the demo, and the UI's factory.
        if let Some(krate) = owning_crate_dir(&root, f) {
            if SIM_CRATES.contains(&krate.as_str()) {
                continue;
            }
        }
        if f.starts_with(root.join("ui")) {
            let base = f.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if UI_SIM_FILES.contains(&base) {
                continue;
            }
        }
        scanned += 1;
        for (line, text) in sim_hits(f) {
            leaks.push(format!(
                "{}:{line}: {text}",
                f.strip_prefix(&root).unwrap_or(f).display()
            ));
        }
    }

    // A search that could not have found anything is not a search.
    assert!(
        scanned > 100,
        "the scanner only looked at {scanned} production files; it is not walking the tree"
    );

    assert!(
        leaks.is_empty(),
        "production code names a Sim type outside the allowlist \
         (crates {SIM_CRATES:?}, ui files {UI_SIM_FILES:?}):\n  {}",
        leaks.join("\n  ")
    );
}

/// The scanner CAN see a Sim construction - proven on the one file that legitimately
/// has them.
///
/// Without this, `production_rust_never_names_a_sim_type` would pass identically if
/// `strip_comments` ate the whole file, if `walk` returned nothing, or if
/// `SIM_TOKENS` were misspelled. A green that cannot go red is not evidence.
///
/// The anchor MOVED with the factory lift, with the construction it anchors: it used to
/// be `ui/src-tauri/src/factory.rs`, and it is now `crates/eigen-station/src/lib.rs`.
/// The Tauri file no longer contains a Sim construction to find.
#[test]
fn the_scanner_finds_the_constructions_it_is_supposed_to_allow() {
    let root = repo_root();

    let factory = root
        .join("crates")
        .join("eigen-station")
        .join("src")
        .join("lib.rs");
    assert!(factory.is_file(), "{} is missing", factory.display());
    let hits = sim_hits(&factory);
    assert!(
        !hits.is_empty(),
        "the scanner found NO Sim token in eigen-station's lib.rs, the one file that \
         must have them - the matcher or the comment stripper is broken, and the main \
         lint's green means nothing"
    );
    assert!(
        hits.iter().any(|(_, l)| l.contains("SimConfig")),
        "eigen-station builds the Sim through SimConfig; the scanner did not see it: {hits:?}"
    );

    // ... and the file the anchor moved OFF of is now clean, which is the factory-lift
    // claim stated as an assertion rather than as prose. `factory.rs` is scanned by the
    // main lint like any other app file now (UI_SIM_FILES is empty), so this is a
    // second, independent statement of the same fact. It binds whenever the app is in
    // the tree (see [`app_present`]); a tree without the app has no app factory.
    if app_present(&root) {
        let tauri_factory = root
            .join("ui")
            .join("src-tauri")
            .join("src")
            .join("factory.rs");
        assert!(
            tauri_factory.is_file(),
            "{} is missing",
            tauri_factory.display()
        );
        assert!(
            sim_hits(&tauri_factory).is_empty(),
            "the Tauri factory wrapper names a Sim type again; the factory lift claimed it \
             does not, and the empty UI_SIM_FILES allowlist depends on that being true"
        );
    } else {
        eprintln!(
            "no ui/src-tauri/Cargo.toml in this tree: the app factory assertions do not apply"
        );
    }

    // And it correctly IGNORES a comment-only mention: the eigen-random engine's
    // module docs still discuss the retired SimBackend drive latch.
    let engine = root
        .join("crates")
        .join("eigen-random")
        .join("src")
        .join("service")
        .join("engine.rs");
    assert!(engine.is_file(), "{} is missing", engine.display());
    let raw = std::fs::read_to_string(&engine).expect("readable");
    assert!(
        raw.contains("SimBackend"),
        "the fixture for this assertion moved: engine.rs no longer mentions SimBackend \
         in prose, so this test no longer proves comments are ignored"
    );
    assert!(
        sim_hits(&engine).is_empty(),
        "a PROSE mention of SimBackend was counted as production code"
    );
}

/// The manifest half: `eigen-hal-sim` may only be a PRODUCTION dependency of the
/// allowlisted crates. Everywhere else it is dev-only.
///
/// This is what `cargo deny check bans` cannot say. Its ban graph includes
/// dev-dependencies, so its wrapper list has to name every crate that touches the
/// Sim at all.
///
/// RED-MUTATION: move `eigen-hal-sim` from `[dev-dependencies]` to `[dependencies]`
/// in `crates/eigen-srs/Cargo.toml` and this reds naming that crate.
#[test]
fn no_crate_outside_the_allowlist_depends_on_the_sim_in_production() {
    let root = repo_root();
    let mut manifests = Vec::new();
    for dir in ["crates", "modules", "ui"] {
        let mut all = Vec::new();
        walk(&root.join(dir), &mut all, "toml");
        manifests.extend(
            all.into_iter()
                .filter(|p| p.file_name().and_then(|s| s.to_str()) == Some("Cargo.toml")),
        );
    }
    assert!(
        manifests.len() > 20,
        "only {} manifests found; the walk is broken",
        manifests.len()
    );

    // Crates allowed to hold it in production: the Sim itself, the demo, and
    // `eigen-station` (the one construction point since the factory lift). The Tauri
    // app crate `ui` was on this list and is NOT any more - it dropped the dependency
    // entirely when `factory.rs` became a wrapper over `eigen-station`.
    let allowed: BTreeSet<&str> = SIM_CRATES.iter().copied().collect();

    let mut violations = Vec::new();
    let mut prod_holders = BTreeSet::new();
    for m in &manifests {
        let Ok(src) = std::fs::read_to_string(m) else {
            continue;
        };
        let name = src
            .lines()
            .find_map(|l| l.strip_prefix("name = "))
            .map(|v| v.trim().trim_matches('"').to_string())
            .unwrap_or_default();
        // Walk sections; only `[dependencies]` (and target-specific ones) count.
        let mut section = String::new();
        for line in src.lines() {
            let t = line.trim();
            if t.starts_with('[') && t.ends_with(']') {
                section = t.to_string();
                continue;
            }
            if !t.starts_with("eigen-hal-sim") {
                continue;
            }
            let is_prod = section == "[dependencies]" || section.ends_with(".dependencies]");
            if is_prod {
                prod_holders.insert(name.clone());
                if !allowed.contains(name.as_str()) {
                    violations.push(format!(
                        "{}: `{name}` holds eigen-hal-sim in {section}",
                        m.strip_prefix(&root).unwrap_or(m).display()
                    ));
                }
            }
        }
    }

    // The scan must have found the LEGITIMATE production holders, or it found
    // nothing and would pass no matter what.
    assert!(
        prod_holders.contains("eigen-demo") && prod_holders.contains("eigen-station"),
        "the manifest scan did not see the two known production holders \
         (eigen-demo, eigen-station); it saw {prod_holders:?} - the section parser is \
         broken and a green here means nothing"
    );
    // The factory-lift claim, as an assertion: the Tauri app crate no longer holds the Sim in
    // ANY dependency section. If it comes back, the lift regressed.
    assert!(
        !prod_holders.contains("ui"),
        "the Tauri app crate holds eigen-hal-sim in production again; the factory lift \
         moved the one construction point to eigen-station and `ui` should reach the \
         Sim only through it"
    );

    assert!(
        violations.is_empty(),
        "eigen-hal-sim is a PRODUCTION dependency outside the allowlist {allowed:?}. \
         Move it to [dev-dependencies], or take the backend from the station factory:\n  {}",
        violations.join("\n  ")
    );
}

/// The double's half of the code lint: no production code outside `eigen-hal` names
/// `ScriptedDelegate`.
///
/// The same documented hole as the Sim lint applies: an item behind a `cfg` predicate
/// that contains the word `test` (including a crate's own `test-support` feature) is
/// read as test code. The manifest half below bounds it for `eigen-hal`'s feature.
///
/// RED-MUTATION (run 2026-10-31, reverted): add
/// `let _ = eigen_hal::delegated::testing::ScriptedDelegate::new(caps);` to
/// `crates/eigen-srs/src/lib.rs` outside any `#[cfg(test)]` block and this reds with
/// that file and line.
#[test]
fn production_rust_never_names_the_delegated_test_double() {
    let root = repo_root();
    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files, "rs");
    walk(&root.join("modules"), &mut files, "rs");
    walk(
        &root.join("ui").join("src-tauri").join("src"),
        &mut files,
        "rs",
    );
    let mut scanned = 0usize;
    let mut leaks = Vec::new();
    for f in &files {
        if is_test_path(f) {
            continue;
        }
        if let Some(krate) = owning_crate_dir(&root, f) {
            if DOUBLE_CRATES.contains(&krate.as_str()) {
                continue;
            }
        }
        scanned += 1;
        for (line, text) in token_hits(f, DOUBLE_TOKENS) {
            leaks.push(format!(
                "{}:{line}: {text}",
                f.strip_prefix(&root).unwrap_or(f).display()
            ));
        }
    }
    assert!(
        scanned > 100,
        "the scanner only looked at {scanned} production files; it is not walking the tree"
    );
    // The matcher can see the double: its own definition file names it in code.
    let def = root
        .join("crates")
        .join("eigen-hal")
        .join("src")
        .join("delegated")
        .join("testing.rs");
    assert!(def.is_file(), "{} is missing", def.display());
    assert!(
        token_hits(&def, DOUBLE_TOKENS)
            .iter()
            .any(|(_, l)| l.contains("pub struct ScriptedDelegate")),
        "the scanner did not see ScriptedDelegate in its own definition; the matcher is \
         broken and the green above means nothing"
    );
    assert!(
        leaks.is_empty(),
        "production code names the delegated-control test double outside {DOUBLE_CRATES:?}:\n  {}",
        leaks.join("\n  ")
    );
}

/// The double's half of the manifest lint: no production `[dependencies]` entry
/// enables `eigen-hal/test-support`.
///
/// RED-MUTATION (run 2026-10-31, reverted): change `crates/eigen-srs/Cargo.toml`'s
/// `eigen-hal = { path = "../eigen-hal" }` to carry `features = ["test-support"]` and
/// this reds naming that manifest.
#[test]
fn no_crate_enables_the_delegated_test_double_in_production() {
    // The parser can fire, and it tells the sections apart.
    let inline =
        "[dependencies]\neigen-hal = { path = \"../eigen-hal\", features = [\"test-support\"] }\n";
    let table =
        "[dependencies.eigen-hal]\npath = \"../eigen-hal\"\nfeatures = [\"test-support\"]\n";
    let dev = "[dev-dependencies]\neigen-hal = { path = \"../eigen-hal\", features = [\"test-support\"] }\n";
    let own = "[features]\ntest-support = []\n";
    assert_eq!(
        test_support_in_production(inline).len(),
        1,
        "inline form missed"
    );
    assert_eq!(
        test_support_in_production(table).len(),
        1,
        "table form missed"
    );
    assert!(
        test_support_in_production(dev).is_empty(),
        "a dev-dependency was counted"
    );
    assert!(
        test_support_in_production(own).is_empty(),
        "the feature's own definition was counted"
    );

    let root = repo_root();
    let mut manifests = Vec::new();
    for dir in ["crates", "modules", "ui"] {
        let mut all = Vec::new();
        walk(&root.join(dir), &mut all, "toml");
        manifests.extend(
            all.into_iter()
                .filter(|p| p.file_name().and_then(|s| s.to_str()) == Some("Cargo.toml")),
        );
    }
    assert!(
        manifests.len() > 20,
        "only {} manifests found; the walk is broken",
        manifests.len()
    );
    let mut violations = Vec::new();
    for m in &manifests {
        let Ok(src) = std::fs::read_to_string(m) else {
            continue;
        };
        for hit in test_support_in_production(&src) {
            violations.push(format!(
                "{}: {hit}",
                m.strip_prefix(&root).unwrap_or(m).display()
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "a production dependency enables eigen-hal/test-support (the delegated-control \
         test double); move it to [dev-dependencies]:\n  {}",
        violations.join("\n  ")
    );
}
