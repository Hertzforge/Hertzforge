//! `ni-latency-probe` - how many frames LATE is the response to the drive?
//!
//! Every sysid estimator in the tree pairs `drive[k]` with `response[k]`. That
//! pairing is only true if the first frame the caller READS is the response to
//! the first frame it WROTE. Two things can break it:
//!
//! - **Physics.** The 9234 is delta-sigma; its digital filter has a fixed group
//!   delay (the driver reports ~39 frames at 51.2 kHz), and the 9263 adds its
//!   own. This is real and cannot be booked away.
//! - **BOOKKEEPING.** Whatever silence sits in the AO buffer ahead of the
//!   caller's drive at start comes back as input frames FIRST. If the backend
//!   prefills a lookahead of zeros so the AO can start at all (-200462), the
//!   caller's `response[0]` is the response to that silence, not to its own
//!   `drive[0]`, and every later index is shifted by the same amount.
//!
//! A constant shift is harmless to a coherence estimate ONLY while it stays
//! small relative to the estimator's segment length. Once the shift approaches
//! or exceeds one segment, drive and response are effectively unrelated inside
//! each segment and coherence collapses toward zero - which is exactly what TWR
//! (0.010) and Sine (0.067) reported on a jumpered WIRE, where coherence should
//! be ~1.0.
//!
//! This probe MEASURES the shift instead of reasoning about it: write silence,
//! then a distinctive burst, then silence; read the same number of frames back;
//! cross-correlate to find the lag that maximizes the match.
//!
//! ```text
//! cargo run --release -p hertzforge-station --example ni_latency_probe
//! ```
//!
//! Bench contract: jumpered AO -> AI, IEPE OFF, nothing downstream.

use hertzforge_hal::Backend;
use hertzforge_station::{
    try_build_station_backend, BackendKind, NiSelection, SimPlantSpec, StationSpec,
    NI_BUILD_RATE_HZ,
};

/// The AI and AO modules this example binds. `HERTZFORGE_NI_AI_DEV` / `HERTZFORGE_NI_AO_DEV`
/// override the defaults, so a chassis that enumerates under another name
/// (`cDAQ2Mod1`, ...) runs without editing this file and without renaming the
/// chassis in NI MAX.
fn ai_dev() -> String {
    ni_dev_from_env("HERTZFORGE_NI_AI_DEV", "cDAQ1Mod1")
}

fn ao_dev() -> String {
    ni_dev_from_env("HERTZFORGE_NI_AO_DEV", "cDAQ1Mod2")
}

