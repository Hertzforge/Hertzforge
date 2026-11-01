//! `ni-analog-truth` - arrival gate 4: does a volt on the wire become the RIGHT
//! engineering unit in Eigen?
//!
//! Two halves, run separately because they answer different questions:
//!
//! **`--external`** (the calibration gate). A function generator drives
//! a KNOWN amplitude and frequency into the 9234. Eigen reports EU through the
//! full host-side scaling path (the channel table's `sensitivity_mv_per_eu`).
//! The operator states what the generator is set to; the probe states what
//! Eigen measured and the error between them. This is the only half that can
//! catch a scaling error, because the loopback half uses Eigen's own DAC as its
//! reference and a scale error there would cancel out on both sides.
//!
//! **`--loopback`** (no generator needed). Eigen commands its own tone on the
//! AO at `--volts-pk` (default 1.0 V), measures it back on the jumpered AI, and
//! compares amplitude + frequency.
//! Catches a broken tone path, a wrong rate, a dead channel - not an absolute
//! scale error.
//!
//! Amplitude is estimated by single-bin Goertzel at the stated frequency (the
//! same estimator gate 3 uses) so noise outside the bin does not inflate it, and
//! the RMS is reported alongside so a grossly non-sinusoidal input is visible.
//!
//! ```text
//! # generator set to 1.000 Vpk at 1000 Hz into AI0, sensitivity 100 mV/EU:
//! cargo run --release -p eigen-station --example ni_analog_truth -- \
//!     --external --volts-pk 1.0 --hz 1000 --sens-mv-per-eu 100
//!
//! # no generator (the tone is 1.0 Vpk unless --volts-pk says otherwise):
//! cargo run --release -p eigen-station --example ni_analog_truth -- --loopback
//! cargo run --release -p eigen-station --example ni_analog_truth -- --loopback --volts-pk 0.5
//! ```
//!
//! Bench: for `--external`, the generator goes to the 9234 input and the AO
//! jumper comes OFF that channel (two sources on one input measures neither).
//! IEPE stays OFF - a generator output is not an IEPE sensor and the 9234's
//! 2.1 mA bias would sit on the generator's output stage.

use eigen_hal::{AcquisitionConfig, Backend, InputRead};
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

struct Args {
    external: bool,
    volts_pk: f64,
    hz: f64,
    sens_mv_per_eu: f64,
    seconds: f64,
}

fn parse() -> Args {
    let mut a = Args {
        external: false,
        volts_pk: 1.0,
        hz: 1000.0,
        sens_mv_per_eu: 100.0,
        seconds: 2.0,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--external" => a.external = true,
            "--loopback" => a.external = false,
            "--volts-pk" => {
                i += 1;
                a.volts_pk = argv[i].parse().expect("--volts-pk")
            }
            "--hz" => {
                i += 1;
                a.hz = argv[i].parse().expect("--hz")
            }
            "--sens-mv-per-eu" => {
                i += 1;
                a.sens_mv_per_eu = argv[i].parse().expect("--sens-mv-per-eu")
            }
            "--seconds" => {
                i += 1;
                a.seconds = argv[i].parse().expect("--seconds")
            }
            "-h" | "--help" => {
                println!(
                    "usage: ni_analog_truth [--external|--loopback] [--volts-pk V] [--hz F] \
                     [--sens-mv-per-eu S] [--seconds N]\n  --volts-pk: the generator's \
                     stated amplitude (--external) or the commanded tone (--loopback); \
                     default 1.0"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(3);
            }
        }
        i += 1;
    }
    // In loopback the amplitude is DRIVE, so it must be a real positive number
    // before anything is written to the AO. NaN-safe: a NaN is not finite. The
    // external half only compares against it and keeps its own handling.
    if !a.external && (!a.volts_pk.is_finite() || a.volts_pk <= 0.0) {
        eprintln!(
            "--volts-pk must be a finite positive amplitude for --loopback, got {}",
            a.volts_pk
        );
        std::process::exit(3);
    }
    a
}

/// Single-bin DFT at bin `k` of an `n`-point block.
fn goertzel(block: &[f64], k: usize) -> (f64, f64) {
    let n = block.len();
    let w = 2.0 * std::f64::consts::PI * k as f64 / n as f64;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in block {
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 - s2 * w.cos(), s2 * w.sin())
}

