//! Readings for several shakers, shown and recorded, not judged, and the
//! sweep-rate warning printed before a run is armed.

use crate::linalg::{mat_vec, norm, reg_pinv};
use crate::modal::ModalForm;
use crate::C64;

/// The opposition ratio: the drive vector's norm over the least norm that
/// gives the same response by the model, `||D|| / ||H^+ H D||` (`H` controls
/// by drives, row-major). 1 when no drive opposes another; it grows as drives
/// push against each other or spend effort the controls cannot see.
pub fn opposition_ratio(
    h: &[C64],
    controls: usize,
    drives: usize,
    d: &[C64],
) -> Result<f64, String> {
    let (hp, _) = reg_pinv(h, controls, drives, 0.0)?;
    let mut c = vec![C64::new(0.0, 0.0); controls];
    mat_vec(h, controls, drives, d, &mut c);
    let mut dmin = vec![C64::new(0.0, 0.0); drives];
    mat_vec(&hp, drives, controls, &c, &mut dmin);
    let least = norm(&dmin);
    Ok(if least > 0.0 { norm(d) / least } else { 1.0 })
}

/// The largest share of the total drive one shaker carries, `max |D_i|^2 /
/// sum |D_i|^2` (1 / drives when they share it evenly, 1 when one carries it
/// all), and which shaker.
pub fn largest_share(d: &[C64]) -> (f64, usize) {
    let total: f64 = d.iter().map(|v| v.norm_sqr()).sum();
    if total <= 0.0 {
        return (0.0, 0);
    }
    let mut best = (0.0, 0);
    for (i, v) in d.iter().enumerate() {
        let s = v.norm_sqr() / total;
        if s > best.0 {
            best = (s, i);
        }
    }
    best
}

/// One resonance's sweep-rate reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SweepRateReading {
    /// The resonance, Hz.
    pub f_hz: f64,
    /// Its quality factor, `1 / (2 zeta)`.
    pub q: f64,
    /// Seconds the tone spends inside its half-power bandwidth.
    pub seconds_in_bandwidth: f64,
    /// The mode's own time constants spent inside its half-power bandwidth,
    /// `272 f / (Q^2 S)`.
    pub time_constants: f64,
    /// Fewer than two: the response will not reach the steady value the laws
    /// work from, whatever the law.
    pub flagged: bool,
}

/// The sweep-rate warning: for each resonance found in the identification and
/// a logarithmic sweep at `oct_min` octaves a minute, the number of the
/// mode's own time constants (`Q / (pi f)`) the sweep spends inside its
/// half-power bandwidth (`f / Q`): `N = 60 pi f / (Q^2 S ln 2) = 272 f /
/// (Q^2 S)`, flagged under two.
pub fn sweep_rate_warning(modes: &ModalForm, oct_min: f64) -> Vec<SweepRateReading> {
    use std::f64::consts::{LN_2, PI};
    let s = oct_min.abs();
    modes
        .f_hz
        .iter()
        .zip(&modes.zeta)
        .map(|(&f, &z)| {
            let q = 1.0 / (2.0 * z);
            let (secs, n) = if s > 0.0 {
                (60.0 / (q * s * LN_2), 60.0 * PI * f / (q * q * s * LN_2))
            } else {
                (f64::INFINITY, f64::INFINITY)
            };
            SweepRateReading {
                f_hz: f,
                q,
                seconds_in_bandwidth: secs,
                time_constants: n,
                flagged: n < 2.0,
            }
        })
        .collect()
}
