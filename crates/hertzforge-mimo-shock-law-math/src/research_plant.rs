//! Research plants and tools for the law-research study (feature
//! `reference-plant`; no law uses this module):
//!
//! - [`LevelPlant`]: the reference modal plant generalized: a damping and a
//!   drop scale per mode, a frequency drop linear or quadratic in level, mode
//!   shapes that change with level, and complex residues. With none of these it
//!   is the [`ModalPlant`] exactly.
//! - [`IntraPulsePlant`]: each mode's frequency and damping follow that mode's
//!   own running amplitude inside the pulse (a softening joint whose damping
//!   rises), integrated in time.
//! - [`identify_h1`] and [`h1_from_records`]: a realistic identification, the
//!   H1 estimate from a finite random record with measurement noise, framed at
//!   the pulse's own line spacing with a Hann window, so it carries noise,
//!   leakage and window bias.
//! - [`run_research_ladder`]: the ladder runner with signed errors, the law's
//!   diagnostics per pulse, a drive ceiling and any plant.

use crate::modal::{ModalModel, ModalTerm};
use crate::reference_plant::{reference_shock, srs_grid_hz, ModalPlant, Rng, FS_HZ, RECORD_LEN};
use crate::spectrum::{irfft, omegas, rfft, rms_all, rows_irfft};
use crate::srs::{error_db, srs_rows};
use crate::{level_scale, LawSteps, MatrixStack, PlantModel, Setup, SrsTarget, C64};
use std::f64::consts::TAU;

/// One mode of a [`LevelPlant`].
#[derive(Clone, Debug, PartialEq)]
pub struct LevelMode {
    /// Natural frequency at zero level, Hz.
    pub fn0_hz: f64,
    /// Damping ratio at zero level.
    pub zeta0: f64,
    /// This mode's drop is `delta * drop_scale`.
    pub drop_scale: f64,
    /// Shape at each control at zero level.
    pub phi: Vec<f64>,
    /// Change of the control shape at full level (`phi + s dphi`); empty for
    /// none.
    pub dphi: Vec<f64>,
    /// Shape at each drive.
    pub psi: Vec<f64>,
    /// Angle of the residue, radians (zero: a real residue).
    pub rotation: f64,
}

/// How a mode's frequency drop grows with the amplitude scale `s`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropLaw {
    /// `delta s`.
    Linear,
    /// `delta s^2`.
    Quadratic,
}

/// The generalized quasi-linear modal plant (module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct LevelPlant {
    /// Number of controls.
    pub n_c: usize,
    /// Number of drives.
    pub n_d: usize,
    /// Quasi-static coupling, `n_c x n_d` row-major.
    pub quasi_static: Vec<f64>,
    /// The modes.
    pub modes: Vec<LevelMode>,
    /// Fractional frequency drop at full level.
    pub delta: f64,
    /// Damping growth at full level.
    pub kappa: f64,
    /// Delay at each control, seconds.
    pub delays_s: Vec<f64>,
    /// How the drop grows with level.
    pub drop_law: DropLaw,
}

impl LevelPlant {
    /// The [`ModalPlant`] as a level plant (the same numbers).
    pub fn from_modal(p: &ModalPlant) -> Self {
        LevelPlant {
            n_c: p.n_c,
            n_d: p.n_d,
            quasi_static: p.quasi_static.clone(),
            modes: p
                .modes
                .iter()
                .map(|m| LevelMode {
                    fn0_hz: m.fn0_hz,
                    zeta0: p.zeta0,
                    drop_scale: 1.0,
                    phi: m.phi.clone(),
                    dphi: Vec::new(),
                    psi: m.psi.clone(),
                    rotation: 0.0,
                })
                .collect(),
            delta: p.delta,
            kappa: p.kappa,
            delays_s: p.delays_s.clone(),
            drop_law: DropLaw::Linear,
        }
    }

    /// The 4 x 4 reference plant with every mode's damping set to `zeta0`.
    pub fn four_by_four_damped(delta: f64, kappa: f64, zeta0: f64) -> Self {
        let mut p = Self::from_modal(&ModalPlant::four_by_four(delta, kappa));
        for m in p.modes.iter_mut() {
            m.zeta0 = zeta0;
        }
        p
    }

