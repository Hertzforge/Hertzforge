//! The comparison study of the five built-in MIMO Sine laws: every law on
//! every standard sine structure in every test type its manifest accepts,
//! each a whole run on the engine on the simulator with fixed seeds, printed
//! as one table per test type.
//!
//! ```text
//! cargo run -p hertzforge-mimo-sine-laws --features simulator --example mimo_sine_law_study
//! ```
//!
//! Each run is the check's run (full level 1 g peak at every control on a
//! flat profile, alarm 1.5 dB and abort 6 dB, ceilings 2 V, identification at
//! -20 dB, noise 0.01 g rms, the guards at their defaults; amplitude only
//! where the law accepts it, every control in phase otherwise). The test by
//! structure:
//!
//! | Structure | Dwell | Stepped | Sweep |
//! |---|---|---|---|
//! | the four-by-four plates, the weakly coupled one, the loopback, the tall and the wide | 96 Hz, 6 s | 60 to 120 Hz, 6 an octave, 2 s each | 80 to 120 Hz, 1 octave a minute |
//! | `sine-hardening-4x4` | 165 Hz | 120 to 240 Hz | 140 to 190 Hz, 1 octave a minute |
//! | `sine-base-article-6x4`, article limited at 2 g | 230 Hz | 150 to 350 Hz | 150 to 350 Hz, 2 octaves a minute |
//! | `sine-near-singular-4x4` | 381.2 Hz | 350 to 420 Hz, 12 an octave | 350 to 420 Hz, 1 octave a minute |
//!
//! Columns: how the run ended (and where, when early); the worst and median
//! error over every judged update in dB (on the tall structure against the
//! response the drives can reach), then each control's worst and median in
//! dB and, where the reference states phase, in degrees; the largest drive
//! against its ceiling in dB; the largest opposition ratio in dB; the
//! deepest notch in dB.

use hertzforge_mimo_sine::laws::builtin;
use hertzforge_mimo_sine::records::Outcome;
use hertzforge_mimo_sine::{StepPlan, TestPlan, TestType};
use hertzforge_mimo_sine_laws::check::{
    check_structure, run_plan, sim_station, RunPlan, StationSpec, StructureEdit, CHECK_ABORT_DB,
    CHECK_FS_HZ, CHECK_LIMIT_BOUND_G, CHECK_NOISE_G,
};
use hertzforge_sine::scheduler::{
    BidirSweepConfig, DwellFor, DwellPlan, StepSineConfig, StepSpacing, SweepDirection, SweepTarget,
};
use hertzforge_sine::SweepRate;

const LAWS: [&str; 5] = ["matrix", "paired", "ratio", "steepest_descent", "secant"];

const STRUCTURES: [&str; 10] = [
    "sine-linear-4x4",
    "sine-softening-4x4-mild",
    "sine-softening-4x4-moderate",
    "sine-hardening-4x4",
    "sine-tall-6x3",
    "sine-wide-3x6",
    "sine-base-article-6x4",
    "sine-near-singular-4x4",
    "sine-weak-4x4",
    "sine-loopback-4x4",
];

/// The dwell, the step grid `(lo, hi, per octave)` and the sweep
/// `(lo, hi, octaves a minute)` on a structure.
fn tests(name: &str) -> (f64, (f64, f64, f64), (f64, f64, f64)) {
    match name {
        "sine-hardening-4x4" => (165.0, (120.0, 240.0, 6.0), (140.0, 190.0, 1.0)),
        "sine-base-article-6x4" => (230.0, (150.0, 350.0, 6.0), (150.0, 350.0, 2.0)),
        "sine-near-singular-4x4" => (381.2, (350.0, 420.0, 12.0), (350.0, 420.0, 1.0)),
        _ => (96.0, (60.0, 120.0, 6.0), (80.0, 120.0, 1.0)),
    }
}

