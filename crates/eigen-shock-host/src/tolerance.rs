//! Classical-pulse tolerance checking - the THREE distinct checks:
//!
//! - **(a) the acceleration-amplitude envelope**: the measured pulse must lie
//!   inside a band around the nominal pulse shape (the standards' broken-line
//!   tolerance figures), including the quiet pre/post surround windows;
//! - **(b) the velocity-change tolerance**: the time integral of the measured
//!   acceleration over the standard's measurement window must match the nominal
//!   pulse's velocity change within the standard's dV tolerance - "(b)/(c) are
//!   core MIL-STD acceptance, not extras";
//! - **(c) pulse-substitution equivalence**: the measured dV expressed as the
//!   equivalent same-shape amplitude (and, design-side,
//!   [`crate::spec::substitute_amplitude_for_velocity_change`] for cross-shape
//!   substitution).
//!
//! # Encoded band parameters (verify against the controlling standard text)
//!
//! The per-standard parameters ([`ToleranceStandard::params`]) encode the
//! widely-reproduced tolerance figures:
//!
//! - **MIL-STD-810, Method 516 classical pulses** (the 516.8 lineage; the
//!   classical-pulse figures carry through the 516.x revisions): amplitude band
//!   `+/-15%` of the nominal peak around the nominal shape; measured velocity
//!   change within `+/-10%` of nominal (the standard's Vi acceptance), with the
//!   dV integration window running from `0.4*T` before the pulse to `0.1*T`
//!   after it (the 516.x velocity-change measurement convention).
//! - **IEC 60068-2-27** (Ea shock): amplitude band `+/-20%` of the nominal peak;
//!   velocity change within `+/-15%` of nominal.
//!
//! The exact broken-line time breakpoints of the printed figures vary by
//! standard revision and pulse shape; this checker encodes the band as
//! `nominal(t) +/- tol*A` over the pulse support plus `+/-tol*A` quiet surround
//! windows of `0.4*T` on each side, which is the figures' envelope structure.
//! Before using a report for FORMAL compliance, verify the parameters against
//! the controlling revision's figure - they are single constants in
//! [`ToleranceStandard::params`] precisely so a revision fix is one line.
//!
//! # What the measured record should be
//!
//! `check_tolerance` takes the measured control-channel acceleration (g) at the
//! FIRED level: scale `reference_spec.amplitude_g` to the commanded level (e.g.
//! `20 g * 10^(-12/20)` for a -12 dB pulse) before checking, because the checks
//! compare absolute values. On the HOST (Path B) path the measured record
//! contains the compensation shoulders; they occupy the surround windows, so a
//! shoulder solved deeper than the band (short/deep geometry) trips the surround
//! check - lengthen the shoulders (lower amplitude) to fit inside the band. That
//! is the real amplitude-vs-duration compensation trade, surfaced honestly.
//!
//! Units: g in, in/s for velocity change, via
//! [`eigen_dsp::integrate::G_TO_IN_S2`].

use crate::spec::ClassicalPulseSpec;
use eigen_dsp::integrate::G_TO_IN_S2;
use eigen_dsp::shock::pulse_kinematics;
use serde::{Deserialize, Serialize};

/// Which standard's tolerance parameters to check against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToleranceStandard {
    /// MIL-STD-810, Method 516 classical shock pulses (the 516.8 lineage).
    Mil810_516,
    /// IEC 60068-2-27 (environmental testing, shock).
    Iec60068_2_27,
}

/// The encoded per-standard band parameters (see the module docs for provenance
/// and the verify-before-formal-compliance caveat).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ToleranceParams {
    /// Amplitude-envelope half-width as a fraction of the nominal peak.
    pub amp_tol_frac_of_peak: f64,
    /// Velocity-change tolerance as a fraction of the nominal dV.
    pub dv_tol_frac: f64,
    /// Quiet surround window BEFORE the pulse, as a fraction of the pulse
    /// duration, held to `+/- amp_tol_frac_of_peak * A`.
    pub window_pre_frac: f64,
    /// Quiet surround window AFTER the pulse (same band).
    pub window_post_frac: f64,
    /// dV integration window start, fraction of the duration BEFORE the pulse.
    pub dv_pre_frac: f64,
    /// dV integration window end, fraction of the duration AFTER the pulse.
    pub dv_post_frac: f64,
}

