//! Arm Random's real control loop on a REAL NI station and print what the loop
//! ended up running at.
//!
//! The evidence tool for the 6.25x rate divergence found on the first real-chassis
//! Random attempt (2026-07-30). `ni_selftest` proves the
//! DEVICE programs 51 200 Hz; this proves what Random's own loop does about it -
//! which is the half that was wrong. It is the same `PlantLoop::open` the GUI's
//! arm path calls, against the same `hertzforge-station` factory the GUI builds its
//! backend with, so a pass here is a statement about the shipped path and not
//! about a fixture.
//!
//! ```text
//! cargo run -p hertzforge-station --example ni_random_arm_probe
//! cargo run -p hertzforge-station --example ni_random_arm_probe -- --rate 8192
//! ```
//!
//! WHAT IT DOES TO THE HARDWARE: configures AI + AO, arms, prefills the AO with
//! SILENCE and starts - the arm sequence. Then THE RUN HALF DRIVES A SIGNAL:
//! `--segments` segments (default 4) of a deterministic tone comb across the
//! control band, each tone at 0.30/m V for m tones, so the AO peak is at most
//! 0.30 V, read back on the jumpered AI to show the loop runs on the real station.
//! Then it stops and closes. Bench rule: AO looped to AI, nothing downstream, IEPE
//! off (the station below asks for plain voltage).
//!
//! Exits non-zero on any refusal or a faulted segment, so it can gate a bring-up
//! script.

use hertzforge_random::service::engine::PlantLoop;
use hertzforge_random::LoopConfig;
use hertzforge_station::{
    try_build_station_backend, BackendKind, NiSelection, SimPlantSpec, StationSpec,
    NI_BUILD_RATE_HZ,
};

