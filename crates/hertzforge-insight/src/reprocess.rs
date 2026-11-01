//! Reprocessing policy: turn time histories into analysis functions with full
//! DAQ-parameter control. Validates parameters HERE so no kernel assert is
//! reachable, dispatches `hertzforge-dsp` kernels,
//! and stamps honest Welch provenance on every product.

use hertzforge_data::{
    provenance::{AvgMode, Detrend as ProvDetrend},
    units, Abscissa, FnId, FunctionKind, FunctionRecord, Ordinate, Provenance, SourceRef, Unit,
};
use hertzforge_dsp::{frf_ext, spectrum_offline, srs, stats, stats_ext};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowKind {
    Hann,
    Flattop,
    Rectangular,
    BlackmanHarris,
    Hamming,
    BlackmanExact,
    Kaiser9,
    Tukey10,
}

impl WindowKind {
    pub fn label(self) -> &'static str {
        match self {
            WindowKind::Hann => "Hann",
            WindowKind::Flattop => "Flat Top",
            WindowKind::Rectangular => "Rectangular",
            WindowKind::BlackmanHarris => "Blackman-Harris",
            WindowKind::Hamming => "Hamming",
            WindowKind::BlackmanExact => "Exact Blackman",
            WindowKind::Kaiser9 => "Kaiser (b=9)",
            WindowKind::Tukey10 => "Tukey (10%)",
        }
    }

    fn coefficients(self, nfft: usize) -> Vec<f64> {
        use hertzforge_dsp::{window, window_ext};
        match self {
            WindowKind::Hann => window::hann(nfft),
            WindowKind::Flattop => window_ext::flattop(nfft),
            WindowKind::Rectangular => window_ext::rectangular(nfft),
            WindowKind::BlackmanHarris => window_ext::blackman_harris(nfft),
            WindowKind::Hamming => window_ext::hamming(nfft),
            WindowKind::BlackmanExact => window_ext::blackman_exact(nfft),
            WindowKind::Kaiser9 => window_ext::kaiser(nfft, 9.0),
            WindowKind::Tukey10 => window_ext::tukey(nfft, 0.1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReprocessKind {
    Psd,
    LinearSpectrum,
    Autopower,
    /// FRF H1 + coherence between (reference, response) pairs.
    FrfH1,
    /// FRF H2 + coherence.
    FrfH2,
    Srs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReprocessParams {
    pub kind: ReprocessKind,
    pub window: WindowKind,
    pub nfft: usize,
    /// Overlap fraction [0, 0.95].
    pub overlap: f64,
    pub detrend: bool,
    /// FRF only: the reference (input) function id.
    pub reference: Option<u64>,
    /// SRS only: quality factor and points/octave.
    pub srs_q: f64,
    pub srs_ppo: u32,
}

impl Default for ReprocessParams {
    fn default() -> Self {
        ReprocessParams {
            kind: ReprocessKind::Psd,
            window: WindowKind::Hann,
            nfft: 4096,
            overlap: 0.5,
            detrend: true,
            reference: None,
            srs_q: 10.0,
            srs_ppo: 12,
        }
    }
}

fn validate(p: &ReprocessParams, n_samples: usize) -> Result<(), String> {
    if !(p.nfft >= 64 && p.nfft <= 1 << 22 && p.nfft.is_power_of_two()) {
        return Err(format!(
            "block size must be a power of two in 64..4M (got {})",
            p.nfft
        ));
    }
    if !(0.0..=0.95).contains(&p.overlap) {
        return Err(format!("overlap must be 0..0.95 (got {})", p.overlap));
    }
    if n_samples < p.nfft {
        return Err(format!(
            "record has {n_samples} samples - shorter than one {} block",
            p.nfft
        ));
    }
    if matches!(p.kind, ReprocessKind::Srs) && !(p.srs_q >= 1.0 && p.srs_q <= 200.0) {
        return Err(format!("SRS Q must be 1..200 (got {})", p.srs_q));
    }
    Ok(())
}

fn time_series(rec: &FunctionRecord) -> Result<(&[f64], f64), String> {
    if rec.kind != FunctionKind::TimeHistory {
        return Err(format!("'{}' is not a time history", rec.name));
    }
    let Abscissa::Even { step, .. } = rec.abscissa else {
        return Err(format!("'{}' is not evenly sampled", rec.name));
    };
    let Ordinate::Real(v) = &rec.ordinate else {
        return Err(format!("'{}' is complex", rec.name));
    };
    Ok((v, 1.0 / step))
}

fn welch_provenance(p: &ReprocessParams, segs: usize, from: Vec<FnId>) -> Provenance {
    Provenance::Welch {
        window: p.window.label().into(),
        nfft: p.nfft,
        overlap_pct: (p.overlap * 100.0).round() as u8,
        avg_mode: AvgMode::Linear,
        avg_count: segs,
        detrend: if p.detrend {
            ProvDetrend::SegmentMean
        } else {
            ProvDetrend::None
        },
        from,
    }
}

/// The result of one reprocess call.
pub struct ReprocessOut {
    pub records: Vec<FunctionRecord>,
    pub warnings: Vec<String>,
}

/// Reprocess one source (plus optional reference for FRF kinds) into analysis
/// functions. Ids are 0 (the session re-bases on adopt).
pub fn reprocess(
    source: &FunctionRecord,
    reference: Option<&FunctionRecord>,
    p: &ReprocessParams,
) -> Result<ReprocessOut, String> {
    let (x, fs) = time_series(source)?;
    validate(p, x.len())?;
    let w = p.window.coefficients(p.nfft);
    let detrend = if p.detrend {
        spectrum_offline::Detrend::SegmentMean
    } else {
        spectrum_offline::Detrend::None
    };
    let mut warnings = Vec::new();
    let base_unit = source.unit_ordinate.clone();
    let hz = units::unit("Hz").unwrap();
    let mk = |name: String,
              kind: FunctionKind,
              abscissa: Abscissa,
              ordinate: Ordinate,
              unit_ord: Unit,
              prov: Provenance| FunctionRecord {
        id: FnId(0),
        name,
        role: None,
        response: source.response,
        reference: reference.and_then(|r| r.response),
        kind,
        abscissa,
        ordinate,
        unit_ordinate: unit_ord,
        unit_abscissa: hz.clone(),
        t0: source.t0,
        provenance: prov,
        source: SourceRef {
            path: "session".into(),
            format: "derived".into(),
            imported_at: 0,
        },
        comments: vec![],
    };

    let mut records = Vec::new();
    match p.kind {
        ReprocessKind::Psd | ReprocessKind::LinearSpectrum | ReprocessKind::Autopower => {
            let out = match p.kind {
                ReprocessKind::Psd => {
                    spectrum_offline::psd_welch_with(x, fs, &w, p.overlap, detrend)
                }
                ReprocessKind::Autopower => {
                    spectrum_offline::autopower_welch_with(x, fs, &w, p.overlap, detrend)
                }
                _ => spectrum_offline::linear_spectrum_with(x, fs, &w, p.overlap, detrend),
            }
            .map_err(|e| format!("spectral estimation: {e}"))?;
            if out.segments == 0 {
                return Err("record shorter than one analysis block".into());
            }
            if out.segments < 4 {
                warnings.push(format!(
                    "only {} average(s) - spectral variance is high; consider a smaller block",
                    out.segments
                ));
            }
            let (kind, label) = match p.kind {
                ReprocessKind::Psd => (FunctionKind::Psd, "PSD"),
                ReprocessKind::Autopower => (FunctionKind::Autopower, "Autopower"),
                _ => (FunctionKind::LinearSpectrum, "Spectrum"),
            };
            let n = out.values.len();
            records.push(mk(
                format!("{label}: {}", source.name),
                kind,
                Abscissa::Even {
                    start: 0.0,
                    step: fs / p.nfft as f64,
                    n,
                },
                Ordinate::Real(out.values),
                base_unit,
                welch_provenance(p, out.segments, vec![source.id]),
            ));
        }
        ReprocessKind::FrfH1 | ReprocessKind::FrfH2 => {
            let refr = reference.ok_or("FRF needs a reference (input) function")?;
            let (rx, rfs) = time_series(refr)?;
            if (rfs - fs).abs() > fs * 1e-9 {
                return Err("reference and response sample rates differ".into());
            }
            let cs = frf_ext::cross_welch_with(rx, x, fs, &w, p.overlap, detrend)
                .map_err(|e| format!("cross-spectral estimation: {e}"))?;
            if cs.segments == 0 {
                return Err("records shorter than one analysis block".into());
            }
            let h = match p.kind {
                ReprocessKind::FrfH1 => cs.h1(),
                _ => cs.h2(),
            };
            let est = if matches!(p.kind, ReprocessKind::FrfH1) {
                "H1"
            } else {
                "H2"
            };
            let n = h.len();
            let df = fs / p.nfft as f64;
            records.push(mk(
                format!("FRF {est}: {} / {}", source.name, refr.name),
                FunctionKind::Frf,
                Abscissa::Even {
                    start: 0.0,
                    step: df,
                    n,
                },
                Ordinate::Complex(h),
                Unit::custom(
                    &format!("{}/{}", base_unit.label, refr.unit_ordinate.label),
                    hertzforge_data::Quantity::Dimensionless,
                    1.0,
                ),
                Provenance::Derived {
                    op: format!("frf-{}", est.to_lowercase()),
                    params_json: serde_json::json!({
                        "window": p.window.label(), "nfft": p.nfft,
                        "overlap": p.overlap, "averages": cs.segments,
                    })
                    .to_string(),
                    from: vec![refr.id, source.id],
                },
            ));
            // Coherence rides along; below-average-threshold WARNS, never
            // hides: the single-average coherence trap.
            let coh = cs.coherence();
            if cs.segments < 3 {
                warnings.push(format!(
                    "coherence from {} average(s) is not meaningful (1.0 by identity at 1 average) - shown, not trusted",
                    cs.segments
                ));
            }
            let n = coh.len();
            records.push(mk(
                format!("Coherence: {} / {}", source.name, refr.name),
                FunctionKind::Coherence,
                Abscissa::Even {
                    start: 0.0,
                    step: df,
                    n,
                },
                Ordinate::Real(coh),
                Unit::dimensionless(),
                Provenance::Derived {
                    op: "coherence".into(),
                    params_json: serde_json::json!({
                        "window": p.window.label(), "nfft": p.nfft,
                        "overlap": p.overlap, "averages": cs.segments,
                    })
                    .to_string(),
                    from: vec![refr.id, source.id],
                },
            ));
        }
        ReprocessKind::Srs => {
            let f_lo = 5.0f64.max(fs / 65536.0);
            let f_hi = fs / 4.0;
            let freqs = srs::octave_freqs(
                f_lo,
                f_hi,
                match p.srs_ppo {
                    1 => srs::OctaveFraction::Whole,
                    3 => srs::OctaveFraction::Third,
                    6 => srs::OctaveFraction::Sixth,
                    _ => srs::OctaveFraction::Twelfth,
                },
            );
            let vals = srs::srs_analyze(x, fs, &freqs, p.srs_q);
            records.push(mk(
                format!("SRS Q{}: {}", p.srs_q, source.name),
                FunctionKind::Srs,
                Abscissa::Explicit(freqs),
                Ordinate::Real(vals),
                base_unit,
                Provenance::Derived {
                    op: "srs".into(),
                    params_json: serde_json::json!({
                        "q": p.srs_q, "ppo": p.srs_ppo, "maximax": true,
                    })
                    .to_string(),
                    from: vec![source.id],
                },
            ));
        }
    }
    Ok(ReprocessOut { records, warnings })
}

/// Region statistics for the inspector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordStats {
    pub rms: f64,
    pub rms_ac: f64,
    pub peak: f64,
    pub crest: f64,
    pub kurtosis: f64,
    pub mean: f64,
    pub n: usize,
    /// For PSD-class functions over a band: gRMS (band-integrated rms).
    pub band_rms: Option<f64>,
}

pub fn statistics(rec: &FunctionRecord, band: Option<(f64, f64)>) -> Result<RecordStats, String> {
    let Ordinate::Real(v) = &rec.ordinate else {
        return Err("statistics on complex functions use magnitude traces".into());
    };
    let band_rms = if rec.kind.rebin_class() {
        let centers = rec.abscissa.values();
        let (lo, hi) = band.unwrap_or((
            rec.abscissa.first().unwrap_or(0.0),
            rec.abscissa.last().unwrap_or(0.0),
        ));
        rec.abscissa
            .edges()
            .and_then(|edges| stats_ext::band_rms_psd(&centers, v, &edges, lo, hi))
    } else {
        None
    };
    Ok(RecordStats {
        rms: stats::rms(v),
        rms_ac: stats_ext::rms_ac(v),
        peak: stats::peak(v),
        crest: stats_ext::crest(v),
        kurtosis: stats_ext::kurtosis(v),
        mean: stats::mean(v),
        n: v.len(),
        band_rms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::mk as mkrec;
    use hertzforge_data::Abscissa;

    fn time_rec(fs: f64, n: usize) -> FunctionRecord {
        let v: Vec<f64> = (0..n)
            .map(|i| {
                let t = i as f64 / fs;
                1.5 * (2.0 * std::f64::consts::PI * 100.0 * t).sin()
                    + 0.3 * (2.0 * std::f64::consts::PI * 317.0 * t).sin()
            })
            .collect();
        let mut r = mkrec(
            FunctionKind::TimeHistory,
            Abscissa::Even {
                start: 0.0,
                step: 1.0 / fs,
                n,
            },
            v,
            "shaker head 101X+",
        );
        r.unit_abscissa = units::unit("s").unwrap();
        r
    }

    #[test]
    fn psd_reprocess_grms_matches_time_rms() {
        let fs = 8192.0;
        let src = time_rec(fs, 1 << 16);
        let p = ReprocessParams {
            nfft: 4096,
            ..Default::default()
        };
        let out = reprocess(&src, None, &p).unwrap();
        assert_eq!(out.records.len(), 1);
        let psd = &out.records[0];
        assert_eq!(psd.kind, FunctionKind::Psd);
        assert!(psd.provenance.recipe_line().contains("Hann 4096"));
        let st = statistics(psd, None).unwrap();
        let Ordinate::Real(tv) = &src.ordinate else {
            panic!()
        };
        let time_rms = stats::rms(tv);
        let g = st.band_rms.expect("psd band rms");
        assert!(
            ((g - time_rms) / time_rms).abs() < 0.02,
            "gRMS {g} vs time rms {time_rms}"
        );
    }

    #[test]
    fn frf_h1_of_known_gain_and_coherence_warning_below_3() {
        let fs = 4096.0;
        let n = 4096 * 8;
        let refr = time_rec(fs, n);
        let mut resp = refr.clone();
        if let Ordinate::Real(v) = &mut resp.ordinate {
            for x in v.iter_mut() {
                *x *= 2.0;
            }
        }
        resp.name = "response 5Z+".into();
        let p = ReprocessParams {
            kind: ReprocessKind::FrfH1,
            nfft: 2048,
            overlap: 0.5,
            ..Default::default()
        };
        let out = reprocess(&resp, Some(&refr), &p).unwrap();
        assert_eq!(out.records.len(), 2);
        let frf = &out.records[0];
        let Ordinate::Complex(h) = &frf.ordinate else {
            panic!()
        };
        // Pure gain 2.0 at the excited lines; check the 100 Hz bin.
        let df = fs / 2048.0;
        let k = (100.0 / df).round() as usize;
        let mag = (h[k].0 * h[k].0 + h[k].1 * h[k].1).sqrt();
        assert!((mag - 2.0).abs() < 0.05, "gain {mag}");
        // Short record -> few averages warning path.
        let short = time_rec(fs, 2048);
        let out2 = reprocess(
            &{
                let mut s = short.clone();
                s.name = "short resp".into();
                s
            },
            Some(&short),
            &ReprocessParams {
                kind: ReprocessKind::FrfH1,
                nfft: 2048,
                overlap: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(out2.warnings.iter().any(|w| w.contains("not meaningful")));
    }

    #[test]
    fn validation_refuses_bad_params_before_any_kernel() {
        let src = time_rec(2048.0, 8192);
        for p in [
            ReprocessParams {
                nfft: 1000,
                ..Default::default()
            },
            ReprocessParams {
                nfft: 32,
                ..Default::default()
            },
            ReprocessParams {
                overlap: 0.99,
                ..Default::default()
            },
            ReprocessParams {
                kind: ReprocessKind::Srs,
                srs_q: 0.5,
                ..Default::default()
            },
        ] {
            assert!(reprocess(&src, None, &p).is_err(), "{p:?}");
        }
    }

    #[test]
    fn srs_carries_q_in_provenance_for_comparability() {
        let src = time_rec(8192.0, 16384);
        let p = ReprocessParams {
            kind: ReprocessKind::Srs,
            srs_q: 10.0,
            ..Default::default()
        };
        let out = reprocess(&src, None, &p).unwrap();
        let srs = &out.records[0];
        assert_eq!(srs.kind, FunctionKind::Srs);
        match &srs.provenance {
            Provenance::Derived { params_json, .. } => assert!(params_json.contains("\"q\":10.0")),
            _ => panic!(),
        }
        // The SRS grid is log-spaced: edges must take the log branch.
        assert_eq!(srs.abscissa.spacing(), hertzforge_data::GridSpacing::Log);
    }
}
