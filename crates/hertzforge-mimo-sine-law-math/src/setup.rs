//! What a law is built with, fixed for the run, and how it is read at a
//! frequency: linear interpolation between the identification's lines, and the
//! identification's group delay near the tone.

use crate::C64;
use hertzforge_sdm::MatrixStack;

/// The shape of a setup: controls against drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// As many controls as drives: the inverse.
    Square,
    /// More controls than drives: least squares.
    Tall,
    /// More drives than controls: least norm, with spare drives.
    Wide,
}

impl Shape {
    /// The shape of `controls` by `drives`.
    pub fn of(controls: usize, drives: usize) -> Shape {
        match controls.cmp(&drives) {
            std::cmp::Ordering::Equal => Shape::Square,
            std::cmp::Ordering::Greater => Shape::Tall,
            std::cmp::Ordering::Less => Shape::Wide,
        }
    }
}

/// The tracking filter's bandwidth against frequency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrackingRule {
    /// `min(f pct / 100, max_hz)`.
    Proportional {
        /// Percent of the tone's frequency.
        pct: f64,
        /// The largest bandwidth, Hz.
        max_hz: f64,
    },
    /// A fixed bandwidth, Hz.
    Fixed {
        /// Bandwidth, Hz.
        hz: f64,
    },
}

impl TrackingRule {
    /// The bandwidth at `f_hz`, never more than half the frequency.
    pub fn bandwidth_hz(&self, f_hz: f64) -> f64 {
        let b = match *self {
            TrackingRule::Proportional { pct, max_hz } => (f_hz * pct / 100.0).min(max_hz),
            TrackingRule::Fixed { hz } => hz,
        };
        b.min(0.5 * f_hz)
    }

    /// The filter's group delay at `f_hz` with the default delay factor (two
    /// one-pole stages, `2 / (2 pi B)`).
    pub fn delay_s(&self, f_hz: f64) -> f64 {
        self.delay_s_with(f_hz, DEFAULT_TRACKER_DELAY_FACTOR)
    }

    /// The filter's group delay at `f_hz` for a tracker whose delay is
    /// `factor / B`.
    pub fn delay_s_with(&self, f_hz: f64, factor: f64) -> f64 {
        factor / self.bandwidth_hz(f_hz)
    }
}

/// The delay factor of the research testbed's tracker, two one-pole stages:
/// `1 / pi`, so its delay is `1 / (pi B)`.
pub const DEFAULT_TRACKER_DELAY_FACTOR: f64 = 1.0 / std::f64::consts::PI;

/// What the loop takes between a drive and its reading besides the
/// structure's own response: the tracker's delay (as a factor over its
/// bandwidth) and a transport delay the identification does not show.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopDelay {
    /// The tracker's group delay times its bandwidth: its delay is
    /// `tracker_delay_factor / B`. Default [`DEFAULT_TRACKER_DELAY_FACTOR`].
    pub tracker_delay_factor: f64,
    /// The delay between a drive and its reading that the identification
    /// does not show, s. Default 0.
    pub transport_delay_s: f64,
}

impl Default for LoopDelay {
    fn default() -> Self {
        LoopDelay {
            tracker_delay_factor: DEFAULT_TRACKER_DELAY_FACTOR,
            transport_delay_s: 0.0,
        }
    }
}

/// The identification taken before the run, on its grid.
#[derive(Clone, Debug, PartialEq)]
pub struct Identification {
    /// The lines, Hz, ascending.
    pub grid_hz: Vec<f64>,
    /// Control response over drive, `lines x controls x drives`.
    pub response: MatrixStack,
    /// Limit-channel response over drive, `lines x limits x drives`.
    pub limit_response: MatrixStack,
    /// Multiple coherence of each control, `controls x lines`.
    pub coherence: Vec<f64>,
    /// Multiple coherence of each limit channel, `limits x lines`.
    pub limit_coherence: Vec<f64>,
    /// Whether each line is trusted.
    pub trusted: Vec<bool>,
}

