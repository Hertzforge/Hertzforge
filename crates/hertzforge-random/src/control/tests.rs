//! Unit tests for the control core: every [`HinvConfig`] knob, the three
//! [`CompositeMode`]s, the notch clamp arbitration, and the per-bin
//! divergence accounting. These are pure-math tests over synthetic
//! per-loop inputs; the closed-loop adversarial fixtures live in
//! `tests/adversarial.rs`.

use super::*;

/// A flat in-band target over `[lo,hi)` on an `n`-bin grid.
fn flat_target(n: usize, lo: usize, hi: usize, level: f64) -> Vec<f64> {
    let mut t = vec![0.0; n];
    for v in t.iter_mut().take(hi).skip(lo) {
        *v = level;
    }
    t
}

/// Build a `LoopInputs` from parts, with an empty notch mask by default.
struct Fixture {
    target: Vec<f64>,
    measured: Vec<f64>,
    h_mag2: Vec<f64>,
    coherence: Vec<f64>,
    notched: Vec<bool>,
    level_db: f64,
    df_hz: f64,
    band: (usize, usize),
}

impl Fixture {
    fn inputs(&self) -> LoopInputs<'_> {
        LoopInputs {
            target_psd: &self.target,
            measured_psd: &self.measured,
            h_mag2: &self.h_mag2,
            coherence: &self.coherence,
            current_level_db: self.level_db,
            df_hz: self.df_hz,
            notched_bins: &self.notched,
            band: self.band,
        }
    }
}

// ---------------------------------------------------------------------------
// composite modes
// ---------------------------------------------------------------------------

#[test]
fn composite_weighted_avg_is_the_weighted_mean() {
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![3.0, 2.0, 1.0];
    // equal weights -> plain average.
    let c = composite_psd(
        &[a.clone(), b.clone()],
        &[1.0, 1.0],
        CompositeMode::WeightedAvg,
    )
    .unwrap();
    assert_eq!(c, vec![2.0, 2.0, 2.0]);
    // 3:1 weighting toward `a`.
    let c = composite_psd(&[a, b], &[3.0, 1.0], CompositeMode::WeightedAvg).unwrap();
    // bin 0: 0.75*1 + 0.25*3 = 1.5
    assert!((c[0] - 1.5).abs() < 1e-12);
    assert!((c[2] - 2.5).abs() < 1e-12);
}

#[test]
fn composite_min_and_max_operate_on_weighted_psds() {
    let a = vec![1.0, 4.0];
    let b = vec![2.0, 2.0];
    // equal (normalized 0.5) weights: weighted PSDs are a/2 and b/2.
    // min: bin0 min(0.5, 1.0)=0.5 ; bin1 min(2.0,1.0)=1.0
    let cmin = composite_psd(&[a.clone(), b.clone()], &[1.0, 1.0], CompositeMode::Min).unwrap();
    assert!((cmin[0] - 0.5).abs() < 1e-12);
    assert!((cmin[1] - 1.0).abs() < 1e-12);
    // max: bin0 max(0.5,1.0)=1.0 ; bin1 max(2.0,1.0)=2.0
    let cmax = composite_psd(&[a, b], &[1.0, 1.0], CompositeMode::Max).unwrap();
    assert!((cmax[0] - 1.0).abs() < 1e-12);
    assert!((cmax[1] - 2.0).abs() < 1e-12);
}

#[test]
fn composite_weighting_shifts_min_and_max_binding_channel() {
    // Heavily up-weight channel a: its weighted PSD dominates the max, and down-
    // weighting b makes b the binding min.
    let a = vec![1.0];
    let b = vec![1.0];
    let cmax = composite_psd(&[a.clone(), b.clone()], &[9.0, 1.0], CompositeMode::Max).unwrap();
    // weights normalize to 0.9/0.1: max(0.9,0.1)=0.9 -> a binds the max.
    assert!((cmax[0] - 0.9).abs() < 1e-12);
    let cmin = composite_psd(&[a, b], &[9.0, 1.0], CompositeMode::Min).unwrap();
    assert!((cmin[0] - 0.1).abs() < 1e-12); // b binds the min
}

