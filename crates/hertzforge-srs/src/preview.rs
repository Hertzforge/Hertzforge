//! OFFLINE SRS synthesis PREVIEW - target -> synthesize -> look at it, with no
//! hardware, no station, no service and no run.
//!
//! # Why this exists
//!
//! Every ingredient was already here: [`crate::iterate::synthesize_to_target`] is a
//! pure function of the component table and the sample rate, and the run service
//! calls it during `assess_feasibility` to build the reference the test intends the
//! control point to see. But that call sits INSIDE the service, behind a
//! [`crate::service::plant::PulsePlant`] - so the only way to see a synthesized
//! pulse was to configure a run against a fire path. An operator authoring a target
//! could not answer "what waveform does this spec actually ask for" without
//! standing up hardware (operator acceptance, 2026-08-09: "There seems to be no
//! controls to do the synthesis of a given SRS pulse?").
//!
//! [`synthesize_preview`] is that answer: the SAME seed table (one component per
//! band, [`crate::table::WaveletTable::default_over_band`] /
//! [`crate::table::DampedSineTable::default_over_band`]) and the SAME iteration the
//! service runs, called directly on an [`SrsTestConfig`].
//!
//! # What it is NOT (provenance)
//!
//! A preview is a COMPUTATION, never a measurement. Nothing here touches a plant,
//! a HAL, a capture or a channel table; the returned SRS is what the synthesized
//! DRIVE would produce through an ideal unity plant, not what any shaker did. The
//! caller is responsible for labeling it as such - the UI card that renders this is
//! titled SYNTHESIS PREVIEW and says so in prose. It must never be mistaken for a
//! fired pulse, and this module deliberately returns no seq, no timestamp and no
//! level, so there is nothing for a pulse-record display to latch onto by accident.
//!
//! # The rate
//!
//! The preview needs a sample rate the run has not adopted yet (SRS states no rate:
//! the test is authored as a target SPECTRUM and synthesized at whatever rate the
//! station programs). So `fs_hz` is an explicit argument, and the SAME rate check
//! the service applies at construction is applied here: `f_hi_hz` must sit below
//! Nyquist, or the preview REFUSES rather than returning a spectrum whose top bands
//! the rate could not carry.
//!
//! # Determinism
//!
//! No RNG, no time, no I/O. A given `(config, fs_hz)` always yields the same
//! waveform, so a preview is reproducible and a golden test can pin it.

use crate::iterate::{synthesize_to_target, SynthesisEngine};
use crate::service::config::{ConfigError, EngineKind, SrsTestConfig};
use crate::table::{DampedSineTable, WaveletTable};
use hertzforge_dsp::shock::{pulse_kinematic_series, pulse_kinematics};
use hertzforge_dsp::srs::srs_analyze;
use serde::{Deserialize, Serialize};

/// The peak quantities of a synthesized pulse (the three the operator asked to see
/// side by side), from the PURE no-high-pass integrator
/// [`hertzforge_dsp::shock::pulse_kinematics`] - the same quantities
/// `hertzforge_shock_common::PulseKinematicsRecord` carries for a FIRED pulse, so the
/// preview card and the fired-pulse card read in one vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PreviewKinematics {
    /// Peak absolute acceleration, g.
    pub peak_accel_g: f64,
    /// Peak absolute velocity, in/s.
    pub peak_velocity_ips: f64,
    /// Maximum (most positive) displacement, in.
    pub peak_disp_in: f64,
    /// Minimum (most negative) displacement, in.
    pub min_disp_in: f64,
    /// Peak-to-peak displacement - the STROKE the pulse demands, in.
    pub pk_pk_disp_in: f64,
    /// Velocity left at the last sample, in/s (~0 for a self-nulling waveform).
    pub residual_velocity_ips: f64,
    /// Displacement left at the last sample, in (~0 for a self-nulling waveform).
    pub residual_disp_in: f64,
}

/// The outcome of an offline synthesis preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SynthesisPreview {
    /// The rate the preview was computed at, Hz (an ARGUMENT, not a device echo).
    pub fs_hz: f64,
    /// Which engine ran ("Wavsyn" | "Seds").
    pub engine: String,
    /// The analysis band centers, Hz.
    pub band_freqs_hz: Vec<f64>,
    /// The target SRS resolved onto the band grid, g.
    pub target_g: Vec<f64>,
    /// The synthesized waveform's SRS on that grid, g.
    pub achieved_srs_g: Vec<f64>,
    /// Per-band error `20*log10(achieved/target)`, dB.
    pub per_band_error_db: Vec<f64>,
    /// Worst per-band `|error|`, dB. `None` when no band was comparable.
    pub worst_err_db: Option<f64>,
    /// Band index of `worst_err_db`.
    pub worst_band: Option<usize>,
    /// Outer-loop iterations performed.
    pub iters: usize,
    /// Whether the worst band reached `synthesis.tol_db`.
    pub converged: bool,
    /// The synthesized acceleration time history, g.
    pub waveform_g: Vec<f64>,
    /// Velocity at each sample, in/s (pure integrator).
    pub velocity_ips: Vec<f64>,
    /// Displacement at each sample, in (pure integrator).
    pub disp_in: Vec<f64>,
    /// Record duration, seconds.
    pub duration_s: f64,
    /// The peak readouts.
    pub kinematics: PreviewKinematics,
    /// The converged component frequencies, Hz (table order).
    pub component_freqs_hz: Vec<f64>,
    /// The converged per-component amplitudes, g (table order).
    pub component_amplitudes_g: Vec<f64>,
}

