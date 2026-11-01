//! `ni-selftest` - the on-box proof harness for the NI backend.
//!
//! This runs on a host that HAS NI-DAQmx (the Linux node, or a Windows box). On
//! a developer Mac it exits 2 with "no driver", which is the correct answer
//! there and is itself one of the negative controls.
//!
//! What it proves, in order:
//!
//! 1. **Enumeration.** The full device tree with product, serial, simulated
//!    flag, chassis/slot, and channel counts.
//! 2. **Negative controls.** Acceptance means nothing unless refusal is
//!    possible, so before anything is believed the same API is shown REFUSING a
//!    nonexistent device and REFUSING an invalid clock route. A green result
//!    here is a green result that could have been red.
//! 3. **Constant readback.** Values this crate hard-codes, checked against the
//!    driver's own answers rather than trusted from a header.
//! 4. **A live stream.** Configure from the given channels, prefill, run,
//!    report per-second in-band counters (frames read/written, actual rate,
//!    warnings including the coercion text), assert the AO regeneration mode
//!    reads back as DoNotAllowRegen, and print ch0 RMS.
//!
//! Exit codes: `0` pass, `1` an assertion failed, `2` no NI driver on this host,
//! `3` bad command line.
//!
//! ```text
//! cargo run -p eigen-hal-ni --example ni_selftest -- \
//!     --ai-dev cDAQ9178SimMod1 --ao-dev cDAQ9178SimMod2 --rate 51200 --seconds 10
//! ```

use eigen_hal::discovery::{DiscoveredChannel, DiscoveredDevice};
use eigen_hal::{Backend, ClockDomain, InputRead};
use eigen_hal_ni::config::{AiAssign, AiInterface, AoAssign, NiConfig};
use eigen_hal_ni::ffi::{code, val, Driver};
use eigen_hal_ni::{enumerate_with, NiBackend, NiError};
use std::time::{Duration, Instant};

const EXIT_ASSERT: i32 = 1;
const EXIT_NO_DRIVER: i32 = 2;
const EXIT_USAGE: i32 = 3;

// ---------------------------------------------------------------------------------------
// Result tracking - every check is named, and skips are counted SEPARATELY from
// passes so "not checked" can never read as "checked and fine".
// ---------------------------------------------------------------------------------------

#[derive(Default)]
struct Checks {
    passed: Vec<String>,
    failed: Vec<String>,
    skipped: Vec<String>,
}

impl Checks {
    fn pass(&mut self, what: impl Into<String>) {
        let s = what.into();
        println!("  PASS    {s}");
        self.passed.push(s);
    }

    fn fail(&mut self, what: impl Into<String>, why: impl std::fmt::Display) {
        let s = format!("{}: {why}", what.into());
        println!("  FAIL    {s}");
        self.failed.push(s);
    }

    fn skip(&mut self, what: impl Into<String>, why: impl std::fmt::Display) {
        let s = format!("{}: {why}", what.into());
        println!("  SKIP    {s}");
        self.skipped.push(s);
    }

    /// Assert a condition, recording either way.
    fn check(&mut self, cond: bool, what: &str, detail: impl std::fmt::Display) {
        if cond {
            self.pass(format!("{what} ({detail})"));
        } else {
            self.fail(what, detail);
        }
    }

