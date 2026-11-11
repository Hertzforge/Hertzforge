//! The jointed modal plant: a cross-coupled modal structure whose resonances fall and
//! whose damping rises with amplitude, seen through a delay per output.
//!
//! # The model
//!
//! Each mode `m` is a unit-mass oscillator in its modal displacement `q_m`, forced by
//! the inner product of its input shape with the drive frame `d` (after the common
//! transport delay):
//!
//! ```text
//!   q_m'' + 2 zeta_m w_m q_m' + w_m^2 q_m = u_m = input_shape_m . d
//!   a_m   = q_m''                                   the mode's acceleration
//!   y_i   = sum_m output_shape_m[i] a_m + sum_j residual[i][j] d_j + feedthrough sum_j d_j
//! ```
//!
//! and output `i` is `y_i` delayed by `output_delays_s[i]` (whole samples plus a
//! fraction, by a four-tap Lagrange interpolator, exact at whole samples and most
//! accurate well below a quarter of the rate: the tests measure each standard plant's
//! delays within a tenth of a sample over the shock band, to 2 kHz at 12 800 Hz), then
//! noise is added. A mode's
//! acceleration response is `s^2 / (s^2 + 2 zeta w s + w^2)` (the
//! [`crate::ModeResponse::Acceleration`] shape); the `residual` is the structure's mass
//! line below its first mode.
//!
//! # The joint
//!
//! With a [`JointLaw`], each mode keeps a running amplitude envelope `A_m`: a peak
//! follower, decaying by `exp(-1)` in `decay_cycles` periods of the mode's small-signal
//! frequency, of the mode's amplitude `sqrt(q_m^2 + (q_m' / w_m)^2)`. That amplitude is
//! the peak displacement of the mode's present motion (its energy over its stiffness),
//! so it is steady through a cycle of steady motion: a follower of `|q_m|` alone dips
//! between peaks, and the stiffness it then modulates twice a cycle pumps the mode,
//! taking away much of its damping. In a free decay the amplitude falls slower than
//! the follower can, so the envelope tracks it and the frequency glides back up as the
//! motion dies. With `r = A_m / reference_amplitude_m`, each sample's frequency and
//! damping are
//!
//! ```text
//!   f_m    = f0_m (1 - delta min(1, r)^freq_exponent)
//!   zeta_m = zeta0_m (1 + damping_gain r^damping_exponent)       (at most MAX_DAMPING)
//! ```
//!
//! so at the reference amplitude the mode has dropped by `delta` and its damping is
//! `1 + damping_gain` times its small-signal damping. The frequency stops falling at
//! the reference amplitude; the damping keeps rising (a joint slips more the harder it
//! is pushed) up to [`MAX_DAMPING`].
//!
//! # The step, and why it is stable at any amplitude
//!
//! Each sample advances every mode by the exact solution of its equation over one
//! sample period with that sample's frequency and damping held and the modal force
//! varying linearly between the previous sample and this one (a triangle hold, which
//! adds no phase). With the parameters held, that step is the true motion of a damped
//! oscillator over one period, which never gains energy, for any frequency and any
//! damping up to [`MAX_DAMPING`]; nothing in it can overflow the way an explicit
//! integrator does past its step limit. The parameters move only through the envelope:
//! the frequency falls as the envelope rises (which takes energy out) and rises back no
//! faster than the envelope's decay, several periods, so the slow modulation cannot
//! pump the mode. The tests drive every standard plant on every drive with random,
//! repeated pulses, and sines at its first mode's shifted resonance and at twice it, at
//! the full drive range (10 V) and at a hundred thousand times it, and check the
//! response against the bound of its small-signal modes.
//!
//! # What its FRF means
//!
//! A level-dependent plant has no single FRF. [`crate::PlantModel::analytic_frf`] of a
//! jointed plant is its **small-signal** FRF (every envelope zero, every mode at its
//! small-signal frequency and damping): what an identification at a low level should
//! find. [`JointedPlant::frf_at_envelope`] gives the FRF with every mode's frequency and
//! damping frozen at a stated envelope, the plant an identification at that level is
//! compared with. Both are exact for the discrete plant the backend steps (a frozen
//! step is linear and time-invariant), the output delays included.

