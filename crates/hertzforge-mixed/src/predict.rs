//! What a Mixed Mode test will ask of the shaker, predicted from the
//! configuration alone (no station): each part's mean square and the total,
//! the peak acceleration, velocity and displacement of the summed signal, the
//! closest two tones come, and the lines the floor estimate leaves out beside
//! dwelling tones.
//!
//! The peak of the sum is the conservative bound: every tone at its largest
//! amplitude over its motion, plus the crest factor times the random part's
//! rms (floor and bands), each tone's velocity and displacement taken at the
//! frequency of its motion where they are largest. The random part's velocity
//! and displacement put each band at its lowest center. Mean squares add
//! because the parts are independent: a tone's is `A^2 / 2`, a band's its
//! added density times its width, the floor's the integral of its profile.

use crate::parts::{ConfigError, MixedConfig, PartBody, ToneAmplitude};
use hertzforge_shaker_db::{RatingBasis, RatingValue};
use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;

/// Standard gravity, m/s^2 per g.
pub const STANDARD_G: f64 = 9.806_65;
/// Points a moving tone's or band's motion is sampled at for its largest
/// velocity and displacement.
const MOTION_SAMPLES: usize = 64;

/// One part's mean square at full level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartMeanSquare {
    /// The part's id.
    pub part: String,
    /// `floor`, `tone` or `narrowband`.
    pub kind: String,
    /// Mean square at full level, g^2 (a tone's at its largest amplitude, a
    /// band's at its widest).
    pub ms_g2: f64,
}

/// One shaker rating against what the test asks of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RatingCheck {
    /// `accel`, `velocity`, `displacement` or `force`.
    pub quantity: String,
    /// What the test asks, in `unit`.
    pub predicted: f64,
    /// The rating, in `unit`.
    pub rating: f64,
    /// `g`, `in/s`, `in pk-pk` or `lbf`, with ` rms` when the rating is an rms.
    pub unit: String,
    /// Whether the prediction is within the rating.
    pub within: bool,
}

/// The prediction for a configuration at full level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixedPrediction {
    /// Each part's mean square, in parts-list order (switched-off parts
    /// included: they can be switched on during the run).
    pub parts: Vec<PartMeanSquare>,
    /// The random part's mean square (floor and bands), g^2.
    pub random_ms_g2: f64,
    /// Every part's mean square, g^2.
    pub total_ms_g2: f64,
    /// Its root, g rms.
    pub total_rms_g: f64,
    /// The summed signal's peak acceleration, g.
    pub accel_g_pk: f64,
    /// The summed signal's peak velocity, m/s.
    pub velocity_m_s_pk: f64,
    /// The summed signal's peak displacement, m (stroke is twice this).
    pub displacement_m_pk: f64,
    /// The moving mass times the peak acceleration, lbf; `None` with no mass.
    pub force_lbf_pk: Option<f64>,
    /// The closest two tones that play together come: `(a, b, Hz)`.
    pub closest_tones: Option<(String, String, f64)>,
    /// The frequency spans the floor estimate leaves out beside each dwelling
    /// tone, `(lo, hi)` Hz: the tone's frequency plus and minus `exclusion_hz`.
    pub excluded_hz: Vec<(f64, f64)>,
    /// Lines of the requested grid (`df_hz`) inside those spans.
    pub excluded_lines: usize,
    /// The shaker's ratings against the prediction; empty without a shaker.
    pub ratings: Vec<RatingCheck>,
    /// The longest the floor plays alone before the parts come in, seconds:
    /// the larger of `floor_settle_s` and the time the requested grid
    /// (`df_hz`, half-overlapped frames, `blocks_per_window` a loop) needs to
    /// reach `control_dof`.
    #[serde(default)]
    pub floor_equalize_s: f64,
}

impl MixedPrediction {
    /// Whether every rating holds (true without a shaker).
    pub fn within_ratings(&self) -> bool {
        self.ratings.iter().all(|r| r.within)
    }
}

