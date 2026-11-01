//! Test suite for `eigen-srs`. Organized by the four synthesis GATES plus the
//! import/validation/determinism checks.
//!
//! GATES:
//! - (i) wavelet composite residual velocity + displacement ~0 (< 1e-9 relative);
//! - (ii) SEDS WITHOUT compensation NONZERO residual, WITH compensation ~0
//!   (< 1e-6 in/s velocity, < 1e-8 in displacement) - THE fixture;
//! - (iii) BOTH engines converge to a canonical target (flat 100 g SRS 100-2000 Hz,
//!   Q=10, 1/6-octave: converged, max |error| <= 3 dB, typical <= 1.5 dB);
//! - (iv) component count != band count synthesizes + converges (no 1:1 assumption).
//!
//! Plus: import round-trips, validation (even half-cycles rejected), determinism.

use crate::iterate::{synthesize_to_target, SynthesisEngine, SynthesisOpts};
use crate::seds::{seds_compensated, seds_composite_raw};
use crate::table::{
    DampedSineComponent, DampedSineTable, Polarity, WaveletComponent, WaveletTable,
};
use crate::wavsyn::wavsyn_composite;
use eigen_dsp::shock::pulse_kinematics;
use eigen_dsp::srs::{octave_freqs, srs_analyze, OctaveFraction};

/// The canonical analysis grid for the convergence gates: 100-2000 Hz, 1/6-octave.
fn canonical_bands() -> Vec<f64> {
    octave_freqs(100.0, 2000.0, OctaveFraction::Sixth)
}

/// Worst and median absolute per-band error, dB.
fn worst_median(errs: &[f64]) -> (f64, f64) {
    let worst = errs.iter().cloned().fold(0.0f64, |m, e| m.max(e.abs()));
    let mut v: Vec<f64> = errs.iter().map(|e| e.abs()).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = if v.is_empty() { 0.0 } else { v[v.len() / 2] };
    (worst, median)
}

// ===========================================================================
// GATE (i): WAVSYN composite self-nulls (zero net velocity + displacement).
// ===========================================================================
mod gate_i_wavelet_selfnull {
    use super::*;

    /// A single odd-half-cycle wavelet self-nulls to ~0 (the 1e-15 single-wavelet
    /// result, re-verified through this crate's synthesis path). Residuals are
    /// negligible relative to the in-pulse peak.
    #[test]
    fn single_wavelet_residuals_near_zero() {
        let fs = 50_000.0;
        let c = WaveletComponent::new(100.0, 1.0, 5, 0.0).unwrap();
        let t = WaveletTable {
            components: vec![c],
        };
        // tail of 0.05 s lets any residual velocity ramp displacement visibly.
        let w = wavsyn_composite(&t, fs, 0.05);
        let k = pulse_kinematics(&w, fs);
        assert!(k.peak_velocity_ips > 1e-3, "sanity: pulse has real motion");
        let rv_ratio = k.residual_velocity_ips.abs() / k.peak_velocity_ips;
        let peak_abs_disp = k.peak_disp_in.abs().max(k.min_disp_in.abs());
        let rd_ratio = k.residual_disp_in.abs() / peak_abs_disp.max(1e-12);
        // The golden vector asserts ~1e-15; assert well under the composite gate.
        assert!(
            rv_ratio < 1e-9,
            "single-wavelet residual velocity ratio {rv_ratio:.3e}"
        );
        assert!(
            rd_ratio < 1e-9,
            "single-wavelet residual displacement ratio {rd_ratio:.3e}"
        );
    }

    /// The COMPOSITE (a reverse-sine-sweep delayed sum of many odd wavelets) inherits
    /// the self-nulling property: net velocity AND net displacement are ~0 relative to
    /// the in-pulse peaks (< 1e-9), BY CONSTRUCTION (no compensation involved). This is
    /// gate (i).
    #[test]
    fn composite_residuals_near_zero() {
        // The grid-exact WAVSYN construction (grid-normalized phase, see
        // wavsyn::wavsyn_component_into) self-nulls to machine precision at ANY sample
        // rate, so this runs at the canonical 20 kHz and still achieves ~1e-15 velocity
        // / ~1e-13 displacement ratios - far under the 1e-9 gate. Contrast the naive
        // absolute-time sampling, which is discretization-limited (~1e-6 here).
        let fs = 20_000.0;
        // Default 1/6-octave table over the canonical band (27 delayed components).
        let mut t = WaveletTable::default_over_band(100.0, 2000.0, OctaveFraction::Sixth).unwrap();
        // Give components nonunity, mixed-sign amplitudes so the composite is a real
        // broadband burst, not a trivial one.
        for (i, c) in t.components.iter_mut().enumerate() {
            c.amplitude_g = if i % 3 == 0 { -2.0 } else { 1.5 };
        }
        let w = wavsyn_composite(&t, fs, 0.1);
        let k = pulse_kinematics(&w, fs);
        assert!(
            k.peak_velocity_ips > 1e-2,
            "sanity: composite has real motion"
        );
        let rv_ratio = k.residual_velocity_ips.abs() / k.peak_velocity_ips;
        let peak_abs_disp = k.peak_disp_in.abs().max(k.min_disp_in.abs());
        let rd_ratio = k.residual_disp_in.abs() / peak_abs_disp.max(1e-12);
        assert!(
            rv_ratio < 1e-9,
            "composite residual velocity {} not ~0 (ratio {rv_ratio:.3e})",
            k.residual_velocity_ips
        );
        assert!(
            rd_ratio < 1e-9,
            "composite residual displacement {} not ~0 (ratio {rd_ratio:.3e})",
            k.residual_disp_in
        );
    }

    /// The default reverse-sine-sweep assigns the highest-frequency component zero
    /// delay and progressively delays lower-frequency components.
    #[test]
    fn reverse_sweep_delay_assignment() {
        let t = WaveletTable::default_over_band(100.0, 2000.0, OctaveFraction::Third).unwrap();
        let hi = t
            .components
            .iter()
            .max_by(|a, b| a.freq_hz.partial_cmp(&b.freq_hz).unwrap())
            .unwrap();
        let lo = t
            .components
            .iter()
            .min_by(|a, b| a.freq_hz.partial_cmp(&b.freq_hz).unwrap())
            .unwrap();
        assert_eq!(
            hi.delay_s, 0.0,
            "highest-freq component fires first (zero delay)"
        );
        assert!(
            lo.delay_s > hi.delay_s,
            "lower-freq components are delayed later"
        );
    }
}

// ===========================================================================
// GATE (ii): SEDS uncompensated NONZERO residual; compensated ~0. THE fixture.
// ===========================================================================
mod gate_ii_seds_compensation {
    use super::*;

    /// A single raw damped sine does NOT self-null: its net velocity is nonzero and
    /// matches the closed form `A/(w*(1+zeta^2))` in in/s (times G_TO_IN_S2). This is
    /// the physical reason compensation is needed.
    #[test]
    fn raw_damped_sine_residual_matches_closed_form() {
        use eigen_dsp::integrate::G_TO_IN_S2;
        let fs = 100_000.0;
        let f = 200.0;
        let zeta = 0.05;
        let a = 3.0;
        let c = DampedSineComponent::new(f, a, zeta, 0.0, Polarity::Positive).unwrap();
        let t = DampedSineTable {
            components: vec![c],
        };
        let n = (0.5 * fs) as usize;
        let w = seds_composite_raw(&t, fs, n);
        let k = pulse_kinematics(&w, fs);
        // int_0^inf A e^{-zwt} sin(wt) dt = A * w/(sigma^2 + w^2) = A / (w (1 + zeta^2)).
        let wn = std::f64::consts::TAU * f;
        let expected = a / (wn * (1.0 + zeta * zeta)) * G_TO_IN_S2;
        let rel = (k.residual_velocity_ips - expected).abs() / expected.abs();
        assert!(
            rel < 1e-3,
            "raw damped-sine net velocity {} vs closed form {expected} (rel {rel:.3e})",
            k.residual_velocity_ips
        );
        assert!(
            expected.abs() > 1e-3,
            "closed-form residual is genuinely nonzero"
        );
    }

    /// THE fixture (gate ii): the uncompensated SEDS composite has NONZERO residual
    /// velocity AND displacement; the compensated composite drives BOTH to ~0
    /// (< 1e-6 in/s, < 1e-8 in), verified through the pure integrator.
    #[test]
    fn compensated_composite_residuals_near_zero() {
        let fs = 20_000.0;
        let table =
            DampedSineTable::default_over_band(100.0, 2000.0, OctaveFraction::Third).unwrap();

        let res = seds_compensated(&table, fs);
        let rep = res.compensation.report;

        // Uncompensated residuals are genuinely NONZERO (the reason to compensate).
        assert!(
            rep.raw_residual_velocity_ips.abs() > 1e-2,
            "uncompensated residual velocity should be clearly nonzero, got {} in/s",
            rep.raw_residual_velocity_ips
        );
        assert!(
            rep.raw_residual_disp_in.abs() > 1e-3,
            "uncompensated residual displacement should be clearly nonzero, got {} in",
            rep.raw_residual_disp_in
        );

        // Compensated residuals ~0 to the gate thresholds.
        assert!(
            rep.comp_residual_velocity_ips.abs() < 1e-6,
            "compensated residual velocity {} not < 1e-6 in/s",
            rep.comp_residual_velocity_ips
        );
        assert!(
            rep.comp_residual_disp_in.abs() < 1e-8,
            "compensated residual displacement {} not < 1e-8 in",
            rep.comp_residual_disp_in
        );

        // Independent verification through the pure double-integrator on the returned
        // waveform: the compensation is only trustworthy if the integrator that
        // will be used in the field agrees with the report.
        let k = pulse_kinematics(&res.waveform_g, fs);
        assert!(
            k.residual_velocity_ips.abs() < 1e-6,
            "pulse_kinematics residual velocity {} not < 1e-6 in/s",
            k.residual_velocity_ips
        );
        assert!(
            k.residual_disp_in.abs() < 1e-8,
            "pulse_kinematics residual displacement {} not < 1e-8 in",
            k.residual_disp_in
        );
        assert!(
            k.peak_velocity_ips > 1.0,
            "compensated composite has real motion"
        );
    }

