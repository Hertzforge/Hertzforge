//! ShockControlLaw seam + PerBandGainLaw tests.

use crate::control_law::{ControlLawError, PerBandGainLaw, ShockControlLaw, ShockLawInputs};

fn inputs<'a>(
    target: &'a [f64],
    measured: &'a [f64],
    bands: &'a [f64],
    comps: &'a [f64],
    scale: f64,
) -> ShockLawInputs<'a> {
    ShockLawInputs {
        target_srs: target,
        measured_srs: measured,
        band_freqs: bands,
        component_freqs: comps,
        predicted_limit_srs: None,
        current_scale: scale,
    }
}

#[test]
fn init_seeds_unity_gains_and_holds_level() {
    let mut law = PerBandGainLaw::new(3).unwrap();
    let bands = [100.0, 200.0, 400.0];
    let comps = [100.0, 200.0, 400.0];
    let out = law.init(&inputs(&[10.0; 3], &[0.0; 3], &bands, &comps, 0.5));
    assert_eq!(out.component_gain_updates, vec![1.0, 1.0, 1.0]);
    assert_eq!(out.level_scale_cmd, 0.5); // ladder owns the rung; law holds
    assert_eq!(law.name(), "per-band-gain");
}

#[test]
fn classic_ratio_iteration_drives_up_low_bands_down_high_bands() {
    let mut law = PerBandGainLaw::new(3).unwrap();
    let bands = [100.0, 200.0, 400.0];
    let comps = [100.0, 200.0, 400.0];
    let target = [10.0, 10.0, 10.0];
    // measured: band 0 low (want gain up), band 1 on target, band 2 high (gain down).
    let measured = [5.0, 10.0, 20.0];
    let out = law.on_pulse(&inputs(&target, &measured, &bands, &comps, 1.0));
    let g = &out.component_gain_updates;
    assert!(g[0] > 1.0, "low band gets driven up");
    assert!((g[1] - 1.0).abs() < 1e-9, "on-target band holds");
    assert!(g[2] < 1.0, "high band gets backed off");
    // Ratio 2.0 at band 0 is under the +3 dB clamp (10^(3/20) ~ 1.41), so it clamps.
    // Verify the clamp applied (2.0 > 1.413 -> clamped).
    let max_lin = 10.0_f64.powf(3.0 / 20.0);
    assert!(
        (g[0] - max_lin).abs() < 1e-9,
        "band-0 ratio 2.0 clamps to +3 dB"
    );
    assert!(out.diagnostics.clamped_components >= 1);
}

#[test]
fn per_iteration_db_clamp_bounds_the_step() {
    let mut law = PerBandGainLaw::with_clamp(1, 1.0).unwrap(); // 1 dB clamp
    let bands = [100.0];
    let comps = [100.0];
    // A wildly low measurement would demand a huge gain; the clamp bounds it to +1 dB.
    let out = law.on_pulse(&inputs(&[100.0], &[0.001], &bands, &comps, 1.0));
    let g = out.component_gain_updates[0];
    let one_db = 10.0_f64.powf(1.0 / 20.0);
    assert!((g - one_db).abs() < 1e-9, "step clamped to +1 dB");
    assert_eq!(out.diagnostics.clamped_components, 1);
}

#[test]
fn dead_measured_band_holds_gain_rather_than_chasing_noise() {
    let mut law = PerBandGainLaw::new(1).unwrap();
    let bands = [100.0];
    let comps = [100.0];
    // measured == 0 (dead): hold unity, do NOT command an infinite push.
    let out = law.on_pulse(&inputs(&[10.0], &[0.0], &bands, &comps, 1.0));
    assert_eq!(out.component_gain_updates[0], 1.0);
    // No trusted band -> no worst-error diagnostic.
    assert!(out.diagnostics.worst_band_err_db.is_none());
}

#[test]
fn components_map_to_nearest_band_when_grids_differ_f3() {
    // 2 components on a 3-band grid: components map to the NEAREST band (the three
    // grids are distinct).
    let mut law = PerBandGainLaw::new(2).unwrap();
    let bands = [100.0, 200.0, 400.0];
    let comps = [110.0, 390.0]; // nearest to band 0 (100) and band 2 (400)
    let target = [10.0, 10.0, 10.0];
    let measured = [5.0, 10.0, 40.0]; // band 0 low, band 2 very high
    let out = law.on_pulse(&inputs(&target, &measured, &bands, &comps, 1.0));
    assert!(
        out.component_gain_updates[0] > 1.0,
        "comp near band 0 (low) drives up"
    );
    assert!(
        out.component_gain_updates[1] < 1.0,
        "comp near band 2 (high) backs off"
    );
}

#[test]
fn worst_band_diagnostic_reports_the_largest_absolute_error() {
    let mut law = PerBandGainLaw::new(3).unwrap();
    let bands = [100.0, 200.0, 400.0];
    let comps = [100.0, 200.0, 400.0];
    let target = [10.0, 10.0, 10.0];
    let measured = [12.0, 10.0, 40.0]; // band 2 is +12 dB, the worst
    let out = law.on_pulse(&inputs(&target, &measured, &bands, &comps, 1.0));
    assert_eq!(out.diagnostics.worst_band_index, Some(2));
    let db = out.diagnostics.worst_band_err_db.unwrap();
    assert!(db > 11.0 && db < 13.0, "band 2 error ~ +12 dB, got {db}");
}

#[test]
fn predicted_limit_srs_is_ignored_by_the_shipped_law() {
    // The shipped law does not limit; passing predicted_limit_srs must not change the
    // output (limiting lives in a separate law).
    let mut law = PerBandGainLaw::new(1).unwrap();
    let bands = [100.0];
    let comps = [100.0];
    let target = [10.0];
    let measured = [10.0];
    let no_limit = law.on_pulse(&inputs(&target, &measured, &bands, &comps, 1.0));

    let limit_curves = vec![vec![1.0]];
    let with_limit = ShockLawInputs {
        target_srs: &target,
        measured_srs: &measured,
        band_freqs: &bands,
        component_freqs: &comps,
        predicted_limit_srs: Some(&limit_curves),
        current_scale: 1.0,
    };
    let mut law2 = PerBandGainLaw::new(1).unwrap();
    let out2 = law2.on_pulse(&with_limit);
    assert_eq!(no_limit.component_gain_updates, out2.component_gain_updates);
}

#[test]
fn config_validation_rejects_bad_knobs() {
    assert!(matches!(
        PerBandGainLaw::new(0),
        Err(ControlLawError::BadConfig(_))
    ));
    assert!(matches!(
        PerBandGainLaw::with_clamp(1, 0.0),
        Err(ControlLawError::BadConfig(_))
    ));
    assert!(matches!(
        PerBandGainLaw::with_clamp(1, f64::NAN),
        Err(ControlLawError::BadConfig(_))
    ));
    assert!(PerBandGainLaw::with_clamp(2, 6.0).is_ok());
}
