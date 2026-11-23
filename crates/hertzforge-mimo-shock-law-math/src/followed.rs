//! The followed laws: `shape` on the followed softening family.
//!
//! - `followed`: probes on the identification, then per octave band a drop
//!   fitted from the pulses at each level, carried to the next level as a
//!   bounded power of the amplitude, times the margin 1.5, at most the
//!   ceiling 12 %; the assumed drop only widens and changes only on a step up
//!   (`Follow::Levels` with no narrowing); the model the shape law inverts is
//!   the family inverse over members 0, 1/2 and 1 of the drop. Assembled as
//!   the MIMO Shock research bridge assembled it, with the same warp options.
//! - `followed-hold`: `followed` with no ceiling and three pieces, each a
//!   setting: the probe check (before each probe, its SRS is predicted
//!   through the identification; when the prediction plus the measured
//!   prediction error carried in amplitude is over the limit, the probe is
//!   scaled down), the power bound
//!   ([`SofteningFamily::with_resolved_power`], gate 0.5 %) and the hold
//!   ([`ShapeLaw::set_hold`], 3 dB).

use crate::shape::{CarryGain, ShapeExtras, ShapeLaw, ShapeParams, ShapeSafety};
use crate::softening::{Follow, SofteningFamily, SofteningPlan};
use crate::spectrum::{apply, rows_irfft, rows_rfft};
use crate::srs::{error_db, srs_rows};
use crate::warp::{WarpModel, WarpOptions};
use crate::{drive_mask, level_scale, LawSteps, PulseInputs, PulseOutputs, Setup, C64};

/// The warp's options the followed laws use (the research bridge's): below a
/// 15 kHz sample rate the warped band is 40 to 2400 Hz, the bank's top
/// 3000 Hz, the delay read in 2500 to 4000 Hz and the noise in 4000 to
/// 6000 Hz; at higher rates the warp's defaults.
pub fn followed_warp_options(fs_hz: f64) -> WarpOptions {
    if fs_hz < 15_000.0 {
        WarpOptions {
            band_lo_hz: 40.0,
            band_hi_hz: 2400.0,
            bank_hi_hz: 3000.0,
            delay_band_hz: (2500.0, 4000.0),
            noise_band_hz: (4000.0, 6000.0),
            ..WarpOptions::default()
        }
    } else {
        WarpOptions::default()
    }
}

/// The probe check's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeCheck {
    /// The levels checked, dB (a level the ladder inserts between them is
    /// not checked).
    pub probes_db: Vec<f64>,
    /// The largest predicted over-test, with the margin, dB.
    pub limit_db: f64,
    /// The power of the amplitude the measured prediction error is carried
    /// with.
    pub p_max: f64,
}

/// The settings of a followed law.
#[derive(Clone, Debug, PartialEq)]
pub struct FollowedOptions {
    /// Levels played on the identification before the family is set.
    pub probe_levels: usize,
    /// The largest drop assumed in any band.
    pub ceiling: f64,
    /// The power bound's gate (`None`: off).
    pub resolved_power_gate: Option<f64>,
    /// The hold's tolerance, dB (`None`: off).
    pub hold_db: Option<f64>,
    /// The probe check (`None`: off).
    pub probe_check: Option<ProbeCheck>,
}

impl FollowedOptions {
    /// `followed`: ceiling 12 %, no probe check, no power bound, no hold.
    pub fn followed() -> Self {
        FollowedOptions {
            probe_levels: 2,
            ceiling: 0.12,
            resolved_power_gate: None,
            hold_db: None,
            probe_check: None,
        }
    }

    /// `followed-hold`: no ceiling, the probe check at -40 and -34 dB
    /// against +3 dB, the power bound at 0.5 %, the hold at 3 dB.
    pub fn followed_hold() -> Self {
        FollowedOptions {
            probe_levels: 2,
            ceiling: f64::INFINITY,
            resolved_power_gate: Some(0.005),
            hold_db: Some(3.0),
            probe_check: Some(ProbeCheck {
                probes_db: vec![-40.0, -34.0],
                limit_db: 3.0,
                p_max: 1.0,
            }),
        }
    }
}

