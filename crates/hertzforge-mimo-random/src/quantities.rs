//! The display quantities of design 9.6, served in process.
//!
//! [`MimoRandomService::quantity`] answers one [`MimoQuantity`] with a
//! [`Series`]: one or more traces on the band's lines, each line with its bin
//! edges so a plot can draw it as steps. Nothing here decimates or encodes: the
//! wire adapter subscribes by quantity and index, calls this, and decimates to the
//! plot's width. A quantity that does not exist yet (before identification, before
//! the first judged loop) or an index out of range is `None`.
//!
//! Levels: `psd`, `ref`, `drive_psd`, `trace` and the cross spectra are at the
//! level the run is at (the reference and its tolerance lines scaled to it), so
//! the measured trace sits on the reference when the loop is converged.
//! `error_db` and the prediction are referred to full level.

use crate::guards::DriveFactors;
use crate::report::Prediction;
use crate::service::MimoRandomService;
use hertzforge_sdm::law::{DiagnosticData, DiagnosticValue, Tolerances};
use hertzforge_sdm::{HermitianStack, MatrixStack};
use serde::{Deserialize, Serialize};

/// A quantity a screen can subscribe to, by name and index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MimoQuantity {
    /// Control `i`'s PSD (control space).
    Psd(usize),
    /// The reference PSD of control `i` with its alarm and abort lines.
    Ref(usize),
    /// Coherence between controls `i` and `j`.
    Coh(usize, usize),
    /// Phase between controls `i` and `j`, degrees.
    Phase(usize, usize),
    /// Magnitude of the cross spectrum between controls `i` and `j`.
    CsdMag(usize, usize),
    /// Every control's error against the reference per line, dB, full level.
    ErrorDb,
    /// Every control's coherence error per line: the rms over its partners `j` of
    /// the measured squared coherence with `j` minus the reference's (the error
    /// heatmap's second mode, design 9.3). Zero on a test with one control.
    CohError,
    /// Drive `k`'s PSD (physical).
    DrivePsd(usize),
    /// Coherence between drives `k` and `l`.
    DriveCoh(usize, usize),
    /// The FRF the law sees, control `i` by law drive `k`: magnitude and phase.
    Frf(usize, usize),
    /// The FRF's singular values per line.
    Sv,
    /// The FRF's condition number per line.
    Cond,
    /// Control `i`'s multiple coherence (identification).
    Mcoh(usize),
    /// The change cap applied to drive `k` per line, dB (0 where it did not bind).
    Cap(usize),
    /// Each control-space DOF's rms over the band, against the reference's.
    DofRms,
    /// The sum of the control PSDs against the sum of the reference PSDs.
    Trace,
    /// The predicted rms of drive `k` against its ceiling.
    PredDriveRms(usize),
    /// The predicted error of control `i` per line, dB.
    PredErrorDb(usize),
    /// A law diagnostic by its declared key; `index` picks a control or a drive
    /// for the per-control and per-drive layouts.
    LawDiagnostic { key: String, index: Option<usize> },
}

/// One trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub label: String,
    pub y: Vec<f64>,
}

/// A quantity's answer: traces over `x`. For a quantity on the line grid `x` is
/// each band line's frequency and `edges` (one more than `x`) the bin edges for
/// stepped drawing; for a quantity over channels `x` is the index and `edges` is
/// empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub name: String,
    /// The unit of `y`.
    pub unit: String,
    pub x: Vec<f64>,
    pub edges: Vec<f64>,
    pub traces: Vec<Trace>,
}

/// What the quantities read from the service.
pub(crate) struct View<'a> {
    pub lines: usize,
    pub df_hz: f64,
    pub band: (usize, usize),
    pub n_r: usize,
    pub n_d: usize,
    pub level_db: f64,
    pub reference: &'a HermitianStack,
    pub tolerances: &'a Tolerances,
    pub response_full: Option<&'a HermitianStack>,
    pub error_lines: &'a [f64],
    pub drive_full: &'a HermitianStack,
    pub frf: Option<&'a MatrixStack>,
    pub coherence: &'a [f64],
    pub condition: &'a [f64],
    pub caps: Option<&'a DriveFactors>,
    pub prediction: Option<&'a Prediction>,
    pub diagnostics: &'a [DiagnosticValue],
}