use crate::plant::{fft_exponent_sign, inverse_dft};
use crate::{DelayLine, Xorshift64};
use hertzforge_dsp::fft::{self, Cplx};
use hertzforge_sdm::{MatrixStack, C64};
use std::f64::consts::TAU;

/// The largest damping ratio a jointed mode takes, however far past its reference
/// amplitude it is driven. It keeps every mode underdamped.
pub const MAX_DAMPING: f64 = 0.9;

/// The longest output delay a jointed plant takes, seconds.
pub const MAX_OUTPUT_DELAY_S: f64 = 0.1;

/// How a jointed plant's modes change with amplitude (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointLaw {
    /// The fractional frequency drop at and above the reference amplitude, `0 <= delta < 1`.
    pub delta: f64,
    /// The damping at the reference amplitude is `1 + damping_gain` times the
    /// small-signal damping (1 doubles it, 0 leaves it).
    pub damping_gain: f64,
    /// The exponent on the amplitude ratio in the frequency drop.
    pub freq_exponent: f64,
    /// The exponent on the amplitude ratio in the damping rise.
    pub damping_exponent: f64,
    /// The envelope falls by `exp(-1)` in this many periods of the mode's
    /// small-signal frequency.
    pub decay_cycles: f64,
}

impl JointLaw {
    /// A law with both exponents 0.5 (a joint's dissipation per cycle growing as the
    /// force amplitude to a power between 2 and 3) and a three-cycle envelope.
    pub fn new(delta: f64, damping_gain: f64) -> Self {
        JointLaw {
            delta,
            damping_gain,
            freq_exponent: 0.5,
            damping_exponent: 0.5,
            decay_cycles: 3.0,
        }
    }

    /// The frequency over its small-signal value at amplitude ratio `r`.
    pub fn frequency_factor(&self, r: f64) -> f64 {
        if r <= 0.0 {
            1.0
        } else {
            1.0 - self.delta * r.min(1.0).powf(self.freq_exponent)
        }
    }

    /// The damping ratio at amplitude ratio `r` of a mode whose small-signal damping
    /// is `zeta0`.
    pub fn damping(&self, zeta0: f64, r: f64) -> f64 {
        if r <= 0.0 {
            zeta0
        } else {
            (zeta0 * (1.0 + self.damping_gain * r.powf(self.damping_exponent))).min(MAX_DAMPING)
        }
    }

    fn validate(&self) -> Result<(), String> {
        if !(self.delta.is_finite() && (0.0..1.0).contains(&self.delta)) {
            return Err(format!(
                "a joint's frequency drop {} is not in [0, 1)",
                self.delta
            ));
        }
        if !(self.damping_gain.is_finite() && self.damping_gain >= 0.0) {
            return Err(format!(
                "a joint's damping gain {} is negative or not finite",
                self.damping_gain
            ));
        }
        for (what, v) in [
            ("frequency exponent", self.freq_exponent),
            ("damping exponent", self.damping_exponent),
            ("envelope decay", self.decay_cycles),
        ] {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("a joint's {what} {v} is not positive"));
            }
        }
        Ok(())
    }
}

/// One mode of a [`JointedPlant`].
#[derive(Debug, Clone, PartialEq)]
pub struct JointedMode {
    /// Small-signal natural frequency, Hz (below the Nyquist frequency).
    pub f_hz: f64,
    /// Small-signal damping ratio, above zero and under [`MAX_DAMPING`].
    pub zeta: f64,
    /// Participation of each drive, `n_drives` long.
    pub input_shape: Vec<f64>,
    /// How each output sees the mode's acceleration, `n_inputs` long.
    pub output_shape: Vec<f64>,
    /// The modal displacement amplitude at which the joint law reaches its full drop.
    pub reference_amplitude: f64,
}