/// Everything a law is built with.
#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    /// Control channels.
    pub controls: usize,
    /// Drives.
    pub drives: usize,
    /// Limit channels.
    pub limits: usize,
    /// Sample rate, Hz.
    pub fs_hz: f64,
    /// Time between updates, s.
    pub tick_s: f64,
    /// The test's band, Hz.
    pub band_lo_hz: f64,
    /// The test's band, Hz.
    pub band_hi_hz: f64,
    /// Each drive's ceiling, volts peak.
    pub drive_ceiling_v: Vec<f64>,
    /// The identification.
    pub identification: Identification,
    /// The level the identification was taken at, dB against full level.
    pub identification_level_db: f64,
    /// The full-level reference on the identification's grid, `controls x lines`.
    pub reference: Vec<C64>,
    /// `false`: amplitude only, the phases between controls are free.
    pub phase_specified: bool,
    /// Each limit channel's bound on the grid, `limits x lines` (infinite where none).
    pub limit_bounds: Vec<f64>,
    /// Alarm band per control, dB.
    pub alarm_db: Vec<f64>,
    /// Abort band per control, dB.
    pub abort_db: Vec<f64>,
    /// The shape, from the counts.
    pub shape: Shape,
    /// The tracking filter's bandwidth rule.
    pub tracking: TrackingRule,
    /// The tracker's delay factor and the transport delay the identification
    /// does not show.
    pub loop_delay: LoopDelay,
}

impl Setup {
    /// Check every count and length against each other.
    pub fn check(&self) -> Result<(), String> {
        let id = &self.identification;
        let lines = id.grid_hz.len();
        let (n, m, p) = (self.controls, self.drives, self.limits);
        if n == 0 || m == 0 {
            return Err("a setup needs at least one control and one drive".into());
        }
        if lines < 2 {
            return Err("the identification needs at least two lines".into());
        }
        if id
            .grid_hz
            .windows(2)
            .any(|w| w[1].partial_cmp(&w[0]) != Some(std::cmp::Ordering::Greater))
        {
            return Err("the identification's grid must ascend strictly".into());
        }
        if id.response.dims() != (lines, n, m) {
            return Err(format!(
                "the response must be {lines} lines by {n} x {m}, got {:?}",
                id.response.dims()
            ));
        }
        if p > 0 && id.limit_response.dims() != (lines, p, m) {
            return Err("the limit response does not match the counts".into());
        }
        if id.trusted.len() != lines || id.coherence.len() != n * lines {
            return Err("the trust mask or the coherence does not match the grid".into());
        }
        if self.reference.len() != n * lines {
            return Err("the reference must be controls x lines".into());
        }
        if self.drive_ceiling_v.len() != m {
            return Err("one drive ceiling per drive".into());
        }
        if self.shape != Shape::of(n, m) {
            return Err("the shape does not match the counts".into());
        }
        if !(self.tick_s > 0.0 && self.tick_s.is_finite()) {
            return Err("the tick must be a positive time".into());
        }
        let ld = &self.loop_delay;
        if !(ld.tracker_delay_factor.is_finite() && ld.tracker_delay_factor > 0.0) {
            return Err("the tracker's delay factor must be a number above 0".into());
        }
        if !(ld.transport_delay_s.is_finite() && ld.transport_delay_s >= 0.0) {
            return Err("the transport delay must be a time of 0 or more".into());
        }
        if !id.response.is_finite() || self.reference.iter().any(|v| !v.is_finite()) {
            return Err("the identification and the reference must be finite".into());
        }
        Ok(())
    }

    /// The identification's response at `f_hz` (`controls x drives`, row-major).
    pub fn response_at(&self, f_hz: f64, out: &mut [C64]) {
        interp_stack(
            &self.identification.grid_hz,
            self.identification.response.data(),
            self.controls * self.drives,
            f_hz,
            out,
        );
    }

    /// The full-level reference at `f_hz`.
    pub fn reference_at(&self, f_hz: f64, out: &mut [C64]) {
        let g = &self.identification.grid_hz;
        let (k0, k1, t) = bracket(g, f_hz);
        let lines = g.len();
        for (c, o) in out.iter_mut().enumerate().take(self.controls) {
            let a = self.reference[c * lines + k0];
            let b = self.reference[c * lines + k1];
            *o = a * (1.0 - t) + b * t;
        }
    }

    /// Whether the lines either side of `f_hz` are both trusted.
    pub fn trusted_at(&self, f_hz: f64) -> bool {
        let g = &self.identification.grid_hz;
        let k = g.partition_point(|&x| x < f_hz).clamp(1, g.len() - 1);
        self.identification.trusted[k - 1] && self.identification.trusted[k]
    }
}

