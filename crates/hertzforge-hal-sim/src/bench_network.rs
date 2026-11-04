//! A passive cross-coupled network for a multiple-output bench: four voltage
//! outputs into four voltage inputs, every input seeing every output, from
//! standard E12 parts. Its frequency response is computed here from the
//! component values by nodal analysis, so the network can be run on the
//! simulator as an [`FrfPlant`] before it is wired, and a bench's
//! identification can be compared with the computed matrix after.
//!
//! # The circuit
//!
//! Each output `j` feeds a node `P_j` through a feed resistor (`R_feed`, with a
//! series capacitor `C_series` on the path that has one). From each `P_j` a
//! shunt goes to ground: a capacitor `C_shunt` (a low-pass corner), or, on the
//! mode path, a resistor `R_base` in series with a resonant tank (an inductor
//! with its winding resistance, in parallel with a capacitor and a damping
//! resistor). Every `P_j` then feeds every input node `IN_i` through a summing
//! resistor `R_sum[i][j]`; each input node has a resistor `R_ground` to ground,
//! the input's own impedance `R_input`, and the cable's capacitance `C_input`.
//!
//! ```text
//!  AO_j --[C_series]--[R_feed]--+-- P_j --[R_sum i,j]--+-- IN_i --> input i
//!                               |                      |
//!                         C_shunt, or                R_ground || R_input || C_input
//!                    R_base + (L+DCR || C_tank || R_q)  |
//!                               |                      |
//!                              GND                    GND
//! ```
//!
//! The analysis is exact for the lumped circuit (ideal sources, the parts as
//! given). The converters' own responses (the output's zero-order hold, the
//! input's delay and filter) are not part of it; a bench comparison accounts
//! for them separately.

// Index loops mirror the nodal equations they build.
#![allow(clippy::needless_range_loop)]

use crate::plant::{FrfPlant, PlantModel};
use crate::plants::{Conditioning, Geometry, NamedPlant, Placement};
use hertzforge_sdm::{MatrixStack, C64};
use std::f64::consts::TAU;

/// The resonant shunt on one path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tank {
    /// The output whose node carries the tank.
    pub drive: usize,
    /// The resistor in series with the tank, ohms.
    pub r_base: f64,
    /// The inductance, henries.
    pub l: f64,
    /// The inductor's winding (DC) resistance, ohms.
    pub dcr: f64,
    /// The tank's capacitance, farads.
    pub c: f64,
    /// The damping resistor across the tank, ohms.
    pub r_q: f64,
}

impl Tank {
    /// The tank's own impedance at `f` Hz.
    pub fn impedance(&self, f: f64) -> C64 {
        let w = TAU * f;
        let y = (C64::new(self.dcr, w * self.l)).inv() + C64::new(1.0 / self.r_q, w * self.c);
        y.inv()
    }

    /// The tank's undamped resonance, Hz.
    pub fn f0(&self) -> f64 {
        1.0 / (TAU * (self.l * self.c).sqrt())
    }
}

/// The network's component values. [`BenchNetwork::nominal`] is the bench's.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchNetwork {
    /// Feed resistor per output, ohms.
    pub r_feed: [f64; 4],
    /// Series capacitor per output (a high-pass corner), farads.
    pub c_series: [Option<f64>; 4],
    /// Shunt capacitor at each output's node (a low-pass corner), farads.
    pub c_shunt: [Option<f64>; 4],
    /// The resonant shunt on one path.
    pub tank: Tank,
    /// Summing resistor from output node `j` to input `i`: `r_sum[i][j]`, ohms.
    pub r_sum: [[f64; 4]; 4],
    /// Resistor from each input node to ground, ohms.
    pub r_ground: [f64; 4],
    /// The input's own impedance, ohms.
    pub r_input: f64,
    /// Capacitance at each input node (cable and input), farads.
    pub c_input: [f64; 4],
}

/// The summing resistors, ohms: input `i` takes output `j` through
/// `SUM_VALUES[(j - i) mod 4]`, so each input is strongest from its own output
/// and every output node carries the same load.
pub const SUM_VALUES: [f64; 4] = [100e3, 220e3, 330e3, 470e3];

