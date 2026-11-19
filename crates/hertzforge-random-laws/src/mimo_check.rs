//! The check of a multiple-output law before it drives hardware: the fixed set
//! of simulated test articles, what a run on each is measured by, and the rules
//! that turn a measurement into a passed or failed case of a [`CheckRecord`].
//!
//! The runs themselves are made by the controller, which owns the MIMO run
//! service and the simulator: it runs the law under test on each plant of
//! [`MimoCheckCase::ALL`] in process, under the core's guards, fills one
//! [`MimoCaseMeasure`] per plant, and calls [`judge`] on it. This module holds
//! everything that decides the verdict, so the rules are the same whoever runs
//! the plants, and are tested here without running one.
//!
//! Every case asks the same of the law first: every call answered, in time,
//! with a drive the core could use, and no protection tripped. Then each case
//! asks for its [`Requirement`]: convergence to the reference on the square
//! plants, a steady drive near it where there are more controls than drives,
//! and survival on the ill-conditioned plant.
//!
//! A pass is recorded against the law file's SHA-256 exactly as a single-output
//! check is ([`mimo_check_record`]), so [`crate::may_arm_on_hardware`] governs a
//! multiple-output law the same way: the simulator runs any law; hardware arms
//! only with a law whose exact file passed.

use crate::check::{CheckCase, CheckRecord};

/// The version of the multiple-output check written into its records. A change
/// to the plant set or to the rules changes it.
pub const MIMO_CHECK_VERSION: &str = "mimo-check 1";

/// Every control's in-band rms error must come within this, dB.
pub const CONVERGE_DB: f64 = 1.0;

/// Loops at full level a run is given.
pub const MAX_LOOPS_AT_FULL: u64 = 20;

/// Loop-to-loop change, dB, under which a control's error counts as settled.
pub const SETTLE_DB: f64 = 0.5;

/// The loops at full level a settled run is judged over.
pub const SETTLE_LOOPS: usize = 3;

/// A settled run's worst control must be within this, dB (the check's alarm
/// lines).
pub const SETTLE_MAX_DB: f64 = 6.0;

/// What a case asks of the law beyond answering every call, in time, with a
/// usable drive, and tripping nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    /// Every control within [`CONVERGE_DB`] of the reference within
    /// [`MAX_LOOPS_AT_FULL`] loops at full level. The square plants: the
    /// reference is one the plant reaches, and every documented method reaches
    /// it.
    Converge,
    /// Converge, or settle: no control's error changes by more than
    /// [`SETTLE_DB`] loop to loop over the last [`SETTLE_LOOPS`] loops, with
    /// every control within [`SETTLE_MAX_DB`]. With more controls than drives a
    /// reference is in general not exactly reachable, and the documented
    /// methods aim at different compromises (least squares, the plant's leading
    /// directions, the structure's own coherence), so the check asks for a
    /// steady drive near the reference, not for one compromise.
    Settle,
    /// As [`Requirement::Settle`], or the run ended by the core's own judgement
    /// (divergence, or the change cap holding the drive loop after loop), which
    /// is reported. On a plant that fails the identification gate that stop is
    /// the core's protection doing its job; what the case checks is that a
    /// nearly singular plant estimate never makes the law miss a call.
    Survive,
}

/// One simulated test article of the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MimoCheckCase {
    /// The well-conditioned 4 x 4 modal plant.
    Modal4x4,
    /// The well-conditioned 12 x 6 modal plant (more controls than drives).
    Modal12x6,
    /// The ill-conditioned 12 x 6 modal plant: two drives act through nearly
    /// the same shape.
    Modal12x6Ill,
    /// The rigid plate, 12 x 6, in its rigid-body band.
    RigidPlate12x6,
    /// The 4 x 4 modal plant with measurement noise 30 dB under the response.
    Noisy4x4,
    /// The 12 x 6 modal plant on a level ladder from -20 dB to full level.
    Ladder12x6,
}

impl MimoCheckCase {
    /// Every case, in the order the check runs them.
    pub const ALL: [MimoCheckCase; 6] = [
        MimoCheckCase::Modal4x4,
        MimoCheckCase::Modal12x6,
        MimoCheckCase::Modal12x6Ill,
        MimoCheckCase::RigidPlate12x6,
        MimoCheckCase::Noisy4x4,
        MimoCheckCase::Ladder12x6,
    ];

