//! Provenance - the recipe every derived trace wears.
//!
//! The verified #1 practitioner complaint about this product category is
//! opaque processing chains ("no info is given on what is actually happening").
//! Insight's answer is structural: provenance is part of the
//! record, rendered verbatim in the inspector, and carried into exports.

use serde::{Deserialize, Serialize};

use crate::FnId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AvgMode {
    Linear,
    Exponential,
    PeakHold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Detrend {
    None,
    SegmentMean,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Provenance {
    /// Came in from a file.
    Imported {
        format: String,
        path: String,
        detail: String,
    },
    /// Welch-family spectral estimate computed inside Insight.
    Welch {
        window: String,
        nfft: usize,
        overlap_pct: u8,
        avg_mode: AvgMode,
        avg_count: usize,
        detrend: Detrend,
        from: Vec<FnId>,
    },
    /// Any other derivation (unit conversion, statistics extraction, err trace,
    /// regrid...). `op` is a stable verb, `params_json` its arguments.
    Derived {
        op: String,
        params_json: String,
        from: Vec<FnId>,
    },
    /// A trace promoted to a tolerance/limit role (the ObserVIEW pattern).
    Promoted {
        role: String,
        offset_db: f64,
        from: FnId,
    },
}

impl Provenance {
    /// The one-line recipe the UI renders under a trace name. Complete
    /// sentences are the inspector's job; this line is the always-visible
    /// breadcrumb.
    pub fn recipe_line(&self) -> String {
        match self {
            Provenance::Imported {
                format,
                path,
                detail,
            } => {
                let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
                if detail.is_empty() {
                    format!("Imported {format}: {file}")
                } else {
                    format!("Imported {format}: {file} ({detail})")
                }
            }
            Provenance::Welch {
                window,
                nfft,
                overlap_pct,
                avg_mode,
                avg_count,
                detrend,
                ..
            } => {
                let avg = match avg_mode {
                    AvgMode::Linear => format!("{avg_count} lin avg"),
                    AvgMode::Exponential => format!("exp avg ({avg_count})"),
                    AvgMode::PeakHold => format!("peak hold ({avg_count})"),
                };
                let dt = match detrend {
                    Detrend::None => "",
                    Detrend::SegmentMean => ", detrend mean",
                };
                format!("{window} {nfft}, {overlap_pct}% overlap, {avg}{dt}")
            }
            Provenance::Derived {
                op, params_json, ..
            } => {
                if params_json.is_empty() || params_json == "{}" {
                    op.clone()
                } else {
                    format!("{op} {params_json}")
                }
            }
            Provenance::Promoted {
                role, offset_db, ..
            } => {
                if *offset_db == 0.0 {
                    format!("Promoted to {role}")
                } else {
                    format!("Promoted to {role} ({offset_db:+.1} dB)")
                }
            }
        }
    }

    /// Parent records, for session-graph integrity checks.
    pub fn parents(&self) -> Vec<FnId> {
        match self {
            Provenance::Imported { .. } => vec![],
            Provenance::Welch { from, .. } | Provenance::Derived { from, .. } => from.clone(),
            Provenance::Promoted { from, .. } => vec![*from],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welch_recipe_reads_like_the_plan_example() {
        let p = Provenance::Welch {
            window: "Hann".into(),
            nfft: 4096,
            overlap_pct: 50,
            avg_mode: AvgMode::Linear,
            avg_count: 32,
            detrend: Detrend::SegmentMean,
            from: vec![FnId(3)],
        };
        assert_eq!(
            p.recipe_line(),
            "Hann 4096, 50% overlap, 32 lin avg, detrend mean"
        );
    }

    #[test]
    fn imported_shows_basename_only() {
        let p = Provenance::Imported {
            format: "uff".into(),
            path: "/a/b/run_012.unv".into(),
            detail: String::new(),
        };
        assert_eq!(p.recipe_line(), "Imported uff: run_012.unv");
    }

    #[test]
    fn promoted_carries_offset() {
        let p = Provenance::Promoted {
            role: "abort-hi".into(),
            offset_db: 6.0,
            from: FnId(9),
        };
        assert_eq!(p.recipe_line(), "Promoted to abort-hi (+6.0 dB)");
    }
}