/// Run the offline SRS-matching iteration on `cfg` at `fs_hz` and return the
/// synthesized pulse with its achieved spectrum and kinematics.
///
/// This is a PURE function: same inputs, same output, no hardware and no service
/// state. See the module docs for the provenance rule (this is a computation, never
/// a measurement) and for why the rate is an argument.
///
/// # Refusals
///
/// - `fs_hz` not finite or `<= 0`.
/// - `cfg.f_hi_hz >= fs_hz / 2` - the SAME check
///   [`crate::service::SrsShockService::new`] applies to the fire path's rate. A
///   preview computed above Nyquist would report band levels the rate cannot
///   represent, which is worse than no preview.
/// - anything [`SrsTestConfig::validate`] rejects.
pub fn synthesize_preview(
    cfg: &SrsTestConfig,
    fs_hz: f64,
) -> Result<SynthesisPreview, ConfigError> {
    if !fs_hz.is_finite() || fs_hz <= 0.0 {
        return Err(ConfigError::BadConfig(format!(
            "preview sample rate must be finite and > 0 (got {fs_hz})"
        )));
    }
    cfg.validate()?;
    if cfg.f_hi_hz >= fs_hz / 2.0 {
        return Err(ConfigError::BadConfig(format!(
            "band top {:.1} Hz is at or above Nyquist for a {:.1} Hz preview rate \
             ({:.1} Hz); raise the preview rate or lower f_hi",
            cfg.f_hi_hz,
            fs_hz,
            fs_hz / 2.0
        )));
    }

    let band_freqs = cfg.band_freqs();
    let target = cfg.target_on_grid()?;

    // The SAME seed the service builds in `default_engine`: one component per band.
    let frac = cfg.fraction.into();
    let seed = match cfg.engine {
        EngineKind::Wavsyn => SynthesisEngine::Wavsyn(
            WaveletTable::default_over_band(cfg.f_lo_hz, cfg.f_hi_hz, frac)
                .map_err(|e| ConfigError::BadConfig(e.to_string()))?,
        ),
        EngineKind::Seds => SynthesisEngine::Seds(
            DampedSineTable::default_over_band(cfg.f_lo_hz, cfg.f_hi_hz, frac)
                .map_err(|e| ConfigError::BadConfig(e.to_string()))?,
        ),
    };

    let result = synthesize_to_target(&target, &band_freqs, fs_hz, seed, cfg.synthesis);

    // Score the returned waveform the same way the envelope check does, so the
    // preview's "worst band" is the same statistic the run reports.
    let mut worst_err_db: Option<f64> = None;
    let mut worst_band: Option<usize> = None;
    for (b, &err) in result.per_band_error_db.iter().enumerate() {
        let comparable = target.get(b).copied().unwrap_or(0.0) > 0.0
            && result.achieved_srs.get(b).copied().unwrap_or(0.0) > 0.0;
        if !comparable {
            continue;
        }
        if worst_err_db
            .map(|w: f64| err.abs() > w.abs())
            .unwrap_or(true)
        {
            worst_err_db = Some(err);
            worst_band = Some(b);
        }
    }

    let waveform = result.waveform;
    let series = pulse_kinematic_series(&waveform, fs_hz);
    let k = pulse_kinematics(&waveform, fs_hz);

    // Re-analyze rather than trusting the loop's last score? No: `achieved_srs` IS
    // the analysis of the returned waveform (the loop re-scores after its final
    // re-synthesis), and analyzing twice would burn the whole SDOF bank again for a
    // bit-identical answer.
    let achieved = result.achieved_srs;

    Ok(SynthesisPreview {
        fs_hz,
        engine: match cfg.engine {
            EngineKind::Wavsyn => "Wavsyn".to_string(),
            EngineKind::Seds => "Seds".to_string(),
        },
        band_freqs_hz: band_freqs,
        target_g: target,
        achieved_srs_g: achieved,
        per_band_error_db: result.per_band_error_db,
        worst_err_db,
        worst_band,
        iters: result.iters,
        converged: result.converged,
        duration_s: waveform.len() as f64 / fs_hz,
        velocity_ips: series.velocity_ips,
        disp_in: series.disp_in,
        kinematics: PreviewKinematics {
            peak_accel_g: k.peak_accel_g,
            peak_velocity_ips: k.peak_velocity_ips,
            peak_disp_in: k.peak_disp_in,
            min_disp_in: k.min_disp_in,
            pk_pk_disp_in: k.pk_pk_disp_in,
            residual_velocity_ips: k.residual_velocity_ips,
            residual_disp_in: k.residual_disp_in,
        },
        component_freqs_hz: result.components.component_freqs(),
        component_amplitudes_g: result.components.amplitudes(),
        waveform_g: waveform,
    })
}

/// Re-analyze an EXISTING acceleration record's SRS on a band grid - the preview's
/// companion for a stored pulse (an overlay, a report, a re-grid at a different Q).
///
/// Pure passthrough to [`hertzforge_dsp::srs::srs_analyze`]; it lives here so a caller
/// that already binds `hertzforge-srs` for the preview does not have to reach into
/// `hertzforge-dsp` separately for the other half of the same screen.
pub fn analyze_srs(accel_g: &[f64], fs_hz: f64, band_freqs_hz: &[f64], q: f64) -> Vec<f64> {
    srs_analyze(accel_g, fs_hz, band_freqs_hz, q)
}
