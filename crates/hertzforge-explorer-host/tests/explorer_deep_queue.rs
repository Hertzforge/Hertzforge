//! sim-6: the fast walk on a station that states the NI chassis's output lookahead
//! (512 frames at 256-frame blocks, 12 800 Hz), where the drive written ahead of the
//! clock is what a survey point must be aligned past. Twelve clicks 1/6 octave apart,
//! 160 ms each, the level alternating 0.1 and 0.2 V pk at 240 dB/s: every point must
//! read its own click's level, not the click before, which is 6 dB away.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_explorer_host::service::{ExplorerHostConfig, ExplorerHostService};
use hertzforge_explorer_host::slew::SlewLimits;
use hertzforge_explorer_host::step::AmplitudeRamp;
use hertzforge_explorer_host::{FrequencyPart, LevelPart};
use hertzforge_module_abi::StationClass;

const FS: f64 = 12_800.0;
const BLOCK: usize = 256;

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut c = Channel::new(1);
    c.role = ChannelRole::Control;
    c.units = Units::AccelG;
    c.range_max = 10.0;
    t.channels.push(c);
    let mut d = Channel::new(2);
    d.role = ChannelRole::Drive;
    d.units = Units::VoltageV;
    t.channels.push(d);
    t
}

#[test]
fn explorer_sim_6_a_fast_walk_behind_the_chassis_queue_reads_each_clicks_own_level() {
    let plant = hertzforge_hal_sim::SimConfig::new()
        .sample_rate_hz(FS)
        .resonances(Vec::new())
        .feedthrough(1.0)
        .noise_level(0.0)
        .transport_delay_samples(0)
        .min_lookahead_frames(2 * BLOCK as u32)
        .build();
    let cfg = ExplorerHostConfig {
        fs_hz: FS,
        band_hz: (20.0, 2_000.0),
        start_hz: 300.0,
        max_drive_vpk: 0.5,
        slew: SlewLimits {
            freq_oct_per_s: 4.0,
            drive_db_per_s: 240.0,
        },
        ramp: AmplitudeRamp::default(),
        display_nfft: 4_096,
        ring_blocks: 16,
    };
    let mut svc =
        ExplorerHostService::new(cfg, table(), Box::new(plant), StationClass::HertzforgeSim)
            .unwrap()
            .with_block_frames(BLOCK);
    svc.arm().unwrap();
    svc.start().unwrap();
    let pass = |svc: &mut ExplorerHostService| {
        svc.run_batches(1).unwrap();
        svc.service_analysis()
    };
    svc.set_amplitude_vpk(0.1).unwrap();
    let mut st = pass(&mut svc);
    for _ in 0..40 {
        st = pass(&mut svc);
    }
    assert_eq!(
        st.pipeline_frames, 1024,
        "two lookaheads, no group delay stated"
    );

    // Four passes of 512 frames: 160 ms a click.
    let clicks: Vec<(f64, f64)> = (0..12)
        .map(|i| {
            let hz = 300.0 * 2f64.powf(i as f64 / 6.0);
            (hz, if i % 2 == 0 { 0.2 } else { 0.1 })
        })
        .collect();
    for (hz, vpk) in &clicks {
        svc.steer(Some(FrequencyPart::Hz(*hz)), Some(LevelPart::Vpk(*vpk)))
            .unwrap();
        for _ in 0..4 {
            pass(&mut svc);
        }
    }
    // A departure for the last click, then long enough for it to arrive.
    svc.steer(Some(FrequencyPart::Hz(1_900.0)), None).unwrap();
    for _ in 0..6 {
        pass(&mut svc);
    }
    let points = svc.survey_points().to_vec();
    for (hz, vpk) in &clicks {
        let (_, level) = points
            .iter()
            .copied()
            .find(|(f, _)| (f - hz).abs() < 1e-9)
            .unwrap_or_else(|| panic!("no point at {hz} Hz: {points:?}"));
        let level = f64::from(level);
        assert!(
            (level - vpk).abs() / vpk < 0.01,
            "{hz:.1} Hz read {level} where its click set {vpk}"
        );
    }
}
