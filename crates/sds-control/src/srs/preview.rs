//! The offline synthesis and `srs.preview` (schema/PROTOCOL.md, "SRS Shock
//! Configuration"): the target on the band grid, the component table fitted to it
//! with the controller's own iteration, the synthesized pulse at full level and its
//! kinematics, and the views a screen draws them from. The shell never computes a
//! synthesis. A preview is a computation, never a measurement.

use super::config::{round, WireSynthesis};
use crate::codec::Arrays;
use crate::protocol::{num, opt_num};
use crate::shock::view::{time_axis, time_series};
use crate::views::{self, Axis, Role, Scale, Series};
use hertzforge_dsp::shock::{pulse_kinematic_series, pulse_kinematics};
use hertzforge_srs::iterate::{synthesize_to_target, SynthesisEngine};
use hertzforge_srs::service::config::EngineKind;
use hertzforge_srs::table::WaveletTable;
use hertzforge_srs::SrsTestConfig;
use serde_json::{json, Value};

/// A list of band centres for a text frame, Hz.
pub fn band_list(x: &[f64]) -> Value {
    Value::Array(x.iter().map(|f| num(round(*f, 4))).collect())
}

/// A list of SRS values for a text frame, g.
pub fn value_list(x: &[f64]) -> Value {
    Value::Array(x.iter().map(|g| num(round(*g, 4))).collect())
}

/// The offline fit of the component table to the target.
#[derive(Debug, Clone, PartialEq)]
pub struct SynthesisFacts {
    pub engine: &'static str,
    pub components: usize,
    pub iters: usize,
    pub converged: bool,
    pub worst_err_db: Option<f64>,
    pub duration_s: f64,
}

impl SynthesisFacts {
    pub fn to_json(&self) -> Value {
        json!({
            "engine": self.engine,
            "components": self.components,
            "iters": self.iters,
            "converged": self.converged,
            "worst_err_db": opt_num(self.worst_err_db.map(|x| round(x, 3))),
            "duration_s": num(round(self.duration_s, 5)),
        })
    }
}

/// The synthesized pulse at full level, g.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReferenceFacts {
    pub peak_g: f64,
    pub velocity_pk_ips: f64,
    pub displacement_pk_pk_in: f64,
    pub residual_velocity_ips: f64,
    pub residual_disp_in: f64,
}

impl ReferenceFacts {
    pub fn to_json(&self) -> Value {
        json!({
            "peak_g": num(round(self.peak_g, 4)),
            "velocity_pk_ips": num(round(self.velocity_pk_ips, 4)),
            "displacement_pk_pk_in": num(round(self.displacement_pk_pk_in, 6)),
            "residual_velocity_ips": num(round(self.residual_velocity_ips, 6)),
            "residual_disp_in": num(round(self.residual_disp_in, 6)),
        })
    }
}

/// One band's envelope as a configuration states it: `(alarm_lo, alarm_hi,
/// abort_lo, abort_hi)` dB, each a positive half-width.
pub type EnvelopeBand = (f64, f64, f64, f64);

/// An envelope's four edges around a target, g: alarm high and low, abort high
/// and low.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Edges {
    pub alarm_hi: Vec<f64>,
    pub alarm_lo: Vec<f64>,
    pub abort_hi: Vec<f64>,
    pub abort_lo: Vec<f64>,
}

/// The offline synthesis.
#[derive(Debug, Clone, PartialEq)]
pub struct Synthesis {
    pub fs_hz: f64,
    pub engine: EngineKind,
    pub bands_hz: Vec<f64>,
    pub target_g: Vec<f64>,
    /// The synthesized pulse's SRS on the band grid, g.
    pub achieved_g: Vec<f64>,
    pub iters: usize,
    pub converged: bool,
    pub worst_err_db: Option<f64>,
    /// The fitted table.
    pub components: SynthesisEngine,
    /// The synthesized pulse at full level, g.
    pub waveform_g: Vec<f64>,
    pub velocity_ips: Vec<f64>,
    pub disp_in: Vec<f64>,
    pub reference: ReferenceFacts,
}

