//! Laws that are wrong on purpose fail the check, each by the requirement
//! its fault breaks: `matrix` with its correction's sign turned over, a law
//! that asks a drive over its ceiling, and a law that answers with one drive
//! too few. Those run with the law's time budget relaxed by the engine's
//! named test-only switch, `TEST_RELAX_COMPUTE_BUDGET`.
//!
//! One test holds a law that comes late to the setup's real budget, as a
//! check on a user's machine does: a single late answer is held and the case
//! passes, a law late on every update fails `on_time`, not `no_law_fault`, and
//! a law late at its engage fails `on_time` after its one call.
//! It sleeps on the wall clock, so it runs on the timing list.

use hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET;
use hertzforge_mimo_sine::law::{LawDescriptor, LawManifest, LawSetup, LawValues};
use hertzforge_mimo_sine::laws::builtin;
use hertzforge_mimo_sine::{LawFault, MimoSineLaw, MimoSineLawFactory, TickInputs, TickOutputs};
use hertzforge_mimo_sine_laws::check::{
    call_times, check_case, check_case_with, check_law_with, check_passed, sim_station,
    MimoSineCheckCase, CHECK_CEILING_V,
};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
enum Wrong {
    /// The correction the law asks for, applied the other way.
    Sign,
    /// From the 150th update the largest drive is asked at 1.2 times its
    /// ceiling.
    Ceiling,
    /// From the 150th update one drive too few.
    Count,
    /// At the 150th update only, 5 ms late (twice the setup's budget).
    LateOnce,
    /// From the 150th update, 5 ms late on every update.
    LateAlways,
    /// 5 ms late at the engage only.
    LateAtEngage,
}

struct WrongFactory(Arc<dyn MimoSineLawFactory>, Wrong);

struct WrongLaw(Box<dyn MimoSineLaw + Send>, Wrong);

impl MimoSineLaw for WrongLaw {
    fn name(&self) -> &str {
        "wrong"
    }
    fn engage(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut LawValues,
    ) -> Result<(), LawFault> {
        self.0.engage(x, o, v)?;
        if let Wrong::LateAtEngage = self.1 {
            std::thread::sleep(LATE);
        }
        Ok(())
    }
    fn tick(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut LawValues,
    ) -> Result<(), LawFault> {
        self.0.tick(x, o, v)?;
        match self.1 {
            Wrong::Sign => {
                for (d, f) in o.drive.iter_mut().zip(x.drive_in_force) {
                    *d = f * 2.0 - *d;
                }
            }
            Wrong::Ceiling if x.index >= 150 => {
                let big = o.drive.iter().map(|d| d.norm()).fold(0.0, f64::max);
                let k = 1.2 * CHECK_CEILING_V / big.max(1e-12);
                o.drive.iter_mut().for_each(|d| *d *= k);
            }
            Wrong::Count if x.index >= 150 => {
                o.drive.pop();
            }
            Wrong::LateOnce if x.index == 150 => std::thread::sleep(LATE),
            Wrong::LateAlways if x.index >= 150 => std::thread::sleep(LATE),
            _ => {}
        }
        Ok(())
    }
}

impl MimoSineLawFactory for WrongFactory {
    fn build(&self, s: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        Ok(Box::new(WrongLaw(self.0.build(s)?, self.1)))
    }
    fn describe(&self) -> LawDescriptor {
        self.0.describe()
    }
    fn manifest(&self) -> LawManifest {
        self.0.manifest()
    }
}

/// Run the check on `matrix` made wrong by `w`: the check fails, and at
/// least one case fails by `rule`. (A law that overdrives may first take a
/// control out of its abort band, so some of its cases fail `no_abort`.)
fn fails(w: Wrong, rule: &str) {
    let law = Arc::new(WrongFactory(builtin("matrix").unwrap(), w));
    let r = check_law_with(law, sim_station, Some(TEST_RELAX_COMPUTE_BUDGET));
    for c in &r {
        println!(
            "{w:?} {:27} {:5} {:16} {}",
            c.verdict.case.name(),
            c.verdict.passed,
            c.verdict.failed.as_deref().unwrap_or(""),
            c.verdict.detail
        );
    }
    let v: Vec<_> = r.iter().map(|c| c.verdict.clone()).collect();
    assert!(!check_passed(&v), "{w:?} passed the check");
    assert!(
        v.iter().any(|v| v.failed.as_deref() == Some(rule)),
        "{w:?}: no case failed {rule}"
    );
    // Every scored case the law ran fails, except the two that ask only
    // that the run end safely.
    for v in v.iter().filter(|v| v.scored) {
        let safe_end = matches!(
            v.case,
            MimoSineCheckCase::NearSingular | MimoSineCheckCase::LawMisbehaves
        );
        assert!(!v.passed || safe_end, "{w:?} passed {}", v.case.name());
    }
}