/// An owned copy of what the quantities read, taken after a loop so a display can
/// be answered from another thread while the run computes the next loop.
#[derive(Debug, Clone)]
pub struct QuantitySnapshot {
    pub(crate) lines: usize,
    pub(crate) df_hz: f64,
    pub(crate) band: (usize, usize),
    pub(crate) n_r: usize,
    pub(crate) n_d: usize,
    pub(crate) level_db: f64,
    pub(crate) reference: std::sync::Arc<HermitianStack>,
    pub(crate) tolerances: std::sync::Arc<Tolerances>,
    pub(crate) achievable: Option<std::sync::Arc<HermitianStack>>,
    pub(crate) response_full: Option<HermitianStack>,
    pub(crate) error_lines: Vec<f64>,
    pub(crate) drive_full: HermitianStack,
    pub(crate) frf: Option<MatrixStack>,
    pub(crate) coherence: Vec<f64>,
    pub(crate) condition: Vec<f64>,
    pub(crate) caps: Option<DriveFactors>,
    pub(crate) prediction: Option<Prediction>,
    pub(crate) diagnostics: Vec<DiagnosticValue>,
}

impl QuantitySnapshot {
    fn view(&self) -> View<'_> {
        View {
            lines: self.lines,
            df_hz: self.df_hz,
            band: self.band,
            n_r: self.n_r,
            n_d: self.n_d,
            level_db: self.level_db,
            reference: &self.reference,
            tolerances: &self.tolerances,
            response_full: self.response_full.as_ref(),
            error_lines: &self.error_lines,
            drive_full: &self.drive_full,
            frf: self.frf.as_ref(),
            coherence: &self.coherence,
            condition: &self.condition,
            caps: self.caps.as_ref(),
            prediction: self.prediction.as_ref(),
            diagnostics: &self.diagnostics,
        }
    }

    /// The quantity `q`, exactly as [`MimoRandomService::quantity`] answered it
    /// when the snapshot was taken.
    pub fn quantity(&self, q: &MimoQuantity) -> Option<Series> {
        self.view().quantity(q)
    }

    /// The control-space response at full level, once a loop has been judged.
    pub fn response_full(&self) -> Option<&HermitianStack> {
        self.response_full.as_ref()
    }

    /// The reference at full level, in the control space.
    pub fn reference(&self) -> &HermitianStack {
        &self.reference
    }

    /// The achievable reference `P G_ref P^H` at full level, in the control space
    /// ([`MimoRandomService::achievable_reference`]).
    pub fn achievable(&self) -> Option<&HermitianStack> {
        self.achievable.as_deref()
    }

    /// The level the run was at, dB (not finite before the loop engages).
    pub fn level_db(&self) -> f64 {
        self.level_db
    }

    /// The controlled band, half-open, in lines.
    pub fn band(&self) -> (usize, usize) {
        self.band
    }

    /// Line spacing, Hz.
    pub fn df_hz(&self) -> f64 {
        self.df_hz
    }
}

impl View<'_> {
    fn grid(&self, name: &str, unit: &str, traces: Vec<Trace>) -> Series {
        let (lo, hi) = self.band;
        Series {
            name: name.into(),
            unit: unit.into(),
            x: (lo..hi).map(|k| k as f64 * self.df_hz).collect(),
            edges: (lo..=hi).map(|k| (k as f64 - 0.5) * self.df_hz).collect(),
            traces,
        }
    }

    fn band_of(&self, f: impl Fn(usize) -> f64) -> Vec<f64> {
        (self.band.0..self.band.1).map(f).collect()
    }

    fn level(&self) -> f64 {
        if self.level_db.is_finite() {
            10f64.powf(self.level_db / 10.0)
        } else {
            1.0
        }
    }
}