/// A cross-coupled modal plant with level-dependent modes and a delay per output.
/// See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct JointedPlant {
    /// Outputs (measurement inputs; rows of the FRF).
    pub n_inputs: usize,
    /// Drives (columns of the FRF).
    pub n_drives: usize,
    pub modes: Vec<JointedMode>,
    /// The amplitude law, or `None` for a linear plant.
    pub joint: Option<JointLaw>,
    /// Added to every FRF element.
    pub feedthrough: f64,
    /// Row-major `n_inputs x n_drives`, or empty for none: the mass line.
    pub residual: Vec<f64>,
    /// Whole samples on every drive before the structure, common to every path.
    pub transport_delay_samples: usize,
    /// Each output's own delay, seconds (whole samples plus a fraction at the
    /// configured rate), or empty for none. At most [`MAX_OUTPUT_DELAY_S`].
    pub output_delays_s: Vec<f64>,
}

/// What [`JointedPlant::simulate`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct Simulated {
    /// `n_inputs` records, each as long as the drive records.
    pub outputs: Vec<Vec<f64>>,
    /// Each mode's largest envelope over the run.
    pub peak_envelope: Vec<f64>,
}

impl Simulated {
    /// Each mode's largest envelope over its reference amplitude.
    pub fn peak_ratio(&self, plant: &JointedPlant) -> Vec<f64> {
        self.peak_envelope
            .iter()
            .zip(&plant.modes)
            .map(|(a, m)| a / m.reference_amplitude)
            .collect()
    }
}

impl JointedPlant {
    /// A linear plant with no residual, no feedthrough and no delays.
    pub fn new(n_inputs: usize, n_drives: usize, modes: Vec<JointedMode>) -> Self {
        JointedPlant {
            n_inputs,
            n_drives,
            modes,
            joint: None,
            feedthrough: 0.0,
            residual: Vec::new(),
            transport_delay_samples: 0,
            output_delays_s: Vec::new(),
        }
    }

    /// The same plant with no joint law: what it is at a vanishing level.
    pub fn linear(&self) -> JointedPlant {
        JointedPlant {
            joint: None,
            ..self.clone()
        }
    }

    /// Mode `k` alone, one drive to one output with unit shapes, its own reference
    /// amplitude and this plant's law, no residual and no delays: the mode as a
    /// single-degree-of-freedom article, for measuring what the law does to it.
    pub fn single_mode(&self, k: usize) -> JointedPlant {
        let m = &self.modes[k];
        JointedPlant {
            joint: self.joint,
            ..JointedPlant::new(
                1,
                1,
                vec![JointedMode {
                    input_shape: vec![1.0],
                    output_shape: vec![1.0],
                    ..m.clone()
                }],
            )
        }
    }

    /// Check the plant can run at `fs_hz`; the message says what is wrong.
    pub fn validate(&self, fs_hz: f64) -> Result<(), String> {
        if self.n_inputs == 0 || self.n_drives == 0 {
            return Err(format!(
                "a jointed plant needs at least one output and one drive, got {} x {}",
                self.n_inputs, self.n_drives
            ));
        }
        if !(self.residual.is_empty() || self.residual.len() == self.n_inputs * self.n_drives) {
            return Err(format!(
                "the residual holds {} values; a {} x {} plant needs {} or none",
                self.residual.len(),
                self.n_inputs,
                self.n_drives,
                self.n_inputs * self.n_drives
            ));
        }
        if !self.feedthrough.is_finite() || self.residual.iter().any(|v| !v.is_finite()) {
            return Err("a jointed plant's feedthrough or residual is not finite".into());
        }
        for (k, m) in self.modes.iter().enumerate() {
            if m.input_shape.len() != self.n_drives || m.output_shape.len() != self.n_inputs {
                return Err(format!(
                    "mode {k} has shapes {} (drives) and {} (outputs); the plant is {} x {}",
                    m.input_shape.len(),
                    m.output_shape.len(),
                    self.n_inputs,
                    self.n_drives
                ));
            }
            if !(m.f_hz.is_finite() && m.f_hz > 0.0 && m.f_hz < 0.5 * fs_hz) {
                return Err(format!(
                    "mode {k} at {} Hz is not between 0 and the Nyquist frequency of {fs_hz} Hz",
                    m.f_hz
                ));
            }
            if !(m.zeta.is_finite() && m.zeta > 0.0 && m.zeta < MAX_DAMPING) {
                return Err(format!(
                    "mode {k} has damping {}; it must be above 0 and under {MAX_DAMPING}",
                    m.zeta
                ));
            }
            if !(m.reference_amplitude.is_finite() && m.reference_amplitude > 0.0) {
                return Err(format!(
                    "mode {k}'s reference amplitude {} is not positive",
                    m.reference_amplitude
                ));
            }
            if m.input_shape
                .iter()
                .chain(&m.output_shape)
                .any(|v| !v.is_finite())
            {
                return Err(format!("mode {k} has a non-finite shape value"));
            }
        }
        if let Some(j) = &self.joint {
            j.validate()?;
        }
        if !(self.output_delays_s.is_empty() || self.output_delays_s.len() == self.n_inputs) {
            return Err(format!(
                "{} output delays for {} outputs",
                self.output_delays_s.len(),
                self.n_inputs
            ));
        }
        if let Some(t) = self
            .output_delays_s
            .iter()
            .find(|t| !(t.is_finite() && **t >= 0.0 && **t <= MAX_OUTPUT_DELAY_S))
        {
            return Err(format!(
                "an output delay of {t} s is not between 0 and {MAX_OUTPUT_DELAY_S} s"
            ));
        }
        Ok(())
    }