/// The two lines around `f` and the weight on the upper one, clamped to the
/// grid: `f` at or below the first line gives the first, at or above the last
/// gives the last.
pub fn bracket(grid: &[f64], f: f64) -> (usize, usize, f64) {
    let l = grid.len();
    let f = f.clamp(grid[0], grid[l - 1]);
    let k = grid.partition_point(|&x| x < f);
    if k == 0 {
        return (0, 0, 0.0);
    }
    if k >= l {
        return (l - 1, l - 1, 0.0);
    }
    let t = (f - grid[k - 1]) / (grid[k] - grid[k - 1]);
    (k - 1, k, t)
}

/// Linear interpolation of a line-major stack (`size` values a line) at `f`.
pub fn interp_stack(grid: &[f64], data: &[C64], size: usize, f: f64, out: &mut [C64]) {
    let (k0, k1, t) = bracket(grid, f);
    let a = &data[k0 * size..(k0 + 1) * size];
    let b = &data[k1 * size..(k1 + 1) * size];
    for ((o, x), y) in out.iter_mut().zip(a).zip(b) {
        *o = x * (1.0 - t) + y * t;
    }
}

/// How the identification's group delay is read on each line.
///
/// The loop-speed rule reads it to keep the loop slower than the structure's
/// own decay near a resonance. Each form takes `|d arg / d w|` of the
/// identification line by line, smooths it over five lines, and is read as the
/// largest within the window the rule asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DelayEstimate {
    /// The largest phase slope over the elements at or above a tenth of the
    /// line's largest magnitude (the research testbed's form). On an
    /// identification with noise, an element just over the tenth can set it
    /// with a phase slope that is the noise's, not the structure's.
    #[default]
    Largest,
    /// The phase slope of the response along its largest singular direction:
    /// `u^H H(w) v`, with `u` and `v` the line's first singular vectors held
    /// over the line and its two neighbours. At a resonance the mode is that
    /// direction and its delay is read; a small element's noise enters in
    /// proportion to its share of the direction, so it cannot set the value.
    Principal,
}

impl DelayEstimate {
    /// The declared name of each form, in order.
    pub const NAMES: [&'static str; 2] = ["largest", "principal"];

    /// The declared name.
    pub fn name(self) -> &'static str {
        match self {
            DelayEstimate::Largest => "largest",
            DelayEstimate::Principal => "principal",
        }
    }

    /// The form a declared name names.
    pub fn from_name(name: &str) -> Option<DelayEstimate> {
        match name {
            "largest" => Some(DelayEstimate::Largest),
            "principal" => Some(DelayEstimate::Principal),
            _ => None,
        }
    }
}

/// The per-line forms measured when the estimate was chosen: the two declared
/// ones and the candidates set aside (kept so the evidence can be rerun).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Form {
    /// Over the elements: the largest at or above `floor` times the line's
    /// largest magnitude, or with `power`, the mean weighted by magnitude to
    /// that power.
    Elements { floor: f64, power: Option<i32> },
    /// Along the singular directions: the first only, or the largest over
    /// every direction at or above a tenth of the first.
    Directions { all: bool },
}

/// The identification's group delay against frequency: the structure's own
/// response time near the tone (about `1 / (zeta w)` at a light resonance).
#[derive(Clone, Debug, PartialEq)]
pub struct GroupDelay {
    grid: Vec<f64>,
    gd: Vec<f64>,
}

impl GroupDelay {
    /// Per line: the largest `|d arg H / d w|` over the elements at or above a
    /// tenth of the line's largest magnitude, smoothed over five lines
    /// ([`DelayEstimate::Largest`]).
    pub fn of(id: &Identification) -> GroupDelay {
        Self::of_with(id, DelayEstimate::Largest)
    }

    /// Per line, by the form `how`, smoothed over five lines.
    pub fn of_with(id: &Identification, how: DelayEstimate) -> GroupDelay {
        Self::of_form(
            id,
            match how {
                DelayEstimate::Largest => Form::Elements {
                    floor: 0.1,
                    power: None,
                },
                DelayEstimate::Principal => Form::Directions { all: false },
            },
        )
    }

