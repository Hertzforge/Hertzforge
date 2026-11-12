//! The exact quasi-linear reference plant, the reference shock and a seeded
//! generator, for tests and the study example. No law uses this module.
//!
//! The plant is modal. Each mode's natural frequency and damping are set by
//! the pulse's amplitude scale `s` and held for the whole pulse:
//!
//! `f_m(s) = f_m0 (1 - delta s)`,   `zeta(s) = zeta0 (1 + kappa s)`
//!
//! so a full-level pulse (`s = 1`) sees every mode lowered by `delta` and its
//! damping multiplied by `1 + kappa`. Per line, with `w` the angular frequency,
//!
//! `H(w; s) = diag(exp(-j w tau_c)) ( B + sum_m w^2 / (w_m^2 - w^2 + 2 j zeta w_m w) phi_m psi_m^T )`
//!
//! with `B` a quasi-static coupling (controls by drives), `phi_m` and `psi_m`
//! the mode's shapes at the controls and at the drives, and `tau_c` a delay per
//! control. A pulse's response is exact in the frequency domain on the
//! record's own lines (a circular convolution), which is what the per-line
//! analysis of the laws assumes. The plants reproduce the numerical
//! experiments of the research record (E3 and E7) so their numbers can be
//! compared.

use crate::spectrum::{omegas, rfft, rows_irfft};
use crate::srs::srs_rows;
use crate::{MatrixStack, PlantModel, Setup, SrsTarget, C64};

/// One mode of a [`ModalPlant`].
#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    /// Natural frequency at zero level, Hz.
    pub fn0_hz: f64,
    /// Shape at each control.
    pub phi: Vec<f64>,
    /// Shape at each drive.
    pub psi: Vec<f64>,
}

/// A modal plant whose frequencies and damping move with the pulse's level.
#[derive(Clone, Debug, PartialEq)]
pub struct ModalPlant {
    /// Number of controls.
    pub n_c: usize,
    /// Number of drives.
    pub n_d: usize,
    /// Quasi-static coupling, `n_c x n_d` row-major.
    pub quasi_static: Vec<f64>,
    /// The modes.
    pub modes: Vec<Mode>,
    /// Damping ratio at zero level.
    pub zeta0: f64,
    /// Fractional frequency drop at full level.
    pub delta: f64,
    /// Damping growth at full level: damping is `zeta0 (1 + kappa)` there.
    pub kappa: f64,
    /// Delay at each control, seconds.
    pub delays_s: Vec<f64>,
}

impl ModalPlant {
    /// The one-drive, one-control plant of experiment E3: a flat path plus
    /// modes at 400 Hz (participation 1.5) and 1100 Hz (0.8), damping 0.02.
    pub fn siso(delta: f64, kappa: f64) -> Self {
        ModalPlant {
            n_c: 1,
            n_d: 1,
            quasi_static: vec![1.0],
            modes: vec![
                Mode {
                    fn0_hz: 400.0,
                    phi: vec![1.5],
                    psi: vec![1.0],
                },
                Mode {
                    fn0_hz: 1100.0,
                    phi: vec![0.8],
                    psi: vec![1.0],
                },
            ],
            zeta0: 0.02,
            delta,
            kappa,
            delays_s: vec![0.0],
        }
    }

    /// The two-drive, three-control plant of experiment E7.
    pub fn two_by_three(delta: f64, kappa: f64) -> Self {
        ModalPlant {
            n_c: 3,
            n_d: 2,
            quasi_static: vec![1.0, 0.2, 0.6, 0.6, 0.2, 1.0],
            modes: vec![
                Mode {
                    fn0_hz: 400.0,
                    phi: vec![1.0, 0.6, -0.4],
                    psi: vec![1.0, 0.5],
                },
                Mode {
                    fn0_hz: 1100.0,
                    phi: vec![0.5, -0.8, 0.9],
                    psi: vec![0.4, -0.9],
                },
            ],
            zeta0: 0.02,
            delta,
            kappa,
            delays_s: vec![0.0, 0.25e-3, 0.5e-3],
        }
    }

    /// A square four-drive, four-control plant with four coupled modes and a
    /// delay per control (defined for this study; not in the earlier record).
    pub fn four_by_four(delta: f64, kappa: f64) -> Self {
        ModalPlant {
            n_c: 4,
            n_d: 4,
            quasi_static: vec![
                1.0, 0.2, 0.1, 0.0, //
                0.2, 1.0, 0.2, 0.1, //
                0.1, 0.2, 1.0, 0.2, //
                0.0, 0.1, 0.2, 1.0,
            ],
            modes: vec![
                Mode {
                    fn0_hz: 400.0,
                    phi: vec![1.0, 0.6, -0.4, 0.2],
                    psi: vec![1.0, 0.5, -0.3, 0.2],
                },
                Mode {
                    fn0_hz: 700.0,
                    phi: vec![0.3, -0.7, 0.8, 0.4],
                    psi: vec![0.5, -0.6, 0.4, 0.3],
                },
                Mode {
                    fn0_hz: 1100.0,
                    phi: vec![0.5, -0.8, 0.9, -0.3],
                    psi: vec![0.4, -0.9, 0.2, 0.6],
                },
                Mode {
                    fn0_hz: 1600.0,
                    phi: vec![0.4, 0.3, -0.6, 0.9],
                    psi: vec![0.2, 0.3, -0.8, 0.7],
                },
            ],
            zeta0: 0.02,
            delta,
            kappa,
            delays_s: vec![0.0, 0.1e-3, 0.2e-3, 0.3e-3],
        }
    }

