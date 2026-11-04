//! Six rigid-body degrees of freedom from sensor geometry.
//!
//! Each control sensor has a position `p` and a sensing direction `n` (normalized
//! here). With the reference point `o` and `r = p - o`, a rigid body with
//! translational acceleration `a_0` and angular acceleration `alpha` at the
//! reference point gives the sensor the reading
//!
//! ```text
//! a_sensor = n . a_0 + alpha . (r x n)
//! ```
//!
//! (the centripetal term is second order in vibration amplitude and is dropped), so
//! sensor `i` contributes the row `[ n^T, (r x n)^T ]` of the kinematic matrix `A`,
//! `n_c` by 6, its columns the degrees of freedom `X, Y, Z, RX, RY, RZ`
//! ([`DOF_LABELS`]). The response transformation is the pseudo-inverse
//! `T_r = A^+` (6 by `n_c`), the least-squares fit when there are more than six
//! sensors. Fewer than six sensors, or a geometry of rank below six (all sensors
//! on a line, all directions parallel, and so on), is refused with the rank named.
//!
//! **Units.** Translations are in the sensors' unit (for example `g` or `m/s^2`).
//! Positions are in a stated length unit, and rotations come out in the sensors'
//! unit per that length unit (for example `g/m`: the reading a sensor at unit
//! distance would see). The condition number of `A`, and so the least-squares
//! weighting between translations and rotations, depends on that length unit;
//! [`RigidBodyFit::condition`] reports it.
//!
//! **Residuals.** A sensor on a part that does not move rigidly is caught by its
//! residual against the fit: [`RigidBodyFit::residuals`] for one set of readings,
//! [`RigidBodyFit::residual_spectrum`] for a measured spectral matrix.
//!
//! **Mapping a reference.** A reference written in the six degrees of freedom maps
//! to the sensors as `G_ff = A G_6 A^H` (rank at most six, so singular when there
//! are more than six sensors: control in the degree-of-freedom space instead), and
//! sensor measurements map back as `G_6 = T_r G_ff T_r^H`.
//!
//! Sources: the kinematic matrix for triaxial gauges in Jacobs et al. (2016); the
//! general sensor-direction row and the residual follow from rigid-body
//! kinematics.

use crate::{Method, Reference, ReferenceError, Transformation, C64};
use hertzforge_sdm::linalg::{congruence, pinv, Regularization};
use hertzforge_sdm::{HermitianStack, MatrixStack};
use serde::{Deserialize, Serialize};

/// The six rigid-body degrees of freedom, in column order.
pub const DOF_LABELS: [&str; 6] = ["X", "Y", "Z", "RX", "RY", "RZ"];

/// A residual above this fraction of the readings' RMS flags a sensor.
pub const RESIDUAL_THRESHOLD: f64 = 0.1;

/// One control sensor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sensor {
    pub label: String,
    /// Position, in the geometry's length unit.
    pub position: [f64; 3],
    /// Sensing direction (any length; normalized when used).
    pub direction: [f64; 3],
}

/// The sensors, the reference point and the units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigidBodyGeometry {
    pub sensors: Vec<Sensor>,
    /// The point the six degrees of freedom are written at.
    pub reference_point: [f64; 3],
    /// The sensors' unit, for example `g`.
    pub sensor_unit: String,
    /// The unit of the positions, for example `m`.
    pub length_unit: String,
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

impl RigidBodyGeometry {
    /// The kinematic matrix `A`, one row per sensor.
    pub fn kinematic_matrix(&self) -> Result<Vec<[f64; 6]>, ReferenceError> {
        let mut rows = Vec::with_capacity(self.sensors.len());
        for s in &self.sensors {
            let finite = s
                .position
                .iter()
                .chain(&s.direction)
                .chain(&self.reference_point)
                .all(|v| v.is_finite());
            let len = s.direction.iter().map(|v| v * v).sum::<f64>().sqrt();
            if !finite || len == 0.0 {
                return Err(ReferenceError::Geometry(format!(
                    "sensor {}: its position and direction must be numbers and its direction \
                     must not be zero",
                    s.label
                )));
            }
            let n = [
                s.direction[0] / len,
                s.direction[1] / len,
                s.direction[2] / len,
            ];
            let r = [
                s.position[0] - self.reference_point[0],
                s.position[1] - self.reference_point[1],
                s.position[2] - self.reference_point[2],
            ];
            let m = cross(r, n);
            rows.push([n[0], n[1], n[2], m[0], m[1], m[2]]);
        }
        Ok(rows)
    }