    /// The 4 x 4 reference plant with a fifth mode at 1060 Hz, close to the
    /// 1100 Hz mode, that drops an eighth as much.
    pub fn four_by_four_close(delta: f64, kappa: f64) -> Self {
        let mut p = Self::from_modal(&ModalPlant::four_by_four(delta, kappa));
        p.modes.push(LevelMode {
            fn0_hz: 1060.0,
            zeta0: 0.02,
            drop_scale: 0.125,
            phi: vec![0.4, 0.5, -0.3, 0.6],
            dphi: Vec::new(),
            psi: vec![0.3, -0.5, 0.6, 0.2],
            rotation: 0.0,
        });
        p
    }

    /// The 4 x 4 reference plant with a fifth mode at 2300 Hz, at the edge of
    /// the 80 to 2400 Hz band.
    pub fn four_by_four_edge(delta: f64, kappa: f64) -> Self {
        let mut p = Self::from_modal(&ModalPlant::four_by_four(delta, kappa));
        p.modes.push(LevelMode {
            fn0_hz: 2300.0,
            zeta0: 0.02,
            drop_scale: 1.0,
            phi: vec![0.3, -0.4, 0.5, 0.6],
            dphi: Vec::new(),
            psi: vec![0.5, 0.4, -0.3, 0.3],
            rotation: 0.0,
        });
        p
    }

    /// The 4 x 4 reference plant whose control shapes change with level: each
    /// entry moves by `rel` times a standard normal value (seeded) at full
    /// level, in proportion to the amplitude scale.
    pub fn four_by_four_shapes_move(delta: f64, kappa: f64, rel: f64, seed: u64) -> Self {
        let mut p = Self::from_modal(&ModalPlant::four_by_four(delta, kappa));
        let mut rng = Rng::new(seed);
        for m in p.modes.iter_mut() {
            m.dphi = m
                .phi
                .iter()
                .map(|v| rel * v.abs().max(0.2) * rng.normal())
                .collect();
        }
        p
    }

    /// A 16-control, 8-drive plant with 8 modes from 300 to 2200 Hz, shapes
    /// and coupling from a seeded generator, delays 0 to 0.4 ms.
    pub fn sixteen_by_eight(delta: f64, kappa: f64, seed: u64) -> Self {
        let (n_c, n_d) = (16, 8);
        let mut rng = Rng::new(seed);
        let mut qs = vec![0.0; n_c * n_d];
        for r in 0..n_c {
            for c in 0..n_d {
                qs[r * n_d + c] = if r % n_d == c {
                    1.0
                } else {
                    0.15 * rng.normal()
                };
            }
        }
        let modes = (0..8)
            .map(|i| LevelMode {
                fn0_hz: 300.0 * (2200.0f64 / 300.0).powf(i as f64 / 7.0),
                zeta0: 0.02,
                drop_scale: 1.0,
                phi: (0..n_c).map(|_| 0.6 * rng.normal()).collect(),
                dphi: Vec::new(),
                psi: (0..n_d).map(|_| 0.6 * rng.normal()).collect(),
                rotation: 0.0,
            })
            .collect();
        LevelPlant {
            n_c,
            n_d,
            quasi_static: qs,
            modes,
            delta,
            kappa,
            delays_s: (0..n_c)
                .map(|r| 0.4e-3 * r as f64 / (n_c - 1) as f64)
                .collect(),
            drop_law: DropLaw::Linear,
        }
    }

    /// The same plant with every shape entry scaled by `1 + rel n`, `n`
    /// standard normal (seeded): a modal model with wrong residues.
    pub fn with_shape_errors(&self, rel: f64, seed: u64) -> Self {
        let mut p = self.clone();
        let mut rng = Rng::new(seed);
        for m in p.modes.iter_mut() {
            for v in m.phi.iter_mut() {
                *v *= 1.0 + rel * rng.normal();
            }
            for v in m.psi.iter_mut() {
                *v *= 1.0 + rel * rng.normal();
            }
        }
        p
    }