    /// The case's name in a record and on screen.
    pub fn name(self) -> &'static str {
        match self {
            MimoCheckCase::Modal4x4 => "modal 4 x 4",
            MimoCheckCase::Modal12x6 => "modal 12 x 6",
            MimoCheckCase::Modal12x6Ill => "ill-conditioned 12 x 6",
            MimoCheckCase::RigidPlate12x6 => "rigid plate 12 x 6",
            MimoCheckCase::Noisy4x4 => "measurement noise 4 x 4",
            MimoCheckCase::Ladder12x6 => "level ladder 12 x 6",
        }
    }

    /// The simulator's standard plant the case runs on (`modal-4x4`, ...).
    pub fn plant(self) -> &'static str {
        match self {
            MimoCheckCase::Modal4x4 | MimoCheckCase::Noisy4x4 => "modal-4x4",
            MimoCheckCase::Modal12x6 | MimoCheckCase::Ladder12x6 => "modal-12x6",
            MimoCheckCase::Modal12x6Ill => "modal-12x6-ill",
            MimoCheckCase::RigidPlate12x6 => "rigid-plate-12x6",
        }
    }

    /// What the case asks of the law.
    pub fn requirement(self) -> Requirement {
        match self {
            MimoCheckCase::Modal4x4 | MimoCheckCase::Noisy4x4 => Requirement::Converge,
            MimoCheckCase::Modal12x6
            | MimoCheckCase::RigidPlate12x6
            | MimoCheckCase::Ladder12x6 => Requirement::Settle,
            MimoCheckCase::Modal12x6Ill => Requirement::Survive,
        }
    }

    /// Measurement noise in band, dB under the smallest control's response at
    /// full level (`None`: the simulator's floor only).
    pub fn noise_below_db(self) -> Option<f64> {
        match self {
            MimoCheckCase::Noisy4x4 => Some(30.0),
            _ => None,
        }
    }

    /// The level schedule, dB, the last entry full level.
    pub fn levels_db(self) -> &'static [f64] {
        match self {
            MimoCheckCase::Ladder12x6 => &[-20.0, -16.0, -12.0, -8.0, -4.0, 0.0],
            _ => &[-6.0, -3.0, 0.0],
        }
    }

    /// The case with this name, if it is one of the set.
    pub fn from_name(name: &str) -> Option<MimoCheckCase> {
        MimoCheckCase::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// How the protection that ended a run is judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TripClass {
    /// No protection ended the run.
    #[default]
    None,
    /// The core's own judgement of the loop: divergence, or the change cap
    /// holding the drive loop after loop.
    CoreJudgement,
    /// Anything else: the envelope, a drive's voltage, a kinematic or limit
    /// abort, a law fault, the plant.
    Other,
}

/// What one run of the law on one plant measured. The runner fills every
/// field; [`judge`] reads them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MimoCaseMeasure {
    /// Why the run could not be made at all (the law refused the setup, the
    /// service refused a step), if it could not.
    pub setup_error: Option<String>,
    /// Whether the run reached full level.
    pub reached_full_level: bool,
    /// Loops at full level until every control's in-band rms error was within
    /// [`CONVERGE_DB`] (`None`: not within the loops run).
    pub loops_to_converge: Option<u64>,
    /// Loops run at full level.
    pub loops_at_full: u64,
    /// The largest in-band rms error of any control at the end, dB.
    pub final_error_db: f64,
    /// The largest error of any control on any band line at the end, dB.
    pub worst_line_db: f64,
    /// The largest loop-to-loop change of any control's in-band rms error over
    /// the last [`SETTLE_LOOPS`] loops at full level, dB (NaN with fewer).
    pub settle_db: f64,
    /// The protection or law fault that ended the run, in words, if one did.
    pub trip: Option<String>,
    /// Which kind of protection that was.
    pub trip_class: TripClass,
    /// Every law call whose output was not used, in words, in order.
    pub misses: Vec<String>,
    /// Of those, the calls whose drive broke an output rule (the core's, or the
    /// law host's at a law file's boundary).
    pub rejected: u64,
    /// Drive-lines the core repaired (small negative eigenvalues clamped).
    pub repaired_lines: u64,
    /// Drive-lines the per-loop change cap held back.
    pub capped_lines: u64,
    /// Loop calls made (after the seed).
    pub calls: u64,
    /// The slowest law call measured, milliseconds.
    pub worst_call_ms: f64,
    /// The time budget of one call, milliseconds.
    pub budget_ms: f64,
    /// Whether the identification gate passed without an override.
    pub gate_passed: bool,
}