fn db(x: f64) -> f64 {
    if x > 0.0 {
        10.0 * x.log10()
    } else {
        f64::NAN
    }
}

/// Control `i`'s coherence error on `line`: the rms over every other control `j`
/// of `gamma^2_measured(i, j) - gamma^2_reference(i, j)`; 0 with no partner.
fn coherence_error(m: &HermitianStack, r: &HermitianStack, line: usize, i: usize) -> f64 {
    let n = m.n();
    if n < 2 {
        return 0.0;
    }
    let s: f64 = (0..n)
        .filter(|&j| j != i)
        .map(|j| {
            let e = coherence(m, line, i, j) - coherence(r, line, i, j);
            e * e
        })
        .sum();
    (s / (n - 1) as f64).sqrt()
}

fn coherence(g: &HermitianStack, line: usize, i: usize, j: usize) -> f64 {
    let d = g.get(line, i, i).re * g.get(line, j, j).re;
    if d > 0.0 {
        (g.get(line, i, j).norm_sqr() / d).min(1.0)
    } else {
        0.0
    }
}

impl MimoRandomService {
    /// The quantity `q`, or `None` when it does not exist yet or the index is out
    /// of range.
    pub fn quantity(&self, q: &MimoQuantity) -> Option<Series> {
        self.quantity_state().quantity(q)
    }

    /// An owned copy of what the quantities read now (see [`QuantitySnapshot`]).
    pub fn quantity_snapshot(&self) -> QuantitySnapshot {
        let v = self.quantity_state();
        QuantitySnapshot {
            lines: v.lines,
            df_hz: v.df_hz,
            band: v.band,
            n_r: v.n_r,
            n_d: v.n_d,
            level_db: v.level_db,
            reference: self.reference_shared(),
            tolerances: self.tolerances_shared(),
            achievable: self.achievable_shared(),
            response_full: v.response_full.cloned(),
            error_lines: v.error_lines.to_vec(),
            drive_full: v.drive_full.clone(),
            frf: v.frf.cloned(),
            coherence: v.coherence.to_vec(),
            condition: v.condition.to_vec(),
            caps: v.caps.cloned(),
            prediction: v.prediction.cloned(),
            diagnostics: v.diagnostics.to_vec(),
        }
    }
}

