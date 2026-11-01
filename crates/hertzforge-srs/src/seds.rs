//! SEDS synthesis (sum of exponentially decaying sinusoids) with Smallwood-Nord-style
//! compensation. This is the hardest math item in the SRS synthesis layer.
//!
//! # The uncompensated SEDS composite
//!
//! Each component is an exponentially-decaying sinusoid
//!
//! ```text
//!   a_m(t) = polarity_m * A_m * exp(-zeta_m*w_m*(t-t_m)) * sin(w_m*(t-t_m)),  t >= t_m
//! ```
//!
//! (`w_m = 2*pi*f_m`), zero before its delay `t_m`. The composite is the sum over
//! components (Nelson & Prasthofer 1974, "Response of an elastic system to a decaying
//! sinusoidal base excitation", and the SEDS synthesis line of work). Damped sinusoids
//! match an SRS efficiently, but a raw damped sine does NOT self-null: its net velocity
//! change `int_0^inf a dt = A*w/((zeta*w)^2 + w^2) = A/(w*(1+zeta^2))` is nonzero, and
//! its net displacement diverges. A shaker driven by the uncompensated composite would
//! drift off center and eventually hit a stroke or velocity limit (gate ii asserts this
//! residual is NONZERO).
//!
//! # Smallwood-Nord compensation (gate ii, the fixture)
//!
//! Smallwood & Nord 1974 ("Matching Shock Spectra with Sums of Decaying Sinusoids
//! Compensated for Shaker Velocity and Displacement Limitations", 45th Shock &
//! Vibration Symposium) solve this by superposing a small set of LOW-FREQUENCY
//! compensating components whose amplitudes are chosen so the COMPOSITE has zero net
//! velocity AND zero net displacement, keeping the shaker centered.
//!
//! ## The construction chosen here (documented)
//!
//! We append a PAIR of low-frequency compensating decaying-sinusoid components at a
//! frequency `f_c` well below the synthesis band (default: half the lowest synthesis
//! frequency). Two compensators give two free amplitudes, which is exactly the number
//! of linear constraints (zero net velocity, zero net displacement), so the
//! amplitudes are the unique solution of a 2x2 linear system. Using two decaying
//! sinusoids (rather than one wavelet) keeps the compensator in the same waveform
//! family as the synthesis - a low-frequency ring-down that the shaker realizes
//! naturally - and the two are made linearly independent in the (velocity,
//! displacement) plane by giving them DIFFERENT decays (a fast and a slow ring-down),
//! so the 2x2 system is well conditioned.
//!
//! ## Numerically self-consistent (why the residuals go to ~0, not just analytically)
//!
//! Rather than cancel the ANALYTIC moments (which would leave the numerical
//! trapezoidal residual nonzero at the sampled record), the compensator amplitudes are
//! solved against the moments computed by the SAME trapezoidal integration that
//! [`hertzforge_dsp::shock::pulse_kinematics`] uses to VERIFY them. The zeroth moment
//! `V = sum_i trap(a)` is the discrete net velocity; the "displacement moment"
//! `D = T*V - sum_i t_i * trap-weight * a_i` is the discrete residual displacement
//! (from the double-integral identity `d(T) = int_0^T (T - tau) a(tau) dtau`). Solving
//! the discrete system makes the discrete residuals cancel to floating-point round-off,
//! so the verification through the pure integrator asserts `< 1e-6 in/s` residual
//! velocity and `< 1e-8 in` residual displacement (gate ii).
//!
//! # References
//!
//! - Smallwood, D. O.; Nord, A. R. (1974). "Matching Shock Spectra with Sums of
//!   Decaying Sinusoids Compensated for Shaker Velocity and Displacement Limitations."
//!   The Shock and Vibration Bulletin 44.
//! - Nelson, D. B.; Prasthofer, P. H. (1974). "A Case for Damped Oscillatory
//!   Excitation as a Natural Pyrotechnic Shock Simulation." The Shock and Vibration
//!   Bulletin 44.

use crate::table::{DampedSineComponent, DampedSineTable, Polarity};
use hertzforge_dsp::integrate::G_TO_IN_S2;

/// The default quiet-tail length used when the caller does not specify one: the tail
/// must be long enough for the pure double-integrator to reveal residual motion. Two of
/// the lowest-frequency component's periods is comfortably enough for the compensated
/// composite to have rung down.
const DEFAULT_TAIL_PERIODS: f64 = 4.0;