    /// The fit: `A`, `T_r = A^+`, its rank and condition. Refuses fewer than six
    /// sensors and a geometry of rank below six.
    pub fn fit(&self) -> Result<RigidBodyFit, ReferenceError> {
        let n_c = self.sensors.len();
        let labels: Vec<String> = self.sensors.iter().map(|s| s.label.clone()).collect();
        crate::check_unique(&labels, "sensors")?;
        if n_c < 6 {
            return Err(ReferenceError::Geometry(format!(
                "{n_c} sensors cannot resolve the six rigid-body degrees of freedom; give at \
                 least six, spread over the body and not all in one direction"
            )));
        }
        let rows = self.kinematic_matrix()?;
        let data = rows
            .iter()
            .flat_map(|r| r.iter().map(|&v| C64::new(v, 0.0)))
            .collect();
        let a = MatrixStack::from_vec(1, n_c, 6, data)?;
        let (t, report) =
            pinv(&a, Regularization::None).map_err(|e| ReferenceError::Geometry(e.to_string()))?;
        let rank = report.rank[0];
        if rank < 6 {
            return Err(ReferenceError::Geometry(format!(
                "the sensors resolve only {rank} of the six rigid-body degrees of freedom (the \
                 kinematic matrix has rank {rank}); add sensors in other directions or off the \
                 line or plane the others share"
            )));
        }
        let u = &self.sensor_unit;
        let l = &self.length_unit;
        Ok(RigidBodyFit {
            kinematic: a,
            transformation: Transformation {
                matrix: t,
                row_labels: DOF_LABELS.iter().map(|s| s.to_string()).collect(),
                col_labels: labels,
            },
            singular_values: report.singular_values,
            condition: report.cond_overall[0],
            dof_units: [
                u.clone(),
                u.clone(),
                u.clone(),
                format!("{u}/{l}"),
                format!("{u}/{l}"),
                format!("{u}/{l}"),
            ],
            length_unit: l.clone(),
        })
    }
}

/// One sensor's residual against the rigid-body fit.
///
/// The residual is standardized: the reading minus the fit, divided by
/// `sqrt(P_ii)` where `P = I - A T_r` is the residual projector. A single sensor
/// that departs from the rigid motion then always has the largest standardized
/// residual (the departure leaks into the others' raw residuals, never more than
/// into its own standardized one), so the sensor flagged first is the one to
/// inspect. A sensor the fit depends on entirely (`P_ii` zero, for example the
/// only sensor in a direction) cannot be checked and reports zero.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SensorResidual {
    pub sensor: String,
    /// The standardized residual, in the sensor unit (from a spectral matrix: the
    /// standardized residual PSD, in the unit squared per hertz, at the worst line).
    pub residual: f64,
    /// The residual relative to the readings' RMS (from a spectral matrix: to the
    /// mean PSD of the sensors at the same line).
    pub relative: f64,
    /// True above [`RESIDUAL_THRESHOLD`].
    pub flagged: bool,
}

/// The rigid-body fit of a sensor geometry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigidBodyFit {
    /// `A`, one line, `n_c` by 6.
    pub kinematic: MatrixStack,
    /// `T_r = A^+`, 6 by `n_c`, rows [`DOF_LABELS`], columns the sensors.
    pub transformation: Transformation,
    /// Singular values of `A`, descending.
    pub singular_values: Vec<f64>,
    /// Condition number of `A` (depends on the length unit).
    pub condition: f64,
    /// The unit of each degree of freedom.
    pub dof_units: [String; 6],
    pub length_unit: String,
}

impl RigidBodyFit {
    fn n_c(&self) -> usize {
        self.kinematic.rows()
    }

    /// The six degrees of freedom that best fit one set of sensor readings
    /// (real or complex, one per sensor, in sensor order).
    pub fn dofs(&self, readings: &[C64]) -> Result<[C64; 6], ReferenceError> {
        if readings.len() != self.n_c() {
            return Err(ReferenceError::Input(format!(
                "{} readings for {} sensors; give one per sensor",
                readings.len(),
                self.n_c()
            )));
        }
        let mut q = [C64::new(0.0, 0.0); 6];
        for (k, qk) in q.iter_mut().enumerate() {
            for (i, &y) in readings.iter().enumerate() {
                *qk += self.transformation.matrix.get(0, k, i) * y;
            }
        }
        Ok(q)
    }