/// The softening plan of the followed laws.
pub fn followed_plan(o: &FollowedOptions) -> SofteningPlan {
    SofteningPlan {
        follow: Follow::Levels {
            narrow: 1.0,
            within_level: false,
        },
        ladder_start_db: Some(-12.0),
        probe_levels: o.probe_levels,
        ceiling: o.ceiling,
        ..SofteningPlan::default()
    }
}

/// The dB to add to a predicted over-test for a prediction error `eps_db`
/// measured at amplitude `a_from`, carried to `a_to` as the power `p_max`
/// of the amplitude: `10 log10(1 + (10^(eps/10) - 1) (a_to / a_from)^(2 p))`,
/// zero for an error not above zero.
pub fn margin_at(eps_db: f64, a_from: f64, a_to: f64, p_max: f64) -> f64 {
    if eps_db <= 0.0 {
        return 0.0;
    }
    10.0 * (1.0 + (10f64.powf(eps_db / 10.0) - 1.0) * (a_to / a_from).powf(2.0 * p_max)).log10()
}

/// The scale, dB in `[-40, 0]`, that brings a predicted over-test
/// `over_pred_db` at amplitude `amp`, plus the margin of every measured
/// prediction error `(amplitude, error dB)`, to `limit_db`: 0 when already
/// within, -40 when even that is not enough, else found by bisection.
pub fn probe_scale_db(
    over_pred_db: f64,
    limit_db: f64,
    amp: f64,
    eps: &[(f64, f64)],
    p_max: f64,
) -> f64 {
    let need = |k: f64| {
        let a = amp * 10f64.powf(k / 20.0);
        let m = eps
            .iter()
            .map(|&(aj, e)| margin_at(e, aj, a, p_max))
            .fold(0.0, f64::max);
        over_pred_db + k + m - limit_db
    };
    if need(0.0) <= 0.0 {
        return 0.0;
    }
    let (mut lo, mut hi) = (-40.0, 0.0);
    if need(lo) > 0.0 {
        return lo;
    }
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if need(mid) > 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    lo
}

/// One probe check as played: level, scale dB, predicted over dB (after the
/// scale), measured over dB.
pub type ProbeRecord = (f64, f64, f64, f64);

/// Rows of spectra.
type Spectra = Vec<Vec<C64>>;

/// `followed` or `followed-hold`.
pub struct FollowedLaw {
    shape: ShapeLaw,
    setup: Setup,
    check: Option<ProbeCheck>,
    k_db: f64,
    prev: Option<(Spectra, Spectra)>,
    eps: Vec<(f64, f64)>,
    pending: Option<(f64, f64, f64)>,
    checks: Vec<ProbeRecord>,
}

impl FollowedLaw {
    /// The law on `setup`, its identification the model it starts from.
    pub fn new(setup: Setup, o: FollowedOptions) -> Result<Self, String> {
        let base = setup.plant.frf.clone();
        let mask = drive_mask(&setup);
        let warp = WarpModel::new(
            base.clone(),
            mask.clone(),
            setup.fs_hz,
            followed_warp_options(setup.fs_hz),
        )?;
        let mut family = SofteningFamily::new(
            warp,
            base,
            mask,
            rows_rfft(&setup.reference, 1.0)?,
            followed_plan(&o),
        )?;
        if let Some(g) = o.resolved_power_gate {
            family = family.with_resolved_power(g);
        }
        let mut shape = ShapeLaw::with_options(
            setup.clone(),
            ShapeParams::default(),
            ShapeExtras {
                provider: Some(Box::new(family)),
                safety: ShapeSafety {
                    carry: CarryGain::Keep,
                    ..ShapeSafety::off()
                },
                robust_first: None,
            },
        )?;
        shape.set_hold(o.hold_db);
        Ok(FollowedLaw {
            shape,
            setup,
            check: o.probe_check,
            k_db: 0.0,
            prev: None,
            eps: Vec::new(),
            pending: None,
            checks: Vec::new(),
        })
    }