    /// A candidate form set aside when the estimate was chosen, by name:
    /// `weighted` (magnitude squared), `weighted4` (magnitude to the fourth),
    /// `half` (the largest over elements at or above half the line's
    /// largest), `directions` (the largest over singular directions at or
    /// above a tenth of the first). For the evidence only.
    #[cfg(feature = "reference-structure")]
    pub fn of_candidate(id: &Identification, name: &str) -> Option<GroupDelay> {
        let form = match name {
            "weighted" => Form::Elements {
                floor: 0.0,
                power: Some(2),
            },
            "weighted4" => Form::Elements {
                floor: 0.0,
                power: Some(4),
            },
            "half" => Form::Elements {
                floor: 0.5,
                power: None,
            },
            "directions" => Form::Directions { all: true },
            other => {
                return DelayEstimate::from_name(other).map(|h| Self::of_with(id, h));
            }
        };
        Some(Self::of_form(id, form))
    }

    fn of_form(id: &Identification, form: Form) -> GroupDelay {
        let raw = match form {
            Form::Elements { floor, power } => element_slopes(id, floor, power),
            Form::Directions { all } => principal_slopes(id, all),
        };
        let l = raw.len();
        // A five-line moving average, zero beyond the ends (a "same" convolution).
        let gd = (0..l)
            .map(|k| {
                let a = k.saturating_sub(2);
                let b = (k + 3).min(l);
                raw[a..b].iter().sum::<f64>() / 5.0
            })
            .collect();
        GroupDelay {
            grid: id.grid_hz.clone(),
            gd,
        }
    }

    /// The smoothed delay on each line of the grid, s.
    pub fn lines(&self) -> (&[f64], &[f64]) {
        (&self.grid, &self.gd)
    }

    /// The group delay at `f_hz`, s. With `span_hz > 0`, the largest within
    /// `f +- span_hz`: a nearby light mode's free response limits the loop
    /// even off its resonance.
    pub fn at(&self, f_hz: f64, span_hz: f64) -> f64 {
        let here = interp_real(&self.grid, &self.gd, f_hz);
        if span_hz > 0.0 {
            let a = self.grid.partition_point(|&x| x < f_hz - span_hz);
            let b = self.grid.partition_point(|&x| x <= f_hz + span_hz);
            if b > a {
                let m = self.gd[a..b]
                    .iter()
                    .copied()
                    .fold(f64::NEG_INFINITY, f64::max);
                return m.max(here);
            }
        }
        here
    }
}

/// Per line, from each element's unwrapped phase slope: the largest over the
/// elements at or above a tenth of the line's largest, or the mean weighted by
/// magnitude squared.
fn element_slopes(id: &Identification, floor: f64, power: Option<i32>) -> Vec<f64> {
    let g = &id.grid_hz;
    let l = g.len();
    let (_, r, c) = id.response.dims();
    let e = r * c;
    let w: Vec<f64> = g.iter().map(|f| 2.0 * std::f64::consts::PI * f).collect();
    let mut raw = vec![0.0f64; l];
    let mut weight = vec![0.0f64; l];
    let mut ph = vec![0.0f64; l];
    let mut d = vec![0.0f64; l];
    let mut big_max = vec![0.0f64; l];
    for (k, bm) in big_max.iter_mut().enumerate() {
        *bm = id
            .response
            .line(k)
            .iter()
            .map(|v| v.norm())
            .fold(0.0, f64::max);
    }
    for el in 0..e {
        for (k, p) in ph.iter_mut().enumerate() {
            *p = id.response.line(k)[el].arg();
        }
        unwrap(&mut ph);
        gradient(&ph, &w, &mut d);
        for k in 0..l {
            let a = id.response.line(k)[el].norm();
            match power {
                Some(pw) => {
                    let wt = a.powi(pw);
                    raw[k] += wt * d[k].abs();
                    weight[k] += wt;
                }
                None => {
                    if a >= floor * big_max[k] {
                        raw[k] = raw[k].max(d[k].abs());
                    }
                }
            }
        }
    }
    if power.is_some() {
        for (v, s) in raw.iter_mut().zip(&weight) {
            *v = if *s > 0.0 { *v / s } else { 0.0 };
        }
    }
    raw
}