/// How the compensating pair's frequency is chosen relative to the lowest synthesis
/// frequency: `f_c = LOW_FREQ_COMP_FRAC * f_lowest`. Placing the compensator BELOW the
/// synthesis band keeps its SRS contribution inside the band negligible (it perturbs
/// the match only at the very low end, which the iteration then re-trims), while giving
/// it the large velocity/displacement moments a compensator needs to cancel the
/// composite's residuals with a modest amplitude.
const LOW_FREQ_COMP_FRAC: f64 = 0.5;

/// The two decays of the compensating pair. They must DIFFER so the pair spans the
/// (velocity, displacement) plane (the 2x2 solve is otherwise singular). A light and a
/// heavier decay at the same low frequency give well-conditioned, distinct moments.
const COMP_ZETA_A: f64 = 0.05;
const COMP_ZETA_B: f64 = 0.30;

/// The compensation applied to a SEDS composite (Smallwood & Nord 1974). Holds the two
/// low-frequency compensating components appended to the table and a report on how well
/// the composite residuals were nulled.
#[derive(Debug, Clone, PartialEq)]
pub struct SedsCompensation {
    /// The two low-frequency compensating components (a fast and a slow ring-down at
    /// `f_c`), with the solved amplitudes.
    pub compensators: [DampedSineComponent; 2],
    /// The residuals report (before vs after compensation) through the discrete
    /// trapezoidal moments.
    pub report: CompensationReport,
}

/// The residuals a [`SedsCompensation`] cancelled and the residuals it left, in the
/// in/g system (velocity in in/s, displacement in inches) - the SAME quantities
/// [`hertzforge_dsp::shock::pulse_kinematics`] reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompensationReport {
    /// Uncompensated composite net (residual) velocity, in/s. Nonzero (gate ii).
    pub raw_residual_velocity_ips: f64,
    /// Uncompensated composite net (residual) displacement, inches. Nonzero.
    pub raw_residual_disp_in: f64,
    /// Compensated composite net (residual) velocity, in/s. ~0 (gate ii: `< 1e-6`).
    pub comp_residual_velocity_ips: f64,
    /// Compensated composite net (residual) displacement, inches. ~0 (gate ii: `< 1e-8`).
    pub comp_residual_disp_in: f64,
}

/// A synthesized SEDS result: the compensated waveform, the sample rate, the record
/// length, and the compensation report.
#[derive(Debug, Clone, PartialEq)]
pub struct SedsSynthResult {
    /// The compensated composite acceleration waveform, g.
    pub waveform_g: Vec<f64>,
    /// Sample rate the waveform was synthesized at, Hz.
    pub fs_hz: f64,
    /// The Smallwood-Nord compensation that was applied.
    pub compensation: SedsCompensation,
}

/// Synthesize a single damped-sine component onto a full record of `out.len()` samples
/// at `fs_hz`, superposed (added) at its `delay_s`. Samples before the delay are
/// unchanged.
///
/// The shape is `polarity * A * exp(-zeta*w*tau) * sin(w*tau)` for local time
/// `tau = t - delay_s >= 0`, `w = 2*pi*f`.
pub fn seds_component_into(component: &DampedSineComponent, fs_hz: f64, out: &mut [f64]) {
    if fs_hz <= 0.0 {
        return;
    }
    let a = component.amplitude_g * component.polarity.sign();
    let w = component.omega();
    let sigma = component.zeta * w;
    let t0 = component.delay_s;
    for (i, slot) in out.iter_mut().enumerate() {
        let t = i as f64 / fs_hz;
        if t < t0 {
            continue;
        }
        let tau = t - t0;
        *slot += a * (-sigma * tau).exp() * (w * tau).sin();
    }
}

/// The number of samples spanning the RAW (uncompensated) SEDS composite over its
/// active support plus a quiet tail. The support is taken as the point at which the
/// slowest component has decayed to a small fraction of its peak, bounded by a sane
/// minimum; the tail then lets residual motion develop.
fn raw_record_samples(table: &DampedSineTable, fs_hz: f64) -> usize {
    // The lowest frequency sets both the tail length and the slowest decay envelope.
    let f_lowest = table
        .components
        .iter()
        .map(|c| c.freq_hz)
        .fold(f64::INFINITY, f64::min);
    // Active support: the longest (slowest) ring-down to ~1e-4 of peak plus the largest
    // delay. exp(-zeta*w*t) = 1e-4  =>  t = ln(1e4)/(zeta*w).
    let mut active_end = 0.0f64;
    for c in &table.components {
        let sigma = c.zeta * c.omega();
        let ring = if sigma > 0.0 {
            (1e4f64).ln() / sigma
        } else {
            0.0
        };
        active_end = active_end.max(c.delay_s + ring);
    }
    let period_lowest = if f_lowest.is_finite() && f_lowest > 0.0 {
        1.0 / f_lowest
    } else {
        0.0
    };
    let total_s = active_end + DEFAULT_TAIL_PERIODS * period_lowest;
    ((total_s * fs_hz).ceil() as usize).max(2)
}

