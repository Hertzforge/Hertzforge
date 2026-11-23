//! The abort watchdog sees ONE acquisition across Random's segment seams.
//!
//! Random runs a gapless acquisition as a chain of fresh engines, one per
//! segment. Each engine's watchdog used to start with fresh integrators, which
//! seed their high-pass on the first sample: mid-run that sample is anywhere in
//! the response, the filter takes its distance from the mean as a step, and the
//! displacement keeps about tau squared times it (about 390 mil per g at the
//! 5 Hz corner). On the bench (2026-11-23) a 0.7 g rms identification probe
//! tripped a 200 mil displacement abort with nothing near that moving.
//!
//! RED-MUTATION: drop the adoption of `carry_watchdog` in `run_segment` and
//! `a_tone_across_warm_seams_does_not_trip_the_displacement_abort` reds.

use hertzforge_channel_table::{AbortLimits, Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal_sim::SimConfig;
use hertzforge_module_abi::StationClass;
use hertzforge_random::service::engine::PlantLoop;
use hertzforge_random::LoopConfig;

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 100.0;
    ctrl.abort = AbortLimits {
        accel_g: Some(50.0),
        velocity_ips: Some(50.0),
        displacement_mil: Some(200.0),
    };
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(drv);
    t
}

#[test]
fn a_tone_across_warm_seams_does_not_trip_the_displacement_abort() {
    let backend = Box::new(
        SimConfig::new()
            .sample_rate_hz(8192.0)
            .resonances(vec![])
            .noise_level(0.0)
            .feedthrough(1.0)
            .shared_clock(true)
            .seed(7)
            .max_input_channels(1)
            .max_output_channels(1)
            .build(),
    );
    let mut cfg = LoopConfig::demo_default();
    let mut plant = PlantLoop::open(backend, StationClass::HertzforgeSim, &mut cfg, table())
        .expect("open must succeed");
    let fs = cfg.fs_hz;
    let n = cfg.window_len();
    // A 2 V, 400 Hz tone through unit feedthrough: about 2 g, and well under a
    // mil of real displacement. A segment that is not a whole number of cycles
    // puts each seam at a different phase, most of them far from zero.
    // It fades in over its first second, as a drive does, so the onset itself
    // is not a step.
    let tone = |i: usize| {
        let fade = (i as f64 / fs).min(1.0);
        let env = 0.5 - 0.5 * (std::f64::consts::PI * fade).cos();
        2.0 * env * (std::f64::consts::TAU * 400.0 * i as f64 / fs + 0.3).sin()
    };
    let mut at = 0usize;
    let leadin: Vec<f64> = (0..plant.cold_leadin_frames()).map(tone).collect();
    at += leadin.len();
    let mut first = Some(leadin);
    let mut peak = 0.0f64;
    for k in 0..8 {
        let drive: Vec<f64> = (at..at + n).map(tone).collect();
        at += n;
        let seg = plant.run_segment(first.take().unwrap_or_default(), drive);
        assert!(
            seg.faulted.is_none(),
            "segment {k} faulted: {:?}",
            seg.faulted
        );
        peak = seg.interleaved.iter().fold(peak, |m, v| m.max(v.abs()));
        assert!(
            seg.watchdog_trip.is_none(),
            "segment {k}: {:?} with a {peak:.2} g tone",
            seg.watchdog_trip
        );
    }
    assert!(
        peak > 1.0,
        "the response must be large for the seams to matter: {peak} g"
    );
}
