//! `shock-shot-probe` - the BASELINE instrument for classical shock on the real NI
//! chassis.
//!
//! COMMITTED BEFORE THE FIX EXISTS, and that is the whole point. Rate adoption
//! and the per-shot cold task have NOT landed. This probe is here to measure the
//! BROKEN state honestly, so that after they land there is a before-number to
//! compare against instead of a memory of one. A pass condition chosen after
//! seeing the data is not a pass condition, and a fix with no before-number is a
//! story.
//!
//! It prints RAW NUMBERS AND NO EXPECTED VALUES. There is deliberately no
//! "expected lag", no tolerance, no PASS/FAIL on the alignment rows - stating an
//! expectation here would pre-commit the later chassis gate to whatever this
//! session happened to guess, and the group-delay model that row will be written
//! against does not exist yet. The ONE threshold in this file is
//! [`PEAK_FLOOR_R`], and it is a DETECTION threshold, not an acceptance one: it
//! decides whether a correlation peak exists at all, and "NO-PEAK" is a distinct
//! third verdict rather than a failure. NO-PEAK is a PLAUSIBLE pre-fix outcome
//! and must not be read as a broken probe.
//!
//! # The three rows
//!
//! (i)   ONE fire. Prints the raw cross-correlation lag between the COMMANDED
//!       record and the INGESTED record over the FULL record, both record
//!       lengths, and the peak |r|.
//! (ii)  ARM -> N seconds idle -> FIRE (N from argv, default 30). The armed-idle
//!       row: on the pre-cold-task shape the AI task runs from arm, so a long
//!       idle is expected to overflow it. Prints the fire result verbatim.
//! (iii) A 3-pulse train at 2.0 s spacing. Prints each pulse's WALL timestamp
//!       (off the service's `TrainPacer`) and each pulse's raw xcorr lag.
//!
//! # What this probe CANNOT see (stated so a clean run is not read as more)
//!
//!  * **Driver warnings.** `Backend::take_warnings` is drained by nobody on this
//!    path yet - draining them is later work that lands AFTER this baseline. The
//!    firer is moved into the service and `BackendPulseFirer::backend()` hands
//!    back a shared reference, while `take_warnings` needs `&mut`, so there is
//!    no route to them from out here. The sitting that runs this greps the
//!    system log for the window instead, and row (ii)'s DAQmx error (if any)
//!    arrives as the text of the fire refusal.
//!  * **The drive on the wire.** Everything here is measured through the AI
//!    jumper, so it cannot distinguish "no drive was produced" from "drive was
//!    produced and the acquisition missed it". `tools/probes/shock_dmm.py` is
//!    the instrument for that question, and it is the only evidence admissible
//!    for the abort rows (the AI is stopped by an abort).
//!  * **Absolute time.** Row (iii)'s wall timestamps come from the service's own
//!    pacer epoch. They are differences, and a rate assay against `Instant` is
//!    a later gate row, not this one.
//!  * **Whether the record it compares against is what the DEVICE played.** The
//!    commanded record is re-synthesized here from the same `ClassicalPulseSpec`
//!    at the same requested `fs_hz` the service uses - so a rate COERCION (the
//!    9234 rides a 13.1072 MHz/(256n) ladder) makes the two disagree in time
//!    base, and that disagreement is exactly the rate-adoption defect. Both rates are
//!    printed side by side; do not read the lag without reading them.
//!
//! # Bench contract
//!
//! The one every 2026-07-30 probe carries: jumpered AO0 -> AI0, NOTHING
//! downstream, IEPE OFF (the 9263 cannot sink the 9234's 2.1 mA), drive
//! amplitudes well inside +/-1 V on the wire.
//!
//! ```text
//! cargo run --release -p hertzforge-station --example shock_shot_probe -- [idle_s]
//! ```
//!
//! Gating: this example REQUIRES the real chassis, and it is gated the way every
//! other hardware example in this crate is gated - AT RUNTIME, not by a cargo
//! feature. (`hertzforge-station`'s manifest declares no `[[example]]` sections and
//! no features; `ni_module_sweep.rs`, `ni_random_arm_probe.rs` and the rest all
//! compile unconditionally and refuse at `station()`.) The refusal is two-fold:
//! the factory failing to build exits 2, and a chassis the driver reports as
//! SIMULATED exits 2 as well - a baseline measured against a simulated device
//! would be a number about nothing.
//!
//! Exit: 0 = ran and printed; 1 = harness error (a row could not be driven);
//! 2 = could not reach hardware.

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