impl MimoCaseMeasure {
    fn settled(&self) -> bool {
        self.settle_db.is_finite()
            && self.settle_db <= SETTLE_DB
            && self.final_error_db <= SETTLE_MAX_DB
    }
}

/// The verdict on one case, with the reasons in words.
pub fn judge(case: MimoCheckCase, m: &MimoCaseMeasure) -> CheckCase {
    let req = case.requirement();
    let mut reasons: Vec<String> = Vec::new();
    if let Some(e) = &m.setup_error {
        reasons.push(format!("the run could not be made: {e}"));
    }
    if !m.misses.is_empty() {
        // Each kind of miss once, with the first one's detail: the kind is the
        // words before a parenthesis (a detail such as a time varies call to call).
        let key = |w: &str| w.split(" (").next().unwrap_or(w).to_string();
        let mut kinds: Vec<&str> = Vec::new();
        for w in &m.misses {
            if !kinds.iter().any(|k| key(k) == key(w)) {
                kinds.push(w);
            }
        }
        reasons.push(format!(
            "{} of {} calls not used ({} with a drive refused at a check): {}",
            m.misses.len(),
            m.calls,
            m.rejected,
            kinds.join("; ")
        ));
    }
    let excused = req == Requirement::Survive && m.trip_class == TripClass::CoreJudgement;
    if let Some(t) = &m.trip {
        if !excused {
            reasons.push(format!("the run ended early: {t}"));
        }
    }
    let ran = m.setup_error.is_none() && (m.trip.is_none() || excused);
    if ran && !m.reached_full_level && !excused {
        reasons.push("the run did not reach full level".into());
    }
    if ran && m.reached_full_level && m.loops_to_converge.is_none() && !excused {
        let not_converged = format!(
            "not every control within +/- {CONVERGE_DB} dB in {} loops at full level (worst \
             control {:.2} dB)",
            m.loops_at_full, m.final_error_db
        );
        match req {
            Requirement::Converge => reasons.push(not_converged),
            Requirement::Settle | Requirement::Survive if !m.settled() => reasons.push(format!(
                "{not_converged}, and not settled (largest change over the last {SETTLE_LOOPS} \
                 loops {:.2} dB against {SETTLE_DB} dB, worst control against {SETTLE_MAX_DB} dB)",
                m.settle_db
            )),
            _ => {}
        }
    }
    if m.worst_call_ms.is_finite() && m.budget_ms > 0.0 && m.worst_call_ms > m.budget_ms {
        reasons.push(format!(
            "a call took {:.0} ms against a budget of {:.0} ms",
            m.worst_call_ms, m.budget_ms
        ));
    }
    let passed = reasons.is_empty();
    let detail = if passed {
        let outcome = match (m.loops_to_converge, excused) {
            (_, true) => format!(
                "every call answered; the core ended the run by its own judgement ({})",
                m.trip.as_deref().unwrap_or("")
            ),
            (Some(n), false) => format!(
                "within +/- {CONVERGE_DB} dB {n} loop{} after full level (worst control {:.2} dB, \
                 worst line {:.2} dB)",
                if n == 1 { "" } else { "s" },
                m.final_error_db,
                m.worst_line_db
            ),
            (None, false) => format!(
                "settled after {} loops at full level (worst control {:.2} dB, last changes \
                 under {:.2} dB)",
                m.loops_at_full, m.final_error_db, m.settle_db
            ),
        };
        let gate = if m.gate_passed {
            ""
        } else {
            "; identification gate overridden for the check"
        };
        format!(
            "{outcome}; {} calls, slowest {:.1} ms of {:.0} ms; {} drive-lines repaired, {} \
             capped{gate}",
            m.calls, m.worst_call_ms, m.budget_ms, m.repaired_lines, m.capped_lines,
        )
    } else {
        reasons.join("; ")
    };
    CheckCase {
        name: case.name().to_string(),
        passed,
        detail,
    }
}