    fn mode_at(&self, m: &LevelMode, s: f64) -> (f64, f64, Vec<C64>) {
        let g = match self.drop_law {
            DropLaw::Linear => s,
            DropLaw::Quadratic => s * s,
        };
        let fn_hz = m.fn0_hz * (1.0 - self.delta * m.drop_scale * g);
        let z = m.zeta0 * (1.0 + self.kappa * s);
        let rot = C64::from_polar(1.0, m.rotation);
        let mut res = vec![C64::new(0.0, 0.0); self.n_c * self.n_d];
        for r in 0..self.n_c {
            let ph = if m.dphi.is_empty() {
                m.phi[r]
            } else {
                m.phi[r] + s * m.dphi[r]
            };
            for c in 0..self.n_d {
                let v = ph * m.psi[c];
                res[r * self.n_d + c] = if m.rotation == 0.0 {
                    C64::new(v, 0.0)
                } else {
                    rot * v
                };
            }
        }
        (fn_hz, z, res)
    }

    /// The plant's own modal model at amplitude scale `s` (shapes, coupling and
    /// delays exact; `b2` zero).
    pub fn modal_truth(&self, s: f64) -> ModalModel {
        ModalModel {
            n_c: self.n_c,
            n_d: self.n_d,
            delays_s: self.delays_s.clone(),
            b0: self
                .quasi_static
                .iter()
                .map(|&v| C64::new(v, 0.0))
                .collect(),
            b2: vec![C64::new(0.0, 0.0); self.n_c * self.n_d],
            f_ref_hz: 2400.0,
            modes: self
                .modes
                .iter()
                .map(|m| {
                    let (fn_hz, zeta, residue) = self.mode_at(m, s);
                    ModalTerm {
                        fn_hz,
                        zeta,
                        residue,
                    }
                })
                .collect(),
        }
    }

    /// The plant at amplitude scale `s` on a record's lines (real at the
    /// Nyquist line).
    pub fn frf(&self, s: f64, fs_hz: f64, record_len: usize) -> MatrixStack {
        self.modal_truth(s).frf(fs_hz, record_len)
    }

    /// Responses to drive records played at amplitude scale `s` (circular, as
    /// [`ModalPlant::respond`]).
    pub fn respond(
        &self,
        drives: &[Vec<f64>],
        s: f64,
        fs_hz: f64,
    ) -> Result<Vec<Vec<f64>>, String> {
        let n = drives.first().map(|d| d.len()).unwrap_or(0);
        let h = self.frf(s, fs_hz, n);
        let d: Vec<Vec<C64>> = drives.iter().map(|x| rfft(x)).collect::<Result<_, _>>()?;
        rows_irfft(&crate::spectrum::apply(&h, &d), n, 1.0)
    }
}

/// The plant with the intra-pulse nonlinearity: mode `m` obeys
/// `q'' + 2 z(a) wn(a) q' + wn(a)^2 q = psi_m . d(t)` with
/// `a = sqrt(q^2 + (q' / wn0)^2) / a_ref_m`, `wn(a) = wn0 (1 - delta tanh a)`,
/// `z(a) = z0 (1 + kappa tanh a)`, and the output `y = B d - sum_m phi_m q_m''`.
/// Fourth-order Runge-Kutta at the sample rate with the drive linear inside a
/// step. `a_ref_m` is the mode's peak amplitude on the linear plant at full
/// level under the exact inverse of the reference (set by
/// [`IntraPulsePlant::calibrate`]), so at full level each mode's frequency
/// falls by about three quarters of `delta` at its peak and recovers as it
/// decays.
#[derive(Clone, Debug, PartialEq)]
pub struct IntraPulsePlant {
    /// Quasi-static coupling, `n_c x n_d`.
    pub quasi_static: Vec<f64>,
    /// Shapes at the controls, one row per mode.
    pub phi: Vec<Vec<f64>>,
    /// Shapes at the drives, one row per mode.
    pub psi: Vec<Vec<f64>>,
    /// Natural angular frequencies at zero amplitude, rad/s.
    pub wn0: Vec<f64>,
    /// Frequency drop at large amplitude.
    pub delta: f64,
    /// Damping growth at large amplitude.
    pub kappa: f64,
    /// Damping at zero amplitude.
    pub z0: f64,
    /// Reference amplitude per mode.
    pub aref: Vec<f64>,
    /// Number of controls and drives.
    pub n_c: usize,
    /// Number of drives.
    pub n_d: usize,
}

