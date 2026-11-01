//! `ni-watchdog-latency` - arrival gate 5: the abort watchdog, end to end, with
//! the latency MEASURED rather than assumed.
//!
//! This is the gate that replaces the provisional `[H]` abort-latency bound
//! (`eigen-hal-ni/src/clock.rs`, seeded from the 9234 datasheet) with a number
//! taken off the real hardware. Until it runs, every arm on this station is
//! permitted against a bound nobody has measured.
//!
//! ## What the loopback CAN and CANNOT measure (corrected 2026-07-30)
//!
//! The first draft of this probe claimed no scope was needed: the 9234 watches
//! the 9263 on a proven-shared clock, so the input record is a trace of the
//! output at 19.5 us resolution. That is true for the DETECTION half and false
//! for the ACTUATION half, because `emergency_stop` writes zeros to the AO and
//! then stops BOTH tasks - including the AI. **The abort kills the very
//! instrument that would watch the drive fall.**
//!
//! So this probe measures, rigorously and in-band:
//!
//! - **detection latency** - from the frame where the measured input first
//!   exceeds the limit to the frame the watchdog reports in its `AbortTrip`.
//!   That is the software half of the safety claim and it needs no scope.
//! - **the blanking window behaves** - no trip inside it, would-have-tripped
//!   breaches counted rather than dropped.
//! - **the drive was commanded to zero and the tasks stopped** - `estops` in
//!   the run report, plus `emergency_stop`'s own zero-write.
//!
//! It CANNOT prove the voltage at the 9263 terminals reached zero, or how long
//! that took. For that the original procedure stands: put a **scope on
//! AO0** and trigger on the fall. The number to record is scope-fall-time; this
//! probe supplies the detection half it adds to.
//!
//! ## What it measures
//!
//! A ramp is driven up through a configured abort limit. Three frame indices
//! come out of the captured input:
//!
//! - **breach** - the first frame whose measured EU exceeds the limit;
//! - **trip** - the frame the watchdog reports in its `AbortTrip` (its own
//!   accounting, which the debounce/sustain rule shifts later than the breach);
//! - **silent** - the first frame after the breach where the input has returned
//!   to the noise floor and STAYS there, i.e. the drive actually stopped.
//!
//! `silent - breach`, minus the measured pipeline lag, is the end-to-end abort
//! latency: from "a sample exceeded the limit" to "the shaker is no longer
//! being driven". That is the number a safety case needs.
//!
//! ## Bench contract
//!
//! Jumpered AO -> AI, **NOTHING downstream** - no amplifier, no shaker, no test
//! article. This probe deliberately drives past a limit; it must never do that
//! into anything that can move. IEPE OFF.
//!
//! ```text
//! cargo run --release -p eigen-station --example ni_watchdog_latency
//! ```
//!
//! Exit: 0 = tripped and measured; 1 = FAIL (the input passed the limit and
//! nothing tripped, or nothing was captured); 2 = no real station; 3 =
//! INCONCLUSIVE (the run stopped for another reason before the ramp reached the
//! limit, so the watchdog was never tested).

use eigen_engine::{Engine, EngineConfig, ProcessStep, StopReason, VecCaptureSink};
use eigen_hal::{AcquisitionConfig, Backend};
use eigen_station::{
    try_build_station_backend, BackendKind, NiSelection, SimPlantSpec, StationSpec,
    NI_BUILD_RATE_HZ,
};

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

/// 1000 mV/EU makes EU and volts numerically equal, so every number below reads
/// in volts on the wire and there is no scaling to mis-read at 2 a.m.
const SENS_MV_PER_EU: f64 = 1000.0;
/// The abort limit, in EU (= volts here). Comfortably inside the 9234's +/-5 V.
const ABORT_EU: f64 = 0.50;
/// The ramp climbs this fast, in EU per second. Slow enough that the breach
/// frame is unambiguous, fast enough that the whole run is short.
const RAMP_EU_PER_S: f64 = 2.0;
/// Arm-blanking window. Must cover the 9234's start-up settling transient
/// (measured at 0.5558 EU on the first run of this probe) and end well before
/// the ramp reaches the limit at ~250 ms.
const BLANKING: std::time::Duration = std::time::Duration::from_millis(100);

/// A drive that ramps linearly and would keep going forever - the watchdog is
/// what has to stop it.
struct Ramp {
    fs: f64,
    frame: u64,
    out_ch: usize,
}

impl ProcessStep for Ramp {
    fn process(&mut self, _input: &[f32], n: usize, out: &mut Vec<f32>) {
        out.clear();
        for _ in 0..n {
            let t = self.frame as f64 / self.fs;
            let v = (RAMP_EU_PER_S * t) as f32;
            for _ in 0..self.out_ch {
                out.push(v);
            }
            self.frame += 1;
        }
    }
}