impl ToleranceStandard {
    /// The encoded parameters for this standard.
    pub const fn params(self) -> ToleranceParams {
        match self {
            ToleranceStandard::Mil810_516 => ToleranceParams {
                amp_tol_frac_of_peak: 0.15,
                dv_tol_frac: 0.10,
                window_pre_frac: 0.4,
                window_post_frac: 0.4,
                dv_pre_frac: 0.4,
                dv_post_frac: 0.1,
            },
            ToleranceStandard::Iec60068_2_27 => ToleranceParams {
                amp_tol_frac_of_peak: 0.20,
                dv_tol_frac: 0.15,
                window_pre_frac: 0.4,
                window_post_frac: 0.4,
                dv_pre_frac: 0.4,
                dv_post_frac: 0.1,
            },
        }
    }
}

/// Check (a): the acceleration-amplitude envelope result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvelopeCheck {
    /// Overall envelope pass (main region AND surround windows).
    pub pass: bool,
    /// The band half-width used, fraction of the nominal peak.
    pub tol_frac_of_peak: f64,
    /// Worst absolute deviation from the band CENTER (nominal shape in the main
    /// region, zero in the surround), as a fraction of the nominal peak.
    pub worst_deviation_frac_of_peak: f64,
    /// Index (into the measured record) of the worst deviation, if any samples
    /// were checked.
    pub worst_sample_index: Option<usize>,
    /// The main pulse region alone passed.
    pub main_region_pass: bool,
    /// The pre/post quiet surround windows alone passed. On the host (Path B)
    /// path the compensation shoulders live here - see the module docs.
    pub surround_pass: bool,
    /// Headroom: `tol_frac_of_peak - worst_deviation_frac_of_peak` (negative
    /// when failing).
    pub margin_frac_of_peak: f64,
    /// How many measured samples fell inside the checked regions.
    pub checked_samples: usize,
}

/// Check (b): the velocity-change result. dV is the time integral of the
/// measured acceleration over the standard's measurement window (in/s).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VelocityChangeCheck {
    /// `|error_frac| <= tol_frac`.
    pub pass: bool,
    /// Measured dV over the standard's window, in/s.
    pub measured_dv_ips: f64,
    /// Nominal dV of the reference pulse (pure integrator on the nominal), in/s.
    pub nominal_dv_ips: f64,
    /// `(measured - nominal) / nominal`.
    pub error_frac: f64,
    /// The standard's dV tolerance fraction.
    pub tol_frac: f64,
    /// Headroom: `tol_frac - |error_frac|` (negative when failing).
    pub margin_frac: f64,
}

/// Check (c): pulse-substitution equivalence applied to the measured pulse -
/// the measured dV expressed as the SAME-SHAPE amplitude that would nominally
/// produce it. Cross-shape substitution design uses
/// [`crate::spec::substitute_amplitude_for_velocity_change`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubstitutionCheck {
    /// Pass mirrors the dV criterion (equivalence is judged on dV, so the
    /// amplitude ratio must sit within the same tolerance band).
    pub pass: bool,
    /// The reference-shape amplitude whose nominal dV equals the measured dV, g.
    pub dv_equivalent_amplitude_g: f64,
    /// The reference amplitude, g.
    pub nominal_amplitude_g: f64,
    /// `dv_equivalent_amplitude_g / nominal_amplitude_g`.
    pub amplitude_ratio: f64,
}

/// The full three-check tolerance report. Serializable: this is
/// the per-pulse pass/fail row of the shock report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToleranceReport {
    /// Which standard's parameters were applied.
    pub standard: ToleranceStandard,
    /// Check (a): the acceleration-amplitude envelope.
    pub amplitude_envelope: EnvelopeCheck,
    /// Check (b): the velocity-change tolerance.
    pub velocity_change: VelocityChangeCheck,
    /// Check (c): the substitution dV equivalence.
    pub substitution: SubstitutionCheck,
    /// Where (sample index into the measured record) the aligned nominal pulse
    /// starts. May be reported even when the alignment landed at the record edge.
    pub aligned_pulse_start_index: i64,
    /// All three checks passed.
    pub overall_pass: bool,
}

