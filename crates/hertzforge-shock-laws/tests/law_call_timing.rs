//! Timing list: how long one call of the built-in law's law file takes,
//! through the adapter the classical core calls it by, at the two grid lengths
//! a classical run has (8192 samples: an 11 ms half-sine at 51.2 kHz; 16384: a
//! longer pulse). Median and p99.9 over 1000 corrections each, printed, and
//! held well inside the default budget (250 ms at the 1.0 s floor).

mod common;

use common::law_file;
use hertzforge_law_host::{HostConfig, LawHost};
use hertzforge_shock_host::{
    ClassicalLawFactory, ClassicalLawSetup, ClassicalPlant, PulseInputs, Verdict,
};
use hertzforge_shock_laws::WasmClassicalLawFactory;
use std::time::{Duration, Instant};

fn time_calls(n: usize) -> (Duration, Duration) {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host
        .load_classical(&std::fs::read(law_file("hertzforge_law_shock_fdc")).unwrap())
        .unwrap();
    let bins = n / 2 + 1;
    let fs = 51_200.0;
    let main = (0.011 * fs) as usize;
    let start = n / 8;
    let reference: Vec<f64> = (0..n)
        .map(|i| {
            if (start..start + main).contains(&i) {
                (std::f64::consts::PI * (i - start) as f64 / main as f64).sin()
            } else {
                0.0
            }
        })
        .collect();
    let frf = vec![(1.0, 0.0); bins];
    let coherence = vec![1.0; bins];
    let trusted: Vec<bool> = (0..bins)
        .map(|k| (k as f64 * fs / n as f64) <= 5000.0 && k > 0)
        .collect();
    let setup = ClassicalLawSetup {
        fs_hz: fs,
        record_len: n,
        band_lo_hz: 5.0,
        band_hi_hz: 5000.0,
        reference: &reference,
        main_start: start,
        main_len: main,
        plant: ClassicalPlant {
            frf: &frf,
            coherence: &coherence,
            inverse: &frf,
            trusted: &trusted,
            epsilon: 1e-4,
        },
        drive_ceiling_v: 1.0,
        call_budget_s: 0.25,
    };
    let factory = WasmClassicalLawFactory::new(law.clone(), law.resolve_params(&[]).unwrap());
    let mut l = factory.build(&setup).unwrap();
    let seed = PulseInputs {
        pulse_index: 0,
        response: &[],
        drive: &[],
        judged: None,
        limited: false,
    };
    let drive = l.seed(&seed).unwrap().drive;
    let response: Vec<f64> = reference.iter().map(|v| 0.9 * v).collect();
    let mut times = Vec::with_capacity(1000);
    for i in 0..1000u64 {
        let inputs = PulseInputs {
            pulse_index: i + 1,
            response: &response,
            drive: &drive,
            judged: Some(Verdict {
                passed: false,
                error_pct: 10.0,
                peak_ratio: 0.9,
                velocity_change_ratio: 0.9,
            }),
            limited: false,
        };
        let t0 = Instant::now();
        l.iterate(&inputs).unwrap();
        times.push(t0.elapsed());
    }
    times.sort();
    (times[500], times[(times.len() * 999) / 1000])
}

#[test]
fn a_law_file_call_at_8192_and_16384_samples_is_well_inside_its_budget() {
    for n in [8192, 16384] {
        let (median, p999) = time_calls(n);
        println!("law-file call at {n} samples: median {median:?}, p99.9 {p999:?}");
        assert!(
            p999 < Duration::from_millis(62),
            "{n}: p99.9 {p999:?} over a quarter of the budget"
        );
    }
}