    /// The compensation is robust across component spacings and frequency bands.
    #[test]
    fn compensation_across_spacings() {
        let fs = 20_000.0;
        for frac in [
            OctaveFraction::Whole,
            OctaveFraction::Third,
            OctaveFraction::Sixth,
        ] {
            let table = DampedSineTable::default_over_band(50.0, 1500.0, frac).unwrap();
            let res = seds_compensated(&table, fs);
            let k = pulse_kinematics(&res.waveform_g, fs);
            assert!(
                k.residual_velocity_ips.abs() < 1e-6 && k.residual_disp_in.abs() < 1e-8,
                "compensation failed at {frac:?}: rv={} rd={}",
                k.residual_velocity_ips,
                k.residual_disp_in
            );
        }
    }

    /// Mixed-polarity, delayed, non-unity components still compensate to ~0.
    #[test]
    fn compensation_with_mixed_components() {
        let fs = 25_000.0;
        let comps = vec![
            DampedSineComponent::new(120.0, 2.0, 0.03, 0.0, Polarity::Positive).unwrap(),
            DampedSineComponent::new(300.0, 5.0, 0.08, 0.001, Polarity::Negative).unwrap(),
            DampedSineComponent::new(800.0, 3.5, 0.05, 0.0005, Polarity::Positive).unwrap(),
            DampedSineComponent::new(1500.0, 1.0, 0.10, 0.0, Polarity::Negative).unwrap(),
        ];
        let table = DampedSineTable { components: comps };
        let res = seds_compensated(&table, fs);
        let k = pulse_kinematics(&res.waveform_g, fs);
        assert!(
            k.residual_velocity_ips.abs() < 1e-6,
            "mixed residual velocity {}",
            k.residual_velocity_ips
        );
        assert!(
            k.residual_disp_in.abs() < 1e-8,
            "mixed residual displacement {}",
            k.residual_disp_in
        );
    }

    /// The two compensators differ in decay (so the 2x2 solve is well conditioned) and
    /// sit below the synthesis band.
    #[test]
    fn compensator_structure() {
        let fs = 20_000.0;
        let table =
            DampedSineTable::default_over_band(100.0, 2000.0, OctaveFraction::Third).unwrap();
        let res = seds_compensated(&table, fs);
        let [ca, cb] = &res.compensation.compensators;
        assert!(ca.zeta != cb.zeta, "compensator decays must differ");
        assert_eq!(
            ca.freq_hz, cb.freq_hz,
            "compensator pair at the same low frequency"
        );
        assert!(
            ca.freq_hz < 100.0,
            "compensator below the synthesis band (got {})",
            ca.freq_hz
        );
    }
}

// ===========================================================================
// GATE (iii): BOTH engines converge to the canonical target.
// ===========================================================================
mod gate_iii_convergence {
    use super::*;

    /// The canonical convergence gate: flat 100 g SRS, 100-2000 Hz, Q=10, 1/6-octave
    /// analysis, default 1/6-octave components. WAVSYN converges with max |error| well
    /// under the 3 dB gate bound.
    #[test]
    fn wavsyn_converges_to_flat_100g() {
        let fs = 20_000.0;
        let bands = canonical_bands();
        let target = vec![100.0f64; bands.len()];
        let engine = SynthesisEngine::Wavsyn(
            WaveletTable::default_over_band(100.0, 2000.0, OctaveFraction::Sixth).unwrap(),
        );
        let opts = SynthesisOpts {
            max_iters: 80,
            tol_db: 1.5,
            max_gain_db_per_iter: 3.0,
            q: 10.0,
        };
        let out = synthesize_to_target(&target, &bands, fs, engine, opts);
        let (worst, median) = worst_median(&out.per_band_error_db);
        assert!(
            out.converged,
            "WAVSYN did not converge (worst {worst:.3} dB, iters {})",
            out.iters
        );
        assert!(
            worst <= 3.0,
            "WAVSYN worst error {worst:.3} dB exceeds 3 dB gate"
        );
        assert!(
            median <= 1.5,
            "WAVSYN median error {median:.3} dB exceeds typical 1.5 dB"
        );
    }

    /// SEDS converges to the same canonical target with max |error| under the 3 dB gate
    /// bound (compensation recomputed each iteration as amplitudes change).
    #[test]
    fn seds_converges_to_flat_100g() {
        let fs = 20_000.0;
        let bands = canonical_bands();
        let target = vec![100.0f64; bands.len()];
        let engine = SynthesisEngine::Seds(
            DampedSineTable::default_over_band(100.0, 2000.0, OctaveFraction::Sixth).unwrap(),
        );
        let opts = SynthesisOpts {
            max_iters: 80,
            tol_db: 1.5,
            max_gain_db_per_iter: 3.0,
            q: 10.0,
        };
        let out = synthesize_to_target(&target, &bands, fs, engine, opts);
        let (worst, _median) = worst_median(&out.per_band_error_db);
        assert!(
            out.converged,
            "SEDS did not converge (worst {worst:.3} dB, iters {})",
            out.iters
        );
        assert!(
            worst <= 3.0,
            "SEDS worst error {worst:.3} dB exceeds 3 dB gate"
        );
        // The SEDS drive must ALSO stay kinematically clean at the converged amplitudes.
        let k = pulse_kinematics(&out.waveform, fs);
        assert!(
            k.residual_velocity_ips.abs() < 1e-6,
            "converged SEDS residual velocity {}",
            k.residual_velocity_ips
        );
        assert!(
            k.residual_disp_in.abs() < 1e-8,
            "converged SEDS residual displacement {}",
            k.residual_disp_in
        );
    }

    /// A non-flat (sloped) target: the loop is not special-cased to a flat spectrum. A
    /// steep 5:1 log-slope target (30 -> 150 g) matches well across the INTERIOR of the
    /// band; the single lowest-frequency boundary band is the hardest (only one
    /// component sits near it and there is no lower component to help fill it - a known
    /// SRS-synthesis edge effect), so this test asserts the interior converges tightly
    /// while documenting the boundary band as the residual. The flat-target gate above
    /// carries the full-band convergence claim.
    #[test]
    fn seds_matches_sloped_target_interior() {
        let fs = 20_000.0;
        let bands = canonical_bands();
        let f_lo = bands[0];
        let f_hi = *bands.last().unwrap();
        let target: Vec<f64> = bands
            .iter()
            .map(|&f| {
                let frac = (f / f_lo).ln() / (f_hi / f_lo).ln();
                30.0 + frac * 120.0
            })
            .collect();
        let engine = SynthesisEngine::Seds(
            DampedSineTable::default_over_band(100.0, 2000.0, OctaveFraction::Sixth).unwrap(),
        );
        let opts = SynthesisOpts {
            max_iters: 120,
            tol_db: 1.5,
            max_gain_db_per_iter: 6.0,
            q: 10.0,
        };
        let out = synthesize_to_target(&target, &bands, fs, engine, opts);
        // Interior bands (drop the single lowest boundary band) converge within 3 dB.
        let interior_worst = out.per_band_error_db[1..]
            .iter()
            .cloned()
            .fold(0.0f64, |m, e| m.max(e.abs()));
        assert!(
            interior_worst <= 3.0,
            "SEDS sloped interior worst {interior_worst:.3} dB exceeds 3 dB"
        );
        // The compensated drive stays kinematically clean at the sloped amplitudes.
        let k = pulse_kinematics(&out.waveform, fs);
        assert!(
            k.residual_velocity_ips.abs() < 1e-6 && k.residual_disp_in.abs() < 1e-8,
            "sloped SEDS residuals: rv={} rd={}",
            k.residual_velocity_ips,
            k.residual_disp_in
        );
    }
}

// ===========================================================================
// GATE (iv): component count != band count synthesizes + converges.
// ===========================================================================
mod gate_iv_count_mismatch {
    use super::*;

    /// SEDS with 1/3-octave components (14) matched against a 1/6-octave analysis grid
    /// (27 bands) synthesizes and converges - NO 1:1 component-to-band assumption.
    #[test]
    fn seds_third_octave_components_vs_sixth_octave_analysis() {
        let fs = 20_000.0;
        let bands = canonical_bands(); // 1/6-octave, 27 bands
        let target = vec![100.0f64; bands.len()];
        let table =
            DampedSineTable::default_over_band(100.0, 2000.0, OctaveFraction::Third).unwrap();
        assert_ne!(
            table.components.len(),
            bands.len(),
            "precondition: component count must differ from band count"
        );
        let opts = SynthesisOpts {
            max_iters: 80,
            tol_db: 1.5,
            max_gain_db_per_iter: 3.0,
            q: 10.0,
        };
        let out = synthesize_to_target(&target, &bands, fs, SynthesisEngine::Seds(table), opts);
        let (worst, _m) = worst_median(&out.per_band_error_db);
        assert!(
            out.converged,
            "SEDS mismatched did not converge (worst {worst:.3} dB)"
        );
        assert!(
            worst <= 3.0,
            "SEDS mismatched worst {worst:.3} dB exceeds 3 dB"
        );
    }