fn table(sens_mv_per_eu: f64, with_drive: bool) -> eigen_channel_table::ChannelTable {
    use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.sensitivity_mv_per_eu = sens_mv_per_eu;
    // +/-5 EU x sens: keeps the requested range inside the 9234's +/-5 V for
    // any sensitivity at or above 1000 mV/EU, and coerces up harmlessly below.
    ctrl.range_max = 5.0;
    t.channels.push(ctrl);
    if with_drive {
        let mut drv = Channel::new(2);
        drv.role = ChannelRole::Drive;
        drv.units = Units::VoltageV;
        t.channels.push(drv);
    }
    t
}

fn main() {
    let args = parse();
    let spec = StationSpec {
        sim: SimPlantSpec::default(),
        ni: NiSelection {
            derive_inputs: 1,
            derive_outputs: if args.external { 0 } else { 1 },
            iepe: false,
            restrict_devices: if args.external {
                vec![ai_dev()]
            } else {
                vec![ai_dev(), ao_dev()]
            },
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
        eprintln!("station is DRIVER-SIMULATED - a calibration measured here means nothing");
        std::process::exit(2);
    }

    println!("=== ni-analog-truth ===");
    println!("station: {}", station.provenance.log_line());
    println!(
        "mode: {}",
        if args.external {
            "EXTERNAL (function generator -> 9234; this is the half that can catch a SCALE error)"
        } else {
            "LOOPBACK (Eigen's own tone; catches a broken path, NOT an absolute scale error)"
        }
    );

    let mut backend = station.backend;
    let block = eigen_hal::default_block_frames(51_200.0);
    let acq = AcquisitionConfig::new(51_200.0, block);
    backend.open().expect("open");
    let actual = backend
        .configure(&table(args.sens_mv_per_eu, !args.external), &acq)
        .expect("configure");
    let fs = actual.actual_rate_hz;
    println!(
        "fs {fs} Hz (driver-programmed), sensitivity {} mV/EU",
        args.sens_mv_per_eu
    );
    backend.arm().expect("arm");

    // The analysis window is a whole number of seconds' worth of frames so the
    // stated tone lands on an exact bin (no leakage bias on the amplitude).
    let n = (fs as usize).max(block);
    let k = (args.hz / (fs / n as f64)).round() as usize;
    let bin_hz = k as f64 * fs / n as f64;
    if (bin_hz - args.hz).abs() > 0.51 {
        println!(
            "NOTE: {} Hz is not on a bin of this window; measuring the nearest bin {bin_hz:.3} Hz",
            args.hz
        );
    }

    // For loopback, generate the tone at `--volts-pk` (default 1.0 V, well inside
    // the 9263's range). It used to be fixed at 1.0 V here whatever the flag said,
    // so a bench limited below 1 V could not use this half at all.
    let mut phase = 0.0f64;
    let dphi = 2.0 * std::f64::consts::PI * args.hz / fs;
    let cmd_vpk = args.volts_pk;
    if !args.external {
        let pre = 4096usize;
        let tone: Vec<f32> = (0..pre)
            .map(|_| {
                let s = (cmd_vpk * phase.sin()) as f32;
                phase += dphi;
                s
            })
            .collect();
        let mut off = 0usize;
        while off < tone.len() {
            match backend.write_drive(&tone[off..]) {
                Ok(0) | Err(_) => break,
                Ok(w) => off += w,
            }
        }
    }
    backend.start().expect("start");

    // Drive/acquire for the WHOLE `--seconds` window, not just the two seconds
    // the analysis needs: an external reference meter tee'd onto the same node
    // has to settle and take its own reading while the tone is up. The analysis
    // still uses the second 1 s window; the rest is hold time for the meter.
    let want_frames = (n * 2).max((args.seconds * fs) as usize);
    let mut samples: Vec<f64> = Vec::with_capacity(want_frames);
    let mut buf = vec![0.0f32; block];
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs_f64(args.seconds.max(1.0) + 10.0);
    println!(
        "holding the tone for {:.1} s (reference meter may read now)...",
        want_frames as f64 / fs
    );
    while samples.len() < want_frames && std::time::Instant::now() < deadline {
        if !args.external {
            let tone: Vec<f32> = (0..block)
                .map(|_| {
                    let s = (cmd_vpk * phase.sin()) as f32;
                    phase += dphi;
                    s
                })
                .collect();
            let mut off = 0usize;
            while off < tone.len() {
                match backend.write_drive(&tone[off..]) {
                    Ok(0) | Err(_) => break,
                    Ok(w) => off += w,
                }
            }
        }
        match backend.read_input(&mut buf) {
            Ok(InputRead::Frames(got)) if got > 0 => {
                samples.extend(buf[..got].iter().map(|&v| v as f64));
            }
            Ok(_) => std::thread::sleep(std::time::Duration::from_micros(200)),
            Err(e) => {
                eprintln!("read: {e}");
                break;
            }
        }
    }
    let _ = backend.stop();

    if samples.len() < n * 2 {
        eprintln!("too few samples ({}) for a {n}-point window", samples.len());
        std::process::exit(1);
    }
    // Analyse the SECOND window: the first covers start-up and (on loopback)
    // the AO's prefill silence.
    let win = &samples[n..n * 2];
    let (re, im) = goertzel(win, k);
    let amp_eu = 2.0 * (re * re + im * im).sqrt() / n as f64;
    let rms_eu = (win.iter().map(|x| x * x).sum::<f64>() / win.len() as f64).sqrt();
    // Back out volts through the SAME conversion the host applied.
    let amp_v = amp_eu * args.sens_mv_per_eu / 1000.0;

    println!();
    println!(
        "--- MEASURED (second {:.0} ms window) ---",
        1000.0 * n as f64 / fs
    );
    println!("  tone bin:          {bin_hz:.3} Hz");
    println!("  amplitude:         {amp_eu:.6} EU  = {amp_v:.6} V");
    println!(
        "  broadband RMS:     {rms_eu:.6} EU  (sinusoid: expect amplitude/sqrt(2) = {:.6})",
        amp_eu / std::f64::consts::SQRT_2
    );

    if args.external {
        let expect_eu = args.volts_pk * 1000.0 / args.sens_mv_per_eu;
        let err_pct = if expect_eu.abs() > 0.0 {
            100.0 * (amp_eu - expect_eu) / expect_eu
        } else {
            f64::NAN
        };
        println!();
        println!("--- ANALOG TRUTH (external) ---");
        println!(
            "  generator stated:  {:.6} Vpk  -> expected {expect_eu:.6} EU at {} mV/EU",
            args.volts_pk, args.sens_mv_per_eu
        );
        println!("  Eigen measured:    {amp_eu:.6} EU");
        println!("  SCALING ERROR:     {err_pct:+.3} %");
        println!();
        if err_pct.abs() <= 2.0 {
            println!("VERDICT: PASS - the host-side EU scaling matches the generator within 2%.");
        } else {
            println!(
                "VERDICT: FAIL - {err_pct:+.3}% is outside 2%. Check the table sensitivity, the \
                 generator's own amplitude convention (Vpk vs Vpp vs Vrms - a 2x or 2.83x error \
                 is almost always this), and the cable."
            );
            std::process::exit(1);
        }
    } else {
        let expect_eu = cmd_vpk * 1000.0 / args.sens_mv_per_eu;
        let err_pct = 100.0 * (amp_eu - expect_eu) / expect_eu;
        println!();
        println!("--- ANALOG TRUTH (loopback) ---");
        println!("  commanded:         {cmd_vpk:.6} Vpk -> expected {expect_eu:.6} EU");
        println!("  measured:          {amp_eu:.6} EU   ({err_pct:+.3} %)");
        println!();
        println!(
            "NOTE: this half uses Eigen's own DAC as the reference, so a scale error common to \
             both sides CANNOT be seen here. Run --external for the calibration claim."
        );
        if err_pct.abs() <= 5.0 {
            println!("VERDICT: PASS - the tone path is intact end to end.");
        } else {
            println!(
                "VERDICT: FAIL - the loopback tone does not come back at the commanded level."
            );
            std::process::exit(1);
        }
    }
}