impl BenchNetwork {
    /// The bench network (values in the bench plan).
    pub fn nominal() -> BenchNetwork {
        let mut r_sum = [[0.0; 4]; 4];
        for (i, row) in r_sum.iter_mut().enumerate() {
            for (j, r) in row.iter_mut().enumerate() {
                *r = SUM_VALUES[(j + 4 - i) % 4];
            }
        }
        BenchNetwork {
            r_feed: [22e3; 4],
            c_series: [None, None, None, Some(150e-9)],
            c_shunt: [Some(33e-9), Some(10e-9), Some(4.7e-9), None],
            tank: Tank {
                drive: 3,
                r_base: 2.2e3,
                l: 10e-3,
                dcr: 5.0,
                c: 220e-9,
                r_q: 6.8e3,
            },
            r_sum,
            r_ground: [47e3; 4],
            r_input: 305e3,
            c_input: [100e-12; 4],
        }
    }

    /// The admittance of output `j`'s feed (series capacitor and resistor).
    fn y_feed(&self, j: usize, w: f64) -> C64 {
        let mut z = C64::new(self.r_feed[j], 0.0);
        if let Some(c) = self.c_series[j] {
            z += C64::new(0.0, -1.0 / (w * c));
        }
        z.inv()
    }

    /// The admittance from output node `j` to ground (its corner capacitor or
    /// the tank branch).
    fn y_shunt(&self, j: usize, f: f64) -> C64 {
        let w = TAU * f;
        let mut y = C64::new(0.0, 0.0);
        if let Some(c) = self.c_shunt[j] {
            y += C64::new(0.0, w * c);
        }
        if self.tank.drive == j {
            y += (C64::new(self.tank.r_base, 0.0) + self.tank.impedance(f)).inv();
        }
        y
    }

    /// The node voltages for 1 V on output `j` (every other output at 0 V):
    /// `[P_0..P_3, IN_0..IN_3]`.
    pub fn nodes_for(&self, j: usize, f: f64) -> [C64; 8] {
        let w = TAU * f.max(1e-9);
        let zero = C64::new(0.0, 0.0);
        let mut y = [[zero; 8]; 8];
        let mut rhs = [zero; 8];
        for k in 0..4 {
            // Output node k: its feed (to a source), its shunt, the summing resistors.
            y[k][k] += self.y_feed(k, w) + self.y_shunt(k, f);
            for i in 0..4 {
                let g = C64::new(1.0 / self.r_sum[i][k], 0.0);
                y[k][k] += g;
                y[4 + i][4 + i] += g;
                y[k][4 + i] -= g;
                y[4 + i][k] -= g;
            }
        }
        for i in 0..4 {
            y[4 + i][4 + i] += C64::new(
                1.0 / self.r_ground[i] + 1.0 / self.r_input,
                w * self.c_input[i],
            );
        }
        rhs[j] = self.y_feed(j, w);
        solve8(y, rhs)
    }

    /// The 4 x 4 transfer matrix at `f` Hz: element `[i][j]` is input `i`'s
    /// voltage per volt on output `j`.
    pub fn frf_at(&self, f: f64) -> [[C64; 4]; 4] {
        let mut h = [[C64::new(0.0, 0.0); 4]; 4];
        for j in 0..4 {
            let v = self.nodes_for(j, f);
            for i in 0..4 {
                h[i][j] = v[4 + i];
            }
        }
        h
    }

    /// The transfer matrix on lines `0, df, 2 df, ...`: `lines x 4 x 4`.
    pub fn frf_stack(&self, lines: usize, df_hz: f64) -> MatrixStack {
        let mut s = MatrixStack::zeros(lines, 4, 4);
        for line in 0..lines {
            let h = self.frf_at(line as f64 * df_hz);
            for (i, row) in h.iter().enumerate() {
                for (j, v) in row.iter().enumerate() {
                    s.set(line, i, j, *v);
                }
            }
        }
        s
    }

