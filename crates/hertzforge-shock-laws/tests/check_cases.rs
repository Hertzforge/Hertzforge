//! The classical law check, run with the built-in law on the simulator: every
//! case passes. And a law that does not correct fails the cases built to catch
//! it.

mod common;

use common::station_for;
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault};
use hertzforge_shock_host::{
    ClassicalLaw, ClassicalLawFactory, ClassicalLawSetup, FdCorrectFactory, PulseInputs,
    PulseOutputs,
};
use hertzforge_shock_laws::{
    classical_check_record, judge_classical, run_classical_check_case, ClassicalCheckCase,
};
use std::sync::Arc;

#[test]
fn the_built_in_law_passes_every_case() {
    let mut cases = Vec::new();
    for case in ClassicalCheckCase::ALL {
        let m = run_classical_check_case(
            case,
            Arc::new(FdCorrectFactory::default()),
            station_for(case),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", case.name()));
        let j = judge_classical(case, &m);
        assert!(j.passed, "{}: {} ({m:?})", case.name(), j.detail);
        cases.push(j);
    }
    let r = classical_check_record("00".repeat(32), "2026-11-07T00:00:00Z", cases);
    assert!(r.passed);
}

/// The seed, and then the same drive forever.
struct Frozen(Box<dyn ClassicalLaw + Send>, Vec<f64>);
impl ClassicalLaw for Frozen {
    fn name(&self) -> &str {
        "frozen"
    }
    fn seed(&mut self, i: &PulseInputs<'_>) -> Result<PulseOutputs, LawFault> {
        let o = self.0.seed(i)?;
        self.1 = o.drive.clone();
        Ok(PulseOutputs {
            drive: o.drive,
            values: Vec::new(),
        })
    }
    fn iterate(&mut self, _: &PulseInputs<'_>) -> Result<PulseOutputs, LawFault> {
        Ok(PulseOutputs {
            drive: self.1.clone(),
            values: Vec::new(),
        })
    }
}
struct FrozenFactory;
impl ClassicalLawFactory for FrozenFactory {
    fn build(&self, s: &ClassicalLawSetup<'_>) -> Result<Box<dyn ClassicalLaw + Send>, LawFault> {
        Ok(Box::new(Frozen(
            FdCorrectFactory::default().build(s)?,
            Vec::new(),
        )))
    }
    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: "frozen".into(),
            name: "frozen".into(),
            version: "0".into(),
            sha256: None,
            params: serde_json::json!({}),
        }
    }
}

#[test]
fn a_law_that_does_not_correct_fails_the_identification_error_case() {
    let case = ClassicalCheckCase::IdentificationError;
    let m = run_classical_check_case(case, Arc::new(FrozenFactory), station_for(case)).unwrap();
    let j = judge_classical(case, &m);
    assert!(!j.passed, "{}", j.detail);
    assert!(j.detail.contains("no pulse passed"), "{}", j.detail);
    assert_eq!(
        m.pulses, 8,
        "eight pulses at the first rung, then not converged"
    );
}
