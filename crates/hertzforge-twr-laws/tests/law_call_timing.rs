//! Timing list: how long one correction of the built-in takes through the
//! trait the TWR core calls it by, natively and as its law file (the host's
//! sandbox, the canonical ABI's copies of three long lists), at a 2^16-sample
//! record (5.12 s at 12 800 Hz, the bench's) and at the 2^20-sample record cap
//! (81.92 s). Median and p99.9 over 1000 corrections at 2^16; at 2^20, where
//! one call moves about 24 MiB, median and worst of 30. Each held under a
//! quarter of the budget for its record (`law_budget` 0.25 of the record:
//! 1.28 s and 20.48 s).

mod common;

use common::law_file;
use hertzforge_law_host::{HostConfig, LawHost};
use hertzforge_twr::{
    PlayInputs, PlayKind, TwrControl, TwrCorrectFactory, TwrLawFactory, TwrLawSetup, TwrPlant,
    Verdict,
};
use hertzforge_twr_laws::WasmTwrLawFactory;
use std::sync::Arc;
use std::time::{Duration, Instant};

const FS: f64 = 12_800.0;

/// `calls` corrections of `factory`'s law at `n` samples: (median, the call at
/// the 99.9th percentile).
fn time_calls(factory: Arc<dyn TwrLawFactory>, n: usize, calls: usize) -> (Duration, Duration) {
    let bins = n / 2 + 1;
    let pad = 512;
    let reference: Vec<f64> = (0..n)
        .map(|i| {
            if (pad..n - 2 * pad).contains(&i) {
                let t = i as f64 / FS;
                (std::f64::consts::TAU * 250.0 * t).sin() * 0.2
                    + (std::f64::consts::TAU * 620.0 * t).sin() * 0.05
            } else {
                0.0
            }
        })
        .collect();
    let frf = vec![(1.0, 0.0); bins];
    let coherence = vec![1.0; bins];
    let trusted: Vec<bool> = (0..bins)
        .map(|k| (20.0..=2000.0).contains(&(k as f64 * FS / n as f64)))
        .collect();
    let budget_s = TwrControl::default().call_budget_s(n as f64 / FS);
    let setup = TwrLawSetup {
        fs_hz: FS,
        record_len: n,
        band_lo_hz: 20.0,
        band_hi_hz: 2000.0,
        reference: &reference,
        section_start: pad,
        section_len: n - 3 * pad,
        plant: TwrPlant {
            frf: &frf,
            coherence: &coherence,
            inverse: &frf,
            trusted: &trusted,
            epsilon: 1e-4,
        },
        drive_ceiling_v: 1.0,
        call_budget_s: budget_s,
    };
    let mut law = factory.build(&setup).unwrap();
    let seed = PlayInputs {
        play_index: 0,
        kind: PlayKind::Iteration,
        response: &[],
        drive: &[],
        judged: None,
        limited: false,
    };
    let drive = law.seed(&seed).unwrap().drive;
    let response: Vec<f64> = reference.iter().map(|v| 0.9 * v).collect();
    let mut times = Vec::with_capacity(calls);
    for i in 0..calls as u64 {
        let inputs = PlayInputs {
            play_index: i + 1,
            kind: PlayKind::Iteration,
            response: &response,
            drive: &drive,
            judged: Some(Verdict {
                gated: false,
                rms_error_pct: 10.0,
                peak_error_pct: 12.0,
                spectral_error_db: 0.9,
            }),
            limited: false,
        };
        let t0 = Instant::now();
        law.iterate(&inputs).unwrap();
        times.push(t0.elapsed());
    }
    times.sort();
    (times[times.len() / 2], times[(times.len() * 999) / 1000])
}

fn file_factory() -> Arc<dyn TwrLawFactory> {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host
        .load_twr(&std::fs::read(law_file("hertzforge_law_twr_correct")).unwrap())
        .unwrap();
    let params = law.resolve_params(&[]).unwrap();
    Arc::new(WasmTwrLawFactory::new(law, params))
}

#[test]
fn a_twr_law_call_at_the_bench_record_and_the_record_cap_is_well_inside_its_budget() {
    for (n, calls) in [(1usize << 16, 1000usize), (1 << 20, 30)] {
        let quarter =
            Duration::from_secs_f64(TwrControl::default().call_budget_s(n as f64 / FS) / 4.0);
        for (door, factory) in [
            (
                "native",
                Arc::new(TwrCorrectFactory::default()) as Arc<dyn TwrLawFactory>,
            ),
            ("law file", file_factory()),
        ] {
            let (median, p999) = time_calls(factory, n, calls);
            println!(
                "{door} call at {n} samples: median {median:?}, p99.9 {p999:?} ({calls} calls); \
                 a quarter of the budget is {quarter:?}"
            );
            assert!(
                p999 < quarter,
                "{door} at {n}: p99.9 {p999:?} over {quarter:?}"
            );
        }
    }
}
