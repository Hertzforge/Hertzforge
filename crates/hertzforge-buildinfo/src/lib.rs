//! The CORE (Rust) build stamp.
//!
//! Why it exists: there was no Rust build stamp. The webview has had a
//! vite stamp for a while; the Rust half had nothing, so a running app could not say
//! which core it was. That matters most in exactly the situation where you cannot ask:
//! a bench session where a fix was built on one machine, copied to another, and the
//! symptom did not change.
//!
//! # What it is
//!
//! Four compile-time constants, embedded by `build.rs`:
//!
//! | Constant | Source | Absent |
//! |---|---|---|
//! | [`GIT_SHA`] | `git rev-parse --short HEAD` | `"unknown"` |
//! | [`GIT_DIRTY`] | `git status --porcelain` non-empty | `false` |
//! | [`BUILD_HOST`] | `hostname` / `%COMPUTERNAME%` (`build-host` feature) | `"unknown"` |
//! | [`BUILD_TARGET`] | cargo's `TARGET` | `"unknown"` |
//!
//! # The comparison rule (the part that is a decision, not a fact)
//!
//! [`BuildInfo`] rides the `hertzforge-protocol` `Hello` handshake. The version check
//! stays EXACT-MATCH on the protocol version `u16`; the stamp is **compared and
//! REPORTED, never enforced** - see [`StampComparison`]. Two dev machines building
//! the same protocol version legitimately differ in sha, dirtiness, host, and target
//! (a Mac front end talking to a Linux backend differs in all four by design), so a
//! stamp mismatch is a logged warning plus a field the UI can surface, not a refused
//! connection. Making it a refusal would break the V1 split deployment on day one.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Short git SHA of the commit this core was built from; `"unknown"` outside a git tree.
pub const GIT_SHA: &str = env!("HERTZFORGE_BUILD_SHA");

/// Whether the working tree had uncommitted TRACKED changes at build time.
/// `false` outside a git tree - absence of evidence is not a clean tree, and the
/// `"unknown"` sha alongside it says so.
pub const GIT_DIRTY: bool = matches!(env!("HERTZFORGE_BUILD_DIRTY").as_bytes(), [b'1']);

/// The machine this core was built on; `"unknown"` if it could not be determined, and
/// always `"unknown"` unless the crate was built with the `build-host` feature.
pub const BUILD_HOST: &str = env!("HERTZFORGE_BUILD_HOST");

/// The target triple this core was built for; `"unknown"` if cargo did not say.
pub const BUILD_TARGET: &str = env!("HERTZFORGE_BUILD_TARGET");

/// The core build stamp, as a serializable value: the `core_build_info` Tauri command
/// return AND the `Hello` handshake field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    /// Short git SHA, or `"unknown"`.
    pub sha: String,
    /// Uncommitted tracked changes at build time.
    pub dirty: bool,
    /// Build host, or `"unknown"`.
    pub host: String,
    /// Target triple, or `"unknown"`.
    pub target: String,
}

impl BuildInfo {
    /// True when nothing about this stamp is actually known (no git, no hostname) -
    /// the honest "do not draw conclusions from this" state.
    pub fn is_unknown(&self) -> bool {
        self.sha == "unknown"
    }

    /// Compare two stamps. See [`StampComparison`] and the module docs: the result is
    /// REPORTED, never used to refuse a connection.
    pub fn compare(&self, other: &BuildInfo) -> StampComparison {
        if self.is_unknown() || other.is_unknown() {
            StampComparison::Unknown
        } else if self.sha == other.sha && self.dirty == other.dirty {
            StampComparison::Match
        } else {
            StampComparison::Differs
        }
    }
}

impl fmt::Display for BuildInfo {
    /// `a1b2c3d-dirty @ linux-host (x86_64-unknown-linux-gnu)` - the one-line form for
    /// logs and the mismatch warning.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.sha)?;
        if self.dirty {
            write!(f, "-dirty")?;
        }
        write!(f, " @ {} ({})", self.host, self.target)
    }
}

/// The outcome of comparing two [`BuildInfo`] stamps.
///
/// Deliberately three-valued. A two-valued match/mismatch would force the
/// unknowable case (a build with no git tree) to be reported as a mismatch, which
/// would then be indistinguishable from a real one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StampComparison {
    /// Same commit and same dirty state. Host and target may still differ - that is
    /// normal for a Mac-UI / Linux-core split and is not a mismatch.
    Match,
    /// Different commit, or the same commit with one side dirty. WARN and surface;
    /// do NOT refuse.
    Differs,
    /// At least one side has no stamp to compare. Say so; claim nothing.
    Unknown,
}

impl fmt::Display for StampComparison {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StampComparison::Match => f.write_str("match"),
            StampComparison::Differs => f.write_str("differs"),
            StampComparison::Unknown => f.write_str("unknown"),
        }
    }
}