    /// The network as the simulator's plant: its computed FRF on `lines` lines
    /// at `df_hz`, played as an [`FrfPlant`].
    pub fn plant(&self, lines: usize, df_hz: f64) -> NamedPlant {
        let place = |k: usize| Placement {
            position_m: [k as f64 * 0.1, 0.0, 0.0],
            direction: [0.0, 0.0, 1.0],
        };
        NamedPlant {
            name: "bench-network-4x4".into(),
            model: PlantModel::Frf(FrfPlant::new(self.frf_stack(lines, df_hz), df_hz)),
            geometry: Geometry {
                inputs: (0..4).map(place).collect(),
                drives: (0..4).map(place).collect(),
                reference_point_m: [0.0; 3],
            },
            conditioning: Conditioning::Well {
                band_hz: BAND_HZ,
                max: CONDITION_CLAIM,
            },
            rigid_body: None,
        }
    }

    /// The largest current any output can be asked for, amps, with every output
    /// at `v_peak` volts in any phase: for each output, `|Y_feed| (v + sum over
    /// outputs k of |P_j per volt on k| v)`, the largest over `freqs`.
    pub fn max_output_current(&self, v_peak: f64, freqs: &[f64]) -> f64 {
        let mut most = 0.0f64;
        for &f in freqs {
            let w = TAU * f;
            let per: Vec<[C64; 8]> = (0..4).map(|k| self.nodes_for(k, f)).collect();
            for j in 0..4 {
                let node: f64 = (0..4).map(|k| per[k][j].norm()).sum();
                most = most.max(self.y_feed(j, w).norm() * (1.0 + node) * v_peak);
            }
        }
        most
    }

    /// The largest voltage any input can see, volts, with every output at
    /// `v_peak` in any phase: `sum over j of max over freqs |H_ij|`, times
    /// `v_peak`, the worst input.
    pub fn max_input_voltage(&self, v_peak: f64, freqs: &[f64]) -> f64 {
        let mut peak = [[0.0f64; 4]; 4];
        for &f in freqs {
            let h = self.frf_at(f);
            for i in 0..4 {
                for j in 0..4 {
                    peak[i][j] = peak[i][j].max(h[i][j].norm());
                }
            }
        }
        peak.iter()
            .map(|row| row.iter().sum::<f64>() * v_peak)
            .fold(0.0, f64::max)
    }

    /// The loaded tank's resonance and quality factor: the peak of the tank's
    /// impedance in parallel with what loads it (the base resistor plus the
    /// node's Thevenin resistance back through the feed and the summing
    /// network), and its half-power width.
    pub fn tank_resonance(&self) -> (f64, f64) {
        let t = self.tank;
        let j = t.drive;
        // The resistance the node sees, the tank branch excluded: the feed and
        // the summing resistors into the inputs' own loads.
        let g_in: Vec<f64> = (0..4)
            .map(|i| {
                1.0 / self.r_ground[i]
                    + 1.0 / self.r_input
                    + (0..4).map(|k| 1.0 / self.r_sum[i][k]).sum::<f64>()
            })
            .collect();
        let g_sum: f64 = (0..4)
            .map(|i| 1.0 / (self.r_sum[i][j] + 1.0 / (g_in[i] - 1.0 / self.r_sum[i][j])))
            .sum();
        let r_node = 1.0 / (1.0 / self.r_feed[j] + g_sum);
        let r_load = t.r_base + r_node;
        let loaded = |f: f64| {
            let z = t.impedance(f);
            (z.inv() + C64::new(1.0 / r_load, 0.0)).inv().norm()
        };
        // Peak by a fine sweep around the undamped resonance.
        let f0 = t.f0();
        let (mut fp, mut zp) = (f0, 0.0f64);
        let n = 20_000;
        for k in 0..=n {
            let f = f0 * (0.5 + k as f64 / n as f64);
            let z = loaded(f);
            if z > zp {
                zp = z;
                fp = f;
            }
        }
        let half = zp / std::f64::consts::SQRT_2;
        let edge = |dir: f64| {
            let mut f = fp;
            while loaded(f) > half && f > 1.0 {
                f *= 1.0 + dir * 1e-5;
            }
            f
        };
        let (lo, hi) = (edge(-1.0), edge(1.0));
        (fp, fp / (hi - lo))
    }