    /// Output `i`'s delay in samples at `fs_hz`.
    pub fn output_delay_samples(&self, i: usize, fs_hz: f64) -> f64 {
        self.output_delays_s.get(i).copied().unwrap_or(0.0) * fs_hz
    }

    /// The FRF with every mode's frequency and damping frozen at envelope
    /// `envelope[m]` (modal displacement), on `freqs_hz`. An all-zero envelope is the
    /// small-signal FRF. Panics when the plant does not validate at `fs_hz` or the
    /// envelope is not one value per mode.
    pub fn frf_at_envelope(&self, freqs_hz: &[f64], fs_hz: f64, envelope: &[f64]) -> MatrixStack {
        if let Err(e) = self.validate(fs_hz) {
            panic!("frf_at_envelope on an invalid jointed plant: {e}");
        }
        assert_eq!(envelope.len(), self.modes.len(), "one envelope per mode");
        let steps: Vec<ModeStep> = self
            .modes
            .iter()
            .zip(envelope)
            .map(|(m, &a)| {
                let (w, z) = self.params(m, a);
                ModeStep::new(w, z, 1.0 / fs_hz)
            })
            .collect();
        let odelays: Vec<FracDelay> = (0..self.n_inputs)
            .map(|i| FracDelay::new(self.output_delay_samples(i, fs_hz)))
            .collect();
        let (n_in, n_d) = (self.n_inputs, self.n_drives);
        let mut out = MatrixStack::zeros(freqs_hz.len(), n_in, n_d);
        let mut rm = vec![C64::new(0.0, 0.0); self.modes.len()];
        for (k, &f) in freqs_hz.iter().enumerate() {
            let om = TAU * f / fs_hz;
            let z = C64::new(om.cos(), om.sin());
            for (r, s) in rm.iter_mut().zip(&steps) {
                *r = s.response(z);
            }
            let ph = -om * self.transport_delay_samples as f64;
            let dl = C64::new(ph.cos(), ph.sin());
            let line = out.line_mut(k);
            for i in 0..n_in {
                let li = odelays[i].response(om) * dl;
                for j in 0..n_d {
                    let mut h = C64::new(self.feedthrough, 0.0);
                    if !self.residual.is_empty() {
                        h.re += self.residual[i * n_d + j];
                    }
                    for (mode, r) in self.modes.iter().zip(&rm) {
                        h += r * (mode.output_shape[i] * mode.input_shape[j]);
                    }
                    line[i * n_d + j] = h * li;
                }
            }
        }
        out
    }