impl View<'_> {
    /// The quantity `q` of this view.
    fn quantity(&self, q: &MimoQuantity) -> Option<Series> {
        let v = self;
        let (n_r, n_d) = (v.n_r, v.n_d);
        let p = v.level();
        let t = |label: &str, y: Vec<f64>| Trace {
            label: label.into(),
            y,
        };
        match q {
            MimoQuantity::Psd(i) if *i < n_r => {
                let r = v.response_full?;
                Some(v.grid(
                    "psd",
                    "units^2/Hz",
                    vec![t("measured", v.band_of(|l| r.get(l, *i, *i).re * p))],
                ))
            }
            MimoQuantity::Ref(i) if *i < n_r => {
                let k = |l: usize| l * n_r + *i;
                let tol = v.tolerances;
                Some(v.grid(
                    "ref",
                    "units^2/Hz",
                    vec![
                        t(
                            "reference",
                            v.band_of(|l| v.reference.get(l, *i, *i).re * p),
                        ),
                        t("alarm upper", v.band_of(|l| tol.alarm_upper[k(l)] * p)),
                        t("alarm lower", v.band_of(|l| tol.alarm_lower[k(l)] * p)),
                        t("abort upper", v.band_of(|l| tol.abort_upper[k(l)] * p)),
                        t("abort lower", v.band_of(|l| tol.abort_lower[k(l)] * p)),
                    ],
                ))
            }
            MimoQuantity::Coh(i, j) if *i < n_r && *j < n_r => {
                let r = v.response_full?;
                Some(v.grid(
                    "coh",
                    "",
                    vec![
                        t("measured", v.band_of(|l| coherence(r, l, *i, *j))),
                        t(
                            "reference",
                            v.band_of(|l| coherence(v.reference, l, *i, *j)),
                        ),
                    ],
                ))
            }
            MimoQuantity::Phase(i, j) if *i < n_r && *j < n_r => {
                let r = v.response_full?;
                Some(v.grid(
                    "phase",
                    "deg",
                    vec![
                        t(
                            "measured",
                            v.band_of(|l| r.get(l, *i, *j).arg().to_degrees()),
                        ),
                        t(
                            "reference",
                            v.band_of(|l| v.reference.get(l, *i, *j).arg().to_degrees()),
                        ),
                    ],
                ))
            }
            MimoQuantity::CsdMag(i, j) if *i < n_r && *j < n_r => {
                let r = v.response_full?;
                Some(v.grid(
                    "csd_mag",
                    "units^2/Hz",
                    vec![
                        t("measured", v.band_of(|l| r.get(l, *i, *j).norm() * p)),
                        t(
                            "reference",
                            v.band_of(|l| v.reference.get(l, *i, *j).norm() * p),
                        ),
                    ],
                ))
            }
            MimoQuantity::ErrorDb => {
                if v.error_lines.is_empty() {
                    return None;
                }
                Some(
                    v.grid(
                        "error_db",
                        "dB",
                        (0..n_r)
                            .map(|i| t(&format!("{i}"), v.band_of(|l| v.error_lines[l * n_r + i])))
                            .collect(),
                    ),
                )
            }
            MimoQuantity::CohError => {
                let r = v.response_full?;
                let rows = (0..n_r)
                    .map(|i| {
                        t(
                            &format!("{i}"),
                            v.band_of(|l| coherence_error(r, v.reference, l, i)),
                        )
                    })
                    .collect();
                Some(v.grid("coh_error", "", rows))
            }
            MimoQuantity::DrivePsd(k) if *k < n_d => Some(v.grid(
                "drive_psd",
                "V^2/Hz",
                vec![t(
                    "drive",
                    v.band_of(|l| v.drive_full.get(l, *k, *k).re * p),
                )],
            )),
            MimoQuantity::DriveCoh(k, m) if *k < n_d && *m < n_d => Some(v.grid(
                "drive_coh",
                "",
                vec![t(
                    "drive",
                    v.band_of(|l| coherence(v.drive_full, l, *k, *m)),
                )],
            )),
            MimoQuantity::Frf(i, k) => {
                let h = v.frf?;
                if *i >= h.rows() || *k >= h.cols() {
                    return None;
                }
                Some(v.grid(
                    "frf",
                    "units/V, deg",
                    vec![
                        t("magnitude", v.band_of(|l| h.get(l, *i, *k).norm())),
                        t("phase", v.band_of(|l| h.get(l, *i, *k).arg().to_degrees())),
                    ],
                ))
            }
            MimoQuantity::Sv => {
                let h = v.frf?;
                let s = crate::pool::singular_values(h, v.band).ok()?;
                let kk = h.rows().min(h.cols());
                Some(
                    v.grid(
                        "sv",
                        "units/V",
                        (0..kk)
                            .map(|j| t(&format!("{j}"), v.band_of(|l| s[l * kk + j])))
                            .collect(),
                    ),
                )
            }
            MimoQuantity::Cond => {
                if v.condition.is_empty() {
                    return None;
                }
                Some(v.grid(
                    "cond",
                    "",
                    vec![t("condition", v.band_of(|l| v.condition[l]))],
                ))
            }
            MimoQuantity::Mcoh(i) if *i < n_r => {
                if v.coherence.is_empty() {
                    return None;
                }
                Some(v.grid(
                    "mcoh",
                    "",
                    vec![t(
                        "multiple coherence",
                        v.band_of(|l| v.coherence[l * n_r + *i]),
                    )],
                ))
            }
            MimoQuantity::Cap(k) if *k < n_d => {
                let c = v.caps?;
                Some(v.grid(
                    "cap",
                    "dB",
                    vec![t("cap", v.band_of(|l| db(c.factors[l * c.drives + *k])))],
                ))
            }
            MimoQuantity::DofRms => {
                let r = v.response_full?;
                let rms = |g: &HermitianStack, i: usize, s: f64| {
                    let sum: f64 = (v.band.0..v.band.1)
                        .map(|l| g.get(l, i, i).re.max(0.0))
                        .sum();
                    (sum * v.df_hz * s).sqrt()
                };
                Some(Series {
                    name: "dof_rms".into(),
                    unit: "units rms".into(),
                    x: (0..n_r).map(|i| i as f64).collect(),
                    edges: Vec::new(),
                    traces: vec![
                        t("measured", (0..n_r).map(|i| rms(r, i, p)).collect()),
                        t(
                            "reference",
                            (0..n_r).map(|i| rms(v.reference, i, p)).collect(),
                        ),
                    ],
                })
            }
            MimoQuantity::Trace => {
                let r = v.response_full?;
                Some(v.grid(
                    "trace",
                    "units^2/Hz",
                    vec![
                        t("measured", v.band_of(|l| r.trace(l) * p)),
                        t("reference", v.band_of(|l| v.reference.trace(l) * p)),
                    ],
                ))
            }
            MimoQuantity::PredDriveRms(k) if *k < n_d => {
                let pr = v.prediction?;
                Some(Series {
                    name: "pred_drive_rms".into(),
                    unit: "V rms".into(),
                    x: vec![*k as f64],
                    edges: Vec::new(),
                    traces: vec![
                        t("predicted", vec![pr.drive_rms[*k]]),
                        t("ceiling", vec![pr.ceilings[*k]]),
                    ],
                })
            }
            MimoQuantity::PredErrorDb(i) if *i < n_r => {
                let pr = v.prediction?;
                Some(Series {
                    name: "pred_error_db".into(),
                    unit: "dB".into(),
                    x: vec![*i as f64],
                    edges: Vec::new(),
                    traces: vec![t("predicted rms error", vec![pr.error_db[*i]])],
                })
            }
            MimoQuantity::LawDiagnostic { key, index } => {
                let d = v.diagnostics.iter().find(|d| &d.key == key)?;
                let pick = |vals: &[f64], width: usize, idx: Option<usize>| -> Option<Vec<f64>> {
                    let i = idx?;
                    if i >= width || vals.len() != v.lines * width {
                        return None;
                    }
                    Some(v.band_of(|l| vals[l * width + i]))
                };
                match &d.data {
                    DiagnosticData::Scalar(x) => Some(Series {
                        name: key.clone(),
                        unit: String::new(),
                        x: vec![0.0],
                        edges: Vec::new(),
                        traces: vec![t(key, vec![*x])],
                    }),
                    DiagnosticData::PerLine(vals) if vals.len() == v.lines => {
                        Some(v.grid(key, "", vec![t(key, v.band_of(|l| vals[l]))]))
                    }
                    DiagnosticData::PerControlLine(vals) => {
                        Some(v.grid(key, "", vec![t(key, pick(vals, n_r, *index)?)]))
                    }
                    DiagnosticData::PerDriveLine(vals) => {
                        let w = vals.len() / v.lines.max(1);
                        Some(v.grid(key, "", vec![t(key, pick(vals, w, *index)?)]))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

impl MimoRandomService {
    /// Every quantity name the service answers, for a screen's subscription list.
    pub fn quantity_names() -> &'static [&'static str] {
        &[
            "psd",
            "ref",
            "coh",
            "phase",
            "csd_mag",
            "error_db",
            "coh_error",
            "drive_psd",
            "drive_coh",
            "frf",
            "sv",
            "cond",
            "mcoh",
            "cap",
            "dof_rms",
            "trace",
            "pred_drive_rms",
            "pred_error_db",
            "law_diagnostic",
        ]
    }
}
