//! The daemon's RUN PROVENANCE block - "what IS this data", for a remote operator.
//!
//! # Why this is a port and not an import
//!
//! `ui/src-tauri/src/provenance.rs` does the same job for the Tauri app, and its
//! `RunProvenance` is the shape the Svelte UI already renders. It lives in the app
//! crate (it emits Tauri events and holds an `AppHandle`), so the daemon cannot
//! import it. The FIELD SHAPE is mirrored here deliberately, in camelCase, so ONE
//! renderer displays both sources - the alternative is a remote-mode provenance panel
//! that looks different from the local one, which teaches an operator that provenance
//! is a presentation detail.
//!
//! Kept aligned by review, like the other mirrors in this system
//! (`hertzforge_protocol::CoreStamp` mirrors `hertzforge_buildinfo::BuildInfo`;
//! `hertzforge_protocol::caps` mirrors `hertzforge_hal::Caps`). It crosses the wire as an opaque
//! `Doc` at `hertzforge_protocol::PROVENANCE_SCHEMA`, so a shape change is a schema bump
//! and a typed refusal rather than a silent misread.

use std::sync::atomic::{AtomicU64, Ordering};

use hertzforge_station::{ProvenanceBasis, StationBackend};
use serde::Serialize;

/// Process-monotonic run ids. `0` is reserved for "unattributed".
static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(1);

/// Mint a run id.
pub fn mint_run_id() -> u64 {
    NEXT_RUN_ID.fetch_add(1, Ordering::SeqCst)
}

/// The provenance of one run.
///
/// Field-for-field the Tauri app's `RunProvenance`, minus the fields that only exist
/// in a windowed app (`identity_drift`, which is the app's configure-time-vs-run-time
/// re-read; the daemon builds its station immediately before the run and has no such
/// window).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunProvenance {
    /// The run id AS A STRING - a u64 loses precision crossing into a JS number, and
    /// a run id that silently changes value in the UI is worse than no run id.
    pub run_id: String,
    /// Which module produced it.
    pub module: String,
    /// `HERTZFORGE-SIM` / `NI-SIM (driver-simulated)` / `NI-REAL`.
    pub station_label: String,
    /// The backend implementation behind it.
    pub backend_kind: String,
    /// The bound device ids. EMPTY for HERTZFORGE-SIM: a plant is not a device.
    pub device_ids: Vec<String>,
    /// Serials, index-aligned with `device_ids`.
    pub serials: Vec<String>,
    /// What the DRIVER says. `None` = no driver to ask (HERTZFORGE-SIM).
    pub driver_simulated: Option<bool>,
    /// `driver_simulated == Some(true)` - the amber badge.
    pub simulated_badge: bool,
    /// The core build that produced the run.
    pub core_build: String,
    /// Unix seconds at run start.
    pub started_unix: u64,
    /// The peer that was holding the run lease when this run started, or `""` when
    /// the run was started locally (the soak harness).
    ///
    /// **NOT in the Tauri app's version, and it is the reason this type is not a
    /// straight copy.** The split deployment needs a run lease with its owner in
    /// provenance: there, "who started this run" is a fact about the run that the run
    /// report has to carry, and a single-operator desktop app never had to answer it.
    pub lease_owner: String,
}

impl RunProvenance {
    /// Assemble from a freshly built station.
    pub fn assemble(
        module: &str,
        run_id: u64,
        station: &StationBackend,
        lease_owner: &str,
        now_unix: u64,
    ) -> Self {
        let b: &ProvenanceBasis = &station.provenance;
        RunProvenance {
            run_id: run_id.to_string(),
            module: module.to_string(),
            station_label: b.station_label.clone(),
            backend_kind: b.backend_kind.to_string(),
            device_ids: b.device_ids.clone(),
            serials: b.serials.clone(),
            driver_simulated: b.driver_simulated,
            simulated_badge: b.driver_simulated == Some(true),
            core_build: b.core_build.to_string(),
            started_unix: now_unix,
            lease_owner: lease_owner.to_string(),
        }
    }

    /// The one-line log form.
    pub fn log_line(&self) -> String {
        format!(
            "run {} [{}] on {} via {} | core {} | lease {}",
            self.run_id,
            self.module,
            self.station_label,
            self.backend_kind,
            self.core_build,
            if self.lease_owner.is_empty() {
                "(local)"
            } else {
                &self.lease_owner
            }
        )
    }

    /// The JSON this rides the wire as, and the same string a capture sidecar stores
    /// as its `run_config_json` (which is what makes the capture LIST verb able to
    /// report a station and a run id at all).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Unix seconds now.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_station::{try_build_station_backend, BackendKind, StationSpec};

    #[test]
    fn a_sim_run_reports_hertzforge_sim_with_no_devices_and_no_badge() {
        let st = try_build_station_backend(BackendKind::Sim, &StationSpec::default()).unwrap();
        let p = RunProvenance::assemble("recorder", 7, &st, "10.0.0.2:5000", 1_753_400_000);
        assert_eq!(p.run_id, "7", "the id crosses as a STRING");
        assert_eq!(p.station_label, "HERTZFORGE-SIM");
        assert!(p.device_ids.is_empty(), "a plant is not a device");
        assert_eq!(p.driver_simulated, None, "there is no driver to ask");
        assert!(!p.simulated_badge, "HERTZFORGE-SIM is not the amber badge");
        assert_eq!(p.lease_owner, "10.0.0.2:5000");
        assert!(!p.core_build.is_empty());

        // The wire form is camelCase, matching the Tauri app's block field for field
        // so ONE renderer shows both.
        let json = p.to_json();
        for key in [
            "\"runId\"",
            "\"stationLabel\"",
            "\"driverSimulated\"",
            "\"simulatedBadge\"",
            "\"coreBuild\"",
            "\"leaseOwner\"",
        ] {
            assert!(json.contains(key), "{key} missing from {json}");
        }
    }

    #[test]
    fn run_ids_are_monotonic_and_never_zero() {
        let a = mint_run_id();
        let b = mint_run_id();
        assert!(a >= 1, "0 is reserved for unattributed");
        assert!(b > a);
    }

    #[test]
    fn a_locally_started_run_says_so_rather_than_naming_an_empty_peer() {
        let st = try_build_station_backend(BackendKind::Sim, &StationSpec::default()).unwrap();
        let p = RunProvenance::assemble("recorder", 1, &st, "", 0);
        assert!(p.log_line().contains("(local)"), "{}", p.log_line());
    }
}
