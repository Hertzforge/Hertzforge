//! Print the standard sine structures: each one's modes (frequency, `Q`), its full level,
//! the drives that level needs by the small-signal response, its conditioning over the
//! band, and for the level-dependent ones the movement measured at full level.
//!
//! Run: `cargo run --release -p hertzforge-hal-sim --example sine_structures`.

use hertzforge_hal_sim::plants::{SineStructure, SINE_BAND_HZ, SINE_CALIBRATION_RATE_HZ};
use hertzforge_hal_sim::{JointedPlant, C64};
use hertzforge_sdm::linalg;
use std::f64::consts::TAU;

const FS: f64 = SINE_CALIBRATION_RATE_HZ;

/// Log-spaced frequencies over the band.
fn band_grid(n: usize) -> Vec<f64> {
    let (lo, hi) = SINE_BAND_HZ;
    (0..n)
        .map(|k| lo * (hi / lo).powf(k as f64 / (n - 1) as f64))
        .collect()
}

/// The complex steady response of a one-by-one structure to a sine at `f_hz` whose
/// modal amplitude is held at `amplitude` (the drive corrected every decay time), as
/// output over drive.
fn held_response(p: &JointedPlant, f_hz: f64, amplitude: f64) -> C64 {
    let m = &p.modes[0];
    let (w, w0) = (TAU * f_hz, TAU * m.f_hz);
    let mut st = p.stepper(FS).expect("a structure");
    let mut u = amplitude * C64::new(w0 * w0 - w * w, 2.0 * m.zeta * w0 * w).norm();
    let window = ((1.0 / (m.zeta * w0) * FS).max(4.0 * FS / f_hz)).round() as usize;
    let mut out = [0.0];
    let mut k = 0usize;
    let mut fit = |st: &mut hertzforge_hal_sim::Stepper, k: &mut usize, len: usize, u: f64| {
        let mut acc = C64::new(0.0, 0.0);
        for _ in 0..len {
            let ph = w * *k as f64 / FS;
            st.step(&[u * ph.cos()], &mut out);
            acc += C64::new(ph.cos(), -ph.sin()) * out[0];
            *k += 1;
        }
        acc * (2.0 / len as f64)
    };
    for _ in 0..400 {
        let y = fit(&mut st, &mut k, window, u);
        let err = amplitude / (y.norm() / (w * w));
        if (err - 1.0).abs() < 1e-3 {
            break;
        }
        u *= err.powf(0.4);
    }
    let cycles = (20.0 * FS / f_hz).round() as usize;
    fit(&mut st, &mut k, cycles, u) / u
}

/// `(peak Hz, half-power damping)` of a one-by-one structure held at `amplitude`.
fn held_resonance(p: &JointedPlant, amplitude: f64) -> (f64, f64) {
    let f0 = p.modes[0].f_hz;
    let z0 = p.modes[0].zeta;
    let gain = |f: f64| held_response(p, f, amplitude).norm();
    let g = 0.5 * (5f64.sqrt() - 1.0);
    let (mut a, mut b) = (f0 * (1.0 - 6.0 * z0), f0 * (1.0 + 6.0 * z0));
    let (mut c, mut d) = (b - g * (b - a), a + g * (b - a));
    let (mut gc, mut gd) = (gain(c), gain(d));
    while b - a > 1e-5 * f0 {
        if gc > gd {
            (b, d, gd) = (d, c, gc);
            c = b - g * (b - a);
            gc = gain(c);
        } else {
            (a, c, gc) = (c, d, gd);
            d = a + g * (b - a);
            gd = gain(d);
        }
    }
    let fp = 0.5 * (a + b);
    let half = gain(fp) / 2f64.sqrt();
    let edge = |mut inside: f64, mut outside: f64| {
        for _ in 0..30 {
            let mid = 0.5 * (inside + outside);
            if gain(mid) > half {
                inside = mid;
            } else {
                outside = mid;
            }
        }
        0.5 * (inside + outside)
    };
    let f1 = edge(fp, fp * (1.0 - 10.0 * z0));
    let f2 = edge(fp, fp * (1.0 + 10.0 * z0));
    (fp, (f2 - f1) / (2.0 * fp))
}

