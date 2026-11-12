//! Three changes the run service asked of the engine, each held by a test:
//!
//! 1. **Abort lines are absolute.** A control's measured abort is judged
//!    against the full-level abort line (the full-level target's peak plus the
//!    abort margin) at every level. A low-level pulse over its own level's
//!    target is a judged failure the law may correct, not an abort.
//! 2. **The joint verdict has a mode.** `All` (every control inside its own
//!    tolerance) and `Global` (an SRS target only: the energy-weighted global
//!    SRS error inside the tolerance and no control over its abort line).
//! 3. **Unreachable targets are held at review.** A control whose best
//!    reachable response fails its own judge holds the run before the first
//!    pulse; it goes on only when acknowledged, or with the `Global` verdict.

mod support;

use hertzforge_hal_sim::plants::ShockPlant;
use hertzforge_mimo_shock::builtin::{InverseFactory, ShapeFactory};
use hertzforge_mimo_shock::{
    EndOutcome, EngineError, HoldTiming, LadderPlan, MimoShockConfig, MimoShockEngine,
    MimoShockLawFactory, Target, TargetSpec, Timing, VerdictMode,
};
use hertzforge_module_abi::StationClass;
use std::sync::Arc;
use support::*;

/// Arm and identify, stopping before the law is built.
fn identified(
    plant: ShockPlant,
    cfg: MimoShockConfig,
    spec: TargetSpec,
    law: Arc<dyn MimoShockLawFactory>,
) -> MimoShockEngine {
    let (nc, nl, nd) = (cfg.controls.len(), cfg.limits.len(), cfg.drives.len());
    let mut e = MimoShockEngine::new(cfg, law)
        .expect("a good configuration")
        .with_target_spec(spec)
        .with_clock(Arc::new(ManualClock::default()))
        .with_pacing(false);
    e.arm(
        station(plant, FS, 11),
        table(nc, nl, nd),
        StationClass::HertzforgeSim,
    )
    .expect("armed");
    e.identify().expect("identified");
    assert!(e.end().is_none(), "held: {:?}", e.end());
    e
}

#[test]
fn mimo_shock_engine_a_low_level_pulse_over_its_own_target_is_judged_not_aborted() {
    // The severe 4 x 4 twist SRS run: the -12 dB seed measures about 7 dB over
    // its own level's target, 5 dB under the full-level target. Under absolute
    // lines (6 dB over the full-level peak) it is a judged failure.
    let cfg = config(4, 0, 4, LEN_SRS);
    let (mut e, _) = ready(
        ShockPlant::Jointed4x4Severe,
        cfg,
        srs_twist(),
        Arc::new(ShapeFactory::default()),
    );
    let first = e.fire_next(None).unwrap();
    let over = first.over_test_db;
    let worst = first
        .controls
        .iter()
        .fold(0.0f64, |a, c| a.max(c.verdict.peak_ratio));
    println!(
        "first pulse at {} dB: worst peak {:.2} dB over its level's target; end {:?}",
        first.level_db,
        20.0 * worst.log10(),
        first.end
    );
    assert!(20.0 * worst.log10() > 6.0, "the case no longer over-tests");
    assert!(over > 3.0 && !first.passed);
    assert!(
        first.end.is_none(),
        "a pulse under the full-level line ended the run: {:?}",
        first.end
    );
}

#[test]
fn mimo_shock_engine_a_pulse_over_the_full_level_line_aborts_below_full_level() {
    // The severe twist SRS seed played at -3 dB measures several dB over the
    // full-level peak while its prediction is near the target: a line 2 dB
    // over the full-level peak ends the run on that first pulse, below full
    // level.
    let mut cfg = config(4, 0, 4, LEN_SRS);
    cfg.ladder = LadderPlan {
        levels_db: vec![-3.0, 0.0],
        ..LadderPlan::default()
    };
    cfg.guards.control_abort_db = Some(2.0);
    let (mut e, _) = ready(
        ShockPlant::Jointed4x4Severe,
        cfg,
        srs_twist(),
        Arc::new(ShapeFactory::default()),
    );
    let p = e.fire_next(None).unwrap();
    let worst = p
        .controls
        .iter()
        .fold(0.0f64, |a, c| a.max(c.verdict.peak_ratio));
    println!(
        "at {} dB: {:.2} dB over the full-level peak",
        p.level_db,
        20.0 * worst.log10() + p.level_db
    );
    let end = p.end.expect("the run ended");
    assert_eq!(p.level_db, -3.0);
    assert_eq!(
        (end.outcome, end.reason.as_str()),
        (EndOutcome::Aborted, "control_abort")
    );
}