    /// The FRF with every mode frozen at `ratio` times its reference amplitude. At the
    /// standard plants' calibration, the reference pulse at a level `L` dB below full
    /// brings the dominant mode to about `ratio = 10^(L / 20)`.
    pub fn frf_at_reference_ratio(&self, freqs_hz: &[f64], fs_hz: f64, ratio: f64) -> MatrixStack {
        let env: Vec<f64> = self
            .modes
            .iter()
            .map(|m| ratio * m.reference_amplitude)
            .collect();
        self.frf_at_envelope(freqs_hz, fs_hz, &env)
    }

    /// Each mode's `(frequency Hz, damping ratio)` at envelope `envelope[m]`.
    pub fn modes_at_envelope(&self, envelope: &[f64]) -> Vec<(f64, f64)> {
        self.modes
            .iter()
            .zip(envelope)
            .map(|(m, &a)| {
                let (w, z) = self.params(m, a);
                (w / TAU, z)
            })
            .collect()
    }

    /// The plant at rest, to be stepped one frame at a time with no noise, by the same
    /// step the backend runs.
    pub fn stepper(&self, fs_hz: f64) -> Result<Stepper, String> {
        self.validate(fs_hz)?;
        Ok(Stepper {
            run: JointedRun::new(self, fs_hz, 0.0, 1),
        })
    }

    /// Play `drives` (`n_drives` records of one length) through the plant from rest,
    /// with no noise, by the same step the backend runs: each output's record and each
    /// mode's largest envelope.
    pub fn simulate(&self, fs_hz: f64, drives: &[Vec<f64>]) -> Result<Simulated, String> {
        self.validate(fs_hz)?;
        if drives.len() != self.n_drives {
            return Err(format!(
                "{} drive records for a plant with {} drives",
                drives.len(),
                self.n_drives
            ));
        }
        let n = drives.first().map_or(0, Vec::len);
        if drives.iter().any(|d| d.len() != n) {
            return Err("the drive records differ in length".into());
        }
        let mut run = JointedRun::new(self, fs_hz, 0.0, 1);
        let mut outputs = vec![Vec::with_capacity(n); self.n_inputs];
        let mut frame = vec![0.0; self.n_drives];
        let mut out = vec![0.0; self.n_inputs];
        for t in 0..n {
            for (f, d) in frame.iter_mut().zip(drives) {
                *f = d[t];
            }
            run.step(&frame, &mut out);
            for (o, &v) in outputs.iter_mut().zip(&out) {
                o.push(v);
            }
        }
        Ok(Simulated {
            outputs,
            peak_envelope: run.modes.iter().map(|m| m.peak).collect(),
        })
    }

    /// The drives of least energy that make output `output` of the small-signal plant
    /// play `target` (one period of a periodic record whose length is a power of two),
    /// line by line: `d(f) = h(f)^H r(f) / |h(f)|^2`, `h` that output's row of the
    /// small-signal FRF. The other outputs go where they go. A line where the row
    /// vanishes gets no drive.
    pub fn least_drive_for(
        &self,
        output: usize,
        target: &[f64],
        fs_hz: f64,
    ) -> Result<Vec<Vec<f64>>, String> {
        self.validate(fs_hz)?;
        let n = target.len();
        if n < 2 || !n.is_power_of_two() {
            return Err(format!("a target of {n} samples is not a power of two"));
        }
        if output >= self.n_inputs {
            return Err(format!("output {output} of a plant with {}", self.n_inputs));
        }
        let lines = n / 2 + 1;
        let df = fs_hz / n as f64;
        let freqs: Vec<f64> = (0..lines).map(|k| k as f64 * df).collect();
        let h = self
            .linear()
            .frf_at_envelope(&freqs, fs_hz, &vec![0.0; self.modes.len()]);
        let conj = fft_exponent_sign() > 0.0;
        let spec = fft::fft(&target.iter().map(|&v| (v, 0.0)).collect::<Vec<Cplx>>());
        let mut d: Vec<Vec<Cplx>> = vec![vec![(0.0, 0.0); n]; self.n_drives];
        for k in 0..lines {
            let r = C64::new(spec[k].0, if conj { -spec[k].1 } else { spec[k].1 });
            let row: Vec<C64> = (0..self.n_drives).map(|j| h.get(k, output, j)).collect();
            let norm: f64 = row.iter().map(|v| v.norm_sqr()).sum();
            if norm <= 0.0 {
                continue;
            }
            for (j, hj) in row.iter().enumerate() {
                let mut v = hj.conj() * r / norm;
                if k == 0 || 2 * k == n {
                    v.im = 0.0;
                }
                d[j][k] = (v.re, v.im);
                if k > 0 && 2 * k < n {
                    d[j][n - k] = (v.re, -v.im);
                }
            }
        }
        Ok(d.iter()
            .map(|s| inverse_dft(s).into_iter().map(|c| c.0).collect())
            .collect())
    }

