//! The core's per-call guard arithmetic on a Sine law: time a call, contain a
//! panic, check the output, cap the change. Pure functions, so a core that runs
//! several tones (one law instance each) applies the same rules as the Sine core.

use crate::law::{
    LawClock, LawFault, LawGuards, LawValues, SineControlLaw, SineOutputRule, TickInputs,
    TickOutputs,
};

/// What a panic carried, as text.
pub fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "the law panicked".to_string()
    }
}

/// Call the law once (`engage` or `tick`) into a cleared values buffer.
/// Under the guards (`timing`: the clock and the budget) the call is timed,
/// a panic is caught, and the output checked; without them it is a plain
/// call. Returns the outcome and the call's time in seconds (0 untimed).
pub fn call_law(
    law: &mut dyn SineControlLaw,
    engage: bool,
    inputs: &TickInputs<'_>,
    values: &mut LawValues,
    timing: Option<(&dyn LawClock, f64)>,
) -> (Result<TickOutputs, LawFault>, f64) {
    values.clear();
    let Some((clock, budget_s)) = timing else {
        let r = if engage {
            law.engage(inputs, values)
        } else {
            law.tick(inputs, values)
        };
        return (r, 0.0);
    };
    let t0 = clock.now_s();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if engage {
            law.engage(inputs, values)
        } else {
            law.tick(inputs, values)
        }
    }));
    let took_s = (clock.now_s() - t0).max(0.0);
    let r = match caught {
        Err(p) => Err(LawFault::Crashed(panic_text(&*p))),
        Ok(Err(fault)) => Err(fault),
        Ok(Ok(_)) if took_s > budget_s => Err(LawFault::OverBudget { took_s, budget_s }),
        Ok(Ok(out)) => match output_rule(&out, values) {
            Some(rule) => Err(LawFault::BadOutput(rule)),
            None => Ok(out),
        },
    };
    (r, took_s)
}

/// The output rule a call broke, if any: the drive finite and not below
/// zero, every value finite. (An undeclared value is refused when the law
/// sets it.)
pub fn output_rule(out: &TickOutputs, values: &LawValues) -> Option<SineOutputRule> {
    if !out.drive.is_finite() {
        return Some(SineOutputRule::NonFinite);
    }
    if out.drive < 0.0 {
        return Some(SineOutputRule::Negative);
    }
    if values.iter().any(|(_, v)| !v.is_finite()) {
        return Some(SineOutputRule::NonFinite);
    }
    None
}

/// Cap a law's drive against the drive in force: at most `max_step_db`
/// either way and at most `max_rise_db_per_s * dt` up. Returns the capped
/// drive and whether a cap bound. A change within a billionth of a dB of a
/// cap does not count as binding (the shipped law's own 1 dB clamp lands on
/// the default cap exactly).
pub fn cap_drive(in_force: f64, proposed: f64, g: &LawGuards, dt: f64) -> (f64, bool) {
    const EPS_DB: f64 = 1e-9;
    if in_force.is_nan() || in_force <= 0.0 {
        return (proposed, false);
    }
    let up = g.max_step_db.min(g.max_rise_db_per_s * dt);
    let down = g.max_step_db;
    let step_db = if proposed > 0.0 {
        20.0 * (proposed / in_force).log10()
    } else {
        f64::NEG_INFINITY
    };
    if step_db > up + EPS_DB {
        (in_force * 10f64.powf(up / 20.0), true)
    } else if step_db < -(down + EPS_DB) {
        (in_force * 10f64.powf(-down / 20.0), true)
    } else {
        (proposed, false)
    }
}