#[test]
fn mimo_shock_engine_the_global_verdict_judges_the_global_error_and_the_abort_lines() {
    // Control 3 is held to a delay of 5 ms, which every-control judging fails
    // on timing until the law has moved it there; the global verdict does not
    // judge timing and passes a pulse whose global SRS error is inside the
    // tolerance.
    let mut cfg = config(4, 0, 4, LEN_SRS);
    cfg.guards.verdict = VerdictMode::Global;
    cfg.timing = Timing::Hold(HoldTiming {
        delays_s: vec![0.0, 0.0, 0.0, 0.005],
        window_s: 0.0005,
    });
    let (mut e, _) = ready(
        ShockPlant::Linear4x4,
        cfg,
        srs(&[20.0, 16.0, 20.0, 16.0]),
        Arc::new(ShapeFactory::default()),
    );
    let Some(Target::Srs(t)) = e.target().cloned() else {
        panic!("an SRS target")
    };
    let tol = (-t.tol_lo_db).min(t.tol_hi_db);
    let mut differs = 0;
    for _ in 0..8 {
        let p = e.fire_next(None).unwrap();
        let g = p.global_srs_error_db.expect("a global error");
        let every = p.controls.iter().all(|c| c.verdict.passed);
        println!(
            "global {g:.2} dB, passed {}, every control {every}",
            p.passed
        );
        assert_eq!(p.passed, g <= tol, "global {g:.2} dB, tolerance {tol} dB");
        differs += (p.passed != every) as u32;
        if p.end.is_some() {
            break;
        }
    }
    assert!(
        differs > 0,
        "the global verdict never differed from every-control judging"
    );
}

#[test]
fn mimo_shock_engine_the_global_verdict_refuses_a_waveform_target() {
    let mut cfg = config(4, 0, 4, LEN_CLASSICAL);
    cfg.guards.verdict = VerdictMode::Global;
    let mut e = MimoShockEngine::new(cfg, Arc::new(InverseFactory::default()))
        .unwrap()
        .with_target_spec(classical(&[20.0; 4]))
        .with_pacing(false);
    let r = e.arm(
        station(ShockPlant::Linear4x4, FS, 11),
        table(4, 0, 4),
        StationClass::HertzforgeSim,
    );
    assert!(matches!(r, Err(EngineError::Config(_))), "{r:?}");
}

#[test]
fn mimo_shock_engine_an_unreachable_target_holds_until_acknowledged() {
    let srs6 = || srs(&[20.0, 16.0, 20.0, 16.0, 18.0, 18.0]);
    let mut e = identified(
        ShockPlant::Jointed6x3,
        config(6, 0, 3, LEN_SRS),
        srs6(),
        Arc::new(ShapeFactory::default()),
    );
    let seed = e
        .seed_report()
        .cloned()
        .expect("floors before the first pulse");
    println!(
        "floors {:?}, reachable {:?}",
        seed.floor_db, seed.reachable_ok
    );
    assert!(seed.reachable_ok.iter().any(|ok| !ok));
    let hold = e.unreachable().expect("held");
    assert!(!hold.is_empty());
    match e.proceed() {
        Err(EngineError::Unreachable {
            controls,
            floors_db,
        }) => {
            assert_eq!(controls.len(), floors_db.len());
            assert!(!controls.is_empty());
        }
        other => panic!("{other:?}"),
    }
    assert!(e.pulses().is_empty() && e.end().is_none());
    e.acknowledge_unreachable("known short; run it");
    e.proceed().expect("acknowledged");
    assert!(e.fire_next(None).is_ok());

    // The global verdict shows the floors and does not hold.
    let mut cfg = config(6, 0, 3, LEN_SRS);
    cfg.guards.verdict = VerdictMode::Global;
    let mut e = identified(
        ShockPlant::Jointed6x3,
        cfg,
        srs6(),
        Arc::new(ShapeFactory::default()),
    );
    assert!(e.unreachable().is_none());
    e.proceed().expect("no hold under the global verdict");
}

#[test]
fn mimo_shock_engine_a_reachable_target_is_not_held() {
    let e = identified(
        ShockPlant::Linear4x4,
        config(4, 0, 4, LEN_CLASSICAL),
        classical(&[20.0, 18.0, 20.0, 18.0]),
        Arc::new(InverseFactory::default()),
    );
    let seed = e.seed_report().cloned().expect("floors");
    assert!(seed.reachable_ok.iter().all(|ok| *ok), "{seed:?}");
    assert!(e.unreachable().is_none());
}