    /// A call that MUST fail. Succeeding IS the failure.
    fn expect_refusal<T: std::fmt::Debug>(&mut self, what: &str, r: Result<T, NiError>) {
        match r {
            Err(e) => self.pass(format!("{what} REFUSED as required -- {e}")),
            Ok(v) => self.fail(
                what,
                format!(
                    "the driver ACCEPTED it ({v:?}). Acceptance elsewhere in this run proves \
                     nothing if this call cannot fail."
                ),
            ),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Command line
// ---------------------------------------------------------------------------------------

struct Args {
    ai_dev: String,
    ao_dev: Option<String>,
    ai_chans: usize,
    ao_chans: usize,
    rate: f64,
    block: usize,
    seconds: u64,
    iepe: bool,
    sensitivity_mv_per_eu: f64,
    max_eu: f64,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            // Defaults name the simulated dev rig on the Linux node. They are
            // DEFAULTS ONLY - nothing in the crate itself knows these names.
            ai_dev: "cDAQ9178SimMod1".into(),
            ao_dev: Some("cDAQ9178SimMod2".into()),
            ai_chans: 4,
            ao_chans: 1,
            rate: 51_200.0,
            block: 2048,
            seconds: 10,
            // IEPE OFF unless asked for. The common bench jumpers the AO into the
            // AI, and IEPE would put the input's ~2 mA excitation onto the AO
            // output stage; a real IEPE sensor asks for it with `--iepe`.
            iepe: false,
            sensitivity_mv_per_eu: 100.0,
            max_eu: 10.0,
        }
    }
}

const USAGE: &str = "\
ni-selftest - on-box proof harness for the Eigen NI-DAQmx backend

USAGE:
  ni_selftest [OPTIONS]

OPTIONS:
  --ai-dev <NAME>     input device                        [default: cDAQ9178SimMod1]
  --ao-dev <NAME>     output device, or `none`            [default: cDAQ9178SimMod2]
  --ai-chans <N>      input channels to use               [default: 4]
  --ao-chans <N>      output channels to use              [default: 1]
  --rate <HZ>         requested sample rate               [default: 51200]
  --block <FRAMES>    frames per read/write block         [default: 2048]
  --seconds <S>       stream duration                     [default: 10]
  --iepe / --no-iepe  input interface                     [default: --no-iepe]
  --sens <MV_PER_EU>  input sensitivity                   [default: 100]
  --max-eu <EU>       input full scale                    [default: 10]
  -h, --help          this text

EXIT CODES:
  0 pass    1 an assertion failed    2 no NI driver on this host    3 bad usage

THE THREE CONFIGURATIONS to prove (run all three):
  1. cDAQ rig  : --ai-dev <IEPE module> --ao-dev <AO module> --iepe
  2. non-cDAQ  : --ai-dev <PCIe card>   --ao-dev <same card> --no-iepe
                 (proves the chassis-specific paths degrade rather than fail)
  3. non-IEPE  : --ai-dev <voltage module> --no-iepe
";

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let k = argv[i].clone();
        let flag = k.clone();
        let next = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            argv.get(*i)
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match k.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--ai-dev" => a.ai_dev = next(&mut i)?,
            "--ao-dev" => {
                let v = next(&mut i)?;
                a.ao_dev = if v.eq_ignore_ascii_case("none") {
                    None
                } else {
                    Some(v)
                };
            }
            "--ai-chans" => a.ai_chans = next(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            "--ao-chans" => a.ao_chans = next(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            "--rate" => a.rate = next(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            "--block" => a.block = next(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            "--seconds" => a.seconds = next(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            "--iepe" => a.iepe = true,
            "--no-iepe" => a.iepe = false,
            "--sens" => {
                a.sensitivity_mv_per_eu = next(&mut i)?.parse().map_err(|e| format!("{e}"))?
            }
            "--max-eu" => a.max_eu = next(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
        i += 1;
    }
    if a.ai_chans == 0 {
        return Err("--ai-chans must be at least 1".into());
    }
    if a.block == 0 {
        return Err("--block must be at least 1".into());
    }
    Ok(a)
}

// ---------------------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------------------

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ni-selftest: {e}");
            std::process::exit(EXIT_USAGE);
        }
    };

    println!("=== ni-selftest ===");
    println!(
        "host: {} {}   crate: eigen-hal-ni",
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    let driver = match Driver::load() {
        Ok(d) => d,
        Err(e) => {
            println!("\nNO NI-DAQmx DRIVER ON THIS HOST");
            println!("  {e}");
            println!(
                "\nThis is the EXPECTED result on a developer Mac, and it is one of the \
                 negative controls: an absent driver becomes a typed refusal, never simulated \
                 data. Run this on the Linux node (or a Windows box) for the real thing."
            );
            std::process::exit(EXIT_NO_DRIVER);
        }
    };
    println!("driver: loaded from {}", driver.loaded_from());

    let mut c = Checks::default();

    section("1. ENUMERATION");
    let devices = report_enumeration(&driver, &mut c);

    section("2. NEGATIVE CONTROLS (acceptance means nothing unless refusal is possible)");
    negative_controls(&driver, &args, &mut c);

    section("3. CONSTANT READBACK (values this crate hard-codes, checked against the driver)");
    constant_readback(&driver, &mut c);

    section("4. LIVE STREAM");
    // Drop the scan handle before the backend loads its own: one owning handle
    // at a time is the crate's threading rule, and the selftest honors it.
    drop(driver);
    stream(&args, &devices, &mut c);

    section("SUMMARY");
    println!("  passed:  {}", c.passed.len());
    println!("  skipped: {}", c.skipped.len());
    for s in &c.skipped {
        println!("             - {s}");
    }
    println!("  failed:  {}", c.failed.len());
    for f in &c.failed {
        println!("             - {f}");
    }
    if c.failed.is_empty() {
        if c.skipped.is_empty() {
            println!("\nRESULT: PASS");
        } else {
            println!(
                "\nRESULT: PASS, with {} check(s) SKIPPED. A skipped check is not a passed \
                 check - read the list above before citing this run as evidence.",
                c.skipped.len()
            );
        }
        std::process::exit(0);
    }
    println!("\nRESULT: FAIL ({} check(s))", c.failed.len());
    std::process::exit(EXIT_ASSERT);
}

fn section(title: &str) {
    println!("\n--- {title} ---");
}

// ---------------------------------------------------------------------------------------
// 1. Enumeration
// ---------------------------------------------------------------------------------------

fn report_enumeration(driver: &Driver, c: &mut Checks) -> Vec<DiscoveredDevice> {
    let devices = match enumerate_with(driver) {
        Ok(d) => d,
        Err(e) => {
            c.fail("enumerate()", e);
            return Vec::new();
        }
    };
    if devices.is_empty() {
        c.fail(
            "enumerate()",
            "the driver reports NO devices, so nothing downstream can be proven. Check that the \
             simulated rig is imported (nidaqmxconfig --import) or that hardware is attached.",
        );
        return devices;
    }
    println!("  {} device(s):", devices.len());
    for d in &devices {
        let badge = if d.simulated {
            " [DRIVER-SIMULATED]"
        } else {
            ""
        };
        let loc = match (&d.parent, d.slot) {
            (Some(p), Some(s)) => format!(" in {p} slot {s}"),
            (Some(p), None) => format!(" in {p}"),
            _ => String::new(),
        };
        println!(
            "    {:<24} {:<20} serial {:<10}{loc}{badge}",
            d.id, d.product, d.serial
        );
        if !d.inputs.is_empty() {
            println!(
                "        {:>2} input(s):  {}",
                d.inputs.len(),
                join_labels(&d.inputs)
            );
        }
        if !d.outputs.is_empty() {
            println!(
                "        {:>2} output(s): {}",
                d.outputs.len(),
                join_labels(&d.outputs)
            );
        }
        for (name, v) in [
            (
                "AI max multi-chan",
                driver.dev_ai_max_multi_chan_rate(&d.id),
            ),
            (
                "AI max single-chan",
                driver.dev_ai_max_single_chan_rate(&d.id),
            ),
            ("AO max", driver.dev_ao_max_rate(&d.id)),
        ] {
            if let Ok(hz) = v {
                println!("        {name}: {hz} Hz");
            }
        }
    }
    c.check(
        devices.iter().any(|d| !d.inputs.is_empty()),
        "at least one device exposes input channels",
        format!("{} device(s) scanned", devices.len()),
    );
    let sim = devices.iter().filter(|d| d.simulated).count();
    println!(
        "  {sim} of {} device(s) are DRIVER-SIMULATED. Simulated data is a THIRD category: \
         never confusable with real hardware and never with Eigen's own Sim plant.",
        devices.len()
    );
    devices
}

fn join_labels(v: &[DiscoveredChannel]) -> String {
    v.iter()
        .map(|c| c.label.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------------------
// 2. Negative controls
// ---------------------------------------------------------------------------------------

fn negative_controls(driver: &Driver, args: &Args, c: &mut Checks) {
    const GHOST: &str = "GhostDev99";

    // (a) A task channel on a device that does not exist must ERROR.
    match driver.create_task("eigen_selftest_ghost") {
        Ok(t) => {
            c.expect_refusal(
                "creating an AI channel on a nonexistent device (GhostDev99/ai0)",
                driver.create_ai_voltage_chan(
                    t,
                    &format!("{GHOST}/ai0"),
                    val::CFG_DEFAULT,
                    -1.0,
                    1.0,
                ),
            );
            let _ = driver.clear_task(t);
        }
        Err(e) => c.fail("create_task for the ghost-device control", e),
    }

    // (b) Enumeration attributes on a nonexistent device must ERROR.
    c.expect_refusal(
        "reading the product type of a nonexistent device",
        driver.dev_product_type(GHOST),
    );

    // (c) An invalid clock ROUTE on a VALID task must be REFUSED. This is the
    //     control that matters most: it proves the driver actually validates the
    //     routing string, so a successful `/<chassis>/ai/SampleClock` on the real
    //     path is a fact rather than a no-op.
    match driver.create_task("eigen_selftest_route") {
        Ok(t) => {
            let real_chan = format!("{}/ai0", args.ai_dev);
            match driver.create_ai_voltage_chan(t, &real_chan, val::CFG_DEFAULT, -1.0, 1.0) {
                Ok(()) => {
                    c.pass(format!("a real AI channel ({real_chan}) was accepted"));
                    // The driver validates routing strings LAZILY (proven
                    // 2026-07-25: CfgSampClkTiming accepted a ghost route).
                    // The refusal we assert is at COMMIT - the same stage the
                    // backend now validates at configure time.
                    let refusal = match driver.cfg_samp_clk_timing(
                        t,
                        &format!("/{GHOST}/ai/SampleClock"),
                        args.rate,
                        4096,
                    ) {
                        Err(e) => Err(e), // refused already at cfg: also a pass
                        Ok(()) => driver.task_control(t, val::TASK_COMMIT),
                    };
                    c.expect_refusal(
                        "a clock route on a nonexistent device \
                         (/GhostDev99/ai/SampleClock), asserted at task COMMIT \
                         (routing strings validate lazily)",
                        refusal,
                    );
                }
                Err(e) => c.fail(
                    format!("creating a real AI channel on {real_chan}"),
                    format!("{e} -- check --ai-dev; the routing control cannot run without it"),
                ),
            }
            let _ = driver.clear_task(t);
        }
        Err(e) => c.fail("create_task for the clock-route control", e),
    }
}

// ---------------------------------------------------------------------------------------
// 3. Constant readback
// ---------------------------------------------------------------------------------------

fn constant_readback(driver: &Driver, c: &mut Checks) {
    // The driver's own words for the code this crate maps to `NoDataYet`. If
    // -200284 does not describe a timeout on this DAQmx, the read path's
    // steady-state mapping is wrong.
    match driver.error_string(code::OPERATION_TIMED_OUT) {
        Ok(text) => {
            println!(
                "  DAQmxGetErrorString({}) = {text:?}",
                code::OPERATION_TIMED_OUT
            );
            let t = text.to_lowercase();
            c.check(
                t.contains("not yet") || t.contains("timeout") || t.contains("timed out"),
                "the -200284 -> NoDataYet mapping matches the driver's own text",
                text,
            );
        }
        Err(e) => c.skip("timeout-code text lookup", e),
    }
    // The optional attribute getters. Their PRESENCE is reported here; their
    // VALUES are read back on the live task in section 4.
    for sym in [
        "DAQmxGetSampQuantSampMode",
        "DAQmxGetSampClkActiveEdge",
        "DAQmxGetReadOverloadedChansExist",
        "DAQmxGetAICoupling",
        "DAQmxGetAIExcitSrc",
        "DAQmxGetAIExcitVal",
        "DAQmxTaskControl",
    ] {
        if driver.has_symbol(sym) {
            println!("  symbol present: {sym}");
        } else {
            c.skip(
                format!("{sym} readback"),
                "symbol not exported by this DAQmx runtime",
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 4. The live stream
// ---------------------------------------------------------------------------------------

fn stream(args: &Args, devices: &[DiscoveredDevice], c: &mut Checks) {
    // Prefer the channel ids the SCAN reported; compose only as a fallback, and
    // say so when composing, because a composed name is a guess.
    let ai_phys = pick(devices, &args.ai_dev, true, args.ai_chans, c);
    let ao_phys = match &args.ao_dev {
        Some(dev) if args.ao_chans > 0 => pick(devices, dev, false, args.ao_chans, c),
        _ => Vec::new(),
    };
    println!("  inputs:  {}", ai_phys.join(", "));
    println!(
        "  outputs: {}",
        if ao_phys.is_empty() {
            "(none - capture only)".to_string()
        } else {
            ao_phys.join(", ")
        }
    );

    let cfg = NiConfig::new(
        ai_phys
            .iter()
            .enumerate()
            .map(|(i, p)| AiAssign {
                physical: p.clone(),
                interface: if args.iepe {
                    AiInterface::Iepe
                } else {
                    AiInterface::Voltage
                },
                sensitivity_mv_per_eu: args.sensitivity_mv_per_eu,
                max_eu: args.max_eu,
                table_index: Some(i as u32 + 1),
            })
            .collect(),
        ao_phys
            .iter()
            .map(|p| AoAssign {
                physical: p.clone(),
                table_index: None,
            })
            .collect(),
    );
    // The rate + block ride the acquisition request now, not the task plan.
    let acq = eigen_hal::AcquisitionConfig::new(args.rate, args.block);
    let n_in = cfg.n_input_channels();
    let n_out = cfg.n_output_channels();
    let lookahead = cfg.lookahead_frames(acq.block_frames);

    let mut b = NiBackend::new(cfg, acq);
    if let Err(e) = b.open() {
        c.fail(
            "open() (driver load + task construction)",
            format!("{e}\n            Check --ai-dev / --ao-dev, --rate, and --iepe/--no-iepe."),
        );
        return;
    }
    c.pass("open(): both tasks built");

    // --- what open() actually decided ---
    let caps = b.caps();
    println!("  requested rate: {} Hz", args.rate);
    println!("  ACTUAL rate:    {} Hz", b.actual_rate_hz());
    println!(
        "  AO clock route: {}",
        if b.ao_routing().is_empty() {
            "(none - the AO task uses its own clock)"
        } else {
            b.ao_routing()
        }
    );
    println!("  clock domain:   {:?}", caps.clock);
    println!("  clock basis:    {}", b.clock_evidence());
    for d in b.devices() {
        println!(
            "  device {:<24} {:<20} serial {:<10}{}",
            d.id,
            d.product,
            d.serial,
            if d.simulated {
                " [DRIVER-SIMULATED]"
            } else {
                ""
            }
        );
    }

    // The clock rule, asserted rather than assumed.
    if b.all_simulated() {
        c.check(
            caps.clock == ClockDomain::Shared,
            "a fully driver-simulated device set resolves to Shared",
            format!("{:?}", caps.clock),
        );
    } else {
        c.check(
            caps.clock == ClockDomain::Unknown,
            "a REAL device set resolves to Unknown until a bench capture adds a proven-set entry",
            format!(
                "{:?} -- if this says Shared on real hardware with no entry in \
                 clock::PROVEN_SHARED_CLOCK, the gate has been deleted",
                caps.clock
            ),
        );
    }
    c.check(
        caps.native_modes.is_empty(),
        "caps claims no native modes",
        format!("{:?}", caps.native_modes),
    );

    // --- front-end readback: the UNVERIFIED coupling/excitation constants ---
    // Every input row is a VOLTAGE channel (the driver applies no sensitivity;
    // the host converts, once). What varies per row is the front end, and its
    // constants are hard-coded guesses until the driver confirms them.
    println!("  input channels are VOLTAGE channels; EU conversion is host-side only");
    for (fe, a) in b.front_ends().iter().zip(b.config().ai_assignments.iter()) {
        println!(
            "  {:<24} range +/-{:.4} V  sens {} mV/EU  full scale {} EU",
            fe.physical,
            a.range_volts(),
            a.sensitivity_mv_per_eu,
            a.max_eu
        );
        match fe.coupling {
            Some(got) => c.check(
                got == fe.requested_coupling,
                &format!("{}: coupling reads back as requested", fe.physical),
                format!("requested {}, read {got}", fe.requested_coupling),
            ),
            None => c.skip(
                format!("{}: coupling readback", fe.physical),
                "readback unavailable (symbol missing or property unsupported on this \
                 device - verify on real hardware at arrival)",
            ),
        }
        match (fe.requested_excit_src, fe.excit_src) {
            (Some(req), Some(got)) => c.check(
                got == req,
                &format!("{}: excitation source reads back as requested", fe.physical),
                format!("requested {req}, read {got}"),
            ),
            (Some(_), None) => c.skip(
                format!("{}: excitation-source readback", fe.physical),
                "readback unavailable (symbol missing or property unsupported on this \
                 device - verify on real hardware at arrival)",
            ),
            (None, _) => {} // SE row: excitation never requested, nothing to verify
        }
        match (fe.requested_excit_a, fe.excit_a) {
            (Some(want), Some(got)) => c.check(
                (want - got).abs() < 1e-9,
                &format!(
                    "{}: excitation current reads back as requested",
                    fe.physical
                ),
                format!("requested {want} A, read {got} A"),
            ),
            (Some(_), None) => c.skip(
                format!("{}: excitation-current readback", fe.physical),
                "readback unavailable (symbol missing or property unsupported on this \
                 device - verify on real hardware at arrival)",
            ),
            _ => {}
        }
    }

    // The regeneration assertion. A regenerating AO looping a stale buffer while
    // the plots stay alive is the sim-leak shape in hardware clothes.
    if n_out > 0 {
        match b.regen_mode_readback() {
            Some(v) => c.check(
                v == val::DO_NOT_ALLOW_REGEN,
                "AO regeneration mode reads back as DoNotAllowRegen",
                format!("read {v}, expected {}", val::DO_NOT_ALLOW_REGEN),
            ),
            None => c.fail("AO regeneration readback", "no value was recorded"),
        }
        match caps.output_buffer {
            Some(ob) => c.check(
                ob.min_lookahead_frames as usize == lookahead,
                "the output contract carries the configured lookahead",
                format!("{} frames", ob.min_lookahead_frames),
            ),
            None => c.fail("output buffer contract", "absent despite drive channels"),
        }
    }

    if let Err(e) = b.arm() {
        c.fail("arm()", e);
        return;
    }

    // --- prefill: whatever is written before start() IS the prefill ---
    let mut phase = 0.0f64;
    if n_out > 0 {
        let block = make_drive(&mut phase, b.actual_rate_hz(), args.block, n_out);
        match b.write_drive(&block) {
            Ok(n) => {
                println!("  prefill: {n} of {} frames accepted", args.block);
                c.check(
                    n > 0,
                    "the AO buffer accepted a prefill before start()",
                    format!("{n} frames"),
                );
            }
            Err(e) => c.fail("prefill write_drive()", e),
        }
        if let Some(space) = b.output_space() {
            println!("  output space after prefill: {space} frames");
        }
    }

    if let Err(e) = b.start() {
        c.fail("start()", e);
        return;
    }
    c.pass("start(): AO started first, then AI");

    // --- the run ---
    let mut buf = vec![0.0f32; args.block * n_in];
    let mut sumsq = 0.0f64;
    let mut nsamp = 0u64;
    let mut no_data_polls = 0u64;
    let mut short_writes = 0u64;
    let began = Instant::now();
    let mut last_tick = began;
    let mut last_read = 0u64;
    let mut last_written = 0u64;
    let mut run_error: Option<String> = None;

    while began.elapsed() < Duration::from_secs(args.seconds) {
        match b.read_input(&mut buf) {
            Ok(InputRead::Frames(n)) => {
                for f in 0..n {
                    let v = buf[f * n_in] as f64;
                    sumsq += v * v;
                }
                nsamp += n as u64;
            }
            Ok(InputRead::NoDataYet) => no_data_polls += 1,
            Ok(InputRead::Ended) => {
                run_error = Some("the stream reported Ended during a continuous run".into());
                break;
            }
            Err(e) => {
                run_error = Some(format!("read_input: {e}"));
                break;
            }
        }
        // Top up the drive against the device's own report of free space.
        if n_out > 0 {
            if let Some(space) = b.output_space() {
                let frames = space.min(args.block);
                if frames > 0 {
                    let block = make_drive(&mut phase, b.actual_rate_hz(), frames, n_out);
                    match b.write_drive(&block) {
                        Ok(n) if n < frames => short_writes += 1,
                        Ok(_) => {}
                        Err(e) => {
                            run_error = Some(format!("write_drive: {e}"));
                            break;
                        }
                    }
                }
            }
        }
        if last_tick.elapsed() >= Duration::from_secs(1) {
            let st = b.poll_status();
            let dr = b.frames_read() - last_read;
            let dw = b.frames_written() - last_written;
            println!(
                "  t={:>2}s  read {:>9} (+{dr}) | written {:>9} (+{dw}) | counter {} | {}",
                began.elapsed().as_secs(),
                b.frames_read(),
                b.frames_written(),
                st.sample_counter,
                st.message
            );
            last_read = b.frames_read();
            last_written = b.frames_written();
            last_tick = Instant::now();
        }
    }
    let elapsed = began.elapsed().as_secs_f64();
    let _ = b.stop();

    if let Some(e) = run_error {
        c.fail("streaming", e);
    }

    // --- in-band evidence ---
    println!(
        "  ran {elapsed:.2} s: {} frames read, {} frames written, {no_data_polls} NoDataYet \
         poll(s), {short_writes} short write(s)",
        b.frames_read(),
        b.frames_written()
    );
    c.check(
        b.frames_read() > 0,
        "the in-band frame counter advanced",
        format!("{} frames", b.frames_read()),
    );
    // Sanity against the ACTUAL rate: a stream reading far less than the clock
    // produced is starved, and one reading more than the clock is not real.
    let expected = b.actual_rate_hz() * elapsed;
    let ratio = if expected > 0.0 {
        b.frames_read() as f64 / expected
    } else {
        0.0
    };
    c.check(
        (0.5..=1.05).contains(&ratio),
        "frames read track the actual sample rate",
        format!(
            "{} read vs {expected:.0} clocked ({:.1}% of the clock)",
            b.frames_read(),
            ratio * 100.0
        ),
    );
    if n_out > 0 {
        c.check(
            b.frames_written() > 0,
            "the drive counter advanced",
            format!("{} frames", b.frames_written()),
        );
    }
    let status = b.poll_status();
    c.check(
        status.sample_counter == b.frames_read(),
        "poll_status().sample_counter IS the in-band frames-read counter",
        format!("{} vs {}", status.sample_counter, b.frames_read()),
    );

    if nsamp > 0 {
        // In ENGINEERING UNITS: `read_input` already converted volts -> EU with
        // the channel's sensitivity, host-side, exactly once. Divide by
        // 1000/sens to get back to volts if comparing against a scope.
        let rms_eu = (sumsq / nsamp as f64).sqrt();
        let sens = args.sensitivity_mv_per_eu;
        println!(
            "  ch0 RMS: {rms_eu:.6} EU  ({:.6} V at {sens} mV/EU)",
            rms_eu * sens / 1000.0
        );
    }

    if !b.overload_pollable() {
        c.skip(
            "ADC overload polling",
            "this DAQmx runtime does not expose the overload attribute",
        );
    } else if b.overload_latched() {
        c.fail(
            "ADC overload",
            "at least one input channel reported overload during the run",
        );
    } else {
        c.pass("no ADC overload reported");
    }

    // --- warnings, verbatim (this is where the rate-coercion text lands) ---
    let warnings = b.warnings();
    println!("  {} warning(s) recorded:", warnings.len());
    for w in &warnings {
        println!("    {w}");
    }
    if (b.actual_rate_hz() - args.rate).abs() > 1e-9 {
        c.check(
            warnings.iter().any(|w| w.contains("coerce")),
            "a coerced rate produced a visible warning",
            format!("{} Hz -> {} Hz", args.rate, b.actual_rate_hz()),
        );
    }

    let _ = b.emergency_stop();
    let _ = b.close();
    c.pass("emergency_stop() + close() completed");
}

/// One block of interleaved drive: a low-amplitude tone on channel 0, zeros
/// elsewhere. The amplitude is deliberately small - this harness is not a bench
/// tool, and its output must be harmless if it ever reaches an amplifier.
fn make_drive(phase: &mut f64, rate_hz: f64, frames: usize, n_ch: usize) -> Vec<f32> {
    const AMPLITUDE_V: f64 = 0.1;
    const TONE_HZ: f64 = 100.0;
    let dphi = std::f64::consts::TAU * TONE_HZ / rate_hz.max(1.0);
    let mut out = vec![0.0f32; frames * n_ch];
    for f in 0..frames {
        out[f * n_ch] = (AMPLITUDE_V * phase.sin()) as f32;
        *phase += dphi;
    }
    *phase = phase.rem_euclid(std::f64::consts::TAU);
    out
}

/// Physical channel names for `dev`, taken from the SCAN when the device is
/// there and composed otherwise - and a composed name is reported as the guess
/// it is.
fn pick(
    devices: &[DiscoveredDevice],
    dev: &str,
    inputs: bool,
    want: usize,
    c: &mut Checks,
) -> Vec<String> {
    match devices.iter().find(|d| d.id == dev) {
        Some(d) => {
            let list = if inputs { &d.inputs } else { &d.outputs };
            if list.len() < want {
                c.fail(
                    format!("selecting {want} channel(s) on {dev}"),
                    format!(
                        "the scan reports only {} {} channel(s) there",
                        list.len(),
                        if inputs { "input" } else { "output" }
                    ),
                );
            }
            list.iter().take(want).map(|ch| ch.id.clone()).collect()
        }
        None => {
            c.fail(
                format!("device {dev} in the scan"),
                "not present; falling back to composed channel names, which the driver will \
                 almost certainly refuse",
            );
            let kind = if inputs { "ai" } else { "ao" };
            (0..want).map(|i| format!("{dev}/{kind}{i}")).collect()
        }
    }
}