impl IntraPulsePlant {
    /// The 4 x 4 reference plant's coupling and shapes, without delays, with
    /// the amplitude law; calibrated.
    pub fn four_by_four(delta: f64, kappa: f64) -> Result<Self, String> {
        let p = ModalPlant::four_by_four(0.0, 0.0);
        let mut out = IntraPulsePlant {
            quasi_static: p.quasi_static.clone(),
            phi: p.modes.iter().map(|m| m.phi.clone()).collect(),
            psi: p.modes.iter().map(|m| m.psi.clone()).collect(),
            wn0: p.modes.iter().map(|m| TAU * m.fn0_hz).collect(),
            delta,
            kappa,
            z0: p.zeta0,
            aref: vec![1.0; p.modes.len()],
            n_c: p.n_c,
            n_d: p.n_d,
        };
        out.calibrate()?;
        Ok(out)
    }

    /// The linear plant (amplitude law off) on a record's lines.
    pub fn linear_frf(&self, fs_hz: f64, record_len: usize) -> MatrixStack {
        LevelPlant {
            n_c: self.n_c,
            n_d: self.n_d,
            quasi_static: self.quasi_static.clone(),
            modes: (0..self.wn0.len())
                .map(|m| LevelMode {
                    fn0_hz: self.wn0[m] / TAU,
                    zeta0: self.z0,
                    drop_scale: 1.0,
                    phi: self.phi[m].clone(),
                    dphi: Vec::new(),
                    psi: self.psi[m].clone(),
                    rotation: 0.0,
                })
                .collect(),
            delta: 0.0,
            kappa: 0.0,
            delays_s: vec![0.0; self.n_c],
            drop_law: DropLaw::Linear,
        }
        .frf(0.0, fs_hz, record_len)
    }

    fn calibrate(&mut self) -> Result<(), String> {
        let h = self.linear_frf(FS_HZ, RECORD_LEN);
        let g = crate::model::regularized_inverse(&h, &vec![true; h.lines()], 1e-3)?;
        let r = rfft(&reference_shock(FS_HZ, RECORD_LEN))?;
        let refs = vec![r; self.n_c];
        let d = rows_irfft(&crate::spectrum::apply(&g, &refs), RECORD_LEN, 1.0)?;
        let (_, amp) = self.simulate_inner(&d, FS_HZ, true);
        self.aref = amp;
        Ok(())
    }

    fn simulate_inner(
        &self,
        d: &[Vec<f64>],
        fs_hz: f64,
        linear: bool,
    ) -> (Vec<Vec<f64>>, Vec<f64>) {
        let n = d.first().map(|x| x.len()).unwrap_or(0);
        let nm = self.wn0.len();
        let dt = 1.0 / fs_hz;
        let u: Vec<Vec<f64>> = (0..nm)
            .map(|m| {
                (0..n)
                    .map(|k| (0..self.n_d).map(|j| self.psi[m][j] * d[j][k]).sum())
                    .collect()
            })
            .collect();
        let mut y: Vec<Vec<f64>> = (0..self.n_c)
            .map(|r| {
                (0..n)
                    .map(|k| {
                        (0..self.n_d)
                            .map(|j| self.quasi_static[r * self.n_d + j] * d[j][k])
                            .sum()
                    })
                    .collect()
            })
            .collect();
        let mut peak = vec![0.0f64; nm];
        for m in 0..nm {
            let wn0 = self.wn0[m];
            let f = |q: f64, v: f64, uu: f64| -> (f64, f64) {
                let (wn, z) = if linear {
                    (wn0, self.z0)
                } else {
                    let a = (q * q + (v / wn0) * (v / wn0)).sqrt() / self.aref[m];
                    let t = a.tanh();
                    (
                        wn0 * (1.0 - self.delta * t),
                        self.z0 * (1.0 + self.kappa * t),
                    )
                };
                (v, uu - 2.0 * z * wn * v - wn * wn * q)
            };
            let (mut q, mut v) = (0.0f64, 0.0f64);
            for k in 0..n {
                let u0 = u[m][k];
                let u1 = if k + 1 < n { u[m][k + 1] } else { u0 };
                let um = 0.5 * (u0 + u1);
                let (k1q, k1v) = f(q, v, u0);
                let (k2q, k2v) = f(q + 0.5 * dt * k1q, v + 0.5 * dt * k1v, um);
                let (k3q, k3v) = f(q + 0.5 * dt * k2q, v + 0.5 * dt * k2v, um);
                let (k4q, k4v) = f(q + dt * k3q, v + dt * k3v, u1);
                let acc = k1v;
                peak[m] = peak[m].max((q * q + (v / wn0) * (v / wn0)).sqrt());
                for (yr, ph) in y.iter_mut().zip(&self.phi[m]) {
                    yr[k] -= ph * acc;
                }
                q += dt / 6.0 * (k1q + 2.0 * k2q + 2.0 * k3q + k4q);
                v += dt / 6.0 * (k1v + 2.0 * k2v + 2.0 * k3v + k4v);
            }
        }
        (y, peak)
    }

