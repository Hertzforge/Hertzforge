//! Plot payload production.
//!
//! TWO SHAPES, and the pane dev-asserts the tag matches its type:
//! - `Native`: one function on its own grid (single-trace panes).
//! - `Aligned`: a compare set's series on ONE shared grid with SHARED
//!   decimation bucket boundaries (overlay panes NEVER receive mixed grids -
//!   that failure is made unrepresentable).
//!
//! Decimation is server-side above `DECIMATE_ABOVE` points (min/max envelope
//! via `hertzforge_dsp::decimator::minmax`); full-resolution summary stats ride
//! alongside regardless, so readouts never quote decimated values.

use hertzforge_data::{FunctionRecord, Ordinate};
use hertzforge_dsp::{decimator, stats};
use serde::{Deserialize, Serialize};

pub const DECIMATE_ABOVE: usize = 16_384;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesPayload {
    pub name: String,
    pub role: Option<String>,
    pub kind: String,
    pub unit: String,
    /// Full-resolution stats (never from decimated data).
    pub rms: f64,
    pub peak: f64,
    pub n_full: usize,
    /// Plot vectors: either `y` (full) or `mins`+`maxs` (decimated envelope).
    pub y: Option<Vec<f64>>,
    pub mins: Option<Vec<f64>>,
    pub maxs: Option<Vec<f64>>,
    /// Complex sources ship magnitude + phase lanes explicitly.
    pub phase_deg: Option<Vec<f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum TracePayload {
    Native {
        id: u64,
        x: Vec<f64>,
        unit_x: String,
        series: SeriesPayload,
        decimated: bool,
    },
    Aligned {
        set: String,
        x: Vec<f64>,
        unit_x: String,
        series: Vec<SeriesPayload>,
        decimated: bool,
    },
}

fn series_of(rec: &FunctionRecord, x_len: usize, decimate: bool) -> SeriesPayload {
    let (full, phase): (Vec<f64>, Option<Vec<f64>>) = match &rec.ordinate {
        Ordinate::Real(v) => (v.clone(), None),
        Ordinate::Complex(c) => (
            c.iter()
                .map(|(re, im)| (re * re + im * im).sqrt())
                .collect(),
            Some(
                c.iter()
                    .map(|(re, im)| im.atan2(*re).to_degrees())
                    .collect(),
            ),
        ),
    };
    let rms = stats::rms(&full);
    let peak = stats::peak(&full);
    let n_full = full.len();
    if decimate {
        let cols = DECIMATE_ABOVE / 8;
        let mm = decimator::minmax(&full, cols);
        SeriesPayload {
            name: rec.name.clone(),
            role: rec.role.clone(),
            kind: rec.kind.label().into(),
            unit: rec.unit_ordinate.label.clone(),
            rms,
            peak,
            n_full,
            y: None,
            mins: Some(mm.iter().map(|(a, _)| *a).collect()),
            maxs: Some(mm.iter().map(|(_, b)| *b).collect()),
            phase_deg: None, // phase lanes only ship full-res (FRFs are small)
        }
    } else {
        let _ = x_len;
        SeriesPayload {
            name: rec.name.clone(),
            role: rec.role.clone(),
            kind: rec.kind.label().into(),
            unit: rec.unit_ordinate.label.clone(),
            rms,
            peak,
            n_full,
            y: Some(full),
            mins: None,
            maxs: None,
            phase_deg: phase,
        }
    }
}

/// Native payload for one function.
pub fn native(id: u64, rec: &FunctionRecord) -> TracePayload {
    let n = rec.ordinate.len();
    let decimate = n > DECIMATE_ABOVE && !rec.ordinate.is_complex();
    let x = if decimate {
        // Envelope columns need a matching x: bucket centers over the span.
        let cols = DECIMATE_ABOVE / 8;
        let (lo, hi) = (
            rec.abscissa.first().unwrap_or(0.0),
            rec.abscissa.last().unwrap_or(0.0),
        );
        (0..cols)
            .map(|i| lo + (hi - lo) * (i as f64 + 0.5) / cols as f64)
            .collect()
    } else {
        rec.abscissa.values()
    };
    TracePayload::Native {
        id,
        unit_x: rec.unit_abscissa.label.clone(),
        series: series_of(rec, x.len(), decimate),
        x,
        decimated: decimate,
    }
}

/// Aligned payload: series ALREADY on one grid (the compare engine's output
/// or same-grid session functions). Errors when any member's length differs -
/// the caller aligns first, this function only refuses to lie.
pub fn aligned(
    set: &str,
    x: Vec<f64>,
    unit_x: &str,
    members: &[&FunctionRecord],
) -> Result<TracePayload, String> {
    for m in members {
        if m.ordinate.len() != x.len() {
            return Err(format!(
                "'{}' has {} points but the aligned grid has {} - align before payload",
                m.name,
                m.ordinate.len(),
                x.len()
            ));
        }
    }
    let decimate = x.len() > DECIMATE_ABOVE;
    if decimate {
        // SHARED buckets: decimate every series with the same column count so
        // envelopes stay column-aligned.
        let cols = DECIMATE_ABOVE / 8;
        let (lo, hi) = (x[0], x[x.len() - 1]);
        let bx: Vec<f64> = (0..cols)
            .map(|i| lo + (hi - lo) * (i as f64 + 0.5) / cols as f64)
            .collect();
        let series = members
            .iter()
            .map(|m| {
                let mut s = series_of(m, bx.len(), true);
                s.phase_deg = None;
                s
            })
            .collect();
        Ok(TracePayload::Aligned {
            set: set.into(),
            x: bx,
            unit_x: unit_x.into(),
            series,
            decimated: true,
        })
    } else {
        Ok(TracePayload::Aligned {
            set: set.into(),
            x,
            unit_x: unit_x.into(),
            series: members.iter().map(|m| series_of(m, 0, false)).collect(),
            decimated: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::mk;
    use hertzforge_data::{Abscissa, FunctionKind};

    #[test]
    fn big_native_traces_decimate_with_honest_stats() {
        let n = 200_000;
        let v: Vec<f64> = (0..n).map(|i| ((i as f64) * 0.01).sin() * 3.0).collect();
        let rec = mk(
            FunctionKind::TimeHistory,
            Abscissa::Even {
                start: 0.0,
                step: 1e-4,
                n,
            },
            v.clone(),
            "big",
        );
        let p = native(7, &rec);
        match p {
            TracePayload::Native {
                series,
                decimated,
                x,
                ..
            } => {
                assert!(decimated);
                assert!(series.y.is_none() && series.mins.is_some());
                assert_eq!(series.n_full, n);
                assert!((series.rms - hertzforge_dsp::stats::rms(&v)).abs() < 1e-12);
                assert_eq!(x.len(), series.mins.as_ref().unwrap().len());
            }
            _ => panic!("expected Native"),
        }
    }

    #[test]
    fn aligned_refuses_mismatched_lengths() {
        let a = mk(
            FunctionKind::Psd,
            Abscissa::Even {
                start: 0.0,
                step: 1.0,
                n: 8,
            },
            vec![1.0; 8],
            "a",
        );
        let b = mk(
            FunctionKind::Psd,
            Abscissa::Even {
                start: 0.0,
                step: 1.0,
                n: 6,
            },
            vec![1.0; 6],
            "b",
        );
        let x = a.abscissa.values();
        assert!(aligned("cs", x, "Hz", &[&a, &b]).is_err());
    }

    #[test]
    fn complex_native_ships_mag_plus_phase_lanes() {
        let mut rec = mk(
            FunctionKind::Frf,
            Abscissa::Even {
                start: 1.0,
                step: 1.0,
                n: 4,
            },
            vec![],
            "h",
        );
        rec.ordinate = Ordinate::Complex(vec![(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)]);
        let p = native(1, &rec);
        match p {
            TracePayload::Native { series, .. } => {
                let ph = series.phase_deg.unwrap();
                assert!((ph[1] - 90.0).abs() < 1e-9);
                let y = series.y.unwrap();
                assert!((y[2] - 1.0).abs() < 1e-12, "magnitude of (-1,0)");
            }
            _ => panic!(),
        }
    }
}