/// Detection floor for the cross-correlation peak, as a normalized correlation
/// coefficient (0 = nothing in common, 1 = identical up to scale).
///
/// WHY 0.30. The comparison is a compensated half-sine against whatever came
/// back on AI0. Two records that share a pulse - even badly misaligned, even
/// with the wrong time base - correlate far above this at their best lag; two
/// records that share nothing (drive never reached the wire, or the acquisition
/// window missed it entirely) wander around a few hundredths, set by the record
/// length. 0.30 sits in the empty space between those regimes with room on both
/// sides, so the verdict does not turn on the exact value. It is NOT an
/// acceptance threshold and says nothing about whether the lag is correct: it
/// only separates "there is a peak to report" from "there is not".
const PEAK_FLOOR_R: f64 = 0.30;

/// Row (iii)'s inter-pulse spacing, seconds. Above the conservative HostDac
/// rate-model floor, so the plan realizes it rather than the floor.
const TRAIN_SPACING_S: f64 = 2.0;
const TRAIN_COUNT: u32 = 3;

/// A fresh REAL station, built through the same factory the GUI uses. Refuses
/// (exit 2) if the factory cannot build it or the driver says it is simulated.
fn station() -> hertzforge_station::StationBackend {
    let spec = StationSpec {
        sim: SimPlantSpec::default(),
        ni: NiSelection {
            inputs: Vec::new(),
            outputs: Vec::new(),
            derive_inputs: 1,
            derive_outputs: 1,
            iepe: false,
            restrict_devices: vec![ai_dev(), ao_dev()],
        },
        ni_block_frames: hertzforge_hal::default_block_frames(NI_BUILD_RATE_HZ),
    };
    let s = match try_build_station_backend(BackendKind::Ni, &spec) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("station: REFUSED - {e:?}");
            std::process::exit(2);
        }
    };
    if s.provenance.driver_simulated == Some(true) {
        eprintln!(
            "station resolved DRIVER-SIMULATED - a baseline measured against a \
             simulated device is a number about nothing"
        );
        std::process::exit(2);
    }
    s
}

/// One control input + one drive row, the bench-safe scaling every 2026-07-30
/// probe uses (100 mV/EU x +/-10 EU = +/-1 V requested). Same table as
/// `ni_module_sweep.rs`'s classical section, deliberately: the two rows must be
/// comparable, and the later chassis gate owns changing it (to a volts-exact
/// table).
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

fn tmp(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "hertzforge-shotprobe-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&p);
    p
}

/// The classical config this probe fires: the `ni_module_sweep` teaching case
/// (0.5 g peak = 0.5 V on the jumper, 11 ms half-sine).
fn probe_config() -> hertzforge_shock_host::ClassicalTestConfig {
    let spec = hertzforge_shock_host::ClassicalPulseSpec::new(
        hertzforge_shock_host::ClassicalShape::HalfSine,
        0.5,
        11.0,
    );
    hertzforge_shock_host::ClassicalTestConfig::new(spec)
}