/// Per line: the phase slope of `u^H H v` over the line and its neighbours,
/// with `u`, `v` the line's first singular vectors (with `all`, the largest
/// over the directions at or above a tenth of the first).
fn principal_slopes(id: &Identification, all: bool) -> Vec<f64> {
    let g = &id.grid_hz;
    let l = g.len();
    let w: Vec<f64> = g.iter().map(|f| 2.0 * std::f64::consts::PI * f).collect();
    let (_, r, c) = id.response.dims();
    let Ok(d) = hertzforge_sdm::linalg::svd(&id.response) else {
        return element_slopes(id, 0.0, Some(2));
    };
    let k_sv = d.k();
    let project = |line: usize, u: &[C64], v: &[C64]| -> C64 {
        let h = id.response.line(line);
        let mut s = C64::new(0.0, 0.0);
        for i in 0..r {
            let hv = (0..c).fold(C64::new(0.0, 0.0), |acc, j| acc + h[i * c + j] * v[j]);
            s += u[i].conj() * hv;
        }
        s
    };
    let mut out = vec![0.0f64; l];
    let mut u = vec![C64::new(0.0, 0.0); r];
    let mut v = vec![C64::new(0.0, 0.0); c];
    for k in 0..l {
        let ul = d.u.line(k);
        let vl = d.v.line(k);
        let s0 = d.s[k * k_sv];
        let count = if all { k_sv } else { 1 };
        for q in 0..count {
            if d.s[k * k_sv + q] < 0.1 * s0 {
                break;
            }
            for (i, ui) in u.iter_mut().enumerate() {
                *ui = ul[i * k_sv + q];
            }
            for (j, vj) in v.iter_mut().enumerate() {
                *vj = vl[j * k_sv + q];
            }
            let p0 = project(k, &u, &v);
            if p0.norm() == 0.0 {
                continue;
            }
            let (a, b) = (k.saturating_sub(1), (k + 1).min(l - 1));
            // Phases relative to the line's own: small angles, no unwrap needed.
            let ph = |j: usize| (project(j, &u, &v) / p0).arg();
            let slope = if a == k {
                ph(b) / (w[b] - w[k])
            } else if b == k {
                -ph(a) / (w[k] - w[a])
            } else {
                let (hs, hd) = (w[k] - w[a], w[b] - w[k]);
                (hs * hs * ph(b) - hd * hd * ph(a)) / (hs * hd * (hd + hs))
            };
            out[k] = out[k].max(slope.abs());
        }
    }
    out
}

/// Linear interpolation of real values, clamped at the ends.
pub fn interp_real(grid: &[f64], y: &[f64], f: f64) -> f64 {
    let (k0, k1, t) = bracket(grid, f);
    y[k0] * (1.0 - t) + y[k1] * t
}

/// Phase unwrapping along a sequence (jumps over pi replaced by their
/// 2 pi complement).
pub fn unwrap(p: &mut [f64]) {
    use std::f64::consts::PI;
    let mut acc = 0.0;
    let mut prev = p.first().copied().unwrap_or(0.0);
    for v in p.iter_mut().skip(1) {
        let dd = *v - prev;
        prev = *v;
        let mut m = (dd + PI).rem_euclid(2.0 * PI) - PI;
        if m == -PI && dd > 0.0 {
            m = PI;
        }
        let corr = if dd.abs() < PI { 0.0 } else { m - dd };
        acc += corr;
        *v += acc;
    }
}

/// The derivative of `y` against `x` (second order inside, first order at the
/// ends; `x` may be unevenly spaced).
pub fn gradient(y: &[f64], x: &[f64], out: &mut [f64]) {
    let l = y.len();
    if l < 2 {
        out.iter_mut().for_each(|o| *o = 0.0);
        return;
    }
    out[0] = (y[1] - y[0]) / (x[1] - x[0]);
    out[l - 1] = (y[l - 1] - y[l - 2]) / (x[l - 1] - x[l - 2]);
    for k in 1..l - 1 {
        let hs = x[k] - x[k - 1];
        let hd = x[k + 1] - x[k];
        out[k] = (hs * hs * y[k + 1] + (hd * hd - hs * hs) * y[k] - hd * hd * y[k - 1])
            / (hs * hd * (hd + hs));
    }
}
