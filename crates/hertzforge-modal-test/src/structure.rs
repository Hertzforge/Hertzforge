//! The simulated structure a Modal Test measures on the simulator, and the hammer that
//! hits it.
//!
//! # The structure
//!
//! Three modes, proportionally damped:
//!
//! | mode | natural frequency | damping ratio |
//! |------|-------------------|---------------|
//! | 1    | 42 Hz             | 0.040         |
//! | 2    | 118 Hz            | 0.030         |
//! | 3    | 196 Hz            | 0.025         |
//!
//! Node `n` sits at `x = ((n - 1) mod 10 + 0.5) / 10` along a beam, and mode `m` has
//! the shape `phi_m(n) = sin(m pi x)` there; a direction ending in `-` flips its sign.
//! The accelerance from a force at DOF `b` to the acceleration at DOF `a` is
//!
//! ```text
//!   H_ab(f) = S * sum_m phi_m(a) phi_m(b) (-w^2) / (w_m^2 - w^2 + 2 j zeta_m w_m w)
//! ```
//!
//! with `w = 2 pi f`, `w_m = 2 pi f_m` and `S` = [`SCALE_G_PER_LBF`] g/lbf, in the
//! textbook convention `X(f) = sum x(t) e^(-j 2 pi f t)`.
//!
//! # The hammer
//!
//! A half-sine pulse of [`PULSE_S`] and [`PULSE_PEAK_LBF`]. An overloaded hit has
//! [`OVERLOAD_FACTOR`] times the hammer row's range; a double hit adds a second pulse of
//! [`DOUBLE_HIT_RATIO`] of the first, [`DOUBLE_HIT_GAP_S`] after it.
//!
//! The structure's responses are played as a finite impulse response whose DFT on its
//! own grid is `H` exactly, so a measurement on any grid that divides it reads `H` at its
//! lines, short only of what a block cuts off the decay.

use hertzforge_sdm::C64;
use std::f64::consts::PI;

/// The modes: natural frequency (Hz) and damping ratio.
pub const MODES: [(f64, f64); 3] = [(42.0, 0.040), (118.0, 0.030), (196.0, 0.025)];
/// The accelerance's scale, g per lbf.
pub const SCALE_G_PER_LBF: f64 = 0.05;
/// The simulated hammer's pulse.
pub const PULSE_S: f64 = 0.004;
pub const PULSE_PEAK_LBF: f64 = 20.0;
/// An overloaded hit's peak over the hammer row's range.
pub const OVERLOAD_FACTOR: f64 = 1.2;
/// A double hit's second pulse, over the first, and its delay.
pub const DOUBLE_HIT_RATIO: f64 = 0.4;
pub const DOUBLE_HIT_GAP_S: f64 = 0.03;
/// A simulated shaker's force per drive volt, lbf/V.
pub const SHAKER_LBF_PER_V: f64 = 100.0;
/// The impulse response's length on the simulator, seconds.
pub const FIR_S: f64 = 1.0;

/// A DOF: a node and a direction (`Z+`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dof {
    pub node: u32,
    pub direction: String,
}

impl Dof {
    pub fn new(node: u32, direction: &str) -> Dof {
        Dof {
            node,
            direction: direction.to_owned(),
        }
    }
}

/// Mode `m`'s (from 1) shape at `dof`.
pub fn shape(m: usize, dof: &Dof) -> f64 {
    let x = (((dof.node.max(1) - 1) % 10) as f64 + 0.5) / 10.0;
    let s = (m as f64 * PI * x).sin();
    if dof.direction.ends_with('-') {
        -s
    } else {
        s
    }
}

/// The accelerance `H_ab(f)`, g/lbf.
pub fn accelerance(a: &Dof, b: &Dof, f_hz: f64) -> C64 {
    let w = 2.0 * PI * f_hz;
    let mut h = C64::new(0.0, 0.0);
    for (m, (fm, z)) in MODES.iter().enumerate() {
        let wm = 2.0 * PI * fm;
        let num = shape(m + 1, a) * shape(m + 1, b) * (-w * w);
        h += C64::new(num, 0.0) / C64::new(wm * wm - w * w, 2.0 * z * wm * w);
    }
    h * SCALE_G_PER_LBF
}