/// Mean-removed, energy-normalized cross-correlation over EVERY lag the two
/// records admit. Returns `(lag_samples, peak_r)`, where a POSITIVE lag means
/// `b` is DELAYED relative to `a` (the response arrived `lag` samples after the
/// drive that caused it).
///
/// Deliberately the textbook O(N*M) form and deliberately over the FULL record:
/// this is an instrument, the records are ~10^3 samples, and a windowed or
/// FFT-based estimator would put its own assumptions (a window, a wrap, a
/// zero-pad) between the numbers and the reader. Mean removal matters because a
/// DC offset on the AI would otherwise dominate every lag equally and flatten
/// the peak; energy normalization is what makes `r` comparable across amplitudes
/// so one floor can serve every level of the ladder.
fn xcorr_peak(a: &[f64], b: &[f64]) -> (isize, f64) {
    if a.is_empty() || b.is_empty() {
        return (0, 0.0);
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let (ma, mb) = (mean(a), mean(b));
    let ea: f64 = a.iter().map(|x| (x - ma) * (x - ma)).sum::<f64>().sqrt();
    let eb: f64 = b.iter().map(|x| (x - mb) * (x - mb)).sum::<f64>().sqrt();
    if ea <= 0.0 || eb <= 0.0 {
        return (0, 0.0);
    }
    let (n, m) = (a.len() as isize, b.len() as isize);
    let mut best_lag = 0isize;
    let mut best = 0.0f64;
    for lag in -(m - 1)..n {
        let mut acc = 0.0;
        // b[j] pairs with a[j + lag].
        let jlo = (-lag).max(0);
        let jhi = (n - lag).min(m);
        for j in jlo..jhi {
            acc += (a[(j + lag) as usize] - ma) * (b[j as usize] - mb);
        }
        let r = acc / (ea * eb);
        if r.abs() > best.abs() {
            best = r;
            best_lag = lag;
        }
    }
    (best_lag, best)
}

/// Print one alignment line for a record pair. No expected value, by design.
fn report_alignment(what: &str, commanded: &[f64], measured: &[f64], fs_hz: f64) {
    let (lag, r) = xcorr_peak(commanded, measured);
    println!(
        "  {what}: commanded {} samples, ingested {} samples, fs {:.4} Hz",
        commanded.len(),
        measured.len(),
        fs_hz
    );
    if r.abs() < PEAK_FLOOR_R {
        println!(
            "  {what}: VERDICT NO-PEAK - best |r| {:.4} is below the {:.2} detection \
             floor at lag {lag}; there is no correlation peak to report (a distinct \
             outcome, not a failure)",
            r.abs(),
            PEAK_FLOOR_R
        );
    } else {
        println!(
            "  {what}: raw xcorr lag {lag} samples ({:+.4} ms at {:.4} Hz), peak r {:+.4}",
            lag as f64 * 1000.0 / fs_hz,
            fs_hz,
            r
        );
    }
}

/// Build + arm one service on a fresh station. `Err` carries a printable reason.
#[allow(clippy::type_complexity)]
fn armed_service(
    tag: &str,
) -> Result<
    hertzforge_shock_host::ClassicalShockService<
        hertzforge_shock_host::BackendPulseFirer<Box<dyn hertzforge_hal::Backend>>,
    >,
    String,
> {
    let st = station();
    let firer = hertzforge_shock_host::BackendPulseFirer::new(st.backend, table(), st.station)
        .map_err(|e| format!("firer construct: {e}"))?;
    let mut svc = hertzforge_shock_host::ClassicalShockService::new(
        probe_config(),
        table(),
        firer,
        Some(tmp(tag)),
    )
    .map_err(|e| format!("service construct: {e:?}"))?;
    svc.assess_feasibility()
        .map_err(|e| format!("feasibility: {e:?}"))?;
    svc.arm().map_err(|e| format!("arm: {e:?}"))?;
    Ok(svc)
}

/// The COMMANDED composite, re-synthesized exactly the way the service does it
/// at arm: same spec, same requested `fs_hz`. See the "cannot see" note about
/// what a rate coercion does to this comparison.
fn commanded_record() -> (Vec<f64>, f64) {
    let cfg = probe_config();
    let comp = cfg.spec.synthesize(cfg.fs_hz);
    (comp.waveform_g, cfg.fs_hz)
}

/// The commanded composite at an ARBITRARY rate. The service now ADOPTS
/// the device's actual rate at arm and synthesizes there, so the xcorr reference
/// must follow the INGESTED record's own `fs_hz` - comparing a 5120 Hz reference
/// against a 51 200 Hz record is a NO-PEAK manufactured by the probe, not by the
/// plant (caught 2026-08-08 before the baseline re-run; the baseline runs predate
/// adoption, so their 5120-vs-5120 comparisons stand).
fn commanded_at(fs_hz: f64) -> Vec<f64> {
    probe_config().spec.synthesize(fs_hz).waveform_g
}

fn main() {
    let idle_s: f64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(30.0);

    println!("=== shock-shot-probe (baseline, PRE-FIX) ===");
    println!("RAW numbers only. No expected values are stated anywhere in this");
    println!("output; NO-PEAK is a distinct verdict, not a failure.");
    let (commanded, requested_fs) = commanded_record();
    println!(
        "commanded record: {} samples synthesized at the REQUESTED {requested_fs:.4} Hz",
        commanded.len()
    );

    let mut harness_error = false;

    // --- (i) one fire ------------------------------------------------------
    println!();
    println!("--- (i) SINGLE FIRE ---");
    match armed_service("single") {
        Err(e) => {
            eprintln!("  HARNESS: {e}");
            harness_error = true;
        }
        Ok(mut svc) => match svc.fire_next_pulse() {
            Err(e) => {
                println!("  fire REFUSED: {e:?}");
            }
            Ok(_) => match svc.pulse_store().records().last() {
                None => println!("  fire returned Ok but the store is EMPTY (no record ingested)"),
                Some(rec) => {
                    println!(
                        "  ingested: seq {} at {:.4} Hz (the DEVICE's rate as the service \
                         recorded it)",
                        rec.seq, rec.fs_hz
                    );
                    report_alignment(
                        "single",
                        &commanded_at(rec.fs_hz),
                        &rec.time_history,
                        rec.fs_hz,
                    );
                }
            },
        },
    }

    // --- (ii) arm -> idle -> fire -----------------------------------------
    println!();
    println!("--- (ii) ARM -> {idle_s:.1} s IDLE -> FIRE ---");
    match armed_service("idle") {
        Err(e) => {
            eprintln!("  HARNESS: {e}");
            harness_error = true;
        }
        Ok(mut svc) => {
            println!("  armed; holding {idle_s:.1} s with nothing driving ...");
            std::thread::sleep(std::time::Duration::from_secs_f64(idle_s));
            // The DAQmx diagnostic, if any, arrives as the TEXT of this refusal -
            // see the "cannot see" note on take_warnings.
            match svc.fire_next_pulse() {
                Err(e) => println!("  fire after idle REFUSED: {e:?}"),
                Ok(_) => match svc.pulse_store().records().last() {
                    None => println!("  fire returned Ok but the store is EMPTY"),
                    Some(rec) => {
                        println!("  fire after idle OK: {} samples", rec.time_history.len());
                        report_alignment(
                            "after-idle",
                            &commanded_at(rec.fs_hz),
                            &rec.time_history,
                            rec.fs_hz,
                        );
                    }
                },
            }
            // Whatever the service DID log for this run, verbatim.
            println!("  event log:");
            for e in svc.log().events() {
                println!("    [{}] {:?}", e.seq, e.kind);
            }
        }
    }

    // --- (iii) 3-pulse train at 2.0 s --------------------------------------
    println!();
    println!("--- (iii) TRAIN: {TRAIN_COUNT} pulses at {TRAIN_SPACING_S:.1} s ---");
    match armed_service("train") {
        Err(e) => {
            eprintln!("  HARNESS: {e}");
            harness_error = true;
        }
        Ok(mut svc) => {
            let spec = hertzforge_shock_common::TrainSpec::delayed(TRAIN_COUNT, TRAIN_SPACING_S);
            match svc.fire_train_with(spec) {
                Err(e) => println!("  train REFUSED: {e:?}"),
                Ok(plan) => {
                    println!(
                        "  planned: count {}, realized spacing {:.6} s, floor {:.6} s",
                        plan.count, plan.realized_spacing_s, plan.floor_s
                    );
                    let report = svc.report_data();
                    let walls = report
                        .trains
                        .last()
                        .map(|t| t.wall_times_s.clone())
                        .unwrap_or_default();
                    for (i, t) in walls.iter().enumerate() {
                        let delta = if i == 0 {
                            String::from("-")
                        } else {
                            format!("{:+.6} s since previous", t - walls[i - 1])
                        };
                        println!("  pulse {i}: wall t = {t:.6} s ({delta})");
                    }
                    for (i, rec) in svc.pulse_store().records().iter().enumerate() {
                        report_alignment(
                            &format!("train-pulse-{i}"),
                            &commanded_at(rec.fs_hz),
                            &rec.time_history,
                            rec.fs_hz,
                        );
                    }
                    // A train that stops short must SAY so - the 2026-08-08 baseline
                    // run fired 1/3 and this probe printed nothing about pulses 1-2
                    // (the terminal error lived only in the service log). Terminal
                    // accounting + the log tail, verbatim, whatever the outcome.
                    let fired = svc.pulse_store().records().len();
                    println!("  train terminal: {fired}/{} pulses fired", plan.count);
                    if fired < plan.count as usize {
                        println!("  train TERMINATED EARLY; event-log tail:");
                        let events = svc.log().events();
                        let tail = events.len().saturating_sub(4);
                        for e in &events[tail..] {
                            println!("    [{}] {:?}", e.seq, e.kind);
                        }
                    }
                }
            }
        }
    }

    println!();
    if harness_error {
        eprintln!("=== one or more rows could not be driven (harness error) ===");
        std::process::exit(1);
    }
    println!("=== ran and printed ===");
}