    /// The residual projector `P = I - A T_r`, one line.
    fn residual_projector(&self) -> MatrixStack {
        let n = self.n_c();
        let mut p = MatrixStack::identity(1, n);
        for i in 0..n {
            for j in 0..n {
                let at: C64 = (0..6)
                    .map(|k| self.kinematic.get(0, i, k) * self.transformation.matrix.get(0, k, j))
                    .sum();
                p.set(0, i, j, p.get(0, i, j) - at);
            }
        }
        p
    }

    /// Each sensor's standardized residual against the rigid-body fit of
    /// `readings`.
    pub fn residuals(&self, readings: &[C64]) -> Result<Vec<SensorResidual>, ReferenceError> {
        let q = self.dofs(readings)?;
        let n = self.n_c();
        let p = self.residual_projector();
        let rms = (readings.iter().map(|y| y.norm_sqr()).sum::<f64>() / n as f64).sqrt();
        Ok((0..n)
            .map(|i| {
                let fit: C64 = (0..6).map(|k| self.kinematic.get(0, i, k) * q[k]).sum();
                let pii = p.get(0, i, i).re;
                let residual = if pii > 1e-12 {
                    (readings[i] - fit).norm() / pii.sqrt()
                } else {
                    0.0
                };
                let relative = if rms > 0.0 { residual / rms } else { 0.0 };
                SensorResidual {
                    sensor: self.transformation.col_labels[i].clone(),
                    residual,
                    relative,
                    flagged: relative > RESIDUAL_THRESHOLD,
                }
            })
            .collect())
    }

    /// Each sensor's standardized residual in a measured spectral matrix of the
    /// sensors: the diagonal of `P G P^H` over `P_ii`, against the sensors' mean
    /// PSD on the same line, at the line where that ratio is worst.
    pub fn residual_spectrum(
        &self,
        g: &HermitianStack,
    ) -> Result<Vec<SensorResidual>, ReferenceError> {
        if g.n() != self.n_c() {
            return Err(ReferenceError::Input(format!(
                "the spectral matrix is {} by {} for {} sensors",
                g.n(),
                g.n(),
                self.n_c()
            )));
        }
        let p = self.residual_projector();
        let r = congruence(&p, g).map_err(|e| ReferenceError::Input(e.to_string()))?;
        let n = g.n();
        let means: Vec<f64> = (0..g.lines()).map(|l| g.trace(l) / n as f64).collect();
        Ok((0..n)
            .map(|i| {
                let pii = p.get(0, i, i).re;
                let mut worst = (0.0f64, 0.0f64);
                for (line, &mean) in means.iter().enumerate() {
                    if pii <= 1e-12 || mean <= 0.0 {
                        continue;
                    }
                    let res = r.get(line, i, i).re.max(0.0) / pii;
                    let rel = res / mean;
                    if rel > worst.1 {
                        worst = (res, rel);
                    }
                }
                SensorResidual {
                    sensor: self.transformation.col_labels[i].clone(),
                    residual: worst.0,
                    relative: worst.1,
                    flagged: worst.1 > RESIDUAL_THRESHOLD,
                }
            })
            .collect())
    }

    /// A degree-of-freedom spectral matrix (6 by 6) at the sensors,
    /// `A G_6 A^H`.
    pub fn to_physical(&self, g_dof: &HermitianStack) -> Result<HermitianStack, ReferenceError> {
        congruence(&self.kinematic, g_dof).map_err(|e| ReferenceError::Input(e.to_string()))
    }

    /// A sensor spectral matrix in the six degrees of freedom, `T_r G T_r^H`.
    pub fn to_dof(&self, g_phys: &HermitianStack) -> Result<HermitianStack, ReferenceError> {
        congruence(&self.transformation.matrix, g_phys)
            .map_err(|e| ReferenceError::Input(e.to_string()))
    }

