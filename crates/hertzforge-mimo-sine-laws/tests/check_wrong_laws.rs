//! Laws that are wrong on purpose fail the check, each by the requirement
//! its fault breaks: `matrix` with its correction's sign turned over, a law
//! that asks a drive over its ceiling, and a law that answers with one drive
//! too few.

use hertzforge_mimo_sine::law::{LawDescriptor, LawManifest, LawSetup, LawValues};
use hertzforge_mimo_sine::laws::builtin;
use hertzforge_mimo_sine::{LawFault, MimoSineLaw, MimoSineLawFactory, TickInputs, TickOutputs};
use hertzforge_mimo_sine_laws::check::{
    check_law, check_passed, sim_station, MimoSineCheckCase, CHECK_CEILING_V,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
enum Wrong {
    /// The correction the law asks for, applied the other way.
    Sign,
    /// From the 150th update the largest drive is asked at 1.2 times its
    /// ceiling.
    Ceiling,
    /// From the 150th update one drive too few.
    Count,
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
        self.0.engage(x, o, v)
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
    let r = check_law(law, sim_station);
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