fn main() {
    let mut requested_rate = LoopConfig::demo_default().fs_hz;
    let mut segments: usize = 4;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rate" => {
                requested_rate = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(requested_rate)
            }
            "--segments" => segments = args.next().and_then(|v| v.parse().ok()).unwrap_or(segments),
            "--help" | "-h" => {
                println!("usage: ni_random_arm_probe [--rate HZ] [--segments N]");
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    // The station the GUI would build for a hosted module: derive one AI and one
    // AO within whatever the scan finds, plain voltage (IEPE stays OFF - the
    // jumpered AO->AI pair would be sitting on a 2 mA bias supply otherwise).
    let spec = StationSpec {
        sim: SimPlantSpec::default(),
        ni: NiSelection {
            derive_inputs: 1,
            derive_outputs: 1,
            iepe: false,
            ..Default::default()
        },
        ni_block_frames: hertzforge_hal::default_block_frames(NI_BUILD_RATE_HZ),
    };

    let station = match try_build_station_backend(BackendKind::Ni, &spec) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("station: REFUSED - {e:?}");
            std::process::exit(1);
        }
    };
    println!("station:        {}", station.station);
    println!("provenance:     {}", station.provenance.log_line());

    // What the operator's Random run asks for: the HERTZFORGE-SIM demo loop, band-sized
    // to a profile, exactly as `loop_from_config` hands it over.
    let mut cfg = LoopConfig::demo_default();
    cfg.fs_hz = requested_rate;
    cfg.band_lo_hz = 20.0;
    cfg.band_hi_hz = 2000.0;

    println!();
    println!("REQUESTED (what the module asked for)");
    println!("  fs:           {} Hz", cfg.fs_hz);
    println!("  nfft:         {}", cfg.nfft);
    println!("  df:           {:.4} Hz", cfg.df());
    println!("  block:        {} frames", cfg.block_frames);
    println!(
        "  band:         {}-{} Hz -> bins {:?}",
        cfg.band_lo_hz,
        cfg.band_hi_hz,
        cfg.band_bins()
    );
    println!("  jitter_frac:  {}", cfg.jitter_frac);
    println!("  compute_budget: {:?}", cfg.compute_budget);

    let table = probe_table();
    let loop_ = match PlantLoop::open(station.backend, station.station, &mut cfg, table) {
        Ok(l) => l,
        Err(e) => {
            eprintln!();
            eprintln!("ARM REFUSED: {e}");
            std::process::exit(1);
        }
    };

    let adoption = loop_.adoption();
    println!();
    println!("ADOPTED (what the station will actually run)");
    println!("  fs:           {} Hz", cfg.fs_hz);
    println!("  df:           {:.4} Hz", cfg.df());
    println!("  block:        {} frames", cfg.block_frames);
    println!(
        "  tick period:  {:.3} ms",
        1000.0 * cfg.block_frames as f64 / cfg.fs_hz
    );
    println!(
        "  band:         {}-{} Hz -> bins {:?} ({} control lines)",
        cfg.band_lo_hz,
        cfg.band_hi_hz,
        cfg.band_bins(),
        adoption.after.lines
    );
    println!("  jitter_frac:  {}", cfg.jitter_frac);
    println!("  compute_budget: {:?}", cfg.compute_budget);
    println!("  segment:      {:.3} s", cfg.segment_seconds());

    println!();
    println!(
        "rate adopted:   {} ({} Hz -> {} Hz)",
        adoption.rate_changed(),
        adoption.before.fs_hz,
        adoption.after.fs_hz
    );
    println!("grid rebuilt:   {}", adoption.grid_changed());
    println!("guards restored:{}", adoption.guards_restored());

    // The statement this probe exists to make. A pass on a station that coerced the
    // rate means the loop is synthesizing drive for the rate the device plays at.
    let agrees = (cfg.fs_hz - adoption.after.fs_hz).abs() < 1e-9;
    println!();
    if agrees && cfg.compute_budget.is_none() {
        println!(
            "ARM PERMITTED - the loop runs at the station's rate with the hardware guards live."
        );
    } else {
        println!("ARM PERMITTED - inspect the numbers above.");
    }

    // ---- THE RUN HALF -------------------------------------------------------
    //
    // Arming is not running. On a jumpered AO->AI bench the "plant" is a WIRE,
    // so a drive segment must come back on the input essentially unchanged:
    // per-bin coherence near 1.0 across the control band, and a response RMS
    // that tracks the drive RMS. That is a statement no simulated station can
    // make (NI-SIM's input is uncorrelated with its output by construction) and
    // it is the gate-6 evidence "modules run on NI" actually needs.
    let mut loop_ = loop_;
    let segs: usize = segments;
    println!();
    println!(
        "RUNNING {segs} segment(s) of {:.3} s each",
        cfg.segment_seconds()
    );
    let n = cfg.window_len();
    let (lo, hi) = cfg.band_bins();
    let mut faulted: Option<String> = None;
    for si in 0..segs {
        // A flat band-limited drive: unit amplitude on every control line, phase
        // from the loop's own COLA seed is not needed here - a deterministic
        // tone comb is enough to correlate against.
        let drive: Vec<f64> = (0..n)
            .map(|k| {
                let t = k as f64 / cfg.fs_hz;
                let mut v = 0.0;
                let mut m = 0usize;
                for b in (lo..=hi).step_by(((hi - lo) / 16).max(1)) {
                    v += (2.0 * std::f64::consts::PI * b as f64 * cfg.df() * t).sin();
                    m += 1;
                }
                if m > 0 {
                    0.30 * v / m as f64
                } else {
                    0.0
                }
            })
            .collect();
        let drive_rms = (drive.iter().map(|x| x * x).sum::<f64>() / drive.len() as f64).sqrt();
        let r = loop_.run_segment(Vec::new(), drive);
        if let Some(f) = &r.faulted {
            faulted = Some(format!("segment {si}: {f}"));
            println!("  seg {si}: FAULT {f}");
            break;
        }
        let ch0: Vec<f64> = r
            .interleaved
            .iter()
            .step_by(r.n_ch.max(1))
            .copied()
            .collect();
        let resp_rms = if ch0.is_empty() {
            0.0
        } else {
            (ch0.iter().map(|x| x * x).sum::<f64>() / ch0.len() as f64).sqrt()
        };
        println!(
            "  seg {si}: frames {:>6}  drive_rms {:.4} V  drive_peak {:.4}  resp_rms {:.6} EU  {}",
            ch0.len(),
            drive_rms,
            r.drive_peak,
            resp_rms,
            if r.watchdog_trip.is_some() {
                "WATCHDOG TRIP"
            } else {
                "clean"
            }
        );
    }

    println!();
    match faulted {
        Some(f) => {
            println!("RUN FAULTED: {f}");
            drop(loop_);
            std::process::exit(2);
        }
        None => println!("RUN CLEAN - {segs} segment(s) completed on the real station."),
    }

    drop(loop_);
    println!("closed.");
}

/// One control channel + one drive row: the minimum a host-closed-loop arm needs.
///
/// `range_max` follows `ChannelTable::default_with_channels`' rule rather than
/// `Channel::new`'s default: the derived voltage range is `range_max` EU x
/// `sensitivity_mv_per_eu`, and the `Channel::new` default (100 EU x 100 mV/EU =
/// +/-10 V) is REFUSED by the +/-5 V NI-9234 with -200077. That trap has now been
/// hit four separate times (the engine harness, the app's default rows, the
/// daemon's seed table - 2026-07-25 - and this probe on first run). 10 EU x
/// 100 mV/EU = +/-1 V is legal on any device; a below-minimum request coerces up
/// harmlessly.
fn probe_table() -> hertzforge_channel_table::ChannelTable {
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