    /// Attach the rigid-body response transformation to a reference written in the
    /// six degrees of freedom (labels [`DOF_LABELS`], in order), and record it in
    /// the provenance.
    pub fn attach(&self, reference: Reference) -> Result<Reference, ReferenceError> {
        if reference.labels.iter().map(String::as_str).ne(DOF_LABELS) {
            return Err(ReferenceError::Labels(format!(
                "a rigid-body reference is written in the degrees of freedom {} in that order; \
                 this one is labeled {}",
                DOF_LABELS.join(", "),
                reference.labels.join(", ")
            )));
        }
        let mut r = reference;
        r.provenance.assumptions.push(format!(
            "rigid-body response transformation over {} sensors, condition {:.4}; translations \
             in {}, rotations in {}",
            self.n_c(),
            self.condition,
            self.dof_units[0],
            self.dof_units[3]
        ));
        let source = std::mem::take(&mut r.provenance.source);
        r.provenance.source = format!("{source}; geometry of {} sensors", self.n_c());
        r.provenance.method = Method::RigidBody;
        r.with_response_transformation(self.transformation.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builders::{BreakpointReference, Grid, PairValue};
    use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};

    fn c(re: f64) -> C64 {
        C64::new(re, 0.0)
    }

    /// Four triaxial gauges at the corners of a 0.4 m by 0.3 m plate, one a little
    /// raised.
    fn plate() -> RigidBodyGeometry {
        let corners = [
            [0.0, 0.0, 0.0],
            [0.4, 0.0, 0.0],
            [0.4, 0.3, 0.02],
            [0.0, 0.3, 0.0],
        ];
        let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let mut sensors = Vec::new();
        for (k, p) in corners.iter().enumerate() {
            for (a, d) in axes.iter().enumerate() {
                sensors.push(Sensor {
                    label: format!("{}{}", k + 1, ["X", "Y", "Z"][a]),
                    position: *p,
                    direction: *d,
                });
            }
        }
        RigidBodyGeometry {
            sensors,
            reference_point: [0.2, 0.15, 0.0],
            sensor_unit: "g".into(),
            length_unit: "m".into(),
        }
    }

    fn rigid_readings(fit: &RigidBodyFit, q: [f64; 6]) -> Vec<C64> {
        (0..fit.n_c())
            .map(|i| (0..6).map(|k| fit.kinematic.get(0, i, k) * q[k]).sum())
            .collect()
    }

    #[test]
    fn the_kinematic_row_is_direction_then_moment_arm_cross_direction() {
        let g = RigidBodyGeometry {
            sensors: vec![Sensor {
                label: "s".into(),
                position: [1.0, 2.0, 3.0],
                direction: [0.0, 0.0, 2.0],
            }],
            reference_point: [1.0, 0.0, 0.0],
            sensor_unit: "g".into(),
            length_unit: "m".into(),
        };
        // r = (0, 2, 3), n = (0, 0, 1): r x n = (2, 0, 0).
        assert_eq!(
            g.kinematic_matrix().unwrap()[0],
            [0.0, 0.0, 1.0, 2.0, 0.0, 0.0]
        );
    }

    #[test]
    fn twelve_sensors_fit_six_dofs_and_a_rigid_motion_has_no_residual() {
        let fit = plate().fit().unwrap();
        assert_eq!(fit.transformation.matrix.dims(), (1, 6, 12));
        assert!(fit.condition.is_finite() && fit.condition > 1.0);
        assert_eq!(fit.dof_units[3], "g/m");
        // T_r A = I.
        for i in 0..6 {
            for j in 0..6 {
                let v: C64 = (0..12)
                    .map(|k| fit.transformation.matrix.get(0, i, k) * fit.kinematic.get(0, k, j))
                    .sum();
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((v - c(want)).norm() < 1e-12, "({i},{j}) {v}");
            }
        }
        let q = [1.0, -2.0, 0.5, 3.0, -1.5, 0.25];
        let y = rigid_readings(&fit, q);
        let back = fit.dofs(&y).unwrap();
        for k in 0..6 {
            assert!((back[k] - c(q[k])).norm() < 1e-12);
        }
        let res = fit.residuals(&y).unwrap();
        assert!(res.iter().all(|r| r.relative < 1e-12 && !r.flagged));
    }

    #[test]
    fn a_sensor_on_a_flexible_part_is_caught_by_its_residual() {
        let fit = plate().fit().unwrap();
        let mut y = rigid_readings(&fit, [1.0, -2.0, 0.5, 3.0, -1.5, 0.25]);
        // Sensor 3Z sits on a panel that bends: it reads 1 g more than the rigid body.
        let bent = 8;
        y[bent] += c(1.0);
        let res = fit.residuals(&y).unwrap();
        let worst = res
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.relative.total_cmp(&b.1.relative))
            .unwrap();
        assert_eq!(worst.0, bent);
        assert_eq!(worst.1.sensor, "3Z");
        assert!(worst.1.flagged, "{:?}", worst.1);

        // The same in a spectral matrix: rigid DOF motion plus an independent
        // flexible contribution at that sensor.
        let lines = 4;
        let mut g6 = HermitianStack::zeros(lines, 6);
        for line in 0..lines {
            for k in 0..6 {
                g6.set(line, k, k, c(1.0 + k as f64));
            }
        }
        let mut gphys = fit.to_physical(&g6).unwrap();
        let clean = fit.residual_spectrum(&gphys).unwrap();
        assert!(clean.iter().all(|r| r.relative < 1e-10), "{clean:?}");
        for line in 0..lines {
            let d = gphys.get(line, bent, bent).re;
            gphys.set(line, bent, bent, c(d * 3.0));
        }
        let res = fit.residual_spectrum(&gphys).unwrap();
        let worst = res
            .iter()
            .max_by(|a, b| a.relative.total_cmp(&b.relative))
            .unwrap();
        assert_eq!(worst.sensor, "3Z");
        assert!(worst.flagged);
    }