    /// A mode's angular frequency and damping at envelope `a`.
    fn params(&self, m: &JointedMode, a: f64) -> (f64, f64) {
        let w0 = TAU * m.f_hz;
        match &self.joint {
            None => (w0, m.zeta),
            Some(j) => {
                let r = a / m.reference_amplitude;
                (w0 * j.frequency_factor(r), j.damping(m.zeta, r))
            }
        }
    }
}

/// A jointed plant being stepped by its caller ([`JointedPlant::stepper`]).
#[derive(Debug, Clone)]
pub struct Stepper {
    run: JointedRun,
}

impl Stepper {
    /// Advance one frame: `drive` holds one value per drive, `out` receives one per
    /// output.
    pub fn step(&mut self, drive: &[f64], out: &mut [f64]) {
        self.run.step(drive, out);
    }

    /// Each mode's present envelope.
    pub fn envelope(&self) -> Vec<f64> {
        self.run.modes.iter().map(|m| m.envelope).collect()
    }

    /// Back to rest.
    pub fn reset(&mut self) {
        self.run.reset();
    }
}

// ---------------------------------------------------------------------------------------
// The step.
// ---------------------------------------------------------------------------------------

/// One mode's step over one sample period at a held frequency and damping: the exact
/// solution under a modal force varying linearly across the period.
#[derive(Debug, Clone, Copy)]
struct ModeStep {
    w: f64,
    zeta: f64,
    dt: f64,
    /// The state transition `exp(A dt)`, row-major.
    p: [f64; 4],
}

impl ModeStep {
    fn new(w: f64, zeta: f64, dt: f64) -> Self {
        let sigma = zeta * w;
        let wd = w * (1.0 - zeta * zeta).sqrt();
        let e = (-sigma * dt).exp();
        let (s, c) = (wd * dt).sin_cos();
        ModeStep {
            w,
            zeta,
            dt,
            p: [
                e * (c + sigma / wd * s),
                e * s / wd,
                -e * w * w / wd * s,
                e * (c - sigma / wd * s),
            ],
        }
    }

    /// Advance `(q, v)` from the previous sample to this one under the force going
    /// linearly from `u0` to `u1`; return the acceleration at this sample.
    #[inline]
    fn advance(&self, q: &mut f64, v: &mut f64, u0: f64, u1: f64) -> f64 {
        let w2 = self.w * self.w;
        let k = (u1 - u0) / self.dt;
        // The particular solution under the ramp, at the start of the period.
        let pq = (u0 - 2.0 * self.zeta * k / self.w) / w2;
        let pv = k / w2;
        let (dq, dv) = (*q - pq, *v - pv);
        *q = pq + k * self.dt / w2 + self.p[0] * dq + self.p[1] * dv;
        *v = pv + self.p[2] * dq + self.p[3] * dv;
        u1 - 2.0 * self.zeta * self.w * *v - w2 * *q
    }

