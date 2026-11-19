//! One implementation through two doors: the native built-in (`twr_correct`
//! in the core) and its law file (`laws/twr-correct`, the same math compiled
//! to WebAssembly) on identical seeded stations, every drive sample within
//! 1e-9 of its record's peak and every discrete outcome exact: on the flat,
//! one-mode, deep-queue and identification-error stations through the AUTO
//! ladder, and a MANUAL run with level moves; at the defaults and at `beta`
//! 0.25 with a 3 dB backoff, the second set moving the run by more than 1e-3
//! of a peak somewhere (the parameters reach the law through both doors).
//!
//! The station plays `f32` samples, so two drives a rounding apart in `f64`
//! can split on an `f32` rounding boundary; after the first play whose
//! responses differ the drives are held to 1e-6 of their peak, the discrete
//! outcomes still exact, and the split is printed (as in the core's own
//! parity test).

mod common;

use common::{law_file, station_for};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_shock_law_math::twr::TwrCorrectParams;
use hertzforge_twr::schedule::TwrRunMode;
use hertzforge_twr::{BackendReferencePlayer, TwrLawFactory, TwrService, TwrState};
use hertzforge_twr_laws::{twr_check_config, BuiltinTwrFactory, TwrCheckCase, WasmTwrLawFactory};
use std::sync::Arc;

type Play = (String, Vec<f64>, Vec<f64>);

