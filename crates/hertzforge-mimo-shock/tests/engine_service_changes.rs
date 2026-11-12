//! Three changes the run service asked of the engine, each held by a test:
//!
//! 1. **Abort lines are absolute, and scaled by choice.** A control's measured
//!    peak over the full-level abort line (the full-level target's peak plus
//!    the abort margin) aborts at every level (`control_abort_absolute`). The
//!    same margin over the level's own target (the scaled line) ends the run in
//!    the controlled stop when scaled aborts are on (`control_abort_scaled`);
//!    off, the pulse is flagged, judged failing, and the ladder holds.
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

/// The severe 4 x 4 twist SRS run: the -12 dB seed measures about 7 dB over its
/// own level's target (over its scaled line, 6 dB) and about 5 dB under the
/// full-level target (under its absolute line).
fn severe_twist(scaled: bool) -> (MimoShockEngine, hertzforge_mimo_shock::PulseReport) {
    let mut cfg = config(4, 0, 4, LEN_SRS);
    cfg.guards.scaled_aborts = scaled;
    let (mut e, _) = ready(
        ShockPlant::Jointed4x4Severe,
        cfg,
        srs_twist(),
        Arc::new(ShapeFactory::default()),
    );
    let first = e.fire_next(None).unwrap();
    let worst = first
        .controls
        .iter()
        .fold(f64::NEG_INFINITY, |a, c| a.max(c.scaled_margin_db));
    let worst_abs = first
        .controls
        .iter()
        .fold(f64::NEG_INFINITY, |a, c| a.max(c.absolute_margin_db));
    println!(
        "scaled aborts {scaled}: first pulse at {} dB, {worst:+.2} dB against its scaled \
         line, {worst_abs:+.2} dB against the absolute line; end {:?}",
        first.level_db, first.end
    );
    assert!(
        worst > 0.0 && worst_abs < 0.0,
        "the case no longer sits between the lines"
    );
    (e, first)
}

#[test]
fn mimo_shock_engine_with_scaled_aborts_a_pulse_over_its_level_line_ends_the_run_by_that_line() {
    let (_, first) = severe_twist(true);
    let end = first.end.expect("the run ended");
    assert_eq!(
        (end.outcome, end.reason.as_str()),
        (EndOutcome::SafeStop, "control_abort_scaled")
    );
}

#[test]
fn mimo_shock_engine_without_scaled_aborts_the_pulse_is_flagged_and_the_ladder_holds() {
    let (mut e, first) = severe_twist(false);
    assert!(first.end.is_none(), "{:?}", first.end);
    assert!(!first.passed);
    assert!(
        first
            .warnings
            .iter()
            .any(|w| w.contains("abort line scaled")),
        "{:?}",
        first.warnings
    );
    let second = e.fire_next(None).unwrap();
    assert_eq!(
        second.level_db, first.level_db,
        "the ladder climbed on a flagged pulse"
    );
}

#[test]
fn mimo_shock_engine_a_pulse_over_the_absolute_line_aborts_by_it_in_both_settings() {
    // The severe twist SRS seed played at -3 dB measures several dB over the
    // full-level peak while its prediction is near the target: a line 2 dB
    // over the full-level peak ends the run on that first pulse, below full
    // level, by the absolute line, whether scaled aborts are on or off.
    for scaled in [true, false] {
        let mut cfg = config(4, 0, 4, LEN_SRS);
        cfg.ladder = LadderPlan {
            levels_db: vec![-3.0, 0.0],
            ..LadderPlan::default()
        };
        cfg.guards.control_abort_db = Some(2.0);
        cfg.guards.scaled_aborts = scaled;
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
            .fold(f64::NEG_INFINITY, |a, c| a.max(c.absolute_margin_db));
        println!("scaled {scaled}: {worst:+.2} dB over the absolute line");
        let end = p.end.expect("the run ended");
        assert_eq!(p.level_db, -3.0);
        assert_eq!(
            (end.outcome, end.reason.as_str()),
            (EndOutcome::Aborted, "control_abort_absolute")
        );
    }
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

/// A law that reports it trapped (a law file's crash) on every iteration.
struct Trapping(InverseFactory);

struct TrappingLaw(Box<dyn hertzforge_mimo_shock::MimoShockLaw + Send>);

impl hertzforge_mimo_shock::MimoShockLaw for TrappingLaw {
    fn name(&self) -> &str {
        "trapping"
    }
    fn seed(
        &mut self,
        level_db: f64,
    ) -> Result<hertzforge_mimo_shock::PulseOutputs, hertzforge_shock_common::law_guard::LawFault>
    {
        self.0.seed(level_db)
    }
    fn iterate(
        &mut self,
        _inputs: &hertzforge_mimo_shock::PulseInputs,
    ) -> Result<hertzforge_mimo_shock::PulseOutputs, hertzforge_shock_common::law_guard::LawFault>
    {
        Err(hertzforge_shock_common::law_guard::LawFault::Crashed(
            "the law trapped".into(),
        ))
    }
}

impl MimoShockLawFactory for Trapping {
    fn build(
        &self,
        setup: &hertzforge_mimo_shock::Setup,
    ) -> Result<
        Box<dyn hertzforge_mimo_shock::MimoShockLaw + Send>,
        hertzforge_shock_common::law_guard::LawFault,
    > {
        Ok(Box::new(TrappingLaw(self.0.build(setup)?)))
    }
    fn describe(&self) -> hertzforge_shock_common::law_guard::LawDescriptor {
        self.0.describe()
    }
    fn manifest(&self) -> hertzforge_mimo_shock::LawManifest {
        self.0.manifest()
    }
}

#[test]
fn mimo_shock_engine_a_law_file_that_traps_ends_the_run_at_once() {
    let (mut e, _) = ready(
        ShockPlant::Linear4x4,
        config(4, 0, 4, LEN_CLASSICAL),
        classical(&[20.0, 18.0, 20.0, 18.0]),
        Arc::new(Trapping(InverseFactory::default())),
    );
    let end = e.run_to_end(None).unwrap();
    assert_eq!(e.pulses().len(), 1, "{end:?}");
    assert_eq!(
        (end.outcome, end.reason.as_str()),
        (EndOutcome::LawFault, "panicked")
    );
}