    /// The step's acceleration response at `z = exp(j w dt)`, held parameters.
    fn response(&self, z: C64) -> C64 {
        let w2 = self.w * self.w;
        let c = 2.0 * self.zeta / (self.w * self.dt);
        let tw = 1.0 / (self.dt * w2);
        // x[n] = P x[n-1] + G0 u[n-1] + G1 u[n], from the ramp's particular solution.
        let (a0, a1) = ([c / w2, -tw], [(1.0 - c) / w2, tw]);
        let (b0, b1) = ([(1.0 + c) / w2, -tw], [-c / w2, tw]);
        let p = &self.p;
        let mul = |b: [f64; 2]| [p[0] * b[0] + p[1] * b[1], p[2] * b[0] + p[3] * b[1]];
        let (pb0, pb1) = (mul(b0), mul(b1));
        let g0 = [a0[0] - pb0[0], a0[1] - pb0[1]];
        let g1 = [a1[0] - pb1[0], a1[1] - pb1[1]];
        let g = [z * g1[0] + g0[0], z * g1[1] + g0[1]];
        let m11 = z - p[0];
        let m22 = z - p[3];
        let det = m11 * m22 - p[1] * p[2];
        let xq = (m22 * g[0] + g[1] * p[1]) / det;
        let xv = (g[0] * p[2] + m11 * g[1]) / det;
        C64::new(1.0, 0.0) - xq * w2 - xv * (2.0 * self.zeta * self.w)
    }
}

/// A delay of whole samples plus a fraction: `n0` whole samples, then a four-tap
/// Lagrange interpolator at fraction `d` (in `[1, 2)` for a delay of a sample or more,
/// where the interpolator is most accurate, else the delay itself).
#[derive(Debug, Clone)]
struct FracDelay {
    buf: Vec<f64>,
    pos: usize,
    n0: usize,
    h: [f64; 4],
}

impl FracDelay {
    fn new(samples: f64) -> Self {
        let (n0, d) = if samples < 1.0 {
            (0usize, samples)
        } else {
            let n0 = samples.floor() as usize - 1;
            (n0, samples - n0 as f64)
        };
        let mut h = [0.0; 4];
        for (k, hk) in h.iter_mut().enumerate() {
            let mut v = 1.0;
            for m in 0..4 {
                if m != k {
                    v *= (d - m as f64) / (k as f64 - m as f64);
                }
            }
            *hk = v;
        }
        FracDelay {
            buf: vec![0.0; n0 + 4],
            pos: 0,
            n0,
            h,
        }
    }

    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let len = self.buf.len();
        self.buf[self.pos] = x;
        let mut y = 0.0;
        for (k, &hk) in self.h.iter().enumerate() {
            y += hk * self.buf[(self.pos + len - self.n0 - k) % len];
        }
        self.pos = (self.pos + 1) % len;
        y
    }

    fn response(&self, om: f64) -> C64 {
        let mut acc = C64::new(0.0, 0.0);
        for (k, &hk) in self.h.iter().enumerate() {
            let ph = -om * (self.n0 + k) as f64;
            acc += C64::new(hk * ph.cos(), hk * ph.sin());
        }
        acc
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }
}

#[derive(Debug, Clone)]
struct ModeState {
    w0: f64,
    zeta0: f64,
    a_ref: f64,
    q: f64,
    v: f64,
    u_prev: f64,
    envelope: f64,
    /// The envelope's decay per sample.
    decay: f64,
    /// The envelope the current step was made for.
    stepped_at: f64,
    step: ModeStep,
    peak: f64,
}

/// A running jointed plant (crate-private; built by the backend at `configure`).
#[derive(Debug, Clone)]
pub(crate) struct JointedRun {
    pub(crate) n_out: usize,
    n_drive: usize,
    /// `n_modes x n_drive`, row-major.
    phi: Vec<f64>,
    /// `n_modes x n_out`, row-major.
    psi: Vec<f64>,
    modes: Vec<ModeState>,
    joint: Option<JointLaw>,
    residual: Vec<(usize, usize, f64)>,
    feedthrough: f64,
    delays: Vec<DelayLine>,
    delayed: Vec<f64>,
    clean: Vec<f64>,
    odelays: Vec<FracDelay>,
    rng: Xorshift64,
    noise_level: f64,
    dt: f64,
}