/// Predict what `cfg` asks of the shaker at full level. The configuration
/// must validate (`cfg.validate(None)`); a refusal is returned as validation
/// returns it. Pure: no station, no rate.
pub fn predict(cfg: &MixedConfig) -> Result<MixedPrediction, ConfigError> {
    cfg.validate(None)?;
    let motions = cfg.motions()?;
    let floor = cfg.floor().ok_or(ConfigError::NoFloor)?;
    let ft = &floor.breakpoints;
    let bad = |why: String| ConfigError::BadConfig {
        field: "parts",
        why,
    };
    let floor_ms = ft.grms_of_table().map_err(|e| bad(e.to_string()))?.powi(2);
    let floor_v = ft.velocity_rms_si().map_err(|e| bad(e.to_string()))?;
    let floor_d = ft.displacement_rms_si().map_err(|e| bad(e.to_string()))?;

    let mut parts = Vec::with_capacity(cfg.parts.len());
    let (mut rand_ms, mut rand_v2, mut rand_d2) = (floor_ms, floor_v * floor_v, floor_d * floor_d);
    let (mut tone_a, mut tone_v, mut tone_d) = (0.0, 0.0, 0.0);
    let mut excluded_hz = Vec::new();
    for (p, m) in cfg.parts.iter().zip(&motions) {
        let ms = match (&p.body, m) {
            (PartBody::Floor(_), _) => floor_ms,
            (PartBody::Tone(t), Some(m)) => {
                let (mut a_max, mut v_max, mut d_max) = (0.0f64, 0.0f64, 0.0f64);
                for f in motion_points(m.lo_hz, m.hi_hz) {
                    let a = match &t.amplitude {
                        ToneAmplitude::Constant { g_pk } => *g_pk,
                        amp => amp.g_pk_at(f).unwrap_or(0.0),
                    };
                    let w = TAU * f;
                    a_max = a_max.max(a);
                    v_max = v_max.max(a * STANDARD_G / w);
                    d_max = d_max.max(a * STANDARD_G / (w * w));
                }
                tone_a += a_max;
                tone_v += v_max;
                tone_d += d_max;
                if m.hi_hz <= m.lo_hz {
                    excluded_hz.push((m.lo_hz - cfg.exclusion_hz, m.lo_hz + cfg.exclusion_hz));
                }
                a_max * a_max / 2.0
            }
            (PartBody::Narrowband(b), Some(m)) => {
                let width_at = |c: f64| {
                    let (lo, hi) = b.width.edges_at(c);
                    hi - lo
                };
                let ms = b.level_g2_hz * width_at(m.hi_hz).max(width_at(m.lo_hz));
                let w = TAU * m.lo_hz;
                let ms_lo = b.level_g2_hz * width_at(m.lo_hz) * STANDARD_G * STANDARD_G;
                rand_ms += ms;
                rand_v2 += ms_lo / (w * w);
                rand_d2 += ms_lo / (w * w * w * w);
                ms
            }
            _ => 0.0,
        };
        parts.push(PartMeanSquare {
            part: p.id.clone(),
            kind: p.kind().to_owned(),
            ms_g2: ms,
        });
    }
    let total_ms: f64 = parts.iter().map(|p| p.ms_g2).sum();
    let k = cfg.crest_factor;
    let accel = tone_a + k * rand_ms.sqrt();
    let vel = tone_v + k * rand_v2.sqrt();
    let disp = tone_d + k * rand_d2.sqrt();
    let mass = cfg.moving_mass.total();
    let force = (mass > 0.0).then_some(mass * accel);
    let excluded_lines = count_lines(&excluded_hz, cfg.df_hz);

    let mut ratings = Vec::new();
    if let Some(sh) = &cfg.shaker {
        let total_rms = total_ms.sqrt();
        let mut check = |q: &str, unit: &str, r: &Option<RatingValue>, pk: f64, rms: f64| {
            if let Some(r) = r {
                let (pred, unit) = match r.basis {
                    RatingBasis::Rms => (rms, format!("{unit} rms")),
                    _ => (pk, unit.to_owned()),
                };
                ratings.push(RatingCheck {
                    quantity: q.to_owned(),
                    predicted: pred,
                    rating: r.value,
                    unit,
                    within: pred <= r.value,
                });
            }
        };
        check("accel", "g", &sh.max_accel_bare_g, accel, total_rms);
        let ips = vel / 0.0254;
        check(
            "velocity",
            "in/s",
            &sh.max_velocity_ips,
            ips,
            ips / k.max(1.0),
        );
        let in_pp = 2.0 * disp / 0.0254;
        check(
            "displacement",
            "in pk-pk",
            &sh.max_displacement_in_pp,
            in_pp,
            in_pp / k.max(1.0),
        );
        if mass > 0.0 {
            let force_rating = if tone_a > 0.0 {
                &sh.force_sine
            } else {
                &sh.force_random
            };
            check("force", "lbf", force_rating, mass * accel, mass * total_rms);
        }
    }

    Ok(MixedPrediction {
        parts,
        random_ms_g2: rand_ms,
        total_ms_g2: total_ms,
        total_rms_g: total_ms.sqrt(),
        accel_g_pk: accel,
        velocity_m_s_pk: vel,
        displacement_m_pk: disp,
        force_lbf_pk: force,
        closest_tones: cfg.closest_tones()?,
        excluded_hz,
        excluded_lines,
        ratings,
        floor_equalize_s: {
            let frame = 1.0 / cfg.df_hz;
            let frames = (cfg.control_dof / crate::floor::dof_per_frame()).ceil();
            let grid = frames * frame / 2.0 + frame + cfg.blocks_per_window as f64 * frame / 2.0;
            cfg.floor_settle_s.max(grid)
        },
    })
}

