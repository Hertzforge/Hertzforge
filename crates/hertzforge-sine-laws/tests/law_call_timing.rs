//! Timing list: how long one call of the built-in Sine law's law file takes,
//! through the adapter the Sine core calls it by, at the default 10 ms tick
//! (its budget a quarter of the tick, 2.5 ms). Median and p99.9 over 10000
//! ticks (100 s of run), printed, and p99.9 held within a quarter of the
//! budget. A machine under other load can stretch this; a failure here is
//! recorded, not loosened.

mod common;

use common::law_file;
use hertzforge_law_host::{HostConfig, LawHost};
use hertzforge_sine::law::{
    Hold, Identification, LawGuards, LawValues, SineLawFactory, SineLawSetup, TickInputs,
    DEFAULT_TICK_S,
};
use hertzforge_sine_laws::WasmSineLawFactory;
use std::time::{Duration, Instant};

#[test]
fn a_sine_law_file_call_at_the_default_tick_is_well_inside_its_budget() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host
        .load_sine(&std::fs::read(law_file("hertzforge_law_sine_servo")).unwrap())
        .unwrap();
    let params = law.resolve_params(&[], 1).unwrap();
    let factory = WasmSineLawFactory::new(law, params);
    let f: Vec<f64> = (1..=400).map(|k| k as f64 * 2.0).collect();
    let n = f.len();
    let (gain, coherence, trusted, target) =
        (vec![0.5; n], vec![0.99; n], vec![true; n], vec![1.0; n]);
    let budget_s = LawGuards::default().call_budget_s(DEFAULT_TICK_S);
    assert!((budget_s - 2.5e-3).abs() < 1e-12);
    let setup = SineLawSetup {
        control_channels: 1,
        tick_s: DEFAULT_TICK_S,
        band_hz: (100.0, 500.0),
        drive_ceiling: 5.0,
        identification: Identification {
            frequency_hz: &f,
            gain: &gain,
            coherence: &coherence,
            trusted: &trusted,
            target: &target,
        },
        call_budget_s: budget_s,
    };
    let mut l = factory.build(&setup).unwrap();
    let mut values = LawValues::new(factory.describe().diagnostics.len());
    let amp = [0.9];
    let phase = [-10.0];
    let inputs = |tick_index: u64, freq: f64, measured: f64, drive: f64| TickInputs {
        tick_index,
        dt_s: DEFAULT_TICK_S,
        frequency_hz: freq,
        sweep_rate_hz_per_s: 0.7,
        level_db: 0.0,
        target: 1.0,
        measured,
        channel_amplitude: &amp,
        channel_phase_deg: &phase,
        drive,
        prediction_db: Some(6.0),
        hold: Hold::Free,
        notch_ceiling: None,
        settling: false,
    };
    let mut drive = l
        .engage(&inputs(0, 100.0, 0.0, 0.0), &mut values)
        .unwrap()
        .drive;
    let mut times = Vec::with_capacity(10_000);
    for k in 1..=10_000u64 {
        // A slow sweep with the response wandering a little either side of
        // the target, so the servo works every tick.
        let freq = 100.0 + 0.007 * k as f64;
        let measured = 1.0 + 0.05 * ((k as f64) * 0.01).sin();
        let i = inputs(k, freq, measured, drive);
        let t0 = Instant::now();
        let o = l.tick(&i, &mut values).unwrap();
        times.push(t0.elapsed());
        drive = o.drive;
    }
    times.sort();
    let (median, p999) = (times[times.len() / 2], times[(times.len() * 999) / 1000]);
    println!(
        "Sine law-file call at a 10 ms tick: median {median:?}, p99.9 {p999:?}, worst {:?}",
        times[times.len() - 1]
    );
    let quarter = Duration::from_secs_f64(budget_s / 4.0);
    assert!(
        p999 <= quarter,
        "p99.9 {p999:?} over a quarter of the {budget_s} s budget"
    );
}
