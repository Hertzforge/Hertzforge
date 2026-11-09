//! The built-in SRS law natively and as its law file (`laws/shock-srs-ratio`)
//! are one law: on the same runs every gain they return agrees to relative
//! 1e-12, and every discrete outcome (pulses, levels, passes, the end, the
//! trip) is the same. At other parameter values the run moves, so the
//! agreement is not the parameters being ignored.

mod common;

use common::{law_file, srs_station_for, SRS_FS};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_shock_law_math::srs::SrsRatioParams;
use hertzforge_shock_laws::{srs_check_config, BuiltinSrsFactory, SrsCheckCase, WasmSrsLawFactory};
use hertzforge_srs::service::pacer::VirtualPacer;
use hertzforge_srs::service::report::RunState;
use hertzforge_srs::{FirePath, HalPulsePlant, SrsLawFactory, SrsShockService};
use std::sync::Arc;

struct Run {
    gains: Vec<Vec<f64>>,
    outcome: String,
}

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut d = Channel::new(1);
    d.role = ChannelRole::Drive;
    let mut c = Channel::new(2);
    c.role = ChannelRole::Control;
    t.channels.push(d);
    t.channels.push(c);
    t
}

fn run(case: SrsCheckCase, factory: Arc<dyn SrsLawFactory>) -> Run {
    let t = table();
    let plant = HalPulsePlant::new(
        srs_station_for(case),
        t.clone(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap();
    let mut s = SrsShockService::new(
        srs_check_config(case, SRS_FS),
        t,
        FirePath::Host(Box::new(plant)),
        None,
    )
    .unwrap()
    .with_law_factory(factory)
    .with_identification_scale_db(case.identification_scale_db().unwrap_or(0.0));
    s.set_pacer(Box::new(VirtualPacer::new(0.0)));
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s.run_sysid().unwrap();
    s.proceed_after_review().unwrap();
    let mut gains = Vec::new();
    let fire = |s: &mut SrsShockService, n: usize| {
        for _ in 0..n {
            if !matches!(s.state(), RunState::Armed | RunState::Firing) || !s.fire_pulse().unwrap()
            {
                break;
            }
        }
    };
    if case == SrsCheckCase::Ladder {
        fire(&mut s, 3);
        s.jump_to_level(1).unwrap();
        fire(&mut s, 3);
        for _ in 0..6 {
            s.step_up().unwrap();
        }
        fire(&mut s, 3);
    } else {
        fire(&mut s, 100);
    }
    for r in s.drain_law_records() {
        assert!(r.miss.is_none(), "a call missed: {:?}", r.miss);
        gains.push(r.payload.gains);
    }
    let rows = s.report_data().pulses;
    let outcome = format!(
        "{:?} {:?} {:?}",
        s.state(),
        s.trip().map(|t| &t.kind),
        rows.iter()
            .map(|r| (
                r.seq,
                r.level_db,
                r.passed,
                r.alarm_bands,
                r.limited_components
            ))
            .collect::<Vec<_>>()
    );
    Run { gains, outcome }
}

fn file_factory(params: Vec<Param>) -> Arc<dyn SrsLawFactory> {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let bytes = std::fs::read(law_file("hertzforge_law_shock_srs_ratio")).unwrap();
    let law = host.load_srs(&bytes).unwrap();
    let params = law.resolve_params(&params).unwrap();
    Arc::new(WasmSrsLawFactory::new(law, params))
}

/// The largest difference between two runs' gains, relative to the larger.
fn worst_relative(a: &Run, b: &Run) -> f64 {
    assert_eq!(a.gains.len(), b.gains.len(), "the same number of calls");
    a.gains
        .iter()
        .zip(&b.gains)
        .flat_map(|(x, y)| {
            assert_eq!(x.len(), y.len());
            x.iter()
                .zip(y)
                .map(|(p, q)| (p - q).abs() / p.abs().max(q.abs()))
        })
        .fold(0.0f64, f64::max)
}

#[test]
fn the_native_law_and_its_law_file_are_one_law() {
    let cases = [
        SrsCheckCase::OneMode,
        SrsCheckCase::Ladder,
        SrsCheckCase::Limit,
    ];
    for max_db in [3.0, 1.5] {
        for case in cases {
            let native = run(
                case,
                Arc::new(BuiltinSrsFactory::new(SrsRatioParams {
                    max_db_per_pulse: max_db,
                })),
            );
            let file = run(
                case,
                file_factory(vec![Param::new(
                    "max_db_per_pulse",
                    ParamValue::Number(max_db),
                )]),
            );
            let worst = worst_relative(&native, &file);
            println!(
                "{} at {max_db} dB: {} calls, worst {worst:e}",
                case.name(),
                native.gains.len()
            );
            assert!(worst <= 1e-12, "{}: {worst}", case.name());
            assert_eq!(native.outcome, file.outcome, "{}", case.name());
        }
    }
    // The second parameter set moves the run.
    let a = run(
        SrsCheckCase::OneMode,
        Arc::new(BuiltinSrsFactory::default()),
    );
    let b = run(
        SrsCheckCase::OneMode,
        Arc::new(BuiltinSrsFactory::new(SrsRatioParams {
            max_db_per_pulse: 1.5,
        })),
    );
    let moved = a
        .gains
        .iter()
        .zip(&b.gains)
        .flat_map(|(x, y)| x.iter().zip(y).map(|(p, q)| (p - q).abs()))
        .fold(0.0f64, f64::max);
    assert!(moved > 1e-3, "1.5 dB moves the run: {moved}");
}

#[test]
fn the_built_in_manifest_is_the_law_files() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let bytes = std::fs::read(law_file("hertzforge_law_shock_srs_ratio")).unwrap();
    let law = host.load_srs(&bytes).unwrap();
    assert_eq!(
        law.manifest(),
        &hertzforge_random_laws::srs_builtin_manifest()
    );
}