    /// `followed` on `setup`.
    pub fn followed(setup: Setup) -> Result<Self, String> {
        Self::new(setup, FollowedOptions::followed())
    }

    /// `followed-hold` on `setup`.
    pub fn followed_hold(setup: Setup) -> Result<Self, String> {
        Self::new(setup, FollowedOptions::followed_hold())
    }

    /// Every probe check so far.
    pub fn probe_checks(&self) -> &[ProbeRecord] {
        &self.checks
    }

    /// How many pulses the hold has kept.
    pub fn holds(&self) -> usize {
        self.shape.holds()
    }

    fn max_over(&self, responses: &[Vec<f64>], scale: f64) -> f64 {
        let t = self.setup.srs.as_ref().expect("checked by the shape law");
        srs_rows(responses, self.setup.fs_hz, &t.natural_hz, t.q, scale)
            .iter()
            .zip(&t.points)
            .flat_map(|(m, p)| error_db(m, p))
            .fold(f64::NEG_INFINITY, f64::max)
    }

    /// The drives the shape law made for `level_db`, through the probe check.
    fn play(&mut self, mut out: PulseOutputs, level_db: f64) -> Result<PulseOutputs, String> {
        let mut k = 0.0;
        self.pending = None;
        if let Some(c) = self.check.clone() {
            if c.probes_db.iter().any(|&p| (p - level_db).abs() < 1e-9) {
                let s = level_scale(level_db);
                let dn = rows_rfft(&out.drives, s)?;
                let h = &self.setup.plant.frf;
                let y = match &self.prev {
                    None => apply(h, &dn),
                    Some((pd, py)) => {
                        let dd: Vec<Vec<C64>> = dn
                            .iter()
                            .zip(pd)
                            .map(|(a, b)| a.iter().zip(b).map(|(x, z)| x - z).collect())
                            .collect();
                        apply(h, &dd)
                            .iter()
                            .zip(py)
                            .map(|(a, b)| a.iter().zip(b).map(|(x, z)| x + z).collect())
                            .collect()
                    }
                };
                let yt = rows_irfft(&y, self.setup.record_len, 1.0)?;
                let op = self.max_over(&yt, 1.0);
                k = probe_scale_db(
                    op,
                    c.limit_db,
                    s * 10f64.powf(op / 20.0),
                    &self.eps,
                    c.p_max,
                );
                self.pending = Some((level_db, k, op + k));
                out.values.push(("probe_predicted_over_db", op));
                out.values.push(("probe_scale_db", k));
            }
        }
        self.k_db = k;
        if k != 0.0 {
            let g = 10f64.powf(k / 20.0);
            for r in out.drives.iter_mut() {
                r.iter_mut().for_each(|v| *v *= g);
            }
        }
        Ok(out)
    }
}

impl LawSteps for FollowedLaw {
    fn seed(&mut self, level_db: f64) -> Result<PulseOutputs, String> {
        let out = self.shape.seed(level_db)?;
        self.play(out, level_db)
    }

    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, String> {
        if self.check.is_none() {
            return self.shape.iterate(inputs);
        }
        let eff = inputs.level_db + self.k_db;
        if let Some((lv, k, pred)) = self.pending.take() {
            let meas = self.max_over(&inputs.responses, level_scale(inputs.level_db));
            self.checks.push((lv, k, pred, meas));
            self.eps.push((
                level_scale(eff) * 10f64.powf((pred - k) / 20.0),
                (meas - pred).max(0.0),
            ));
        }
        let s_eff = level_scale(eff);
        self.prev = Some((
            rows_rfft(&inputs.drives, s_eff)?,
            rows_rfft(&inputs.responses, s_eff)?,
        ));
        let inner = PulseInputs {
            level_db: eff,
            ..inputs.clone()
        };
        let out = self.shape.iterate(&inner)?;
        self.play(out, inputs.next_level_db)
    }
}
