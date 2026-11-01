//! Capture + snapshots for the Explorer/SMART run. Every type here is device-neutral,
//! so the delegated and host-synthesized paths share the whole module.
//!
//! The mode-generic capture infrastructure lives in `hertzforge-capture`, so every module
//! shares it:
//! the streaming [`CaptureSink`], the [`CaptureConfig`] / [`disk_budget_max_seconds`]
//! disk budget, the [`CaptureSidecar`] JSON convention, the operator [`Mark`], and the
//! generic snapshot writer. This module RE-EXPORTS them at their original
//! `crate::capture::*` paths (and the delegated Explorer service re-exports THIS module) so
//! every existing caller - the service, the tests, and the downstream Tauri app -
//! compiles unchanged, and it adds the ONE module-specific piece: the snapshot payload.
//!
//! ## The snapshot payload
//!
//! `hertzforge-capture`'s [`Snapshot<P>`](hertzforge_capture::Snapshot) is generic over a
//! mode payload. This module's payload ([`SmartSnapshotPayload`]) carries the live
//! spectra, the resonance profile traces, and the frequency/drive estimates - the fields
//! that used to live inline on the pre-lift `Snapshot`. [`Snapshot`] is the specialized
//! alias (`Snapshot<SmartSnapshotPayload>`), and [`save_snapshot`] is the specialized
//! writer, so the surface a caller sees is exactly what it was before the lift.
//!
//! The `Smart` in the payload's name is the MODULE's identity (the SMART/Explorer test
//! module), not the crate's or the backend's. The same payload serves the delegated path and
//! the host-synthesized path; the name is kept verbatim so the move renames nothing.

use crate::analysis::{ProfileTrace, SpectrumFrame};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

// Re-export the mode-generic capture infrastructure at the original paths.
pub use hertzforge_capture::{
    default_data_root, disk_budget_max_seconds, CaptureConfig, CaptureSidecar, CaptureSink, Mark,
    DATA_ROOT_ENV, DEFAULT_CAPTURE_RATE_HZ, RAW_CAPTURE_RATE_HZ,
};

/// The SMART-specific snapshot payload: the live spectra, resonance profiles, and
/// host-side estimates captured at a moment in time. Carried in the `payload`
/// field of the generic [`hertzforge_capture::Snapshot`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartSnapshotPayload {
    /// The current live spectra, one per channel.
    pub spectra: Vec<SpectrumFrame>,
    /// The current resonance profile traces, one per channel.
    pub profiles: Vec<ProfileTrace>,
    /// The current frequency estimate (Hz).
    pub est_freq_hz: f64,
    /// The current drive estimate (device-internal EU).
    pub est_drive: f64,
}

/// A SMART snapshot bundle: the generic envelope (timestamp + settings + marks) plus the
/// SMART payload (spectra + profiles + estimates). This is the SMART specialization of
/// [`hertzforge_capture::Snapshot`].
pub type Snapshot = hertzforge_capture::Snapshot<SmartSnapshotPayload>;

/// Write a timestamped SMART snapshot bundle into `run_dir`. Thin SMART-specialized
/// wrapper over [`hertzforge_capture::save_snapshot`].
pub fn save_snapshot(run_dir: &Path, snapshot: &Snapshot) -> io::Result<PathBuf> {
    hertzforge_capture::save_snapshot(run_dir, snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::ProfileTrace;

    #[test]
    fn smart_snapshot_round_trips_through_json() {
        let tmp = std::env::temp_dir().join(format!(
            "hertzforge-explorer-host-snap-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let snap = Snapshot {
            unix_secs: 1_700_000_000,
            settings_json: "{\"band\":[20,2000]}".into(),
            marks: vec![Mark {
                ch: 1,
                freq_hz: 250.0,
                value: 3.2,
                label: "resonance".into(),
            }],
            payload: SmartSnapshotPayload {
                spectra: vec![],
                profiles: vec![ProfileTrace {
                    ch: 1,
                    points: vec![],
                }],
                est_freq_hz: 250.0,
                est_drive: 1.5,
            },
        };
        let path = save_snapshot(&tmp, &snap).unwrap();
        assert!(path.exists());
        let text = std::fs::read_to_string(&path).unwrap();
        let back: Snapshot = serde_json::from_str(&text).unwrap();
        assert_eq!(back.payload.est_freq_hz, 250.0);
        assert_eq!(back.marks.len(), 1);
        assert_eq!(back.marks[0].label, "resonance");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