impl JointedRun {
    pub(crate) fn new(p: &JointedPlant, fs_hz: f64, noise_level: f64, seed: u64) -> Self {
        let dt = 1.0 / fs_hz;
        let mut phi = Vec::with_capacity(p.modes.len() * p.n_drives);
        let mut psi = Vec::with_capacity(p.modes.len() * p.n_inputs);
        for m in &p.modes {
            phi.extend_from_slice(&m.input_shape);
            psi.extend_from_slice(&m.output_shape);
        }
        let decay_cycles = p.joint.map_or(3.0, |j| j.decay_cycles);
        let modes = p
            .modes
            .iter()
            .map(|m| {
                let w0 = TAU * m.f_hz;
                ModeState {
                    w0,
                    zeta0: m.zeta,
                    a_ref: m.reference_amplitude,
                    q: 0.0,
                    v: 0.0,
                    u_prev: 0.0,
                    envelope: 0.0,
                    decay: (-m.f_hz * dt / decay_cycles).exp(),
                    stepped_at: 0.0,
                    step: ModeStep::new(w0, m.zeta, dt),
                    peak: 0.0,
                }
            })
            .collect();
        let residual = if p.residual.is_empty() {
            Vec::new()
        } else {
            (0..p.n_inputs)
                .flat_map(|i| (0..p.n_drives).map(move |j| (i, j)))
                .filter_map(|(i, j)| {
                    let v = p.residual[i * p.n_drives + j];
                    (v != 0.0).then_some((i, j, v))
                })
                .collect()
        };
        JointedRun {
            n_out: p.n_inputs,
            n_drive: p.n_drives,
            phi,
            psi,
            modes,
            joint: p.joint,
            residual,
            feedthrough: p.feedthrough,
            delays: (0..p.n_drives)
                .map(|_| DelayLine::new(p.transport_delay_samples))
                .collect(),
            delayed: vec![0.0; p.n_drives],
            clean: vec![0.0; p.n_inputs],
            odelays: (0..p.n_inputs)
                .map(|i| FracDelay::new(p.output_delay_samples(i, fs_hz)))
                .collect(),
            rng: Xorshift64::new(seed),
            noise_level,
            dt,
        }
    }

    #[inline]
    pub(crate) fn step(&mut self, drive: &[f64], out: &mut [f64]) {
        let mut sum = 0.0;
        for ((d, line), &x) in self.delayed.iter_mut().zip(&mut self.delays).zip(drive) {
            *d = line.process(x);
            sum += *d;
        }
        self.clean.fill(self.feedthrough * sum);
        for &(i, j, v) in &self.residual {
            self.clean[i] += v * self.delayed[j];
        }
        let (nd, no) = (self.n_drive, self.n_out);
        for (m, st) in self.modes.iter_mut().enumerate() {
            if let Some(j) = &self.joint {
                if st.envelope != st.stepped_at {
                    let r = st.envelope / st.a_ref;
                    st.step = ModeStep::new(
                        st.w0 * j.frequency_factor(r),
                        j.damping(st.zeta0, r),
                        self.dt,
                    );
                    st.stepped_at = st.envelope;
                }
            }
            let u: f64 = self.phi[m * nd..(m + 1) * nd]
                .iter()
                .zip(&self.delayed)
                .map(|(a, b)| a * b)
                .sum();
            let a = st.step.advance(&mut st.q, &mut st.v, st.u_prev, u);
            st.u_prev = u;
            let amplitude = st.q.hypot(st.v / st.step.w);
            st.envelope = amplitude.max(st.envelope * st.decay);
            st.peak = st.peak.max(st.envelope);
            for (c, &s) in self.clean.iter_mut().zip(&self.psi[m * no..(m + 1) * no]) {
                *c += s * a;
            }
        }
        let out = &mut out[..self.n_out];
        for ((o, line), &c) in out.iter_mut().zip(&mut self.odelays).zip(&self.clean) {
            *o = line.process(c);
        }
        crate::plant::add_noise(&mut self.rng, self.noise_level, out);
    }

    pub(crate) fn reset(&mut self) {
        let dt = self.dt;
        for st in &mut self.modes {
            st.q = 0.0;
            st.v = 0.0;
            st.u_prev = 0.0;
            st.envelope = 0.0;
            st.stepped_at = 0.0;
            st.peak = 0.0;
            st.step = ModeStep::new(st.w0, st.zeta0, dt);
        }
        self.delays.iter_mut().for_each(DelayLine::reset);
        self.delayed.fill(0.0);
        self.odelays.iter_mut().for_each(FracDelay::reset);
    }
}