/// The chance a sample of a sine of peak `a` (uniform phase) plus Gaussian
/// noise of rms `sigma` exceeds `c` in magnitude (the brief's 6.2):
/// `(1/pi) int_0^pi [Q((c - a sin th)/sigma) + Q((c + a sin th)/sigma)] dth`.
pub fn sine_gauss_exceedance(c: f64, a: f64, sigma: f64) -> f64 {
    let q = |z: f64| 0.5 * erfc(z / std::f64::consts::SQRT_2);
    let n = 2000;
    let h = std::f64::consts::PI / n as f64;
    // Simpson's rule over [0, pi].
    let f = |th: f64| q((c - a * th.sin()) / sigma) + q((c + a * th.sin()) / sigma);
    let mut s = f(0.0) + f(std::f64::consts::PI);
    for i in 1..n {
        s += f(i as f64 * h) * if i % 2 == 1 { 4.0 } else { 2.0 };
    }
    s * h / 3.0 / std::f64::consts::PI
}

/// The complementary error function (Numerical Recipes' Chebyshev fit,
/// relative error under 1.2e-7).
pub fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let r = t
        * (-z * z - 1.265_512_23
            + t * (1.000_023_68
                + t * (0.374_091_96
                    + t * (0.096_784_18
                        + t * (-0.186_288_06
                            + t * (0.278_868_07
                                + t * (-1.135_203_98
                                    + t * (1.488_515_87
                                        + t * (-0.822_152_23 + t * 0.170_872_77)))))))))
            .exp();
    if x >= 0.0 {
        r
    } else {
        2.0 - r
    }
}

/// `MOTION_SAMPLES` log-spaced points from `lo` to `hi` (one point at a dwell).
fn motion_points(lo: f64, hi: f64) -> impl Iterator<Item = f64> {
    let n = if hi > lo { MOTION_SAMPLES } else { 1 };
    let r = if n > 1 {
        (hi / lo).ln() / (n - 1) as f64
    } else {
        0.0
    };
    (0..n).map(move |i| lo * (r * i as f64).exp())
}