#[test]
fn composite_rejects_bad_shapes_and_weights() {
    assert!(matches!(
        composite_psd(&[], &[], CompositeMode::WeightedAvg),
        Err(ControlError::BadComposite(_))
    ));
    assert!(matches!(
        composite_psd(&[vec![1.0]], &[1.0, 2.0], CompositeMode::WeightedAvg),
        Err(ControlError::BadComposite(_))
    ));
    assert!(matches!(
        composite_psd(
            &[vec![1.0, 2.0], vec![1.0]],
            &[1.0, 1.0],
            CompositeMode::WeightedAvg
        ),
        Err(ControlError::BadComposite(_))
    ));
    // zero-sum weights rejected (no silent zero composite).
    assert!(matches!(
        composite_psd(
            &[vec![1.0], vec![2.0]],
            &[0.0, 0.0],
            CompositeMode::WeightedAvg
        ),
        Err(ControlError::BadComposite(_))
    ));
}

// ---------------------------------------------------------------------------
// notch clamp behavior
// ---------------------------------------------------------------------------

#[test]
fn notch_clamps_drive_and_reports_mask() {
    let mut ns = NotchSet::unlimited(4);
    // explicit drive ceiling on two bins.
    ns.set_drive_ceiling(&[f64::INFINITY, 2.0, 0.5, f64::INFINITY])
        .unwrap();
    let mut drive = vec![1.0, 5.0, 1.0, 3.0];
    let rep = ns.apply(&mut drive);
    assert_eq!(drive, vec![1.0, 2.0, 0.5, 3.0]);
    assert_eq!(rep.notched_lines, vec![1, 2]);
    assert_eq!(rep.notched, vec![false, true, true, false]);
}

#[test]
fn notch_maps_response_ceiling_through_h() {
    // A response ceiling of 4 g^2/Hz at a bin with |H|^2 = 2 becomes a drive ceiling
    // of 4/2 = 2. A bin with |H|^2 below the floor is left unlimited (+inf).
    let mut ns = NotchSet::unlimited(3);
    ns.add_channel_ceiling(&[4.0, 4.0, 4.0], &[2.0, 1e-9, 8.0], 1e-6)
        .unwrap();
    let c = ns.ceiling();
    assert!((c[0] - 2.0).abs() < 1e-12);
    assert!(c[1].is_infinite(), "un-driveable bin must stay unlimited");
    assert!((c[2] - 0.5).abs() < 1e-12);
}

#[test]
fn notch_takes_tightest_ceiling_across_channels() {
    let mut ns = NotchSet::unlimited(2);
    ns.add_channel_ceiling(&[10.0, 10.0], &[1.0, 1.0], 1e-6)
        .unwrap();
    ns.add_channel_ceiling(&[3.0, 20.0], &[1.0, 1.0], 1e-6)
        .unwrap();
    let c = ns.ceiling();
    assert!((c[0] - 3.0).abs() < 1e-12); // second channel tighter at bin 0
    assert!((c[1] - 10.0).abs() < 1e-12); // first channel tighter at bin 1
}

#[test]
fn notch_rejects_grid_mismatch() {
    let mut ns = NotchSet::unlimited(2);
    assert!(matches!(
        ns.add_channel_ceiling(&[1.0], &[1.0], 0.0),
        Err(ControlError::BadInputs(_))
    ));
    assert!(matches!(
        ns.set_drive_ceiling(&[1.0, 2.0, 3.0]),
        Err(ControlError::BadInputs(_))
    ));
}

// ---------------------------------------------------------------------------
// HinvConfig knob validation
// ---------------------------------------------------------------------------

#[test]
fn config_default_validates() {
    assert!(HinvConfig::default().validate().is_ok());
}