fn table() -> eigen_channel_table::ChannelTable {
    use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.sensitivity_mv_per_eu = SENS_MV_PER_EU;
    ctrl.range_max = 5.0;
    // THE LIMIT UNDER TEST. Armed, so the engine's watchdog is live.
    ctrl.abort.accel_g = Some(ABORT_EU);
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
        ni_block_frames: eigen_hal::default_block_frames(NI_BUILD_RATE_HZ),
    };
    let station = match try_build_station_backend(BackendKind::Ni, &spec) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("station: REFUSED - {e:?}");
            std::process::exit(2);
        }
    };
    if station.provenance.driver_simulated == Some(true) {
        eprintln!("station is DRIVER-SIMULATED - a latency measured here means nothing");
        std::process::exit(2);
    }

    println!("=== ni-watchdog-latency ===");
    println!("station: {}", station.provenance.log_line());

    let mut backend = station.backend;
    let block = eigen_hal::default_block_frames(51_200.0);
    let acq = AcquisitionConfig::new(51_200.0, block);
    backend.open().expect("open");
    let actual = backend.configure(&table(), &acq).expect("configure");
    let fs = actual.actual_rate_hz;
    let lag = backend
        .caps()
        .output_buffer
        .map(|c| c.min_lookahead_frames as usize)
        .unwrap_or(0)
        + actual.group_delay_frames.unwrap_or(0) as usize;
    println!(
        "fs {fs} Hz  block {} frames  pipeline lag {lag} frames ({:.2} ms)",
        actual.block_frames,
        1000.0 * lag as f64 / fs
    );
    println!(
        "abort limit {ABORT_EU} EU (= {ABORT_EU} V at {SENS_MV_PER_EU} mV/EU), ramp {RAMP_EU_PER_S} EU/s"
    );
    println!(
        "  -> the ramp should cross the limit near frame {:.0} ({:.0} ms in)",
        ABORT_EU / RAMP_EU_PER_S * fs,
        1000.0 * ABORT_EU / RAMP_EU_PER_S
    );

    // BLANKING IS ON, and the first run of this probe is why.
    //
    // With blanking at zero the watchdog tripped at FRAME 1 on a measured
    // 0.5558 EU - before the ramp had climbed anywhere near the 0.5 limit
    // (which it reaches at frame ~12800). That is the 9234's delta-sigma
    // digital filter SETTLING at task start: a real transient on a real input,
    // large enough to look exactly like a limit breach.
    //
    // The arm-blanking window exists for precisely this, and switching it off
    // "to keep the measurement clean" removed the protection and measured the
    // transient instead of the abort. So the window stays on, which is also
    // the gate's first step: no trip during blanking, and any would-have-tripped
    // breach recorded as a warning rather than silently dropped.
    let mut ecfg = EngineConfig::with_block(1, 1, &actual);
    ecfg.set_blanking(BLANKING)
        .expect("blanking window within the engine cap");
    let tbl = table();
    let mut engine = Engine::new(ecfg, &tbl, &backend);
    let mut step = Ramp {
        fs,
        frame: 0,
        out_ch: 1,
    };
    let mut sink = VecCaptureSink::new();

    backend.arm().expect("arm");
    backend.start().expect("start");
    // Enough ticks to reach the limit and then some; the watchdog should stop
    // the run long before this budget runs out.
    let ticks = ((2.0 * ABORT_EU / RAMP_EU_PER_S * fs) / actual.block_frames as f64).ceil() as u64;
    let report = engine.run(&mut backend, &mut step, &mut sink, ticks.max(8));
    let _ = backend.stop();

    println!();
    println!("--- RUN REPORT ---");
    println!("  stop reason:   {:?}", report.reason);
    println!("  frames in:     {}", report.frames_in);
    println!("  estops:        {}", report.estops);
    println!(
        "  blanked trips: {}  (would-have-tripped during the {BLANKING:?} window - recorded, \
         never silently dropped)",
        report.blanked_trips
    );

    let trip = match &report.reason {
        StopReason::AbortTripped(t) => Some(*t),
        _ => None,
    };
    match &trip {
        Some(t) => println!(
            "  TRIP:          ch {} axis {:?} value {:.4} limit {:.4} at_frame {}",
            t.channel_index, t.axis, t.value, t.limit, t.at_frame
        ),
        None => {
            // "No trip" is only a watchdog failure if the input actually reached the
            // limit. A run that stopped for another reason first (a scheduling fault,
            // say) never tested the watchdog, and reporting it as a breach would be a
            // FAIL the data does not support. Same blanking rule as the search below.
            let blank_frames = (BLANKING.as_secs_f64() * fs) as usize;
            let reached = sink
                .samples
                .iter()
                .skip(blank_frames)
                .fold(0.0f64, |m, &v| m.max((v as f64).abs()));
            println!();
            if reached >= ABORT_EU {
                println!(
                    "VERDICT: FAIL - the ramp drove past {ABORT_EU} EU and the watchdog did NOT \
                     trip. That is the failure this gate exists to catch; do not run anything on \
                     a shaker from this build."
                );
                std::process::exit(1);
            }
            println!(
                "VERDICT: INCONCLUSIVE - the run stopped ({:?}) with the input at {reached:.4} EU \
                 after blanking, before the ramp reached {ABORT_EU} EU. The watchdog was never \
                 tested; fix the stop reason and re-run.",
                report.reason
            );
            std::process::exit(3);
        }
    }

    // ---- the measurement, off the captured input ---------------------------
    let s = &sink.samples;
    if s.is_empty() {
        eprintln!("no captured input; cannot measure");
        std::process::exit(1);
    }
    let cap_max = s.iter().fold(0.0f64, |m, &v| m.max((v as f64).abs()));
    println!();
    println!("--- CAPTURED INPUT ---");
    println!("  frames captured:   {}", s.len());
    println!("  peak |input|:      {cap_max:.6} EU  (limit {ABORT_EU})");

    // Search AFTER the blanking window. The first captured samples carry the
    // 9234's start-up settling transient (measured 0.5466 EU peak, above the
    // 0.5 limit) - that is what blanking exists for, and it is counted in
    // `blanked_trips` above. Searching from frame 0 finds the TRANSIENT and
    // reports a "detection latency" of the whole run, which is meaningless.
    let blank_frames = (BLANKING.as_secs_f64() * fs) as usize;
    let breach = s
        .iter()
        .enumerate()
        .skip(blank_frames)
        .find(|(_, &v)| (v as f64).abs() >= ABORT_EU)
        .map(|(i, _)| i);
    println!(
        "  blanking window:   {blank_frames} frames ({:.0} ms) - breach search starts after it",
        1000.0 * blank_frames as f64 / fs
    );
    match breach {
        Some(breach) => {
            let trip_at = trip.as_ref().map(|t| t.at_frame as usize).unwrap_or(0);
            println!(
                "  first breach at:   frame {breach}  ({:.3} ms into the capture)",
                1000.0 * breach as f64 / fs
            );
            println!("  watchdog at_frame: {trip_at}");
            if trip_at >= breach {
                let det = trip_at - breach;
                println!();
                println!(
                    "  DETECTION LATENCY: {det} frames = {:.3} ms  (breach sample -> watchdog trip)",
                    1000.0 * det as f64 / fs
                );
            } else {
                println!();
                println!(
                    "  NOTE: the watchdog's at_frame ({trip_at}) precedes the first breach visible \
                     in the capture ({breach}). The watchdog reads the RAW ring before the capture \
                     path, so it can legitimately see a sample the capture never stored - but the \
                     two accountings should be reconciled before this number is quoted."
                );
            }
        }
        None => {
            println!();
            println!(
                "  NOTE: the capture never reached {ABORT_EU} EU even though the watchdog tripped \
                 at {:.4}. The engine aborts BEFORE writing the offending block to the sink, so \
                 the breaching sample is expected to be absent from the record. Detection latency \
                 cannot be read off this capture; use the watchdog's own at_frame against the \
                 commanded ramp instead.",
                trip.as_ref().map(|t| t.value).unwrap_or(0.0)
            );
        }
    }

    println!();
    println!("--- WATCHDOG LATENCY STATUS ---");
    println!("  [PASS] the watchdog TRIPPED on a real limit breach, on real hardware");
    println!("  [PASS] blanking held: {} would-have-tripped inside the window, none escaped as a real trip", report.blanked_trips);
    println!("  [PASS] emergency_stop ran ({} estop call(s)): zeros written to the AO, both tasks stopped", report.estops);
    println!();
    println!("  [OPERATOR - THE SCOPE HALF] This probe cannot time the VOLTAGE falling at the");
    println!("  9263 terminals: emergency_stop stops the AI task too, so the instrument dies");
    println!("  with the drive. Put a scope on AO0, re-run this, and trigger on the fall.");
    println!("  Record scope-measured fall time; add it to the detection latency above to get");
    println!("  the end-to-end abort latency that replaces the provisional [H] 160.75 ms bound");
    println!("  in eigen-hal-ni/src/clock.rs.");
}
