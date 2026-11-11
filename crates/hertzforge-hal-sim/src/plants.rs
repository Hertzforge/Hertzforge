//! A standard set of cross-coupled plants for multiple-output control work.
//!
//! | Name | Inputs x drives | Modes | Claim |
//! |---|---|---|---|
//! | `modal-4x4` | 4 x 4 | 12 | well-conditioned 20 Hz to 2 kHz |
//! | `modal-12x6` | 12 x 6 | 20 | well-conditioned 20 Hz to 2 kHz |
//! | `modal-12x6-ill` | 12 x 6 | 20 | ill-conditioned: drives 4 and 5 act through nearly the same shape |
//! | `modal-24x24` | 24 x 24 | 40 | well-conditioned 20 Hz to 2 kHz |
//! | `modal-72x24` | 72 x 24 | 40 | well-conditioned 20 Hz to 2 kHz |
//! | `rigid-plate-12x6` | 12 x 6 | 6 rigid + 4 flexible | well-conditioned 25 to 150 Hz, its rigid-body band |
//!
//! Every plant carries its geometry (where each input sensor and each drive sits and
//! which way it points) and its conditioning claim ([`Conditioning`]). The claims are
//! checked against [`PlantModel::analytic_frf`] by this crate's tests.
//!
//! # The random modal plants
//!
//! Built by [`random_modal`] from a seed. Sensors and drives sit on a 1 m square plate,
//! all pointing along `+z`. The first `n_drives` inputs are collocated with the drives
//! (same point, same direction); the rest sit on their own grid. Each mode has a natural
//! frequency spread log-uniformly over the band (with jitter), a `Q` drawn uniformly
//! from the range, and a shape that is a random smooth field over the plate, so every
//! drive excites every mode and every input sees every mode. Modes are
//! [`ModeResponse::Bandpass`].
//!
//! A frequency-independent residual (the out-of-band modes' residual flexibility) is
//! added: `residual[i][j] = r0 * exp(-(|p_i - p_j| / 5 cm)^2)`, a local stiffness felt by
//! sensors near a drive. Why this makes the conditioning a property of the construction
//! rather than of the seed: the collocated block is `r0 K + sum_m R_m(f) phi_m phi_m^T`
//! with `K` the kernel among the drive points, and a bandpass mode has a non-negative
//! real part, so the block's Hermitian part is at least `r0 * lambda_min(K)` and so is
//! the smallest singular value of the whole FRF (extra rows can only raise it). The
//! largest is at most `r0 * |K| + sum_m |R_m(f)| |psi_m| |phi_m|`. `r0` is chosen so the
//! ratio is at most [`RandomModalSpec::target_condition`] (40 for the standard set, under
//! the claimed 100), with `lambda_min(K)` bounded by Gershgorin's theorem. Two drives
//! placed almost on top of each other make `K` nearly singular, which is how
//! `modal-12x6-ill` is ill-conditioned.
//!
//! # The rigid plate
//!
//! [`rigid_plate_12x6`]: a 1.2 m by 0.9 m, 120 kg plate on a soft suspension, four
//! triaxial accelerometers at its corners (12 inputs) and six drives (three vertical,
//! two along `x`, one along `y`). Six rigid-body modes between 5 and 12 Hz, one per
//! degree of freedom in the order [`RIGID_DOFS`] about the plate centre, with shapes
//! from the geometry (sensor kinematics and the drives' generalized forces over mass and
//! inertia), plus four plate bending modes between 700 Hz and 1.4 kHz (each with half
//! the plate's mass as its modal mass). All modes are
//! [`ModeResponse::Acceleration`], so above the suspension the plate sits on its mass
//! line, and a six-degree-of-freedom transformation built from the geometry alone
//! controls it.
//!
//! # The shock plants
//!
//! A second set, [`ShockPlant`], for multiple-drive shock control: a fixture plate driven
//! at several points, read by accelerometers near them, with flexible modes between
//! 60 Hz and 1.8 kHz on a mass line, every mode of the acceleration shape, and a delay
//! per control point from 0 to 0.5 ms (the time a shock takes to reach each point).
//! Every drive reaches every control. They are [`PlantModel::Jointed`] plants: the
//! jointed ones lose frequency and gain damping with amplitude, as a bolted structure
//! does, so the phase of the plant at full level is not its phase at a low level.
//!
//! | Name | Controls x drives | Joint |
//! |---|---|---|
//! | `shock-linear-4x4` | 4 x 4 | none (linear) |
//! | `shock-jointed-4x4-moderate` | 4 x 4 | 3 % drop, damping doubled at the reference amplitude |
//! | `shock-jointed-4x4-severe` | 4 x 4 | 8 % drop, damping unchanged |
//! | `shock-jointed-6x3` | 6 x 3 | moderate |
//!
//! The three 4 x 4 plants are one structure. The standard shock plants' frequency drop
//! grows linearly with amplitude (`freq_exponent` 1) and their damping as its square
//! root (`damping_exponent` 0.5), so 20 dB below full level a resonance sits within
//! 1 % of its small-signal frequency while its damping has already begun to rise.
//!
//! ## Calibration
//!
//! Each mode's reference amplitude is a modal velocity divided by its frequency,
//! `reference_amplitude = V_ref / (2 pi f)`: one modal velocity for every mode, the
//! quantity a shock's severity tracks across frequency. `V_ref` is set so the
//! reference pulse ([`reference_pulse`]: a 20 g, 11 ms half-sine with its pre- and
//! post-pulses) played at the first control by the drives of least energy that make
//! the linear plant play it ([`JointedPlant::least_drive_for`], at 12 800 Hz over
//! 16 384 samples) brings the mode nearest its reference (the dominant mode) exactly
//! to it. Other modes stay below theirs under that pulse. The tests recompute the
//! calibration and measure the shift and the damping at full level from the plant's
//! own response.