/// Run the three tolerance checks of `measured_g` (the control-channel
/// acceleration at the FIRED level, g, sampled at `fs_hz`) against
/// `reference_spec` under `standard`. See the module docs for what the measured
/// record should be and the encoded-parameter caveat.
///
/// # Method
///
/// 1. Generate the nominal (uncompensated) pulse from the spec at `fs_hz`.
/// 2. Align it to the measured record by maximizing the cross-correlation over
///    all integer lags (robust for flat-top shapes where a peak-sample alignment
///    is ambiguous).
/// 3. **(a)** check every measured sample in the aligned pulse support against
///    `nominal +/- tol*A`, and in the `0.4*T` pre/post surround windows against
///    `+/- tol*A`;
/// 4. **(b)** trapezoid-integrate the measured record over the standard's dV
///    window (`dv_pre_frac*T` before to `dv_post_frac*T` after the pulse) and
///    compare to the nominal pulse's dV within `dv_tol_frac`;
/// 5. **(c)** express the measured dV as the equivalent reference-shape
///    amplitude.
///
/// Degenerate inputs (empty measured record, invalid spec / `fs_hz`) return a
/// failed report with zeroed metrics rather than panicking (library path,
/// matching eigen-shock-common's no-panic convention).
pub fn check_tolerance(
    measured_g: &[f64],
    reference_spec: &ClassicalPulseSpec,
    fs_hz: f64,
    standard: ToleranceStandard,
) -> ToleranceReport {
    let p = standard.params();
    let nominal = if reference_spec.validate().is_ok() && fs_hz > 0.0 {
        reference_spec.main_pulse(fs_hz)
    } else {
        Vec::new()
    };
    if measured_g.is_empty() || nominal.is_empty() {
        return failed_report(standard, reference_spec.amplitude_g, p);
    }

    let n_nom = nominal.len();
    let peak_a = reference_spec.amplitude_g;
    let s0 = align_by_cross_correlation(measured_g, &nominal);

    // ---- (a) amplitude envelope ------------------------------------------
    let w_pre = ((n_nom as f64) * p.window_pre_frac).round() as i64;
    let w_post = ((n_nom as f64) * p.window_post_frac).round() as i64;
    let mut worst_frac = 0.0f64;
    let mut worst_idx: Option<usize> = None;
    let mut main_worst = 0.0f64;
    let mut surround_worst = 0.0f64;
    let mut checked = 0usize;
    let lo = s0 - w_pre;
    let hi = s0 + n_nom as i64 + w_post; // exclusive
    for i in lo..hi {
        if i < 0 || i as usize >= measured_g.len() {
            continue; // window clipped by the record edge
        }
        let m = measured_g[i as usize];
        let in_main = i >= s0 && i < s0 + n_nom as i64;
        let center = if in_main {
            nominal[(i - s0) as usize]
        } else {
            0.0
        };
        let dev_frac = (m - center).abs() / peak_a;
        checked += 1;
        if in_main {
            main_worst = main_worst.max(dev_frac);
        } else {
            surround_worst = surround_worst.max(dev_frac);
        }
        if dev_frac > worst_frac {
            worst_frac = dev_frac;
            worst_idx = Some(i as usize);
        }
    }
    let tol = p.amp_tol_frac_of_peak;
    let main_pass = main_worst <= tol;
    let surround_pass = surround_worst <= tol;
    let envelope = EnvelopeCheck {
        pass: main_pass && surround_pass && checked > 0,
        tol_frac_of_peak: tol,
        worst_deviation_frac_of_peak: worst_frac,
        worst_sample_index: worst_idx,
        main_region_pass: main_pass,
        surround_pass,
        margin_frac_of_peak: tol - worst_frac,
        checked_samples: checked,
    };

    // ---- (b) velocity change ---------------------------------------------
    // Nominal dV from the SAME pure integrator that measures (consistency).
    let nominal_dv_ips = pulse_kinematics(&nominal, fs_hz).residual_velocity_ips;
    let dv_lo = s0 - ((n_nom as f64) * p.dv_pre_frac).round() as i64;
    let dv_hi = s0 + n_nom as i64 + ((n_nom as f64) * p.dv_post_frac).round() as i64;
    let measured_dv_ips = windowed_dv_ips(measured_g, fs_hz, dv_lo, dv_hi);
    let error_frac = if nominal_dv_ips.abs() > 0.0 {
        (measured_dv_ips - nominal_dv_ips) / nominal_dv_ips
    } else {
        f64::INFINITY
    };
    let dv_pass = error_frac.is_finite() && error_frac.abs() <= p.dv_tol_frac;
    let velocity_change = VelocityChangeCheck {
        pass: dv_pass,
        measured_dv_ips,
        nominal_dv_ips,
        error_frac,
        tol_frac: p.dv_tol_frac,
        margin_frac: p.dv_tol_frac - error_frac.abs(),
    };

    // ---- (c) substitution equivalence -------------------------------------
    // dV scales linearly with amplitude for a fixed shape/duration, so the
    // same-shape equivalent amplitude is A * (dv_meas / dv_nom).
    let ratio = if nominal_dv_ips.abs() > 0.0 {
        measured_dv_ips / nominal_dv_ips
    } else {
        0.0
    };
    let substitution = SubstitutionCheck {
        pass: dv_pass,
        dv_equivalent_amplitude_g: peak_a * ratio,
        nominal_amplitude_g: peak_a,
        amplitude_ratio: ratio,
    };

    let overall_pass = envelope.pass && velocity_change.pass && substitution.pass;
    ToleranceReport {
        standard,
        amplitude_envelope: envelope,
        velocity_change,
        substitution,
        aligned_pulse_start_index: s0,
        overall_pass,
    }
}