    /// WAVSYN with 1/3-octave components (14) vs a 1/6-octave analysis grid (27 bands)
    /// synthesizes and converges within the 3 dB gate bound - a genuine count mismatch
    /// (14 components cannot fill 27 bands to 1.5 dB, but the loop makes no 1:1
    /// assumption and reaches the 3 dB gate bound).
    #[test]
    fn wavsyn_third_octave_components_vs_sixth_octave_analysis() {
        let fs = 20_000.0;
        let bands = canonical_bands();
        let target = vec![100.0f64; bands.len()];
        let table = WaveletTable::default_over_band(100.0, 2000.0, OctaveFraction::Third).unwrap();
        assert_ne!(
            table.components.len(),
            bands.len(),
            "precondition: counts differ"
        );
        let opts = SynthesisOpts {
            max_iters: 80,
            tol_db: 3.0,
            max_gain_db_per_iter: 3.0,
            q: 10.0,
        };
        let out = synthesize_to_target(&target, &bands, fs, SynthesisEngine::Wavsyn(table), opts);
        let (worst, _m) = worst_median(&out.per_band_error_db);
        assert!(
            out.converged,
            "WAVSYN mismatched did not converge (worst {worst:.3} dB, iters {})",
            out.iters
        );
        assert!(
            worst <= 3.0,
            "WAVSYN mismatched worst {worst:.3} dB exceeds 3 dB"
        );
    }

    /// The OTHER direction: FEWER analysis bands than components (1/6-octave components,
    /// 27, vs 1/3-octave analysis, 14 bands). Multiple components map to one band; the
    /// nearest-band mapping still converges.
    #[test]
    fn more_components_than_bands() {
        let fs = 20_000.0;
        let bands = octave_freqs(100.0, 2000.0, OctaveFraction::Third); // 14 bands
        let target = vec![100.0f64; bands.len()];
        let table =
            DampedSineTable::default_over_band(100.0, 2000.0, OctaveFraction::Sixth).unwrap(); // 27
        assert!(
            table.components.len() > bands.len(),
            "precondition: more components than bands"
        );
        let opts = SynthesisOpts {
            max_iters: 80,
            tol_db: 1.5,
            max_gain_db_per_iter: 3.0,
            q: 10.0,
        };
        let out = synthesize_to_target(&target, &bands, fs, SynthesisEngine::Seds(table), opts);
        let (worst, _m) = worst_median(&out.per_band_error_db);
        assert!(
            out.converged,
            "more-components case did not converge (worst {worst:.3} dB)"
        );
        assert!(
            worst <= 3.0,
            "more-components worst {worst:.3} dB exceeds 3 dB"
        );
    }
}

// ===========================================================================
// Import round-trips (TSV / CSV / clipboard) + JSON round-trip.
// ===========================================================================
mod import_and_json {
    use super::*;

    #[test]
    fn wavelet_tsv_with_header_aliases() {
        let src = "\
# a wavelet component table
freq\tamp\thalf-cycles\tdelay
100\t1.0\t3\t0.0
200\t2.5\t5\t0.001
// a comment line
400\t-1.5\t7\t0.002
";
        let t = WaveletTable::from_tsv(src).unwrap();
        assert_eq!(t.components.len(), 3);
        assert_eq!(t.components[0].freq_hz, 100.0);
        assert_eq!(t.components[1].amplitude_g, 2.5);
        assert_eq!(t.components[1].half_cycles, 5);
        assert_eq!(t.components[2].amplitude_g, -1.5);
        assert_eq!(t.components[2].delay_s, 0.002);
    }

    #[test]
    fn wavelet_csv_rfc4180_and_positional() {
        let src = "100,1.0,3,0\n250,2.0,5,0.001\n";
        let t = WaveletTable::from_csv(src).unwrap();
        assert_eq!(t.components.len(), 2);
        assert_eq!(t.components[1].freq_hz, 250.0);
        assert_eq!(t.components[1].half_cycles, 5);
        let q = "\"freq\",\"amp\",\"n\"\n\"100\",\"1.0\",\"3\"\n";
        let t2 = WaveletTable::from_csv(q).unwrap();
        assert_eq!(t2.components[0].freq_hz, 100.0);
        assert_eq!(t2.components[0].half_cycles, 3);
    }

    #[test]
    fn wavelet_missing_optional_columns_default() {
        let src = "hz\tg\n100\t1.0\n200\t2.0\n";
        let t = WaveletTable::from_tsv(src).unwrap();
        assert_eq!(
            t.components[0].half_cycles,
            crate::table::DEFAULT_WAVELET_HALF_CYCLES
        );
        assert_eq!(t.components[0].delay_s, 0.0);
    }

    #[test]
    fn damped_sine_clipboard_autodetect_delimiters() {
        let src =
            "frequency;amplitude;damping;delay;polarity\n100;1.0;0.05;0;+\n300;2.0;0.08;0.001;-\n";
        let t = DampedSineTable::paste_from_clipboard(src).unwrap();
        assert_eq!(t.components.len(), 2);
        assert_eq!(t.components[0].zeta, 0.05);
        assert_eq!(t.components[0].polarity, Polarity::Positive);
        assert_eq!(t.components[1].polarity, Polarity::Negative);
    }

    #[test]
    fn damped_sine_whitespace_positional() {
        let src = "100   1.0   0.05   0.0\n300   2.0   0.08   0.001\n";
        let t = DampedSineTable::paste_from_clipboard(src).unwrap();
        assert_eq!(t.components.len(), 2);
        assert_eq!(t.components[1].freq_hz, 300.0);
        assert_eq!(t.components[1].zeta, 0.08);
        assert_eq!(t.components[1].polarity, Polarity::Positive);
    }

    #[test]
    fn damped_sine_numeric_polarity_sign() {
        let src = "freq,amp,zeta,delay,sign\n100,1.0,0.05,0,1\n200,1.0,0.05,0,-1\n";
        let t = DampedSineTable::from_csv(src).unwrap();
        assert_eq!(t.components[0].polarity, Polarity::Positive);
        assert_eq!(t.components[1].polarity, Polarity::Negative);
    }

    /// Approximate f64 equality to `serde_json`'s shortest-representation precision
    /// (a re-parse may differ by up to ~1 ULP, i.e. ~1e-13 relative; see the round-trip
    /// note on `WaveletTable::to_json`).
    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-12 * a.abs().max(b.abs()).max(1.0)
    }

    #[test]
    fn wavelet_json_round_trip() {
        let t = WaveletTable::default_over_band(100.0, 2000.0, OctaveFraction::Third).unwrap();
        let json = t.to_json().unwrap();
        let back = WaveletTable::from_json(&json).unwrap();
        // Structure is preserved exactly; float values to shortest-repr precision.
        assert_eq!(t.components.len(), back.components.len());
        for (x, y) in t.components.iter().zip(&back.components) {
            assert!(
                approx(x.freq_hz, y.freq_hz),
                "freq {} vs {}",
                x.freq_hz,
                y.freq_hz
            );
            assert!(approx(x.amplitude_g, y.amplitude_g));
            assert_eq!(
                x.half_cycles, y.half_cycles,
                "half-cycles are integral (exact)"
            );
            assert!(
                approx(x.delay_s, y.delay_s),
                "delay {} vs {}",
                x.delay_s,
                y.delay_s
            );
        }
        // The JSON is IDEMPOTENT after the first parse: re-serializing `back` and
        // re-parsing it yields a byte-identical string. (The very first serialize of a
        // freshly-computed f64 may pick a shortest-repr string that re-parses one ULP
        // off, so `json` itself is not guaranteed byte-identical to `back`'s
        // serialization; what matters for a persisted table is that once written and
        // read back, it is stable.)
        let json2 = back.to_json().unwrap();
        let back2 = WaveletTable::from_json(&json2).unwrap();
        let json3 = back2.to_json().unwrap();
        assert_eq!(
            json2, json3,
            "JSON must be idempotent after the first round-trip"
        );
    }

    #[test]
    fn damped_sine_json_round_trip() {
        let t = DampedSineTable::default_over_band(100.0, 2000.0, OctaveFraction::Third).unwrap();
        let json = t.to_json().unwrap();
        let back = DampedSineTable::from_json(&json).unwrap();
        assert_eq!(t.components.len(), back.components.len());
        for (x, y) in t.components.iter().zip(&back.components) {
            assert!(
                approx(x.freq_hz, y.freq_hz),
                "freq {} vs {}",
                x.freq_hz,
                y.freq_hz
            );
            assert!(approx(x.amplitude_g, y.amplitude_g));
            assert!(approx(x.zeta, y.zeta));
            assert!(approx(x.delay_s, y.delay_s));
            assert_eq!(x.polarity, y.polarity, "polarity is exact");
        }
        // Idempotent after the first parse (see the wavelet round-trip test).
        let json2 = back.to_json().unwrap();
        let back2 = DampedSineTable::from_json(&json2).unwrap();
        let json3 = back2.to_json().unwrap();
        assert_eq!(
            json2, json3,
            "JSON must be idempotent after the first round-trip"
        );
    }

    #[test]
    fn imported_table_matches_manual() {
        let src = "freq,amp,n,delay\n100,1.0,5,0\n200,1.0,5,0\n";
        let imported = WaveletTable::from_csv(src).unwrap();
        let manual = WaveletTable {
            components: vec![
                WaveletComponent::new(100.0, 1.0, 5, 0.0).unwrap(),
                WaveletComponent::new(200.0, 1.0, 5, 0.0).unwrap(),
            ],
        };
        assert_eq!(imported, manual);
    }

    #[test]
    fn empty_import_errors() {
        assert!(WaveletTable::from_tsv("# only a comment\n").is_err());
        assert!(DampedSineTable::from_tsv("\n\n").is_err());
    }
}

// ===========================================================================
// Validation (even half-cycles rejected, bad params rejected).
// ===========================================================================
mod validation {
    use super::*;
    use crate::table::ComponentError;