/// The record of a multiple-output check of the law file `law_sha256`, its
/// verdict derived from `cases`.
pub fn mimo_check_record(
    law_sha256: impl Into<String>,
    date: impl Into<String>,
    cases: Vec<CheckCase>,
) -> CheckRecord {
    CheckRecord::new(law_sha256, MIMO_CHECK_VERSION, date, cases)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> MimoCaseMeasure {
        MimoCaseMeasure {
            setup_error: None,
            reached_full_level: true,
            loops_to_converge: Some(3),
            loops_at_full: 3,
            final_error_db: 0.4,
            worst_line_db: 1.6,
            settle_db: 0.1,
            trip: None,
            trip_class: TripClass::None,
            misses: Vec::new(),
            rejected: 0,
            repaired_lines: 0,
            capped_lines: 12,
            calls: 9,
            worst_call_ms: 4.0,
            budget_ms: 1500.0,
            gate_passed: true,
        }
    }

    fn unconverged(final_db: f64, settle_db: f64) -> MimoCaseMeasure {
        MimoCaseMeasure {
            loops_to_converge: None,
            loops_at_full: MAX_LOOPS_AT_FULL,
            final_error_db: final_db,
            settle_db,
            ..good()
        }
    }

    #[test]
    fn a_converged_run_with_no_misses_and_no_trip_passes() {
        let c = judge(MimoCheckCase::Modal4x4, &good());
        assert!(c.passed, "{}", c.detail);
        assert_eq!(c.name, "modal 4 x 4");
        assert!(
            c.detail
                .contains("within +/- 1 dB 3 loops after full level"),
            "{}",
            c.detail
        );
    }

    #[test]
    fn an_overridden_gate_is_said_but_does_not_fail_the_case() {
        let mut m = good();
        m.gate_passed = false;
        let c = judge(MimoCheckCase::Modal12x6Ill, &m);
        assert!(c.passed);
        assert!(c.detail.contains("gate overridden"), "{}", c.detail);
    }

    #[test]
    fn a_square_plant_needs_convergence_and_a_settled_error_is_not_enough() {
        let m = unconverged(2.75, 0.05);
        let c = judge(MimoCheckCase::Modal4x4, &m);
        assert!(!c.passed);
        assert!(
            c.detail.contains("in 20 loops at full level"),
            "{}",
            c.detail
        );
        assert!(c.detail.contains("2.75 dB"), "{}", c.detail);
        assert!(!judge(MimoCheckCase::Noisy4x4, &m).passed);
    }

    #[test]
    fn more_controls_than_drives_passes_a_steady_drive_near_the_reference() {
        let c = judge(MimoCheckCase::Modal12x6, &unconverged(2.3, 0.2));
        assert!(c.passed, "{}", c.detail);
        assert!(
            c.detail.starts_with("settled after 20 loops"),
            "{}",
            c.detail
        );
    }

    #[test]
    fn more_controls_than_drives_fails_an_error_still_moving() {
        let c = judge(MimoCheckCase::Ladder12x6, &unconverged(2.3, 1.4));
        assert!(!c.passed);
        assert!(c.detail.contains("not settled"), "{}", c.detail);
        assert!(c.detail.contains("1.40 dB against 0.5 dB"), "{}", c.detail);
    }

    #[test]
    fn more_controls_than_drives_fails_a_steady_drive_far_from_the_reference() {
        let c = judge(MimoCheckCase::RigidPlate12x6, &unconverged(7.5, 0.0));
        assert!(!c.passed);
        assert!(c.detail.contains("not settled"), "{}", c.detail);
    }

    #[test]
    fn a_settle_measure_that_was_not_taken_does_not_pass() {
        let c = judge(MimoCheckCase::Modal12x6, &unconverged(0.2, f64::NAN));
        assert!(!c.passed, "{}", c.detail);
    }

    #[test]
    fn a_missed_call_fails_the_case_and_names_each_reason_once() {
        let mut m = good();
        m.misses = vec![
            "it reported an error (the law's output was refused: line 2 is NaN)".into(),
            "it reported an error (the law's output was refused: line 2 is NaN)".into(),
            "it took longer than its time budget (2100 ms against 1500 ms)".into(),
            "it took longer than its time budget (2200 ms against 1500 ms)".into(),
        ];
        m.rejected = 2;
        let c = judge(MimoCheckCase::Modal4x4, &m);
        assert!(!c.passed);
        assert!(
            c.detail
                .contains("4 of 9 calls not used (2 with a drive refused at a check)"),
            "{}",
            c.detail
        );
        assert_eq!(c.detail.matches("is NaN").count(), 1);
        assert_eq!(c.detail.matches("time budget").count(), 1);
    }

    #[test]
    fn a_trip_fails_the_case_with_its_words() {
        let mut m = good();
        m.trip = Some("control 2 left its abort lines on 14 lines".into());
        m.trip_class = TripClass::Other;
        m.reached_full_level = false;
        m.loops_to_converge = None;
        let c = judge(MimoCheckCase::Ladder12x6, &m);
        assert!(!c.passed);
        assert!(
            c.detail.starts_with("the run ended early: control 2"),
            "{}",
            c.detail
        );
        assert!(
            !c.detail.contains("did not reach full level"),
            "{}",
            c.detail
        );
    }

    #[test]
    fn the_cores_own_judgement_is_excused_only_on_the_ill_conditioned_plant() {
        let mut m = good();
        m.trip = Some("the change cap held drive 4 back".into());
        m.trip_class = TripClass::CoreJudgement;
        m.loops_to_converge = None;
        m.settle_db = f64::NAN;
        let c = judge(MimoCheckCase::Modal12x6Ill, &m);
        assert!(c.passed, "{}", c.detail);
        assert!(
            c.detail.contains("by its own judgement (the change cap"),
            "{}",
            c.detail
        );
        assert!(!judge(MimoCheckCase::Modal12x6, &m).passed);
        // An envelope abort is not excused, even there.
        m.trip_class = TripClass::Other;
        assert!(!judge(MimoCheckCase::Modal12x6Ill, &m).passed);
        // Nor a miss.
        m.trip_class = TripClass::CoreJudgement;
        m.misses = vec!["it failed while computing".into()];
        assert!(!judge(MimoCheckCase::Modal12x6Ill, &m).passed);
    }

    #[test]
    fn a_run_that_never_reached_full_level_fails() {
        let mut m = good();
        m.reached_full_level = false;
        m.loops_to_converge = None;
        let c = judge(MimoCheckCase::Ladder12x6, &m);
        assert!(!c.passed);
        assert!(
            c.detail.contains("did not reach full level"),
            "{}",
            c.detail
        );
    }

    #[test]
    fn a_setup_the_law_refuses_fails_the_case() {
        let m = MimoCaseMeasure {
            setup_error: Some("the law does not support more controls than drives".into()),
            ..MimoCaseMeasure::default()
        };
        let c = judge(MimoCheckCase::Modal12x6, &m);
        assert!(!c.passed);
        assert_eq!(
            c.detail,
            "the run could not be made: the law does not support more controls than drives"
        );
    }

    #[test]
    fn a_call_over_its_budget_fails_the_case() {
        let mut m = good();
        m.worst_call_ms = 2100.0;
        let c = judge(MimoCheckCase::RigidPlate12x6, &m);
        assert!(!c.passed);
        assert!(
            c.detail.contains("2100 ms against a budget of 1500 ms"),
            "{}",
            c.detail
        );
    }

    #[test]
    fn a_record_passes_only_when_every_case_passed() {
        let sha = "a".repeat(64);
        let all: Vec<CheckCase> = MimoCheckCase::ALL
            .into_iter()
            .map(|c| judge(c, &good()))
            .collect();
        let r = mimo_check_record(sha.clone(), "2026-11-03T12:00:00Z", all.clone());
        assert!(r.passed);
        assert_eq!(r.tool_version, MIMO_CHECK_VERSION);
        assert_eq!(r.cases.len(), 6);
        let mut one_bad = all;
        let mut m = good();
        m.trip = Some("a law fault".into());
        m.trip_class = TripClass::Other;
        one_bad[4] = judge(MimoCheckCase::Noisy4x4, &m);
        let r = mimo_check_record(sha, "2026-11-03T12:00:00Z", one_bad);
        assert!(!r.passed);
        r.validate().unwrap();
    }

    #[test]
    fn case_names_are_unique_and_round_trip() {
        for c in MimoCheckCase::ALL {
            assert_eq!(MimoCheckCase::from_name(c.name()), Some(c));
            assert_eq!(*c.levels_db().last().unwrap(), 0.0);
        }
    }
}
