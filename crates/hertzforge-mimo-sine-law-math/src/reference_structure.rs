//! Test support, not part of any law: an exact linear multi-drive modal
//! structure with the tracking filter's lag, a tone schedule counted in
//! samples, and a closed loop with no guards, enough to run a law in this
//! crate's own tests without the simulator.
//!
//! The structure answers in steady state at the frequency of each interval's
//! last sample, `H(f) = sum_r o_r d_r^T (-w^2) / (w_r^2 - w^2 + 2 j zeta_r w_r w)`
//! (acceleration per volt), and a first-order lag of time constant
//! `2 / (2 pi B)` per update stands in for the two-pole tracking filter.

use crate::law::{Hold, Inputs, Outputs, SineLaw};
use crate::setup::{Identification, Setup, Shape, TrackingRule};
use crate::{MatrixStack, C64};
use std::f64::consts::PI;

const ZERO: C64 = C64::new(0.0, 0.0);

/// A linear modal structure.
#[derive(Clone, Debug, PartialEq)]
pub struct ModalStructure {
    /// Natural frequencies, Hz.
    pub f_hz: Vec<f64>,
    /// Damping ratios.
    pub zeta: Vec<f64>,
    /// `modes x drives`, modal force per volt.
    pub drive_shape: Vec<f64>,
    /// `modes x controls`.
    pub output_shape: Vec<f64>,
    /// Drives.
    pub drives: usize,
    /// Controls.
    pub controls: usize,
}

impl ModalStructure {
    /// The response at `f` (`controls x drives`, row-major), with each mode's
    /// frequency and damping optionally replaced.
    pub fn frf_with(&self, f: f64, fn_hz: &[f64], zeta: &[f64], out: &mut [C64]) {
        let (n, m) = (self.controls, self.drives);
        out.iter_mut().for_each(|v| *v = ZERO);
        let w = 2.0 * PI * f;
        for r in 0..self.f_hz.len() {
            let wn = 2.0 * PI * fn_hz[r];
            let h = C64::new(-(w * w), 0.0) / C64::new(wn * wn - w * w, 2.0 * zeta[r] * wn * w);
            for i in 0..n {
                let o = self.output_shape[r * n + i];
                for j in 0..m {
                    out[i * m + j] += h * (o * self.drive_shape[r * m + j]);
                }
            }
        }
    }

    /// One update interval of [`Dynamics::Envelope`] at frequency `f` with the
    /// tracking bandwidth `b`: the drive ramps from `d0` to `d1`.
    pub fn envelope_interval(
        &self,
        f: f64,
        b: f64,
        d0: &[C64],
        d1: &[C64],
        env: &mut [C64],
        stages: &mut [[C64; 2]],
    ) {
        let (n, m) = (self.controls, self.drives);
        let w = 2.0 * PI * f;
        let jw = C64::new(0.0, w);
        let alpha = 1.0 - (-2.0 * PI * b / FS_HZ).exp();
        let modes = self.f_hz.len();
        let mut coef = Vec::with_capacity(modes);
        for r in 0..modes {
            let wr = 2.0 * PI * self.f_hz[r];
            let z = self.zeta[r];
            let wd = wr * (1.0 - z * z).max(1e-6).sqrt();
            let p = C64::new(-z * wr, wd);
            let kk = p * p / C64::new(0.0, 2.0 * wd);
            let qs = C64::new(1.0, 0.0) + kk.conj() / (jw - p.conj());
            let lam = p - jw;
            let e = (lam / FS_HZ).exp();
            coef.push((e, (e - 1.0) / lam * kk, qs));
        }
        let mut u = vec![ZERO; modes];
        for s in 0..TICK_SAMPLES {
            let t = (s + 1) as f64 / TICK_SAMPLES as f64;
            for (r, ur) in u.iter_mut().enumerate() {
                *ur = (0..m).fold(ZERO, |acc, j| {
                    acc + (d0[j] * (1.0 - t) + d1[j] * t) * self.drive_shape[r * m + j]
                });
                let (e, g, _) = coef[r];
                env[r] = e * env[r] + g * *ur;
            }
            for (i, sg) in stages.iter_mut().enumerate().take(n) {
                let y = (0..modes).fold(ZERO, |acc, r| {
                    acc + (coef[r].2 * u[r] + env[r]) * self.output_shape[r * n + i]
                });
                sg[0] += (y - sg[0]) * alpha;
                sg[1] += (sg[0] - sg[1]) * alpha;
            }
        }
    }