    #[test]
    fn too_few_sensors_or_a_degenerate_geometry_is_refused_with_the_rank() {
        let mut g = plate();
        g.sensors.truncate(5);
        let e = g.fit().unwrap_err();
        assert!(e.to_string().contains("5 sensors"), "{e}");

        // Every sensor along Z: Z, RX and RY only.
        let mut g = plate();
        for s in &mut g.sensors {
            s.direction = [0.0, 0.0, 1.0];
        }
        g.sensors.dedup_by(|a, b| a.position == b.position);
        let mut extra = g.sensors.clone();
        for (k, s) in extra.iter_mut().enumerate() {
            s.label = format!("z{k}");
            s.position[0] += 0.05;
        }
        g.sensors.extend(extra);
        let e = g.fit().unwrap_err();
        assert!(e.to_string().contains("rank 3"), "{e}");
    }

    #[test]
    fn dof_and_physical_spectral_matrices_map_both_ways() {
        let fit = plate().fit().unwrap();
        // A positive definite 6 by 6 with cross terms.
        let mut g6 = HermitianStack::zeros(2, 6);
        for line in 0..2 {
            for i in 0..6 {
                g6.set(line, i, i, c(2.0 + i as f64));
                for j in i + 1..6 {
                    g6.set(line, i, j, C64::new(0.1 * (i + j) as f64, 0.05 * j as f64));
                }
            }
        }
        let phys = fit.to_physical(&g6).unwrap();
        assert_eq!(phys.n(), 12);
        let back = fit.to_dof(&phys).unwrap();
        for line in 0..2 {
            for i in 0..6 {
                for j in 0..6 {
                    assert!((back.get(line, i, j) - g6.get(line, i, j)).norm() < 1e-10);
                }
            }
        }
    }

    #[test]
    fn a_dof_reference_takes_the_rigid_body_transformation() {
        let fit = plate().fit().unwrap();
        let mut table = BreakpointTable::new("dof");
        table.push(BreakpointRow::psd(20.0, 0.01));
        table.push(BreakpointRow::psd(500.0, 0.01));
        let labels: Vec<String> = DOF_LABELS.iter().map(|s| s.to_string()).collect();
        let b = BreakpointReference::new(Grid::new(2.0, 512).unwrap(), labels, vec![table; 6])
            .pair(0, 4, PairValue::Constant(0.5), PairValue::Constant(10.0));
        let (dof_ref, _) = b.build(None).unwrap();
        let r = fit.attach(dof_ref.clone()).unwrap();
        assert!(r.is_virtual());
        assert_eq!(r.provenance.method, Method::RigidBody);
        assert_eq!(
            r.response_transformation.as_ref().unwrap().col_labels[0],
            "1X"
        );
        assert!(r.validate().unwrap().response_condition.unwrap() > 1.0);

        let mut wrong = dof_ref;
        wrong.labels.swap(0, 1);
        assert!(matches!(fit.attach(wrong), Err(ReferenceError::Labels(_))));
    }
}