    /// Responses to drive records (the level is in the drives themselves).
    pub fn respond(&self, drives: &[Vec<f64>], fs_hz: f64) -> Vec<Vec<f64>> {
        self.simulate_inner(drives, fs_hz, false).0
    }
}

/// Independent white Gaussian drives of `len` samples, unit standard
/// deviation times `scale`.
pub fn random_drives(n_d: usize, len: usize, scale: f64, rng: &mut Rng) -> Vec<Vec<f64>> {
    (0..n_d)
        .map(|_| (0..len).map(|_| scale * rng.normal()).collect())
        .collect()
}

/// Adds white noise of `rel` times each row's own rms to every row.
pub fn add_relative_noise(rows: &mut [Vec<f64>], rel: f64, rng: &mut Rng) {
    for row in rows.iter_mut() {
        let rms = (row.iter().map(|v| v * v).sum::<f64>() / row.len().max(1) as f64).sqrt();
        for v in row.iter_mut() {
            *v += rel * rms * rng.normal();
        }
    }
}

/// The H1 estimate `H = G_yx G_xx^-1` from drive and response records, with
/// Hann-windowed frames of `frame` samples at 50% overlap (the record's line
/// spacing is `fs / frame`).
pub fn h1_from_records(
    drives: &[Vec<f64>],
    responses: &[Vec<f64>],
    frame: usize,
) -> Result<MatrixStack, String> {
    let n_d = drives.len();
    let n_c = responses.len();
    let len = drives.first().map(|d| d.len()).unwrap_or(0);
    if len < frame || !frame.is_power_of_two() {
        return Err("the records must be at least one power-of-two frame long".into());
    }
    let lines = frame / 2 + 1;
    let win: Vec<f64> = (0..frame)
        .map(|i| 0.5 - 0.5 * (TAU * i as f64 / frame as f64).cos())
        .collect();
    let zero = C64::new(0.0, 0.0);
    let mut gxx = vec![zero; lines * n_d * n_d];
    let mut gyx = vec![zero; lines * n_c * n_d];
    let hop = frame / 2;
    let mut start = 0;
    let mut buf = vec![0.0; frame];
    while start + frame <= len {
        let spec = |x: &[f64], buf: &mut Vec<f64>| -> Result<Vec<C64>, String> {
            for i in 0..frame {
                buf[i] = x[start + i] * win[i];
            }
            rfft(buf)
        };
        let xs: Vec<Vec<C64>> = drives
            .iter()
            .map(|d| spec(d, &mut buf))
            .collect::<Result<_, _>>()?;
        let ys: Vec<Vec<C64>> = responses
            .iter()
            .map(|d| spec(d, &mut buf))
            .collect::<Result<_, _>>()?;
        for f in 0..lines {
            for a in 0..n_d {
                for b in 0..n_d {
                    gxx[(f * n_d + a) * n_d + b] += xs[a][f] * xs[b][f].conj();
                }
            }
            for r in 0..n_c {
                for b in 0..n_d {
                    gyx[(f * n_c + r) * n_d + b] += ys[r][f] * xs[b][f].conj();
                }
            }
        }
        start += hop;
    }
    let mut h = MatrixStack::zeros(lines, n_c, n_d);
    for f in 0..lines {
        let a = &gxx[f * n_d * n_d..(f + 1) * n_d * n_d];
        let trace: f64 = (0..n_d).map(|i| a[i * n_d + i].re).sum();
        if trace <= 0.0 {
            continue;
        }
        let mut a = a.to_vec();
        for i in 0..n_d {
            a[i * n_d + i] += C64::new(1e-12 * trace, 0.0);
        }
        // H Gxx = Gyx, so Gxx H^H = Gyx^H.
        let mut rhs = vec![zero; n_d * n_c];
        for r in 0..n_c {
            for b in 0..n_d {
                rhs[b * n_c + r] = gyx[(f * n_c + r) * n_d + b].conj();
            }
        }
        let x = crate::lsq::hpd_solve(&a, n_d, &rhs, n_c)?;
        for r in 0..n_c {
            for b in 0..n_d {
                h.set(f, r, b, x[b * n_c + r].conj());
            }
        }
    }
    Ok(h)
}