#[test]
fn config_rejects_every_bad_knob() {
    let bad = [
        HinvConfig {
            lambda: Some(-1.0),
            ..Default::default()
        },
        HinvConfig {
            suggest_beta: 0.0,
            ..Default::default()
        },
        HinvConfig {
            suggest_beta: 1.0,
            ..Default::default()
        },
        HinvConfig {
            trust_coherence_floor: -0.1,
            ..Default::default()
        },
        HinvConfig {
            trust_coherence_floor: 1.1,
            ..Default::default()
        },
        HinvConfig {
            trust_h_floor_frac: 1.0,
            ..Default::default()
        },
        HinvConfig {
            max_correction_db_per_iter: 0.0,
            ..Default::default()
        },
        HinvConfig {
            svd_keep: Some(0),
            ..Default::default()
        },
        HinvConfig {
            divergence_n: 0,
            ..Default::default()
        },
        HinvConfig {
            servo_rate_db_per_s: 0.0,
            ..Default::default()
        },
        HinvConfig {
            servo_fault_fraction: 0.0,
            ..Default::default()
        },
        HinvConfig {
            servo_fault_fraction: 1.0,
            ..Default::default()
        },
        HinvConfig {
            servo_drive_ceiling: 0.0,
            ..Default::default()
        },
    ];
    for (i, cfg) in bad.iter().enumerate() {
        assert!(cfg.validate().is_err(), "bad config {i} accepted");
    }
}

#[test]
fn config_svd_keep_is_accepted_but_inert_for_siso() {
    // svd_keep >= 1 is accepted (MIMO-future); it changes no SISO behavior.
    let n = 16;
    let band = (2, 14);
    let target = flat_target(n, band.0, band.1, 1.0);
    let cfg_none = HinvConfig::default();
    let cfg_svd = HinvConfig {
        svd_keep: Some(3),
        ..Default::default()
    };
    let mut law_none = HinvLaw::new(cfg_none, &target, band, 1.0).unwrap();
    let mut law_svd = HinvLaw::new(cfg_svd, &target, band, 1.0).unwrap();

    let h_mag2 = vec![1.0; n];
    let coherence = vec![1.0; n];
    let fx = Fixture {
        target: target.clone(),
        measured: target.clone(),
        h_mag2,
        coherence,
        notched: vec![],
        level_db: 0.0,
        df_hz: 1.0,
        band,
    };
    let out_none = law_none.init(&fx.inputs());
    let out_svd = law_svd.init(&fx.inputs());
    assert_eq!(
        out_none.drive_psd, out_svd.drive_psd,
        "svd_keep changed SISO seed drive"
    );
}

#[test]
fn law_new_rejects_bad_band_and_empty_target() {
    let target = flat_target(8, 2, 6, 1.0);
    assert!(HinvLaw::new(HinvConfig::default(), &target, (6, 2), 1.0).is_err());
    assert!(HinvLaw::new(HinvConfig::default(), &target, (0, 20), 1.0).is_err());
    // no in-band energy.
    let empty = vec![0.0; 8];
    assert!(HinvLaw::new(HinvConfig::default(), &empty, (2, 6), 1.0).is_err());
}

// ---------------------------------------------------------------------------
// seed + trust gate + clamp + divergence
// ---------------------------------------------------------------------------

#[test]
fn init_seed_matches_target_over_flat_unit_plant() {
    // With |H|^2 = 1 everywhere and a flat target, the seed drive shape equals the
    // target (drive = target / |H|^2), and the drive at unit level equals the target.
    let n = 32;
    let band = (4, 28);
    let target = flat_target(n, band.0, band.1, 0.01);
    let mut law = HinvLaw::new(HinvConfig::default(), &target, band, 2.0).unwrap();
    let fx = Fixture {
        target: target.clone(),
        measured: target.clone(),
        h_mag2: vec![1.0; n],
        coherence: vec![1.0; n],
        notched: vec![],
        level_db: 0.0,
        df_hz: 2.0,
        band,
    };
    let out = law.init(&fx.inputs());
    // In-band bins ~ target. Not EXACT: the Tikhonov seed softens the divide, so with
    // the default beta=0.05 on a unit plant the drive is target/(1+eps)^2 with
    // eps=(0.05)^2, i.e. ~0.5% low. That softening is intended Tikhonov behavior, not error;
    // allow ~1.5%. The point is the flat unit plant reproduces the flat target shape.
    for (k, (&d, &t)) in out
        .drive_psd
        .iter()
        .zip(target.iter())
        .enumerate()
        .take(band.1)
        .skip(band.0)
    {
        assert!(
            (d - t).abs() < 1.5e-2 * t.max(1e-9),
            "bin {k}: seed drive {d} != target {t}"
        );
    }
    // Out-of-band stays zero.
    assert_eq!(out.drive_psd[0], 0.0);
    assert_eq!(out.drive_psd[n - 1], 0.0);
}