/// Run `case` (MANUAL with level moves when `manual`) with `factory`; every
/// play's discrete facts, its drive and its response, and the end.
fn run(case: TwrCheckCase, manual: bool, factory: Arc<dyn TwrLawFactory>) -> (Vec<Play>, String) {
    let mut table = ChannelTable::new();
    let mut c = Channel::new(1);
    c.role = ChannelRole::Control;
    table.channels.push(c);
    let mut d = Channel::new(2);
    d.role = ChannelRole::Drive;
    table.channels.push(d);
    let player = BackendReferencePlayer::new(
        station_for(case),
        table,
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap();
    let mut cfg = twr_check_config();
    if manual {
        cfg.schedule.mode = TwrRunMode::Manual;
    }
    let mut svc = TwrService::new(player, cfg)
        .unwrap()
        .with_law_factory(factory);
    if let Some(db) = case.identification_scale_db() {
        svc = svc.with_identification_scale_db(db);
    }
    let mut plays = Vec::new();
    let mut step = |svc: &mut TwrService<_>, what: &str| {
        let r = match what {
            "advance" => svc.advance().map(|_| ()),
            "up" => svc.step_level_up(),
            "jump" => svc.jump_to_level(4),
            "play" => svc.begin_playback(),
            _ => unreachable!(),
        };
        if svc.state() == TwrState::Review {
            svc.proceed().unwrap();
        }
        let have = plays.len();
        for row in &svc.plays()[have..] {
            plays.push((
                format!(
                    "{} {} {:?} {:?} {} {} {}",
                    row.index,
                    row.kind.as_str(),
                    row.level_db,
                    row.iteration,
                    row.gated,
                    row.clipped_samples,
                    row.law.as_ref().is_some_and(|c| c.used)
                ),
                svc.drive_time().to_vec(),
                svc.achieved_time().to_vec(),
            ));
        }
        r.is_ok()
    };
    if manual {
        for what in [
            "advance", "advance", "advance", "up", "advance", "jump", "advance", "advance",
            "advance", "play", "advance",
        ] {
            step(&mut svc, what);
        }
    } else {
        for _ in 0..200 {
            if matches!(
                svc.state(),
                TwrState::Complete
                    | TwrState::Aborted
                    | TwrState::Holding
                    | TwrState::CoherenceHold
            ) {
                break;
            }
            step(&mut svc, "advance");
        }
    }
    let st = svc.status();
    let end = format!(
        "{:?} {:?} {:?} {:?}",
        st.state,
        st.level_db,
        st.trip.map(|t| t.kind.as_str()),
        svc.plays().len()
    );
    (plays, end)
}

fn law_factory(params: TwrCorrectParams) -> Arc<dyn TwrLawFactory> {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host
        .load_twr(&std::fs::read(law_file("hertzforge_law_twr_correct")).unwrap())
        .unwrap();
    let supplied = [
        Param::new("beta", ParamValue::Number(params.beta)),
        Param::new(
            "max_db_per_iter",
            ParamValue::Number(params.max_db_per_iter),
        ),
        Param::new(
            "initial_backoff_db",
            ParamValue::Number(params.initial_backoff_db),
        ),
    ];
    let resolved = law.resolve_params(&supplied).unwrap();
    Arc::new(WasmTwrLawFactory::new(law, resolved))
}

fn peak(x: &[f64]) -> f64 {
    x.iter().fold(0.0f64, |m, v| m.max(v.abs()))
}

#[test]
fn the_law_file_is_the_built_in() {
    let tuned = TwrCorrectParams {
        beta: 0.25,
        max_db_per_iter: 6.0,
        initial_backoff_db: 3.0,
    };
    let mut report = Vec::new();
    let mut moved = 0.0f64;
    for (case, manual) in [
        (TwrCheckCase::Flat, false),
        (TwrCheckCase::OneMode, false),
        (TwrCheckCase::DeepQueue, false),
        (TwrCheckCase::IdentificationError, false),
        (TwrCheckCase::OneMode, true),
    ] {
        let mut by_params = Vec::new();
        for params in [TwrCorrectParams::default(), tuned] {
            let (native, ne) = run(case, manual, Arc::new(BuiltinTwrFactory::new(params)));
            let (file, fe) = run(case, manual, law_factory(params));
            let what = format!(
                "{} {} {params:?}",
                case.name(),
                if manual { "manual" } else { "auto" }
            );
            assert_eq!(ne, fe, "{what}: the end");
            assert_eq!(native.len(), file.len(), "{what}: the plays");
            let (mut worst, mut split, mut calls) = (0.0f64, None, 0);
            for (i, ((a, da, ya), (b, db, yb))) in native.iter().zip(&file).enumerate() {
                assert_eq!(a, b, "{what}: play {i}");
                if a.ends_with("true") {
                    calls += 1;
                }
                let pk = peak(da).max(f64::MIN_POSITIVE);
                let tol = if split.is_some() { 1e-6 } else { 1e-9 };
                for (k, (x, y)) in da.iter().zip(db).enumerate() {
                    let d = (x - y).abs() / pk;
                    assert!(
                        d <= tol,
                        "{what}: play {i} sample {k}: {d:e} of peak (split {split:?})"
                    );
                    worst = worst.max(d);
                }
                if split.is_none() && ya.iter().zip(yb).any(|(p, q)| p.to_bits() != q.to_bits()) {
                    split = Some(i);
                }
            }
            report.push(format!(
                "{what}: {} plays, {calls} with a call behind them, end {ne}, worst {worst:.2e} \
                 of peak, responses split at {split:?}",
                native.len()
            ));
            by_params.push(native);
        }
        // The parameters move the run.
        for ((_, da, _), (_, db, _)) in by_params[0].iter().zip(&by_params[1]) {
            let pk = peak(da).max(f64::MIN_POSITIVE);
            for (x, y) in da.iter().zip(db) {
                moved = moved.max((x - y).abs() / pk);
            }
        }
    }
    for line in &report {
        println!("{line}");
    }
    assert!(
        moved > 1e-3,
        "the second set of parameters moved nothing ({moved:e})"
    );
}

/// The catalog lists the built-in with the manifest its law file describes.
#[test]
fn the_built_in_manifest_is_the_law_files() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host
        .load_twr(&std::fs::read(law_file("hertzforge_law_twr_correct")).unwrap())
        .unwrap();
    assert_eq!(
        *law.manifest(),
        hertzforge_random_laws::twr_builtin_manifest()
    );
}
