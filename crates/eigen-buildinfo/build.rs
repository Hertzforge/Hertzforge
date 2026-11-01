//! Embed the CORE build stamp.
//!
//! Emits four `rustc-env` values consumed by `src/lib.rs`:
//! `EIGEN_BUILD_SHA`, `EIGEN_BUILD_DIRTY`, `EIGEN_BUILD_HOST`, `EIGEN_BUILD_TARGET`.
//!
//! `EIGEN_BUILD_HOST` is probed only with the `build-host` feature; without it the
//! build machine's name is never read, and the value is `"unknown"`.
//!
//! # Rules this script obeys
//!
//! 1. **It never fails the build.** Every probe degrades to `"unknown"` (or `"0"` for
//!    the dirty flag): a source tarball, a vendored build, a CI image without `git`,
//!    or a `PATH` without `hostname` must all still compile. A build stamp is a
//!    diagnostic, and a diagnostic that can break the build is a liability.
//! 2. **`std::process::Command` only.** No `git2`, no `hostname` crate, nothing added
//!    to the dependency tree or the license audit for a string.
//! 3. **"unknown" is an honest answer, never a fabricated one.** An absent git tree
//!    reports `unknown`; it does not report the last sha someone happened to bake in.

use std::process::Command;

fn main() {
    // Re-stamp when HEAD moves. In a plain checkout `.git/HEAD` is the file; in a
    // git WORKTREE (how the agent sessions build) `.git` is a FILE pointing at the
    // real gitdir, so watch both and let the missing one be ignored.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git");
    // An escape hatch for forcing a re-stamp (dirty-flag refresh) without touching
    // sources: `EIGEN_BUILD_STAMP_NONCE=$(date +%s) cargo build`.
    println!("cargo:rerun-if-env-changed=EIGEN_BUILD_STAMP_NONCE");

    let sha = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());

    // DIRTY = the working tree has ANY modification, staged or not, tracked only
    // (`--untracked-files=no`): a stray scratch file is not a different build, an
    // edited source file is. Unknown-when-unknowable: with no git we cannot say the
    // tree is clean, so the flag stays 0 and the sha already says "unknown".
    let dirty = match git(&["status", "--porcelain", "--untracked-files=no"]) {
        Some(out) if !out.trim().is_empty() => "1",
        _ => "0",
    };

    // The hostname is read ONLY when the crate is built with `build-host`; cargo sets
    // CARGO_FEATURE_BUILD_HOST for the build script exactly then.
    let host = if std::env::var_os("CARGO_FEATURE_BUILD_HOST").is_some() {
        hostname().unwrap_or_else(|| "unknown".to_string())
    } else {
        "unknown".to_string()
    };
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=EIGEN_BUILD_SHA={sha}");
    println!("cargo:rustc-env=EIGEN_BUILD_DIRTY={dirty}");
    println!("cargo:rustc-env=EIGEN_BUILD_HOST={host}");
    println!("cargo:rustc-env=EIGEN_BUILD_TARGET={target}");
}

/// Run `git <args>` in the crate directory, returning trimmed stdout on a clean exit.
/// Any failure - binary missing, not a repository, non-zero status - is `None`.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    Some(s.trim().to_string())
}

/// Best-effort build host. `hostname(1)` exists on macOS and Linux; on Windows the
/// `COMPUTERNAME` environment variable is the equivalent. Neither present = unknown.
fn hostname() -> Option<String> {
    if let Ok(name) = std::env::var("COMPUTERNAME") {
        if !name.is_empty() {
            return Some(name);
        }
    }
    let out = Command::new("hostname").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}