#[test]
fn trust_gate_holds_low_coherence_and_low_h_bins() {
    let n = 20;
    let band = (2, 18);
    let target = flat_target(n, band.0, band.1, 1.0);
    let mut law = HinvLaw::new(HinvConfig::default(), &target, band, 1.0).unwrap();

    let mut h_mag2 = vec![1.0; n];
    let mut coherence = vec![1.0; n];
    // Bin 5: low coherence (below 0.5 default floor).
    coherence[5] = 0.2;
    // Bin 9: low |H| (below 0.05 * peak |H|; peak |H| = 1 so |H|^2 floor = 0.0025).
    h_mag2[9] = 1e-6;
    let fx0 = Fixture {
        target: target.clone(),
        measured: target.clone(),
        h_mag2: h_mag2.clone(),
        coherence: coherence.clone(),
        notched: vec![],
        level_db: 0.0,
        df_hz: 1.0,
        band,
    };
    law.init(&fx0.inputs());
    let shape_before = law.shape().to_vec();

    // Measure a NON-UNIFORM error (bin 3 is 4x high, others on target). A uniform
    // error is a level error the servo owns and the mean-preserving renorm cancels;
    // a differential error is a SHAPE error the trim must correct at bin 3. The two
    // untrusted bins (5 low-coherence, 9 low-|H|) must be held regardless.
    let mut measured = target.clone();
    measured[3] = target[3] * 4.0;
    let fx = Fixture {
        target,
        measured,
        h_mag2,
        coherence,
        notched: vec![],
        level_db: 0.0,
        df_hz: 1.0,
        band,
    };
    let out = law.iterate(&fx.inputs());
    assert!(out.diagnostics.untrusted_bins.contains(&5));
    assert!(out.diagnostics.untrusted_bins.contains(&9));
    // Held bins' shape is unchanged; the differentially-errored trusted bin moved.
    assert!(
        (law.shape()[5] - shape_before[5]).abs() < 1e-12,
        "held bin 5 moved"
    );
    assert!(
        (law.shape()[9] - shape_before[9]).abs() < 1e-12,
        "held bin 9 moved"
    );
    assert!(
        (law.shape()[3] - shape_before[3]).abs() > 1e-9,
        "trusted bin 3 did not move"
    );
}

#[test]
fn correction_clamp_bounds_the_per_iter_step() {
    // A huge single-bin error must clamp the trim step at max_correction_db_per_iter.
    let n = 12;
    let band = (2, 10);
    let target = flat_target(n, band.0, band.1, 1.0);
    let cfg = HinvConfig {
        max_correction_db_per_iter: 2.0,
        ..Default::default()
    };
    let mut law = HinvLaw::new(cfg, &target, band, 1.0).unwrap();
    let fx0 = Fixture {
        target: target.clone(),
        measured: target.clone(),
        h_mag2: vec![1.0; n],
        coherence: vec![1.0; n],
        notched: vec![],
        level_db: 0.0,
        df_hz: 1.0,
        band,
    };
    law.init(&fx0.inputs());
    let shape_before = law.shape().to_vec();

    // Measured is 100x LOW at bin 4 (target/meas = 100 => would want +20 dB); clamp
    // to +2 dB => step ratio 10^(2/10) ~= 1.585.
    let mut measured = target.clone();
    measured[4] = target[4] / 100.0;
    let fx = Fixture {
        target,
        measured,
        h_mag2: vec![1.0; n],
        coherence: vec![1.0; n],
        notched: vec![],
        level_db: 0.0,
        df_hz: 1.0,
        band,
    };
    let out = law.iterate(&fx.inputs());
    assert!(out.diagnostics.clamped_bins.contains(&4));
    // The pre-renormalization step was clamped to 10^(0.2) ~= 1.585. Renormalization
    // scales the whole set slightly, but the bin-4 growth is bounded well under 100x.
    let grew = law.shape()[4] / shape_before[4].max(1e-30);
    assert!(grew < 2.0, "bin 4 grew {grew}x, exceeds the clamp bound");
    assert!(grew > 1.0, "bin 4 did not grow at all");
}