    #[test]
    fn even_half_cycles_rejected() {
        assert!(matches!(
            WaveletComponent::new(100.0, 1.0, 4, 0.0),
            Err(ComponentError::BadHalfCycles(4))
        ));
        assert!(matches!(
            WaveletComponent::new(100.0, 1.0, 6, 0.0),
            Err(ComponentError::BadHalfCycles(6))
        ));
    }

    #[test]
    fn half_cycles_below_minimum_rejected() {
        assert!(matches!(
            WaveletComponent::new(100.0, 1.0, 1, 0.0),
            Err(ComponentError::BadHalfCycles(1))
        ));
        assert!(WaveletComponent::new(100.0, 1.0, 0, 0.0).is_err());
    }

    #[test]
    fn odd_half_cycles_at_least_three_accepted() {
        assert!(WaveletComponent::new(100.0, 1.0, 3, 0.0).is_ok());
        assert!(WaveletComponent::new(100.0, 1.0, 5, 0.0).is_ok());
        assert!(WaveletComponent::new(100.0, 1.0, 101, 0.0).is_ok());
    }

    #[test]
    fn bad_frequency_amplitude_delay_rejected() {
        assert!(matches!(
            WaveletComponent::new(0.0, 1.0, 3, 0.0),
            Err(ComponentError::BadFrequency(_))
        ));
        assert!(matches!(
            WaveletComponent::new(-100.0, 1.0, 3, 0.0),
            Err(ComponentError::BadFrequency(_))
        ));
        assert!(matches!(
            WaveletComponent::new(100.0, f64::NAN, 3, 0.0),
            Err(ComponentError::BadAmplitude(_))
        ));
        assert!(matches!(
            WaveletComponent::new(100.0, 1.0, 3, -0.001),
            Err(ComponentError::BadDelay(_))
        ));
    }

    #[test]
    fn damped_sine_bad_zeta_rejected() {
        assert!(matches!(
            DampedSineComponent::new(100.0, 1.0, 0.0, 0.0, Polarity::Positive),
            Err(ComponentError::BadZeta(_))
        ));
        assert!(matches!(
            DampedSineComponent::new(100.0, 1.0, -0.05, 0.0, Polarity::Positive),
            Err(ComponentError::BadZeta(_))
        ));
    }

    #[test]
    fn import_rejects_even_half_cycles() {
        let src = "freq,amp,n\n100,1.0,4\n";
        assert!(WaveletTable::from_csv(src).is_err());
    }

    #[test]
    fn import_rejects_non_integer_half_cycles() {
        let src = "freq,amp,n\n100,1.0,3.5\n";
        assert!(WaveletTable::from_csv(src).is_err());
    }

    #[test]
    fn table_validate_catches_bad_row() {
        let t = DampedSineTable {
            components: vec![DampedSineComponent {
                freq_hz: 100.0,
                amplitude_g: 1.0,
                zeta: -1.0,
                delay_s: 0.0,
                polarity: Polarity::Positive,
            }],
        };
        assert!(t.validate().is_err());
    }
}

// ===========================================================================
// Determinism (no RNG, no dither).
// ===========================================================================
mod determinism {
    use super::*;

    #[test]
    fn wavsyn_synthesis_is_deterministic() {
        let fs = 20_000.0;
        let t = WaveletTable::default_over_band(100.0, 2000.0, OctaveFraction::Sixth).unwrap();
        let a = wavsyn_composite(&t, fs, 0.05);
        let b = wavsyn_composite(&t, fs, 0.05);
        assert_eq!(a, b, "WAVSYN synthesis must be bit-identical across runs");
    }

    #[test]
    fn seds_synthesis_is_deterministic() {
        let fs = 20_000.0;
        let t = DampedSineTable::default_over_band(100.0, 2000.0, OctaveFraction::Sixth).unwrap();
        let a = seds_compensated(&t, fs);
        let b = seds_compensated(&t, fs);
        assert_eq!(
            a.waveform_g, b.waveform_g,
            "SEDS synthesis must be bit-identical"
        );
        assert_eq!(
            a.compensation, b.compensation,
            "SEDS compensation must be bit-identical"
        );
    }

    #[test]
    fn iteration_is_deterministic() {
        let fs = 20_000.0;
        let bands = canonical_bands();
        let target = vec![100.0f64; bands.len()];
        let mk = || {
            SynthesisEngine::Seds(
                DampedSineTable::default_over_band(100.0, 2000.0, OctaveFraction::Sixth).unwrap(),
            )
        };
        let opts = SynthesisOpts {
            max_iters: 80,
            tol_db: 1.5,
            max_gain_db_per_iter: 3.0,
            q: 10.0,
        };
        let a = synthesize_to_target(&target, &bands, fs, mk(), opts);
        let b = synthesize_to_target(&target, &bands, fs, mk(), opts);
        assert_eq!(a.waveform, b.waveform);
        assert_eq!(a.iters, b.iters);
        assert_eq!(a.per_band_error_db, b.per_band_error_db);
    }
}

// ===========================================================================
// SrsSynthesisLaw (ShockControlLaw seam).
// ===========================================================================
mod control_law {
    use super::*;
    use crate::law::SrsSynthesisLaw;
    use eigen_shock_common::{ShockControlLaw, ShockLawInputs};

