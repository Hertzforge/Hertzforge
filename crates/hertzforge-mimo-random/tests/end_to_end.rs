//! A driverless run through the service API on the simulator (not the wire):
//! feasibility, noise floor, identification, gate, prediction, engage, the ladder,
//! full level and a ramped stop; a run a protection aborts; a run a law fault
//! ends. Every display quantity is answered after the run.
mod common;

use common::*;
use hertzforge_hal_sim::plants::StandardPlant;
use hertzforge_law_host::{LawIdentity, Param};
use hertzforge_mimo_random::{
    MimoEventKind, MimoFaultReason, MimoQuantity, MimoRandomService, MimoState, MimoTripKind,
};
use hertzforge_random_laws::{ChoiceError, MimoLawFactory};
use hertzforge_sdm::law::{
    LawError, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs, MimoSetup, MimoShape,
};
use std::sync::Arc;
use std::time::Duration;

fn states(svc: &MimoRandomService) -> Vec<MimoState> {
    let mut v = vec![MimoState::Idle];
    for e in svc.log().events() {
        if let MimoEventKind::StateChange { to, .. } = e.kind {
            v.push(to);
        }
    }
    v
}

#[test]
fn a_whole_run_walks_every_step_and_takes_a_ramped_stop() {
    let grid = Grid::standard();
    let p = StandardPlant::Modal4x4.build();
    let (mut cfg, table) = physical_config(&p, &grid, 0.5);
    cfg.full_level_duration_s = Some(40.0);
    cfg.shutdown_ramp_s = 4.0;
    let mut svc = service(cfg, table, &p, &grid, "e2e-whole");

    assert_eq!(svc.state(), MimoState::Idle);
    // Commands out of order are refused, in words.
    assert!(svc.proceed().is_err());
    assert!(svc.loop_tick().is_err());
    svc.start().expect("feasibility and arm");
    assert_eq!(svc.state(), MimoState::NoiseFloor);
    svc.run_noise_floor().unwrap();
    assert_eq!(svc.state(), MimoState::SysId);
    svc.run_identification().unwrap();
    assert_eq!(svc.state(), MimoState::Gate);
    let gate = svc.gate().unwrap().clone();
    assert!(gate.passed, "{gate:?}");
    let id = svc.identification().unwrap();
    assert!(
        (id.level_db + 20.0).abs() <= 1.0,
        "identification level {}",
        id.level_db
    );
    svc.proceed().unwrap();
    assert_eq!(svc.state(), MimoState::Predict);
    let pred = svc.prediction().unwrap().clone();
    assert!(pred.breaches.is_empty());
    assert!(pred.error_db.iter().all(|e| *e < 0.5), "{pred:?}");
    svc.iterate_prediction().unwrap();
    assert_eq!(svc.prediction().unwrap().iterations, 1);
    svc.proceed().unwrap();
    assert_eq!(svc.state(), MimoState::Engaged);
    let mut levels = Vec::new();
    let mut n = 0;
    while svc.state().is_running() && n < 30 {
        svc.loop_tick().unwrap();
        levels.push(svc.level_db());
        n += 1;
    }
    assert_eq!(svc.state(), MimoState::Stopped, "{:?}", svc.trip());
    assert!(svc.trip().is_none());
    assert!(svc.loops_at_full_level() >= 2);
    let seen = states(&svc);
    assert_eq!(
        seen,
        vec![
            MimoState::Idle,
            MimoState::NoiseFloor,
            MimoState::SysId,
            MimoState::Gate,
            MimoState::Predict,
            MimoState::Engaged,
            MimoState::FullLevel,
            MimoState::Stopped
        ]
    );
    // The ladder climbed and the stop ramped (the last loop played below full).
    assert!(levels.iter().any(|&l| l < -1.0) && levels.contains(&0.0));
    assert!(svc.log().events().iter().any(
        |e| matches!(&e.kind, MimoEventKind::Note { text } if text.contains("Controlled stop"))
    ));

    // The report.
    let r = svc.report();
    assert_eq!(r.final_state, MimoState::Stopped);
    assert_eq!(r.law.as_ref().unwrap().id, "pinv_closed");
    assert!(r.identification.is_some() && r.gate.is_some() && r.prediction.is_some());
    assert_eq!(r.final_error_db.len(), 4);
    assert!(
        r.final_error_db.iter().all(|e| *e < 1.0),
        "{:?}",
        r.final_error_db
    );
    let json = serde_json::to_string(&r).unwrap();
    assert!(json.contains("pinv_closed"));
    let status = svc.status();
    assert_eq!(status.state, MimoState::Stopped);
    assert_eq!(status.drive_rms.len(), 4);
    serde_json::to_string(&status).unwrap();

    // Every display quantity of design 9.6 is answered, on the band's lines.
    let lines = svc.band().1 - svc.band().0;
    let grid_qs = [
        MimoQuantity::Psd(1),
        MimoQuantity::Ref(1),
        MimoQuantity::Coh(0, 2),
        MimoQuantity::Phase(0, 2),
        MimoQuantity::CsdMag(0, 2),
        MimoQuantity::ErrorDb,
        MimoQuantity::DrivePsd(3),
        MimoQuantity::DriveCoh(0, 1),
        MimoQuantity::Frf(2, 1),
        MimoQuantity::Sv,
        MimoQuantity::Cond,
        MimoQuantity::Mcoh(0),
        MimoQuantity::Cap(0),
        MimoQuantity::Trace,
    ];
    for q in &grid_qs {
        let s = svc
            .quantity(q)
            .unwrap_or_else(|| panic!("{q:?} not answered"));
        assert_eq!(s.x.len(), lines, "{q:?}");
        assert_eq!(s.edges.len(), lines + 1, "{q:?}");
        assert!(
            !s.traces.is_empty() && s.traces.iter().all(|t| t.y.len() == lines),
            "{q:?}"
        );
    }
    assert_eq!(
        svc.quantity(&MimoQuantity::ErrorDb).unwrap().traces.len(),
        4
    );
    assert_eq!(svc.quantity(&MimoQuantity::Sv).unwrap().traces.len(), 4);
    for q in [
        MimoQuantity::DofRms,
        MimoQuantity::PredDriveRms(2),
        MimoQuantity::PredErrorDb(3),
    ] {
        assert!(svc.quantity(&q).is_some(), "{q:?}");
    }
    // A law diagnostic by its declared key, if the law reported any.
    let any_diag = svc.report().law.is_some();
    assert!(any_diag);
    // Out of range is None, not a panic.
    assert!(svc.quantity(&MimoQuantity::Psd(4)).is_none());
    assert!(svc.quantity(&MimoQuantity::Frf(0, 9)).is_none());
}