use crate::jointed::{JointLaw, JointedMode, JointedPlant};
use crate::plant::{ModalPlant, Mode, ModeResponse, PlantModel, DEFAULT_FEEDTHROUGH};
use crate::Xorshift64;

/// The rigid-body degrees of freedom, in the order of a plate's first six modes:
/// translations along and rotations about the reference point's axes.
pub const RIGID_DOFS: [&str; 6] = ["x", "y", "z", "rx", "ry", "rz"];

/// The standard set, by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StandardPlant {
    Modal4x4,
    Modal12x6,
    Modal12x6Ill,
    Modal24x24,
    Modal72x24,
    RigidPlate12x6,
}

impl StandardPlant {
    /// Every standard plant.
    pub const ALL: [StandardPlant; 6] = [
        StandardPlant::Modal4x4,
        StandardPlant::Modal12x6,
        StandardPlant::Modal12x6Ill,
        StandardPlant::Modal24x24,
        StandardPlant::Modal72x24,
        StandardPlant::RigidPlate12x6,
    ];

    /// The plant's name (`modal-4x4`, ...).
    pub fn name(self) -> &'static str {
        match self {
            StandardPlant::Modal4x4 => "modal-4x4",
            StandardPlant::Modal12x6 => "modal-12x6",
            StandardPlant::Modal12x6Ill => "modal-12x6-ill",
            StandardPlant::Modal24x24 => "modal-24x24",
            StandardPlant::Modal72x24 => "modal-72x24",
            StandardPlant::RigidPlate12x6 => "rigid-plate-12x6",
        }
    }

    /// The plant with this name, if it is one of the set.
    pub fn from_name(name: &str) -> Option<StandardPlant> {
        StandardPlant::ALL.into_iter().find(|p| p.name() == name)
    }

    /// `(n_inputs, n_drives)`.
    pub fn dims(self) -> (usize, usize) {
        match self {
            StandardPlant::Modal4x4 => (4, 4),
            StandardPlant::Modal12x6 | StandardPlant::Modal12x6Ill => (12, 6),
            StandardPlant::Modal24x24 => (24, 24),
            StandardPlant::Modal72x24 => (72, 24),
            StandardPlant::RigidPlate12x6 => (12, 6),
        }
    }

    /// Build the plant.
    pub fn build(self) -> NamedPlant {
        let spec = |n_in, n_d, n_modes, seed| RandomModalSpec {
            n_inputs: n_in,
            n_drives: n_d,
            n_modes,
            seed,
            ..RandomModalSpec::default()
        };
        let mut p = match self {
            StandardPlant::Modal4x4 => random_modal(&spec(4, 4, 12, 0x4D4F_4441_4C04_0401)),
            StandardPlant::Modal12x6 => random_modal(&spec(12, 6, 20, 0x4D4F_4441_4C12_0601)),
            StandardPlant::Modal12x6Ill => random_modal(&RandomModalSpec {
                near_pair: Some((4, 5, 0.001)),
                ..spec(12, 6, 20, 0x4D4F_4441_4C12_0601)
            }),
            StandardPlant::Modal24x24 => random_modal(&spec(24, 24, 40, 0x4D4F_4441_4C24_2401)),
            StandardPlant::Modal72x24 => random_modal(&spec(72, 24, 40, 0x4D4F_4441_4C72_2401)),
            StandardPlant::RigidPlate12x6 => rigid_plate_12x6(),
        };
        p.name = self.name().to_string();
        p
    }
}

