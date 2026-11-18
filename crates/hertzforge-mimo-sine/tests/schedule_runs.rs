//! The schedule on the station: the level ladder, the operator's hold, resume
//! and manual frequency, and a law's request to hold or slow the sweep.

mod common;

use common::*;
use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_mimo_sine::core::Command;
use hertzforge_mimo_sine::law::{LawDescriptor, LawManifest, LawSetup, LawValues};
use hertzforge_mimo_sine::records::{EventKind, Outcome};
use hertzforge_mimo_sine::schedule::RequestOutcome;
use hertzforge_mimo_sine::*;
use hertzforge_sine::scheduler::{
    BidirSweepConfig, LadderConfig, RungFor, SweepDirection, SweepTarget,
};
use hertzforge_sine::SweepRate;
use std::sync::Arc;

fn sweep(rate: f64) -> TestPlan {
    TestPlan::Sweep(BidirSweepConfig {
        band: (40.0, 400.0),
        rate: SweepRate::LogOctPerMin(rate),
        start_hz: 60.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1),
    })
}

#[test]
fn mimo_sine_level_ladder_steps_up_and_the_controls_follow() {
    let s = SineStructure::Linear4x4;
    let mut st = setup(s, dwell(96.0, 30.0), "matrix");
    st.schedule.ladder = LadderConfig {
        sysid_level_db: -20.0,
        start_db: -12.0,
        table_db: vec![-12.0, -6.0, 0.0],
        per_rung: RungFor::Seconds(2.0),
        full_for: RungFor::Seconds(2.0),
    };
    let r = run(engaged(s, st, law("matrix"), NOISE), 40.0);
    assert_eq!(r.outcome, Some(Outcome::Completed));
    let levels: Vec<f64> = r
        .events
        .iter()
        .filter(|e| e.kind == EventKind::Level)
        .map(|e| e.level_db)
        .collect();
    assert_eq!(levels, vec![-6.0, 0.0]);
    // At the end of each rung (its last update, the settle after each step
    // not counted in the rung's time) every control sits at its level.
    for db in [-12.0, -6.0, 0.0] {
        let t = r
            .ticks
            .iter()
            .rfind(|t| t.level_db == db && !t.settling)
            .unwrap();
        let want = 10f64.powf(db / 20.0);
        for c in t.controls() {
            assert!(
                (20.0 * (c.norm() / want).log10()).abs() < 0.5,
                "{db} dB: {c}"
            );
        }
    }
}

#[test]
fn mimo_sine_hold_resume_and_manual_frequency_move_the_tone() {
    let s = SineStructure::Linear4x4;
    let mut svc = engaged(s, setup(s, sweep(2.0), "matrix"), law("matrix"), NOISE);
    let (rt, _re) = svc.take_records().unwrap();
    let cmd = svc.commands();
    let mut ticks = Vec::new();
    let step = |svc: &mut service::MimoSineService, secs: f64| {
        for _ in 0..(secs * FS / 512.0) as usize {
            svc.run_batches(1).unwrap();
        }
    };
    step(&mut svc, 2.0);
    cmd.send(Command::Hold).unwrap();
    step(&mut svc, 2.0);
    cmd.send(Command::Resume).unwrap();
    step(&mut svc, 1.0);
    cmd.send(Command::GoTo(200.0)).unwrap();
    step(&mut svc, 2.0);
    ticks.extend(rt.try_iter());
    let at = |t_s: f64| {
        ticks
            .iter()
            .find(|t| (t.time_s - t_s).abs() < 0.006)
            .unwrap()
    };
    // Held: the frequency stands still and the sweep rate the law is told is 0.
    let (a, b) = (at(2.5).frequency_hz, at(3.9).frequency_hz);
    assert_eq!(a, b);
    assert_eq!(at(3.0).sweep_oct_per_min, 0.0);
    // Resumed: it moves again at two octaves a minute.
    let (c, d) = (at(4.2).frequency_hz, at(4.9).frequency_hz);
    assert!((d / c).log2() * 60.0 / 0.7 > 1.9, "{c} {d}");
    // Manual frequency: the tone is at 200 Hz, the law engaged there, and
    // the sweep goes on from it.
    let j = ticks.iter().find(|t| t.time_s > 5.0 && t.engage).unwrap();
    // (the sweep has moved on by one update's worth from where it was put)
    assert!((j.frequency_hz - 200.0).abs() < 0.1, "{}", j.frequency_hz);
    assert!(at(6.9).frequency_hz > 200.0);
    let worst = ticks
        .iter()
        .filter(|t| t.time_s > 6.0 && !t.settling)
        .flat_map(|t| t.errors_db().to_vec())
        .fold(0.0f64, |a, e| a.max(e.abs()));
    assert!(worst < 3.0, "{worst}");
}

