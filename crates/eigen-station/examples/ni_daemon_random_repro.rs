//! Reproduce the DAEMON's exact Random construction on the real chassis, outside
//! the daemon, and print the sysID internals the daemon cannot show.
//!
//! Written 2026-08-01 while chasing a defect: the first remote Random run built its
//! station, passed the arm gate, ran sysID on the real 9263->jumper->9234 path -
//! and the coherence gate HELD at 0.041, while `ni_random_arm_probe` (different
//! construction: 1 derived input, probe table) coheres near 1.0 on the same bench.
//! This probe holds everything at the DAEMON's choices (the 4-row default seed
//! table, the wire-profile translation, `RandomTestConfig::siso`, the demo
//! LoopConfig band-sized to the profile) so the differing ingredient can be
//! bisected instead of guessed.

use eigen_breakpoints::{BreakpointRow, BreakpointTable};
use eigen_random::service::config::{LevelSchedule, RunMode};
use eigen_random::{LoopConfig, RandomService, RandomTestConfig};
use eigen_station::{
    try_build_station_backend, BackendKind, NiSelection, SimPlantSpec, StationSpec,
};

fn main() {
    let mut derive_inputs: usize = 4; // the daemon's table-derived count
    let mut sysid_db: f64 = -20.0; // RandomTestConfig::siso's default
    let mut dwell_s: f64 = 30.0; // the UI schedule's per-level dwell
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--inputs" => derive_inputs = args.next().and_then(|v| v.parse().ok()).unwrap_or(4),
            "--sysid-db" => sysid_db = args.next().and_then(|v| v.parse().ok()).unwrap_or(-20.0),
            "--dwell" => dwell_s = args.next().and_then(|v| v.parse().ok()).unwrap_or(30.0),
            other => {
                eprintln!("unknown argument: {other} (usage: [--inputs N])");
                std::process::exit(2);
            }
        }
    }

    // ---- the daemon's seed table: default_with_channels(4) ----
    let table = eigen_channel_table::ChannelTable::default_with_channels(derive_inputs);
    let control_index = table
        .channels
        .iter()
        .find(|c| c.enabled && c.role == eigen_channel_table::ChannelRole::Control)
        .or_else(|| {
            table
                .channels
                .iter()
                .find(|c| c.enabled && c.role != eigen_channel_table::ChannelRole::Drive)
        })
        .map(|c| c.index)
        .expect("a control channel");
    println!(
        "table: {} rows, control_index {}",
        table.channels.len(),
        control_index
    );
    for c in &table.channels {
        println!(
            "  ch{} role={:?} enabled={} sens={} mV/EU range={} EU",
            c.index, c.role, c.enabled, c.sensitivity_mv_per_eu, c.range_max
        );
    }

    // ---- the wire profile the UI sent, translated exactly as the daemon does ----
    // The UI's DEFAULT profile (randomStore seed rows): the shaped NAVMAT-ish
    // table, not a flat line - the shape is what separates the runs that abort
    // (UI-driven daemon) from the flat-profile runs that pass.
    let mut bp = BreakpointTable::new("remote profile");
    bp.rows.push(BreakpointRow::psd(20.0, 0.01));
    bp.rows.push(BreakpointRow::psd(80.0, 0.04));
    bp.rows.push(BreakpointRow::psd(350.0, 0.04));
    bp.rows.push(BreakpointRow::psd(2000.0, 0.007));
    let mut cfg = RandomTestConfig::siso(bp, control_index);
    cfg.schedule = LevelSchedule {
        mode: RunMode::Auto,
        start_db: -12.0,
        table_db: vec![-12.0, -9.0, -6.0, -3.0, 0.0],
        dwell_s: vec![dwell_s],
    };
    cfg.full_level_duration_s = 10.0;
    cfg.sysid_level_db = sysid_db;
    println!("sysid_level_db: {sysid_db}");

    // ---- the daemon's loop config ----
    let mut plant = LoopConfig::demo_default();
    if let Ok(anchors) = cfg.breakpoints.resolve() {
        if let (Some(first), Some(last)) = (anchors.first(), anchors.last()) {
            let nyq = plant.fs_hz / 2.0;
            let lo = first.freq_hz.max(plant.df());
            let hi = last.freq_hz.min(nyq * 0.95);
            if hi > lo {
                plant.band_lo_hz = lo;
                plant.band_hi_hz = hi;
            }
        }
    }
    let n_inputs = table
        .channels
        .iter()
        .filter(|c| c.enabled && c.role != eigen_channel_table::ChannelRole::Drive)
        .count()
        .max(1) as u32;
    println!(
        "loop: fs {} nfft {} band {}-{} Hz, n_inputs {}",
        plant.fs_hz, plant.nfft, plant.band_lo_hz, plant.band_hi_hz, n_inputs
    );

    // ---- the daemon's station spec ----
    let spec = StationSpec {
        sim: SimPlantSpec {
            sample_rate_hz: plant.fs_hz,
            resonances: vec![(200.0, 2.5), (550.0, 2.5)],
            noise_level: 2e-4,
            feedthrough: 0.35,
            shared_clock: true,
            seed: 0x5A5A_1234,
            max_input_channels: n_inputs.max(1),
            max_output_channels: 1,
            ..SimPlantSpec::default()
        },
        ni: NiSelection {
            inputs: eigen_station::physical_from_table(&table).0,
            outputs: eigen_station::physical_from_table(&table).1,
            derive_inputs: n_inputs.max(1) as usize,
            derive_outputs: 1,
            iepe: false,
            restrict_devices: vec![ai_dev(), ao_dev()],
        },
        ni_block_frames: eigen_hal::default_block_frames(plant.fs_hz),
    };
    let station = match try_build_station_backend(BackendKind::Ni, &spec) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("station REFUSED: {e}");
            std::process::exit(1);
        }
    };
    println!("station: {}", station.provenance.log_line());

    let mut svc = RandomService::new(cfg, table, plant, station.backend, station.station, None)
        .expect("service");

    // The FULL AUTO ladder, exactly what the daemon's Start + pump drive.
    let end = svc.run_auto(4, 200_000).expect("run_auto");
    let st = svc.status();
    println!("run ended in {:?}", end);
    println!(
        "  level {:.2} dB  measured {:?} gRMS  band coherence {:.4}",
        st.current_level_db, st.measured_grms, st.band_coherence
    );
    println!(
        "  alarm_active {}  abort_active {}",
        st.alarm_active, st.abort_active
    );
    if let Some(t) = svc.trip() {
        println!("  TRIP: {:?}", t);
    }
    for ev in svc.log().events().iter().rev().take(12) {
        println!("  log: {:?}", ev.kind);
    }
    svc.stop().ok();
    println!("stopped.");
}

/// The AI and AO modules this example binds. `EIGEN_NI_AI_DEV` / `EIGEN_NI_AO_DEV`
/// override the defaults, so a chassis that enumerates under another name
/// (`cDAQ2Mod1`, ...) runs without editing this file and without renaming the
/// chassis in NI MAX.
fn ai_dev() -> String {
    ni_dev_from_env("EIGEN_NI_AI_DEV", "cDAQ1Mod1")
}

fn ao_dev() -> String {
    ni_dev_from_env("EIGEN_NI_AO_DEV", "cDAQ1Mod2")
}

/// `var` when it is set and not blank, else `default`.
fn ni_dev_from_env(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}