/// Where a sensor or a drive sits and which way it points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Position in metres, in the plant's frame.
    pub position_m: [f64; 3],
    /// Unit direction of measurement or of the applied force.
    pub direction: [f64; 3],
}

/// A plant's geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    /// One per measurement input (FRF row), in order.
    pub inputs: Vec<Placement>,
    /// One per drive (FRF column), in order.
    pub drives: Vec<Placement>,
    /// The point rigid-body rotations are about.
    pub reference_point_m: [f64; 3],
}

/// The rigid body behind a plant whose first six modes are its rigid-body modes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigidBody {
    pub mass_kg: f64,
    /// Principal moments of inertia about the reference point's `x`, `y`, `z` axes.
    pub inertia_kg_m2: [f64; 3],
    /// Force per drive unit, newtons.
    pub drive_gain_n: f64,
    /// Suspension frequency of each degree of freedom, in [`RIGID_DOFS`] order.
    pub dof_hz: [f64; 6],
    /// Suspension `Q`, every degree of freedom.
    pub q: f64,
}

/// A plant's conditioning claim, over a band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Conditioning {
    /// The FRF's condition number stays below `max` on every line in the band.
    Well { band_hz: (f64, f64), max: f64 },
    /// The FRF's condition number exceeds `exceeds` somewhere in the band.
    Ill { band_hz: (f64, f64), exceeds: f64 },
}

/// A plant with its name, geometry and conditioning claim.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedPlant {
    pub name: String,
    pub model: PlantModel,
    pub geometry: Geometry,
    pub conditioning: Conditioning,
    /// Set when the first six modes are a rigid body's, in [`RIGID_DOFS`] order.
    pub rigid_body: Option<RigidBody>,
}

/// The recipe for [`random_modal`].
#[derive(Debug, Clone, PartialEq)]
pub struct RandomModalSpec {
    pub n_inputs: usize,
    pub n_drives: usize,
    pub n_modes: usize,
    pub f_lo_hz: f64,
    pub f_hi_hz: f64,
    pub q_lo: f64,
    pub q_hi: f64,
    pub seed: u64,
    /// The condition number the residual is sized for (see the module docs). The
    /// claim made is 100; the construction aims under it.
    pub target_condition: f64,
    /// `(a, b, offset_m)`: place drive `b` (and its collocated input) `offset_m` from
    /// drive `a`, so the two act through nearly the same shape.
    pub near_pair: Option<(usize, usize, f64)>,
}

impl Default for RandomModalSpec {
    fn default() -> Self {
        RandomModalSpec {
            n_inputs: 4,
            n_drives: 4,
            n_modes: 12,
            f_lo_hz: 20.0,
            f_hi_hz: 2000.0,
            q_lo: 10.0,
            q_hi: 50.0,
            seed: 1,
            target_condition: 40.0,
            near_pair: None,
        }
    }
}

/// The width of the residual's locality kernel, metres.
const KERNEL_M: f64 = 0.05;

/// Kernel values below this are exactly zero, so the residual is sparse (sensors near a
/// drive) and the plant steps only what matters. Part of the model, so the exact FRF and
/// the stepped plant agree; the conditioning bounds are computed on the cut kernel.
const KERNEL_CUT: f64 = 1e-6;

fn uniform(rng: &mut Xorshift64, lo: f64, hi: f64) -> f64 {
    lo + (hi - lo) * rng.next_unit()
}

/// Positions on a jittered grid over the unit plate centred at the origin.
fn jittered_grid(n: usize, rng: &mut Xorshift64) -> Vec<[f64; 3]> {
    let g = (n as f64).sqrt().ceil().max(1.0) as usize;
    let cell = 1.0 / g as f64;
    (0..n)
        .map(|k| {
            let (cx, cy) = ((k % g) as f64, (k / g) as f64);
            [
                -0.5 + (cx + 0.5) * cell + uniform(rng, -0.2, 0.2) * cell,
                -0.5 + (cy + 0.5) * cell + uniform(rng, -0.2, 0.2) * cell,
                0.0,
            ]
        })
        .collect()
}

