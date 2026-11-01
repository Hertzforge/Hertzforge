//! `clock-loopback` - arrival-day gate 3: is the AI/AO clock genuinely SHARED?
//!
//! The measurement `AI_AO_Clock_Domain_Findings_2026-07-16.md` section 4 specifies,
//! as a tool: generate a long tone on the AO, acquire it on the AI **for minutes,
//! not seconds**, and watch the ACCUMULATED PHASE between what was commanded and
//! what was measured.
//!
//! - Phase drift FLAT (bounded, slope ~0) -> the clocks are genuinely locked:
//!   `ClockDomain::Shared` is earned, and the proven-set entry in
//!   `eigen-hal-ni/src/clock.rs` may cite this run's CSV.
//! - Phase drift ACCUMULATING monotonically -> `Independent`. Random/Sine stay
//!   refused on this pair; the NI-9260 path activates.
//!
//! **Run it BOTH ways** (the negative control the findings doc demands):
//!
//! ```text
//! # routed: AO rides /<chassis>/ai/SampleClock (the shared-clock claim)
//! cargo run -p eigen-hal-ni --example clock_loopback -- \
//!     --ai-dev cDAQ1Mod1 --ao-dev cDAQ1Mod2 --minutes 5 --csv routed.csv
//!
//! # unrouted: AO on its own clock (MUST show drift, or the instrument can't see)
//! cargo run -p eigen-hal-ni --example clock_loopback -- \
//!     --ai-dev cDAQ1Mod1 --ao-dev cDAQ1Mod2 --minutes 5 --unrouted --csv unrouted.csv
//! ```
//!
//! Wiring: AO0 -> AI0 direct, **IEPE OFF on the looped channel** (never drive
//! excitation current into an AO). No amplifier, no shaker.
//!
//! The per-block phase estimate is a single-bin DFT (Goertzel) at the commanded
//! tone frequency; the tone is placed on an exact bin of the block so leakage
//! does not bias the phase. Drift is the slope of a least-squares line through
//! the unwrapped phase-difference series; the verdict thresholds are stated on
//! the output rows. The CONSTANT part of the phase offset is the group-delay
//! estimate, reported modulo one tone period (run twice at two frequencies to
//! disambiguate).
//!
//! On the DRIVER-SIMULATED rig this tool runs but its verdict is MEANINGLESS
//! (simulated AI is uncorrelated with AO - the tone will simply be absent, and
//! the tool says so honestly instead of printing a fake verdict). Exit codes:
//! 0 = measured with a verdict, 1 = measurement failed, 2 = no driver,
//! 3 = usage.

use eigen_hal::{AcquisitionConfig, Backend, InputRead};
use eigen_hal_ni::config::NiConfig;
use eigen_hal_ni::{plan_from_table, NiBackend};
use std::f64::consts::PI;
use std::io::Write;

struct Args {
    ai_dev: String,
    ao_dev: String,
    rate: f64,
    tone_hz: f64,
    minutes: f64,
    unrouted: bool,
    csv: Option<String>,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        ai_dev: "cDAQ9178SimMod1".into(),
        ao_dev: "cDAQ9178SimMod2".into(),
        rate: 51_200.0,
        tone_hz: 1_000.0,
        minutes: 5.0,
        unrouted: false,
        csv: None,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let flag = argv[i].clone();
        let mut next = || -> Result<String, String> {
            i += 1;
            argv.get(i).cloned().ok_or(format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--ai-dev" => a.ai_dev = next()?,
            "--ao-dev" => a.ao_dev = next()?,
            "--rate" => a.rate = next()?.parse().map_err(|e| format!("{e}"))?,
            "--tone-hz" => a.tone_hz = next()?.parse().map_err(|e| format!("{e}"))?,
            "--minutes" => a.minutes = next()?.parse().map_err(|e| format!("{e}"))?,
            "--unrouted" => a.unrouted = true,
            "--csv" => a.csv = Some(next()?),
            other => return Err(format!("unknown flag {other}")),
        }
        i += 1;
    }
    Ok(a)
}