fn plan(si: usize, name: &'static str, tt: TestType) -> RunPlan {
    let structure = check_structure(name).expect("a standard structure");
    let (dwell, (slo, shi, spo), (wlo, whi, rate)) = tests(name);
    let (test, max_s) = match tt {
        TestType::Dwell => (
            TestPlan::Dwell(DwellPlan {
                freq_hz: dwell,
                for_: DwellFor::Seconds(6.0),
            }),
            16.0,
        ),
        TestType::Stepped => (
            TestPlan::Stepped(StepPlan::Grid(StepSineConfig {
                band: (slo, shi),
                steps_per_octave: spo,
                spacing: StepSpacing::Log,
                dwell: Some(DwellFor::Seconds(2.0)),
            })),
            60.0,
        ),
        TestType::Sweep => (
            TestPlan::Sweep(BidirSweepConfig {
                band: (wlo, whi),
                rate: SweepRate::LogOctPerMin(rate),
                start_hz: wlo,
                initial_direction: SweepDirection::Up,
                target: SweepTarget::Sweeps(1),
            }),
            60.0 * (whi / wlo).log2() / rate + 10.0,
        ),
    };
    let ti = TestType::ALL.iter().position(|t| *t == tt).unwrap_or(0);
    RunPlan {
        structure,
        station: StationSpec {
            structure: name,
            edit: StructureEdit::None,
            transport_delay_s: 0.0,
            noise_g: CHECK_NOISE_G,
            seed: 0x57d0_0000 + (si as u64) * 16 + ti as u64,
            fs_hz: CHECK_FS_HZ,
        },
        test,
        ladder_db: vec![0.0],
        limit_bound_g: (structure.outputs > structure.controls).then_some(CHECK_LIMIT_BOUND_G),
        abort_db: CHECK_ABORT_DB,
        max_s,
        reachable: structure.controls > structure.drives,
        settle_after_s: None,
        resonance: None,
        misbehave_from: None,
        reference_rise_hz: None,
    }
}

fn f(x: f64, p: usize) -> String {
    if x.is_finite() {
        format!("{x:.p$}")
    } else {
        "-".into()
    }
}

fn main() {
    let t0 = std::time::Instant::now();
    for tt in TestType::ALL {
        println!("\n## {tt:?}\n");
        println!(
            "| Law | Structure | Ended | Worst dB | Median dB | Per control worst/median dB | \
             Per control worst/median deg | Drive / ceiling dB | Opposition dB | Notch dB |"
        );
        println!("|---|---|---|---|---|---|---|---|---|---|");
        for id in LAWS {
            let law = builtin(id).expect("a built-in law");
            if !law.manifest().test_types.contains(&tt) {
                continue;
            }
            for (si, name) in STRUCTURES.into_iter().enumerate() {
                let p = plan(si, name, tt);
                let row = sim_station(&p.station)
                    .map_err(|e| e.to_string())
                    .and_then(|b| run_plan(&p, law.clone(), b).map_err(|e| e.to_string()));
                match row {
                    Err(e) => println!("| {id} | {name} | not run: {e} | | | | | | | |"),
                    Ok(m) => {
                        let ended = match (m.outcome, m.stop_hz) {
                            (Some(o), Some(hz)) if o != Outcome::Completed => {
                                format!("{} at {hz:.1} Hz", o.as_str())
                            }
                            (Some(o), _) => o.as_str().to_string(),
                            (None, _) => "not ended".into(),
                        };
                        let pc_db = m
                            .per_control
                            .iter()
                            .map(|c| format!("{}/{}", f(c.worst_db, 2), f(c.median_db, 2)))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let pc_deg = m
                            .per_control
                            .iter()
                            .map(|c| format!("{}/{}", f(c.worst_deg, 1), f(c.median_deg, 1)))
                            .collect::<Vec<_>>()
                            .join(" ");
                        println!(
                            "| {id} | {name} | {ended} | {} | {} | {pc_db} | {pc_deg} | {} | {} | {} |",
                            f(m.worst_db, 2),
                            f(m.median_db, 2),
                            f(m.largest_drive_db, 1),
                            f(m.opposition_db, 2),
                            f(m.deepest_notch_db, 1),
                        );
                    }
                }
            }
        }
    }
    println!(
        "\nThe study took {:.0} s of wall time.",
        t0.elapsed().as_secs_f64()
    );
}