    /// Build a `ShockLawInputs` from borrowed slices (helper to keep tests terse).
    fn inputs<'a>(
        target: &'a [f64],
        measured: &'a [f64],
        bands: &'a [f64],
        comp: &'a [f64],
        limit: Option<&'a [Vec<f64>]>,
    ) -> ShockLawInputs<'a> {
        ShockLawInputs {
            target_srs: target,
            measured_srs: measured,
            band_freqs: bands,
            component_freqs: comp,
            predicted_limit_srs: limit,
            current_scale: 1.0,
        }
    }

    #[test]
    fn init_seeds_unity_gains() {
        let comp_freqs = vec![100.0, 200.0, 400.0];
        let mut law = SrsSynthesisLaw::new(comp_freqs.clone()).unwrap();
        let bands = vec![100.0, 200.0, 400.0];
        let target = vec![100.0, 100.0, 100.0];
        let measured = vec![0.0, 0.0, 0.0];
        let out = law.init(&inputs(&target, &measured, &bands, &comp_freqs, None));
        assert_eq!(out.component_gain_updates, vec![1.0, 1.0, 1.0]);
        assert_eq!(out.level_scale_cmd, 1.0);
        assert_eq!(law.name(), "srs-synthesis");
    }

    #[test]
    fn on_pulse_ratio_update_clamped() {
        let comp_freqs = vec![100.0, 200.0, 400.0];
        let mut law = SrsSynthesisLaw::with_clamp(comp_freqs.clone(), 3.0).unwrap();
        let bands = vec![100.0, 200.0, 400.0];
        let target = vec![100.0, 100.0, 100.0];
        let measured = vec![50.0, 100.0, 1000.0];
        let out = law.on_pulse(&inputs(&target, &measured, &bands, &comp_freqs, None));
        let max_lin = 10.0_f64.powf(3.0 / 20.0);
        assert!(
            (out.component_gain_updates[0] - max_lin).abs() < 1e-9,
            "low band clamps up"
        );
        assert!(
            (out.component_gain_updates[1] - 1.0).abs() < 1e-9,
            "mid band holds"
        );
        assert!(
            (out.component_gain_updates[2] - 1.0 / max_lin).abs() < 1e-9,
            "high band clamps down"
        );
        assert_eq!(
            out.diagnostics.clamped_components, 2,
            "two bands hit the clamp"
        );
        assert_eq!(out.level_scale_cmd, 1.0);
    }

    #[test]
    fn on_pulse_component_band_grid_mismatch() {
        let comp_freqs = vec![150.0, 600.0];
        let mut law = SrsSynthesisLaw::new(comp_freqs.clone()).unwrap();
        let bands = vec![100.0, 200.0, 400.0, 800.0];
        let target = vec![100.0, 100.0, 100.0, 100.0];
        let measured = vec![80.0, 90.0, 110.0, 120.0];
        let out = law.on_pulse(&inputs(&target, &measured, &bands, &comp_freqs, None));
        assert_eq!(
            out.component_gain_updates.len(),
            2,
            "one gain per component, not per band"
        );
        // 150 Hz -> nearest band 200 Hz: 100/90 > 1.
        assert!(out.component_gain_updates[0] > 1.0);
        // 600 Hz -> nearest band 800 Hz: 100/120 < 1.
        assert!(out.component_gain_updates[1] < 1.0);
    }

    /// A law built WITHOUT limit bounds has nothing to compare a prediction against
    /// and ignores `predicted_limit_srs` (like `PerBandGainLaw`). The LIMITING
    /// variant is `with_limits` - tested below.
    #[test]
    fn predictions_without_bounds_ignored() {
        let comp_freqs = vec![100.0, 200.0];
        let bands = vec![100.0, 200.0];
        let target = vec![100.0, 100.0];
        let measured = vec![80.0, 120.0];
        let limit = vec![vec![10.0, 10.0]];
        let mut law_a = SrsSynthesisLaw::new(comp_freqs.clone()).unwrap();
        let a = law_a.on_pulse(&inputs(
            &target,
            &measured,
            &bands,
            &comp_freqs,
            Some(&limit),
        ));
        let mut law_b = SrsSynthesisLaw::new(comp_freqs.clone()).unwrap();
        let b = law_b.on_pulse(&inputs(&target, &measured, &bands, &comp_freqs, None));
        assert_eq!(a.component_gain_updates, b.component_gain_updates);
        assert_eq!(law_a.last_limit_status().limited_components, 0);
    }

    #[test]
    fn bad_config_rejected() {
        assert!(SrsSynthesisLaw::new(vec![]).is_err());
        assert!(SrsSynthesisLaw::with_clamp(vec![100.0], 0.0).is_err());
        assert!(SrsSynthesisLaw::with_clamp(vec![100.0], -1.0).is_err());
        // Limiting-variant config errors.
        use crate::law::LawLimits;
        assert!(SrsSynthesisLaw::with_limits(
            vec![100.0],
            3.0,
            LawLimits {
                bounds: vec![],
                headroom_db: 0.0
            }
        )
        .is_err());
        assert!(SrsSynthesisLaw::with_limits(
            vec![100.0],
            3.0,
            LawLimits {
                bounds: vec![vec![]],
                headroom_db: 0.0
            }
        )
        .is_err());
        assert!(SrsSynthesisLaw::with_limits(
            vec![100.0],
            3.0,
            LawLimits {
                bounds: vec![vec![0.0]],
                headroom_db: 0.0
            }
        )
        .is_err());
        assert!(SrsSynthesisLaw::with_limits(
            vec![100.0],
            3.0,
            LawLimits {
                bounds: vec![vec![100.0]],
                headroom_db: -1.0
            }
        )
        .is_err());
        assert!(SrsSynthesisLaw::with_limits(
            vec![100.0],
            3.0,
            LawLimits {
                bounds: vec![vec![100.0, f64::INFINITY]],
                headroom_db: 1.0
            }
        )
        .is_ok());
    }

    /// Limit wiring: a `with_limits` law caps the gain update at the predicted
    /// headroom ratio `allowed / predicted`, and a limit-driven DOWN move is NOT
    /// rate-clamped (limit-down beats the per-iteration dB clamp - the Random
    /// arbitration rule).
    #[test]
    fn with_limits_caps_binding_band_below_rate_clamp() {
        use crate::law::LawLimits;
        let comp_freqs = vec![100.0, 200.0];
        let bands = vec![100.0, 200.0];
        let target = vec![100.0, 100.0];
        // Control is exactly on target -> the control update alone would be 1.0.
        let measured = vec![100.0, 100.0];
        // Channel prediction: fine at band 0 (50 vs allowed 200), 2x over at band 1
        // (400 vs allowed 200).
        let preds = vec![vec![50.0, 400.0]];
        let mut law = SrsSynthesisLaw::with_limits(
            comp_freqs.clone(),
            3.0,
            LawLimits {
                bounds: vec![vec![200.0, 200.0]],
                headroom_db: 0.0,
            },
        )
        .unwrap();
        let out = law.on_pulse(&inputs(
            &target,
            &measured,
            &bands,
            &comp_freqs,
            Some(&preds),
        ));
        assert!(
            (out.component_gain_updates[0] - 1.0).abs() < 1e-12,
            "unconstrained band holds the control update"
        );
        // Cap = allowed/pred = 200/400 = 0.5 (times the fixed down-overcut), BELOW
        // the -3 dB rate clamp (0.708): the limit wins over the rate clamp.
        let overcut = 10.0f64.powf(-crate::law::LIMIT_DOWN_OVERCUT_DB / 20.0);
        assert!(
            (out.component_gain_updates[1] - 0.5 * overcut).abs() < 1e-12,
            "binding band must be capped at allowed/predicted * overcut = {}, got {}",
            0.5 * overcut,
            out.component_gain_updates[1]
        );
        let status = law.last_limit_status();
        assert_eq!(status.limited_components, 1);
        let worst = status.worst_predicted_exceedance_db.unwrap();
        assert!(
            (worst - 20.0 * 2.0f64.log10()).abs() < 1e-9,
            "worst exceedance is 2x = 6.02 dB, got {worst}"
        );
    }

    /// The headroom shrinks the allowed bound: a prediction exactly AT the bound
    /// binds when headroom_db > 0.
    #[test]
    fn with_limits_headroom_applies() {
        use crate::law::LawLimits;
        let comp_freqs = vec![100.0];
        let bands = vec![100.0];
        let target = vec![100.0];
        let measured = vec![100.0];
        let preds = vec![vec![200.0]]; // exactly at the raw bound
        let mut law = SrsSynthesisLaw::with_limits(
            comp_freqs.clone(),
            3.0,
            LawLimits {
                bounds: vec![vec![200.0]],
                headroom_db: 6.0,
            },
        )
        .unwrap();
        let out = law.on_pulse(&inputs(
            &target,
            &measured,
            &bands,
            &comp_freqs,
            Some(&preds),
        ));
        // allowed/pred = 10^(-6/20), times the fixed down-overcut.
        let expect = 10.0f64.powf(-(6.0 + crate::law::LIMIT_DOWN_OVERCUT_DB) / 20.0);
        assert!(
            (out.component_gain_updates[0] - expect).abs() < 1e-12,
            "headroom must shrink the cap to {expect}, got {}",
            out.component_gain_updates[0]
        );
    }

    /// Bounds configured but NO prediction supplied: the safety-monotone fallback -
    /// nothing may be driven UP blind (updates capped at 1.0), and the status says
    /// so.
    #[test]
    fn with_limits_no_prediction_holds_up_moves() {
        use crate::law::LawLimits;
        let comp_freqs = vec![100.0, 200.0];
        let bands = vec![100.0, 200.0];
        let target = vec![100.0, 100.0];
        // Control wants band 0 UP (x2, clamps to +3 dB) and band 1 DOWN.
        let measured = vec![50.0, 200.0];
        let mut law = SrsSynthesisLaw::with_limits(
            comp_freqs.clone(),
            3.0,
            LawLimits {
                bounds: vec![vec![300.0, 300.0]],
                headroom_db: 0.0,
            },
        )
        .unwrap();
        let out = law.on_pulse(&inputs(&target, &measured, &bands, &comp_freqs, None));
        assert!(
            (out.component_gain_updates[0] - 1.0).abs() < 1e-12,
            "up-move must be held without a prediction, got {}",
            out.component_gain_updates[0]
        );
        assert!(
            out.component_gain_updates[1] < 1.0,
            "down-moves still allowed without a prediction"
        );
        assert!(law.last_limit_status().held_no_prediction);
    }

    /// Task-B gate: a closed-loop law run with a BINDING limit converges the control
    /// SRS to the target at unconstrained bands while holding every limit-channel
    /// prediction under its bound (the limited bands settle BELOW target - honest).
    ///
    /// Plant model: the control channel measures the synthesized waveform's own SRS
    /// (identity plant); the limit channel's predicted SRS is the measured SRS times
    /// a resonant per-band bump (peak 5x at 500 Hz) - the band-domain equivalent of
    /// a resonant fixture FRF.
    #[test]
    fn law_with_binding_limit_converges_and_holds_bound() {
        use crate::law::LawLimits;
        let fs = 20_000.0;
        let bands = canonical_bands();
        let target = vec![100.0f64; bands.len()];
        let bump = |f: f64| 1.0 + 4.0 / (1.0 + ((f - 500.0) / 150.0).powi(2));
        let bound = 300.0f64;
        let headroom_db = 0.5;
        let allowed = bound * 10.0f64.powf(-headroom_db / 20.0);

        // Start from the OFFLINE-converged table (the real flow: synthesize first,
        // then run the law against the plant).
        let base = synthesize_to_target(
            &target,
            &bands,
            fs,
            SynthesisEngine::Wavsyn(
                WaveletTable::default_over_band(100.0, 2000.0, OctaveFraction::Sixth).unwrap(),
            ),
            SynthesisOpts {
                max_iters: 80,
                tol_db: 1.5,
                max_gain_db_per_iter: 3.0,
                q: 10.0,
            },
        );
        assert!(base.converged, "precondition: offline synthesis converged");
        let SynthesisEngine::Wavsyn(mut table) = base.components else {
            unreachable!()
        };
        let comp_freqs = table.component_freqs();
        let mut law = SrsSynthesisLaw::with_limits(
            comp_freqs.clone(),
            3.0,
            LawLimits {
                bounds: vec![vec![bound; bands.len()]],
                headroom_db,
            },
        )
        .unwrap();

        let synth = |t: &WaveletTable| {
            let f_lo = t
                .components
                .iter()
                .map(|c| c.freq_hz)
                .fold(f64::INFINITY, f64::min);
            wavsyn_composite(t, fs, 4.0 / f_lo)
        };

        let mut any_limited = false;
        let mut measured = vec![0.0f64; bands.len()];
        for _pulse in 0..12 {
            let w = synth(&table);
            measured = srs_analyze(&w, fs, &bands, 10.0);
            let preds: Vec<Vec<f64>> = vec![measured
                .iter()
                .zip(&bands)
                .map(|(&m, &f)| m * bump(f))
                .collect()];
            let out = law.on_pulse(&inputs(
                &target,
                &measured,
                &bands,
                &comp_freqs,
                Some(&preds),
            ));
            if law.last_limit_status().limited_components > 0 {
                any_limited = true;
            }
            for (c, g) in table.components.iter_mut().zip(&out.component_gain_updates) {
                c.amplitude_g *= g;
            }
        }
        assert!(any_limited, "the limit must actually bind during the run");

        // Final state: every limit prediction under the bound...
        let mut worst_pred_over_db = f64::NEG_INFINITY;
        for (b, &f) in bands.iter().enumerate() {
            let pred = measured[b] * bump(f);
            worst_pred_over_db = worst_pred_over_db.max(20.0 * (pred / bound).log10());
        }
        assert!(
            worst_pred_over_db <= 0.0,
            "limit prediction must end under the bound (worst {worst_pred_over_db:.2} dB over)"
        );
        // ...unconstrained bands (where the bound cannot bind even at full target)
        // still match the target...
        for (b, &f) in bands.iter().enumerate() {
            if bump(f) * target[b] < 0.8 * allowed {
                let err_db = 20.0 * (measured[b] / target[b]).log10();
                assert!(
                    err_db.abs() <= 3.0,
                    "unconstrained band {f:.0} Hz off target by {err_db:.2} dB"
                );
            }
        }
        // ...and the binding bands settled BELOW target (the honest trade).
        let peak_band = bands
            .iter()
            .position(|&f| (f - 500.0).abs() < 30.0)
            .expect("a band near 500 Hz exists");
        assert!(
            measured[peak_band] < target[peak_band],
            "the limited band must sit below target ({} vs {})",
            measured[peak_band],
            target[peak_band]
        );
    }

    /// Applying the law's returned gains to a real SEDS drive and re-analyzing reduces
    /// the worst per-band error (an end-to-end run-time-counterpart smoke test).
    #[test]
    fn law_applied_to_measured_srs_reduces_error() {
        let fs = 20_000.0;
        let bands = canonical_bands();
        let target = vec![100.0f64; bands.len()];
        let mut table =
            DampedSineTable::default_over_band(100.0, 2000.0, OctaveFraction::Sixth).unwrap();
        let comp_freqs = table.component_freqs();
        let mut law = SrsSynthesisLaw::new(comp_freqs.clone()).unwrap();

        let w0 = seds_compensated(&table, fs).waveform_g;
        let meas0 = srs_analyze(&w0, fs, &bands, 10.0);
        let errs0: Vec<f64> = meas0
            .iter()
            .zip(&target)
            .map(|(m, t)| 20.0 * (m / t).log10())
            .collect();
        let (err0, _) = worst_median(&errs0);

        let out = law.on_pulse(&inputs(&target, &meas0, &bands, &comp_freqs, None));
        for (c, g) in table.components.iter_mut().zip(&out.component_gain_updates) {
            c.amplitude_g *= g;
        }
        let w1 = seds_compensated(&table, fs).waveform_g;
        let meas1 = srs_analyze(&w1, fs, &bands, 10.0);
        let errs1: Vec<f64> = meas1
            .iter()
            .zip(&target)
            .map(|(m, t)| 20.0 * (m / t).log10())
            .collect();
        let (err1, _) = worst_median(&errs1);
        assert!(
            err1 < err0,
            "one law step should reduce worst error ({err0:.3} -> {err1:.3} dB)"
        );
    }
}