    /// The small-signal response at `f`.
    pub fn frf(&self, f: f64, out: &mut [C64]) {
        self.frf_with(f, &self.f_hz, &self.zeta, out);
    }

    /// An identification on a grid of `step` Hz over `band`, exact or with a
    /// fixed multiplicative scatter of about 2 percent from line to line.
    pub fn identification(&self, band: (f64, f64), step: f64, scatter: bool) -> Identification {
        let (n, m) = (self.controls, self.drives);
        let k0 = (band.0 / step).ceil() as i64;
        let k1 = (band.1 / step).floor() as i64;
        let grid: Vec<f64> = (k0..=k1).map(|k| k as f64 * step).collect();
        let l = grid.len();
        let mut data = vec![ZERO; l * n * m];
        for (k, &f) in grid.iter().enumerate() {
            let line = &mut data[k * n * m..(k + 1) * n * m];
            self.frf(f, line);
            if scatter {
                for i in 0..n {
                    for j in 0..m {
                        let (kf, fi, fj) = (k as f64, i as f64, j as f64);
                        let s = C64::new(
                            1.0 + 0.02 * (1.7 * kf + 0.9 * fi + 2.3 * fj).sin(),
                            0.02 * (1.3 * kf + 0.7 * fi + 0.4 * fj).cos(),
                        );
                        line[i * m + j] *= s;
                    }
                }
            }
        }
        Identification {
            grid_hz: grid,
            response: MatrixStack::from_vec(l, n, m, data).expect("sizes agree"),
            limit_response: MatrixStack::zeros(l, 0, m),
            coherence: vec![1.0; n * l],
            limit_coherence: Vec::new(),
            trusted: vec![true; l],
        }
    }