/// Align `nominal` to `measured` by maximizing the raw cross-correlation over
/// all integer lags (nominal fully-before to fully-after the record). Returns
/// the lag: the measured index where `nominal[0]` best sits (may be negative if
/// the pulse is clipped at the record start).
fn align_by_cross_correlation(measured: &[f64], nominal: &[f64]) -> i64 {
    let n_m = measured.len() as i64;
    let n_n = nominal.len() as i64;
    let mut best = f64::NEG_INFINITY;
    let mut best_lag = 0i64;
    for lag in (1 - n_n)..n_m {
        let mut s = 0.0;
        for (j, &nv) in nominal.iter().enumerate() {
            let i = lag + j as i64;
            if i >= 0 && i < n_m {
                s += measured[i as usize] * nv;
            }
        }
        if s > best {
            best = s;
            best_lag = lag;
        }
    }
    best_lag
}

/// Trapezoidal time integral of `measured` (g) over sample window `[lo, hi)`
/// (clipped to the record), converted to in/s via [`G_TO_IN_S2`].
fn windowed_dv_ips(measured: &[f64], fs_hz: f64, lo: i64, hi: i64) -> f64 {
    let n = measured.len() as i64;
    let lo = lo.max(0);
    let hi = hi.min(n);
    if hi - lo < 2 {
        return 0.0;
    }
    let dt = 1.0 / fs_hz;
    let mut sum = 0.0;
    for i in (lo + 1)..hi {
        sum += 0.5 * (measured[(i - 1) as usize] + measured[i as usize]) * dt;
    }
    sum * G_TO_IN_S2
}