#[test]
fn divergence_reported_after_n_consecutive_clamped_iters() {
    // Hold a persistent, un-satisfiable error at one bin so it clamps every loop;
    // after divergence_n consecutive clamps the bin is reported divergent.
    let n = 10;
    let band = (2, 8);
    let target = flat_target(n, band.0, band.1, 1.0);
    let cfg = HinvConfig {
        max_correction_db_per_iter: 1.0,
        divergence_n: 4,
        ..Default::default()
    };
    let mut law = HinvLaw::new(cfg, &target, band, 1.0).unwrap();
    let h_mag2 = vec![1.0; n];
    let coherence = vec![1.0; n];
    let fx0 = Fixture {
        target: target.clone(),
        measured: target.clone(),
        h_mag2: h_mag2.clone(),
        coherence: coherence.clone(),
        notched: vec![],
        level_db: 0.0,
        df_hz: 1.0,
        band,
    };
    law.init(&fx0.inputs());

    // Persistently report bin 4 as far below target every loop -> clamps every loop.
    let mut measured = target.clone();
    measured[4] = target[4] / 1000.0;
    let mut divergent_at = None;
    for loop_i in 1..=6u32 {
        let fx = Fixture {
            target: target.clone(),
            measured: measured.clone(),
            h_mag2: h_mag2.clone(),
            coherence: coherence.clone(),
            notched: vec![],
            level_db: 0.0,
            df_hz: 1.0,
            band,
        };
        let out = law.iterate(&fx.inputs());
        assert!(out.diagnostics.clamped_bins.contains(&4));
        if out.diagnostics.divergent_bins.contains(&4) && divergent_at.is_none() {
            divergent_at = Some(loop_i);
        }
    }
    assert_eq!(
        divergent_at,
        Some(4),
        "bin 4 should be flagged divergent at the 4th consecutive clamp"
    );
}

#[test]
fn a_non_clamping_iter_resets_the_divergence_streak() {
    let n = 10;
    let band = (2, 8);
    let target = flat_target(n, band.0, band.1, 1.0);
    let cfg = HinvConfig {
        max_correction_db_per_iter: 1.0,
        divergence_n: 3,
        ..Default::default()
    };
    let mut law = HinvLaw::new(cfg, &target, band, 1.0).unwrap();
    let h_mag2 = vec![1.0; n];
    let coherence = vec![1.0; n];
    let base = Fixture {
        target: target.clone(),
        measured: target.clone(),
        h_mag2: h_mag2.clone(),
        coherence: coherence.clone(),
        notched: vec![],
        level_db: 0.0,
        df_hz: 1.0,
        band,
    };
    law.init(&base.inputs());

    let mut clamped = target.clone();
    clamped[4] = target[4] / 1000.0;
    // Two clamped loops (streak 2), then one on-target loop (reset), then two more.
    let seq = [true, true, false, true, true];
    let mut ever_divergent = false;
    for &clamp in &seq {
        let measured = if clamp {
            clamped.clone()
        } else {
            target.clone()
        };
        let fx = Fixture {
            target: target.clone(),
            measured,
            h_mag2: h_mag2.clone(),
            coherence: coherence.clone(),
            notched: vec![],
            level_db: 0.0,
            df_hz: 1.0,
            band,
        };
        let out = law.iterate(&fx.inputs());
        if out.diagnostics.divergent_bins.contains(&4) {
            ever_divergent = true;
        }
    }
    // Max consecutive clamps is 2 (< divergence_n 3), so never divergent.
    assert!(
        !ever_divergent,
        "streak did not reset on the on-target loop"
    );
}