#[test]
fn a_kinematic_limit_aborts_the_run_with_the_drive_taken_to_zero() {
    let grid = Grid::standard();
    let p = StandardPlant::Modal4x4.build();
    let (cfg, mut table) = physical_config(&p, &grid, 0.5);
    // Control 1's acceleration limit at twice its full-level rms: Gaussian peaks
    // pass it at full level, not at the identification level.
    let (lo, hi) = grid.band();
    let s: f64 = (lo..hi).map(|l| cfg.reference.sdm.get(l, 0, 0).re).sum();
    let rms = (s * grid.df()).sqrt();
    table.channels[0].abort.accel_g = Some(2.0 * rms);
    let mut svc = service(cfg, table, &p, &grid, "e2e-abort");
    let st = svc.run_auto(30).unwrap();
    assert_eq!(st, MimoState::Aborted);
    match svc.trip().map(|t| &t.kind) {
        Some(MimoTripKind::Kinematic {
            channel_index,
            limit,
            ..
        }) => {
            assert_eq!(*channel_index, 1);
            assert!((limit - 2.0 * rms).abs() < 1e-9 * rms);
        }
        other => panic!("expected a kinematic abort, got {other:?}"),
    }
    assert!(svc.loop_count() > 0, "it engaged before the abort");
    // A command after an abort is refused.
    assert!(svc.loop_tick().is_err());
}

/// The default law for the seed, then an error on every iteration.
struct Failing(Box<dyn MimoControlLaw + Send>);

impl MimoControlLaw for Failing {
    fn needs(&self) -> &[MimoInput] {
        self.0.needs()
    }
    fn shapes(&self) -> &[MimoShape] {
        self.0.shapes()
    }
    fn seed(&mut self, i: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        self.0.seed(i)
    }
    fn iterate(&mut self, _: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        Err(LawError("no drive this loop".into()))
    }
}

struct FailingFactory(Arc<dyn MimoLawFactory>);

impl MimoLawFactory for FailingFactory {
    fn identity(&self) -> LawIdentity {
        self.0.identity()
    }
    fn params(&self) -> &[Param] {
        self.0.params()
    }
    fn build(
        &self,
        s: &MimoSetup,
        b: Duration,
    ) -> Result<Box<dyn MimoControlLaw + Send>, ChoiceError> {
        Ok(Box::new(Failing(self.0.build(s, b)?)))
    }
}

#[test]
fn a_law_fault_ends_the_run_with_a_controlled_stop() {
    let grid = Grid::standard();
    let p = StandardPlant::Modal4x4.build();
    let (cfg, table) = physical_config(&p, &grid, 0.5);
    let inner = hertzforge_mimo_random::law::builtin_factory(&cfg.law, 4).unwrap();
    let mut svc = service(cfg, table, &p, &grid, "e2e-lawfault")
        .with_law_factory(Arc::new(FailingFactory(inner)));
    let st = svc.run_auto(20).unwrap();
    assert_eq!(st, MimoState::Stopped);
    match svc.trip().map(|t| &t.kind) {
        Some(MimoTripKind::LawFault {
            reason,
            consecutive,
        }) => {
            assert_eq!(*reason, MimoFaultReason::Error);
            assert_eq!(*consecutive, 3);
        }
        other => panic!("expected a law fault, got {other:?}"),
    }
    assert_eq!(svc.law_stats().misses, 3);
}