/// The analog magnitude of a constant-peak bandpass resonator (peak `q` at `f_n`).
fn bandpass_mag(f: f64, f_n: f64, q: f64) -> f64 {
    let x = f / f_n - f_n / f;
    q / (1.0 + q * q * x * x).sqrt()
}

/// A seeded random modal plant (see the module docs). Panics on a spec with no inputs,
/// fewer inputs than drives, or no modes.
pub fn random_modal(spec: &RandomModalSpec) -> NamedPlant {
    let (n_in, n_d) = (spec.n_inputs, spec.n_drives);
    assert!(
        n_d >= 1 && n_in >= n_d,
        "random_modal needs n_inputs >= n_drives >= 1"
    );
    assert!(spec.n_modes >= 1, "random_modal needs at least one mode");
    let mut rng = Xorshift64::new(spec.seed);

    // Geometry: drives on one jittered grid, the first n_d inputs collocated, the rest
    // on their own grid.
    let mut drive_pos = jittered_grid(n_d, &mut rng);
    if let Some((a, b, off)) = spec.near_pair {
        let p = drive_pos[a];
        drive_pos[b] = [p[0] + off, p[1], p[2]];
    }
    let mut input_pos = drive_pos.clone();
    input_pos.extend(jittered_grid(n_in - n_d, &mut rng));
    let z = [0.0, 0.0, 1.0];

    // Modes: log-spread frequencies with jitter, uniform Q, smooth random fields.
    let ratio = spec.f_hi_hz / spec.f_lo_hz;
    let mut modes = Vec::with_capacity(spec.n_modes);
    for m in 0..spec.n_modes {
        let t = (m as f64 + 0.5 + uniform(&mut rng, -0.3, 0.3)) / spec.n_modes as f64;
        let f_hz = spec.f_lo_hz * ratio.powf(t.clamp(0.0, 1.0));
        let q = uniform(&mut rng, spec.q_lo, spec.q_hi);
        let k_max = 1.0 + 3.0 * (m + 1) as f64 / spec.n_modes as f64;
        let comps: Vec<[f64; 5]> = (0..3)
            .map(|_| {
                [
                    rng.next_gaussian(),
                    uniform(&mut rng, 0.0, k_max),
                    uniform(&mut rng, 0.0, k_max),
                    uniform(&mut rng, 0.0, 2.0),
                    uniform(&mut rng, 0.0, 2.0),
                ]
            })
            .collect();
        let field = |p: &[f64; 3]| -> f64 {
            comps
                .iter()
                .map(|c| {
                    c[0] * (std::f64::consts::PI * (c[1] * (p[0] + 0.5) + c[3])).cos()
                        * (std::f64::consts::PI * (c[2] * (p[1] + 0.5) + c[4])).cos()
                })
                .sum()
        };
        let at_drives: Vec<f64> = drive_pos.iter().map(field).collect();
        let at_inputs: Vec<f64> = input_pos.iter().map(field).collect();
        let all: f64 = at_drives
            .iter()
            .chain(&at_inputs[n_d..])
            .map(|v| v * v)
            .sum();
        let rms = (all / (n_in as f64)).sqrt().max(1e-9);
        let s = rms * (n_d as f64).sqrt();
        modes.push(Mode {
            f_hz,
            q,
            input_shape: at_drives.iter().map(|v| v / s).collect(),
            output_shape: at_inputs.iter().map(|v| v / s).collect(),
            response: ModeResponse::Bandpass,
        });
    }

    // The residual's kernel, and the bounds that size it.
    let kernel = |a: &[f64; 3], b: &[f64; 3]| -> f64 {
        let d2: f64 = (0..3).map(|k| (a[k] - b[k]).powi(2)).sum();
        let v = (-d2 / (KERNEL_M * KERNEL_M)).exp();
        if v < KERNEL_CUT {
            0.0
        } else {
            v
        }
    };
    let k_full: Vec<f64> = input_pos
        .iter()
        .flat_map(|pi| drive_pos.iter().map(move |pj| kernel(pi, pj)))
        .collect();
    let gersh_lo = (0..n_d)
        .map(|i| {
            1.0 - (0..n_d)
                .filter(|&j| j != i)
                .map(|j| k_full[i * n_d + j])
                .sum::<f64>()
        })
        .fold(f64::INFINITY, f64::min);
    let row_max = (0..n_in)
        .map(|i| k_full[i * n_d..(i + 1) * n_d].iter().sum::<f64>())
        .fold(0.0, f64::max);
    let col_max = (0..n_d)
        .map(|j| (0..n_in).map(|i| k_full[i * n_d + j]).sum::<f64>())
        .fold(0.0, f64::max);
    let k_norm = (row_max * col_max).sqrt();
    let norms: Vec<f64> = modes
        .iter()
        .map(|m| {
            let a: f64 = m.input_shape.iter().map(|v| v * v).sum::<f64>().sqrt();
            let b: f64 = m.output_shape.iter().map(|v| v * v).sum::<f64>().sqrt();
            a * b
        })
        .collect();
    let s_at = |f: f64| -> f64 {
        modes
            .iter()
            .zip(&norms)
            .map(|(m, w)| w * bandpass_mag(f, m.f_hz, m.q))
            .sum()
    };
    let n_grid = 4000;
    let (g_lo, g_hi) = (spec.f_lo_hz / 4.0, spec.f_hi_hz * 4.0);
    let s_max = (0..n_grid)
        .map(|k| g_lo * (g_hi / g_lo).powf(k as f64 / (n_grid - 1) as f64))
        .chain(modes.iter().map(|m| m.f_hz))
        .map(s_at)
        .fold(0.0, f64::max);
    // A near pair drives the Gershgorin bound to nothing; size the residual as for a
    // well-separated layout (the plant is then ill-conditioned, which is its point).
    let lam_lo = if gersh_lo > 0.5 { gersh_lo } else { 1.0 };
    let r0 = s_max / (spec.target_condition * lam_lo - k_norm).max(1.0);
    let feedthrough = DEFAULT_FEEDTHROUGH * r0;
    let residual: Vec<f64> = k_full.iter().map(|k| r0 * k).collect();
    let s_bound = s_max + feedthrough * ((n_in * n_d) as f64).sqrt();

    let band = (spec.f_lo_hz, spec.f_hi_hz);
    let conditioning = if spec.near_pair.is_some() || gersh_lo <= 0.5 {
        Conditioning::Ill {
            band_hz: band,
            exceeds: 1000.0,
        }
    } else {
        debug_assert!((r0 * k_norm + s_bound) / (r0 * lam_lo) <= spec.target_condition * 1.01);
        Conditioning::Well {
            band_hz: band,
            max: 100.0,
        }
    };

    NamedPlant {
        name: "random-modal".to_string(),
        model: PlantModel::Modal(ModalPlant {
            n_inputs: n_in,
            n_drives: n_d,
            modes,
            feedthrough,
            residual,
            transport_delay_samples: 0,
            ..ModalPlant::new(0, 0, Vec::new())
        }),
        geometry: Geometry {
            inputs: input_pos
                .into_iter()
                .map(|p| Placement {
                    position_m: p,
                    direction: z,
                })
                .collect(),
            drives: drive_pos
                .into_iter()
                .map(|p| Placement {
                    position_m: p,
                    direction: z,
                })
                .collect(),
            reference_point_m: [0.0; 3],
        },
        conditioning,
        rigid_body: None,
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The rigid plate on a soft suspension (see the module docs).
pub fn rigid_plate_12x6() -> NamedPlant {
    const LX: f64 = 1.2;
    const LY: f64 = 0.9;
    let body = RigidBody {
        mass_kg: 120.0,
        inertia_kg_m2: [
            120.0 * LY * LY / 12.0,
            120.0 * LX * LX / 12.0,
            120.0 * (LX * LX + LY * LY) / 12.0,
        ],
        drive_gain_n: 100.0,
        dof_hz: [5.0, 6.0, 7.5, 9.0, 10.5, 12.0],
        q: 10.0,
    };
    let (ex, ey, ez) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
    let corners = [
        [0.5, 0.35, 0.0],
        [-0.5, 0.35, 0.0],
        [-0.5, -0.35, 0.0],
        [0.5, -0.35, 0.0],
    ];
    let inputs: Vec<Placement> = corners
        .iter()
        .flat_map(|&p| {
            [ex, ey, ez].into_iter().map(move |d| Placement {
                position_m: p,
                direction: d,
            })
        })
        .collect();
    let drives = vec![
        Placement {
            position_m: [0.45, 0.3, 0.0],
            direction: ez,
        },
        Placement {
            position_m: [-0.45, 0.3, 0.0],
            direction: ez,
        },
        Placement {
            position_m: [0.0, -0.35, 0.0],
            direction: ez,
        },
        Placement {
            position_m: [0.0, 0.42, 0.0],
            direction: ex,
        },
        Placement {
            position_m: [0.0, -0.42, 0.0],
            direction: ex,
        },
        Placement {
            position_m: [0.55, 0.0, 0.0],
            direction: ey,
        },
    ];

    let mut modes = Vec::new();
    // Rigid-body modes: shapes from kinematics (sensor reading per unit DOF motion) and
    // generalized force over mass or inertia (DOF acceleration per drive unit).
    for (m, &f_hz) in body.dof_hz.iter().enumerate() {
        let output_shape = inputs
            .iter()
            .map(|s| {
                if m < 3 {
                    s.direction[m]
                } else {
                    cross(s.position_m, s.direction)[m - 3]
                }
            })
            .collect();
        let input_shape = drives
            .iter()
            .map(|d| {
                let g = body.drive_gain_n;
                if m < 3 {
                    g * d.direction[m] / body.mass_kg
                } else {
                    g * cross(d.position_m, d.direction)[m - 3] / body.inertia_kg_m2[m - 3]
                }
            })
            .collect();
        modes.push(Mode {
            f_hz,
            q: body.q,
            input_shape,
            output_shape,
            response: ModeResponse::Acceleration,
        });
    }
    // Plate bending: out-of-plane shapes, felt by vertical sensors and drives only.
    type Shape = fn(f64, f64) -> f64;
    let bending: [(f64, Shape); 4] = [
        (700.0, |x, y| (2.0 * x / LX) * (2.0 * y / LY)),
        (850.0, |x, _| (std::f64::consts::TAU * x / LX).cos()),
        (1100.0, |_, y| (std::f64::consts::TAU * y / LY).cos()),
        (1400.0, |x, y| {
            (2.0 * x / LX) * (std::f64::consts::TAU * y / LY).cos()
        }),
    ];
    let modal_mass = body.mass_kg / 2.0;
    for (f_hz, w) in bending {
        modes.push(Mode {
            f_hz,
            q: 25.0,
            input_shape: drives
                .iter()
                .map(|d| {
                    body.drive_gain_n * w(d.position_m[0], d.position_m[1]) * dot(d.direction, ez)
                        / modal_mass
                })
                .collect(),
            output_shape: inputs
                .iter()
                .map(|s| w(s.position_m[0], s.position_m[1]) * dot(s.direction, ez))
                .collect(),
            response: ModeResponse::Acceleration,
        });
    }

    NamedPlant {
        name: "rigid-plate-12x6".to_string(),
        model: PlantModel::Modal(ModalPlant {
            n_inputs: inputs.len(),
            n_drives: drives.len(),
            modes,
            feedthrough: DEFAULT_FEEDTHROUGH,
            residual: Vec::new(),
            transport_delay_samples: 0,
            ..ModalPlant::new(0, 0, Vec::new())
        }),
        geometry: Geometry {
            inputs,
            drives,
            reference_point_m: [0.0; 3],
        },
        conditioning: Conditioning::Well {
            band_hz: (25.0, 150.0),
            max: 100.0,
        },
        rigid_body: Some(body),
    }
}

// ---------------------------------------------------------------------------------------
// The shock plants.
// ---------------------------------------------------------------------------------------

/// The standard shock plants, by name (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShockPlant {
    /// `shock-linear-4x4`.
    Linear4x4,
    /// `shock-jointed-4x4-moderate`.
    Jointed4x4Moderate,
    /// `shock-jointed-4x4-severe`.
    Jointed4x4Severe,
    /// `shock-jointed-6x3`.
    Jointed6x3,
}

/// The rate the shock plants are calibrated at, Hz.
pub const SHOCK_CALIBRATION_RATE_HZ: f64 = 12_800.0;

/// The record length the shock plants are calibrated over, samples.
pub const SHOCK_CALIBRATION_SAMPLES: usize = 16_384;

/// The reference pulse's peak, g, and duration, s.
pub const REFERENCE_PULSE_G: f64 = 20.0;
pub const REFERENCE_PULSE_S: f64 = 0.011;

/// The modal velocity at the reference amplitude of the 4 x 4 structure and of the
/// 6 x 3, from the calibration in the module docs (the tests recompute both).
pub const SHOCK_4X4_REFERENCE_VELOCITY: f64 = 1.2886e-2;
pub const SHOCK_6X3_REFERENCE_VELOCITY: f64 = 1.2400e-2;

impl ShockPlant {
    /// Every shock plant.
    pub const ALL: [ShockPlant; 4] = [
        ShockPlant::Linear4x4,
        ShockPlant::Jointed4x4Moderate,
        ShockPlant::Jointed4x4Severe,
        ShockPlant::Jointed6x3,
    ];

    /// The plant's name (`shock-linear-4x4`, ...).
    pub fn name(self) -> &'static str {
        match self {
            ShockPlant::Linear4x4 => "shock-linear-4x4",
            ShockPlant::Jointed4x4Moderate => "shock-jointed-4x4-moderate",
            ShockPlant::Jointed4x4Severe => "shock-jointed-4x4-severe",
            ShockPlant::Jointed6x3 => "shock-jointed-6x3",
        }
    }

    /// The plant with this name, if it is one of the set.
    pub fn from_name(name: &str) -> Option<ShockPlant> {
        ShockPlant::ALL.into_iter().find(|p| p.name() == name)
    }

    /// `(n_controls, n_drives)`.
    pub fn dims(self) -> (usize, usize) {
        match self {
            ShockPlant::Jointed6x3 => (6, 3),
            _ => (4, 4),
        }
    }

    /// The joint law, `None` for the linear plant.
    pub fn joint(self) -> Option<JointLaw> {
        let law = |delta, damping_gain| JointLaw {
            freq_exponent: 1.0,
            damping_exponent: 0.5,
            ..JointLaw::new(delta, damping_gain)
        };
        match self {
            ShockPlant::Linear4x4 => None,
            ShockPlant::Jointed4x4Moderate | ShockPlant::Jointed6x3 => Some(law(0.03, 1.0)),
            ShockPlant::Jointed4x4Severe => Some(law(0.08, 0.0)),
        }
    }

    /// The jointed plant itself.
    pub fn plant(self) -> JointedPlant {
        let mut p = match self {
            ShockPlant::Jointed6x3 => shock_structure(&SHOCK_6X3, SHOCK_6X3_REFERENCE_VELOCITY),
            _ => shock_structure(&SHOCK_4X4, SHOCK_4X4_REFERENCE_VELOCITY),
        };
        p.joint = self.joint();
        p
    }

    /// Build the plant with its name and geometry.
    pub fn build(self) -> NamedPlant {
        let layout = match self {
            ShockPlant::Jointed6x3 => &SHOCK_6X3,
            _ => &SHOCK_4X4,
        };
        let z = [0.0, 0.0, 1.0];
        let at = |p: &[f64; 2]| Placement {
            position_m: [p[0] * layout.half_width_m, p[1] * layout.half_width_m, 0.0],
            direction: z,
        };
        NamedPlant {
            name: self.name().to_string(),
            model: PlantModel::Jointed(self.plant()),
            geometry: Geometry {
                inputs: layout.controls.iter().map(at).collect(),
                drives: layout.drives.iter().map(at).collect(),
                reference_point_m: [0.0; 3],
            },
            conditioning: Conditioning::Well {
                band_hz: (20.0, 2000.0),
                max: 100.0,
            },
            rigid_body: None,
        }
    }
}