/// The realistic identification of a level plant (module documentation):
/// white drives for `records` record lengths at amplitude scale `s_id`, run as
/// one continuous record (circular over the whole of it, so each frame sees
/// leakage from its neighbours), responses with noise of `noise_rel` times
/// each control's rms, the H1 estimate at the pulse's line spacing.
pub fn identify_h1(
    plant: &LevelPlant,
    s_id: f64,
    records: usize,
    noise_rel: f64,
    seed: u64,
) -> Result<MatrixStack, String> {
    let len = RECORD_LEN * records.next_power_of_two();
    let mut rng = Rng::new(seed);
    let drives = random_drives(plant.n_d, len, 1.0, &mut rng);
    let model = plant.modal_truth(s_id);
    let x: Vec<Vec<C64>> = drives.iter().map(|d| rfft(d)).collect::<Result<_, _>>()?;
    let w = omegas(FS_HZ, len);
    let mut y = vec![vec![C64::new(0.0, 0.0); w.len()]; plant.n_c];
    let mut hl = vec![C64::new(0.0, 0.0); plant.n_c * plant.n_d];
    for (f, &wf) in w.iter().enumerate() {
        model.eval(wf, &mut hl);
        if f == w.len() - 1 {
            for v in hl.iter_mut() {
                *v = C64::new(v.re, 0.0);
            }
        }
        for r in 0..plant.n_c {
            let mut acc = C64::new(0.0, 0.0);
            for c in 0..plant.n_d {
                acc += hl[r * plant.n_d + c] * x[c][f];
            }
            y[r][f] = acc;
        }
    }
    let mut resp: Vec<Vec<f64>> = y.iter().map(|s| irfft(s, len)).collect::<Result<_, _>>()?;
    add_relative_noise(&mut resp, noise_rel, &mut rng);
    h1_from_records(&drives, &resp, RECORD_LEN)
}

/// An SRS setup on any plant model: every control has the reference shock as
/// its waveform and its SRS (Q 10) as its target, every line trusted and
/// driven (as [`crate::reference_plant::srs_setup`]).
pub fn research_setup(frf: MatrixStack) -> Setup {
    let n_c = frf.rows();
    research_setup_signed(frf, &vec![1.0; n_c])
}

/// [`research_setup`] with each control's waveform multiplied by its sign in
/// `polarity` (the SRS targets are unchanged, a maximax SRS being blind to
/// polarity). Mixed polarity excites the modes whose shapes differ in sign
/// between the controls.
pub fn research_setup_signed(frf: MatrixStack, polarity: &[f64]) -> Setup {
    let (_, n_c, n_d) = frf.dims();
    let r = reference_shock(FS_HZ, RECORD_LEN);
    let refs: Vec<Vec<f64>> = (0..n_c)
        .map(|c| {
            r.iter()
                .map(|v| v * polarity.get(c).copied().unwrap_or(1.0))
                .collect()
        })
        .collect();
    let grid = srs_grid_hz();
    let t = srs_rows(std::slice::from_ref(&r), FS_HZ, &grid, 10.0, 1.0).remove(0);
    Setup {
        fs_hz: FS_HZ,
        record_len: RECORD_LEN,
        n_drives: n_d,
        n_controls: n_c,
        band_lo_hz: 0.0,
        band_hi_hz: FS_HZ / 2.0,
        reference: refs,
        srs: Some(SrsTarget {
            natural_hz: grid,
            q: 10.0,
            points: vec![t; n_c],
        }),
        plant: PlantModel {
            frf,
            trusted: vec![true; RECORD_LEN / 2 + 1],
        },
    }
}

/// A plant as the ladder runner sees it: drive records and the pulse's
/// amplitude scale in, response records out.
pub type Respond<'a> = dyn FnMut(&[Vec<f64>], f64) -> Result<Vec<Vec<f64>>, String> + 'a;