/// This build's stamp.
pub fn build_info() -> BuildInfo {
    BuildInfo {
        sha: GIT_SHA.to_string(),
        dirty: GIT_DIRTY,
        host: BUILD_HOST.to_string(),
        target: BUILD_TARGET.to_string(),
    }
}

/// The one-line stamp for logs: `core a1b2c3d-dirty @ host (target)`.
pub fn build_line() -> String {
    format!("core {}", build_info())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The constants exist and are non-empty on every build, including one with no
    /// git available. A blank stamp would read as a field that was never wired.
    #[test]
    fn the_stamp_is_always_populated() {
        assert!(!GIT_SHA.is_empty());
        assert!(!BUILD_HOST.is_empty());
        assert!(!BUILD_TARGET.is_empty());
        let info = build_info();
        assert_eq!(info.sha, GIT_SHA);
        assert_eq!(info.dirty, GIT_DIRTY);
        // The one-line form always names the sha.
        assert!(build_line().contains(GIT_SHA), "{}", build_line());
    }

    /// Without the `build-host` feature the build machine's name is never embedded:
    /// an open build cannot leak it, and the stamp says "unknown" rather than guessing.
    #[cfg(not(feature = "build-host"))]
    #[test]
    fn the_build_host_is_not_embedded_without_the_feature() {
        assert_eq!(BUILD_HOST, "unknown");
        assert_eq!(build_info().host, "unknown");
    }

    /// In THIS repository the build script really did find git - so the stamp is
    /// evidence, not a placeholder that happens to compile. Skipped honestly when the
    /// build genuinely had no git tree (a source-tarball build), because asserting a
    /// sha there would be asserting something false.
    #[test]
    fn the_sha_is_a_real_sha_when_git_was_available() {
        if GIT_SHA == "unknown" {
            return; // No git at build time: nothing to verify, and nothing claimed.
        }
        assert!(GIT_SHA.len() >= 7, "short sha too short: {GIT_SHA}");
        assert!(
            GIT_SHA.chars().all(|c| c.is_ascii_hexdigit()),
            "not a hex sha: {GIT_SHA}"
        );
    }

    /// The comparison rule: same sha + same dirtiness matches EVEN ACROSS hosts and
    /// targets (the V1 split is Mac UI + Linux core, and refusing that would be a
    /// self-inflicted outage).
    #[test]
    fn a_split_deployment_across_two_machines_still_matches() {
        let mac = BuildInfo {
            sha: "a1b2c3d".into(),
            dirty: false,
            host: "mac-host".into(),
            target: "aarch64-apple-darwin".into(),
        };
        let linux = BuildInfo {
            sha: "a1b2c3d".into(),
            dirty: false,
            host: "linux-host".into(),
            target: "x86_64-unknown-linux-gnu".into(),
        };
        assert_eq!(mac.compare(&linux), StampComparison::Match);
    }

    #[test]
    fn a_different_commit_or_a_dirty_side_differs() {
        let a = BuildInfo {
            sha: "a1b2c3d".into(),
            dirty: false,
            host: "h".into(),
            target: "t".into(),
        };
        let mut b = a.clone();
        b.sha = "9999999".into();
        assert_eq!(a.compare(&b), StampComparison::Differs);
        let mut c = a.clone();
        c.dirty = true;
        assert_eq!(
            a.compare(&c),
            StampComparison::Differs,
            "same commit, one side dirty: the binaries are NOT the same binary"
        );
    }

    /// The unknowable case is its own answer, never folded into `Differs`.
    #[test]
    fn an_unknown_stamp_reports_unknown_not_a_mismatch() {
        let known = BuildInfo {
            sha: "a1b2c3d".into(),
            dirty: false,
            host: "h".into(),
            target: "t".into(),
        };
        let unknown = BuildInfo {
            sha: "unknown".into(),
            dirty: false,
            host: "unknown".into(),
            target: "unknown".into(),
        };
        assert_eq!(known.compare(&unknown), StampComparison::Unknown);
        assert_eq!(unknown.compare(&known), StampComparison::Unknown);
        assert!(unknown.is_unknown());
    }

    #[test]
    fn the_display_line_carries_the_dirty_marker() {
        let dirty = BuildInfo {
            sha: "a1b2c3d".into(),
            dirty: true,
            host: "linux-host".into(),
            target: "x86_64-unknown-linux-gnu".into(),
        };
        let s = dirty.to_string();
        assert!(s.starts_with("a1b2c3d-dirty"), "{s}");
        assert!(s.contains("linux-host"), "{s}");
        assert!(s.contains("x86_64-unknown-linux-gnu"), "{s}");
    }

    #[test]
    fn build_info_round_trips_through_json_in_camel_case() {
        let info = build_info();
        let text = serde_json::to_string(&info).unwrap();
        let back: BuildInfo = serde_json::from_str(&text).unwrap();
        assert_eq!(back, info);
        // The Tauri command return + `Hello` field are read by TypeScript.
        assert!(text.contains("\"sha\""), "{text}");
        assert!(text.contains("\"dirty\""), "{text}");
    }
}