// ===========================================================================
// PREDICTIVE-NOTCH GATES: response limiting (SAND2016-1335C).
// ===========================================================================
mod predictive_notch {
    use super::*;
    use crate::iterate::synthesize_to_target_capped;
    use crate::limit::{predict_response_srs, LimitChannel, PredictiveNotch};
    use crate::wavsyn::wavsyn_component;
    use eigen_dsp::fft;

    /// The resonant fixture FRF: |H(f)| = 1 + 4 * lorentzian(500 Hz, half-width
    /// 150 Hz) - a 5x bump at 500 Hz on a unity floor (mirrors the Random
    /// overlapping-resonance fixture shape).
    fn bump_frf() -> (Vec<f64>, Vec<f64>) {
        let freqs: Vec<f64> = (0..=400).map(|i| i as f64 * 25.0).collect(); // 0..10 kHz
        let mags: Vec<f64> = freqs
            .iter()
            .map(|&f| 1.0 + 4.0 / (1.0 + ((f - 500.0) / 150.0).powi(2)))
            .collect();
        (freqs, mags)
    }

    /// The shared predictive-notch fixture: an offline-converged WAVSYN drive (1/3-octave
    /// components vs the canonical 1/6-octave analysis grid - counts DIFFER), its
    /// per-component waveforms, and the band grid.
    fn converged_third_octave_fixture() -> (WaveletTable, Vec<Vec<f64>>, Vec<f64>, Vec<f64>, f64) {
        let fs = 20_000.0;
        let bands = canonical_bands(); // 27 bands (1/6-octave)
        let target = vec![100.0f64; bands.len()];
        let table = WaveletTable::default_over_band(100.0, 2000.0, OctaveFraction::Third).unwrap();
        let out = synthesize_to_target(
            &target,
            &bands,
            fs,
            SynthesisEngine::Wavsyn(table),
            SynthesisOpts {
                max_iters: 80,
                tol_db: 3.0,
                max_gain_db_per_iter: 3.0,
                q: 10.0,
            },
        );
        assert!(out.converged, "precondition: base synthesis converged");
        let SynthesisEngine::Wavsyn(table) = out.components else {
            unreachable!()
        };
        let n = out.waveform.len();
        let comps: Vec<Vec<f64>> = table
            .components
            .iter()
            .map(|c| wavsyn_component(c, fs, n))
            .collect();
        // Sanity: the component superposition IS the drive (the LS re-fit's model).
        let mut sum = vec![0.0f64; n];
        for w in &comps {
            for (s, v) in sum.iter_mut().zip(w) {
                *s += v;
            }
        }
        let max_dev = sum
            .iter()
            .zip(&out.waveform)
            .fold(0.0f64, |m, (&a, &b)| m.max((a - b).abs()));
        assert!(
            max_dev < 1e-9,
            "component superposition must equal the drive (dev {max_dev})"
        );
        (table, comps, bands, target, fs)
    }

    /// GATE (i): a limit channel with a resonant FRF bump overlapping a band the
    /// target wants filled -> the notch reduces the offending components, the
    /// predicted limit-channel SRS drops under its limit, and the control SRS still
    /// matches the target away from the notch (via the capped re-fit).
    #[test]
    fn resonant_bump_notches_and_control_matches_elsewhere() {
        let (table, comps, bands, target, fs) = converged_third_octave_fixture();
        let (frf_freqs, frf_mag) = bump_frf();
        let limit_curve = 300.0f64;
        let notch = PredictiveNotch {
            limit_channels: vec![LimitChannel {
                name: "fixture_resp".into(),
                frf_mag: frf_mag.clone(),
                frf_freqs: frf_freqs.clone(),
                limit_srs: vec![limit_curve; bands.len()],
            }],
            headroom_db: 1.0,
        };
        let outcome = notch.plan(&comps, fs, &bands, 10.0).unwrap();
        let report = &outcome.report;

        // The bump makes the prediction exceed: ~100 g * 5 = 500 g vs allowed
        // ~267 g -> a few dB over. (Measured fixture numbers: +4.65 dB worst
        // exceedance before, -0.01 dB after, 7 refine iterations, LS re-fit
        // residual 0.4%, 6 bands notched; the 500 Hz component cut to gain 0.49,
        // out-of-bump components >= 0.999.)
        let before = report.worst_exceedance_before_db.unwrap();
        assert!(
            before > 2.0,
            "fixture must start in exceedance (got {before:.2} dB)"
        );
        assert!(!report.notched_bands.is_empty());
        for nb in &report.notched_bands {
            assert!(
                nb.band_freq_hz > 250.0 && nb.band_freq_hz < 1100.0,
                "exceedances must cluster around the 500 Hz bump, got {} Hz",
                nb.band_freq_hz
            );
        }
        assert!(
            report.converged,
            "notch must converge (after {:?} dB, iters {})",
            report.worst_exceedance_after_db, report.iterations
        );
        assert!(report.worst_exceedance_after_db.unwrap() <= 0.0);

        // The offending components (near the bump) came down; far components did
        // not (the notch is local, not a global level cut).
        for (j, c) in table.components.iter().enumerate() {
            let g = outcome.component_gains[j];
            assert!((0.0..=1.0).contains(&g), "gain {g} out of [0,1]");
            if (c.freq_hz - 500.0).abs() < 150.0 {
                assert!(
                    g < 0.9,
                    "component at {} Hz must be notched (gain {g})",
                    c.freq_hz
                );
            }
            if !(250.0..=1100.0).contains(&c.freq_hz) {
                assert!(
                    g > 0.9,
                    "component at {} Hz should be nearly untouched (gain {g})",
                    c.freq_hz
                );
            }
        }

        // Independent verification: the notched drive's predicted response SRS is
        // under the FULL limit at every band.
        let mut notched_table = table.clone();
        for (c, &g) in notched_table
            .components
            .iter_mut()
            .zip(&outcome.component_gains)
        {
            c.amplitude_g *= g;
        }
        let f_lo = notched_table
            .components
            .iter()
            .map(|c| c.freq_hz)
            .fold(f64::INFINITY, f64::min);
        let notched_drive = wavsyn_composite(&notched_table, fs, 4.0 / f_lo);
        let pred = predict_response_srs(&notched_drive, fs, &frf_freqs, &frf_mag, &bands, 10.0);
        for (b, &p) in pred.iter().enumerate() {
            assert!(
                p <= limit_curve,
                "band {:.0} Hz predicted {p:.1} g over the {limit_curve} g limit",
                bands[b]
            );
        }

        // Arbitration (the Random rule): re-run the SRS-matching loop WITH the notch caps
        // - it may NOT re-inflate the notched components, so the prediction stays
        // under the limit, while the control SRS still matches the target at bands
        // away from the notch.
        let current_amps: Vec<f64> = table.components.iter().map(|c| c.amplitude_g).collect();
        let caps = outcome.amplitude_caps(&current_amps);
        let refit = synthesize_to_target_capped(
            &target,
            &bands,
            fs,
            SynthesisEngine::Wavsyn(notched_table),
            SynthesisOpts {
                max_iters: 40,
                tol_db: 3.0,
                max_gain_db_per_iter: 3.0,
                q: 10.0,
            },
            Some(&caps),
        );
        let SynthesisEngine::Wavsyn(refit_table) = &refit.components else {
            unreachable!()
        };
        for (j, c) in refit_table.components.iter().enumerate() {
            assert!(
                c.amplitude_g.abs() <= caps[j] + 1e-12,
                "capped component {j} re-inflated past its cap"
            );
        }
        let pred2 = predict_response_srs(&refit.waveform, fs, &frf_freqs, &frf_mag, &bands, 10.0);
        for (b, &p) in pred2.iter().enumerate() {
            assert!(
                p <= limit_curve,
                "after the capped re-fit, band {:.0} Hz predicted {p:.1} g over the limit",
                bands[b]
            );
        }
        // Control match away from the notch: |error| <= 3 dB outside the bump.
        for (b, &f) in bands.iter().enumerate() {
            if !(250.0..=1100.0).contains(&f) {
                let err = refit.per_band_error_db[b];
                assert!(
                    err.abs() <= 3.0,
                    "control SRS at {f:.0} Hz off target by {err:.2} dB after limiting"
                );
            }
        }
    }