/// What one pulse of [`run_research_ladder`] measured.
#[derive(Clone, Debug, PartialEq)]
pub struct ResearchRow {
    /// Level played, dB.
    pub level_db: f64,
    /// Largest absolute SRS error over every control and point, dB.
    pub worst_db: f64,
    /// Largest signed SRS error (an over-test when positive), dB.
    pub over_db: f64,
    /// `norm(y / s - r) / norm(r)`.
    pub waveform_error: f64,
    /// Drive rms over every drive, divided by the pulse's scale.
    pub drive_rms: f64,
    /// The law's diagnostics from the iteration on this pulse.
    pub values: Vec<(&'static str, f64)>,
}

impl ResearchRow {
    /// A named diagnostic, NaN when absent.
    pub fn value(&self, name: &str) -> f64 {
        self.values
            .iter()
            .find(|(n, _)| *n == name)
            .map(|v| v.1)
            .unwrap_or(f64::NAN)
    }
}

/// A drive ceiling: drive `drive` is clipped to `fraction` of the seed's
/// peak on that drive (scaled to each pulse's level).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ceiling {
    /// The clipped drive.
    pub drive: usize,
    /// The ceiling as a fraction of the seed's full-level peak.
    pub fraction: f64,
}

/// Plays `ladder` through `respond` with a law, as
/// [`crate::reference_plant::run_ladder`] does, and also iterates on the last
/// pulse so its diagnostics are reported. `noise_rel` is the absolute noise
/// as a fraction of the reference's rms.
pub fn run_research_ladder(
    law: &mut dyn LawSteps,
    setup: &Setup,
    respond: &mut Respond<'_>,
    ladder: &[f64],
    noise_rel: f64,
    seed: u64,
    ceiling: Option<Ceiling>,
) -> Result<Vec<ResearchRow>, String> {
    let mut rng = Rng::new(seed);
    let target = setup.srs.as_ref().ok_or("the run needs an SRS target")?;
    let ref_rms = rms_all(&setup.reference);
    let mut out = law.seed(ladder[0])?;
    let clip_level = ceiling.map(|c| {
        let s0 = level_scale(ladder[0]);
        c.fraction
            * out.drives[c.drive]
                .iter()
                .fold(0.0f64, |a, v| a.max(v.abs()))
            / s0
    });
    let mut rows = Vec::with_capacity(ladder.len());
    for (k, &level) in ladder.iter().enumerate() {
        let s = level_scale(level);
        let mut drives = out.drives;
        if let (Some(c), Some(lim)) = (ceiling, clip_level) {
            for v in drives[c.drive].iter_mut() {
                *v = v.clamp(-lim * s, lim * s);
            }
        }
        let mut y = respond(&drives, s)?;
        if noise_rel > 0.0 {
            for row in y.iter_mut() {
                for v in row.iter_mut() {
                    *v += noise_rel * ref_rms * rng.normal();
                }
            }
        }
        let srs = srs_rows(&y, setup.fs_hz, &target.natural_hz, target.q, s);
        let (mut worst, mut over) = (0.0f64, f64::NEG_INFINITY);
        for (m, t) in srs.iter().zip(&target.points) {
            for e in error_db(m, t) {
                worst = worst.max(e.abs());
                over = over.max(e);
            }
        }
        let yt: Vec<Vec<f64>> = y
            .iter()
            .map(|r| r.iter().map(|v| v / s).collect())
            .collect();
        let next_level = ladder.get(k + 1).copied().unwrap_or(level);
        let inputs = crate::PulseInputs {
            pulse_index: k,
            level_db: level,
            next_level_db: next_level,
            srs: Some(
                srs.iter()
                    .map(|r| r.iter().map(|v| v * s).collect())
                    .collect(),
            ),
            drives: drives.clone(),
            responses: y,
        };
        let drive_rms = rms_all(&drives) / s;
        out = law.iterate(&inputs)?;
        rows.push(ResearchRow {
            level_db: level,
            worst_db: worst,
            over_db: over,
            waveform_error: crate::spectrum::relative_error(&yt, &setup.reference),
            drive_rms,
            values: out.values.clone(),
        });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_shock_level_plant_reproduces_the_modal_plant() {
        let p = ModalPlant::four_by_four(0.08, 0.5);
        let q = LevelPlant::from_modal(&p);
        let a = p.frf(0.7, FS_HZ, 1024);
        let b = q.frf(0.7, FS_HZ, 1024);
        let worst = a.data().iter().zip(b.data()).fold(0.0f64, |m, (x, y)| {
            m.max((x - y).norm() / x.norm().max(1e-12))
        });
        assert!(worst < 1e-12, "{worst}");
    }
}
