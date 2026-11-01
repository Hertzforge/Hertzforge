//! WAVSYN wavelet synthesis.
//!
//! # The wavelet (Smallwood WAVSYN)
//!
//! A single WAVSYN wavelet is a carrier sinusoid at `f_m` amplitude-modulated by a
//! half-cycle of a lower-frequency envelope sinusoid:
//!
//! ```text
//!   W_m(t) = A_m * sin(2*pi*f_m*t / N_m) * sin(2*pi*f_m*t),   t in [0, N_m/(2*f_m)]
//! ```
//!
//! The envelope `sin(2*pi*f_m*t/N_m)` completes exactly its first half-lobe
//! (`0 -> pi`) over the support `t in [0, N_m/(2*f_m)]`, so the wavelet is a compact,
//! finitely-supported burst of `N_m` half-cycles of the carrier under a single
//! sine-squared-like hump.
//!
//! # Zero net velocity AND displacement BY CONSTRUCTION (gate i)
//!
//! When `N_m` is ODD (`>= 3`) the wavelet integrates to zero net velocity and zero net
//! displacement over its support - the self-nulling property WAVSYN is prized for,
//! because it means the synthesized drive needs NO compensation to keep the shaker
//! centered (contrast SEDS, [`crate::seds`], which does). This was bit-verified:
//! the single-wavelet residual velocity and displacement are ~1e-15 through the pure
//! double-integrator [`eigen_dsp::shock::pulse_kinematics`] (see
//! `eigen-dsp/tests/shock.rs::pulse_kinematics_odd_half_cycle_wavelet_self_nulls`). A
//! delay is a pure time shift and preserves the property, and a SUM of self-nulling
//! signals is self-nulling, so the composite (a delayed sum of odd wavelets) is
//! self-nulling too - asserted in this crate's gate (i) to `< 1e-9` relative.
//!
//! # Reverse-sine-sweep delays
//!
//! The default component delays (assigned by
//! [`crate::table::WaveletTable::default_over_band`]) place the highest-frequency
//! component at zero delay and progressively delay lower-frequency components, tiling
//! their lobes from high frequency to low - a reverse (downward) sine sweep. This
//! module honors whatever `delay_s` each component carries; the delay does not affect
//! self-nulling.

use crate::table::{WaveletComponent, WaveletTable};

/// Synthesize a single WAVSYN wavelet component onto a full record of `out.len()`
/// samples at `fs_hz`, placed at its `delay_s`. Samples outside the component's active
/// lobe are left unchanged (the component is ADDED, so several components sum into one
/// composite).
///
/// # Grid-exact self-nulling (why the composite self-nulls to machine precision)
///
/// The component's start is SNAPPED to the nearest sample (`i0 = round(delay_s*fs)`)
/// and the lobe spans exactly `M = round(support*fs)` samples. On that grid the shape
/// is evaluated with the phase normalized by `M` rather than by absolute time:
///
/// ```text
///   envelope[k] = sin(pi * k / M),   carrier[k] = sin(N * pi * k / M),   k = 0..=M
/// ```
///
/// so at `k = 0` both vanish and at `k = M` the envelope is `sin(pi) = 0` and the
/// carrier is `sin(N*pi) = 0` EXACTLY (integer multiples of pi). Because BOTH the
/// envelope half-lobe (0 -> pi) and the carrier's `N` half-cycles (0 -> N*pi) complete
/// exactly on the discrete grid, the wavelet is an odd function about its midpoint
/// sampled symmetrically, and its discrete trapezoidal double-integral self-nulls to
/// floating-point round-off (~1e-15) regardless of frequency - the property gate (i)
/// asserts for the composite. This is the standard DISCRETE WAVSYN construction
/// (matching the eigen-dsp pulse generators' `phase = k/(M)` normalization); the
/// realized carrier frequency is `N / (2 * M * dt)`, which differs from `freq_hz` only
/// by the rounding of `M` to an integer sample count (a fraction of one bin, negligible
/// for the SRS match and re-trimmed by the iteration). Sampling instead on absolute
/// time `tau = k/fs` would truncate the final partial carrier half-cycle whenever
/// `support*fs` is not an integer and leave an `O(1/fs^2)` residual - the reason the
/// grid-normalized phase is used here.
///
/// Snapping the START to a sample (`i0`) is also the physically correct model - a
/// sample-clocked DAC can only begin a waveform on a sample boundary - and it keeps the
/// per-component local grid identical (an integer sample offset), so summing several
/// delayed components preserves each one's self-nulling.
pub fn wavsyn_component_into(component: &WaveletComponent, fs_hz: f64, out: &mut [f64]) {
    if fs_hz <= 0.0 || out.is_empty() {
        return;
    }
    let a = component.amplitude_g;
    let nn = component.half_cycles as f64;
    let support = component.support_s();

    // Snap the start to the nearest sample (integer offset -> preserves self-nulling
    // when several delayed components are summed).
    let i0 = (component.delay_s * fs_hz).round() as usize;
    // Lobe sample span (>= 1). The shape is evaluated on the grid-normalized phase so
    // both the envelope and the carrier close exactly on the last sample (see doc).
    let m = ((support * fs_hz).round() as usize).max(1);
    let pi = std::f64::consts::PI;

    for k in 0..=m {
        let idx = i0 + k;
        if idx >= out.len() {
            break;
        }
        let phase = k as f64 / m as f64; // 0 .. 1
        let envelope = (pi * phase).sin();
        let carrier = (nn * pi * phase).sin();
        out[idx] += a * envelope * carrier;
    }
}

/// Synthesize a single WAVSYN wavelet component as a fresh record of `n` samples at
/// `fs_hz`. Convenience wrapper over [`wavsyn_component_into`] for a zeroed buffer.
pub fn wavsyn_component(component: &WaveletComponent, fs_hz: f64, n: usize) -> Vec<f64> {
    let mut out = vec![0.0f64; n];
    wavsyn_component_into(component, fs_hz, &mut out);
    out
}

/// Synthesize the composite WAVSYN waveform for a whole table: the sample-for-sample
/// sum of every component placed at its delay, plus a quiet tail so any (tiny) residual
/// motion can develop and be measured.
///
/// # Record length
///
/// The record spans from `t = 0` to the latest component lobe end
/// (`max_m (delay_m + support_m)`), plus `tail_s` seconds of quiet tail. The tail lets
/// the pure double-integrator [`eigen_dsp::shock::pulse_kinematics`] reveal any residual
/// velocity as a displacement ramp; for a correctly-constructed odd-wavelet table the
/// residual is ~0 and the tail stays flat (gate i). Pass a tail of at least a few of
/// the lowest component periods.
///
/// Returns `(waveform_g, fs_hz)` echo is unnecessary; just the waveform (the caller
/// holds `fs_hz`). An empty table yields an empty vector.
pub fn wavsyn_composite(table: &WaveletTable, fs_hz: f64, tail_s: f64) -> Vec<f64> {
    if fs_hz <= 0.0 || table.components.is_empty() {
        return Vec::new();
    }
    let latest_end = table
        .components
        .iter()
        .map(|c| c.end_time_s())
        .fold(0.0f64, f64::max);
    let total_s = latest_end + tail_s.max(0.0);
    let n = ((total_s * fs_hz).ceil() as usize).max(1);
    let mut out = vec![0.0f64; n];
    for c in &table.components {
        wavsyn_component_into(c, fs_hz, &mut out);
    }
    out
}