fn main() {
    let grid = band_grid(2000);
    // Names on the command line print only those structures.
    let only: Vec<String> = std::env::args().skip(1).collect();
    for s in SineStructure::ALL {
        if !only.is_empty() && !only.iter().any(|n| n == s.name()) {
            continue;
        }
        let p = s.structure();
        let (n_out, n_d) = s.dims();
        println!(
            "{}  ({} outputs x {} drives, {} controls)",
            s.name(),
            n_out,
            n_d,
            s.n_controls()
        );
        println!("  {}", s.description());
        println!(
            "  full level {} g peak, seed {:#018x}",
            s.full_level_g(),
            s.seed()
        );
        // For each mode, how far past its reference amplitude the full level takes it
        // when the controls' phases are the ones that excite it most (the calibration
        // is for the controls in phase): `sum |g_i| / |sum g_i|`, `g` the mode's force
        // per unit reference at each control.
        let lin = s.linear_structure();
        let nc = s.n_controls();
        println!("  modes (f Hz, Q, reference amplitude, worst-phase ratio at full level):");
        for m in &p.modes {
            let h = lin.frf_at_envelope(&[m.f_hz], FS, &vec![0.0; lin.modes.len()]);
            let mut hc = hertzforge_hal_sim::MatrixStack::zeros(1, nc, n_d);
            for i in 0..nc {
                for j in 0..n_d {
                    hc.set(0, i, j, h.get(0, i, j));
                }
            }
            let (inv, _) = linalg::pinv(&hc, linalg::Regularization::None).expect("pinv");
            let g: Vec<C64> = (0..nc)
                .map(|i| {
                    (0..n_d).fold(C64::new(0.0, 0.0), |acc, j| {
                        acc + inv.get(0, j, i) * m.input_shape[j]
                    })
                })
                .collect();
            let sum_abs: f64 = g.iter().map(|v| v.norm()).sum();
            let abs_sum = g.iter().fold(C64::new(0.0, 0.0), |a, v| a + v).norm();
            println!(
                "    {:7.1}  {:5.1}  {:.3e}  {:6.1}",
                m.f_hz,
                1.0 / (2.0 * m.zeta),
                m.reference_amplitude,
                sum_abs / abs_sum
            );
        }
        print!("  full-level drive, largest |d| V:");
        for f in [5.0, 20.0, 40.0, 100.0, 300.0, 1000.0, 2000.0] {
            let d = s.full_level_drive(f);
            let mx = d.iter().map(|v| v.norm()).fold(0.0, f64::max);
            print!(" {f} Hz {mx:.3}");
        }
        println!();
        // Over the band: the largest drive and the singular values of the controlled rows.
        let h = lin.frf_at_envelope(&grid, FS, &vec![0.0; lin.modes.len()]);
        let mut hc = hertzforge_hal_sim::MatrixStack::zeros(grid.len(), nc, n_d);
        for k in 0..grid.len() {
            for i in 0..nc {
                for j in 0..n_d {
                    hc.set(k, i, j, h.get(k, i, j));
                }
            }
        }
        let svd = linalg::svd(&hc).expect("svd");
        let kk = svd.k();
        let (mut worst, mut at) = (0.0f64, 0.0);
        for (k, &f) in grid.iter().enumerate() {
            let c = svd.s[k * kk] / svd.s[k * kk + kk - 1];
            if c > worst {
                (worst, at) = (c, f);
            }
        }
        let mut dmax = (0.0f64, 0.0);
        for &f in grid.iter().step_by(4) {
            let d = s.full_level_drive(f);
            let mx = d.iter().map(|v| v.norm()).fold(0.0, f64::max);
            if mx > dmax.0 {
                dmax = (mx, f);
            }
        }
        println!(
            "  over the band: largest drive {:.3} V at {:.1} Hz; condition number at most {:.1} ({:.1} dB) at {:.1} Hz",
            dmax.0,
            dmax.1,
            worst,
            20.0 * worst.log10(),
            at
        );
        if s == SineStructure::BaseArticle6x4 {
            let a = p.modes.last().expect("the article mode").f_hz;
            let fine: Vec<f64> = (0..400)
                .map(|k| a * (0.9 + 0.2 * k as f64 / 399.0))
                .collect();
            let mut best = (0.0f64, 0.0);
            for &f in &fine {
                let hh = lin.frf_at_envelope(&[f], FS, &vec![0.0; lin.modes.len()]);
                let d = s.full_level_drive(f);
                for i in nc..n_out {
                    let y =
                        (0..n_d).fold(C64::new(0.0, 0.0), |acc, j| acc + hh.get(0, i, j) * d[j]);
                    let r = y.norm() / s.full_level_g();
                    if r > best.0 {
                        best = (r, f);
                    }
                }
            }
            println!(
                "  article: with the base held at full level, the article reads {:.2} times the base at {:.1} Hz",
                best.0, best.1
            );
        }
        if let Some(d) = s.level_design() {
            println!("  measured at full level (each mode alone, its modal amplitude held):");
            for &k in d.modes {
                let one = p.single_mode(k);
                let r = one.modes[0].reference_amplitude;
                let (f_lo, z_lo) = held_resonance(&one, 1e-3 * r);
                let (f_hi, z_hi) = held_resonance(&one, r);
                let bw = one.modes[0].f_hz * 2.0 * one.modes[0].zeta;
                println!(
                    "    mode {k} at {:.1} Hz: low level {:.3} Hz, full level {:.3} Hz, moved {:+.3} half-power bandwidths, damping x {:.3}",
                    one.modes[0].f_hz,
                    f_lo,
                    f_hi,
                    (f_hi - f_lo) / bw,
                    z_hi / z_lo
                );
            }
        }
        println!();
    }
}