impl Synthesis {
    pub fn facts(&self) -> SynthesisFacts {
        SynthesisFacts {
            engine: super::config::engine_name(self.engine),
            components: self.components.component_freqs().len(),
            iters: self.iters,
            converged: self.converged,
            worst_err_db: self.worst_err_db,
            duration_s: self.waveform_g.len() as f64 / self.fs_hz,
        }
    }

    pub fn reference_facts(&self) -> ReferenceFacts {
        self.reference
    }
}

/// The controller's configuration of the parts the offline synthesis reads.
fn synthesis_cfg(s: &WireSynthesis) -> SrsTestConfig {
    let mut cfg = SrsTestConfig::flat(s.bands.f_lo_hz, s.bands.f_hi_hz, 1.0);
    cfg.fraction = s.bands.fraction();
    cfg.q = s.bands.q;
    cfg.engine = s.engine;
    cfg.synthesis = s.opts();
    cfg.target = s.target.crate_target();
    cfg.components = s.components.as_ref().map(|c| c.engine_table());
    cfg
}

/// The table a synthesis starts from: the given one, or one component per band,
/// by the controller's own rule.
pub fn seed_table(s: &WireSynthesis) -> Option<SynthesisEngine> {
    synthesis_cfg(s).seed_table().ok()
}

/// The target on the band grid, by the controller's own interpolation.
pub fn target_on_grid(s: &WireSynthesis) -> Vec<f64> {
    synthesis_cfg(s).target_on_grid().unwrap_or_default()
}

/// Fit the table to the target offline at the configured rate. The fields must
/// have been checked ([`super::config::check_synthesis`]).
pub fn synthesize(s: &WireSynthesis) -> Synthesis {
    let bands = s.bands.centres();
    let target = target_on_grid(s);
    let seed = seed_table(s).unwrap_or_else(|| SynthesisEngine::Wavsyn(WaveletTable::new()));
    let result = synthesize_to_target(&target, &bands, s.fs_hz, seed, s.opts());
    let mut worst: Option<f64> = None;
    for (b, err) in result.per_band_error_db.iter().enumerate() {
        let comparable = target.get(b).copied().unwrap_or(0.0) > 0.0
            && result.achieved_srs.get(b).copied().unwrap_or(0.0) > 0.0;
        if comparable && worst.is_none_or(|w| err.abs() > w.abs()) {
            worst = Some(*err);
        }
    }
    let series = pulse_kinematic_series(&result.waveform, s.fs_hz);
    let k = pulse_kinematics(&result.waveform, s.fs_hz);
    Synthesis {
        fs_hz: s.fs_hz,
        engine: s.engine,
        bands_hz: bands,
        target_g: target,
        achieved_g: result.achieved_srs,
        iters: result.iters,
        converged: result.converged,
        worst_err_db: worst,
        components: result.components,
        waveform_g: result.waveform,
        velocity_ips: series.velocity_ips,
        disp_in: series.disp_in,
        reference: ReferenceFacts {
            peak_g: k.peak_accel_g,
            velocity_pk_ips: k.peak_velocity_ips,
            displacement_pk_pk_in: k.pk_pk_disp_in,
            residual_velocity_ips: k.residual_velocity_ips,
            residual_disp_in: k.residual_disp_in,
        },
    }
}

fn freq_axis() -> Axis {
    Axis::new("Natural frequency", Some("Hz"), Scale::Log)
}

fn srs_axis() -> Axis {
    Axis::new("Peak acceleration", Some("g"), Scale::Log)
}

fn time_axis_spec() -> Axis {
    Axis::new("Time", Some("s"), Scale::Linear)
}

/// The envelope's edges around `target`.
pub fn envelope_edges(target: &[f64], env: &[EnvelopeBand]) -> Edges {
    let up = |t: f64, db: f64| t * 10f64.powf(db / 20.0);
    let down = |t: f64, db: f64| t / 10f64.powf(db / 20.0);
    let e = |b: usize| env.get(b).copied().unwrap_or((0.0, 0.0, 0.0, 0.0));
    let pick = |f: &dyn Fn(f64, EnvelopeBand) -> f64| {
        target
            .iter()
            .enumerate()
            .map(|(b, &t)| f(t, e(b)))
            .collect::<Vec<f64>>()
    };
    Edges {
        alarm_hi: pick(&|t, b| up(t, b.1)),
        alarm_lo: pick(&|t, b| down(t, b.0)),
        abort_hi: pick(&|t, b| up(t, b.3)),
        abort_lo: pick(&|t, b| down(t, b.2)),
    }
}

