//! A level step carries the drive with it: the level schedule is the
//! controller's, so when the level moves the controller scales the drive in
//! force by the same ratio (at the change caps' pace, every guard applied)
//! and tells the law the result. On a linear structure that is exactly right,
//! so a law that never corrects still lands every control on the new level.

mod common;

use common::*;
use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_mimo_sine::core::Command;
use hertzforge_mimo_sine::law::{LawDescriptor, LawManifest, LawSetup, LawValues};
use hertzforge_mimo_sine::records::{EventKind, Outcome};
use hertzforge_mimo_sine::*;
use hertzforge_sine::scheduler::{LadderConfig, RungFor};
use std::sync::Arc;

/// A law that engages as `matrix` does and then never corrects: every
/// answer is the drive in force, as it was told it.
struct Frozen(Arc<dyn MimoSineLawFactory>);

struct FrozenLaw(Box<dyn MimoSineLaw + Send>);

impl MimoSineLaw for FrozenLaw {
    fn name(&self) -> &str {
        "frozen"
    }
    fn engage(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut LawValues,
    ) -> Result<(), LawFault> {
        self.0.engage(x, o, v)
    }
    fn tick(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        _: &mut LawValues,
    ) -> Result<(), LawFault> {
        o.drive.copy_from_slice(x.drive_in_force);
        Ok(())
    }
}

impl MimoSineLawFactory for Frozen {
    fn build(&self, s: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        Ok(Box::new(FrozenLaw(self.0.build(s)?)))
    }
    fn describe(&self) -> LawDescriptor {
        self.0.describe()
    }
    fn manifest(&self) -> LawManifest {
        self.0.manifest()
    }
}

/// The error the carried drive is held to on the linear 4 by 4, dB: the
/// identification's own error at the engage, with nothing learned after it.
const CARRIED_DB: f64 = 0.5;

/// The worst |error| over the judged updates at `level_db`.
fn worst_at(r: &Run, level_db: f64) -> (usize, f64) {
    let judged: Vec<_> = r
        .ticks
        .iter()
        .filter(|t| t.level_db == level_db && !t.settling && t.miss.is_none())
        .collect();
    let w = judged
        .iter()
        .flat_map(|t| t.errors_db().iter().map(|e| e.abs()).collect::<Vec<_>>())
        .fold(0.0f64, f64::max);
    (judged.len(), w)
}

#[test]
fn mimo_sine_a_ladder_step_carries_the_drive_without_the_laws_help() {
    let s = SineStructure::Linear4x4;
    let mut st = setup(s, dwell(130.0, 30.0), "matrix");
    st.schedule.ladder = LadderConfig {
        sysid_level_db: -20.0,
        start_db: -20.0,
        table_db: vec![-20.0, -12.0, -6.0, 0.0],
        per_rung: RungFor::Seconds(1.5),
        full_for: RungFor::Seconds(1.5),
    };
    let r = run(engaged(s, st, Arc::new(Frozen(law("matrix"))), NOISE), 40.0);
    assert_eq!(r.outcome, Some(Outcome::Completed), "{:?}", r.outcome);
    for db in [-20.0, -12.0, -6.0, 0.0] {
        let (n, w) = worst_at(&r, db);
        println!("{db} dB: {n} judged updates, worst {w:.3} dB");
        assert!(n > 50, "{db} dB: {n} judged updates");
        assert!(w < CARRIED_DB, "{db} dB: {w}");
    }
    // The carry moved no drive by more than the change cap in one update.
    let cap = GuardSettings::default().max_step_db;
    for w in r.ticks.windows(2) {
        for (a, b) in w[0].drives().iter().zip(w[1].drives()) {
            if a.norm() > 0.0 && b.norm() > 0.0 && !w[1].engage {
                let step = 20.0 * (b.norm() / a.norm()).log10();
                assert!(step.abs() <= cap + 1e-9, "{step}");
            }
        }
    }
}

#[test]
fn mimo_sine_a_manual_level_step_carries_the_drive_the_same_way() {
    let s = SineStructure::Linear4x4;
    let mut st = setup(s, dwell(130.0, 30.0), "matrix");
    st.schedule.mode = LevelMode::Manual;
    st.schedule.ladder = LadderConfig {
        sysid_level_db: -20.0,
        start_db: -12.0,
        table_db: vec![-12.0, -6.0, 0.0],
        per_rung: RungFor::Seconds(1e6),
        full_for: RungFor::Seconds(1e6),
    };
    let mut svc = engaged(s, st, Arc::new(Frozen(law("matrix"))), NOISE);
    let (rt, re) = svc.take_records().unwrap();
    let cmd = svc.commands();
    let step = |svc: &mut service::MimoSineService, secs: f64| {
        for _ in 0..(secs * FS / 512.0) as usize {
            svc.run_batches(1).unwrap();
        }
    };
    step(&mut svc, 2.0);
    cmd.send(Command::StepLevel(6.0)).unwrap();
    step(&mut svc, 2.0);
    cmd.send(Command::StepLevel(-6.0)).unwrap();
    step(&mut svc, 2.0);
    let r = Run {
        ticks: rt.try_iter().collect(),
        events: re.try_iter().collect(),
        outcome: None,
        service: svc,
    };
    let levels: Vec<f64> = r
        .events
        .iter()
        .filter(|e| e.kind == EventKind::Level)
        .map(|e| e.level_db)
        .collect();
    assert_eq!(levels, vec![-6.0, -12.0]);
    assert!(!r
        .events
        .iter()
        .any(|e| matches!(e.kind, EventKind::Stopping { .. })));
    // After each step's settle window, every control is at the new level.
    let after_up: Vec<_> = r
        .ticks
        .iter()
        .filter(|t| t.time_s > 2.0 && t.time_s < 4.0 && !t.settling && t.level_db == -6.0)
        .collect();
    let after_down: Vec<_> = r
        .ticks
        .iter()
        .filter(|t| t.time_s > 4.0 && !t.settling && t.level_db == -12.0)
        .collect();
    for (name, ts) in [("up", after_up), ("down", after_down)] {
        assert!(ts.len() > 50, "{name}: {}", ts.len());
        let w = ts
            .iter()
            .flat_map(|t| t.errors_db().iter().map(|e| e.abs()).collect::<Vec<_>>())
            .fold(0.0f64, f64::max);
        println!("manual step {name}: worst {w:.3} dB");
        assert!(w < CARRIED_DB, "{name}: {w}");
    }
}