/// Synthesize the RAW (uncompensated) SEDS composite for a table onto a record of `n`
/// samples at `fs_hz`: the sample-for-sample sum of every component. This does NOT
/// self-null; its residuals are nonzero (gate ii). Use [`seds_compensated`] for the
/// compensated composite.
pub fn seds_composite_raw(table: &DampedSineTable, fs_hz: f64, n: usize) -> Vec<f64> {
    let mut out = vec![0.0f64; n];
    for c in &table.components {
        seds_component_into(c, fs_hz, &mut out);
    }
    out
}

/// Discrete trapezoidal moments of a sampled acceleration record (g) at `fs_hz`, in the
/// in/g system:
/// - `V` = net velocity change (in/s), `int a dt` by the trapezoidal rule, EXACTLY the
///   `residual_velocity_ips` [`hertzforge_dsp::shock::pulse_kinematics`] computes;
/// - `D` = net displacement change (inches), the trapezoidal double-integral end value,
///   EXACTLY the `residual_disp_in` `pulse_kinematics` computes.
///
/// Returning both from one double-integration pass (rather than the moment identity)
/// guarantees the solved compensation cancels the SAME numbers the verifier will read,
/// so the compensated residuals go to round-off (gate ii).
fn trap_residuals(accel_g: &[f64], fs_hz: f64) -> (f64, f64) {
    let n = accel_g.len();
    if n < 2 || fs_hz <= 0.0 {
        return (0.0, 0.0);
    }
    let dt = 1.0 / fs_hz;
    let mut v = 0.0f64; // running velocity, in/s
    let mut d = 0.0f64; // running displacement, in
    let mut v_prev = 0.0f64;
    let mut a_prev = accel_g[0] * G_TO_IN_S2;
    for &g in &accel_g[1..] {
        let a_cur = g * G_TO_IN_S2;
        let v_cur = v + 0.5 * (a_prev + a_cur) * dt;
        d += 0.5 * (v_prev + v_cur) * dt;
        v = v_cur;
        v_prev = v_cur;
        a_prev = a_cur;
    }
    (v, d)
}