/// The target with its alarm and abort bands, and a measured or synthesized SRS
/// beside it when there is one.
pub fn srs_bands_view(
    title: &str,
    bands_hz: &[f64],
    target: &[f64],
    edges: &Edges,
    other: Option<(&str, Role, &[f64])>,
    arrays: &mut Arrays,
) -> Value {
    let x = || bands_hz.to_vec();
    let mut series = vec![
        Series::new("Target", Role::Reference, x(), target.to_vec()),
        Series::new("Alarm high", Role::Tolerance, x(), edges.alarm_hi.clone()),
        Series::new("Alarm low", Role::Tolerance, x(), edges.alarm_lo.clone()),
        Series::new("Abort high", Role::Abort, x(), edges.abort_hi.clone()),
        Series::new("Abort low", Role::Abort, x(), edges.abort_lo.clone()),
    ];
    if let Some((name, role, y)) = other {
        series.push(Series::new(name, role, x(), y.to_vec()));
    }
    views::spectrum(
        arrays,
        title,
        &freq_axis(),
        &srs_axis(),
        series,
        Some("frequency"),
    )
}

/// The Synthesis view: the synthesized pulse's SRS against the target.
pub fn synthesis_view(
    bands_hz: &[f64],
    target: &[f64],
    achieved: &[f64],
    arrays: &mut Arrays,
) -> Value {
    views::spectrum(
        arrays,
        "Synthesis",
        &freq_axis(),
        &srs_axis(),
        vec![
            Series::new(
                "Target",
                Role::Reference,
                bands_hz.to_vec(),
                target.to_vec(),
            ),
            Series::new(
                "Synthesis",
                Role::Other,
                bands_hz.to_vec(),
                achieved.to_vec(),
            ),
        ],
        Some("frequency"),
    )
}

/// A time history from the record's first sample: `title`, its axis and one series.
#[allow(clippy::too_many_arguments)]
pub fn time_view(
    title: &str,
    y_label: &str,
    unit: &str,
    name: &str,
    role: Role,
    fs_hz: f64,
    y: &[f64],
    points: Option<usize>,
    arrays: &mut Arrays,
) -> Value {
    views::spectrum(
        arrays,
        title,
        &time_axis_spec(),
        &Axis::new(y_label, Some(unit), Scale::Linear),
        vec![time_series(
            name,
            role,
            time_axis(0, y.len(), fs_hz),
            y,
            points,
        )],
        Some("time"),
    )
}

/// What a synthesis's views are drawn from: the target with its envelope, the
/// synthesized pulse's SRS, and the pulse and its kinematics at full level.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewFigures {
    pub fs_hz: f64,
    pub bands_hz: Vec<f64>,
    pub target: Vec<f64>,
    pub edges: Edges,
    pub achieved: Vec<f64>,
    pub waveform: Vec<f64>,
    pub velocity: Vec<f64>,
    pub displacement: Vec<f64>,
}

impl Synthesis {
    /// The figures of this synthesis, its target in the envelope `env`.
    pub fn figures(&self, env: &[EnvelopeBand]) -> PreviewFigures {
        PreviewFigures {
            fs_hz: self.fs_hz,
            bands_hz: self.bands_hz.clone(),
            target: self.target_g.clone(),
            edges: envelope_edges(&self.target_g, env),
            achieved: self.achieved_g.clone(),
            waveform: self.waveform_g.clone(),
            velocity: self.velocity_ips.clone(),
            displacement: self.disp_in.clone(),
        }
    }
}

/// The five views of a synthesis: Target, Synthesis, Pulse, Velocity and
/// Displacement.
pub fn synthesis_views(
    f: &PreviewFigures,
    points: Option<usize>,
    arrays: &mut Arrays,
) -> Vec<Value> {
    vec![
        srs_bands_view("Target", &f.bands_hz, &f.target, &f.edges, None, arrays),
        synthesis_view(&f.bands_hz, &f.target, &f.achieved, arrays),
        time_view(
            "Pulse",
            "Acceleration",
            "g",
            "Synthesized",
            Role::Reference,
            f.fs_hz,
            &f.waveform,
            points,
            arrays,
        ),
        time_view(
            "Velocity",
            "Velocity",
            "in/s",
            "Synthesized",
            Role::Reference,
            f.fs_hz,
            &f.velocity,
            points,
            arrays,
        ),
        time_view(
            "Displacement",
            "Displacement",
            "in",
            "Synthesized",
            Role::Reference,
            f.fs_hz,
            &f.displacement,
            points,
            arrays,
        ),
    ]
}

