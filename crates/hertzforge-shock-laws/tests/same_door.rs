//! The built-in classical law and its law file are one implementation: the
//! native `fd_correct` and `laws/shock-fdc` (the law file, run through the law
//! host) give the same run, through the same door.
//!
//! Seeded simulator runs on the one-mode, station, delay and identification-
//! error plants, and a MANUAL run with level moves, at the default parameters
//! and at beta 0.25 with a 3 dB backoff. Every drive sample is within a
//! relative 1e-9 of the record's peak, and the discrete outcome (pulses,
//! verdicts, levels, trips, misses, capped flags) is the same. The second
//! parameter set must move the run by more than 1e-3 somewhere, or the test
//! would not see a law file that ignored its parameters.

mod common;

use common::{law_file, station_for, FS};
use hertzforge_channel_table::ChannelTable;
use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_shock_common::ShockLevelSchedule;
use hertzforge_shock_host::service::pacer::VirtualPacer;
use hertzforge_shock_host::{
    BackendPulseFirer, ClassicalLawFactory, ClassicalShockService, FdCorrectFactory, RunState,
};
use hertzforge_shock_law_math::classical::FdCorrectParams;
use hertzforge_shock_laws::{classical_check_config, ClassicalCheckCase, WasmClassicalLawFactory};
use std::sync::Arc;

struct Run {
    drives: Vec<Vec<f64>>,
    outcome: String,
}

fn run(case: ClassicalCheckCase, factory: Arc<dyn ClassicalLawFactory>, manual: bool) -> Run {
    let mut cfg = classical_check_config(case, FS);
    if manual {
        cfg.schedule = ShockLevelSchedule::manual_default();
    }
    let table = ChannelTable::default_with_channels(1);
    let firer = BackendPulseFirer::new(
        station_for(case),
        table.clone(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap();
    let mut s = ClassicalShockService::new(
        cfg,
        table,
        firer,
        Some(std::env::temp_dir().join("hertzforge-shock-same-door")),
    )
    .unwrap()
    .with_law_factory(factory)
    .with_identification_scale_db(case.identification_scale_db().unwrap_or(0.0));
    s.set_pacer(Box::new(VirtualPacer::new(0.0)));
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    assert_eq!(s.identify().unwrap(), RunState::Firing);
    if manual {
        for step in [
            "fire", "up", "up", "fire", "fire", "jump4", "fire", "down", "fire",
        ] {
            match step {
                "fire" => {
                    s.fire_next_pulse().unwrap();
                }
                "up" => {
                    s.step_up().unwrap();
                }
                "down" => {
                    s.step_down().unwrap();
                }
                _ => {
                    s.jump_to_level(4).unwrap();
                }
            }
        }
        s.stop().unwrap();
    } else {
        s.run_auto(100).unwrap();
    }
    let report = s.report_data();
    let drives = (0..report.pulses.len() as u64)
        .map(|p| s.pulse_view(p).unwrap().drive)
        .collect();
    let mut outcome = format!("{:?} {:?}\n", s.state(), s.trip().map(|t| &t.kind));
    for r in &report.pulses {
        let law = r
            .law
            .as_ref()
            .map(|l| (l.used, l.miss.clone(), l.capped_bins, l.peak_capped));
        outcome += &format!(
            "{} {} {:?} {:?} {:?} {:?} {law:?}\n",
            r.pulse,
            r.scheduled_level_db,
            r.tolerance_overall,
            r.envelope_pass,
            r.velocity_change_pass,
            r.substitution_pass
        );
    }
    let law = report.law.unwrap();
    outcome += &format!(
        "calls {} misses {} refused {} capped {} {}",
        law.calls, law.misses, law.refused, law.capped_bins, law.capped_peaks
    );
    Run { drives, outcome }
}

fn worst_relative(a: &[Vec<f64>], b: &[Vec<f64>]) -> f64 {
    let mut worst = 0.0f64;
    for (x, y) in a.iter().zip(b) {
        let peak = x
            .iter()
            .fold(0.0f64, |m, v| m.max(v.abs()))
            .max(f64::MIN_POSITIVE);
        for (p, q) in x.iter().zip(y) {
            worst = worst.max((p - q).abs() / peak);
        }
    }
    worst
}

#[test]
fn the_native_law_and_its_law_file_give_the_same_run() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let bytes = std::fs::read(law_file("hertzforge_law_shock_fdc")).unwrap();
    let file = host.load_classical(&bytes).unwrap();
    let sets: [(&str, FdCorrectParams, Vec<Param>); 2] = [
        ("defaults", FdCorrectParams::default(), Vec::new()),
        (
            "beta 0.25, 3 dB backoff",
            FdCorrectParams {
                beta: 0.25,
                initial_backoff_db: 3.0,
                ..FdCorrectParams::default()
            },
            vec![
                Param::new("beta", ParamValue::Number(0.25)),
                Param::new("initial_backoff_db", ParamValue::Number(3.0)),
            ],
        ),
    ];
    let runs = [
        (ClassicalCheckCase::OneMode, false),
        (ClassicalCheckCase::StationPlant, false),
        (ClassicalCheckCase::Delay, false),
        (ClassicalCheckCase::IdentificationError, false),
        (ClassicalCheckCase::OneMode, true),
    ];
    let mut by_set: Vec<Vec<Run>> = Vec::new();
    for (name, native, supplied) in &sets {
        let params = file.resolve_params(supplied).unwrap();
        let mut these = Vec::new();
        for &(case, manual) in &runs {
            let what = format!(
                "{} {} at {name}",
                case.name(),
                if manual { "manual" } else { "auto" }
            );
            let a = run(case, Arc::new(FdCorrectFactory::new(*native)), manual);
            let b = run(
                case,
                Arc::new(WasmClassicalLawFactory::new(file.clone(), params.clone())),
                manual,
            );
            assert_eq!(a.outcome, b.outcome, "{what}");
            assert_eq!(a.drives.len(), b.drives.len(), "{what}");
            let w = worst_relative(&a.drives, &b.drives);
            assert!(w <= 1e-9, "{what}: drives differ by {w:e} of their peak");
            eprintln!(
                "{what}: {} pulses, worst drive difference {w:e} of the peak",
                a.drives.len()
            );
            these.push(a);
        }
        by_set.push(these);
    }
    // The second set moved the run.
    let moved = by_set[0]
        .iter()
        .zip(&by_set[1])
        .map(|(a, b)| worst_relative(&a.drives, &b.drives))
        .fold(0.0f64, f64::max);
    assert!(
        moved > 1e-3,
        "the parameters did not move the run ({moved:e})"
    );
}

#[test]
fn the_built_in_manifest_is_the_law_files() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let bytes = std::fs::read(law_file("hertzforge_law_shock_fdc")).unwrap();
    let file = host.load_classical(&bytes).unwrap();
    assert_eq!(
        file.manifest(),
        &hertzforge_random_laws::classical_builtin_manifest()
    );
}
