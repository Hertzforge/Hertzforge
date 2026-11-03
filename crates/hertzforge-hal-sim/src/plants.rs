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