    /// The plant at amplitude scale `s` on the lines of a record of
    /// `record_len` samples at `fs_hz`: `lines x n_c x n_d`.
    pub fn frf(&self, s: f64, fs_hz: f64, record_len: usize) -> MatrixStack {
        let w = omegas(fs_hz, record_len);
        let mut h = MatrixStack::zeros(w.len(), self.n_c, self.n_d);
        let z = self.zeta0 * (1.0 + self.kappa * s);
        for (f, &wf) in w.iter().enumerate() {
            let line = h.line_mut(f);
            for r in 0..self.n_c {
                for c in 0..self.n_d {
                    line[r * self.n_d + c] = C64::new(self.quasi_static[r * self.n_d + c], 0.0);
                }
            }
            for m in &self.modes {
                let wn = std::f64::consts::TAU * m.fn0_hz * (1.0 - self.delta * s);
                let q = C64::new(wf * wf, 0.0) / C64::new(wn * wn - wf * wf, 2.0 * z * wn * wf);
                for r in 0..self.n_c {
                    for c in 0..self.n_d {
                        line[r * self.n_d + c] += q * (m.phi[r] * m.psi[c]);
                    }
                }
            }
            for r in 0..self.n_c {
                let ph = C64::from_polar(1.0, -wf * self.delays_s[r]);
                for c in 0..self.n_d {
                    line[r * self.n_d + c] *= ph;
                }
            }
        }
        // A real record carries only a real value at the Nyquist line, so the
        // plant a real record sees there is the real part; the model given to
        // a law must be the same plant.
        let last = w.len() - 1;
        for v in h.line_mut(last) {
            *v = C64::new(v.re, 0.0);
        }
        h
    }

    /// The control records for drive records played at amplitude scale `s`.
    pub fn respond(
        &self,
        drives: &[Vec<f64>],
        s: f64,
        fs_hz: f64,
    ) -> Result<Vec<Vec<f64>>, String> {
        let n = drives.first().map(|d| d.len()).unwrap_or(0);
        let h = self.frf(s, fs_hz, n);
        let d: Vec<Vec<C64>> = drives.iter().map(|x| rfft(x)).collect::<Result<_, _>>()?;
        let y = crate::spectrum::apply(&h, &d);
        rows_irfft(&y, n, 1.0)
    }
}

/// A small seeded pseudo-random generator (SplitMix64, normal values by the
/// Box-Muller transform), so tests and the study are reproducible.
pub struct Rng {
    state: u64,
    spare: Option<f64>,
}

impl Rng {
    /// A generator from a seed.
    pub fn new(seed: u64) -> Self {
        Rng {
            state: seed,
            spare: None,
        }
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `(0, 1)`.
    pub fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Standard normal.
    pub fn normal(&mut self) -> f64 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        let u1 = self.uniform();
        let u2 = self.uniform();
        let r = (-2.0 * u1.ln()).sqrt();
        let a = std::f64::consts::TAU * u2;
        self.spare = Some(r * a.sin());
        r * a.cos()
    }
}

/// Sample rate of the record's experiments, Hz.
pub const FS_HZ: f64 = 20480.0;
/// Record length of the record's experiments.
pub const RECORD_LEN: usize = 16384;

/// The component frequencies of the reference shock: one-sixth octave from
/// 100 Hz, up to 2000 Hz.
pub fn component_hz() -> Vec<f64> {
    (0..26)
        .map(|i| 100.0 * 2f64.powf(i as f64 / 6.0))
        .filter(|&f| f <= 2000.0)
        .collect()
}

/// The SRS natural frequencies: one-twelfth octave from 100 Hz to 2000 Hz.
pub fn srs_grid_hz() -> Vec<f64> {
    (0..53)
        .map(|i| 100.0 * 2f64.powf(i as f64 / 12.0))
        .filter(|&f| f <= 2000.0)
        .collect()
}

/// The reference shock of experiment E3: a sum of decaying sinusoids at
/// [`component_hz`], amplitude `10 (f / 100)^0.3`, damping 0.03, delays
/// `4 + 0.5 (i mod 3)` ms.
pub fn reference_shock(fs_hz: f64, record_len: usize) -> Vec<f64> {
    let mut x = vec![0.0; record_len];
    for (i, f) in component_hz().into_iter().enumerate() {
        let a = 10.0 * (f / 100.0).powf(0.3);
        let tau = 0.004 + 0.0005 * (i % 3) as f64;
        for (k, v) in x.iter_mut().enumerate() {
            let tt = k as f64 / fs_hz - tau;
            if tt > 0.0 {
                let wt = std::f64::consts::TAU * f * tt;
                *v += a * (-0.03 * wt).exp() * wt.sin();
            }
        }
    }
    x
}