/// `matrix`, asking every update for a hold or a slow.
struct Asking(Arc<dyn MimoSineLawFactory>, Request);

struct AskingLaw(Box<dyn MimoSineLaw + Send>, Request);

impl MimoSineLaw for AskingLaw {
    fn name(&self) -> &str {
        "asking"
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
        v: &mut LawValues,
    ) -> Result<(), LawFault> {
        self.0.tick(x, o, v)?;
        o.request = Some(self.1);
        Ok(())
    }
}

impl MimoSineLawFactory for Asking {
    fn build(&self, s: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        Ok(Box::new(AskingLaw(self.0.build(s)?, self.1)))
    }
    fn describe(&self) -> LawDescriptor {
        self.0.describe()
    }
    fn manifest(&self) -> LawManifest {
        self.0.manifest()
    }
}

fn asking_run(req: Request, honour: bool, secs: f64) -> Run {
    let s = SineStructure::Linear4x4;
    let mut st = setup(s, sweep(2.0), "matrix");
    st.requests.honour = honour;
    let f = Arc::new(Asking(law("matrix"), req));
    let mut svc = engaged(s, st, f, NOISE);
    let (rt, re) = svc.take_records().unwrap();
    for _ in 0..(secs * FS / 512.0) as usize {
        svc.run_batches(1).unwrap();
    }
    Run {
        ticks: rt.try_iter().collect(),
        events: re.try_iter().collect(),
        outcome: None,
        service: svc,
    }
}

#[test]
fn mimo_sine_a_law_request_is_recorded_and_honoured_only_when_allowed() {
    // Off: recorded, not acted on; the sweep moves.
    let r = asking_run(Request::Hold, false, 6.0);
    let t = r
        .ticks
        .iter()
        .find(|t| t.time_s > 4.0 && !t.settling)
        .unwrap();
    assert_eq!(t.request, Some(Request::Hold));
    assert_eq!(t.request_outcome, RequestOutcome::NotHonoured);
    assert!(r.ticks.last().unwrap().frequency_hz > 60.0 * 1.05);
    // On: the sweep holds at most two seconds without a break, then the
    // request is refused for an update and the hold may begin again.
    let r = asking_run(Request::Hold, true, 6.0);
    let granted: Vec<_> = r
        .ticks
        .iter()
        .filter(|t| t.request_outcome == RequestOutcome::Granted)
        .collect();
    assert!(!granted.is_empty());
    assert!(r
        .ticks
        .iter()
        .any(|t| t.request_outcome == RequestOutcome::HoldTooLong));
    let mut run_s = 0.0f64;
    let mut longest = 0.0f64;
    for t in &r.ticks {
        if t.request_outcome == RequestOutcome::Granted {
            run_s += 0.01;
        } else {
            run_s = 0.0;
        }
        longest = longest.max(run_s);
    }
    assert!(longest <= 2.0 + 1e-9, "{longest}");
    assert!(r
        .events
        .iter()
        .any(|e| e.kind == EventKind::Held { by_law: true }));
    // A slow under the floor is raised to it.
    let r = asking_run(Request::Slow(0.01), true, 6.0);
    let t = r
        .ticks
        .iter()
        .find(|t| t.time_s > 4.0 && !t.settling)
        .unwrap();
    assert_eq!(t.request_outcome, RequestOutcome::Bounded);
    assert!(
        (t.sweep_oct_per_min - 0.2).abs() < 1e-9,
        "{}",
        t.sweep_oct_per_min
    );
}