/// A failed report for degenerate inputs (empty record / invalid spec).
fn failed_report(
    standard: ToleranceStandard,
    nominal_amplitude_g: f64,
    p: ToleranceParams,
) -> ToleranceReport {
    ToleranceReport {
        standard,
        amplitude_envelope: EnvelopeCheck {
            pass: false,
            tol_frac_of_peak: p.amp_tol_frac_of_peak,
            worst_deviation_frac_of_peak: 0.0,
            worst_sample_index: None,
            main_region_pass: false,
            surround_pass: false,
            margin_frac_of_peak: 0.0,
            checked_samples: 0,
        },
        velocity_change: VelocityChangeCheck {
            pass: false,
            measured_dv_ips: 0.0,
            nominal_dv_ips: 0.0,
            error_frac: 0.0,
            tol_frac: p.dv_tol_frac,
            margin_frac: 0.0,
        },
        substitution: SubstitutionCheck {
            pass: false,
            dv_equivalent_amplitude_g: 0.0,
            nominal_amplitude_g,
            amplitude_ratio: 0.0,
        },
        aligned_pulse_start_index: 0,
        overall_pass: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{ClassicalPulseSpec, ClassicalShape};

    const FS: f64 = 5120.0;

    fn spec_20g_11ms() -> ClassicalPulseSpec {
        ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, 11.0)
    }

    /// A measured record: `scale x` the nominal pulse embedded at `offset` in a
    /// quiet record of `total` samples.
    fn embedded(spec: &ClassicalPulseSpec, scale: f64, offset: usize, total: usize) -> Vec<f64> {
        let nominal = spec.main_pulse(FS);
        let mut rec = vec![0.0f64; total];
        for (j, &v) in nominal.iter().enumerate() {
            rec[offset + j] = scale * v;
        }
        rec
    }

    #[test]
    fn perfect_pulse_passes_all_three_checks_both_standards() {
        let spec = spec_20g_11ms();
        let rec = embedded(&spec, 1.0, 200, 800);
        for std_ in [
            ToleranceStandard::Mil810_516,
            ToleranceStandard::Iec60068_2_27,
        ] {
            let r = check_tolerance(&rec, &spec, FS, std_);
            assert!(r.overall_pass, "{std_:?}: {r:?}");
            assert!(r.amplitude_envelope.pass);
            assert!(r.velocity_change.pass);
            assert!(r.substitution.pass);
            assert_eq!(r.aligned_pulse_start_index, 200);
            assert!(r.amplitude_envelope.margin_frac_of_peak > 0.1);
            assert!(r.velocity_change.error_frac.abs() < 1e-9);
            assert!((r.substitution.amplitude_ratio - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn twelve_percent_high_pulse_discriminates_the_standards() {
        // 1.12x amplitude: dV error +12% -> FAILS MIL (10%) and PASSES IEC (15%);
        // the envelope deviation (12% of peak) passes both bands (15% / 20%).
        let spec = spec_20g_11ms();
        let rec = embedded(&spec, 1.12, 200, 800);

        let mil = check_tolerance(&rec, &spec, FS, ToleranceStandard::Mil810_516);
        assert!(mil.amplitude_envelope.pass, "{mil:?}");
        assert!(!mil.velocity_change.pass);
        assert!(!mil.substitution.pass);
        assert!(!mil.overall_pass);

        let iec = check_tolerance(&rec, &spec, FS, ToleranceStandard::Iec60068_2_27);
        assert!(iec.overall_pass, "{iec:?}");
        assert!((iec.velocity_change.error_frac - 0.12).abs() < 0.005);
        // Check (c): the measured dV is equivalent to a ~22.4 g half-sine.
        assert!((iec.substitution.dv_equivalent_amplitude_g - 22.4).abs() < 0.15);
    }

    #[test]
    fn grossly_high_pulse_fails_envelope_and_dv() {
        let spec = spec_20g_11ms();
        let rec = embedded(&spec, 1.30, 200, 800);
        for std_ in [
            ToleranceStandard::Mil810_516,
            ToleranceStandard::Iec60068_2_27,
        ] {
            let r = check_tolerance(&rec, &spec, FS, std_);
            assert!(!r.amplitude_envelope.pass, "{std_:?}");
            assert!(!r.amplitude_envelope.main_region_pass);
            assert!(!r.velocity_change.pass);
            assert!(!r.overall_pass);
            assert!(r.amplitude_envelope.worst_deviation_frac_of_peak > 0.25);
        }
    }

    #[test]
    fn surround_garbage_trips_the_quiet_windows() {
        // A clean main pulse but a big blip just before it: main passes, the
        // surround window fails, and the envelope reports the split.
        let spec = spec_20g_11ms();
        let mut rec = embedded(&spec, 1.0, 200, 800);
        rec[190] = 8.0; // 40% of peak, well inside the 0.4*T pre window
        let r = check_tolerance(&rec, &spec, FS, ToleranceStandard::Iec60068_2_27);
        assert!(r.amplitude_envelope.main_region_pass);
        assert!(!r.amplitude_envelope.surround_pass);
        assert!(!r.amplitude_envelope.pass);
    }

    #[test]
    fn alignment_finds_the_pulse_anywhere_in_the_record() {
        let spec = ClassicalPulseSpec::new(ClassicalShape::TerminalPeakSawtooth, 30.0, 6.0);
        for offset in [0usize, 37, 500] {
            let rec = embedded(&spec, 1.0, offset, 700);
            let r = check_tolerance(&rec, &spec, FS, ToleranceStandard::Mil810_516);
            assert_eq!(
                r.aligned_pulse_start_index, offset as i64,
                "offset {offset}"
            );
            assert!(r.overall_pass, "offset {offset}: {r:?}");
        }
    }

    #[test]
    fn flat_top_shapes_align_and_pass() {
        // Cross-correlation alignment must handle plateau shapes where the peak
        // sample is ambiguous.
        for shape in [
            ClassicalShape::Rectangle,
            ClassicalShape::Trapezoid {
                rise_frac: 0.2,
                fall_frac: 0.2,
            },
        ] {
            let spec = ClassicalPulseSpec::new(shape, 15.0, 10.0);
            let rec = embedded(&spec, 1.0, 120, 600);
            let r = check_tolerance(&rec, &spec, FS, ToleranceStandard::Iec60068_2_27);
            assert!(r.overall_pass, "{shape:?}: {r:?}");
        }
    }

    #[test]
    fn duration_stretch_fails_dv() {
        // Measured pulse is the right amplitude but 1.4x the duration: dV runs
        // ~40% high, failing both standards' dV bands.
        let spec = spec_20g_11ms();
        let stretched = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, 11.0 * 1.4);
        let rec = embedded(&stretched, 1.0, 200, 900);
        for std_ in [
            ToleranceStandard::Mil810_516,
            ToleranceStandard::Iec60068_2_27,
        ] {
            let r = check_tolerance(&rec, &spec, FS, std_);
            assert!(!r.velocity_change.pass, "{std_:?}: {r:?}");
            assert!(r.velocity_change.error_frac > 0.2);
        }
    }

    #[test]
    fn degenerate_inputs_fail_gracefully() {
        let spec = spec_20g_11ms();
        let r = check_tolerance(&[], &spec, FS, ToleranceStandard::Mil810_516);
        assert!(!r.overall_pass);
        assert_eq!(r.amplitude_envelope.checked_samples, 0);

        let mut bad = spec_20g_11ms();
        bad.amplitude_g = -1.0;
        let r2 = check_tolerance(&[0.0; 100], &bad, FS, ToleranceStandard::Mil810_516);
        assert!(!r2.overall_pass);

        let r3 = check_tolerance(&[0.0; 100], &spec, 0.0, ToleranceStandard::Iec60068_2_27);
        assert!(!r3.overall_pass);
    }

    #[test]
    fn report_serde_round_trip() {
        let spec = spec_20g_11ms();
        let rec = embedded(&spec, 1.05, 100, 500);
        let r = check_tolerance(&rec, &spec, FS, ToleranceStandard::Mil810_516);
        let json = serde_json::to_string(&r).unwrap();
        let back: ToleranceReport = serde_json::from_str(&json).unwrap();
        // Workspace serde_json runs WITHOUT `float_roundtrip`, so parsing may be
        // 1 ULP off on pathological decimals; compare the decision-bearing
        // fields exactly and the floats to well below any engineering tolerance.
        assert!(json.contains("Mil810_516"));
        assert_eq!(back.standard, r.standard);
        assert_eq!(back.overall_pass, r.overall_pass);
        assert_eq!(back.amplitude_envelope.pass, r.amplitude_envelope.pass);
        assert_eq!(back.velocity_change.pass, r.velocity_change.pass);
        assert_eq!(back.substitution.pass, r.substitution.pass);
        assert_eq!(back.aligned_pulse_start_index, r.aligned_pulse_start_index);
        assert!((back.velocity_change.error_frac - r.velocity_change.error_frac).abs() < 1e-12);
    }
}
