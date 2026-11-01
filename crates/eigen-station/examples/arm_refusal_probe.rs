//! `arm-refusal-probe` - arrival-day gate 0: REFUSAL BEFORE PROOF.
//!
//! On the REAL chassis, before any per-configuration proven-set entry exists,
//! a closed-loop arm request (what Random/Sine make) must be REFUSED with the
//! typed `ClockNotShared`-class reason - the resolver has no evidence the AI
//! and AO clocks are locked, and it must say so rather than arm.
//!
//! **A run that ARMS here is a CRITICAL failure** (exit 2): stop, diagnose the
//! caps path, record the defect. Exit 0 = the refusal happened and printed verbatim
//! (record it in the gate row). Exit 1 = could not reach the point of asking
//! (driver/config error - fix and re-run).
//!
//! ```text
//! cargo run -p eigen-station --example arm_refusal_probe -- \
//!     --ai-dev cDAQ1Mod1 --ao-dev cDAQ1Mod2
//! ```
//!
//! The devices default to `cDAQ1Mod1` / `cDAQ1Mod2`, or to `EIGEN_NI_AI_DEV` /
//! `EIGEN_NI_AO_DEV` when those are set; `--ai-dev` / `--ao-dev` override both.

use eigen_hal::{AcquisitionConfig, Backend};
use eigen_hal_ni::{plan_from_table, NiBackend};

fn main() {
    let mut ai_dev = ni_dev_from_env("EIGEN_NI_AI_DEV", "cDAQ1Mod1");
    let mut ao_dev = ni_dev_from_env("EIGEN_NI_AO_DEV", "cDAQ1Mod2");
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--ai-dev" => {
                i += 1;
                ai_dev = argv[i].clone();
            }
            "--ao-dev" => {
                i += 1;
                ao_dev = argv[i].clone();
            }
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // A closed-loop-shaped table: one control input + one drive. Voltage front
    // end (the bench is jumpered AO->AI; IEPE would push excitation current
    // into the 9263's output stage).
    let mut table = eigen_channel_table::ChannelTable::new();
    let mut ch = eigen_channel_table::Channel::new(1);
    ch.role = eigen_channel_table::ChannelRole::Control;
    ch.sensitivity_mv_per_eu = 100.0;
    ch.range_max = 10.0;
    table.channels.push(ch);
    let mut drive = eigen_channel_table::Channel::new(2);
    drive.role = eigen_channel_table::ChannelRole::Drive;
    table.channels.push(drive);

    let inputs = vec![format!("{ai_dev}/ai0")];
    let outputs = vec![format!("{ao_dev}/ao0")];
    let cfg = match plan_from_table(&table, &inputs, &outputs) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("plan: {e}");
            std::process::exit(1);
        }
    };
    let acq = AcquisitionConfig::new(51_200.0, 2048);
    let mut backend = NiBackend::new(cfg, acq);
    if let Err(e) = backend.open() {
        eprintln!("open: {e}");
        std::process::exit(1);
    }
    let caps = backend.caps();
    let sim = backend.all_simulated();
    println!("=== arm-refusal-probe ===");
    println!(
        "station: {} / {}  simulated={sim}  clock={:?}",
        ai_dev, ao_dev, caps.clock
    );
    println!("clock evidence: {}", backend.clock_evidence());
    if sim {
        println!(
            "NOTE: DRIVER-SIMULATED devices - this run does NOT satisfy the arm-refusal check; \
             re-run against the real chassis."
        );
    }

    // The exact request Random/Sine make: host synthesizes drive AND adapts
    // closed-loop. StationClass reflects what the driver said.
    let class = if sim {
        eigen_module_abi::StationClass::NiSimulated
    } else {
        eigen_module_abi::StationClass::NiReal
    };
    let request = eigen_module_abi::Resolution::host_closed_loop();
    let proven_shared = matches!(caps.clock, eigen_hal::ClockDomain::Shared) && !sim;
    match eigen_module_abi::arm_gate(&caps, class, request, None, caps.sample_rate_hz) {
        Err(reason) => {
            if proven_shared {
                println!(
                    "--- INCONSISTENT: the clock is proven Shared yet the arm was \
                     refused - diagnose the gate path ---"
                );
                println!("refusal (verbatim): {reason}");
                std::process::exit(2);
            }
            println!("--- arm-refusal PASS: the arm was REFUSED, typed ---");
            println!("refusal (verbatim): {reason}");
            std::process::exit(0);
        }
        Ok(_) if proven_shared => {
            println!(
                "--- arm PERMITTED, consistent with the proven-set entry above: the \
                 shared-clock proof is live on this station ---"
            );
            std::process::exit(0);
        }
        Ok(_) => {
            println!(
                "--- arm-refusal CRITICAL FAILURE: a closed-loop arm was PERMITTED with no \
                 proven clock entry ---"
            );
            std::process::exit(2);
        }
    }
}

/// `var` when it is set and not blank, else `default`. The flags above still win.
fn ni_dev_from_env(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}