    /// GATE (ii): component count != band count != FFT bin count - the plan makes no
    /// 1:1 assumption anywhere; the LS re-fit is the proof.
    #[test]
    fn all_three_grids_differ() {
        let (table, comps, bands, _target, fs) = converged_third_octave_fixture();
        let ncomp = table.components.len();
        let nbands = bands.len();
        let nbins = fft::next_pow2(comps[0].len()) / 2 + 1;
        assert_ne!(ncomp, nbands, "components vs bands must differ");
        assert_ne!(nbands, nbins, "bands vs FFT bins must differ");
        assert_ne!(ncomp, nbins, "components vs FFT bins must differ");

        let (frf_freqs, frf_mag) = bump_frf();
        let notch = PredictiveNotch {
            limit_channels: vec![LimitChannel {
                name: "resp".into(),
                frf_mag,
                frf_freqs,
                limit_srs: vec![300.0; nbands],
            }],
            headroom_db: 1.0,
        };
        let outcome = notch.plan(&comps, fs, &bands, 10.0).unwrap();
        assert_eq!(
            outcome.component_gains.len(),
            ncomp,
            "one gain per COMPONENT (not per band, not per bin)"
        );
        assert!(outcome.report.converged);
        // The LS re-fit spreads the notch across the overlapping components: more
        // than one component moves (a per-line copy could not even produce a
        // component-shaped answer here).
        let moved = outcome
            .component_gains
            .iter()
            .filter(|&&g| g < 0.999)
            .count();
        assert!(
            moved >= 2,
            "the re-fit should reduce multiple overlapping components (moved {moved})"
        );
        assert!(
            outcome.report.refit_residual_rel < 0.5,
            "the component basis should express the notch reasonably (residual {})",
            outcome.report.refit_residual_rel
        );
    }

    /// GATE (iii): no limit exceedance -> the notch is a NO-OP: gains all exactly
    /// 1.0, nothing notched, zero refine iterations.
    #[test]
    fn no_exceedance_is_noop() {
        let (_table, comps, bands, _target, fs) = converged_third_octave_fixture();
        let (frf_freqs, frf_mag) = bump_frf();
        let notch = PredictiveNotch {
            limit_channels: vec![LimitChannel {
                name: "resp".into(),
                frf_mag,
                frf_freqs,
                limit_srs: vec![10_000.0; bands.len()], // far above any prediction
            }],
            headroom_db: 1.0,
        };
        let outcome = notch.plan(&comps, fs, &bands, 10.0).unwrap();
        assert!(
            outcome.component_gains.iter().all(|&g| g == 1.0),
            "no-op must leave every gain at exactly 1.0"
        );
        assert!(outcome.report.notched_bands.is_empty());
        assert_eq!(outcome.report.iterations, 0);
        assert!(outcome.report.converged);
        assert!(outcome.report.worst_exceedance_before_db.unwrap() < 0.0);
    }

    /// Two limit channels: the DEEPEST notch wins per bin (safety-monotone
    /// arbitration across channels). A flat second channel with a low flat limit
    /// forces the whole drive down, on top of the resonant channel's local notch.
    #[test]
    fn multiple_channels_deepest_notch_wins() {
        let (_table, comps, bands, _target, fs) = converged_third_octave_fixture();
        let (frf_freqs, frf_mag) = bump_frf();
        let flat_limit = 80.0f64; // control SRS ~100 g -> everything exceeds via |H|=1
        let notch = PredictiveNotch {
            limit_channels: vec![
                LimitChannel {
                    name: "resonant".into(),
                    frf_mag: frf_mag.clone(),
                    frf_freqs: frf_freqs.clone(),
                    limit_srs: vec![300.0; bands.len()],
                },
                LimitChannel {
                    name: "flat_tight".into(),
                    frf_mag: vec![1.0, 1.0],
                    frf_freqs: vec![0.0, 10_000.0],
                    limit_srs: vec![flat_limit; bands.len()],
                },
            ],
            headroom_db: 0.5,
        };
        let outcome = notch.plan(&comps, fs, &bands, 10.0).unwrap();
        assert!(outcome.report.converged, "multi-channel plan must converge");
        // Both channels' bands appear in the notch list.
        assert!(outcome
            .report
            .notched_bands
            .iter()
            .any(|nb| nb.channel == "resonant"));
        assert!(outcome
            .report
            .notched_bands
            .iter()
            .any(|nb| nb.channel == "flat_tight"));
        // The flat channel forces EVERY component down (not just the bump).
        assert!(
            outcome.component_gains.iter().all(|&g| g < 1.0),
            "the flat tight limit must pull every component down"
        );
        // Verify both channels end under their limits on the notched drive.
        let n = comps[0].len();
        let mut drive = vec![0.0f64; n];
        for (w, &g) in comps.iter().zip(&outcome.component_gains) {
            for (d, v) in drive.iter_mut().zip(w) {
                *d += g * v;
            }
        }
        let pred_res = predict_response_srs(&drive, fs, &frf_freqs, &frf_mag, &bands, 10.0);
        assert!(pred_res.iter().all(|&p| p <= 300.0));
        let pred_flat =
            predict_response_srs(&drive, fs, &[0.0, 10_000.0], &[1.0, 1.0], &bands, 10.0);
        assert!(pred_flat.iter().all(|&p| p <= flat_limit));
    }

    /// The capped iteration honors caps standalone: a cap on the mid-band
    /// components is never exceeded even though the target asks for more.
    #[test]
    fn capped_iteration_cannot_reinflate() {
        let fs = 20_000.0;
        let bands = canonical_bands();
        let target = vec![100.0f64; bands.len()];
        let table = WaveletTable::default_over_band(100.0, 2000.0, OctaveFraction::Sixth).unwrap();
        // Cap the components near 500 Hz at HALF their converged demand; leave the
        // rest uncapped (infinite cap).
        let caps: Vec<f64> = table
            .components
            .iter()
            .map(|c| {
                if (c.freq_hz - 500.0).abs() < 100.0 {
                    0.02 // far below what a 100 g target demands
                } else {
                    f64::INFINITY
                }
            })
            .collect();
        let out = synthesize_to_target_capped(
            &target,
            &bands,
            fs,
            SynthesisEngine::Wavsyn(table),
            SynthesisOpts {
                max_iters: 40,
                tol_db: 1.5,
                max_gain_db_per_iter: 3.0,
                q: 10.0,
            },
            Some(&caps),
        );
        let SynthesisEngine::Wavsyn(t) = &out.components else {
            unreachable!()
        };
        for (j, c) in t.components.iter().enumerate() {
            assert!(
                c.amplitude_g.abs() <= caps[j] + 1e-12,
                "component {j} ({} Hz) exceeded its cap: {} > {}",
                c.freq_hz,
                c.amplitude_g.abs(),
                caps[j]
            );
        }
        // The capped run cannot converge the capped bands - honest outcome.
        assert!(
            !out.converged,
            "a binding cap must show up as non-convergence on the capped bands"
        );
    }

    /// The amplitude-caps helper is gains * |amps|.
    #[test]
    fn amplitude_caps_helper() {
        let (_t, comps, bands, _target, fs) = converged_third_octave_fixture();
        let (frf_freqs, frf_mag) = bump_frf();
        let notch = PredictiveNotch {
            limit_channels: vec![LimitChannel {
                name: "resp".into(),
                frf_mag,
                frf_freqs,
                limit_srs: vec![300.0; bands.len()],
            }],
            headroom_db: 1.0,
        };
        let outcome = notch.plan(&comps, fs, &bands, 10.0).unwrap();
        let amps = vec![2.0; outcome.component_gains.len()];
        let caps = outcome.amplitude_caps(&amps);
        for (cap, g) in caps.iter().zip(&outcome.component_gains) {
            assert!((cap - 2.0 * g).abs() < 1e-12);
        }
    }