#[test]
fn mimo_sine_check_a_law_with_its_gain_the_wrong_way_fails() {
    fails(Wrong::Sign, "no_abort");
}

#[test]
fn mimo_sine_check_a_law_that_ignores_the_ceiling_fails() {
    fails(Wrong::Ceiling, "within_ceiling");
}

#[test]
fn mimo_sine_check_a_law_that_returns_the_wrong_count_fails() {
    fails(Wrong::Count, "no_law_fault");
}

/// How late the late laws answer: twice the setup's budget of a quarter of
/// the 10 ms update.
const LATE: Duration = Duration::from_millis(5);

#[test]
fn mimo_sine_check_a_late_law_is_held_to_the_real_budget() {
    let case = MimoSineCheckCase::DwellAtResonance;
    let run = |w: Wrong| {
        let law = Arc::new(WrongFactory(builtin("matrix").unwrap(), w));
        let r = check_case(case, law, &mut sim_station);
        let m = r.measure.clone().expect("the case ran");
        println!(
            "{w:?} {} {} {}; {}",
            r.verdict.passed,
            r.verdict.failed.as_deref().unwrap_or(""),
            r.verdict.detail,
            call_times(&m)
        );
        (r.verdict, m)
    };
    // One answer late: refused, held, and the case passes on the rest.
    let (v, m) = run(Wrong::LateOnce);
    assert!(v.passed, "{v:?}");
    assert!(m.late_misses >= 1 && m.misses == m.late_misses, "{m:?}");
    assert!(m.calls_over_budget >= 1);
    assert!(m.slowest_call_s >= LATE.as_secs_f64());
    assert!((m.budget_in_force_s - m.law_budget_s).abs() < 1e-12);
    // Late on every update: the engine's miss rule ends the run, and the
    // check names the time, not the law's answers.
    let (v, m) = run(Wrong::LateAlways);
    assert_eq!(v.failed.as_deref(), Some("on_time"), "{v:?}");
    assert!(m.ended_late);
    assert_eq!(m.misses, m.late_misses);
    assert_eq!(m.late_in_a_row, m.misses, "{m:?}");
    assert!(!m.ended_at_engage);
    assert!(
        v.detail
            .contains(&format!("after {} late answers in a row", m.late_in_a_row)),
        "{v:?}"
    );
    // Late at the engage: the engage holds no miss, so the run ends after
    // one call, and the detail says that rather than the miss rule's count.
    let (v, m) = run(Wrong::LateAtEngage);
    assert_eq!(v.failed.as_deref(), Some("on_time"), "{v:?}");
    assert!(m.ended_late && m.ended_at_engage, "{m:?}");
    assert_eq!(
        (m.calls, m.misses, m.late_misses, m.late_in_a_row),
        (1, 1, 1, 1)
    );
    assert!(
        v.detail.contains("on a late answer at an engage") && !v.detail.contains("in a row"),
        "{v:?}"
    );
    // Under the engine's test-only budget the same late engage is taken and
    // the case runs its whole dwell: a slow call on a busy machine cannot end
    // a test's run early.
    let law = Arc::new(WrongFactory(
        builtin("matrix").unwrap(),
        Wrong::LateAtEngage,
    ));
    let r = check_case_with(case, law, &mut sim_station, Some(TEST_RELAX_COMPUTE_BUDGET));
    let m = r.measure.expect("the case ran");
    assert!(r.verdict.passed, "{:?}", r.verdict);
    assert!(m.calls > 500 && m.misses == 0, "{m:?}");
    assert!(m.calls_over_budget >= 1, "{m:?}");
}