/// The reference pulse at `fs_hz` over `n` samples, scaled by `10^(level_db / 20)`: a
/// half-sine of [`REFERENCE_PULSE_G`] and [`REFERENCE_PULSE_S`] centred at a quarter of
/// the record, with a negative half-sine of a tenth of its height and five times its
/// duration on each side, so the pulse's velocity and displacement both return to zero
/// (the compensation a shaker needs to play it).
pub fn reference_pulse(fs_hz: f64, n: usize, level_db: f64) -> Vec<f64> {
    let s = 10f64.powf(level_db / 20.0);
    let t_main = REFERENCE_PULSE_S;
    let t_comp = 5.0 * t_main;
    let centre = 0.25 * n as f64 / fs_hz;
    let half_sine = |t: f64, start: f64, dur: f64, peak: f64| -> f64 {
        let x = (t - start) / dur;
        if (0.0..=1.0).contains(&x) {
            peak * (std::f64::consts::PI * x).sin()
        } else {
            0.0
        }
    };
    (0..n)
        .map(|k| {
            let t = k as f64 / fs_hz;
            let start = centre - 0.5 * t_main;
            s * (half_sine(t, start, t_main, REFERENCE_PULSE_G)
                + half_sine(t, start - t_comp, t_comp, -0.1 * REFERENCE_PULSE_G)
                + half_sine(t, start + t_main, t_comp, -0.1 * REFERENCE_PULSE_G))
        })
        .collect()
}