    /// Each plain path's corner, Hz: the frequency where output `j`'s node is
    /// 3 dB below its value at the corner's flat side (low for a low-pass, high
    /// for a high-pass). `None` on a path with neither capacitor.
    pub fn corner_hz(&self, j: usize) -> Option<f64> {
        let mag = |f: f64| self.nodes_for(j, f)[j].norm();
        let (flat, rising) = if self.c_series[j].is_some() {
            (mag(BAND_HZ.1 * 0.5), true)
        } else if self.c_shunt[j].is_some() {
            (mag(1.0), false)
        } else {
            return None;
        };
        let target = flat / std::f64::consts::SQRT_2;
        let (mut lo, mut hi) = (1.0f64, 50_000.0f64);
        for _ in 0..200 {
            let mid = (lo * hi).sqrt();
            let below = mag(mid) < target;
            if below == rising {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some((lo * hi).sqrt())
    }
}

/// The band the network is designed for, Hz.
pub const BAND_HZ: (f64, f64) = (20.0, 5000.0);

/// The condition number the network's FRF stays under in [`BAND_HZ`] (the
/// tests hold it to this).
pub const CONDITION_CLAIM: f64 = 100.0;

/// Solve the 8 x 8 complex system `y v = rhs` by Gaussian elimination with
/// partial pivoting.
fn solve8(mut a: [[C64; 8]; 8], mut b: [C64; 8]) -> [C64; 8] {
    for col in 0..8 {
        let p = (col..8)
            .max_by(|&x, &y| a[x][col].norm().total_cmp(&a[y][col].norm()))
            .unwrap_or(col);
        a.swap(col, p);
        b.swap(col, p);
        let d = a[col][col];
        for r in col + 1..8 {
            let m = a[r][col] / d;
            if m == C64::new(0.0, 0.0) {
                continue;
            }
            for c in col..8 {
                let v = a[col][c];
                a[r][c] -= m * v;
            }
            let v = b[col];
            b[r] -= m * v;
        }
    }
    let mut x = [C64::new(0.0, 0.0); 8];
    for r in (0..8).rev() {
        let mut s = b[r];
        for c in r + 1..8 {
            s -= a[r][c] * x[c];
        }
        x[r] = s / a[r][r];
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band_freqs(n: usize) -> Vec<f64> {
        let (lo, hi) = BAND_HZ;
        (0..n)
            .map(|k| lo * (hi / lo).powf(k as f64 / (n - 1) as f64))
            .collect()
    }

    #[test]
    fn the_solver_matches_a_hand_divider() {
        // One output, nothing but its feed and the summing network: at DC with
        // no capacitors, input i's voltage is the resistive divider.
        let mut n = BenchNetwork::nominal();
        n.c_series = [None; 4];
        n.c_shunt = [None; 4];
        n.tank.r_base = 1e15;
        n.c_input = [0.0; 4];
        let h = n.frf_at(0.0);
        // Superposition: the inputs for all four outputs at 1 V together are
        // the sum of the columns, and by symmetry every node then sits at the
        // same voltage: v = g_feed / (g_feed + g_load) at each P, where g_load
        // is the P node's conductance into the inputs, which are each at
        // v_in = (sum g_sum) v / (sum g_sum + g_ground + g_input).
        let g_sum: f64 = SUM_VALUES.iter().map(|r| 1.0 / r).sum();
        let g_gnd = 1.0 / 47e3 + 1.0 / 305e3;
        let k = g_sum / (g_sum + g_gnd);
        let g_feed = 1.0 / 22e3;
        let v_p = g_feed / (g_feed + g_sum * (1.0 - k));
        let v_in = k * v_p;
        for row in &h {
            let s: C64 = row.iter().sum();
            assert!(
                (s.re - v_in).abs() < 1e-9 && s.im.abs() < 1e-9,
                "{s} vs {v_in}"
            );
        }
    }

    #[test]
    fn every_input_sees_every_output_and_the_matrix_is_well_conditioned() {
        let n = BenchNetwork::nominal();
        let freqs = band_freqs(400);
        let mut worst_cond = 0.0f64;
        let mut weakest = f64::INFINITY;
        for &f in &freqs {
            let h = n.frf_at(f);
            let mut s = MatrixStack::zeros(1, 4, 4);
            for i in 0..4 {
                for j in 0..4 {
                    s.set(0, i, j, h[i][j]);
                    weakest = weakest.min(h[i][j].norm());
                }
            }
            let c = hertzforge_sdm::linalg::condition_numbers(&s).unwrap()[0];
            worst_cond = worst_cond.max(c);
        }
        eprintln!(
            "worst condition {worst_cond:.1}, weakest path {:.1} dB",
            20.0 * weakest.log10()
        );
        assert!(worst_cond < CONDITION_CLAIM, "{worst_cond}");
        assert!(20.0 * weakest.log10() > -70.0, "{weakest}");
    }

    #[test]
    fn no_input_exceeds_5_v_and_no_output_1_ma_at_full_output() {
        let n = BenchNetwork::nominal();
        let freqs = band_freqs(400);
        let v_in = n.max_input_voltage(10.0, &freqs);
        let i_out = n.max_output_current(10.0, &freqs);
        eprintln!(
            "worst input {v_in:.3} V, worst output current {:.3} mA",
            i_out * 1e3
        );
        assert!(v_in < 4.5, "{v_in}");
        assert!(i_out < 1.0e-3, "{i_out}");
    }

    #[test]
    fn the_corners_are_distinct_and_the_mode_has_a_q_of_10_to_20_in_band() {
        let n = BenchNetwork::nominal();
        let corners: Vec<f64> = (0..4).filter_map(|j| n.corner_hz(j)).collect();
        let (f0, q) = n.tank_resonance();
        eprintln!("corners {corners:.1?} Hz; mode {f0:.1} Hz, Q {q:.2}");
        assert_eq!(corners.len(), 4);
        for (a, c) in corners.iter().enumerate() {
            assert!(*c > BAND_HZ.0 && *c < BAND_HZ.1, "{c}");
            for d in &corners[a + 1..] {
                assert!((c / d).log2().abs() > 0.5, "{c} and {d} too close");
            }
            assert!(
                (c / f0).log2().abs() > 0.3,
                "corner {c} too close to the mode {f0}"
            );
        }
        assert!(f0 > BAND_HZ.0 && f0 < BAND_HZ.1);
        assert!((10.0..=20.0).contains(&q), "{q}");
    }

    #[test]
    fn the_simulators_frf_plant_plays_the_computed_matrix() {
        // The bench grid: 12.8 kHz, 4096 points, 3.125 Hz lines.
        let (fs, nfft) = (12_800.0, 4096usize);
        let (lines, df) = (nfft / 2 + 1, fs / nfft as f64);
        let n = BenchNetwork::nominal();
        let p = n.plant(lines, df);
        assert_eq!(p.model.dims(), Some((4, 4)));
        let played = p.model.analytic_frf(lines, df, fs);
        let want = n.frf_stack(lines, df);
        // What the plant plays, less its partition latency (a pure delay), is the
        // network within a hundredth of a dB and a tenth of a degree in band.
        let block = match &p.model {
            PlantModel::Frf(f) => f.block_frames.min(f.max_taps),
            _ => unreachable!(),
        };
        let lo = (BAND_HZ.0 / df).ceil() as usize;
        let hi = (BAND_HZ.1 / df).floor() as usize;
        let (mut db, mut deg) = (0.0f64, 0.0f64);
        for line in lo..=hi {
            let f = line as f64 * df;
            let undelay = C64::from_polar(1.0, TAU * f * block as f64 / fs);
            for i in 0..4 {
                for j in 0..4 {
                    let r = played.get(line, i, j) * undelay / want.get(line, i, j);
                    db = db.max((20.0 * r.norm().log10()).abs());
                    deg = deg.max(r.arg().to_degrees().abs());
                }
            }
        }
        eprintln!("FRF plant against the network: {db:.4} dB, {deg:.4} deg");
        assert!(db < 0.01 && deg < 0.1, "{db} dB, {deg} deg");
    }
}