/// The impulse response from `b` to `a` at `fs`, `taps` long (a power of two): its DFT
/// on the `taps` grid is [`accelerance`] at those lines.
pub fn impulse_response(a: &Dof, b: &Dof, fs_hz: f64, taps: usize) -> Vec<f64> {
    let l = taps.next_power_of_two();
    let df = fs_hz / l as f64;
    // The crate's transforms take the positive exponent: their spectra are the
    // textbook ones conjugated, so the response's spectrum is conj(H) there.
    let mut spec = vec![(0.0, 0.0); l];
    for k in 0..=l / 2 {
        let h = accelerance(a, b, k as f64 * df).conj();
        let h = if k == l / 2 { C64::new(h.re, 0.0) } else { h };
        spec[k] = (h.re, h.im);
        if k > 0 && k < l / 2 {
            spec[l - k] = (h.re, -h.im);
        }
    }
    hertzforge_dsp::fft::ifft(&spec)
        .into_iter()
        .map(|c| c.0)
        .collect()
}

/// The linear convolution of `x` with `h`, `len` samples.
pub fn convolve(x: &[f64], h: &[f64], len: usize) -> Vec<f64> {
    let mut y = vec![0.0; len];
    for (i, xv) in x.iter().enumerate() {
        if *xv == 0.0 {
            continue;
        }
        for (j, hv) in h.iter().enumerate() {
            let n = i + j;
            if n >= len {
                break;
            }
            y[n] += xv * hv;
        }
    }
    y
}

/// The kind of a simulated hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    Clean,
    Double,
    Overload,
}

impl HitKind {
    pub fn name(self) -> &'static str {
        match self {
            HitKind::Clean => "clean",
            HitKind::Double => "double",
            HitKind::Overload => "overload",
        }
    }

    pub fn from_name(s: &str) -> Option<HitKind> {
        match s {
            "clean" => Some(HitKind::Clean),
            "double" => Some(HitKind::Double),
            "overload" => Some(HitKind::Overload),
            _ => None,
        }
    }
}

/// A simulated hit's force, `len` samples, the pulse starting at sample `at`.
/// `peak_lbf` scales a clean pulse (the default is [`PULSE_PEAK_LBF`]).
pub fn hammer(
    kind: HitKind,
    fs_hz: f64,
    len: usize,
    at: usize,
    peak_lbf: f64,
    range_lbf: f64,
) -> Vec<f64> {
    let mut f = vec![0.0; len];
    let w = ((PULSE_S * fs_hz).round() as usize).max(2);
    let peak = match kind {
        HitKind::Overload => OVERLOAD_FACTOR * range_lbf,
        _ => peak_lbf,
    };
    let mut put = |start: usize, p: f64| {
        for k in 0..w {
            if let Some(v) = f.get_mut(start + k) {
                *v += p * (PI * (k as f64 + 0.5) / w as f64).sin();
            }
        }
    };
    put(at, peak);
    if kind == HitKind::Double {
        put(
            at + (DOUBLE_HIT_GAP_S * fs_hz).round() as usize,
            DOUBLE_HIT_RATIO * peak,
        );
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_test_structure_shapes_vary_with_node() {
        let a = Dof::new(1, "Z+");
        let b = Dof::new(5, "Z+");
        assert_ne!(shape(1, &a), shape(1, &b));
        assert_eq!(shape(2, &Dof::new(3, "Z-")), -shape(2, &Dof::new(3, "Z+")));
        // Near each mode the magnitude peaks.
        let d = Dof::new(2, "Z+");
        for (fm, _) in MODES {
            let at = accelerance(&d, &d, fm).norm();
            assert!(at > accelerance(&d, &d, fm * 0.8).norm());
            assert!(at > accelerance(&d, &d, fm * 1.2).norm());
        }
    }

    #[test]
    fn modal_test_impulse_response_reads_the_accelerance_on_its_grid() {
        let (a, b) = (Dof::new(2, "Z+"), Dof::new(7, "Z+"));
        let fs = 1024.0;
        let h = impulse_response(&a, &b, fs, 1024);
        let s = hertzforge_dsp::fft::rfft(&h);
        for k in [10usize, 42, 118, 196, 300] {
            let got = C64::new(s[k].0, -s[k].1);
            let want = accelerance(&a, &b, k as f64);
            assert!((got - want).norm() < 1e-9 * (1.0 + want.norm()), "line {k}");
        }
    }
}