/// Single-bin DFT (Goertzel) at bin `k` of an `n`-point block: returns (re, im).
fn goertzel(block: &[f64], k: usize) -> (f64, f64) {
    let n = block.len();
    let w = 2.0 * PI * k as f64 / n as f64;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in block {
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let re = s1 - s2 * w.cos();
    let im = s2 * w.sin();
    (re, im)
}

fn main() {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("usage: {e}");
            std::process::exit(3);
        }
    };

    // One looped input row + one drive row. A fresh row is SingleEnded by
    // default - IEPE stays OFF on the looped channel (wiring rule: never drive
    // excitation current into an AO). 100 mV/EU x +/-10 EU = +/-1 V requested,
    // legal on every AI family (fact proven three times over in this buildout).
    let mut table = eigen_channel_table::ChannelTable::new();
    let mut ch = eigen_channel_table::Channel::new(1);
    ch.role = eigen_channel_table::ChannelRole::Control;
    ch.sensitivity_mv_per_eu = 100.0;
    ch.range_max = 10.0;
    table.channels.push(ch);
    let mut drive = eigen_channel_table::Channel::new(2);
    drive.role = eigen_channel_table::ChannelRole::Drive;
    table.channels.push(drive);

    let inputs = vec![format!("{}/ai0", args.ai_dev)];
    let outputs = vec![format!("{}/ao0", args.ao_dev)];
    let mut cfg: NiConfig = match plan_from_table(&table, &inputs, &outputs) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("plan: {e}");
            std::process::exit(3);
        }
    };
    if args.unrouted {
        // The negative control: AO on its own clock. Without this run a slow
        // drift can read as "locked"; the unrouted run must show drift or the
        // instrument cannot see drift at all.
        cfg.force_unrouted_ao = true;
    }

    // Block = 1 second: tone-on-a-bin means tone_hz must be an integer (it is
    // the bin index), giving an unbiased phase per block and 1 Hz resolution.
    let block = args.rate as usize;
    let k = args.tone_hz.round() as usize;
    let acq = AcquisitionConfig::new(args.rate, block);
    let mut backend = NiBackend::new(cfg, acq);

    if let Err(e) = backend.open() {
        let no_driver = matches!(e, eigen_hal::HalError::DriverNotAvailable(_))
            || e.to_string().contains("driver");
        eprintln!("open: {e}");
        std::process::exit(if no_driver { 2 } else { 1 });
    }
    let caps = backend.caps();
    let fs = caps.sample_rate_hz;
    println!("=== clock-loopback ===");
    println!(
        "ai {} / ao {}  fs {} Hz  tone {} Hz  window {} min  mode {}",
        args.ai_dev,
        args.ao_dev,
        fs,
        args.tone_hz,
        args.minutes,
        if args.unrouted {
            "UNROUTED (negative control)"
        } else {
            "routed"
        }
    );
    println!(
        "clock caps say: {:?} ({})",
        caps.clock,
        backend.clock_evidence()
    );
    if backend.all_simulated() {
        println!(
            "NOTE: every device is DRIVER-SIMULATED - simulated AI is uncorrelated \
             with AO, so this run CANNOT produce a clock verdict. It exercises the \
             tool only; the real verdict needs the physical chassis."
        );
    }

    if let Err(e) = backend.arm() {
        eprintln!("arm: {e}");
        std::process::exit(1);
    }

    // Prefill + start: the AO plays the tone continuously (non-regenerating,
    // topped up each block from the same phase-continuous generator).
    let mut phase_gen = 0.0f64;
    let dphi = 2.0 * PI * args.tone_hz / fs;
    let mut make_block = |n: usize| -> Vec<f32> {
        (0..n)
            .map(|_| {
                let s = (phase_gen.sin() * 0.5) as f32;
                phase_gen += dphi;
                if phase_gen > 2.0 * PI {
                    phase_gen -= 2.0 * PI;
                }
                s
            })
            .collect()
    };
    let pre = make_block(block * 2);
    let mut off = 0usize;
    while off < pre.len() {
        match backend.write_drive(&pre[off..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => off += n,
        }
    }
    if let Err(e) = backend.start() {
        eprintln!("start: {e}");
        std::process::exit(1);
    }

    let blocks_total = (args.minutes * 60.0).ceil() as usize;
    let mut buf = vec![0.0f32; block];
    let mut series: Vec<(f64, f64)> = Vec::with_capacity(blocks_total); // (t_s, phase_diff_rad unwrapped)
    let mut prev_phase = 0.0f64;
    let mut unwrapped = 0.0f64;
    let mut got_any_tone = false;

    for bi in 0..blocks_total {
        // Keep the AO fed one block ahead.
        let tone = make_block(block);
        let mut o = 0usize;
        while o < tone.len() {
            match backend.write_drive(&tone[o..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => o += n,
            }
        }
        // Read one full block (fixed-N; NoDataYet retries).
        let mut have = 0usize;
        while have < block {
            match backend.read_input(&mut buf[have..]) {
                Ok(InputRead::Frames(n)) if n > 0 => have += n,
                Ok(InputRead::Frames(_)) | Ok(InputRead::NoDataYet) => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Ok(InputRead::Ended) => {
                    eprintln!("input ended mid-measurement");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("read: {e}");
                    std::process::exit(1);
                }
            }
        }
        let b64: Vec<f64> = buf.iter().map(|v| *v as f64).collect();
        let (re, im) = goertzel(&b64, k);
        let mag = (re * re + im * im).sqrt() / (block as f64 / 2.0);
        if mag < 1e-3 {
            // No tone present (open loop, wrong wiring, or the simulated rig).
            if bi == 2 && !got_any_tone {
                println!(
                    "NO TONE at {} Hz on the AI (mag {:.2e}) - check the loopback wiring; \
                     on the simulated rig this is the expected honest outcome.",
                    args.tone_hz, mag
                );
            }
        } else {
            got_any_tone = true;
        }
        let phase = im.atan2(re);
        if bi > 0 {
            let mut d = phase - prev_phase;
            while d > PI {
                d -= 2.0 * PI;
            }
            while d < -PI {
                d += 2.0 * PI;
            }
            unwrapped += d;
        }
        prev_phase = phase;
        series.push((bi as f64, unwrapped));
        if bi % 30 == 0 {
            println!(
                "t={:>5.0}s  phase_acc {:+9.4} rad  ({:+8.3} us equivalent)",
                bi as f64,
                unwrapped,
                unwrapped / (2.0 * PI * args.tone_hz) * 1e6
            );
        }
    }
    let _ = backend.stop();

    if let Some(path) = &args.csv {
        let mut f = std::fs::File::create(path).expect("csv create");
        writeln!(f, "t_s,phase_acc_rad").unwrap();
        for (t, p) in &series {
            writeln!(f, "{t},{p}").unwrap();
        }
        println!("series written: {path}");
    }

    if !got_any_tone {
        println!("VERDICT: NO MEASUREMENT - the tone never appeared on the AI.");
        println!("(Simulated rig, or wiring. A clock verdict needs the physical loopback.)");
        std::process::exit(1);
    }

    // Least-squares slope of unwrapped phase vs time = drift rate. The first
    // blocks are EXCLUDED from the fit: the AO start transient settles inside
    // the first few seconds (observed: complete by block 5 on the real 9234/
    // 9263 pair), and a settle step inside a short fit reads as drift - a
    // 60-block run measured "-4.3 ppm" from a series that was dead flat after
    // block 5 (2026-07-30). Ten blocks is settle x2 headroom; the verdict
    // window remains minutes long per the findings doc.
    const SETTLE_BLOCKS: usize = 10;
    let series: Vec<(f64, f64)> = if series.len() > 3 * SETTLE_BLOCKS {
        series[SETTLE_BLOCKS..].to_vec()
    } else {
        println!(
            "WARNING: window too short to exclude the settle transient - the              slope may read settle as drift; use --minutes 3 or more."
        );
        series
    };
    let n = series.len() as f64;
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
    for (t, p) in &series {
        sx += t;
        sy += p;
        sxx += t * t;
        sxy += t * p;
    }
    let slope = (n * sxy - sx * sy) / (n * sxx - sx * sx); // rad/block (1 s blocks)
    let intercept = (sy - slope * sx) / n; // rad - the constant part of the offset
    let ppm = slope / (2.0 * PI * args.tone_hz) * 1e6; // us/s = ppm of time
    let const_delay_us = intercept / (2.0 * PI * args.tone_hz) * 1e6;

    println!("--- verdict ---");
    println!("drift slope: {slope:+.3e} rad/s = {ppm:+.4} ppm");
    println!(
        "constant phase offset (group-delay estimate, MODULO one tone period \
         {:.1} us - run at a second tone frequency to disambiguate): {:+.3} us",
        1e6 / args.tone_hz,
        const_delay_us
    );
    // Threshold: 0.05 ppm over a multi-minute window is far below any real
    // independent-oscillator pair (typically 1-50 ppm) and above the estimator
    // noise floor at these block counts.
    let shared = ppm.abs() < 0.05;
    if args.unrouted {
        println!(
            "NEGATIVE CONTROL: drift {} - {}",
            if shared { "NOT seen" } else { "seen" },
            if shared {
                "the instrument could not demonstrate drift on independent clocks; \
                 the routed run's 'flat' proves NOTHING until this control shows drift"
            } else {
                "the instrument can see drift; a flat routed run is now meaningful"
            }
        );
        std::process::exit(0);
    }
    println!(
        "CLOCK VERDICT: {}",
        if shared {
            "FLAT within 0.05 ppm -> genuinely SHARED (cite this CSV in the proven-set entry)"
        } else {
            "ACCUMULATING -> INDEPENDENT (honest declaration; Random/Sine stay refused; \
             the NI-9260 path activates)"
        }
    );
}