/// `var` when it is set and not blank, else `default`.
fn ni_dev_from_env(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn table() -> hertzforge_channel_table::ChannelTable {
    use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 10.0;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(drv);
    t
}

fn main() {
    let spec = StationSpec {
        sim: SimPlantSpec::default(),
        ni: NiSelection {
            derive_inputs: 1,
            derive_outputs: 1,
            iepe: false,
            restrict_devices: vec![ai_dev(), ao_dev()],
            ..Default::default()
        },
        ni_block_frames: hertzforge_hal::default_block_frames(NI_BUILD_RATE_HZ),
    };
    let station = match try_build_station_backend(BackendKind::Ni, &spec) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("station: REFUSED - {e:?}");
            std::process::exit(2);
        }
    };
    if station.provenance.driver_simulated == Some(true) {
        eprintln!("station is DRIVER-SIMULATED - a lag measured here means nothing");
        std::process::exit(2);
    }
    println!("=== ni-latency-probe ===");
    println!("station: {}", station.provenance.log_line());

    let mut backend = station.backend;
    let block = hertzforge_hal::default_block_frames(51_200.0);
    let acq = hertzforge_hal::AcquisitionConfig::new(51_200.0, block);
    backend.open().expect("open");
    let actual = backend.configure(&table(), &acq).expect("configure");
    let fs = actual.actual_rate_hz;
    println!(
        "fs {fs} Hz  block {} frames  group_delay_frames {:?}",
        actual.block_frames, actual.group_delay_frames
    );
    let contract = backend.caps().output_buffer;
    println!("output contract: {contract:?}");

    backend.arm().expect("arm");
    backend.start().expect("start");

    // silence | burst | silence. The burst is a short tone package so the
    // cross-correlation has a sharp, unambiguous peak.
    let pad = 4096usize;
    let burst = 2048usize;
    let total = pad + burst + pad;
    let drive: Vec<f32> = (0..total)
        .map(|k| {
            if k < pad || k >= pad + burst {
                0.0
            } else {
                let t = (k - pad) as f64 / fs;
                let env = (std::f64::consts::PI * (k - pad) as f64 / burst as f64).sin();
                (0.30 * env * (2.0 * std::f64::consts::PI * 1500.0 * t).sin()) as f32
            }
        })
        .collect();

    // Write and read in lockstep by BLOCK, honoring accepted counts - the same
    // contract `play_block_wise` enforces, written out here so the probe does
    // not depend on the helper it is measuring.
    let mut response = vec![0.0f32; 0];
    let mut written = 0usize;
    let mut scratch = vec![0.0f32; block];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while response.len() < total && std::time::Instant::now() < deadline {
        if written < total {
            let want = (total - written).min(block);
            match backend.write_drive(&drive[written..written + want]) {
                Ok(n) => written += n,
                Err(e) => {
                    eprintln!("write: {e}");
                    break;
                }
            }
        }
        let want_read = (written - response.len()).min(block);
        if want_read == 0 {
            continue;
        }
        match backend.read_input(&mut scratch[..want_read]) {
            Ok(hertzforge_hal::InputRead::Frames(n)) if n > 0 => {
                response.extend_from_slice(&scratch[..n]);
            }
            Ok(_) => std::thread::sleep(std::time::Duration::from_micros(200)),
            Err(e) => {
                eprintln!("read: {e}");
                break;
            }
        }
    }
    let _ = backend.stop();
    println!("wrote {written} frames, read {} frames", response.len());
    if response.len() < pad + burst {
        eprintln!("too few frames read to measure a lag");
        std::process::exit(1);
    }

    // Cross-correlate |drive| against |response| over a generous lag window.
    // Envelope correlation (not raw) so the tone's own phase does not create
    // spurious peaks one cycle apart.
    let env = |v: &[f32]| -> Vec<f64> { v.iter().map(|x| x.abs() as f64).collect() };
    let d = env(&drive);
    let r = env(&response);
    let max_lag = (pad + burst).min(r.len().saturating_sub(1));
    let mut best = (0usize, f64::MIN);
    for lag in 0..max_lag {
        let n = (d.len().min(r.len() - lag)).min(pad + burst + 512);
        if n < burst {
            break;
        }
        let mut s = 0.0;
        for k in 0..n {
            s += d[k] * r[k + lag];
        }
        if s > best.1 {
            best = (lag, s);
        }
    }
    let (lag, _) = best;
    println!();
    println!("--- MEASURED ---");
    println!(
        "drive->response lag: {lag} frames ({:.3} ms at {fs} Hz)",
        1000.0 * lag as f64 / fs
    );
    let gd = actual.group_delay_frames.unwrap_or(0) as usize;
    let lookahead = contract
        .map(|c| c.min_lookahead_frames as usize)
        .unwrap_or(0);
    println!("  driver group delay:      {gd} frames  (physics - cannot be booked away)",);
    println!(
        "  AO lookahead (prefill):  {lookahead} frames  (BOOKKEEPING - silence queued ahead of the caller's drive)",
    );
    let explained = gd + lookahead;
    println!("  gd + lookahead:          {explained} frames");
    println!();
    if lag > 64 {
        println!(
            "VERDICT: the response is {lag} frames LATE. Any estimator that pairs drive[k] with \
             response[k] over segments shorter than ~{lag} frames sees UNRELATED signals and its \
             coherence collapses - which is the TWR (0.010) / Sine (0.067) symptom on a wire."
        );
    } else {
        println!("VERDICT: lag is small ({lag} frames); pairing is essentially aligned.");
    }
}