/// Compute the Smallwood-Nord compensation for a SEDS table: solve the amplitudes of a
/// low-frequency compensating PAIR so the compensated composite has zero net velocity
/// AND zero net displacement. Returns the compensation (the two solved
/// components + the residual report) WITHOUT synthesizing the final waveform; use
/// [`seds_compensated`] for the full waveform.
///
/// # Method
///
/// 1. Synthesize the raw composite and measure its discrete residuals `(V0, D0)` via
///    [`trap_residuals`] on the record of `n` samples at `fs_hz`.
/// 2. Build two unit-amplitude compensators at `f_c = LOW_FREQ_COMP_FRAC * f_lowest`
///    with distinct decays [`COMP_ZETA_A`] / [`COMP_ZETA_B`], and measure THEIR
///    residuals `(Va, Da)` and `(Vb, Db)` on the same record.
/// 3. Solve the 2x2 system for amplitudes `(alpha_a, alpha_b)`:
///    `alpha_a*Va + alpha_b*Vb = -V0`,  `alpha_a*Da + alpha_b*Db = -D0`.
/// 4. Return the two compensators scaled by the solved amplitudes; the report records
///    the raw residuals and the (near-zero) compensated residuals.
///
/// A degenerate table (empty, or a singular 2x2 from equal moments) returns
/// zero-amplitude compensators and reports the raw residuals unchanged - the caller can
/// detect a failed compensation from a nonzero `comp_residual_*` in the report.
pub fn compute_compensation(table: &DampedSineTable, fs_hz: f64, n: usize) -> SedsCompensation {
    let f_lowest = table
        .components
        .iter()
        .map(|c| c.freq_hz)
        .fold(f64::INFINITY, f64::min);
    let f_c = if f_lowest.is_finite() && f_lowest > 0.0 {
        LOW_FREQ_COMP_FRAC * f_lowest
    } else {
        1.0
    };

    // Two unit-amplitude compensators at f_c with distinct decays, no delay, positive.
    let comp_a = DampedSineComponent {
        freq_hz: f_c,
        amplitude_g: 1.0,
        zeta: COMP_ZETA_A,
        delay_s: 0.0,
        polarity: Polarity::Positive,
    };
    let comp_b = DampedSineComponent {
        freq_hz: f_c,
        amplitude_g: 1.0,
        zeta: COMP_ZETA_B,
        delay_s: 0.0,
        polarity: Polarity::Positive,
    };

    // Raw composite residuals.
    let raw = seds_composite_raw(table, fs_hz, n);
    let (v0, d0) = trap_residuals(&raw, fs_hz);

    // Each compensator's residuals at unit amplitude.
    let a_wave = {
        let mut o = vec![0.0f64; n];
        seds_component_into(&comp_a, fs_hz, &mut o);
        o
    };
    let b_wave = {
        let mut o = vec![0.0f64; n];
        seds_component_into(&comp_b, fs_hz, &mut o);
        o
    };
    let (va, da) = trap_residuals(&a_wave, fs_hz);
    let (vb, db) = trap_residuals(&b_wave, fs_hz);

    // Solve [Va Vb; Da Db] [alpha_a; alpha_b] = [-V0; -D0].
    let det = va * db - vb * da;
    let (alpha_a, alpha_b) = if det.abs() > 1e-30 {
        ((-v0 * db + vb * d0) / det, (-va * d0 + v0 * da) / det)
    } else {
        (0.0, 0.0)
    };

    let mut ca = comp_a;
    ca.amplitude_g = alpha_a;
    let mut cb = comp_b;
    cb.amplitude_g = alpha_b;

    // Compensated residuals: superpose the scaled compensators and re-measure.
    let mut comp_wave = raw.clone();
    seds_component_into(&ca, fs_hz, &mut comp_wave);
    seds_component_into(&cb, fs_hz, &mut comp_wave);
    let (vc, dc) = trap_residuals(&comp_wave, fs_hz);

    SedsCompensation {
        compensators: [ca, cb],
        report: CompensationReport {
            raw_residual_velocity_ips: v0,
            raw_residual_disp_in: d0,
            comp_residual_velocity_ips: vc,
            comp_residual_disp_in: dc,
        },
    }
}

/// Synthesize the COMPENSATED SEDS composite for a table at `fs_hz`: the raw composite
/// plus the Smallwood-Nord compensating pair. The record length
/// is chosen automatically ([`raw_record_samples`]) to span the ring-down plus a quiet
/// tail so the pure integrator can verify the (near-zero) residuals.
///
/// Returns the compensated waveform, its sample rate, and the compensation report. An
/// empty table yields an empty waveform and a zeroed report.
pub fn seds_compensated(table: &DampedSineTable, fs_hz: f64) -> SedsSynthResult {
    if fs_hz <= 0.0 || table.components.is_empty() {
        return SedsSynthResult {
            waveform_g: Vec::new(),
            fs_hz,
            compensation: SedsCompensation {
                compensators: [
                    DampedSineComponent {
                        freq_hz: 1.0,
                        amplitude_g: 0.0,
                        zeta: COMP_ZETA_A,
                        delay_s: 0.0,
                        polarity: Polarity::Positive,
                    },
                    DampedSineComponent {
                        freq_hz: 1.0,
                        amplitude_g: 0.0,
                        zeta: COMP_ZETA_B,
                        delay_s: 0.0,
                        polarity: Polarity::Positive,
                    },
                ],
                report: CompensationReport {
                    raw_residual_velocity_ips: 0.0,
                    raw_residual_disp_in: 0.0,
                    comp_residual_velocity_ips: 0.0,
                    comp_residual_disp_in: 0.0,
                },
            },
        };
    }
    let n = raw_record_samples(table, fs_hz);
    let compensation = compute_compensation(table, fs_hz, n);
    let mut waveform_g = seds_composite_raw(table, fs_hz, n);
    for c in &compensation.compensators {
        seds_component_into(c, fs_hz, &mut waveform_g);
    }
    SedsSynthResult {
        waveform_g,
        fs_hz,
        compensation,
    }
}