/// An SRS test on a plant: every control has the reference shock as its
/// waveform and its SRS (Q 10) as its target, the plant identified exactly at
/// `id_scale`, every line trusted and driven.
pub fn srs_setup(plant: &ModalPlant, id_scale: f64) -> Setup {
    let r = reference_shock(FS_HZ, RECORD_LEN);
    let grid = srs_grid_hz();
    let t = srs_rows(std::slice::from_ref(&r), FS_HZ, &grid, 10.0, 1.0).remove(0);
    Setup {
        fs_hz: FS_HZ,
        record_len: RECORD_LEN,
        n_drives: plant.n_d,
        n_controls: plant.n_c,
        band_lo_hz: 0.0,
        band_hi_hz: FS_HZ / 2.0,
        reference: vec![r; plant.n_c],
        srs: Some(SrsTarget {
            natural_hz: grid,
            q: 10.0,
            points: vec![t; plant.n_c],
        }),
        plant: PlantModel {
            frf: plant.frf(id_scale, FS_HZ, RECORD_LEN),
            trusted: vec![true; RECORD_LEN / 2 + 1],
        },
    }
}

/// The ladder of the record's closed-loop experiments: -12, -9, -6, -3 dB, then
/// ten pulses at 0 dB.
pub const LADDER_DB: [f64; 14] = [
    -12.0, -9.0, -6.0, -3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
];

/// What one pulse of [`run_ladder`] measured.
#[derive(Clone, Debug, PartialEq)]
pub struct PulseRow {
    /// Level played, dB.
    pub level_db: f64,
    /// Worst-point SRS error, dB (largest absolute error over every control
    /// and natural frequency, response divided by the pulse's scale).
    pub worst_srs_db: f64,
    /// Worst-point SRS error per control, dB.
    pub srs_db_per_control: Vec<f64>,
    /// `norm(y / s - r) / norm(r)` over every control.
    pub waveform_error: f64,
    /// Rms of every drive together, divided by the pulse's scale.
    pub drive_rms: f64,
    /// The delays the law reported building this pulse for, seconds.
    pub delays_s: Option<Vec<f64>>,
}

/// Play `ladder` through `plant` with a law: seed, then fire, measure and
/// iterate. `noise_rel` adds Gaussian noise to every response, its standard
/// deviation that fraction of the reference's rms, in absolute terms (the
/// same at every level). Drives are played as the law returned them.
pub fn run_ladder(
    law: &mut dyn crate::LawSteps,
    setup: &Setup,
    plant: &ModalPlant,
    ladder: &[f64],
    noise_rel: f64,
    seed: u64,
) -> Result<Vec<PulseRow>, String> {
    let mut rng = Rng::new(seed);
    let target = setup.srs.as_ref().ok_or("the run needs an SRS target")?;
    let ref_rms = crate::spectrum::rms_all(&setup.reference);
    let mut out = law.seed(ladder[0])?;
    let mut rows = Vec::with_capacity(ladder.len());
    for (k, &level) in ladder.iter().enumerate() {
        let s = crate::level_scale(level);
        let mut y = plant.respond(&out.drives, s, setup.fs_hz)?;
        if noise_rel > 0.0 {
            for row in y.iter_mut() {
                for v in row.iter_mut() {
                    *v += noise_rel * ref_rms * rng.normal();
                }
            }
        }
        let srs = srs_rows(&y, setup.fs_hz, &target.natural_hz, target.q, s);
        let per: Vec<f64> = srs
            .iter()
            .zip(&target.points)
            .map(|(m, t)| {
                crate::srs::worst_error_db(std::slice::from_ref(m), std::slice::from_ref(t))
            })
            .collect();
        let yt: Vec<Vec<f64>> = y
            .iter()
            .map(|r| r.iter().map(|v| v / s).collect())
            .collect();
        rows.push(PulseRow {
            level_db: level,
            worst_srs_db: per.iter().fold(0.0f64, |a, &b| a.max(b)),
            srs_db_per_control: per,
            waveform_error: crate::spectrum::relative_error(&yt, &setup.reference),
            drive_rms: crate::spectrum::rms_all(&out.drives) / s,
            delays_s: out.delays_s.clone(),
        });
        if k + 1 == ladder.len() {
            break;
        }
        let inputs = crate::PulseInputs {
            pulse_index: k,
            level_db: level,
            next_level_db: ladder[k + 1],
            srs: Some(
                srs.iter()
                    .map(|r| r.iter().map(|v| v * s).collect())
                    .collect(),
            ),
            drives: out.drives,
            responses: y,
        };
        out = law.iterate(&inputs)?;
    }
    Ok(rows)
}