#[test]
fn notched_bins_are_excluded_from_renormalization() {
    // A notched bin must not be re-inflated by the mean-preserving renorm, and it
    // must not count in the renorm set. Feed a notch mask and verify the notched
    // bin's shape is held while trusted bins renormalize among themselves.
    let n = 12;
    let band = (2, 10);
    let target = flat_target(n, band.0, band.1, 1.0);
    let mut law = HinvLaw::new(HinvConfig::default(), &target, band, 1.0).unwrap();
    let h_mag2 = vec![1.0; n];
    let coherence = vec![1.0; n];
    let fx0 = Fixture {
        target: target.clone(),
        measured: target.clone(),
        h_mag2: h_mag2.clone(),
        coherence: coherence.clone(),
        notched: vec![],
        level_db: 0.0,
        df_hz: 1.0,
        band,
    };
    law.init(&fx0.inputs());
    let shape_before = law.shape().to_vec();

    // Notch bin 5; measure a uniform error so trusted bins want to move.
    let mut notched = vec![false; n];
    notched[5] = true;
    let measured: Vec<f64> = target.iter().map(|&t| t * 2.0).collect();
    let fx = Fixture {
        target,
        measured,
        h_mag2,
        coherence,
        notched,
        level_db: 0.0,
        df_hz: 1.0,
        band,
    };
    law.iterate(&fx.inputs());
    // Notched bin held exactly.
    assert!(
        (law.shape()[5] - shape_before[5]).abs() < 1e-12,
        "notched bin 5 was modified by the trim/renorm"
    );
}

#[test]
fn servo_fault_hold_on_dead_measurement() {
    // A measured RMS far below target (dead control channel) must latch the servo
    // fault and NOT ramp the level up.
    let n = 12;
    let band = (2, 10);
    let target = flat_target(n, band.0, band.1, 1.0);
    let mut law = HinvLaw::new(HinvConfig::default(), &target, band, 1.0).unwrap();
    let h_mag2 = vec![1.0; n];
    let coherence = vec![1.0; n];
    let fx0 = Fixture {
        target: target.clone(),
        measured: target.clone(),
        h_mag2: h_mag2.clone(),
        coherence: coherence.clone(),
        notched: vec![],
        level_db: 0.0,
        df_hz: 1.0,
        band,
    };
    let seed = law.init(&fx0.inputs());
    let level_before = seed.level_cmd_db;

    // Measured PSD is essentially zero (dead channel): RMS << target*fault_fraction.
    let measured = vec![1e-20; n];
    let fx = Fixture {
        target,
        measured,
        h_mag2,
        coherence,
        notched: vec![],
        level_db: 0.0,
        df_hz: 1.0,
        band,
    };
    let out = law.iterate(&fx.inputs());
    assert!(
        out.diagnostics.servo_faulted,
        "dead measurement did not latch fault"
    );
    assert!(
        (out.level_cmd_db - level_before).abs() < 1e-9,
        "servo ramped the level on a dead measurement (before {level_before}, after {})",
        out.level_cmd_db
    );
}

#[test]
fn law_does_not_panic_on_ragged_inputs() {
    // A ragged grid must be absorbed (no panic in library paths), returning a safe
    // zero/hold drive rather than indexing out of bounds.
    let n = 8;
    let band = (2, 6);
    let target = flat_target(n, band.0, band.1, 1.0);
    let mut law = HinvLaw::new(HinvConfig::default(), &target, band, 1.0).unwrap();
    let short = vec![0.0; 3];
    let inputs = LoopInputs {
        target_psd: &target,
        measured_psd: &short, // ragged
        h_mag2: &short,
        coherence: &short,
        current_level_db: 0.0,
        df_hz: 1.0,
        notched_bins: &[],
        band,
    };
    let out = law.init(&inputs);
    assert_eq!(out.drive_psd.len(), n);
    let out2 = law.iterate(&inputs);
    assert_eq!(out2.drive_psd.len(), n);
}

#[test]
fn name_is_stable() {
    let target = flat_target(8, 2, 6, 1.0);
    let law = HinvLaw::new(HinvConfig::default(), &target, (2, 6), 1.0).unwrap();
    assert_eq!(law.name(), "hinv");
}

#[test]
fn config_round_trips_through_json() {
    let cfg = HinvConfig {
        lambda: Some(1e-4),
        svd_keep: Some(2),
        divergence_n: 7,
        ..Default::default()
    };
    let js = serde_json::to_string(&cfg).unwrap();
    let back: HinvConfig = serde_json::from_str(&js).unwrap();
    assert_eq!(cfg, back);
}