/// A shock structure's layout on a square plate, positions in half-widths.
struct ShockLayout {
    half_width_m: f64,
    drives: &'static [[f64; 2]],
    controls: &'static [[f64; 2]],
    /// `(f_hz, zeta)` per flexible mode, with the mode's shape index.
    modes: &'static [(f64, f64, usize)],
    /// Each control's delay, s.
    delays_s: &'static [f64],
    /// The mass line at a drive's own point, g per volt.
    mass_line: f64,
    /// Generalized force per volt over modal mass, relative to the mass line.
    participation: f64,
}

const SHOCK_4X4: ShockLayout = ShockLayout {
    half_width_m: 0.3,
    drives: &[[0.8, 0.8], [-0.8, 0.8], [-0.8, -0.8], [0.8, -0.8]],
    controls: &[[0.6, 0.75], [-0.75, 0.55], [-0.55, -0.7], [0.7, -0.6]],
    modes: &[
        (72.0, 0.02, 0),
        (135.0, 0.025, 1),
        (210.0, 0.02, 2),
        (330.0, 0.03, 3),
        (520.0, 0.025, 4),
        (780.0, 0.03, 5),
        (1150.0, 0.035, 6),
        (1650.0, 0.04, 7),
    ],
    delays_s: &[0.0, 0.12e-3, 0.31e-3, 0.5e-3],
    mass_line: 6.0,
    participation: 0.3,
};