    /// An identification on a grid of `step` Hz over `band` with seeded
    /// scatter: each element times `1 + relative n1`, plus `floor` times the
    /// line's largest magnitude times `n2`, with `n1`, `n2` complex, each part
    /// uniform in -1 to 1. The floor is the shape of measurement noise: the
    /// same absolute error on every element of a line, so a small element's
    /// phase is the least certain.
    pub fn identification_scattered(
        &self,
        band: (f64, f64),
        step: f64,
        scatter: Scatter,
    ) -> Identification {
        let mut id = self.identification(band, step, false);
        let (l, n, m) = id.response.dims();
        let mut data = id.response.data().to_vec();
        let mut rng = scatter.seed ^ 0x9e37_79b9_7f4a_7c15;
        let mut uni = || {
            rng = rng.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = rng;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            (z >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        };
        for k in 0..l {
            let line = &mut data[k * n * m..(k + 1) * n * m];
            let big = line.iter().map(|v| v.norm()).fold(0.0, f64::max);
            for v in line.iter_mut() {
                let n1 = C64::new(uni(), uni());
                let n2 = C64::new(uni(), uni());
                *v = *v * (C64::new(1.0, 0.0) + n1 * scatter.relative) + n2 * (scatter.floor * big);
            }
        }
        id.response = MatrixStack::from_vec(l, n, m, data).expect("sizes agree");
        id
    }

    /// [`ModalStructure::setup`] with a seeded scatter in place of the fixed
    /// one.
    pub fn setup_scattered(&self, band: (f64, f64), scatter: Scatter) -> Setup {
        let mut s = self.setup(band, false);
        s.identification = self.identification_scattered(band, 0.25, scatter);
        s
    }

    /// A setup on this structure with a reference of ones, the testbed's
    /// defaults (16384 Hz, an update every 164 samples, tracking at 10 percent
    /// of the frequency up to 20 Hz).
    pub fn setup(&self, band: (f64, f64), scatter: bool) -> Setup {
        let id = self.identification(band, 0.25, scatter);
        let l = id.grid_hz.len();
        let (n, m) = (self.controls, self.drives);
        Setup {
            controls: n,
            drives: m,
            limits: 0,
            fs_hz: FS_HZ,
            tick_s: TICK_SAMPLES as f64 / FS_HZ,
            band_lo_hz: band.0,
            band_hi_hz: band.1,
            drive_ceiling_v: vec![1e6; m],
            identification: id,
            identification_level_db: -20.0,
            reference: vec![C64::new(1.0, 0.0); n * l],
            phase_specified: true,
            limit_bounds: Vec::new(),
            alarm_db: vec![3.0; n],
            abort_db: vec![12.0; n],
            shape: Shape::of(n, m),
            tracking: TrackingRule::Proportional {
                pct: 10.0,
                max_hz: 20.0,
            },
            loop_delay: Default::default(),
        }
    }
}

/// Seeded scatter on an identification (see
/// [`ModalStructure::identification_scattered`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scatter {
    /// Relative scatter on each element.
    pub relative: f64,
    /// Absolute scatter on each element, as a fraction of the line's largest.
    pub floor: f64,
    /// Seed.
    pub seed: u64,
}

impl Scatter {
    /// `fraction` of both parts: "2 percent scatter" is `Scatter::of(0.02, seed)`.
    pub fn of(fraction: f64, seed: u64) -> Scatter {
        Scatter {
            relative: fraction,
            floor: fraction,
            seed,
        }
    }
}

/// The testbed's sample rate, Hz.
pub const FS_HZ: f64 = 16384.0;
/// Samples per update (10 ms rounded to whole samples).
pub const TICK_SAMPLES: usize = 164;
/// The updates the engage settle window lasts at 10 ms (under 0.3 s).
pub const ENGAGE_SETTLE_S: f64 = 0.3;

/// A schedule of one segment, counted in samples as the testbed's tone is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Schedule {
    /// A dwell.
    Dwell {
        /// Hz.
        f: f64,
        /// s.
        secs: f64,
    },
    /// A stepped sine, `per_octave` steps from `f0` to `f1`, each `secs` long.
    Steps {
        /// Hz.
        f0: f64,
        /// Hz.
        f1: f64,
        /// Steps per octave.
        per_octave: f64,
        /// Each step's dwell, s.
        secs: f64,
    },
    /// A logarithmic sweep.
    Sweep {
        /// Hz.
        f0: f64,
        /// Hz.
        f1: f64,
        /// Octaves a minute.
        oct_min: f64,
    },
}