/// Lines `k * df` (k >= 0) inside any span, each counted once.
fn count_lines(spans: &[(f64, f64)], df: f64) -> usize {
    if df.is_nan() || df <= 0.0 || spans.is_empty() {
        return 0;
    }
    let top = spans.iter().map(|s| s.1).fold(0.0, f64::max);
    (0..=(top / df).floor() as usize)
        .filter(|&k| {
            let f = k as f64 * df;
            spans.iter().any(|&(lo, hi)| f >= lo && f <= hi)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::tests_support::*;
    use crate::parts::{BandWidth, PartSweepTarget};
    use hertzforge_shaker_db::{MovingMass, ShakerRecord};

    /// The brief's V2 setup, in g: a flat 0.01 g^2/Hz floor from 20 to 2000 Hz
    /// (19.8 g^2), tones of 2 and 1 g peak (2 and 0.5), a 10 Hz band of 0.05
    /// added (0.5): 22.8 g^2.
    fn v2() -> MixedConfig {
        let mut t1 = tone("t1", fixed(100.0));
        let mut t2 = tone("t2", fixed(300.0));
        for (t, a) in [(&mut t1, 2.0), (&mut t2, 1.0)] {
            if let PartBody::Tone(tp) = &mut t.body {
                tp.amplitude = ToneAmplitude::Constant { g_pk: a };
            }
        }
        config(vec![
            floor("floor"),
            t1,
            t2,
            band("b1", fixed(1000.0), BandWidth::Hz { hz: 10.0 }),
        ])
    }

    #[test]
    fn mean_squares_add_to_the_brief_v2_total() {
        let p = predict(&v2()).unwrap();
        let ms: Vec<f64> = p.parts.iter().map(|x| x.ms_g2).collect();
        assert!((ms[0] - 19.8).abs() < 1e-9, "{ms:?}");
        assert!((ms[1] - 2.0).abs() < 1e-12 && (ms[2] - 0.5).abs() < 1e-12);
        assert!((ms[3] - 0.5).abs() < 1e-12);
        assert!((p.total_ms_g2 - 22.8).abs() < 1e-9);
        assert!((p.total_rms_g - 22.8f64.sqrt()).abs() < 1e-9);
        assert!((p.random_ms_g2 - 20.3).abs() < 1e-9);
        // The peak is the tones' sum plus the crest factor on the random rms.
        let want = 3.0 + p_crest(&v2()) * 20.3f64.sqrt();
        assert!((p.accel_g_pk - want).abs() < 1e-9);
        assert_eq!(p.closest_tones, Some(("t1".into(), "t2".into(), 200.0)));
        // Two dwelling tones, each excluding 100 +/- 2 Hz: 5 lines at 1 Hz each.
        assert_eq!(p.excluded_hz, vec![(98.0, 102.0), (298.0, 302.0)]);
        assert_eq!(p.excluded_lines, 10);
        // 120 DOF at 1 Hz lines: 64 half-overlapped 1 s frames, one more
        // frame and a 12-frame loop, 39 s, over the 30 s settle time.
        assert!(
            (p.floor_equalize_s - 39.0).abs() < 1e-9,
            "{}",
            p.floor_equalize_s
        );
    }

    fn p_crest(c: &MixedConfig) -> f64 {
        c.crest_factor
    }

    #[test]
    fn a_tone_velocity_and_displacement_are_taken_where_its_sweep_is_lowest() {
        let mut c = config(vec![
            floor("floor"),
            tone(
                "s",
                sweep(50.0, 400.0, 100.0, PartSweepTarget::Sweeps { n: 2 }),
            ),
        ]);
        c.crest_factor = 3.0;
        let p = predict(&c).unwrap();
        let w = TAU * 50.0;
        let floor = c.floor().unwrap();
        let fv = floor.breakpoints.velocity_rms_si().unwrap();
        let fd = floor.breakpoints.displacement_rms_si().unwrap();
        assert!((p.velocity_m_s_pk - (STANDARD_G / w + 3.0 * fv)).abs() < 1e-9);
        assert!((p.displacement_m_pk - (STANDARD_G / (w * w) + 3.0 * fd)).abs() < 1e-12);
        // A sweep excludes nothing.
        assert!(p.excluded_hz.is_empty());
    }

    #[test]
    fn a_shaker_rating_the_test_exceeds_is_named() {
        let mut c = v2();
        let mut sh = ShakerRecord::new("bench");
        sh.max_accel_bare_g = Some(RatingValue::peak(10.0));
        sh.force_sine = Some(RatingValue::peak(1000.0));
        c.shaker = Some(sh);
        c.moving_mass = MovingMass {
            armature_lb: 20.0,
            ..MovingMass::default()
        };
        let p = predict(&c).unwrap();
        let accel = p.ratings.iter().find(|r| r.quantity == "accel").unwrap();
        assert!(!accel.within, "{accel:?}");
        let force = p.ratings.iter().find(|r| r.quantity == "force").unwrap();
        assert!((force.predicted - 20.0 * p.accel_g_pk).abs() < 1e-9);
        assert!(!p.within_ratings());
        // Without a shaker nothing is judged.
        assert!(predict(&v2()).unwrap().within_ratings());
    }

    #[test]
    fn a_refused_configuration_is_refused_by_name() {
        let c = config(vec![tone("t", fixed(100.0))]);
        assert_eq!(predict(&c).unwrap_err().name(), "no_floor");
    }
}
