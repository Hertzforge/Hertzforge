//! `laws.check` of a MIMO Sine law: the laws crate's cases
//! (`hertzforge_mimo_sine_laws::check`), each run on the simulator's sine
//! structure it names and judged by that crate's `judge`, one `laws.check`
//! event as each case starts and as it ends, and the report on the wire. A law
//! file's scored verdicts are recorded against its exact file by the caller;
//! the hardware arming gate reads that record, as every other world's does.

use crate::protocol::{num, MethodError};
use hertzforge_hal::Backend;
use hertzforge_mimo_sine::MimoSineLawFactory;
use hertzforge_mimo_sine_laws::check::{
    self as mcheck, CaseVerdict, StationSpec, StructureEdit, MIMO_SINE_CHECK_VERSION,
};
use serde_json::{json, Value};
use std::sync::Arc;

/// The simulator's station of a case: its sine structure, edited as the case
/// says, its delay, noise and seed.
pub fn station(spec: &StationSpec) -> Result<Box<dyn Backend>, String> {
    use hertzforge_hal_sim::plants::SineStructure;
    use hertzforge_hal_sim::PlantModel;
    let s = SineStructure::from_name(spec.structure)
        .ok_or_else(|| format!("no standard sine structure {}", spec.structure))?;
    let mut model = s.build().model;
    if spec.edit != StructureEdit::None || spec.transport_delay_s > 0.0 {
        let PlantModel::Jointed(p) = &mut model else {
            return Err(format!("{} is not a modal structure", spec.structure));
        };
        match spec.edit {
            StructureEdit::None => {}
            StructureEdit::MassLineOnly => p.modes.clear(),
            StructureEdit::MoveMode { from_hz, to_hz } => {
                let k = (0..p.modes.len())
                    .min_by(|&a, &b| {
                        (p.modes[a].f_hz - from_hz)
                            .abs()
                            .total_cmp(&(p.modes[b].f_hz - from_hz).abs())
                    })
                    .ok_or("the structure has no mode")?;
                p.modes[k].f_hz = to_hz;
            }
        }
        p.transport_delay_samples = (spec.transport_delay_s * spec.fs_hz).round() as usize;
    }
    Ok(Box::new(
        hertzforge_hal_sim::SimConfig::new()
            .sample_rate_hz(spec.fs_hz)
            .plant(model)
            .noise_level(spec.noise_g)
            .seed(spec.seed)
            .shared_clock(true)
            .build(),
    ))
}

/// One case's line in the check report.
pub fn case_json(v: &CaseVerdict) -> Value {
    json!({
        "case": v.case.name(),
        "structure": v.case.describe(),
        "scored": v.scored,
        "passed": v.passed,
        "failed": v.failed,
        "number": num(crate::views::round_to(v.number, 3)),
        "detail": v.detail,
    })
}

/// Run the check of `factory`'s law (catalog id `law_id`): every case its
/// manifest accepts, telling `progress` as each starts and ends.
pub fn run_check(
    law_id: &str,
    factory: Arc<dyn MimoSineLawFactory>,
    progress: &mut dyn FnMut(Value),
) -> Result<Vec<CaseVerdict>, MethodError> {
    let cases = mcheck::cases_for(&factory.manifest());
    let of = cases.len();
    let mut out = Vec::with_capacity(of);
    for (index, case) in cases.into_iter().enumerate() {
        progress(json!({"law_id": law_id, "index": index, "of": of, "case": case.name(),
                        "state": "running", "result": null}));
        let ran = station(&case.station())
            .map_err(mcheck::CheckRunError::Setup)
            .and_then(|b| mcheck::run_check_case(case, factory.clone(), b));
        let v = match ran {
            Ok(m) => mcheck::judge(&m),
            Err(e) => CaseVerdict {
                case,
                scored: case.scored(),
                passed: false,
                failed: Some("ran".into()),
                number: f64::NAN,
                detail: e.to_string(),
            },
        };
        progress(json!({"law_id": law_id, "index": index, "of": of, "case": case.name(),
                        "state": "done", "result": case_json(&v)}));
        out.push(v);
    }
    Ok(out)
}

/// The `laws.check` report: passed when every scored case passed.
pub fn report_json(verdicts: &[CaseVerdict], recorded: bool) -> Value {
    json!({
        "passed": !verdicts.is_empty() && mcheck::check_passed(verdicts),
        "recorded": recorded,
        "tool_version": MIMO_SINE_CHECK_VERSION,
        "cases": verdicts.iter().map(case_json).collect::<Vec<_>>(),
    })
}