/// The tone's state after `k` intervals: (frequency the law is told, frequency
/// of the interval's last sample, sweep rate Hz/s, sweep rate oct/min, done).
pub fn tone_at(s: Schedule, k: usize) -> (f64, f64, f64, f64, bool) {
    let consumed = k * TICK_SAMPLES;
    match s {
        Schedule::Dwell { f, secs } => {
            let total = (secs * FS_HZ).round() as usize;
            (f, f, 0.0, 0.0, consumed >= total)
        }
        Schedule::Steps {
            f0,
            f1,
            per_octave,
            secs,
        } => {
            let n_oct = (f1 / f0).log2().abs();
            let nst = (n_oct * per_octave + 1e-9).floor() as usize + 1;
            let sign = if f1 >= f0 { 1.0 } else { -1.0 };
            let step_f = |i: usize| f0 * 2f64.powf(sign * i as f64 / per_octave);
            let ls = (secs * FS_HZ).round() as usize;
            let total = nst * ls;
            if k == 0 {
                return (step_f(0), step_f(0), 0.0, 0.0, false);
            }
            if consumed >= total {
                let last = step_f(nst - 1);
                return (last, last, 0.0, 0.0, true);
            }
            let played = step_f((consumed - 1) / ls);
            (step_f(consumed / ls), played, 0.0, 0.0, false)
        }
        Schedule::Sweep { f0, f1, oct_min } => {
            let sign = if f1 >= f0 { 1.0 } else { -1.0 };
            let total = (f1 / f0).log2().abs();
            let dpos = oct_min / 60.0 / FS_HZ;
            let samples = ((total / dpos) - 1e-9).ceil() as usize;
            let done = consumed >= samples;
            let pos = (consumed.min(samples) as f64 * dpos).min(total);
            let f = f0 * 2f64.powf(sign * pos);
            if done || k == 0 {
                return (f, f, 0.0, 0.0, done);
            }
            let octm = sign * oct_min;
            (f, f, f * 2f64.ln() * octm / 60.0, octm, false)
        }
    }
}

/// One update of a closed-loop run.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Frequency the law was told, Hz.
    pub f: f64,
    /// Tracked controls the law saw.
    pub controls: Vec<C64>,
    /// The drive played (after the loop's own ceiling, if any).
    pub drive: Vec<C64>,
    /// The drive the law asked for.
    pub asked: Vec<C64>,
    /// Its diagnostics.
    pub values: Vec<f64>,
}

/// How the loop treats the law's answer before it is played.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Played {
    /// As asked.
    AsAsked,
    /// The whole vector scaled, direction kept, so that no drive is over this
    /// ceiling (volts peak).
    Ceiling(f64),
}

/// How the structure and the tracking filter answer between updates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dynamics {
    /// The research testbed's fast path: the structure in steady state at the
    /// interval's last frequency, a first-order lag of `2 / (2 pi B)` for the
    /// filter. No build-up of a mode.
    QuasiSteady,
    /// Each mode's resonant part builds up as one pole sample by sample
    /// (`Y' = (p - j w) Y + k u`, exact with the force held over a sample,
    /// the drive ramped across the interval), the rest of the mode answers at
    /// once, and the tracking filter is its two one-pole stages per sample.
    Envelope,
}

/// Run `law` on `st` through `schedule` with a reference of ones and no
/// guards.
#[allow(clippy::too_many_arguments)]
pub fn run_closed_loop(
    law: &mut dyn SineLaw,
    st: &ModalStructure,
    setup: &Setup,
    schedule: Schedule,
    played: Played,
    dynamics: Dynamics,
    values: usize,
    max_updates: usize,
) -> Result<Vec<Row>, String> {
    let ones = |_: f64| 1.0;
    run_closed_loop_with(
        law,
        st,
        setup,
        schedule,
        played,
        dynamics,
        values,
        max_updates,
        &ones,
    )
}

/// A reference amplitude against frequency, the same on every control, in
/// phase: `lo` up to `f1`, `hi` from `f2`, straight in the logarithms of
/// both between them (as a profile's rows are joined).
pub fn stepped_reference(lo: f64, hi: f64, f1: f64, f2: f64) -> impl Fn(f64) -> f64 {
    move |f| {
        if f <= f1 {
            lo
        } else if f >= f2 {
            hi
        } else {
            let t = (f.ln() - f1.ln()) / (f2.ln() - f1.ln());
            (lo.ln() + t * (hi.ln() - lo.ln())).exp()
        }
    }
}

/// The setup's reference on its grid from `reference` (the same amplitude on
/// every control, in phase), as the controller samples a profile for a law.
pub fn sample_reference(setup: &mut Setup, reference: &dyn Fn(f64) -> f64) {
    let lines = setup.identification.grid_hz.len();
    for c in 0..setup.controls {
        for k in 0..lines {
            let f = setup.identification.grid_hz[k];
            setup.reference[c * lines + k] = C64::new(reference(f), 0.0);
        }
    }
}