    /// Config/input validation errors.
    #[test]
    fn validation_errors() {
        let ch = |frf_freqs: Vec<f64>, frf_mag: Vec<f64>, limit: Vec<f64>| LimitChannel {
            name: "c".into(),
            frf_mag,
            frf_freqs,
            limit_srs: limit,
        };
        let bands = vec![100.0, 200.0];
        // FRF grid length mismatch.
        assert!(PredictiveNotch {
            limit_channels: vec![ch(vec![0.0, 100.0], vec![1.0], vec![10.0, 10.0])],
            headroom_db: 0.0
        }
        .validate(bands.len())
        .is_err());
        // Non-ascending FRF grid.
        assert!(PredictiveNotch {
            limit_channels: vec![ch(vec![100.0, 0.0], vec![1.0, 1.0], vec![10.0, 10.0])],
            headroom_db: 0.0
        }
        .validate(bands.len())
        .is_err());
        // Limit curve length != band count.
        assert!(PredictiveNotch {
            limit_channels: vec![ch(vec![0.0, 100.0], vec![1.0, 1.0], vec![10.0])],
            headroom_db: 0.0
        }
        .validate(bands.len())
        .is_err());
        // Non-positive limit.
        assert!(PredictiveNotch {
            limit_channels: vec![ch(vec![0.0, 100.0], vec![1.0, 1.0], vec![10.0, 0.0])],
            headroom_db: 0.0
        }
        .validate(bands.len())
        .is_err());
        // Negative headroom.
        assert!(PredictiveNotch {
            limit_channels: vec![ch(vec![0.0, 100.0], vec![1.0, 1.0], vec![10.0, 10.0])],
            headroom_db: -1.0
        }
        .validate(bands.len())
        .is_err());
        // Infinite limit entries are legal ("no limit at this band").
        assert!(PredictiveNotch {
            limit_channels: vec![ch(
                vec![0.0, 100.0],
                vec![1.0, 1.0],
                vec![10.0, f64::INFINITY]
            )],
            headroom_db: 0.0
        }
        .validate(bands.len())
        .is_ok());

        // Runtime input errors.
        let good = PredictiveNotch {
            limit_channels: vec![ch(vec![0.0, 100.0], vec![1.0, 1.0], vec![10.0, 10.0])],
            headroom_db: 0.0,
        };
        assert!(good.plan(&[], 100.0, &bands, 10.0).is_err()); // no components
        assert!(good
            .plan(&[vec![1.0; 8], vec![1.0; 9]], 100.0, &bands, 10.0)
            .is_err()); // mismatched lengths
        assert!(good.plan(&[vec![1.0; 8]], 0.0, &bands, 10.0).is_err()); // bad fs
        assert!(good.plan(&[vec![1.0; 8]], 100.0, &[], 10.0).is_err()); // no bands
        assert!(good.plan(&[vec![1.0; 8]], 100.0, &bands, 0.0).is_err()); // bad q
    }

    /// Determinism: the plan is a pure function of its inputs.
    #[test]
    fn plan_is_deterministic() {
        let (_t, comps, bands, _target, fs) = converged_third_octave_fixture();
        let (frf_freqs, frf_mag) = bump_frf();
        let notch = PredictiveNotch {
            limit_channels: vec![LimitChannel {
                name: "resp".into(),
                frf_mag,
                frf_freqs,
                limit_srs: vec![300.0; bands.len()],
            }],
            headroom_db: 1.0,
        };
        let a = notch.plan(&comps, fs, &bands, 10.0).unwrap();
        let b = notch.plan(&comps, fs, &bands, 10.0).unwrap();
        assert_eq!(a.component_gains, b.component_gains);
        assert_eq!(a.report, b.report);
    }
}

// ===========================================================================
// The OFFLINE PREVIEW (operator acceptance): target -> synthesize -> look,
// with no plant, no station and no run.
// ===========================================================================
mod preview_offline {
    use super::*;
    use crate::preview::synthesize_preview;
    use crate::service::config::{BandFraction, EngineKind, SrsTestConfig, TargetSrs};

    /// The canonical preview config: flat 50 g, 100-2000 Hz, 1/3-octave, WAVSYN.
    fn cfg() -> SrsTestConfig {
        let mut c = SrsTestConfig::flat(100.0, 2000.0, 50.0);
        c.fraction = BandFraction::Third;
        c.synthesis.max_iters = 20;
        c.synthesis.tol_db = 1.0;
        c
    }

    /// The preview runs with NO plant of any kind in scope and produces the four
    /// things the operator screen draws: a band grid, a target on it, an achieved
    /// SRS on it, and a time history with matching velocity/displacement traces.
    #[test]
    fn preview_needs_no_plant_and_returns_the_whole_screen() {
        let fs = 51_200.0;
        let p = synthesize_preview(&cfg(), fs).unwrap();

        let nb = p.band_freqs_hz.len();
        assert!(
            nb > 5,
            "1/3-octave 100-2000 Hz should give many bands, got {nb}"
        );
        assert_eq!(p.target_g.len(), nb);
        assert_eq!(p.achieved_srs_g.len(), nb);
        assert_eq!(p.per_band_error_db.len(), nb);
        assert!(p.target_g.iter().all(|&t| (t - 50.0).abs() < 1e-9));

        assert!(!p.waveform_g.is_empty());
        assert_eq!(p.velocity_ips.len(), p.waveform_g.len());
        assert_eq!(p.disp_in.len(), p.waveform_g.len());
        assert!((p.duration_s - p.waveform_g.len() as f64 / fs).abs() < 1e-12);
        assert_eq!(p.fs_hz, fs);
        assert_eq!(p.engine, "Wavsyn");

        // One component per band, and the iteration moved them off the unit seed.
        assert_eq!(p.component_freqs_hz.len(), nb);
        assert_eq!(p.component_amplitudes_g.len(), nb);
        assert!(p
            .component_amplitudes_g
            .iter()
            .any(|&a| (a - 1.0).abs() > 0.1));
    }

    /// It CONVERGES: the achieved SRS matches the flat target, and the reported
    /// worst band agrees with the per-band error array it came from.
    #[test]
    fn preview_converges_and_reports_its_own_worst_band() {
        let p = synthesize_preview(&cfg(), 51_200.0).unwrap();
        assert!(
            p.converged,
            "flat target did not converge in {} iters",
            p.iters
        );
        let (worst, _median) = worst_median(&p.per_band_error_db);
        assert!(worst <= 1.0, "worst band error {worst:.2} dB over tol");

        let w = p.worst_err_db.expect("a comparable band");
        assert!((w.abs() - worst).abs() < 1e-12, "{w} vs {worst}");
        let b = p.worst_band.expect("a worst band index");
        assert_eq!(p.per_band_error_db[b], w);
    }

    /// The peaks card's three numbers are the pure-integrator ones, and the WAVSYN
    /// composite self-nulls - the property that makes it the safe default. The
    /// residuals are what a display would otherwise have to take on faith.
    #[test]
    fn preview_kinematics_are_the_pure_integrator_and_wavsyn_self_nulls() {
        let fs = 51_200.0;
        let p = synthesize_preview(&cfg(), fs).unwrap();
        let k = pulse_kinematics(&p.waveform_g, fs);

        assert_eq!(p.kinematics.peak_accel_g, k.peak_accel_g);
        assert_eq!(p.kinematics.peak_velocity_ips, k.peak_velocity_ips);
        assert_eq!(p.kinematics.pk_pk_disp_in, k.pk_pk_disp_in);
        assert_eq!(p.kinematics.peak_disp_in, k.peak_disp_in);
        assert_eq!(p.kinematics.min_disp_in, k.min_disp_in);

        assert!(p.kinematics.peak_accel_g > 0.0);
        assert!(p.kinematics.peak_velocity_ips > 0.0);
        assert!(p.kinematics.pk_pk_disp_in > 0.0);

        // Gate (i)'s property, seen through the preview the operator reads.
        let rv = p.kinematics.residual_velocity_ips.abs() / p.kinematics.peak_velocity_ips;
        assert!(rv < 1e-9, "wavsyn residual velocity ratio {rv:.2e}");
    }

    /// SEDS previews too, and says which engine ran (the card labels it).
    #[test]
    fn preview_runs_the_seds_engine_and_names_it() {
        let mut c = cfg();
        c.engine = EngineKind::Seds;
        let p = synthesize_preview(&c, 51_200.0).unwrap();
        assert_eq!(p.engine, "Seds");
        assert!(!p.waveform_g.is_empty());
        assert!(p.kinematics.peak_accel_g > 0.0);
    }

    /// A breakpoint target is resolved LOG-LOG onto the grid before synthesis, so a
    /// sloped spec previews as a slope rather than as its first corner.
    #[test]
    fn preview_resolves_a_breakpoint_target_onto_the_grid() {
        let mut c = cfg();
        c.target = TargetSrs::Breakpoints {
            points: vec![(100.0, 30.0), (1000.0, 80.0), (2100.0, 40.0)],
        };
        let p = synthesize_preview(&c, 51_200.0).unwrap();
        let first = p.target_g[0];
        let last = *p.target_g.last().unwrap();
        let peak = p.target_g.iter().cloned().fold(0.0f64, f64::max);
        assert!((first - 30.0).abs() < 1e-6, "first band {first}");
        assert!(
            peak > 70.0,
            "the 1 kHz corner never reached 80 g (peak {peak})"
        );
        // A slope, not a step: the grid rises to the 1 kHz corner and falls after
        // it. A pre-interpolated table pasted onto the wrong grid would be flat.
        assert!(last < peak, "the target should fall past the 1 kHz corner");
        assert!(first < peak, "the target should rise to the 1 kHz corner");
    }

    /// A rate that cannot carry the band is REFUSED, not previewed. Reporting band
    /// levels a rate cannot represent would be worse than showing nothing - the
    /// same check the run service applies to the fire path's rate.
    #[test]
    fn preview_refuses_a_rate_below_the_band_top() {
        // f_hi = 2000 Hz needs fs > 4000 Hz.
        let err = synthesize_preview(&cfg(), 4000.0).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Nyquist"), "unhelpful refusal: {msg}");
        assert!(synthesize_preview(&cfg(), 0.0).is_err());
        assert!(synthesize_preview(&cfg(), f64::NAN).is_err());
        // And a config the validator rejects never reaches the iteration.
        let mut bad = cfg();
        bad.q = 0.1;
        assert!(synthesize_preview(&bad, 51_200.0).is_err());
    }

    /// Determinism: a preview is a pure function of (config, rate). This is what
    /// lets the card be redrawn without re-running, and a golden test pin it.
    #[test]
    fn preview_is_deterministic() {
        let a = synthesize_preview(&cfg(), 51_200.0).unwrap();
        let b = synthesize_preview(&cfg(), 51_200.0).unwrap();
        assert_eq!(a, b);
    }
}
