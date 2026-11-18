//! `secant`: the published secant law with a complex gain, a two-step cycle
//! (`<x, v> = v^H x`):
//!
//! ```text
//! learn:   probe D_k + c Z_k E_k,                    measure Chat_k
//! gain:    beta = (Chat_k - C_k)^H E_k / ||Chat_k - C_k||^2,  a = c beta
//! control: D_{k+1} = D_k + beta P_k (P_k the probe as played), measure C_{k+1}
//! model:   Z <- (I + u S^H) Z,  u = (S - Z y) / (S^H Z y),  y = C_{k+1} - C_k
//! ```
//!
//! Each step holds the drive `settle` updates first (by default five times the
//! tracking filter's delay plus the identification's group delay), so the
//! response is read settled. `c` starts at `c0`, doubles to at most `c_max`
//! when `|a - 1| < a_band` and halves otherwise. When `||E|| <= tol ||R||` the
//! cycle holds instead of probing. How `Z` moves along frequency is not in the
//! source: `Factor` (default) keeps `Z(f) = M Z0(f)` with the updates acting on
//! the drive-side factor `M`; `Plain` carries `Z` unchanged from engage. The
//! law has no gain: its pace is the settle wait.
//!
//! A level change: the controller scales the drive in force by the level's
//! ratio and tells the law the new level. The readings and drives the cycle
//! holds (the base reading, its drive and error, and the last control step's)
//! are scaled by the same ratio, so the probe, its gain and the secant pair are
//! read against the drive as it is now; the model `Z` is unchanged, since a
//! linear structure's response over drive does not move with level. The
//! present settle wait is restarted, so the next reading is taken settled at
//! the new level.

use crate::law::{aimed_reference, worst_error_db, Inputs, Outputs, SineLaw};
use crate::linalg::{mat_vec, norm, reg_pinv};
use crate::setup::{GroupDelay, Setup};
use crate::{TestTypes, C64};

const ZERO: C64 = C64::new(0.0, 0.0);

/// The diagnostics `secant` declares, in order.
pub const SECANT_VALUES: [&str; 6] = ["a_re", "a_im", "c", "phase", "error_db", "secant_skipped"];

/// The test types `secant` accepts: a dwell only (either carry). On the
/// research testbed's linear 4 by 4 it completed 4 of 5 dwells at the lightest
/// mode, 0 of 5 stepped sines and 0 of 20 sweeps at one octave a minute: each
/// cycle holds the drive for several settle waits while the tone moves on.
pub const SECANT_TEST_TYPES: TestTypes = TestTypes {
    sweep: false,
    dwell: true,
    stepped: false,
};

/// How the inverse model moves from one frequency to the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecantCarry {
    /// `Z(f) = M Z0(f)`, the updates acting on `M`.
    Factor,
    /// `Z` carried unchanged from engage.
    Plain,
}

/// Parameters of `secant`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SecantParams {
    /// The first probe weight.
    pub c0: f64,
    /// The largest probe weight.
    pub c_max: f64,
    /// How the model moves along frequency.
    pub carry: SecantCarry,
    /// Updates held before each reading; `None` sets it from the delays.
    pub settle: Option<usize>,
    /// Regularization of the inverse.
    pub eps: f64,
    /// `|a - 1|` under which `c` doubles.
    pub a_band: f64,
    /// Relative error under which the cycle holds.
    pub tol: f64,
    /// Multiple of the delays the settle wait lasts.
    pub settle_factor: f64,
}