/// [`run_closed_loop`] with the reference told the law each update taken from
/// `reference` at the frequency told (the same amplitude on every control,
/// in phase), as the controller reads its profile.
#[allow(clippy::too_many_arguments)]
pub fn run_closed_loop_with(
    law: &mut dyn SineLaw,
    st: &ModalStructure,
    setup: &Setup,
    schedule: Schedule,
    played: Played,
    dynamics: Dynamics,
    values: usize,
    max_updates: usize,
    reference: &dyn Fn(f64) -> f64,
) -> Result<Vec<Row>, String> {
    let modes = st.f_hz.len();
    let mut env = vec![ZERO; modes];
    let mut stages = vec![[ZERO; 2]; st.controls];
    let (n, m) = (st.controls, st.drives);
    let mut r = vec![C64::new(1.0, 0.0); n];
    let mut z = vec![ZERO; n];
    let mut d_force = vec![ZERO; m];
    let mut out = Outputs::new(m, values);
    let mut rows = Vec::new();
    let mut h = vec![ZERO; n * m];
    let dt = TICK_SAMPLES as f64 / FS_HZ;
    let mut hold = Hold::Engage;
    for k in 0..max_updates {
        let (f, _, hz_s, oct, done) = tone_at(schedule, k);
        r.iter_mut().for_each(|v| *v = C64::new(reference(f), 0.0));
        let x = Inputs {
            index: k as u64,
            dt_s: if k == 0 { 0.0 } else { dt },
            f_hz: f,
            sweep_hz_s: hz_s,
            sweep_oct_min: oct,
            level: 1.0,
            reference: &r,
            controls: &z,
            limits: &[],
            limit_bounds: &[],
            drive_in_force: &d_force,
            hold,
            notch_scale: 1.0,
            settle_open: (k as f64) * dt < ENGAGE_SETTLE_S,
        };
        if k == 0 {
            law.engage(&x, &mut out)?;
        } else {
            law.tick(&x, &mut out)?;
        }
        if out.drive.iter().any(|v| !v.is_finite()) {
            return Err(format!("non-finite drive at update {k}"));
        }
        let d_new: Vec<C64> = match played {
            Played::AsAsked => out.drive.clone(),
            Played::Ceiling(c) => {
                let top = out.drive.iter().map(|v| v.norm()).fold(0.0, f64::max);
                let s = if top > c { c / top } else { 1.0 };
                out.drive.iter().map(|v| v * s).collect()
            }
        };
        rows.push(Row {
            f,
            controls: z.clone(),
            drive: d_new.clone(),
            asked: out.drive.clone(),
            values: out.values.clone(),
        });
        hold = if (k as f64) * dt + dt < ENGAGE_SETTLE_S {
            Hold::Engage
        } else {
            Hold::Free
        };
        if done {
            break;
        }
        let (_, fe, _, _, _) = tone_at(schedule, k + 1);
        let b = setup.tracking.bandwidth_hz(fe);
        match dynamics {
            Dynamics::QuasiSteady => {
                st.frf(fe, &mut h);
                let a = 1.0 - (-dt * 2.0 * PI * b / 2.0).exp();
                for (i, zi) in z.iter_mut().enumerate() {
                    let yt = (0..m).fold(ZERO, |s, j| s + h[i * m + j] * d_new[j]);
                    *zi += (yt - *zi) * a;
                }
            }
            Dynamics::Envelope => {
                st.envelope_interval(fe, b, &d_force, &d_new, &mut env, &mut stages);
                for (zi, sg) in z.iter_mut().zip(&stages) {
                    *zi = sg[1];
                }
            }
        }
        d_force = d_new;
    }
    Ok(rows)
}