/// `srs.preview`'s result from a synthesis's facts and figures.
pub fn preview_frame(
    facts: &SynthesisFacts,
    reference: &ReferenceFacts,
    f: &PreviewFigures,
    points: Option<usize>,
    arrays: &mut Arrays,
) -> Value {
    let views = synthesis_views(f, points, arrays);
    json!({
        "derived": {
            "bands_hz": band_list(&f.bands_hz),
            "target_g": value_list(&f.target),
            "synthesis": facts.to_json(),
            "reference": reference.to_json(),
        },
        "views": views,
    })
}

/// `srs.preview`'s result for a synthesis whose target sits in `env`.
pub fn preview_json(
    s: &Synthesis,
    env: &[EnvelopeBand],
    points: Option<usize>,
    arrays: &mut Arrays,
) -> Value {
    preview_frame(&s.facts(), &s.reference, &s.figures(env), points, arrays)
}

/// A uniform envelope over `n` bands from `[low, high]` alarm and abort dB.
pub fn uniform_envelope(n: usize, alarm: (f64, f64), abort: (f64, f64)) -> Vec<EnvelopeBand> {
    vec![(alarm.0, alarm.1, abort.0, abort.1); n]
}

#[cfg(test)]
mod tests {
    use super::super::config::{parse_preview, WireBands, WireTarget};
    use super::*;

    #[test]
    fn a_flat_target_synthesizes_at_rest_at_both_ends_near_the_target() {
        let p = json!({
            "target": {"kind": "flat", "accel_g": 50},
            "bands": {"f_lo_hz": 100, "f_hi_hz": 2000},
            "fs_hz": 20480,
        });
        let w = parse_preview(p.as_object().unwrap()).unwrap();
        let s = synthesize(&w);
        assert_eq!(s.bands_hz.len(), 14);
        assert_eq!(s.target_g, vec![50.0; 14]);
        assert!(s.worst_err_db.unwrap().abs() < 3.0, "{:?}", s.worst_err_db);
        assert!(s.reference.residual_velocity_ips.abs() < 1e-6);
        assert!(s.reference.residual_disp_in.abs() < 1e-6);
        assert_eq!(s.facts().components, 14);
    }

    #[test]
    fn breakpoints_are_joined_on_log_log_axes() {
        let w = WireSynthesis {
            target: WireTarget::Breakpoints {
                points: vec![(100.0, 10.0), (1000.0, 100.0), (2100.0, 100.0)],
            },
            bands: WireBands {
                f_lo_hz: 100.0,
                f_hi_hz: 2000.0,
                bands_per_octave: 3.0,
                q: 10.0,
            },
            engine: EngineKind::Wavsyn,
            components: None,
            max_iters: 1,
            tol_db: 1.5,
            max_gain_db_per_iter: 3.0,
            fs_hz: 20480.0,
        };
        let t = target_on_grid(&w);
        // Ten times the frequency is ten times the SRS on the rising leg.
        assert!((t[0] - 10.0).abs() < 1e-9);
        assert!((t[3] / t[0] - 2.0).abs() < 1e-9, "{t:?}");
    }

    #[test]
    fn the_envelope_edges_sit_their_decibels_either_side() {
        let e = envelope_edges(&[10.0], &[(6.0, 3.0, 9.0, 6.0)]);
        let (ah, al, bh, bl) = (e.alarm_hi, e.alarm_lo, e.abort_hi, e.abort_lo);
        assert!((20.0 * (ah[0] / 10.0).log10() - 3.0).abs() < 1e-12);
        assert!((20.0 * (10.0 / al[0]).log10() - 6.0).abs() < 1e-12);
        assert!((20.0 * (bh[0] / 10.0).log10() - 6.0).abs() < 1e-12);
        assert!((20.0 * (10.0 / bl[0]).log10() - 9.0).abs() < 1e-12);
    }
}