const SHOCK_6X3: ShockLayout = ShockLayout {
    half_width_m: 0.3,
    drives: &[[0.8, 0.8], [-0.8, 0.4], [0.2, -0.8]],
    controls: &[
        [0.6, 0.7],
        [-0.7, 0.3],
        [0.1, -0.6],
        [0.0, 0.0],
        [-0.5, -0.6],
        [0.7, -0.2],
    ],
    modes: &[
        (64.0, 0.02, 0),
        (118.0, 0.025, 2),
        (190.0, 0.02, 1),
        (300.0, 0.03, 3),
        (470.0, 0.025, 5),
        (720.0, 0.03, 4),
        (1080.0, 0.035, 7),
        (1550.0, 0.04, 6),
    ],
    delays_s: &[0.0, 0.1e-3, 0.2e-3, 0.3e-3, 0.4e-3, 0.5e-3],
    mass_line: 6.0,
    participation: 0.3,
};

/// The plate's flexible shapes, `x` and `y` in half-widths.
fn plate_shape(k: usize, x: f64, y: f64) -> f64 {
    use std::f64::consts::PI;
    match k {
        0 => x * y,
        1 => x,
        2 => y,
        3 => x * x + y * y - 0.8,
        4 => (PI * x).cos() * y,
        5 => (PI * y).cos() * x,
        6 => (PI * x).cos() * (PI * y).cos(),
        _ => (0.5 * PI * x).sin() * (PI * y).cos(),
    }
}

/// A shock structure as a linear jointed plant (the caller sets the law).
fn shock_structure(l: &ShockLayout, reference_velocity: f64) -> JointedPlant {
    let (n_c, n_d) = (l.controls.len(), l.drives.len());
    let modes = l
        .modes
        .iter()
        .map(|&(f_hz, zeta, k)| JointedMode {
            f_hz,
            zeta,
            input_shape: l
                .drives
                .iter()
                .map(|p| l.participation * l.mass_line * plate_shape(k, p[0], p[1]))
                .collect(),
            output_shape: l
                .controls
                .iter()
                .map(|p| plate_shape(k, p[0], p[1]))
                .collect(),
            reference_amplitude: reference_velocity / (std::f64::consts::TAU * f_hz),
        })
        .collect();
    // The mass line: each control moves most with the drive nearest it.
    let residual = l
        .controls
        .iter()
        .flat_map(|c| {
            l.drives.iter().map(move |d| {
                let d2 = (c[0] - d[0]).powi(2) + (c[1] - d[1]).powi(2);
                l.mass_line * (-d2).exp()
            })
        })
        .collect();
    JointedPlant {
        residual,
        output_delays_s: l.delays_s.to_vec(),
        ..JointedPlant::new(n_c, n_d, modes)
    }
}