impl Default for SecantParams {
    fn default() -> Self {
        SecantParams {
            c0: 0.1,
            c_max: 1.0,
            carry: SecantCarry::Factor,
            settle: None,
            eps: 1e-3,
            a_band: 0.3,
            tol: 0.0116,
            settle_factor: 5.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    AwaitBase,
    AwaitProbe,
}

/// The `secant` law.
#[derive(Clone, Debug)]
pub struct SecantLaw {
    params: SecantParams,
    setup: Setup,
    delay: GroupDelay,
    c: f64,
    a: C64,
    skipped: usize,
    /// `drives x drives`.
    factor: Vec<C64>,
    /// `drives x controls`.
    plain: Vec<C64>,
    phase: Phase,
    wait: usize,
    /// (C before the control step, D before it, Z used).
    prev: Option<(Vec<C64>, Vec<C64>, Vec<C64>)>,
    /// (C, D, E) at the base reading.
    base: (Vec<C64>, Vec<C64>, Vec<C64>),
    zk: Vec<C64>,
    /// The level the held readings and drives were taken at.
    level: f64,
}

impl SecantLaw {
    /// Build the law.
    pub fn new(setup: &Setup, params: SecantParams) -> Result<SecantLaw, String> {
        setup.check()?;
        let (n, m) = (setup.controls, setup.drives);
        let mut factor = vec![ZERO; m * m];
        for i in 0..m {
            factor[i * m + i] = C64::new(1.0, 0.0);
        }
        Ok(SecantLaw {
            delay: GroupDelay::of(&setup.identification),
            setup: setup.clone(),
            c: params.c0,
            params,
            a: C64::new(1.0, 0.0),
            skipped: 0,
            factor,
            plain: vec![ZERO; m * n],
            phase: Phase::AwaitBase,
            wait: 0,
            prev: None,
            base: (Vec::new(), Vec::new(), Vec::new()),
            zk: Vec::new(),
            level: 1.0,
        })
    }

    fn z0(&self, f: f64) -> Result<Vec<C64>, String> {
        let (n, m) = (self.setup.controls, self.setup.drives);
        let mut h = vec![ZERO; n * m];
        self.setup.response_at(f, &mut h);
        Ok(reg_pinv(&h, n, m, self.params.eps)?.0)
    }

    fn z(&self, f: f64) -> Result<Vec<C64>, String> {
        let (n, m) = (self.setup.controls, self.setup.drives);
        match self.params.carry {
            SecantCarry::Plain => Ok(self.plain.clone()),
            SecantCarry::Factor => {
                let z0 = self.z0(f)?;
                let mut out = vec![ZERO; m * n];
                for i in 0..m {
                    for j in 0..n {
                        out[i * n + j] =
                            (0..m).fold(ZERO, |s, k| s + self.factor[i * m + k] * z0[k * n + j]);
                    }
                }
                Ok(out)
            }
        }
    }

    fn settle_n(&self, f: f64) -> usize {
        if let Some(s) = self.params.settle {
            return s;
        }
        let ld = self.setup.loop_delay;
        let tf = self.setup.tracking.delay_s_with(f, ld.tracker_delay_factor);
        let ts = self.delay.at(f, 0.0) + ld.transport_delay_s;
        ((self.params.settle_factor * (tf + ts) / self.setup.tick_s).ceil() as usize).max(1)
    }

    fn emit(&self, d: &[C64], x: &Inputs, out: &mut Outputs) {
        out.drive.copy_from_slice(d);
        let err = if x.controls.iter().any(|c| c.norm() != 0.0) {
            worst_error_db(x.controls, x.reference)
        } else {
            0.0
        };
        let phase = if self.phase == Phase::AwaitBase {
            0.0
        } else {
            1.0
        };
        out.values.clear();
        out.values.extend_from_slice(&[
            self.a.re,
            self.a.im,
            self.c,
            phase,
            err,
            self.skipped as f64,
        ]);
    }

    /// Carry the held readings and drives to the level `x.level` (see the
    /// module's documentation).
    fn follow_level(&mut self, x: &Inputs) {
        if !(x.level > 0.0 && self.level > 0.0) || x.level == self.level {
            return;
        }
        let r = x.level / self.level;
        self.level = x.level;
        let scale = |v: &mut Vec<C64>| v.iter_mut().for_each(|a| *a *= r);
        scale(&mut self.base.0);
        scale(&mut self.base.1);
        scale(&mut self.base.2);
        if let Some((c0, d0, _)) = self.prev.as_mut() {
            scale(c0);
            scale(d0);
        }
        self.wait = self.wait.max(self.settle_n(x.f_hz));
    }

    /// The minimum-change update of the inverse model.
    fn secant(&mut self, s: &[C64], y: &[C64], zk: &[C64]) {
        let (n, m) = (self.setup.controls, self.setup.drives);
        let mut zy = vec![ZERO; m];
        mat_vec(zk, m, n, y, &mut zy);
        let den = s
            .iter()
            .zip(&zy)
            .fold(ZERO, |a, (si, zi)| a + si.conj() * zi);
        let ns = norm(s);
        if !den.is_finite() || den.norm() <= 1e-9 * ns * norm(&zy) || ns == 0.0 {
            self.skipped += 1;
            return;
        }
        let u: Vec<C64> = s.iter().zip(&zy).map(|(a, b)| (a - b) / den).collect();
        let target = match self.params.carry {
            SecantCarry::Plain => &mut self.plain,
            SecantCarry::Factor => &mut self.factor,
        };
        let cols = target.len() / m;
        let old = target.clone();
        // (I + u S^H) T = T + u (S^H T).
        for j in 0..cols {
            let sht = (0..m).fold(ZERO, |a, k| a + s[k].conj() * old[k * cols + j]);
            for i in 0..m {
                target[i * cols + j] = old[i * cols + j] + u[i] * sht;
            }
        }
    }
}

impl SineLaw for SecantLaw {
    fn engage(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        let (n, m) = (self.setup.controls, self.setup.drives);
        let z0 = self.z0(x.f_hz)?;
        self.plain = z0.clone();
        self.prev = None;
        self.level = x.level;
        self.phase = Phase::AwaitBase;
        self.wait = self.settle_n(x.f_hz);
        let mut d = vec![ZERO; m];
        mat_vec(&z0, m, n, &aimed_reference(x), &mut d);
        self.emit(&d, x, out);
        Ok(())
    }

    fn tick(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        let (n, m) = (self.setup.controls, self.setup.drives);
        self.follow_level(x);
        if self.wait > 0 {
            self.wait -= 1;
            self.emit(x.drive_in_force, x, out);
            return Ok(());
        }
        if self.phase == Phase::AwaitBase {
            let ck = x.controls.to_vec();
            let dk = x.drive_in_force.to_vec();
            if let Some((c0, d0, zs)) = self.prev.take() {
                let s: Vec<C64> = dk.iter().zip(&d0).map(|(a, b)| a - b).collect();
                let y: Vec<C64> = ck.iter().zip(&c0).map(|(a, b)| a - b).collect();
                self.secant(&s, &y, &zs);
            }
            let aimed = aimed_reference(x);
            let e: Vec<C64> = aimed.iter().zip(&ck).map(|(r, c)| r - c).collect();
            let small = norm(&e) <= self.params.tol * norm(&aimed);
            self.base = (ck, dk.clone(), e.clone());
            if small {
                self.prev = None;
                self.wait = self.settle_n(x.f_hz);
                self.emit(&dk, x, out);
                return Ok(());
            }
            self.zk = self.z(x.f_hz)?;
            self.phase = Phase::AwaitProbe;
            self.wait = self.settle_n(x.f_hz);
            let mut p = vec![ZERO; m];
            mat_vec(&self.zk, m, n, &e, &mut p);
            let d: Vec<C64> = dk.iter().zip(&p).map(|(a, b)| a + b * self.c).collect();
            self.emit(&d, x, out);
            return Ok(());
        }
        let (ck, dk, ek) = self.base.clone();
        let probe: Vec<C64> = x
            .drive_in_force
            .iter()
            .zip(&dk)
            .map(|(a, b)| a - b)
            .collect();
        let dc: Vec<C64> = x.controls.iter().zip(&ck).map(|(a, b)| a - b).collect();
        let den: f64 = dc.iter().map(|v| v.norm_sqr()).sum();
        let beta = if den > 0.0 {
            dc.iter().zip(&ek).fold(ZERO, |a, (d, e)| a + d.conj() * e) / den
        } else {
            ZERO
        };
        self.a = beta * self.c;
        let ok = (self.a - 1.0).norm() < self.params.a_band;
        self.c = if ok {
            (2.0 * self.c).min(self.params.c_max)
        } else {
            (0.5 * self.c).max(self.params.c0)
        };
        self.prev = Some((ck, dk.clone(), self.zk.clone()));
        self.phase = Phase::AwaitBase;
        self.wait = self.settle_n(x.f_hz);
        let d: Vec<C64> = dk.iter().zip(&probe).map(|(a, b)| a + b * beta).collect();
        self.emit(&d, x, out);
        Ok(())
    }

    fn value_names(&self) -> &'static [&'static str] {
        &SECANT_VALUES
    }
}
