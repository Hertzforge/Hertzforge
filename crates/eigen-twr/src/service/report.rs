//! The TWR run report + snapshot payload (the house report-data shape: config
//! echo, honesty section, sysid summary, iteration table, final metrics,
//! events, metadata).

use serde::{Deserialize, Serialize};

use crate::limits::FeasibilityReport;
use crate::metrics::{ConvergenceHistory, ErrorMetrics};
use crate::player::{ReferencePlayer, TwrPath, TwrPathCapabilities};
use crate::service::config::MetaField;
use crate::service::{TwrService, TwrState, TwrStatus};

/// The honesty section every report carries verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Honesty {
    pub path: TwrPath,
    pub capabilities: TwrPathCapabilities,
    /// The acceptance statement - Sim-validated, real delegated drive pending.
    pub note: String,
}

/// Sysid summary for the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SysIdSummary {
    pub nfft: usize,
    pub fs_hz: f64,
    pub in_band_mean_coherence: f64,
    pub band_lo_hz: f64,
    pub band_hi_hz: f64,
    pub tikhonov_eps: f64,
    pub hmax_inband: f64,
}

/// The assembled run report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TwrReport {
    pub state: TwrState,
    pub status: TwrStatus,
    pub honesty: Honesty,
    /// Reference identity: label, rate, length, duration.
    pub reference_label: String,
    pub reference_fs_hz: f64,
    pub reference_samples: usize,
    pub section_len: usize,
    pub grid_n: usize,
    pub feasibility: FeasibilityReport,
    pub override_ack: Option<String>,
    pub sysid: Option<SysIdSummary>,
    /// The full convergence table (the acceptance evidence).
    pub history: ConvergenceHistory,
    /// Final (last) metrics, if any play happened.
    pub final_metrics: Option<ErrorMetrics>,
    pub events: Vec<String>,
    pub metadata: Vec<MetaField>,
    /// Serialized config echo (setups round-trip).
    pub config_json: String,
}

impl TwrReport {
    pub(crate) fn assemble<P: ReferencePlayer>(svc: &TwrService<P>) -> TwrReport {
        let status = svc.status();
        let path = svc.player_path();
        let sysid = svc.sysid.as_ref().map(|s| SysIdSummary {
            nfft: s.nfft,
            fs_hz: s.fs_hz,
            in_band_mean_coherence: s.in_band_mean_coherence,
            band_lo_hz: svc.config().sysid.f_lo_hz,
            band_hi_hz: svc.config().sysid.f_hi_hz,
            tikhonov_eps: svc.inv.as_ref().map(|i| i.eps).unwrap_or(0.0),
            hmax_inband: svc.inv.as_ref().map(|i| i.hmax_inband).unwrap_or(0.0),
        });
        TwrReport {
            state: svc.state(),
            honesty: Honesty {
                path,
                capabilities: TwrPathCapabilities::for_path(path),
                // The SAME derived sentence the live banner carries, so the
                // report and the panel cannot drift apart. It used to be a
                // second hardcoded copy naming a station the run might not be
                // on and a gap that was no longer the live one.
                note: status.path_label.clone(),
            },
            reference_label: svc.config().reference.label.clone(),
            reference_fs_hz: svc.prepared_reference().fs_hz,
            reference_samples: svc.prepared_reference().samples.len(),
            section_len: svc.grid().section_len,
            grid_n: svc.grid().n,
            feasibility: svc.feasibility().clone(),
            override_ack: svc.override_ack().map(|s| s.to_string()),
            sysid,
            history: svc.history().clone(),
            final_metrics: status.last_metrics,
            events: svc.events().to_vec(),
            metadata: svc.config().metadata.clone(),
            config_json: serde_json::to_string(svc.config()).unwrap_or_default(),
            status,
        }
    }
}

/// The snapshot payload ([`eigen_capture::Snapshot`] seam).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TwrSnapshotPayload {
    pub status: TwrStatus,
    pub history: ConvergenceHistory,
}
