//! Timing list: how long one call of the built-in SRS law's law file takes,
//! through the adapter the SRS core calls it by, at the default band count (a
//! 1/3-octave grid from 100 to 2000 Hz: 14 bands, one component each).
//! Median and p99.9 over 1000 iterations, printed, and held well inside the
//! default budget (250 ms at the 1.0 s floor).

mod common;

use common::law_file;
use hertzforge_law_host::{HostConfig, LawHost};
use hertzforge_shock_laws::WasmSrsLawFactory;
use hertzforge_srs::{SrsInputs, SrsLawFactory, SrsLawSetup};
use std::time::{Duration, Instant};

#[test]
fn an_srs_law_file_call_at_the_default_band_count_is_well_inside_its_budget() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host
        .load_srs(&std::fs::read(law_file("hertzforge_law_shock_srs_ratio")).unwrap())
        .unwrap();
    let bands = hertzforge_srs::SrsTestConfig::flat(100.0, 2000.0, 1.0).band_freqs();
    assert_eq!(bands.len(), 14);
    let target = vec![1.0; bands.len()];
    let gain = vec![1.0; bands.len()];
    let setup = SrsLawSetup {
        band_hz: &bands,
        component_hz: &bands,
        target: &target,
        q: 10.0,
        component_gain: Some(&gain),
        call_budget_s: 0.25,
    };
    let factory = WasmSrsLawFactory::new(law.clone(), law.resolve_params(&[]).unwrap());
    let mut l = factory.build(&setup).unwrap();
    let zeros = vec![0.0; bands.len()];
    l.seed(&SrsInputs {
        pulse_index: 0,
        target: &target,
        measured: &zeros,
        commanded_scale: 1.0,
    })
    .unwrap();
    let measured: Vec<f64> = target.iter().map(|t| 0.9 * t).collect();
    let mut times = Vec::with_capacity(1000);
    for i in 0..1000u64 {
        let inputs = SrsInputs {
            pulse_index: i + 1,
            target: &target,
            measured: &measured,
            commanded_scale: 1.0,
        };
        let t0 = Instant::now();
        l.iterate(&inputs).unwrap();
        times.push(t0.elapsed());
    }
    times.sort();
    let (median, p999) = (times[500], times[(times.len() * 999) / 1000]);
    println!("SRS law-file call at 14 bands: median {median:?}, p99.9 {p999:?}");
    assert!(
        p999 < Duration::from_millis(62),
        "p99.9 {p999:?} over a quarter of the budget"
    );
}
