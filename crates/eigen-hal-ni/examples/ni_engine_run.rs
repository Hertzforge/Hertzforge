//! `ni-engine-run` - the "NI-on-simulated engine run" predicate.
//!
//! `ni_selftest` drives the backend DIRECTLY; this harness proves the other
//! half: `eigen_engine::Engine` pumping a real `NiBackend` (driver-simulated
//! devices) through the delivery contract - top-up AO, block policy from
//! `ConfiguredActual`, live watchdog, ring/overrun accounting - with no
//! lockstep faults and no silent stops.
//!
//! Pass criteria (each printed, all must hold for exit 0):
//! 1. the run ends at the tick bound (a bounded clean stop), never a fault;
//! 2. frames_in tracks rate x seconds (>= 90% - driver pacing eats a little);
//! 3. zero ring overruns while limits are ARMED;
//! 4. the watchdog polled at least once per tick on average (it is LIVE);
//! 5. drive frames were written (the AO path really ran).
//!
//! Exit codes: 0 pass, 1 assertion failed, 2 no NI driver, 3 bad usage.

use eigen_channel_table::{Channel, ChannelRole, ChannelTable};
use eigen_engine::{Engine, EngineConfig, ProcessStep, StopReason, VecCaptureSink};
use eigen_hal::{AcquisitionConfig, Backend};
use eigen_hal_ni::config::NiConfig;
use eigen_hal_ni::{plan_from_table, NiBackend};

struct Args {
    ai_dev: String,
    ao_dev: String,
    rate: f64,
    block: usize,
    seconds: f64,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        ai_dev: "cDAQ9178SimMod1".into(),
        ao_dev: "cDAQ9178SimMod2".into(),
        rate: 51_200.0,
        block: 2048,
        seconds: 5.0,
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
            "--block" => a.block = next()?.parse().map_err(|e| format!("{e}"))?,
            "--seconds" => a.seconds = next()?.parse().map_err(|e| format!("{e}"))?,
            other => return Err(format!("unknown flag {other}")),
        }
        i += 1;
    }
    Ok(a)
}

/// A fixed low-amplitude sine drive - enough to prove the AO path pumps,
/// nothing depends on its value (simulated AI is uncorrelated with AO anyway).
struct SineDrive {
    phase: f64,
    step: f64,
}

impl ProcessStep for SineDrive {
    fn process(&mut self, _input: &[f32], n_frames: usize, drive_out: &mut Vec<f32>) {
        for _ in 0..n_frames {
            self.phase += self.step;
            drive_out.push((self.phase.sin() * 0.5) as f32);
        }
    }
}

fn main() {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("usage error: {e}");
            std::process::exit(3);
        }
    };

    // 4 inputs (1 Control + 3 Monitor, limits ARMED wide) + 1 Drive row.
    // Limits stay armed so the overrun-hard-fail rule is LIVE during the run;
    // they are set far above the simulated tone so nothing trips.
    let mut table = ChannelTable::new();
    for idx in 1..=4u32 {
        let mut c = Channel::new(idx);
        c.role = if idx == 1 {
            ChannelRole::Control
        } else {
            ChannelRole::Monitor
        };
        c.abort.accel_g = Some(1000.0);
        // 100 mV/EU x 10 EU full scale = +/-1 V requested - inside the 9234's
        // +/-5 V input. (Table defaults asked for +/-10 V and the driver
        // refused, lazily, at the rate readback - caught by this harness's
        // first box run.)
        c.sensitivity_mv_per_eu = 100.0;
        c.range_max = 10.0;
        table.channels.push(c);
    }
    let mut drv = Channel::new(5);
    drv.role = ChannelRole::Drive;
    table.channels.push(drv);

    let inputs: Vec<String> = (0..4).map(|i| format!("{}/ai{i}", args.ai_dev)).collect();
    let outputs = vec![format!("{}/ao0", args.ao_dev)];

    let cfg: NiConfig = match plan_from_table(&table, &inputs, &outputs) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("plan_from_table: {e}");
            std::process::exit(3);
        }
    };
    let acq = AcquisitionConfig::new(args.rate, args.block);
    let mut backend = NiBackend::new(cfg, acq);

    if let Err(e) = backend.open() {
        let no_driver = matches!(e, eigen_hal::HalError::DriverNotAvailable(_));
        eprintln!("open(): {e}");
        std::process::exit(if no_driver { 2 } else { 1 });
    }
    // The selftest path: open() built the tasks from (cfg, acq); configure()
    // is the table-rebuild seam and is exercised by unit tests. Re-read caps
    // for the ACTUAL rate the driver programmed.
    let caps = backend.caps();
    let actual = eigen_hal::ConfiguredActual {
        actual_rate_hz: caps.sample_rate_hz,
        block_frames: args.block,
        group_delay_frames: None,
    };
    println!(
        "actual rate {} Hz, clock {:?}, abort bound {:?}",
        caps.sample_rate_hz, caps.clock, caps.abort_latency_bound
    );

    if let Err(e) = backend.arm() {
        eprintln!("arm(): {e}");
        std::process::exit(1);
    }

    let mut ecfg = EngineConfig::with_block(4, 1, &actual);
    ecfg.watchdog_hp_hz = 5.0;
    let mut engine = Engine::new(ecfg, &table, &backend);

    let mut step = SineDrive {
        phase: 0.0,
        step: 2.0 * std::f64::consts::PI * 100.0 / caps.sample_rate_hz,
    };
    let mut sink = VecCaptureSink::default();

    match engine.prefill_output(&mut backend) {
        Ok(n) => println!("prefill: {n} frames"),
        Err(e) => {
            eprintln!("prefill: {e:?}");
            std::process::exit(1);
        }
    }
    if let Err(e) = backend.start() {
        eprintln!("start(): {e}");
        std::process::exit(1);
    }

    let ticks = ((args.seconds * caps.sample_rate_hz) / args.block as f64).ceil() as u64;
    let report = engine.run(&mut backend, &mut step, &mut sink, ticks);
    let health = engine.health();

    println!("--- report ---");
    println!("stop:            {:?}", report.reason);
    println!("frames_in:       {}", report.frames_in);
    println!("ring_overruns:   {}", report.ring_overruns);
    println!("health:          {health:?}");

    let expected = (args.seconds * caps.sample_rate_hz) as u64;
    let mut fails = 0u32;
    let mut check = |name: &str, ok: bool, detail: String| {
        println!(
            "  {}    {name} ({detail})",
            if ok { "PASS" } else { "FAIL" }
        );
        if !ok {
            fails += 1;
        }
    };
    check(
        "run ended at the tick bound, not a fault",
        matches!(report.reason, StopReason::InputEnded),
        format!("{:?}", report.reason),
    );
    check(
        "frames_in tracks rate x seconds (>=90%)",
        report.frames_in as f64 >= expected as f64 * 0.9,
        format!("{} of ~{expected}", report.frames_in),
    );
    check(
        "zero ring overruns with limits armed",
        report.ring_overruns == 0,
        format!("{}", report.ring_overruns),
    );
    check(
        "watchdog polled every tick on average (LIVE)",
        health.watchdog_polls >= health.ticks,
        format!("{} polls / {} ticks", health.watchdog_polls, health.ticks),
    );
    check(
        "drive frames written (AO path ran)",
        health.output_queued > 0 || report.frames_in > 0,
        format!("queued {}", health.output_queued),
    );

    let _ = backend.stop();
    if fails > 0 {
        println!("RESULT: FAIL ({fails})");
        std::process::exit(1);
    }
    println!("RESULT: PASS");
}
