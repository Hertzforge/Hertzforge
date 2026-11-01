//! `ni-module-sweep` - arrival gate 6: every remaining module RUNS on the real
//! NI station, rank order (Recorder -> TWR -> SRS -> Shock B -> Sine ->
//! Explorer).
//!
//! Each section builds a FRESH station through the same `hertzforge-station` factory
//! the GUI uses (backends are consumed by arm paths), constructs the module's
//! service exactly the way the app's tauri layer does, and drives its minimal
//! open-loop lifecycle on the real chassis. PASS rows state what actually
//! happened (frames, spectra, pulses, coherence); a section that cannot run
//! prints FAIL with the verbatim refusal and the sweep continues, so one
//! module's defect never hides another's result.
//!
//! **`explorer` is a different KIND of section**. The others prove a lifecycle
//! completes; that one asserts NUMBERS off the acquired signal - commanded volts
//! against measured volts, commanded Hz against measured Hz, worst-tick compute
//! against the budget - because Explorer is the first module whose drive an
//! operator moves by hand while the run is live, and "it ran" is not what has to
//! be true about it.
//!
//! **The paragraph above no longer describes "the others" as a group.** `srs` and
//! `classical` were rebuilt to the same numeric bar later, and so was `twr` -
//! each states what it measured, which INDEPENDENT authority it compared that
//! against, and the tolerance between them. `recorder` is the remaining lifecycle
//! section and says so in its own header. Read each section's header for which
//! kind it is rather than this one.
//!
//! Random is deliberately absent: it ran first (`ni_random_arm_probe`, 2026-07-30).
//!
//! Bench contract (the same one every 2026-07-30 probe carries): jumpered
//! AO0-3 -> AI0-3, NOTHING downstream, IEPE OFF everywhere (the 9263 cannot
//! sink the 9234's 2.1 mA excitation), every drive amplitude well inside
//! +/-1 V on the wire.
//!
//! ```text
//! cargo run --release -p hertzforge-station --example ni_module_sweep
//! ```
//!
//! Exit: 0 = every section passed; 1 = at least one FAIL; 2 = the station
//! could not be built at all (or resolved simulated - a sweep row must never
//! pass on a simulated device).

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

struct Rows {
    passed: u32,
    failed: u32,
}

impl Rows {
    fn row(&mut self, ok: bool, what: &str, detail: String) {
        if ok {
            self.passed += 1;
            println!("  PASS    {what} ({detail})");
        } else {
            self.failed += 1;
            println!("  FAIL    {what} ({detail})");
        }
    }
}

/// A fresh REAL station: the two chassis modules, voltage front end, exactly
/// what the GUI's per-module factory request derives. Refuses (exit 2) if the
/// driver says any bound device is simulated.
fn station() -> hertzforge_station::StationBackend {
    station_with_outputs(1)
}

/// Same station, N derived AO channels. The COLA rows need 2: the 9263 has four
/// physical AOs, only AO0 is jumpered/measured, and that is fine for those rows -
/// their asserts are host-state + AI0; the aux channel's electrical output is
/// scored by the operator's DMM tee per the row's own dmm_mark.
fn station_with_outputs(n_out: usize) -> hertzforge_station::StationBackend {
    let spec = StationSpec {
        sim: SimPlantSpec::default(),
        ni: NiSelection {
            inputs: Vec::new(),
            outputs: Vec::new(),
            derive_inputs: 1,
            derive_outputs: n_out,
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
        eprintln!("station resolved DRIVER-SIMULATED - this sweep is only evidence on real iron");
        std::process::exit(2);
    }
    s
}

/// One control input + one drive row, the bench-safe scaling every 2026-07-30
/// probe uses: 100 mV/EU x +/-10 EU = +/-1 V requested (legal on any device),
/// drive amplitudes stay well under 1 V on the jumper.
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
    let p = std::env::temp_dir().join(format!("hertzforge-sweep-{}-{name}", std::process::id()));
    let _ = std::fs::create_dir_all(&p);
    p
}

fn main() {
    println!("=== ni-module-sweep (rank order) ===");
    let mut rows = Rows {
        passed: 0,
        failed: 0,
    };

    let only = std::env::args().nth(1);
    let want = |name: &str| only.as_deref().is_none_or(|o| o == name);
    if want("recorder") {
        recorder(&mut rows);
    }
    if want("twr") {
        twr(&mut rows);
    }
    if want("srs") {
        srs(&mut rows);
    }
    if want("classical") {
        classical(&mut rows);
    }
    if want("sine") {
        sine(&mut rows);
    }
    // The sine BASELINE is opt-in, never part of a full sweep: it is a ~6 minute
    // driven run whose whole purpose is to OBSERVE the pre-conversion chunk-
    // boundary defect, so it costs a bench sitting and it is expected to FAIL a
    // row. `want()` returns true when no section was named, which would drag it
    // into every sweep; this asks for the name explicitly.
    if only.as_deref() == Some("sine-baseline") {
        sine_baseline(&mut rows);
    }
    if want("explorer") {
        explorer(&mut rows);
    }

    println!();
    println!(
        "--- SUMMARY: {} passed, {} failed ---",
        rows.passed, rows.failed
    );
    if rows.failed > 0 {
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// 1. RECORDER - monitor + pump + spectra, locally on the chassis (the daemon
//    already proved this path remotely; this is the local half).
// ---------------------------------------------------------------------------
fn recorder(rows: &mut Rows) {
    println!();
    println!("--- RECORDER ---");
    let st = station();
    // INPUT-ONLY table: the recorder injects its own synthetic drive row,
    // and an explicit one alongside it is two drive rows against one AO -
    // refused (correctly) as an unassignable row. Found by this sweep's first
    // run on the chassis.
    let mut t = table();
    t.channels
        .retain(|c| c.role != hertzforge_channel_table::ChannelRole::Drive);
    let cfg = hertzforge_recorder::service::RecorderConfig::default();
    let fs = cfg.acq.native_rate_hz;
    let svc = hertzforge_recorder::service::RecorderService::with_backend(
        cfg,
        t,
        st.backend,
        st.station,
        tmp("recorder"),
    );
    let mut svc = match svc {
        Ok(s) => s.with_block_frames(hertzforge_hal::default_block_frames(fs)),
        Err(e) => {
            rows.row(false, "recorder construct", format!("{e}"));
            return;
        }
    };
    if let Err(e) = svc.configure() {
        rows.row(false, "recorder configure", format!("{e}"));
        return;
    }
    if let Err(e) = svc.start_monitor() {
        rows.row(false, "recorder start_monitor", format!("{e}"));
        return;
    }
    let mut frames = 0u64;
    let mut fault = None;
    for _ in 0..4 {
        match svc.pump() {
            Ok(n) => frames += n,
            Err(e) => {
                fault = Some(format!("{e}"));
                break;
            }
        }
    }
    let spectra = svc.spectra();
    let n_spectra = spectra.len();
    let spectra_nonempty = spectra.iter().all(|(_, m)| !m.is_empty());
    let _ = svc.stop_monitor();
    match fault {
        Some(f) => rows.row(false, "recorder pump", f),
        None => {
            rows.row(
                frames > 0,
                "recorder monitor pumped frames",
                format!("{frames} frames"),
            );
            rows.row(
                n_spectra > 0 && spectra_nonempty,
                "recorder live spectra non-empty",
                format!("{n_spectra} trace(s)"),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. TWR - THE CHASSIS GATE.
//
// This section used to be ONE row: build a service, call `run_to_completion(1)`
// and PASS on `Ok`. That row is what let the 2026-07-30 chassis run be written
// up as "RUNS ON REAL NI HARDWARE: sysid + a replication unit on the wire" when
// `run_to_completion(1)` from `Configured` runs `do_sysid()` and NOTHING ELSE:
// ZERO TWR replication iterations have ever run on hardware, and the row's own
// detail string printed `0 convergence record(s)` while passing (corrected
// 2026-08-08). Worse, it could not fail for a second reason:
// `run_to_completion` returns `Ok` from SIX of the eight end states including
// two PARKS, so a run that never identified the plant and a run that converged
// the whole ladder produced the same green.
//
// Rebuilt to the bar the shock and explorer sections carry (see their header
// comments): every row states a MEASURED quantity, the INDEPENDENT authority it
// is compared against, and the tolerance between them. NO ROW WHOSE TWO SIDES
// TRACE TO THE SAME AUTHORITY - the old TWR row had ONE side and no authority
// at all.
//
// FIVE PHASES, each on its own fresh station:
//
//   A  bare player, one sysid-shaped play  -> ROW 1 (in-band coherence vs a
//      floor derived from THIS rig's arithmetic) + ROW 2 (the MEASURED xcorr
//      lag vs the group-delay decomposition). Both rows come off ONE play, so
//      the coherence number and the alignment number describe the same record.
//   B  the service, FULL LADDER to Complete -> ROW 3 (convergence) + ROW 4
//      (over-buffer streaming with zero driver underrun warnings) + ROW 7 (the
//      per-play task-cycle cost that feeds the watchdog thresholds).
//   C  bare player, a reference stamped at an OFF-LADDER rate -> ROW 5 (the
//      arm gate refuses by name, quoting both rates). Nothing is driven.
//   D  the service, a target whose inverse demands more than the ceiling ->
//      ROW 6 (named refusal carrying both numbers; the run aborts, the SWEEP
//      does not).
//   E  the service, cancel raised MID-PLAY from another thread -> ROW 8 (the
//      run ends `Stopped`, never `Aborted`, and the DAQmx task is released).
//
// WHY A WRAPPER BACKEND (`ProbedBackend`). Three of these rows need facts that
// live on the far side of the player: what `configure()` actually answered, how
// many frames the driver ACCEPTED before `start()`, how long each arm->start
// and stop->unreserve cycle took, and what `Backend::take_warnings()` had to
// say after each play. `TwrService::player()` is `pub(crate)` and
// `BackendReferencePlayer::backend()` hands back a shared reference while
// `take_warnings` needs `&mut`, so none of it is reachable from here through
// the service. The wrapper sits between the factory's backend and the player,
// forwards every trait method, and records those four things into an
// `Arc<Mutex<Probe>>` the gate keeps. It is an INSTRUMENT, not a stand-in: it
// changes no behavior and every number it reports is the driver's own.
//
// Bench contract, the same one every other section carries: AO0 jumpered to
// AI0, NOTHING downstream, IEPE OFF (the 9263 cannot sink the 9234's 2.1 mA),
// every drive amplitude well inside +/-1 V on the wire. Unlike the shock
// sections, this one does NOT stand in for a missing ceiling: TWR HAS a
// drive-volts ceiling (`TwrLimits::max_drive_vpk`, peak volts on the realized
// record, refuses and never clips), so [`twr_rig::BENCH_MAX_DRIVE_V`] is
// INSTALLED IN THE SERVICE on every phase that drives, and the bench cap and
// the run-time protection are the same number rather than two.
// ---------------------------------------------------------------------------
fn twr(rows: &mut Rows) {
    twr_rig::twr(rows);
}

mod twr_rig {
    use super::{ai_dev, ao_dev, station, Rows};
    use hertzforge_hal::{
        AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
    };
    use hertzforge_station::{
        try_build_station_backend, BackendKind, NiSelection, SimPlantSpec, StationSpec,
        NI_BUILD_RATE_HZ,
    };
    use hertzforge_twr::player::LagSource;
    use hertzforge_twr::reference::ImportSource;
    use hertzforge_twr::schedule::TwrRunMode;
    use hertzforge_twr::{
        BackendReferencePlayer, ReferencePlayer, ReferenceWaveform, SysIdConfig, TwrConfig,
        TwrService, TwrState,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    // -----------------------------------------------------------------------
    // Bench + profile constants
    // -----------------------------------------------------------------------

    /// The section's drive ceiling, volts PEAK - and unlike the shock sections'
    /// constant of the same name, this one is not a stand-in for a missing
    /// service concept. It is written into `TwrLimits::max_drive_vpk` on every
    /// phase that drives, so the bench contract and the run-time refusal are
    /// ONE number. Half the +/-1 V the jumper contract allows.
    const BENCH_MAX_DRIVE_V: f64 = 0.50;

    /// ROW 1's floor for the sysID in-band MEAN coherence.
    ///
    /// DERIVED from this rig and this module's own sysid config, not copied
    /// from a sibling section (shock uses 0.90, sine 0.999 - both are answers
    /// to different questions at different drive levels). The two ceilings that
    /// actually bound the number here:
    ///
    /// 1. **Quantization.** The excitation is [`SYSID_DRIVE_RMS`] = 0.05 V rms
    ///    spread over [`SYSID_BAND_HZ`] (1900 Hz wide). The 9263's 16-bit DAC
    ///    over its fixed +/-10 V span has an LSB of 20/65536 = 305 uV, so a
    ///    uniform quantization noise of LSB/sqrt(12) = 88 uV rms spread across
    ///    the whole 0-25.6 kHz Nyquist band; the in-band share is
    ///    88 uV * sqrt(1900/25600) = 24 uV. That is an in-band SNR of ~2100
    ///    (66 dB), whose coherence ceiling `SNR^2/(1+SNR^2)` is 1 - 2.3e-7.
    ///    (This is the number that bit the SINE section, whose burst is 25 mV -
    ///    a 2000x smaller signal against the same LSB. At 0.05 V rms TWR is
    ///    nowhere near it, which is why 0.99 is not a tight floor here.)
    /// 2. **Residual alignment inside the Welch segment.** The estimator is
    ///    Hann-windowed, 2048-point, 50% overlap. The measured lag (ROW 2) is
    ///    integer-frame, so the residual is under one frame in 2048 and the
    ///    Hann window's own correlation loss over that offset is below 1e-6.
    ///
    /// So the honest expectation on a wire is 0.999-plus, and 0.99 sits an
    /// order of magnitude of error below it. A RED on this row is not a
    /// marginal rig: it is a broken alignment (the 2026-07-30 chassis read
    /// **0.010** in-band on this same jumper, with the lag 2106 frames out
    /// against an nfft of 2048 - drive and response in different segments) or a
    /// broken acquisition. It is deliberately TIGHTER than the service's own
    /// shipped hold threshold (`SysIdConfig::min_mean_coherence` = 0.8): the
    /// gate must red before the module holds, not after.
    const SYSID_COHERENCE_FLOOR: f64 = 0.99;

    /// ROW 2's tolerance, FRAMES, between the measured lag and the model.
    ///
    /// The model (see [`lag_model`]) carries the two terms this rig can state.
    /// The residual it does NOT carry is the 9263's own analog-OUTPUT path
    /// delay, which no constant in this workspace models - the shock section
    /// hit the identical unattributed term and measured it at +2 frames at
    /// 51 200 Hz on this chassis. It is NOT folded in here (a model tuned to a
    /// number explains nothing), so the band has to
    /// admit it: +/-4 frames = 78 us at 51 200 Hz, room for that term and half
    /// again.
    ///
    /// The band is still ~250x tighter than every failure mode it exists to
    /// catch, all of which are block-scale or larger: one block (1024), the
    /// output lookahead (2048), the whole AO buffer (3072), and the 2026-07-30
    /// chassis miss (2106).
    const ALIGN_TOL_FRAMES: i64 = 4;

    /// ROW 3's convergence gate, percent rms error. The SHIPPED default
    /// (`TwrLevelSchedule::gate_rms_error_pct`), stated here so the row prints
    /// the number it gates on instead of implying one.
    const CONVERGENCE_GATE_PCT: f64 = 10.0;

    /// ROW 7's sanity bound on ONE task cycle (arm->commit->start, or
    /// stop->unreserve). The row is informational with a floor: its job is to
    /// PRINT min/median/max so the watchdog deadlines can be derived from
    /// measured cadence rather than from a guess, and to fail only if a cycle
    /// is so long that the per-play shape is not viable at all.
    const CYCLE_SANITY_BOUND: Duration = Duration::from_secs(5);

    /// ROW 4's multiple: the played record must be at least this many times the
    /// device's whole AO buffer, so that MOST of it streams after `start()` and
    /// the row is actually asking the underrun question (the
    /// per-play cycle buys a silence-free HEAD, it does not delete the underrun
    /// class for the streaming window).
    const OVER_BUFFER_MULTIPLE: usize = 10;

    /// sysID excitation RMS, volts (see [`SYSID_COHERENCE_FLOOR`] for what this
    /// number buys). Peak is ~4.5 sigma = 0.23 V, comfortably under
    /// [`BENCH_MAX_DRIVE_V`], and the gate checks that before it plays.
    const SYSID_DRIVE_RMS: f64 = 0.05;
    /// sysID excitation length, seconds.
    const SYSID_DURATION_S: f64 = 2.0;
    /// The identification band. The low edge is 100 Hz rather than the shipped
    /// 20 Hz for a reason that is arithmetic, not taste: at nfft 2048 and
    /// 51 200 Hz the bin spacing is 25 Hz, so a 20 Hz edge puts the LOWEST
    /// in-band bin at 25 Hz - one bin from DC, where Hann leakage from the
    /// (unexcited) DC bin is still significant, and where one bad bin out of 80
    /// moves the in-band MEAN by 1.25%. 100 Hz puts the edge four bins out.
    const SYSID_BAND_HZ: (f64, f64) = (100.0, 2000.0);
    /// The correction band, which config validation requires to sit INSIDE the
    /// sysid band (the inverse is extrapolation outside the identified band).
    const LAW_BAND_HZ: (f64, f64) = (120.0, 1800.0);
    /// Reference content, Hz - four tones, all inside [`LAW_BAND_HZ`], under a
    /// sin^2 envelope.
    const REFERENCE_TONES_HZ: [f64; 4] = [150.0, 320.0, 620.0, 1100.0];

    /// ROW 3/4's reference: 0.75 s at 51 200 Hz = 38 400 samples, which the
    /// padded grid rounds up to 65 536 - 21x the 3072-frame AO buffer, so
    /// [`OVER_BUFFER_MULTIPLE`] is met with margin rather than by accident.
    const LADDER_SECONDS: f64 = 0.75;
    /// ROW 3's target peak, volts. On the volts-exact table a target EU IS a
    /// volt and the plant is a wire, so the converged drive lands near this
    /// number - well under [`BENCH_MAX_DRIVE_V`].
    const LADDER_TARGET_V: f64 = 0.20;

    /// ROW 6's target peak, volts: what the inverse will demand at the single
    /// 0 dB rung.
    const CEILING_TARGET_V: f64 = 0.40;
    /// ROW 6's ceiling, volts. Between the ranging excitation's peak (0.09 V at
    /// [`CEILING_SYSID_DRIVE_RMS`]) and the inverse's demand
    /// ([`CEILING_TARGET_V`]), so the refusal comes from the INVERSE-derived
    /// drive record and not from the excitation - which would be a ceiling
    /// refusal about the wrong thing.
    const CEILING_V: f64 = 0.25;
    /// ROW 6's ranging RMS: low enough that its ~4.5 sigma peak stays under
    /// [`CEILING_V`] with better than 2x margin, so sysID completes and the
    /// refusal lands where the row says it does.
    const CEILING_SYSID_DRIVE_RMS: f64 = 0.02;

    /// ROW 5's stated reference rate, Hz - a rate the 9234 WILL NOT PROGRAM.
    /// Its sample clock is 13.1072 MHz / (256 n) for integer n, and
    /// 13 107 200 / (256 * 8192) = 6.25 is not an integer: asked for 8192 the
    /// device coerces to n = 6, i.e. 8533.33 Hz. So this is not merely a rate
    /// that disagrees with the current configuration - no reconfiguration would
    /// rescue it either, which is exactly the class the arm gate exists for.
    const OFF_LADDER_RATE_HZ: f64 = 8192.0;

    // -----------------------------------------------------------------------
    // THE RED-MUTATION HOOKS (the per-row "must go red" rule)
    // -----------------------------------------------------------------------
    //
    // A result that cannot fail is not evidence. Every row below has a mutation
    // that must turn it red IN THE SAME SITTING, and the ones that are awkward to
    // inject by hand have an env hook here so the operator can fire them without
    // editing source at the bench.
    //
    // Every hook is either NON-DRIVING or TIGHTEN-ONLY, with one stated
    // exception: `HERTZFORGE_T2_CEILING_V` RAISES a drive ceiling and is therefore
    // clamped to BENCH_MAX_DRIVE_V and refused above it. And any hook at all
    // makes the run un-fileable - see [`mutation_guard_row`].

    fn env_flag(name: &str) -> bool {
        std::env::var(name).as_deref() == Ok("1")
    }

    fn env_f64(name: &str) -> Option<f64> {
        std::env::var(name).ok().and_then(|v| v.parse().ok())
    }

    /// Every mutation hook this section honors, and whether it is armed.
    /// Ordered as the rows are.
    fn mutations() -> Vec<(&'static str, bool)> {
        let num = |n: &str| env_f64(n).is_some();
        let on = env_flag;
        // Names are printed with the `HERTZFORGE_T2_` prefix restored by the guard.
        vec![
            ("COH_FLOOR (row 1)", num("HERTZFORGE_T2_COH_FLOOR")),
            ("ALIGN_SKEW (row 2)", num("HERTZFORGE_T2_ALIGN_SKEW")),
            ("GATE_PCT (row 3)", num("HERTZFORGE_T2_GATE_PCT")),
            ("FAKE_WARNING (row 4)", on("HERTZFORGE_T2_FAKE_WARNING")),
            ("CLIP_V (row 3 clip)", num("HERTZFORGE_T2_CLIP_V")),
            ("DROP_REF_RATE (row 5)", on("HERTZFORGE_T2_DROP_REF_RATE")),
            ("CEILING_V (row 6)", num("HERTZFORGE_T2_CEILING_V")),
            (
                "CYCLE_BOUND_MS (row 7)",
                num("HERTZFORGE_T2_CYCLE_BOUND_MS"),
            ),
            ("NO_CANCEL (row 8)", on("HERTZFORGE_T2_NO_CANCEL")),
        ]
    }

    /// The row that makes a mutated sitting un-fileable (the `shock_rig`
    /// convention). A mutated run demonstrates that a row CAN red; it is never
    /// itself a gate result, and the sweep must exit non-zero so the log cannot
    /// be filed as a pass.
    fn mutation_guard_row(rows: &mut Rows) {
        let armed: Vec<String> = mutations()
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(n, _)| format!("HERTZFORGE_T2_{n}"))
            .collect();
        if armed.is_empty() {
            return;
        }
        println!();
        println!("  ***********************************************************");
        println!("  *** THIS IS A RED-MUTATION RUN. NOTHING HERE IS EVIDENCE. ***");
        println!("  ***********************************************************");
        rows.row(
            false,
            "twr runs UNMUTATED (no HERTZFORGE_T2_* hook set)",
            format!(
                "armed: {}. A mutated run demonstrates a row can red and is NOT a gate \
                 result. Unset every HERTZFORGE_T2_* hook for the real sitting.",
                armed.join(", ")
            ),
        );
    }

    /// ROW 1's floor, tighten-only: a hook that could LOOSEN the floor would be
    /// a way to make a failing rig pass, which is the opposite of a mutation.
    fn coherence_floor() -> f64 {
        match env_f64("HERTZFORGE_T2_COH_FLOOR") {
            Some(v) if v > SYSID_COHERENCE_FLOOR => v,
            _ => SYSID_COHERENCE_FLOOR,
        }
    }

    /// ROW 3's gate, tighten-only for the same reason.
    fn gate_pct() -> f64 {
        match env_f64("HERTZFORGE_T2_GATE_PCT") {
            Some(v) if v > 0.0 && v < CONVERGENCE_GATE_PCT => v,
            _ => CONVERGENCE_GATE_PCT,
        }
    }

    /// ROW 7's bound, tighten-only.
    fn cycle_bound() -> Duration {
        match env_f64("HERTZFORGE_T2_CYCLE_BOUND_MS") {
            Some(ms) if ms > 0.0 && Duration::from_secs_f64(ms / 1e3) < CYCLE_SANITY_BOUND => {
                Duration::from_secs_f64(ms / 1e3)
            }
            _ => CYCLE_SANITY_BOUND,
        }
    }

    /// ROW 6's ceiling. The ONE hook that raises a drive limit, so it is capped
    /// at the bench contract: a request above [`BENCH_MAX_DRIVE_V`] is ignored
    /// and the shipped [`CEILING_V`] stands.
    fn ceiling_v() -> f64 {
        match env_f64("HERTZFORGE_T2_CEILING_V") {
            Some(v) if v > 0.0 && v <= BENCH_MAX_DRIVE_V => v,
            _ => CEILING_V,
        }
    }

    /// ROW 3's clip arm. `None` is the SHIPPED default and the reason the
    /// `clipped_frac == 0` assertion is a regression guard rather than
    /// live evidence - see the row's own note. The hook arms a clip below the
    /// demanded drive so the plumbing from `apply_drive_clip` to
    /// `TwrStatus::clipped_frac` to this row can be shown live.
    fn clip_v() -> Option<f64> {
        env_f64("HERTZFORGE_T2_CLIP_V").filter(|v| *v > 0.0 && *v <= BENCH_MAX_DRIVE_V)
    }

    // -----------------------------------------------------------------------
    // The station + the VOLTS-EXACT table
    // -----------------------------------------------------------------------

    /// A fresh REAL station that REFUSES instead of exiting.
    ///
    /// Same rationale as `shock_rig::try_station`, which states it at length:
    /// this section builds FIVE stations and ROW 8 exists precisely to catch a
    /// DAQmx task left RESERVED by the previous one, so a build failure must
    /// red one row rather than kill the sweep. The section's FIRST station
    /// still goes through the file's exiting `station()` - a genuinely absent
    /// chassis is still exit 2.
    fn try_station() -> Result<hertzforge_station::StationBackend, String> {
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
        let s = try_build_station_backend(BackendKind::Ni, &spec).map_err(|e| format!("{e:?}"))?;
        if s.provenance.driver_simulated == Some(true) {
            return Err("station resolved DRIVER-SIMULATED".into());
        }
        Ok(s)
    }

    /// The VOLTS-EXACT table: `1000 mV/EU` makes the host's EU conversion the
    /// IDENTITY, so a measured "EU" IS a volt and every number in this section
    /// is directly comparable to the commanded volts.
    ///
    /// It matters more here than in a lifecycle section. TWR's drive is the
    /// output of an INVERSE FILTER: `G = conj(H)/(|H|^2 + eps)` where `H` is
    /// measured in EU-per-volt, so a table that scaled EU by 10 would scale
    /// every drive record this gate produces by 1/10 and quietly make the
    /// ceiling rows meaningless. `shock_rig::volts_table` carries the full
    /// essay on the pseudo-kinematic consequence; it applies verbatim and is
    /// not repeated.
    fn volts_table() -> hertzforge_channel_table::ChannelTable {
        use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        ctrl.units = Units::VoltageV;
        ctrl.sensitivity_mv_per_eu = 1000.0;
        ctrl.range_max = 1.0;
        t.channels.push(ctrl);
        let mut drv = Channel::new(2);
        drv.role = ChannelRole::Drive;
        drv.units = Units::VoltageV;
        t.channels.push(drv);
        t
    }

    // -----------------------------------------------------------------------
    // THE PROBE: the wrapper backend and what it records
    // -----------------------------------------------------------------------

    /// One task cycle as the DRIVER experienced it.
    #[derive(Clone, Debug)]
    struct Cycle {
        /// Frames the driver ACCEPTED before `start()` - the wrapper's own
        /// count off `write_drive`'s return value, independent of the player's
        /// `last_committed_frames()`.
        committed_frames: usize,
        /// Every frame the driver accepted this cycle: the record plus its
        /// exact-zero rest pad. `written - committed` is the STREAMING WINDOW,
        /// which is the part of the play the underrun class still owns.
        written_frames: usize,
        /// `arm()` entry -> `start()` return. `None` marks the once-per-run
        /// BRING-UP PARK: `open_and_configure` arms and stops without ever
        /// starting, and that cycle is not a play.
        arm_to_start: Option<Duration>,
        /// `stop()` entry -> return. On the NI backend that call is the taper
        /// (skipped when the last clocked frame is bit-exact 0.0), the task
        /// stop, and the UNRESERVE - all three, which is why it is timed as one
        /// number and named for the unreserve.
        stop: Duration,
        /// Whatever `Backend::take_warnings()` had after this cycle's stop,
        /// verbatim.
        warnings: Vec<String>,
    }

    #[derive(Default)]
    struct Probe {
        /// What `configure()` ANSWERED. ROW 2's second authority lives here.
        configured: Option<ConfiguredActual>,
        cycles: Vec<Cycle>,
        /// Warnings drained outside a cycle (an `emergency_stop`, a stop with
        /// no matching arm). Reported, never silently dropped.
        stray_warnings: Vec<String>,
        // in-flight cycle state
        armed_at: Option<Instant>,
        arm_to_start: Option<Duration>,
        committed: usize,
        written: usize,
        running: bool,
    }

    /// The instrument. Forwards every `Backend` method - INCLUDING the two with
    /// default bodies, because a default that is not forwarded silently wins
    /// over the inner backend's real implementation (the `Box<T>` impl's own
    /// doc comment catalogues `output_space` and `take_warnings` doing exactly
    /// that, the second one costing every module its DAQmx underrun channel).
    struct ProbedBackend {
        inner: Box<dyn Backend>,
        probe: Arc<Mutex<Probe>>,
    }

    impl ProbedBackend {
        fn new(inner: Box<dyn Backend>) -> (Self, Arc<Mutex<Probe>>) {
            let probe = Arc::new(Mutex::new(Probe::default()));
            (
                ProbedBackend {
                    inner,
                    probe: probe.clone(),
                },
                probe,
            )
        }
    }

    impl Backend for ProbedBackend {
        fn caps(&self) -> Caps {
            self.inner.caps()
        }
        fn open(&mut self) -> Result<(), HalError> {
            self.inner.open()
        }
        fn close(&mut self) -> Result<(), HalError> {
            self.inner.close()
        }
        fn take_warnings(&mut self) -> Vec<String> {
            self.inner.take_warnings()
        }
        fn configure(
            &mut self,
            table: &hertzforge_channel_table::ChannelTable,
            acq: &AcquisitionConfig,
        ) -> Result<ConfiguredActual, HalError> {
            let actual = self.inner.configure(table, acq)?;
            self.probe.lock().unwrap().configured = Some(actual);
            Ok(actual)
        }
        fn arm(&mut self) -> Result<(), HalError> {
            let t0 = Instant::now();
            let r = self.inner.arm();
            let mut p = self.probe.lock().unwrap();
            p.armed_at = Some(t0);
            p.arm_to_start = None;
            p.committed = 0;
            p.written = 0;
            p.running = false;
            r
        }
        fn start(&mut self) -> Result<(), HalError> {
            let r = self.inner.start();
            let mut p = self.probe.lock().unwrap();
            p.arm_to_start = p.armed_at.map(|t| t.elapsed());
            p.running = true;
            r
        }
        fn stop(&mut self) -> Result<(), HalError> {
            let t0 = Instant::now();
            let r = self.inner.stop();
            let dt = t0.elapsed();
            let mut warnings = self.inner.take_warnings();
            if env_flag("HERTZFORGE_T2_FAKE_WARNING") {
                warnings.push(
                    "HERTZFORGE_T2_FAKE_WARNING: synthetic driver warning injected by the \
                     red-mutation hook"
                        .into(),
                );
            }
            let mut p = self.probe.lock().unwrap();
            if p.armed_at.take().is_some() {
                let cycle = Cycle {
                    committed_frames: p.committed,
                    written_frames: p.written,
                    arm_to_start: p.arm_to_start.take(),
                    stop: dt,
                    warnings,
                };
                p.cycles.push(cycle);
            } else {
                p.stray_warnings.extend(warnings);
            }
            p.running = false;
            r
        }
        fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
            self.inner.read_input(out)
        }
        fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
            let n = self.inner.write_drive(drive)?;
            let mut p = self.probe.lock().unwrap();
            p.written += n;
            if !p.running {
                p.committed += n;
            }
            Ok(n)
        }
        fn output_space(&mut self) -> Option<usize> {
            self.inner.output_space()
        }
        fn poll_status(&mut self) -> BackendStatus {
            self.inner.poll_status()
        }
        fn emergency_stop(&mut self) -> Result<(), HalError> {
            let r = self.inner.emergency_stop();
            let w = self.inner.take_warnings();
            let mut p = self.probe.lock().unwrap();
            p.stray_warnings.extend(w);
            p.running = false;
            r
        }
    }

    /// The cycles that were PLAYS (the bring-up park is not one).
    fn plays(p: &Probe) -> Vec<Cycle> {
        p.cycles
            .iter()
            .filter(|c| c.arm_to_start.is_some())
            .cloned()
            .collect()
    }

    // -----------------------------------------------------------------------
    // Small shared helpers
    // -----------------------------------------------------------------------

    fn peak(v: &[f64]) -> f64 {
        v.iter().fold(0.0f64, |m, &x| m.max(x.abs()))
    }

    /// min / median / max in milliseconds.
    fn spread_ms(mut v: Vec<Duration>) -> (f64, f64, f64) {
        v.sort();
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        (ms(v[0]), ms(v[v.len() / 2]), ms(v[v.len() - 1]))
    }

    /// The section's synthetic field record: four tones inside the correction
    /// band under a `sin^2` envelope, normalized to an EXACT peak.
    ///
    /// Sparse-spectrum on purpose. The trust mask excludes bins more than
    /// `law.ref_floor_db` (50 dB) below the in-band target peak, and
    /// `law.min_masked_energy_frac` (0.9) aborts a run whose reference content
    /// lies outside what the mask covers - so a reference whose energy is in a
    /// few narrow lobes well inside the band puts the coverage guard far from
    /// its threshold and keeps ROW 3 a test of the CONTROL LAW rather than of
    /// the coverage guard.
    fn reference(label: &str, seconds: f64, peak_v: f64, fs_hz: f64) -> ReferenceWaveform {
        let n = (seconds * fs_hz).round() as usize;
        let mut s: Vec<f64> = (0..n)
            .map(|k| {
                let t = k as f64 / fs_hz;
                let env = (std::f64::consts::PI * k as f64 / n as f64).sin().powi(2);
                let v: f64 = REFERENCE_TONES_HZ
                    .iter()
                    .map(|f| (std::f64::consts::TAU * f * t).sin())
                    .sum();
                env * v
            })
            .collect();
        let pk = peak(&s);
        if pk > 0.0 {
            let g = peak_v / pk;
            for v in s.iter_mut() {
                *v *= g;
            }
        }
        ReferenceWaveform {
            label: label.into(),
            fs_hz,
            samples: s,
            eu: "V".into(),
            source: ImportSource::Synthesized(label.into()),
        }
    }

    /// The section's sysID config. `min_mean_coherence` is the caller's, because
    /// the two uses differ: the INSTRUMENT (phase A) wants the number even when
    /// it is bad, the SERVICE wants its shipped hold.
    fn sysid_cfg(drive_rms: f64, min_mean_coherence: f64) -> SysIdConfig {
        SysIdConfig {
            drive_rms,
            duration_s: SYSID_DURATION_S,
            f_lo_hz: SYSID_BAND_HZ.0,
            f_hi_hz: SYSID_BAND_HZ.1,
            min_mean_coherence,
            ..SysIdConfig::default()
        }
    }

    /// The section's base config around a reference: sysID band, correction
    /// band, and the ceiling installed as the run-time protection.
    fn base_cfg(reference: ReferenceWaveform, sysid_drive_rms: f64, ceiling: f64) -> TwrConfig {
        let mut cfg = TwrConfig::for_reference(reference);
        // The SHIPPED hold threshold (0.8) - the gate's own floor is tighter
        // and lives in ROW 1, which is the point: a gate that relaxed the
        // module's safety default to make its rows easier would be measuring a
        // configuration nobody ships.
        cfg.sysid = sysid_cfg(sysid_drive_rms, SysIdConfig::default().min_mean_coherence);
        cfg.law.f_lo_hz = LAW_BAND_HZ.0;
        cfg.law.f_hi_hz = LAW_BAND_HZ.1;
        cfg.limits.max_drive_vpk = Some(ceiling);
        cfg.limits.drive_clip = clip_v();
        cfg
    }

    // -----------------------------------------------------------------------
    // THE GROUP-DELAY DECOMPOSITION (ROW 2's second authority)
    // -----------------------------------------------------------------------

    /// The terms that separate the commanded drive from where it lands in the
    /// captured record, in frames at the configured rate.
    #[derive(Debug, Clone, Copy)]
    struct LagModel {
        /// **Term 1 - the ADC group delay**, as the DRIVER stated it in
        /// `ConfiguredActual::group_delay_frames`. The 9234's delta-sigma input
        /// path delays every sample by `38.4 + 3.2e-6 * fs` samples (NI 9234
        /// Operating Instructions; `[H] PROVISIONAL`, paper-derived). Read off
        /// the configure ANSWER rather than recomputed here, so the model and
        /// the device cannot drift apart.
        adc_group_delay: i64,
        /// **Term 2 - silence ahead of the record. ZERO by construction under
        /// the per-play cycle, and this term exists to say so.**
        ///
        /// A non-regenerating NI AO refuses to start on an empty buffer (-200462), so
        /// `NiBackend::start()` tops the buffer up to its lookahead with SILENCE. That
        /// top-up goes into a FIFO *behind* whatever is already queued, so it delays
        /// the leading edge only when the queue was EMPTY. The per-play cycle's whole
        /// point is that it never is: the player commits real drive before `start()`
        /// (`commit_what_fits`), and [`Cycle::committed_frames`] is the observable that
        /// proves the head was drive and not silence. The counterfactual is printed
        /// beside the model, because the number that WOULD be here - the whole
        /// lookahead - is the head-silence class this design exists to close.
        head_silence: i64,
        /// The red-mutation skew, normally 0.
        skew: i64,
        /// `adc_group_delay + head_silence + skew`. POSITIVE: the response
        /// TRAILS the drive, and `measure_lag_xcorr` searches non-negative lags
        /// only (a response cannot precede its cause).
        lag: i64,
    }

    fn lag_model(group_delay_frames: i64, committed_frames: usize, lookahead: usize) -> LagModel {
        let head_silence = if committed_frames == 0 {
            lookahead as i64
        } else {
            0
        };
        let skew = env_f64("HERTZFORGE_T2_ALIGN_SKEW").unwrap_or(0.0) as i64;
        LagModel {
            adc_group_delay: group_delay_frames,
            head_silence,
            skew,
            lag: group_delay_frames + head_silence + skew,
        }
    }

    // -----------------------------------------------------------------------
    // THE DMM HAND-OFF (drive-side evidence)
    // -----------------------------------------------------------------------

    /// `HERTZFORGE_T2_DMM=1` - the plays the DMM witnesses BLOCK until
    /// `tools/probes/shock_dmm.py` has its digitize window open.
    ///
    /// PLAY-GATED, never window-gated: `p050_dmm.py`'s two modes both judge
    /// against a live-drive MEDIAN and refuse a verdict when it falls below
    /// their floor, which is correct for a continuous Random drive and
    /// structurally wrong here - a TWR play is 1.3 s inside a 5 s window with
    /// silence either side, so the median of the capture is the silence and
    /// every correct capture would come back INCONCLUSIVE. `shock_dmm.py`
    /// judges on PEAK against an absolute threshold, which is the question this
    /// design can answer.
    ///
    /// A wall time and a DMM verdict are evidence for a HUMAN, never predicates
    /// this process can evaluate, so these are INFO lines and never rows (the
    /// `sine_rig::dmm_mark` convention).
    fn dmm_enabled() -> bool {
        env_flag("HERTZFORGE_T2_DMM")
    }

    /// `HERTZFORGE_SHOCK_TRIGGER`, else `shock_go` in the OS temp directory: `/tmp` on
    /// Linux with `TMPDIR` unset (the same `/tmp/shock_go` as before), `%TEMP%` on
    /// Windows. The DMM helper must create the file at the same path, or set the
    /// variable for both.
    fn dmm_trigger() -> std::path::PathBuf {
        std::env::var_os("HERTZFORGE_SHOCK_TRIGGER")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("shock_go"))
    }

    /// Block until the DMM's window is open, up to 60 s. With `HERTZFORGE_T2_DMM`
    /// unset this returns immediately and the play runs with no drive-side
    /// witness, which the caller states in its own output.
    fn wait_for_dmm(label: &str) {
        if !dmm_enabled() {
            return;
        }
        let path = dmm_trigger();
        println!(
            "  DMM     {label}: waiting for {} (run `shock_dmm.py --expect present` on the \
             box NOW)",
            path.display()
        );
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if path.exists() {
                println!("  DMM     {label}: window open, playing");
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        println!(
            "  DMM     {label}: TIMED OUT waiting for the trigger; playing anyway with NO \
             drive-side witness"
        );
    }

    /// The inter-play REST hold: park long enough for a 5 s `--expect absent`
    /// capture to fit inside a gap that is naturally ~100 ms.
    ///
    /// The natural inter-play gap under the per-play cycle is one correction pass -
    /// far too short to point a blocking 5 s digitize at - so the gap is WIDENED
    /// deliberately and its width is printed. The tasks are parked and unreserved
    /// for the whole hold, and the last frame the device clocked was a bit-exact
    /// 0.0 from the record's exact-zero rest pad, so AO0 must read 0.000 V for the
    /// entire window. That is the drive-side proof of the rest-at-zero claim; the
    /// AI cannot testify to it, because between plays there is no AI task.
    fn dmm_rest_hold(label: &str) {
        if !dmm_enabled() {
            return;
        }
        let hold = Duration::from_secs(8);
        println!(
            "  DMM     {label}: tasks PARKED + UNRESERVED for {:.0} s - run `shock_dmm.py \
             --expect absent` NOW; AO0 must read 0.000 V for the whole window (rest-at-zero). \
             Expected quiet: the full {:.0} s, bounded by this hold.",
            hold.as_secs_f64(),
            hold.as_secs_f64()
        );
        std::thread::sleep(hold);
    }

    // =======================================================================
    // THE SECTION
    // =======================================================================

    pub fn twr(rows: &mut Rows) {
        println!();
        println!("--- TWR (chassis gate, NUMERIC) ---");
        mutation_guard_row(rows);
        println!(
            "  INFO    bench contract: AO0 -> AI0 jumpered, nothing downstream, IEPE OFF; \
             every commanded drive capped at {BENCH_MAX_DRIVE_V:.2} V peak by \
             TwrLimits::max_drive_vpk, which is the SERVICE's own ceiling and not a \
             gate-local stand-in"
        );
        // The EFFECTIVE thresholds, printed before anything runs. Two jobs: the
        // sitting log records what the rows actually gated on, and the
        // tighten-only clamps on the mutation hooks become OBSERVABLE - a hook
        // asked to LOOSEN a threshold leaves the shipped number standing here,
        // which is the only way to see that the clamp held.
        println!(
            "  INFO    effective thresholds: coherence floor {:.4} (shipped \
             {SYSID_COHERENCE_FLOOR:.4}), convergence gate {:.4}% (shipped \
             {CONVERGENCE_GATE_PCT:.4}%), cycle bound {:.0} ms (shipped {:.0} ms), phase-D \
             ceiling {:.4} V (shipped {CEILING_V:.4} V), clip {:?}",
            coherence_floor(),
            gate_pct(),
            cycle_bound().as_secs_f64() * 1e3,
            CYCLE_SANITY_BOUND.as_secs_f64() * 1e3,
            ceiling_v(),
            clip_v()
        );
        phase_a(rows);
        phase_b(rows);
        phase_c(rows);
        phase_d(rows);
        phase_e(rows);
    }

    // -----------------------------------------------------------------------
    // PHASE A - ROWS 1 + 2, both off one sysID-shaped play on a BARE player.
    //
    // Bare rather than through the service because the two numbers ROW 2 needs
    // - `pipeline_lag_frames()` and `lag_source()` - are player-level and
    // `TwrService::player()` is `pub(crate)`. Running the pass here also means
    // ROW 1's coherence and ROW 2's alignment describe the SAME record, which
    // matters: a coherence number is only interpretable next to the alignment
    // that produced it (the 2026-07-30 chassis read 0.010 purely because of a
    // misalignment, and nothing in that row said so).
    // -----------------------------------------------------------------------
    fn phase_a(rows: &mut Rows) {
        println!();
        println!("  -- phase A: sysID alignment + coherence (bare player) --");
        let st = station();
        let caps = st.backend.caps();
        let lookahead = caps
            .output_buffer
            .map(|c| c.min_lookahead_frames as usize)
            .unwrap_or(0);
        // The rate the record is BUILT against: caps before `open()` is the
        // request, and stating it is what keeps the arm gate's rate arm from
        // being vacuous. The service does exactly this.
        let build_rate = caps.sample_rate_hz;
        let (backend, probe) = ProbedBackend::new(st.backend);
        let mut player = match BackendReferencePlayer::new(backend, volts_table(), st.station) {
            // `with_quiet_frames` is `with_flush_samples`'s current name. The
            // 1024 frames did not move - their POSITION did: the window used to
            // be a FLUSH ahead of the record, where it pushed the leading edge
            // behind a lookahead of silence (the head-silence defect), and it is now a
            // quiet ring-down TAIL. The pad the player actually applies is
            // `max(lookahead + block, quiet)`, printed below, so on this
            // chassis the 1024 is not even the binding term.
            Ok(p) => p.with_quiet_frames(1024).with_reference_rate_hz(build_rate),
            Err(e) => {
                rows.row(false, "twr phase A player construct", format!("{e}"));
                return;
            }
        };
        if let Err(e) = player.arm() {
            rows.row(false, "twr phase A open_and_configure", format!("{e}"));
            return;
        }
        let fs = player.fs_hz();
        let configured = probe.lock().unwrap().configured;
        let Some(conf) = configured else {
            rows.row(
                false,
                "twr phase A captured ConfiguredActual",
                "the probe saw no configure() answer, so ROW 2 has no second authority".into(),
            );
            return;
        };
        println!(
            "  INFO    configured: requested {build_rate:.4} Hz -> ACTUAL {:.4} Hz, block {} \
             frames, group delay {:?} frames; AO lookahead {lookahead} frames, rest pad {} \
             frames",
            conf.actual_rate_hz,
            conf.block_frames,
            conf.group_delay_frames,
            player.rest_pad_frames()
        );

        // The ranging pass. `min_mean_coherence` is dropped to 0.01 HERE and
        // only here: `SysIdConfig::estimate` returns an Err instead of a number
        // when the mean falls below the floor, and a row that can only report
        // "it was bad" cannot report HOW bad. The gate's own floor is
        // [`coherence_floor`], applied below.
        let cfg = sysid_cfg(SYSID_DRIVE_RMS, 0.01);
        let exc = cfg.build_excitation(fs);
        let exc_peak = peak(&exc);
        if exc_peak > BENCH_MAX_DRIVE_V {
            rows.row(
                false,
                "twr sysID excitation inside the bench cap",
                format!(
                    "excitation peak {exc_peak:.4} V exceeds the {BENCH_MAX_DRIVE_V:.2} V bench \
                     cap - REFUSED before anything played"
                ),
            );
            return;
        }
        println!(
            "  INFO    ranging: {:.1} s at {SYSID_DRIVE_RMS:.3} V rms ({exc_peak:.4} V peak), \
             band {:.0}-{:.0} Hz, nfft {}",
            cfg.duration_s, cfg.f_lo_hz, cfg.f_hi_hz, cfg.nfft
        );
        wait_for_dmm("phase A ranging play");
        let response = match player.play(&exc) {
            Ok(Some(y)) => y,
            Ok(None) => {
                rows.row(
                    false,
                    "twr phase A ranging play returned a response",
                    "the path reported no observable response".into(),
                );
                return;
            }
            Err(e) => {
                rows.row(false, "twr phase A ranging play", format!("{e}"));
                return;
            }
        };
        dmm_rest_hold("phase A post-play rest");

        let committed = {
            let p = probe.lock().unwrap();
            for w in p.stray_warnings.iter() {
                println!("  INFO    driver warning (outside a cycle): {w}");
            }
            for (i, c) in p.cycles.iter().enumerate() {
                for w in c.warnings.iter() {
                    println!("  INFO    driver warning (cycle {i}): {w}");
                }
            }
            plays(&p).last().map(|c| c.committed_frames).unwrap_or(0)
        };

        // --- ROW 2: ALIGNMENT, measured against the decomposition. ----------
        // Two authorities, and they must be named on the output because the
        // whole value of the row is which one moved. Authority A is the
        // MEASUREMENT: `measure_lag_xcorr` over the played record, accepted
        // only above the `4.1/sqrt(n)` noise floor. Authority B is the DRIVER:
        // `ConfiguredActual::group_delay_frames` plus the head-silence term the
        // per-play commit drives to zero. Neither reads the other.
        let measured = player.pipeline_lag_frames() as i64;
        let source = player.lag_source();
        let model = lag_model(
            conf.group_delay_frames.unwrap_or(0) as i64,
            committed,
            lookahead,
        );
        let residual = measured - model.lag;
        println!(
            "  MODEL   lag decomposition at fs = {:.4} Hz:",
            conf.actual_rate_hz
        );
        println!(
            "            ADC group delay    = {} frames   [ConfiguredActual::group_delay_frames \
             = ceil(38.4 + 3.2e-6 x fs); NI 9234 OI&S, [H] PROVISIONAL]",
            model.adc_group_delay
        );
        println!(
            "            head silence       = {} frames   [per-play cycle: {committed} frames of \
             REAL drive were committed before start(), so start()'s top-up landed BEHIND the \
             record; had the commit been 0 this term would be the whole {lookahead}-frame \
             lookahead]",
            model.head_silence
        );
        if model.skew != 0 {
            println!(
                "            MUTATION SKEW      = {} frames   [HERTZFORGE_T2_ALIGN_SKEW]",
                model.skew
            );
        }
        println!(
            "            MODEL LAG          = {} frames; MEASURED (xcorr) = {measured} frames, \
             source {source:?}; RESIDUAL = {residual:+} frames",
            model.lag
        );
        rows.row(
            source == LagSource::Measured && residual.abs() <= ALIGN_TOL_FRAMES,
            "twr alignment: MEASURED xcorr lag agrees with the group-delay decomposition",
            format!(
                "measured {measured} frames [authority: xcorr over the ranging record, \
                 source {source:?}] vs model {} frames [authority: the DRIVER - \
                 ConfiguredActual::group_delay_frames {} + head silence {} + skew {}], \
                 residual {residual:+} frames against a +/-{ALIGN_TOL_FRAMES}-frame band. \
                 The band admits the UNMODELED 9263 analog-output path delay (shock measured \
                 +2 frames for the same unattributed term on this chassis) and is still ~250x \
                 tighter than every failure this row exists to catch: one block \
                 ({}), the lookahead ({lookahead}), the AO buffer, and the 2026-07-30 \
                 2106-frame miss. A `DerivedNoPeak` source FAILS the row outright - a \
                 measurement that declined to happen is not an alignment.",
                model.lag, model.adc_group_delay, model.head_silence, model.skew, conf.block_frames
            ),
        );

        // --- ROW 1: sysID IN-BAND MEAN COHERENCE vs the derived floor. ------
        // Authorities: the MEASURED Welch Hv coherence over the acquired
        // response vs a floor derived from the 9263's LSB, the excitation's
        // in-band rms and the estimator's segment length (see
        // SYSID_COHERENCE_FLOOR). The floor is arithmetic off datasheets; the
        // measurement is the wire. Neither reads the other.
        let floor = coherence_floor();
        match cfg.estimate(&exc, &response, fs) {
            Ok(sysid) => {
                let coh = sysid.in_band_mean_coherence;
                rows.row(
                    coh >= floor,
                    "twr sysID in-band mean coherence at or above the derived floor",
                    format!(
                        "measured mean in-band coherence {coh:.7} over {:.0}-{:.0} Hz \
                         (nfft {}, Hann, 50% overlap) vs the {floor:.3} floor derived from \
                         this rig: 16-bit AO over +/-10 V = 305 uV LSB -> 24 uV in-band \
                         quantization noise against {SYSID_DRIVE_RMS:.3} V rms of drive = \
                         66 dB in-band SNR, coherence ceiling 1 - 2.3e-7; residual \
                         sub-frame alignment inside a 2048-point Hann segment costs under \
                         1e-6. The plant is a WIRE, so this says the excitation and the \
                         acquisition agree - it says NOTHING about a shaker. The alignment \
                         row above is the discriminating observation for any red here.",
                        cfg.f_lo_hz, cfg.f_hi_hz, cfg.nfft
                    ),
                );
            }
            Err(e) => rows.row(
                false,
                "twr sysID in-band mean coherence at or above the derived floor",
                format!("the estimator refused: {e}"),
            ),
        }
    }

    // -----------------------------------------------------------------------
    // PHASE B - ROWS 3, 4 and 7: the full ladder to Complete, on a record ten
    // times the AO buffer, with the task-cycle cost instrumented.
    // -----------------------------------------------------------------------
    fn phase_b(rows: &mut Rows) {
        println!();
        println!("  -- phase B: full-ladder convergence, over-buffer streaming, cycle cost --");
        let st = match try_station() {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "twr phase B station", e);
                return;
            }
        };
        let caps = st.backend.caps();
        let lookahead = caps
            .output_buffer
            .map(|c| c.min_lookahead_frames as usize)
            .unwrap_or(0);
        let build_rate = caps.sample_rate_hz;
        let (backend, probe) = ProbedBackend::new(st.backend);
        let mut cfg = base_cfg(
            reference("t2-ladder", LADDER_SECONDS, LADDER_TARGET_V, build_rate),
            SYSID_DRIVE_RMS,
            BENCH_MAX_DRIVE_V,
        );
        // AUTO is the shipped mode and the one the row needs: ROW 3 asserts the
        // LADDER completed, which means every rung gated and the schedule
        // walked itself to full level. A manual ladder would prove one rung.
        cfg.schedule.mode = TwrRunMode::Auto;
        cfg.schedule.gate_rms_error_pct = gate_pct();
        let gate = cfg.schedule.gate_rms_error_pct;
        let rungs = cfg.schedule.table_db.clone();

        let player = match BackendReferencePlayer::new(backend, volts_table(), st.station) {
            Ok(p) => p.with_quiet_frames(1024),
            Err(e) => {
                rows.row(false, "twr phase B player construct", format!("{e}"));
                return;
            }
        };
        let mut svc = match TwrService::new(player, cfg) {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "twr phase B service construct", format!("{e}"));
                return;
            }
        };
        let record_frames = svc.grid().n;
        println!(
            "  INFO    ladder {:?} dB, gate {gate:.3}% rms, record {record_frames} frames \
             ({:.3} s at {build_rate:.0} Hz), target peak {LADDER_TARGET_V:.2} V, ceiling \
             {BENCH_MAX_DRIVE_V:.2} V",
            rungs,
            record_frames as f64 / build_rate
        );
        wait_for_dmm("phase B first play");
        let outcome = svc.run_to_completion(64);
        let status = svc.status();
        let state = svc.state();
        let records = svc.history().records.len();
        let final_rms = status.last_metrics.map(|m| m.rms_error_pct);

        // --- ROW 3: FULL-LADDER CONVERGENCE. --------------------------------
        // Authorities: the SERVICE's terminal state and ladder position vs the
        // MEASURED error of the last play against the target. The state arm is
        // load-bearing rather than decorative: `run_to_completion` returns `Ok`
        // from `Holding`, `FeasibilityBlocked` and `CoherenceHold`,
        // so a park and a converged run are the same `Ok` and only the state
        // tells them apart. Every park FAILS this row and the detail names it.
        let converged = matches!(outcome, Ok(TwrState::Complete))
            && state == TwrState::Complete
            && records > 0
            && final_rms.is_some_and(|r| r <= gate)
            && status.abort_reason.is_none()
            && status.clipped_frac == 0.0
            && status.ladder_completed;
        rows.row(
            converged,
            "twr FULL LADDER converged: state Complete, ladder completed, error inside the gate",
            format!(
                "outcome {outcome:?}, state {state:?}, ladder_completed {}, level {:.1} dB, \
                 {records} convergence record(s), {} correction iteration(s), final rms error \
                 {} vs the {gate:.3}% gate, abort_reason {:?}, clipped_frac {:.6}. \
                 A park (Holding / FeasibilityBlocked / CoherenceHold) reaches this row as \
                 Ok and is what the state assert exists to catch. NOTE on clipped_frac: with \
                 the SHIPPED `drive_clip: None` the clip counter is structurally zero, so \
                 that arm is a REGRESSION GUARD on the default (arm HERTZFORGE_T2_CLIP_V to see it \
                 move); the live drive-limit evidence is the ceiling row in phase D.",
                status.ladder_completed,
                status.level_db,
                status.total_iterations,
                final_rms
                    .map(|r| format!("{r:.4}%"))
                    .unwrap_or_else(|| "NONE (no play produced metrics)".into()),
                status.abort_reason,
                status.clipped_frac,
            ),
        );

        let p = probe.lock().unwrap();
        for w in p.stray_warnings.iter() {
            println!("  INFO    driver warning (outside a cycle): {w}");
        }
        let played = plays(&p);
        let ao_buffer = p
            .configured
            .map(|c| lookahead + c.block_frames)
            .unwrap_or(lookahead);

        // --- ROW 4: OVER-BUFFER STREAMING WITH ZERO UNDERRUN WARNINGS. ------
        // Authorities: the DRIVER's own diagnostics (`Backend::take_warnings`,
        // drained after every task cycle) vs the arithmetic of the record
        // against the device's AO buffer. An earlier claim that the per-play
        // cycle removes the underrun class was withdrawn: it removes it for the
        // COMMITTED head and leaves it for the streaming window, so this row
        // must first prove the streaming window EXISTED (every play committed
        // strictly less than it wrote) and only then that nothing complained.
        // A row over a record that fit in the buffer would be a green that
        // could not have gone red.
        let warned: Vec<String> = played.iter().flat_map(|c| c.warnings.clone()).collect();
        let shortest = played.iter().map(|c| c.written_frames).min().unwrap_or(0);
        let all_streamed =
            !played.is_empty() && played.iter().all(|c| c.written_frames > c.committed_frames);
        let over_buffer = ao_buffer > 0 && shortest >= OVER_BUFFER_MULTIPLE * ao_buffer;
        for (i, c) in played.iter().enumerate() {
            println!(
                "  INFO    play {i}: wrote {} frames, {} committed before start(), {} STREAMED \
                 after it; {} driver warning(s)",
                c.written_frames,
                c.committed_frames,
                c.written_frames.saturating_sub(c.committed_frames),
                c.warnings.len()
            );
        }
        for w in warned.iter() {
            println!("  WARN    driver said, verbatim: {w}");
        }
        rows.row(
            over_buffer && all_streamed && warned.is_empty(),
            "twr over-buffer streaming: every play streamed, ZERO driver underrun warnings",
            format!(
                "{} play(s); shortest wrote {shortest} frames against a {ao_buffer}-frame AO \
                 buffer (lookahead {lookahead} + one block) = {:.1}x, floor \
                 {OVER_BUFFER_MULTIPLE}x [authority: the device's own OutputBufferContract]; \
                 every play committed strictly less than it wrote, so the streaming window is \
                 real; {} driver warning(s) from Backend::take_warnings() drained after each \
                 cycle [authority: the DAQmx driver]{}",
                played.len(),
                if ao_buffer > 0 {
                    shortest as f64 / ao_buffer as f64
                } else {
                    0.0
                },
                warned.len(),
                if warned.is_empty() {
                    String::new()
                } else {
                    format!(" - verbatim: {}", warned.join(" | "))
                }
            ),
        );

        // --- ROW 7: PER-PLAY TASK-CYCLE COST. -------------------------------
        // Informational WITH A FLOOR. The numbers are the deliverable: the
        // watchdogs must be derived from measured cadence rather than guessed,
        // and the design arms two separate deadlines - a driver beat while a play
        // is in flight and a COMPUTE deadline between plays - whose budgets
        // start from exactly these figures. The row only fails when a single
        // cycle is so long the per-play shape is not viable.
        let bound = cycle_bound();
        if played.is_empty() {
            rows.row(
                false,
                "twr per-play task-cycle cost inside the sanity bound",
                "no plays were instrumented".into(),
            );
        } else {
            let (a_min, a_med, a_max) =
                spread_ms(played.iter().map(|c| c.arm_to_start.unwrap()).collect());
            let (s_min, s_med, s_max) = spread_ms(played.iter().map(|c| c.stop).collect());
            let worst = played
                .iter()
                .map(|c| c.arm_to_start.unwrap().max(c.stop))
                .max()
                .unwrap_or_default();
            let bringup = p
                .cycles
                .iter()
                .find(|c| c.arm_to_start.is_none())
                .map(|c| format!("{:.1} ms", c.stop.as_secs_f64() * 1e3))
                .unwrap_or_else(|| "not seen".into());
            rows.row(
                worst <= bound,
                "twr per-play task-cycle cost inside the sanity bound",
                format!(
                    "arm->commit->start over {} plays: min {a_min:.1} / median {a_med:.1} / \
                     max {a_max:.1} ms. stop->unreserve: min {s_min:.1} / median {s_med:.1} / \
                     max {s_max:.1} ms. Worst single cycle {:.1} ms against a {:.0} ms sanity \
                     bound. Bring-up park (arm+stop, no start): {bringup}. THESE NUMBERS ARE \
                     THE DELIVERABLE - the driver beat and its separate between-plays \
                     compute deadline are budgeted from them. Measured wall time here \
                     [authority: this process's monotonic clock] against a stated bound \
                     [authority: the stated viability threshold].",
                    played.len(),
                    worst.as_secs_f64() * 1e3,
                    bound.as_secs_f64() * 1e3
                ),
            );
        }
    }

    // -----------------------------------------------------------------------
    // PHASE C - ROW 5: the rate refusal. NOTHING IS DRIVEN.
    // -----------------------------------------------------------------------
    fn phase_c(rows: &mut Rows) {
        println!();
        println!("  -- phase C: rate refusal (can-fail row; nothing is driven) --");
        let st = match try_station() {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "twr phase C station", e);
                return;
            }
        };
        let mut player = match BackendReferencePlayer::new(st.backend, volts_table(), st.station) {
            Ok(p) => p,
            Err(e) => {
                rows.row(false, "twr phase C player construct", format!("{e}"));
                return;
            }
        };
        // THE RED-MUTATION, in one line: `Some -> None` on the reference rate.
        // `arm_gate` SKIPS its rate check entirely on `None`, so with the hook
        // set the arm succeeds and this row goes red - loudly, not vacuously.
        // That is the whole point of the row: at one time nothing in the tree
        // called `with_reference_rate_hz` outside two unit tests, so on the
        // chassis the rate arm compared the device to itself and a reference at
        // a rate the 9234 will not program would have played at the wrong speed
        // with every frequency shifted and nothing refusing.
        if !env_flag("HERTZFORGE_T2_DROP_REF_RATE") {
            player = player.with_reference_rate_hz(OFF_LADDER_RATE_HZ);
        }
        let outcome = player.arm();
        let actual = player.actual_fs_hz();
        let msg = match &outcome {
            Err(e) => format!("{e}"),
            Ok(()) => "ACCEPTED (nothing refused the arm)".to_string(),
        };
        let quotes_both = actual.is_some_and(|a| {
            msg.contains(&format!("{OFF_LADDER_RATE_HZ}")) && msg.contains(&format!("{a}"))
        });
        // `UnsupportedReason::RateMismatch` has TWO Display arms and the gap
        // decides which: inside the regeneration band it offers a regenerate,
        // outside it calls the mismatch a configuration error. 8192 against
        // 51 200 is far outside, so the second arm is expected - but the row
        // accepts either, because which arm fires is the policy's business and
        // not this row's question.
        let named = (msg.contains("the profile was generated at")
            && msg.contains("Hertzforge never resamples"))
            || msg.contains("Regenerate the profile at the actual rate");
        println!(
            "  INFO    the 9234's sample clock is 13.1072 MHz / (256 n): \
             13107200 / (256 x {OFF_LADDER_RATE_HZ}) = 6.25 is not an integer, so this device \
             cannot program {OFF_LADDER_RATE_HZ} Hz at all - asked for it, it coerces to n = 6 \
             = 8533.33 Hz. The refusal is therefore not merely about the CURRENT \
             configuration; no reconfiguration would rescue this record."
        );
        rows.row(
            outcome.is_err() && named && quotes_both,
            "twr REFUSES a reference stated at a rate the device will not program, by name",
            format!(
                "reference record stamped {OFF_LADDER_RATE_HZ} Hz [authority: the record's own \
                 stated rate] against a device configured at {} Hz [authority: \
                 ConfiguredActual, read back through actual_fs_hz()]; arm -> {msg}. \
                 The row requires the refusal to QUOTE BOTH NUMBERS, so a generic \"arm \
                 refused\" cannot satisfy it. RED-MUTATION: HERTZFORGE_T2_DROP_REF_RATE=1 drops \
                 the `with_reference_rate_hz` call, arm_gate then skips the rate check on \
                 None, the arm succeeds and this row goes red.",
                actual
                    .map(|a| format!("{a}"))
                    .unwrap_or_else(|| "UNKNOWN".into())
            ),
        );
    }

    // -----------------------------------------------------------------------
    // PHASE D - ROW 6: the ceiling refusal. The RUN aborts; the SWEEP does not.
    // -----------------------------------------------------------------------
    fn phase_d(rows: &mut Rows) {
        println!();
        println!("  -- phase D: drive-ceiling refusal --");
        let st = match try_station() {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "twr phase D station", e);
                return;
            }
        };
        let build_rate = st.backend.caps().sample_rate_hz;
        let ceiling = ceiling_v();
        let mut cfg = base_cfg(
            reference("t2-ceiling", 0.25, CEILING_TARGET_V, build_rate),
            CEILING_SYSID_DRIVE_RMS,
            ceiling,
        );
        // ONE rung, at full level, so the FIRST corrected drive is the full
        // inverse demand and the refusal is deterministic rather than reached
        // several rungs up an auto ladder.
        cfg.schedule.mode = TwrRunMode::Manual;
        cfg.schedule.table_db = vec![0.0];

        // THE PRECONDITION, checked before anything plays: the ranging
        // excitation must fit UNDER the ceiling, or the refusal this row
        // reports would be about the excitation and not about the inverse. The
        // excitation is deterministic (`ColaSynth::with_seed`), so this is the
        // exact record the service will build, not an estimate.
        let exc_peak = peak(&cfg.sysid.build_excitation(build_rate));
        if exc_peak >= ceiling {
            rows.row(
                false,
                "twr ceiling refusal comes from the INVERSE, not the ranging excitation",
                format!(
                    "ranging excitation peak {exc_peak:.4} V already reaches the \
                     {ceiling:.4} V ceiling - the row would refuse on the wrong record"
                ),
            );
            return;
        }
        println!(
            "  INFO    ranging peak {exc_peak:.4} V < ceiling {ceiling:.4} V < target peak \
             {CEILING_TARGET_V:.2} V: sysID completes, the first corrected drive does not"
        );

        let player = match BackendReferencePlayer::new(st.backend, volts_table(), st.station) {
            Ok(p) => p.with_quiet_frames(1024),
            Err(e) => {
                rows.row(false, "twr phase D player construct", format!("{e}"));
                return;
            }
        };
        let mut svc = match TwrService::new(player, cfg) {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "twr phase D service construct", format!("{e}"));
                return;
            }
        };
        wait_for_dmm("phase D ranging play");
        // The refusal is the row's EXPECTED outcome, so it is caught here and
        // scored - `abort_with` returns `Ok(Terminal(Aborted))`, so this never
        // propagates out of the section and the remaining phases still run.
        let outcome = svc.run_to_completion(8);
        let status = svc.status();
        let reason = status.abort_reason.clone().unwrap_or_default();
        let names_both = reason.contains("exceeds the max drive ceiling")
            && reason.contains(&format!("{ceiling:.4}"))
            && reason.contains("REFUSED, not clipped");
        rows.row(
            svc.state() == TwrState::Aborted && names_both,
            "twr REFUSES a drive the inverse demands above the ceiling, naming both numbers",
            format!(
                "outcome {outcome:?}, state {:?}; abort_reason: {reason}. Two authorities: the \
                 REALIZED drive record's peak, measured after the irfft and before the player \
                 saw it [the module's own `check_drive_peak`], against the configured \
                 {ceiling:.4} V ceiling [the operator's authorization]. Shock's profile-time \
                 projection is deliberately NOT the model here - TWR's drive is an \
                 inverse-filter output and a plant notch makes it arbitrarily larger than the \
                 reference, so only the realized record can be checked. \
                 RED-MUTATION: HERTZFORGE_T2_CEILING_V={BENCH_MAX_DRIVE_V:.2} raises the ceiling \
                 above the demand, the drive plays, and this row goes red.",
                svc.state()
            ),
        );
    }

    // -----------------------------------------------------------------------
    // PHASE E - ROW 8: stop semantics. Cancel raised MID-PLAY from a thread.
    // -----------------------------------------------------------------------
    fn phase_e(rows: &mut Rows) {
        println!();
        println!("  -- phase E: stop semantics (cancel mid-play) --");
        let st = match try_station() {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "twr phase E station", e);
                return;
            }
        };
        let build_rate = st.backend.caps().sample_rate_hz;
        let mut cfg = base_cfg(
            reference("t2-stop", 0.25, LADDER_TARGET_V, build_rate),
            SYSID_DRIVE_RMS,
            BENCH_MAX_DRIVE_V,
        );
        // A 3 s ranging play gives the cancel a wide, unambiguous mid-play
        // window to land in.
        cfg.sysid.duration_s = 3.0;

        // The gate OWNS the flag, which is the production shape: the UI's
        // `twr_stop` holds it for the life of its glue so no lock stands
        // between an operator Stop and the flag being raised.
        let flag = Arc::new(AtomicBool::new(false));
        let player = match BackendReferencePlayer::new(st.backend, volts_table(), st.station) {
            Ok(p) => p.with_quiet_frames(1024).with_cancel_flag(flag.clone()),
            Err(e) => {
                rows.row(false, "twr phase E player construct", format!("{e}"));
                return;
            }
        };
        // THE AUTHORITY LINK. A thread raising some flag proves nothing unless
        // it is THE flag the service's own `stop()` verb raises; pointer
        // identity is the check, and it is what ties this row to the operator
        // path rather than to a private test channel.
        let same_flag = Arc::ptr_eq(&flag, &player.cancel_flag());
        let mut svc = match TwrService::new(player, cfg) {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "twr phase E service construct", format!("{e}"));
                return;
            }
        };
        let armed = !env_flag("HERTZFORGE_T2_NO_CANCEL");
        let raiser = if armed {
            let f = flag.clone();
            Some(std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1000));
                f.store(true, Ordering::Relaxed);
            }))
        } else {
            None
        };
        wait_for_dmm("phase E ranging play (cancel lands ~1 s in)");
        let outcome = svc.run_to_completion(64);
        if let Some(h) = raiser {
            let _ = h.join();
        }
        let state = svc.state();
        let status = svc.status();
        // "The task is down" asked of the DRIVER rather than of the service:
        // the service is STILL ALIVE and still owns its DAQmx handles, so a
        // second task on the same physical channels can only be created if the
        // stop path really did unreserve. A task left RESERVED fails this build
        // - the exact failure `shock_rig::try_station` was written for.
        let rebuild = try_station();
        let task_released = rebuild.is_ok();
        let rebuild_err = rebuild.err().unwrap_or_default();
        let ended_stopped = state == TwrState::Stopped && status.abort_reason.is_none();
        rows.row(
            same_flag && ended_stopped && task_released,
            "twr Stop mid-play ends STOPPED (never Aborted) and releases the task",
            format!(
                "cancel raised from another thread ~1.0 s into the ranging play{}; \
                 run_to_completion -> {outcome:?}; state {state:?}; abort_reason {:?}. \
                 Flag identity: the thread's Arc and the one `TwrService::stop()` raises are \
                 the same allocation = {same_flag}. Task release, asked of the DRIVER while \
                 the stopped service is STILL HOLDING its handles: a fresh task on the same \
                 physical channels {} [{}]. Stop is not an abort - the operator asked and no \
                 guard tripped - so `TwrError::Cancelled` maps to Stopped and the reason \
                 field stays empty. RED-MUTATION: HERTZFORGE_T2_NO_CANCEL=1 never \
                 raises the flag, the run completes normally, and this row goes red.",
                if armed {
                    ""
                } else {
                    " [MUTATION: the raiser thread was not spawned]"
                },
                status.abort_reason,
                if task_released {
                    "BUILT"
                } else {
                    "was REFUSED"
                },
                if task_released {
                    "unreserved".to_string()
                } else {
                    rebuild_err
                }
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// 3 + 4. SHOCK - SRS and CLASSICAL SHOCK, the shock chassis gate.
//
// These two sections used to be lifecycle rows: build, arm, fire, and assert
// `store.len() > 0`. That assertion is what let the 2026-07-30 chassis run be
// written up as "RUNS ON REAL NI HARDWARE" while the pulse on the wire was
// almost certainly ~10x short (the service synthesized at 5120 Hz
// and the 9234 played at 51 200). A row that only asks whether a record came
// back cannot see amplitude, duration, alignment, tolerance, limiting, trains,
// stop or abort - so it saw none of them.
//
// Rebuilt to the EXPLORER bar: every row states a measured quantity, the
// authority it is being compared against, and the tolerance between them. The
// standing constraint is enforced row by row - NO GATE ROW
// WHOSE TWO SIDES TRACE TO THE SAME AUTHORITY. The pre-rework arm gate was
// exactly that (`backend.caps().sample_rate_hz` vs `ConfiguredActual`: the
// device compared to itself), and it is why this file's comments name both
// authorities on every row.
//
// Bench contract, the same one every other section carries: AO0 jumpered to
// AI0, NOTHING downstream, IEPE OFF (the 9263 cannot sink the 9234's 2.1 mA),
// every drive amplitude well inside +/-1 V on the wire. The shock sections cap
// their own commanded drive at [`shock_rig::BENCH_MAX_DRIVE_V`] and REFUSE to
// fire above it, because no drive-volts ceiling exists in either shock service
// yet (see the ceiling row's SKIP note).
// ---------------------------------------------------------------------------
fn srs(rows: &mut Rows) {
    shock_rig::srs(rows);
}

fn classical(rows: &mut Rows) {
    shock_rig::classical(rows);
}

mod shock_rig {
    use super::{ai_dev, ao_dev, station, tmp, Rows};
    use hertzforge_shock_common::{
        DriveMonitor, DriveMonitorKind, RateModel, ShockLevelSchedule, ShockRunMode, TrainSpec,
    };
    use hertzforge_shock_host::service::firer::{
        quiet_lead_frames, MIN_QUIET_LEAD_S, QUIET_LEAD_FRACTION_OF_PULSE,
    };
    use hertzforge_shock_host::{
        BackendPulseFirer, ClassicalPulseSpec, ClassicalShape, ClassicalShockService,
        ClassicalTestConfig, RunState, ToleranceStandard,
    };
    use hertzforge_station::{
        try_build_station_backend, BackendKind, NiSelection, SimPlantSpec, StationSpec,
        NI_BUILD_RATE_HZ,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    // -----------------------------------------------------------------------
    // Bench + profile constants
    // -----------------------------------------------------------------------

    /// The MAIN pulse this gate fires: the MIL-STD teaching case, 11 ms
    /// half-sine. On the VOLTS-EXACT table below its "g" IS a volt, so the
    /// 0 dB rung puts [`AMPLITUDE`] volts on the wire and the ladder's -12 dB
    /// start puts a quarter of that.
    const DURATION_MS: f64 = 11.0;
    /// Commanded MAIN-pulse peak. 0.5 V at 0 dB, half the bench ceiling.
    const AMPLITUDE: f64 = 0.5;

    /// The hard bench ceiling this gate enforces ON ITSELF, volts peak.
    ///
    /// **This is the gate standing in for a service that has none.** At the time of
    /// writing `eu_to_volts` has zero hits in the workspace, `fire()` writes
    /// g-valued samples straight into a volt-valued DAC, and NEITHER shock crate
    /// contains any drive-volts concept - so the binding 5 V max-drive default has
    /// no enforcement point in Shock today. Until `max_drive_vpk` lands on the wire
    /// profiles, the only thing between a mis-typed amplitude and the output node
    /// is this constant and the rows that check against it before firing.
    const BENCH_MAX_DRIVE_V: f64 = 0.5;

    /// Amplitude row tolerance: commanded volts vs measured volts.
    const AMPLITUDE_TOL: f64 = 0.05;
    /// Wall-clock rate assay tolerance (`<= 0.5%`).
    const RATE_ASSAY_TOL: f64 = 0.005;
    /// Train wall-clock spacing tolerance (`+/- 10%`).
    const TRAIN_SPACING_TOL: f64 = 0.10;
    /// Alignment row tolerance, FRAMES (`+/- 2 frames`). See
    /// [`delay_model`] for why the band is this tight and what the residual is.
    const ALIGN_TOL_FRAMES: isize = 2;
    /// Detection floor for the alignment cross-correlation, as a normalized
    /// correlation coefficient. Same value and same meaning as
    /// `examples/shock_shot_probe.rs`'s `PEAK_FLOOR_R`: it decides whether
    /// there is a peak to report at all, NOT whether the lag is right.
    const PEAK_FLOOR_R: f64 = 0.30;
    /// sysID in-band coherence floor for the SRS row. On a jumper the plant is
    /// a wire and coherence lands at ~1.0; anything below this is a broken
    /// excitation or a broken acquisition, not a hard plant.
    const SYSID_COHERENCE_FLOOR: f64 = 0.90;

    /// Commanded inter-pulse spacing for the train row, seconds. Above the
    /// conservative HostDac rate-model floor (1.0 s), so the plan realizes the
    /// commanded number rather than the floor.
    const TRAIN_SPACING_S: f64 = 2.0;
    /// Pulses in the train row.
    const TRAIN_COUNT: u32 = 5;
    /// Armed-idle dwell for the AI-side row, seconds.
    const IDLE_S: f64 = 30.0;
    /// Pulses the rate-model floor row measures its turnaround over.
    const FLOOR_PULSES: usize = 10;

    // -----------------------------------------------------------------------
    // The red-mutation hook
    // -----------------------------------------------------------------------

    /// `HERTZFORGE_S2_FORCE_5120=1` - THE RED-MUTATION, and nothing else.
    ///
    /// Set, every rate-denominated expectation in this module is derived from the
    /// profile's REQUESTED `fs_hz` (5120) instead of the rate the device adopted
    /// (51 200 on this chassis). That is the rate-divorce defect re-injected on the
    /// GATE's side of the comparison, and it exists for one reason: to demonstrate
    /// on the bench that these rows CAN go red (a result that cannot fail is not
    /// evidence).
    ///
    /// **What it reds, stated so the sitting knows what it is looking at:** the
    /// group-delay alignment row (the model's quiet-lead term is computed at
    /// the wrong rate: 29 frames instead of 282), the wall-clock rate assay
    /// (measured ~51 200 against an asserted 5120), the tolerance-window row
    /// (the MIL surround window is `0.4 x n_nominal` SAMPLES, so the wrong rate
    /// moves every boundary), and the PRINTED expected DMM width (scaled by
    /// `synthesis_rate / play_rate`, the physically correct counterfactual for
    /// a record built at one rate and played at another).
    ///
    /// **What it is NOT.** It does not re-inject the defect into the SERVICE - the
    /// service adopts the device's rate at arm and there is no external hook that
    /// unadopts it, and this gate does not edit service crates. So this proves the
    /// GATE can see a rate disagreement; it does not re-prove rate adoption. Stated
    /// rather than blurred.
    ///
    /// A mutated run is NOT a gate result: [`mutation_guard_row`] fails
    /// whenever this is set, so the sweep exits non-zero and a mutated log can
    /// never be filed as a pass.
    fn forced_5120() -> bool {
        std::env::var("HERTZFORGE_S2_FORCE_5120").as_deref() == Ok("1")
    }

    /// The rate every expectation in this module is denominated at: the ADOPTED
    /// rate in a real run, the REQUESTED rate under the red-mutation.
    fn denominating_hz(svc: &Svc) -> f64 {
        if forced_5120() {
            svc.requested_fs_hz()
        } else {
            svc.fs_hz()
        }
    }

    /// The row that makes a mutated sitting un-fileable. Runs once per section.
    fn mutation_guard_row(rows: &mut Rows, section: &str) {
        if !forced_5120() {
            return;
        }
        println!();
        println!("  ***********************************************************");
        println!("  *** HERTZFORGE_S2_FORCE_5120=1 - THIS IS A RED-MUTATION RUN.  ***");
        println!("  *** Every rate-denominated row below is derived from the ***");
        println!("  *** REQUESTED 5120 Hz, not the rate the device adopted.  ***");
        println!("  *** NOTHING in this run is gate evidence.                ***");
        println!("  ***********************************************************");
        rows.row(
            false,
            &format!("{section} runs UNMUTATED (HERTZFORGE_S2_FORCE_5120 unset)"),
            "the red-mutation is ON: this run demonstrates the timing rows can red, and is \
             NOT a gate result. Unset HERTZFORGE_S2_FORCE_5120 for the real sitting."
                .into(),
        );
    }

    // -----------------------------------------------------------------------
    // The DMM hand-off (drive-side evidence)
    // -----------------------------------------------------------------------

    /// `HERTZFORGE_S2_DMM=1` - the fires that the DMM witnesses BLOCK until
    /// `tools/probes/shock_dmm.py` has its digitize window open.
    ///
    /// Off by default so the sweep runs unattended. The trigger-file direction
    /// is `shock_dmm.py`'s, not `p050_run_driver.py`'s, and deliberately so: an
    /// 11 ms pulse cannot be positioned inside a 5 s window by hand, so the DMM
    /// creates the file immediately before `:READ:DIG?` blocks and the FIRING
    /// side waits on it.
    fn dmm_enabled() -> bool {
        std::env::var("HERTZFORGE_S2_DMM").as_deref() == Ok("1")
    }

    /// `HERTZFORGE_SHOCK_TRIGGER`, else `shock_go` in the OS temp directory: `/tmp` on
    /// Linux with `TMPDIR` unset (the same `/tmp/shock_go` as before), `%TEMP%` on
    /// Windows. The DMM helper must create the file at the same path, or set the
    /// variable for both.
    fn dmm_trigger() -> std::path::PathBuf {
        std::env::var_os("HERTZFORGE_SHOCK_TRIGGER")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("shock_go"))
    }

    /// Block until the DMM's window is open, up to 60 s. `true` = fire now.
    /// With `HERTZFORGE_S2_DMM` unset this returns immediately and the row runs
    /// without drive-side evidence, which is stated in its detail string.
    fn wait_for_dmm(label: &str) -> bool {
        if !dmm_enabled() {
            return true;
        }
        let path = dmm_trigger();
        println!("  DMM     {label}: waiting for {} (run `shock_dmm.py --expect present` on the box now)", path.display());
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if path.exists() {
                println!("  DMM     {label}: window open, firing");
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        println!("  DMM     {label}: TIMED OUT waiting for the trigger; firing anyway with NO drive-side witness");
        false
    }

    /// Print the wall-clock window an operator (or the main session) points the
    /// DMM at for a row whose evidence is drive-side only. A wall time is
    /// evidence for a human, never a predicate this process can evaluate - so
    /// this is an INFO line and never a row (the `sine_rig::dmm_mark`
    /// convention).
    fn dmm_window(label: &str, t0: Instant, t1: Instant) {
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let back = t1.elapsed().as_secs_f64();
        println!(
            "  DMM     {label}: window was unix {:.3} .. {:.3} ({:.3} s wide)",
            now_unix - back - (t1 - t0).as_secs_f64(),
            now_unix - back,
            (t1 - t0).as_secs_f64()
        );
    }

    // -----------------------------------------------------------------------
    // The station + the VOLTS-EXACT table
    // -----------------------------------------------------------------------

    /// A fresh REAL station that REFUSES instead of exiting.
    ///
    /// The file's shared `station()` exits 2 on a build failure, which is right
    /// for "there is no chassis" and wrong here: this gate builds eight
    /// stations, and the abort -> re-arm row exists precisely to catch a
    /// DAQmx task left RESERVED by a previous service. A reserved task makes
    /// the next build fail, and under `station()` that would kill the sweep
    /// instead of reddening the one row that is asking the question. So the
    /// section's FIRST station goes through `station()` (a genuinely absent
    /// chassis still exits 2) and every later one through this.
    fn try_station() -> Result<hertzforge_station::StationBackend, String> {
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
        let s = try_build_station_backend(BackendKind::Ni, &spec)
            .map_err(|e| format!("station REFUSED: {e:?}"))?;
        if s.provenance.driver_simulated == Some(true) {
            return Err(
                "station resolved DRIVER-SIMULATED - no shock row is evidence on a \
                        simulated device"
                    .into(),
            );
        }
        Ok(s)
    }

    /// The VOLTS-EXACT table, and the whole reason the units carry-forward from
    /// the baseline chassis re-probe is retired here.
    ///
    /// `eu = volts / (sensitivity_mv_per_eu / 1000)`, so `1000 mV/EU` makes the
    /// host's EU conversion the IDENTITY and every measured number below is a
    /// VOLT, directly comparable to the commanded volts. The file's shared
    /// `table()` helper uses 100 mV/EU, under which the same wire reads 10x
    /// high - correct for a lifecycle section, and the exact reason every pulse
    /// of the baseline re-probe tripped the tolerance abort at `error_frac` 8.28
    /// (measured "1.27 g" against a commanded 0.127). That was never a plant
    /// defect; it was the gate's own channel table.
    ///
    /// THE PSEUDO-g CONSEQUENCE, stated because it propagates. Both shock
    /// services are written in g throughout (g in, in/s for
    /// velocity change, inches for displacement, via `G_TO_IN_S2`). Under this
    /// table a "g" is a volt, so every kinematic number the services compute
    /// here - `peak_velocity_ips`, `pk_pk_disp_in`, the dV tolerance's
    /// `measured_dv_ips` and `nominal_dv_ips` - is a PSEUDO-kinematic: the
    /// integral of a voltage, carrying g-derived units it does not have. They
    /// are still exactly right for the checks these rows make, because both
    /// sides of every comparison ride the same fiction: the tolerance check
    /// compares the measured pseudo-dV against the NOMINAL pseudo-dV computed
    /// from the same spec through the same integrator (`tolerance.rs:280`), so
    /// the units cancel and the ratio is real. What must NOT happen is a
    /// number off this table being quoted as an acceleration, a velocity or a
    /// stroke. Nothing on a jumper is any of those.
    fn volts_table() -> hertzforge_channel_table::ChannelTable {
        use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        ctrl.units = Units::VoltageV;
        ctrl.sensitivity_mv_per_eu = 1000.0;
        ctrl.range_max = 1.0;
        t.channels.push(ctrl);
        let mut drv = Channel::new(2);
        drv.role = ChannelRole::Drive;
        drv.units = Units::VoltageV;
        t.channels.push(drv);
        t
    }

    /// Volts on the wire per EU on the control row - the `sens/1000` factor of
    /// the amplitude row, read back off the table rather than assumed, so a
    /// table edit that broke the identity would red the row instead of silently
    /// widening it.
    fn volts_per_eu() -> f64 {
        volts_table()
            .channels
            .first()
            .map(|c| c.sensitivity_mv_per_eu / 1000.0)
            .unwrap_or(f64::NAN)
    }

    // -----------------------------------------------------------------------
    // THE GROUP-DELAY MODEL (a carry-forward from the baseline re-probe)
    // -----------------------------------------------------------------------

    /// The three terms that separate the COMMANDED composite from where it
    /// lands in the INGESTED record, in frames at the adopted rate.
    #[derive(Debug, Clone, Copy)]
    struct DelayModel {
        /// **Term 1 - the quiet lead-in.** `BackendPulseFirer::fire` builds the
        /// record as `[q zeros][composite][q zeros]`
        /// (`service/firer.rs:562-577`), and `q` is TIME-denominated:
        /// `max(0.5 x T_pulse, 5 ms)` at the adopted rate
        /// (`service/firer.rs:73-84`, set by the service at
        /// `service/mod.rs:663-666`). At 51 200 Hz on an 11 ms pulse that is
        /// `ceil(0.0055 x 51200) = 282` frames. This is a HOST-side offset: the
        /// composite is 282 frames into its own record before the device has
        /// done anything.
        quiet_lead: usize,
        /// **Term 2 - the ADC group delay.** The 9234's delta-sigma input path
        /// delays every sample by `38.4 + 3.2e-6 x fs` samples
        /// (`hertzforge_hal_ni::clock::group_delay_samples`, NI 9234 Operating
        /// Instructions; `[H] PROVISIONAL`, paper-derived). The value is read
        /// FROM the HAL rather than copied, so the model and the abort-latency
        /// bound cannot drift apart. At 51 200 Hz: `ceil(38.5638) = 39` frames.
        adc_group_delay: usize,
        /// **Term 3 - the read-window offset the fire path introduces. ZERO,
        /// by construction, and this term exists to say so.**
        ///
        /// `fire()` writes the head of the record into the output buffer BEFORE
        /// `start()` (`service/firer.rs:585-587`), so `NiBackend::start()`'s
        /// silence top-up lands BEHIND the record instead of in front of it,
        /// and `start()` releases output and input together. The play loop then
        /// reads back exactly `record_frames` frames from the AI cursor that
        /// began at that same instant (`hertzforge-hal/src/stream.rs:219-278` -
        /// `read` counts sequentially from AI frame 0). So measured frame `k`
        /// is AI at `k`, which is AO frame `k` delayed by term 2, which is
        /// `drive_scratch[k]`. That is the cold-task invariant, and its arithmetic
        /// consequence is that this term is 0.
        ///
        /// It was NOT 0 before the cold task: `start()` at ARM prefilled the
        /// output with `min_lookahead_frames` of SILENCE (2048 at a 1024-frame
        /// block) and the whole 624-frame record sat behind it, which is why the
        /// first baseline found NO correlation peak at all
        /// (`service/firer.rs:436-444`). The counterfactual is printed beside
        /// the model so the number that used to be here is visible.
        read_window_offset: usize,
        /// `-(quiet_lead + adc_group_delay + read_window_offset)`.
        ///
        /// NEGATIVE because of [`xcorr_peak`]'s convention: `b[j]` pairs with
        /// `a[j + lag]`, so a `b` that is `a` delayed by `D` peaks at `lag = -D`. The
        /// ingested record is the commanded composite delayed, so the lag is negative.
        /// (`examples/shock_shot_probe.rs`'s `xcorr_peak` doc comment states the
        /// opposite convention and is wrong about its own indexing. Its NUMBERS are
        /// correct; only the sentence describing them is inverted, which is why the
        /// baseline re-probe's `-323` reads as a 323-frame DELAY and not as a
        /// physically impossible 323-frame advance.)
        lag: isize,
    }

    /// Build the model for `pulse_duration_s` at `fs_hz`.
    ///
    /// # Does this explain the baseline re-probe's `-323`?
    ///
    /// Yes, to within 2 frames, and only as the SUM. The re-probe banner records a
    /// constant `-323`-sample lag at `r = 1.0000` and notes that it "does not
    /// equal the quiet lead or the 39-frame group delay" - correct, and the sum
    /// was never tried: `282 + 39 + 0 = 321`.
    ///
    /// The residual is `+2` frames = 39 us at 51 200 Hz. It is NOT absorbed into
    /// the model and must not be (a model tuned to a number explains nothing). What
    /// it plausibly is: the NI-9263's own analog-OUTPUT path delay, which no
    /// constant in this workspace models - `group_delay_samples` is the 9234 INPUT
    /// delay and nothing else. The discriminating observation that would settle it
    /// is a rate sweep: an output delay fixed in SECONDS scales with `fs` in
    /// frames, one fixed in FRAMES does not. Neither this gate nor the re-probe ran
    /// at a second rate, so the term is unattributed, not explained.
    ///
    /// Consequence for [`ALIGN_TOL_FRAMES`]: the band is `+/-2`, the observed
    /// residual is exactly 2, so the row has NO margin on the high side. That
    /// is deliberate. The alignment is deterministic (`r = 1.0000`, not a
    /// noisy estimate), so a residual of 3 is a real change in the signal path
    /// and worth stopping for, not jitter to be tolerated.
    ///
    /// # A note on the re-probe banner's arithmetic
    ///
    /// The banner reads the 2817-frame ingested record as "563-frame
    /// time-denominated quiet lead + 2253 composite + tail". The code gives
    /// `record = composite + 2q` with `q = 282`, i.e. `2253 + 564 = 2817`: 563
    /// is the TOTAL surround off by one, not the lead. The distinction is
    /// load-bearing here - a model built on a 563-frame LEAD would predict
    /// `-602` and miss by 279 frames.
    fn delay_model(pulse_duration_s: f64, fs_hz: f64) -> DelayModel {
        let quiet_lead = quiet_lead_frames(pulse_duration_s, fs_hz);
        let adc_group_delay = hertzforge_hal_ni::clock::group_delay_samples(fs_hz)
            .map(|s| s.ceil() as usize)
            .unwrap_or(0);
        let read_window_offset = 0usize;
        DelayModel {
            quiet_lead,
            adc_group_delay,
            read_window_offset,
            lag: -((quiet_lead + adc_group_delay + read_window_offset) as isize),
        }
    }

    /// Print the model term by term, with the counterfactual `read_window`
    /// value the pre-cold-task shape would have carried.
    fn print_delay_model(m: &DelayModel, fs_hz: f64, pulse_duration_s: f64, lookahead: u32) {
        let raw_delay = hertzforge_hal_ni::clock::group_delay_samples(fs_hz).unwrap_or(f64::NAN);
        println!(
            "  MODEL   group-delay decomposition at fs = {fs_hz:.4} Hz, T = {:.3} ms:",
            pulse_duration_s * 1e3
        );
        println!(
            "            quiet lead-in      = ceil(max({QUIET_LEAD_FRACTION_OF_PULSE} x {:.6} s, \
             {MIN_QUIET_LEAD_S} s) x {fs_hz:.4}) = {} frames   [service: firer.rs:73-84]",
            pulse_duration_s, m.quiet_lead
        );
        println!(
            "            ADC group delay    = ceil(38.4 + 3.2e-6 x {fs_hz:.4}) = ceil({raw_delay:.4}) \
             = {} frames   [hertzforge_hal_ni::clock, NI 9234 OI&S, [H] PROVISIONAL]",
            m.adc_group_delay
        );
        println!(
            "            read-window offset = {} frames   [cold-task invariant, firer.rs:585-598 + \
             stream.rs:219-278; before the cold task this term was the arm-time silence \
             prefill = {lookahead} frames]",
            m.read_window_offset
        );
        println!(
            "            MODEL LAG          = -({} + {} + {}) = {} frames",
            m.quiet_lead, m.adc_group_delay, m.read_window_offset, m.lag
        );
    }

    /// Mean-removed, energy-normalized cross-correlation over every lag the two
    /// records admit. Returns `(lag, peak_r)`.
    ///
    /// Deliberately identical in form to `examples/shock_shot_probe.rs`'s, so
    /// the baseline re-probe's number and this gate's number are the same
    /// measurement. `b[j]` pairs with `a[j + lag]`, so a `b` DELAYED relative
    /// to `a` peaks at a NEGATIVE lag.
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

    /// The contiguous run around the record's peak whose `|v| >= |peak|/2`, in
    /// SAMPLES - the identical definition `tools/probes/shock_dmm.py`'s
    /// `half_amplitude_width` measures on the wire, so the expected width this
    /// gate prints and the width the DMM reports are the same quantity.
    ///
    /// This is why the duration row does NOT compare the DMM's number against
    /// the commanded 11 ms: a half-sine `sin(pi t / T)` is at or above half
    /// amplitude for `t/T` in `[1/6, 5/6]`, so an 11 ms command measures
    /// 7.33 ms at half amplitude and a row comparing 7.33 against 11 would fail
    /// a correct pulse. The conversion is shape-aware and belongs beside the
    /// profile - see `shock_dmm.py:178-198`, which says exactly that and names
    /// this row as its consumer.
    fn half_amplitude_samples(v: &[f64]) -> Option<usize> {
        if v.is_empty() {
            return None;
        }
        let peak_idx = (0..v.len()).max_by(|&i, &j| {
            v[i].abs()
                .partial_cmp(&v[j].abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        let half = v[peak_idx].abs() / 2.0;
        if half <= 0.0 {
            return None;
        }
        let mut lo = peak_idx;
        while lo > 0 && v[lo - 1].abs() >= half {
            lo -= 1;
        }
        let mut hi = peak_idx;
        while hi + 1 < v.len() && v[hi + 1].abs() >= half {
            hi += 1;
        }
        Some(hi - lo + 1)
    }

    // -----------------------------------------------------------------------
    // Service construction
    // -----------------------------------------------------------------------

    type Firer = BackendPulseFirer<Box<dyn hertzforge_hal::Backend>>;
    type Svc = ClassicalShockService<Firer>;

    /// An armed classical service plus the one backend fact that must be read
    /// BEFORE the backend moves into the firer.
    struct Armed {
        svc: Svc,
        /// `Caps::output_buffer.min_lookahead_frames` - the counterfactual
        /// `read_window_offset` the pre-cold-task shape carried. Printed, never
        /// asserted.
        lookahead: u32,
    }

    /// The gate's base config: the teaching pulse on the MIL-STD-810 516 bands,
    /// AUTO ladder, single-pulse "train", conservative rate model.
    fn base_cfg() -> ClassicalTestConfig {
        let mut spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, AMPLITUDE, DURATION_MS);
        // MIL-COMPLIANT compensation geometry, not the crate default. The
        // default (1.5x shoulders) solves to ~-33% of peak - the decoded OEM
        // geometry - and the MIL-810-516 surround band is +/-15% of peak, so
        // the default can NEVER pass the envelope row (run 1 measured worst
        // dev 0.2396 of peak and tripped every downstream row). tolerance.rs's
        // own module docs prescribe the fix: "lengthen the shoulders (lower
        // amplitude) to fit inside the band". 4.5x shoulders solve to
        // ~-11% of peak, inside the band with margin. CONSEQUENCE: the
        // composite is ~10x the main-pulse duration (~6200 frames at the
        // device rate), LONGER than the 3072-frame AO buffer - which is
        // exactly the record>buffer shape the capped pre-start commit exists
        // for. MIL-compliant shock exercises it as the NORMAL case.
        spec.compensation.pre_frac = 4.5;
        spec.compensation.post_frac = 4.5;
        let mut cfg = ClassicalTestConfig::new(spec);
        cfg.standard = ToleranceStandard::Mil810_516;
        cfg
    }

    /// A MANUAL-ladder config pinned at `start_db` - the shape every row that
    /// wants control over the level uses.
    fn manual_cfg(start_db: f64) -> ClassicalTestConfig {
        let mut cfg = base_cfg();
        cfg.schedule = ShockLevelSchedule::manual_default();
        cfg.schedule.start_db = start_db;
        cfg
    }

    /// The peak the composite will actually put on the wire at `level_db`,
    /// volts. Checked against [`BENCH_MAX_DRIVE_V`] BEFORE anything is armed.
    fn commanded_peak_v(cfg: &ClassicalTestConfig, fs_hz: f64, level_db: f64) -> f64 {
        cfg.spec
            .synthesize(fs_hz)
            .waveform_g
            .iter()
            .fold(0.0f64, |m, &v| m.max(v.abs()))
            * 10.0f64.powf(level_db / 20.0)
    }

    /// Build + feasibility + arm on a fresh station. `first` routes the station
    /// build through the file's exiting `station()` (see [`try_station`]).
    fn build(cfg: ClassicalTestConfig, tag: &str, first: bool) -> Result<Armed, String> {
        // The bench contract, enforced before a task exists. The service has no
        // drive-volts ceiling to enforce it with (see the ceiling row).
        let peak = commanded_peak_v(&cfg, cfg.fs_hz, cfg.schedule.start_db);
        if !(peak.is_finite() && peak <= BENCH_MAX_DRIVE_V) {
            return Err(format!(
                "REFUSED before arming: the commanded composite peaks at {peak:.4} V at the \
                 {:.1} dB start level, above this gate's {BENCH_MAX_DRIVE_V:.2} V bench ceiling",
                cfg.schedule.start_db
            ));
        }
        let st = if first { station() } else { try_station()? };
        let lookahead = st
            .backend
            .caps()
            .output_buffer
            .map(|c| c.min_lookahead_frames)
            .unwrap_or(0);
        let firer = BackendPulseFirer::new(st.backend, volts_table(), st.station)
            .map_err(|e| format!("firer construct: {e:?}"))?;
        let mut svc = ClassicalShockService::new(cfg, volts_table(), firer, Some(tmp(tag)))
            .map_err(|e| format!("service construct: {e:?}"))?;
        svc.assess_feasibility()
            .map_err(|e| format!("feasibility: {e:?}"))?;
        svc.arm().map_err(|e| format!("arm: {e:?}"))?;
        Ok(Armed { svc, lookahead })
    }

    /// A per-block observer that timestamps every delivered input block with
    /// `Instant`, plus the frame counters. The shared instrument behind the
    /// rate assay and the train-spacing row: both need a HOST clock, and the
    /// service's own `run_clock_s` is `x += spacing` (asserting against it
    /// asserts `(x + s) - x == s`).
    type BlockMarks = Arc<Mutex<Vec<(Instant, usize, usize)>>>;

    fn attach_marks(svc: &mut Svc) -> BlockMarks {
        let marks: BlockMarks = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&marks);
        svc.set_block_observer(Box::new(move |done, total| {
            if let Ok(mut v) = sink.lock() {
                v.push((Instant::now(), done, total));
            }
        }));
        marks
    }

    /// The `Instant` at which each FIRE's first block was delivered. A fire
    /// boundary is a `delivered` count that did not increase - the counter
    /// restarts at every `fire()`.
    fn fire_starts(marks: &BlockMarks) -> Vec<Instant> {
        let Ok(v) = marks.lock() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut prev = usize::MAX;
        for &(t, done, _) in v.iter() {
            if done <= prev {
                out.push(t);
            }
            prev = done;
        }
        out
    }

    // =======================================================================
    // CLASSICAL SHOCK
    // =======================================================================

    pub fn classical(rows: &mut Rows) {
        println!();
        println!("--- CLASSICAL SHOCK (chassis gate, NUMERIC) ---");
        mutation_guard_row(rows, "classical");

        row_units_and_alignment(rows);
        row_rate_assay(rows);
        row_drive_volts_ceiling(rows);
        row_ladder_walk(rows);
        row_drive_limit_trip(rows);
        row_train(rows);
        row_arm_idle_fire(rows);
        row_abort_rearm_fire(rows);
        row_stop_mid_train(rows);
        info_rate_model_floor(rows);
    }

    /// ROWS 1-5: one fire on the VOLTS-EXACT table carries the units row, the
    /// amplitude row, the tolerance verdict, the tolerance-window row and the
    /// group-delay alignment row - they are five questions about ONE pulse and
    /// firing five pulses to ask them would let each row answer about a
    /// different one.
    fn row_units_and_alignment(rows: &mut Rows) {
        let Armed { mut svc, lookahead } = match build(manual_cfg(-12.0), "classical-units", true) {
            Ok(a) => a,
            Err(e) => {
                rows.row(false, "classical arm (adopt-actual + arm gate)", e);
                return;
            }
        };

        // --- ROW 1a: THE ADOPTION, with both rates on screen. ---------------
        // Authorities: the PROFILE's requested `fs_hz` vs the DEVICE's
        // `ConfiguredActual`. This is the one comparison in the arm path with
        // two genuine sides - `arm_gate` itself receives the post-adoption rate
        // on both sides by the deliberate arm ordering, so it is an
        // adoption ASSERTION and cannot red here.
        let requested = svc.requested_fs_hz();
        let actual = svc.actual_fs_hz();
        let logged_coercion = svc.log().events().iter().any(|e| {
            matches!(
                e.kind,
                hertzforge_shock_host::service::report::EventKind::RateAdopted { .. }
            )
        });
        let diverged = actual.is_some_and(|a| (requested - a).abs() / a > 1e-9);
        rows.row(
            actual.is_some_and(|a| a.is_finite() && a > 0.0) && (!diverged || logged_coercion),
            "classical adopts the DEVICE rate and logs the divergence",
            format!(
                "requested {requested:.4} Hz, device programmed {} Hz, RateAdopted logged: {logged_coercion}",
                actual.map(|a| format!("{a:.4}")).unwrap_or_else(|| "NONE".into())
            ),
        );
        let fs = denominating_hz(&svc);
        let play_fs = svc.fs_hz();

        // --- FIRE. --------------------------------------------------------
        let dmm = wait_for_dmm("single fire");
        let t0 = Instant::now();
        if let Err(e) = svc.fire_next_pulse() {
            rows.row(false, "classical single fire", format!("{e:?}"));
            return;
        }
        let t1 = Instant::now();
        if dmm_enabled() {
            dmm_window("single fire", t0, t1);
        }
        let report = svc.report_data();
        let (Some(row), Some(rec)) = (report.pulses.last(), svc.pulse_store().last()) else {
            rows.row(
                false,
                "classical single fire ingests a record",
                "the fire returned Ok and the store is EMPTY".into(),
            );
            return;
        };

        // --- ROW 2: AMPLITUDE, commanded volts vs measured volts. -----------
        // Authorities: the PROFILE (`spec.amplitude_g x 10^(dB/20)`, scaled to
        // volts by the table's own sensitivity) vs the AI (the ingested
        // record's peak). Two independent sides: nothing in the acquisition
        // path reads the profile.
        let vpe = volts_per_eu();
        let commanded_v = row.commanded_amplitude_g * vpe;
        let measured_v = rec.kinematics.peak_accel_g * vpe;
        let amp_err = if commanded_v != 0.0 {
            (measured_v - commanded_v).abs() / commanded_v
        } else {
            f64::INFINITY
        };
        // The composite's own peak, so a compensation shoulder that grew above
        // the main pulse shows as a number instead of as a mystery failure.
        let comp_peak = svc
            .config()
            .spec
            .synthesize(play_fs)
            .waveform_g
            .iter()
            .fold(0.0f64, |m, &v| m.max(v.abs()));
        rows.row(
            amp_err < AMPLITUDE_TOL,
            "classical amplitude: commanded x (sens/1000) V vs measured V within 5%",
            format!(
                "commanded {commanded_v:.5} V ({:.4} EU x {vpe:.3} V/EU at {:.1} dB), measured \
                 {measured_v:.5} V ({:.2}% off); composite peak / main peak = {:.4}",
                row.commanded_amplitude_g,
                row.commanded_db,
                amp_err * 100.0,
                comp_peak / svc.config().spec.amplitude_g
            ),
        );

        // --- ROW 3: THE UNITS ROW - the re-probe tolerance trip, retired. ---
        // Authorities: the AI record integrated over the standard's dV window
        // vs the NOMINAL dV of the same spec at the adopted rate. The baseline
        // re-probe measured `error_frac` 8.28-8.29 on EVERY pulse against the
        // 0.10 MIL band - the 10x units mismatch of the shared 100 mV/EU
        // table, arriving as a tolerance abort (Firing -> Aborted). If this row
        // passes, the trip has STOPPED OCCURRING at a correct amplitude, which
        // is what the carry-forward asked for.
        let tol = report.tolerance.last.clone();
        match tol {
            None => rows.row(
                false,
                "classical tolerance verdict read",
                "no tolerance report on the fired pulse".into(),
            ),
            Some(t) => {
                let tripped = svc.trip().is_some() || svc.state() == RunState::Aborted;
                rows.row(
                    t.overall_pass && !tripped,
                    "classical VOLTS-EXACT: tolerance PASSES; the 10x units trip is gone",
                    format!(
                        "dV error_frac {:+.4} vs the {:.2} MIL-810-516 band (baseline, 100 mV/EU \
                         table: +8.28); envelope {} (worst dev {:.4} of peak vs {:.2} band), \
                         substitution {}, overall {}; trip: {}",
                        t.velocity_change.error_frac,
                        t.velocity_change.tol_frac,
                        t.amplitude_envelope.pass,
                        t.amplitude_envelope.worst_deviation_frac_of_peak,
                        t.amplitude_envelope.tol_frac_of_peak,
                        t.substitution.pass,
                        t.overall_pass,
                        svc.trip()
                            .map(|r| format!("{:?}", r.kind))
                            .unwrap_or_else(|| "none".into())
                    ),
                );

                // --- ROW 4: THE TOLERANCE WINDOW WAS NOT CLIPPED. ----------
                // Authorities: the CHECKER's `checked_samples` (counted over
                // the real record) vs the STANDARD's window arithmetic
                // (`w_pre + n_nominal + w_post`, from `ToleranceParams` and the
                // nominal pulse length at the denominating rate).
                //
                // This row exists because a clipped window is a green earned by
                // not looking: `check_tolerance` silently `continue`s on any
                // sample outside the record (`tolerance.rs:241-244`), so a
                // quiet surround too short for MIL-STD-810's +/-0.4 T window
                // makes the envelope check pass by checking fewer samples. The
                // time-denominated quiet lead is what
                // buys the full window, and this is the row that proves it.
                let p = report.tolerance.params;
                let mut nominal_spec = svc.config().spec.clone();
                nominal_spec.amplitude_g = row.commanded_amplitude_g;
                let n_nom = nominal_spec.main_pulse(fs).len();
                let w_pre = ((n_nom as f64) * p.window_pre_frac).round() as usize;
                let w_post = ((n_nom as f64) * p.window_post_frac).round() as usize;
                let want = w_pre + n_nom + w_post;
                rows.row(
                    t.amplitude_envelope.checked_samples == want,
                    "classical tolerance window UNCLIPPED: checked_samples == the full MIL surround",
                    format!(
                        "checked {} samples; the standard's window is {w_pre} + {n_nom} + {w_post} \
                         = {want} ({:.2} T pre / {:.2} T post at {fs:.4} Hz); record holds {} \
                         samples with a {}-frame quiet lead each side",
                        t.amplitude_envelope.checked_samples,
                        p.window_pre_frac,
                        p.window_post_frac,
                        rec.time_history.len(),
                        quiet_lead_frames(DURATION_MS / 1000.0, fs),
                    ),
                );
            }
        }

        // --- ROW 5: THE GROUP-DELAY MODEL. ---------------------------------
        // Authorities: the MEASURED cross-correlation lag (commanded composite
        // vs ingested record) against the MODEL built from the service's own
        // quiet-lead rule and the HAL's own 9234 group-delay formula. The model
        // is the second authority precisely because it is assembled from
        // constants neither side of the measurement can see.
        let model = delay_model(DURATION_MS / 1000.0, fs);
        print_delay_model(&model, fs, DURATION_MS / 1000.0, lookahead);
        let commanded_wave = svc.config().spec.synthesize(fs).waveform_g;
        let (lag, r) = xcorr_peak(&commanded_wave, &rec.time_history);
        let residual = lag - model.lag;
        // --- DISCRIMINATOR (2026-08-08 instrumentation): the
        // offline math proved this row's xcorr correct on an IDEAL record
        // (s2_xcorr_offline: -321, r 1.000000), so the deviation is IN the
        // acquired record. Print r AT the model lag and the per-region RMS
        // deviation (pre-shoulder / main / post-shoulder, at the model
        // alignment) so ONE run names WHERE the record differs from the
        // reference - a shoulder-selective deviation indicts a low-frequency
        // mechanism (coupling/droop); a boundary-localized one indicts the
        // committed-head seam at frame 3072.
        {
            let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
            let (ma, mb) = (mean(&commanded_wave), mean(&rec.time_history));
            let ea: f64 = commanded_wave
                .iter()
                .map(|x| (x - ma) * (x - ma))
                .sum::<f64>()
                .sqrt();
            let eb: f64 = rec
                .time_history
                .iter()
                .map(|x| (x - mb) * (x - mb))
                .sum::<f64>()
                .sqrt();
            let tl = model.lag;
            let (n, m) = (
                commanded_wave.len() as isize,
                rec.time_history.len() as isize,
            );
            let (jlo, jhi) = ((-tl).max(0), (n - tl).min(m));
            let mut acc = 0.0;
            for j in jlo..jhi {
                acc +=
                    (commanded_wave[(j + tl) as usize] - ma) * (rec.time_history[j as usize] - mb);
            }
            let r_true = acc / (ea * eb);
            // Per-region RMS deviation at the model alignment. The record is
            // level-scaled; normalize by the fitted gain over the overlap.
            let mut num = 0.0;
            let mut den = 0.0;
            for j in jlo..jhi {
                num += commanded_wave[(j + tl) as usize] * rec.time_history[j as usize];
                den += commanded_wave[(j + tl) as usize] * commanded_wave[(j + tl) as usize];
            }
            let gain = if den > 0.0 { num / den } else { 1.0 };
            let region = |name: &str, a0: usize, a1: usize| {
                let (mut dev2, mut cnt) = (0.0f64, 0usize);
                for (ai, &cw) in commanded_wave.iter().enumerate().take(a1).skip(a0) {
                    let j = ai as isize - tl;
                    if j >= 0 && j < m {
                        let d = cw * gain - rec.time_history[j as usize];
                        dev2 += d * d;
                        cnt += 1;
                    }
                }
                if cnt > 0 {
                    println!(
                        "  DISC    {name}: rms deviation {:.5} EU over {cnt} frames (gain {gain:.4})",
                        (dev2 / cnt as f64).sqrt()
                    );
                }
            };
            println!("  DISC    r at the MODEL lag {tl}: {r_true:+.6} (best was {r:+.6} at {lag})");
            let shoulder = commanded_wave.len().saturating_sub(563) / 2;
            region("pre-shoulder ", 0, shoulder);
            region("main pulse   ", shoulder, shoulder + 563);
            region("post-shoulder", shoulder + 563, commanded_wave.len());
        }
        println!(
            "  MODEL   measured lag {lag} frames (peak r {r:+.6}); model {} frames; RESIDUAL \
             {residual:+} frames = {:+.1} us at {fs:.4} Hz",
            model.lag,
            residual as f64 * 1e6 / fs
        );
        if r.abs() < PEAK_FLOOR_R {
            rows.row(
                false,
                "classical group-delay model: measured lag == model +/- 2 frames",
                format!(
                    "NO-PEAK: best |r| {:.4} is below the {PEAK_FLOOR_R:.2} detection floor at lag \
                     {lag}; there is no alignment to compare against the model (commanded {} \
                     samples, ingested {} samples)",
                    r.abs(),
                    commanded_wave.len(),
                    rec.time_history.len()
                ),
            );
        } else {
            rows.row(
                residual.abs() <= ALIGN_TOL_FRAMES,
                "classical group-delay model: measured lag == model +/- 2 frames",
                format!(
                    "measured {lag} (xcorr, r {r:+.6}) vs model {} (= -({} quiet lead + {} ADC \
                     group delay + {} read-window)); residual {residual:+} frames",
                    model.lag, model.quiet_lead, model.adc_group_delay, model.read_window_offset
                ),
            );
        }

        // --- THE DURATION HAND-OFF. ----------------------------------------
        // Authorities: the DMM's own clock on AO0 vs the profile-derived
        // composite at the adopted rate. The comparison is NOT in-sweep - the
        // DMM digitize blocks in its own process and only a human (or the main
        // session) can read its verdict back - so this prints the expectation
        // and the sweep's own timing assertion is the wall-clock rate assay.
        let composite_play = svc.config().spec.synthesize(fs).waveform_g;
        match half_amplitude_samples(&composite_play) {
            None => println!("  DMM     expected width: INDETERMINATE (the composite has no peak)"),
            Some(n) => {
                // Width in ms = samples/synthesis_rate, then scaled by
                // synthesis_rate/play_rate: a record built at one rate and
                // played at another comes out in that ratio. Both are the
                // adopted rate in a real run (factor 1.0); under the
                // red-mutation the factor is 1/10, which is the physically
                // correct counterfactual for the rate-divorce defect and reds the
                // operator's DMM row.
                let width_ms = (n as f64) * 1e3 / fs * (fs / play_fs);
                println!(
                    "  DMM     expected half-amplitude width on AO0: {width_ms:.3} ms (+/- 0.100 ms, \
                     one sample at shock_dmm.py's 10 kSa/s) for a commanded {DURATION_MS:.1} ms \
                     {:?}; ratio {:.4} (a half-sine is >= half amplitude over t/T in [1/6, 5/6] = \
                     2/3). Run: `HERTZFORGE_DMM_THRESHOLD_V=0.005 python3 tools/probes/shock_dmm.py \
                     --expect present` and compare its PRESENT width against this number.",
                    svc.config().spec.shape,
                    width_ms / DURATION_MS
                );
                if !dmm {
                    println!(
                        "  DMM     (no drive-side witness this run: HERTZFORGE_S2_DMM unset or the \
                         trigger timed out - the duration comparison is owed to the sitting)"
                    );
                }
            }
        }
        let _ = svc.stop();
    }

    /// ROW 6: THE WALL-CLOCK RATE ASSAY.
    ///
    /// Authorities: the HOST's `Instant` across a known number of delivered
    /// frames vs the DEVICE's `ConfiguredActual` rate. Two genuinely separate
    /// clocks - the assertion is that the chassis is playing at the rate it
    /// says it is, and neither side can produce the other's number.
    ///
    /// It fires a LONGER pulse than the rest of the section (100 ms, so the record
    /// is ~0.5 s and spans ~25 blocks) for a stated reason: across the 2817-frame
    /// teaching record the play is only ~3 blocks, the first-to-last span is 2
    /// block intervals, and a 0.5% band on 55 ms is 275 us - inside the driver's
    /// own delivery jitter. A row whose tolerance sits inside the noise is not an
    /// assertion.
    ///
    /// An earlier draft's "the pulse played at the device rate" row is DELETED: it
    /// read the rate off the same `ConfiguredActual` both sides of the comparison
    /// came from.
    fn row_rate_assay(rows: &mut Rows) {
        const ASSAY_MS: f64 = 100.0;
        let mut cfg = manual_cfg(-12.0);
        cfg.spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, AMPLITUDE, ASSAY_MS);
        let Armed { mut svc, .. } = match build(cfg, "classical-assay", false) {
            Ok(a) => a,
            Err(e) => {
                rows.row(false, "classical rate assay: arm", e);
                return;
            }
        };
        let marks = attach_marks(&mut svc);
        if let Err(e) = svc.fire_next_pulse() {
            rows.row(false, "classical rate assay: fire", format!("{e:?}"));
            return;
        }
        let expect_hz = denominating_hz(&svc);
        let Ok(v) = marks.lock() else {
            rows.row(
                false,
                "classical rate assay",
                "observer mutex poisoned".into(),
            );
            return;
        };
        // First-to-last, so the task-start transient is outside the window.
        if v.len() < 8 {
            rows.row(
                false,
                "classical wall-clock rate assay: frames/elapsed vs actual_rate_hz within 0.5%",
                format!(
                    "INCONCLUSIVE: only {} delivered block(s) - too few for a rate this row can \
                     stand behind (floor 8). A pass here would be arithmetic on two samples.",
                    v.len()
                ),
            );
            return;
        }
        let (t_first, f_first, _) = v[0];
        let (t_last, f_last, total) = v[v.len() - 1];
        let frames = f_last.saturating_sub(f_first);
        let elapsed = (t_last - t_first).as_secs_f64();
        let measured_hz = if elapsed > 0.0 {
            frames as f64 / elapsed
        } else {
            f64::INFINITY
        };
        let err = (measured_hz - expect_hz).abs() / expect_hz;
        rows.row(
            err < RATE_ASSAY_TOL && elapsed >= 0.25,
            "classical wall-clock rate assay: frames/elapsed vs actual_rate_hz within 0.5%",
            format!(
                "{frames} frames delivered in {elapsed:.6} s (host Instant) = {measured_hz:.4} Hz \
                 vs the driver's {expect_hz:.4} Hz ({:.4}% off); {} blocks across a {total}-frame \
                 record",
                err * 100.0,
                v.len()
            ),
        );
    }

    /// ROW 7: THE DRIVE-VOLTS CEILING. **SKIPPED, with a stated note.**
    ///
    /// The row as specified is "a 6 V-implied profile refuses by name
    /// with both numbers". There is nothing to ask: `max_drive`, `drive_vpk`, `vpk_ceiling`
    /// and `eu_to_volts` have ZERO hits across `hertzforge-shock-host`, `hertzforge-srs` and
    /// `hertzforge-shock-common`. `ClassicalTestConfig` carries `drive_limit` (a peak-ACCEL
    /// ceiling, predictive, in EU) and nothing denominated in volts, and `fire()` writes
    /// the g-valued record straight into the DAC - so a 6 V profile is not refused, it is
    /// played.
    ///
    /// The field lands with the wire work (both wire profiles carry
    /// `max_drive_vpk`, default 5.0, clearing it refuses). Until
    /// then the enforcement point is this gate's own [`BENCH_MAX_DRIVE_V`],
    /// checked in [`build`] before a task exists, and the row below proves THAT
    /// refusal works rather than claiming a service protection that does not
    /// exist.
    fn row_drive_volts_ceiling(rows: &mut Rows) {
        println!(
            "  SKIP    classical drive-volts ceiling refusal: NO drive-volts concept exists in \
             the shock crates at this commit (max_drive/drive_vpk/eu_to_volts: zero hits). \
             Lands with max_drive_vpk on the wire profiles; this row is owed then."
        );
        // What CAN be asserted today: the gate's own pre-arm bench ceiling
        // refuses by name with both numbers, so the sweep cannot drive a
        // profile past the bench contract while the service is blind to volts.
        // Authorities: the PROFILE's synthesized composite peak vs the stated
        // bench ceiling.
        let mut cfg = manual_cfg(0.0);
        cfg.spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 6.0, DURATION_MS);
        let peak = commanded_peak_v(&cfg, cfg.fs_hz, cfg.schedule.start_db);
        match build(cfg, "classical-ceiling", false) {
            Err(e) if e.contains("bench ceiling") => rows.row(
                true,
                "classical 6 V-implied profile refuses at the GATE's bench ceiling (service has none)",
                format!("composite peak {peak:.4} V vs the {BENCH_MAX_DRIVE_V:.2} V ceiling; refusal: {e}"),
            ),
            Err(e) => rows.row(
                false,
                "classical 6 V-implied profile refuses at the GATE's bench ceiling (service has none)",
                format!("refused for the WRONG reason ({peak:.4} V profile): {e}"),
            ),
            Ok(_) => rows.row(
                false,
                "classical 6 V-implied profile refuses at the GATE's bench ceiling (service has none)",
                format!(
                    "ARMED a profile whose composite peaks at {peak:.4} V, above the \
                     {BENCH_MAX_DRIVE_V:.2} V bench ceiling - nothing refused it"
                ),
            ),
        }
    }

    /// ROW 8: THE LADDER WALK, -12 -> 0 dB, with per-level pulse counts.
    ///
    /// Authorities: the SCHEDULE (`table_db` and `pulses_for(i)` - the test
    /// definition) vs the REPORT's per-pulse rows (what the run actually
    /// fired, one row per pulse, recorded by the fire pipeline). The schedule
    /// never reads the report and the report never reads the schedule.
    ///
    /// The rate model is left at its conservative default here on purpose: the AUTO
    /// ladder walks UP in level, and until it was fixed it charged the floors to a
    /// `f64` field and returned, so on the chassis all five rungs fired back to
    /// back with the fastest firing at the highest drive. This row walks the real
    /// ladder at the real pace.
    fn row_ladder_walk(rows: &mut Rows) {
        let mut cfg = base_cfg();
        cfg.schedule = ShockLevelSchedule::auto_default();
        cfg.schedule.pulses_per_level = vec![2];
        let table_db = cfg.schedule.table_db.clone();
        let per_level: Vec<u32> = (0..table_db.len())
            .map(|i| cfg.schedule.pulses_for(i))
            .collect();
        let want_total: u64 = per_level.iter().map(|&n| n as u64).sum();
        let Armed { mut svc, .. } = match build(cfg, "classical-ladder", false) {
            Ok(a) => a,
            Err(e) => {
                rows.row(false, "classical ladder walk: arm", e);
                return;
            }
        };
        let state = match svc.run_auto(want_total + 4) {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "classical ladder walk: run_auto", format!("{e:?}"));
                return;
            }
        };
        let report = svc.report_data();
        let mut counts: Vec<u32> = vec![0; table_db.len()];
        let mut off_rung = 0u32;
        for r in &report.pulses {
            match table_db
                .iter()
                .position(|&db| (db - r.scheduled_level_db).abs() < 1e-9)
            {
                Some(i) => counts[i] += 1,
                None => off_rung += 1,
            }
        }
        rows.row(
            counts == per_level && off_rung == 0 && state == RunState::Stopped,
            "classical ladder walk -12 -> 0 dB: per-level pulse counts match the schedule",
            format!(
                "levels {table_db:?} dB; fired {counts:?} vs scheduled {per_level:?} ({} pulses \
                 total, {off_rung} off-rung); final state {state:?}",
                report.pulses.len()
            ),
        );
    }

    /// ROW 9: A DRIVE-LIMIT TRIP FIRES AND SCALES THE NEXT PULSE.
    ///
    /// Authorities: the CONFIGURED between-pulse ceiling (test config, in the
    /// control channel's EU) vs the AI-MEASURED peak of the pulse that just fired.
    /// The honest half of drive limiting: this is enforcement, not prediction - the
    /// first breach has already fired by the time it is caught, and the row asserts
    /// the SECOND pulse came down, not that the first did.
    ///
    /// The ceiling is set below the level the -12 dB rung is known to produce,
    /// so the trip is guaranteed to fire. That is not the row cheating: what is
    /// being tested is that a breach is detected, recorded, and CARRIED into
    /// the next pulse's amplitude, and a ceiling nothing reaches would test
    /// none of it.
    fn row_drive_limit_trip(rows: &mut Rows) {
        let mut cfg = manual_cfg(-12.0);
        // The commanded peak at this rung, in EU on the volts-exact table.
        let expect_peak = AMPLITUDE * 10.0f64.powf(-12.0 / 20.0);
        let ceiling = 0.5 * expect_peak;
        cfg.drive_limit.monitors = vec![DriveMonitor {
            // The service names the control channel `ch<index>`
            // (`service/mod.rs:303-307`); the monitor must match that name or
            // `between_pulse` reports NoMeasurement and enforces nothing.
            channel: "ch1".into(),
            eu: "g".into(),
            peak_ceiling: ceiling,
            kind: DriveMonitorKind::BetweenPulse,
        }];
        let Armed { mut svc, .. } = match build(cfg, "classical-limit", false) {
            Ok(a) => a,
            Err(e) => {
                rows.row(false, "classical drive-limit trip: arm", e);
                return;
            }
        };
        for i in 0..2 {
            if let Err(e) = svc.fire_next_pulse() {
                rows.row(
                    false,
                    "classical drive-limit trip fires and scales the next pulse",
                    format!("pulse {i} refused: {e:?}"),
                );
                return;
            }
        }
        let report = svc.report_data();
        let first_peak = report
            .pulses
            .first()
            .and_then(|r| r.peak_g)
            .unwrap_or(f64::NAN);
        let want_scale = ceiling / first_peak;
        let got_scale = report
            .pulses
            .get(1)
            .map(|r| r.drive_limit_scale)
            .unwrap_or(f64::NAN);
        let second_peak = report
            .pulses
            .get(1)
            .and_then(|r| r.peak_g)
            .unwrap_or(f64::NAN);
        let acted = report
            .drive_limit_actions
            .iter()
            .any(|a| a.phase == "between-pulse" && a.scale < 1.0);
        let scale_ok = (got_scale - want_scale).abs() / want_scale < 0.02;
        let landed_ok = (second_peak - ceiling).abs() / ceiling < AMPLITUDE_TOL;
        rows.row(
            acted && scale_ok && landed_ok,
            "classical drive-limit trip fires and SCALES the next pulse onto the ceiling",
            format!(
                "ceiling {ceiling:.5} EU; pulse 0 measured {first_peak:.5} EU (breach) -> next-pulse \
                 scale {got_scale:.5} vs ceiling/measured {want_scale:.5}; pulse 1 measured \
                 {second_peak:.5} EU ({:.2}% off the ceiling); between-pulse action recorded: {acted}",
                (second_peak - ceiling).abs() / ceiling * 100.0
            ),
        );
        let _ = svc.stop();
    }

    /// ROW 10: A TRAIN OF 5 AT 2.0 s, WALL-CLOCK.
    ///
    /// Authorities: the HOST's `Instant` at each fire's first delivered block
    /// vs the COMMANDED spacing in the train plan. Measured, never accumulated -
    /// the service's `run_clock_s` is `x += spacing`, so asserting against it
    /// asserts `(x + s) - x == s`, and the `TrainRecord::wall_times_s` come off
    /// the service's own injected pacer. The `Instant` marks come from inside
    /// the fire's block loop and know nothing about either.
    fn row_train(rows: &mut Rows) {
        let Armed { mut svc, .. } = match build(manual_cfg(-12.0), "classical-train", false) {
            Ok(a) => a,
            Err(e) => {
                rows.row(false, "classical train: arm", e);
                return;
            }
        };
        let marks = attach_marks(&mut svc);
        let dmm = wait_for_dmm("train of 5");
        let t0 = Instant::now();
        let plan = match svc.fire_train_with(TrainSpec::delayed(TRAIN_COUNT, TRAIN_SPACING_S)) {
            Ok(p) => p,
            Err(e) => {
                rows.row(false, "classical train", format!("{e:?}"));
                return;
            }
        };
        let t1 = Instant::now();
        if dmm_enabled() {
            dmm_window("train of 5", t0, t1);
        } else if !dmm {
            println!("  DMM     train continuity is owed to the sitting (HERTZFORGE_S2_DMM unset)");
        }
        let starts = fire_starts(&marks);
        if starts.len() < 2 {
            rows.row(
                false,
                "classical train of 5 at 2.0 s: wall-clock spacing within 10%",
                format!(
                    "only {} pulse start(s) observed of {} planned - the train did not run",
                    starts.len(),
                    plan.count
                ),
            );
            return;
        }
        let mut worst = 0.0f64;
        let mut detail = String::new();
        for i in 1..starts.len() {
            let d = (starts[i] - starts[i - 1]).as_secs_f64();
            let err = (d - plan.realized_spacing_s).abs() / plan.realized_spacing_s;
            worst = worst.max(err);
            detail.push_str(&format!("{i}: {d:.4} s ({:+.2}%); ", err * 100.0));
        }
        println!("  INFO    classical train per-pulse wall deltas (host Instant): {detail}");
        rows.row(
            starts.len() as u32 == plan.count && worst < TRAIN_SPACING_TOL,
            "classical train of 5 at 2.0 s: wall-clock spacing within 10%",
            format!(
                "{} of {} pulses fired; worst inter-pulse error {:.2}% against the plan's realized \
                 {:.6} s (floor {:.6} s); deltas: {detail}",
                starts.len(),
                plan.count,
                worst * 100.0,
                plan.realized_spacing_s,
                plan.floor_s
            ),
        );
        let _ = svc.stop();
    }

    /// ROW 11: ARM -> 30 s IDLE -> FIRE.
    ///
    /// Authorities: the DEVICE's answer to a fire after a 30 s park vs the
    /// RECORD LENGTH the profile and the adopted rate predict
    /// (`composite + 2 x quiet_lead`). A refusal reds it; so does a short
    /// record, which is what a partially-starved play would produce.
    ///
    /// This is the AI-side row and it FAILED at the first baseline: the
    /// pre-cold-task shape started both tasks at arm, so the input ring (2.0 s
    /// floor, no overwrite property, drained only during a fire) had 30 s to
    /// overflow and the AO write path died first - "drive output accepted nothing
    /// after 0 frames with none in flight". Parked tasks cannot do either, and this
    /// row is the one that says so on iron.
    fn row_arm_idle_fire(rows: &mut Rows) {
        let Armed { mut svc, .. } = match build(manual_cfg(-12.0), "classical-idle", false) {
            Ok(a) => a,
            Err(e) => {
                rows.row(false, "classical arm -> 30 s idle -> fire: arm", e);
                return;
            }
        };
        let fs = svc.fs_hz();
        let want = svc.config().spec.synthesize(fs).waveform_g.len()
            + 2 * quiet_lead_frames(DURATION_MS / 1000.0, fs);
        println!("  INFO    classical holding armed for {IDLE_S:.0} s with nothing driving ...");
        std::thread::sleep(Duration::from_secs_f64(IDLE_S));
        match svc.fire_next_pulse() {
            Err(e) => rows.row(
                false,
                "classical arm -> 30 s idle -> fire",
                format!("fire after {IDLE_S:.0} s idle REFUSED: {e:?}"),
            ),
            Ok(_) => {
                let got = svc
                    .pulse_store()
                    .last()
                    .map(|r| r.time_history.len())
                    .unwrap_or(0);
                rows.row(
                    got == want,
                    "classical arm -> 30 s idle -> fire",
                    format!(
                        "fired after {IDLE_S:.0} s parked; ingested {got} frames vs the \
                         {want} the profile predicts at {fs:.4} Hz (composite + 2 x {} quiet)",
                        quiet_lead_frames(DURATION_MS / 1000.0, fs)
                    ),
                );
            }
        }
        let _ = svc.stop();
    }

    /// ROW 12: ABORT -> RE-ARM -> FIRE (the estop-unreserve zombie check).
    ///
    /// Authorities: the DRIVER's willingness to build and run a second task set
    /// while the aborted service is still alive vs the RECORD LENGTH the
    /// profile predicts for the second fire. Both sides can fail
    /// independently: a task left RESERVED reds the build, and a zombie AO that
    /// "succeeds" while consuming nothing (measured 2026-08-03) reds the length.
    ///
    /// The first service is deliberately NOT dropped before the second is
    /// built. That is the whole question: `emergency_stop` gained
    /// `TASK_UNRESERVE` so abort -> re-arm -> fire is not a zombie, and a
    /// row that dropped the first service first would let `Drop` do the
    /// unreserving and prove nothing about the abort path.
    ///
    /// `arm()` is valid only from `Feasibility`, so "re-arm" here is a fresh
    /// service on a fresh station rather than re-arming the aborted one - the
    /// service state machine does not offer the other shape, and saying so is
    /// better than implying a verb that does not exist.
    fn row_abort_rearm_fire(rows: &mut Rows) {
        let Armed2 { mut first } = match build(manual_cfg(-12.0), "classical-abort", false) {
            Ok(a) => Armed2 { first: a.svc },
            Err(e) => {
                rows.row(false, "classical abort -> re-arm -> fire: first arm", e);
                return;
            }
        };
        if let Err(e) = first.fire_next_pulse() {
            rows.row(
                false,
                "classical abort -> re-arm -> fire",
                format!("the pre-abort fire refused: {e:?}"),
            );
            return;
        }
        if let Err(e) = first.abort() {
            rows.row(
                false,
                "classical abort -> re-arm -> fire",
                format!("abort refused: {e:?}"),
            );
            return;
        }
        let aborted = first.state();
        let trip = first
            .trip()
            .map(|t| format!("{:?}", t.kind))
            .unwrap_or_else(|| "none".into());
        // The abort arithmetic, printed with the row: mid-record preemption is
        // REFUSED by this chassis (-200289), so the committed queue bounds how
        // long drive stays on the wire and the DMM on AO0 is the only
        // admissible witness (emergency_stop stops the INPUT task too).
        println!(
            "  INFO    classical abort: state {aborted:?}, trip {trip}; drive-off evidence is \
             DMM-side only (emergency_stop stops the AI task). Committed-queue bound at a \
             1024-frame block and 2-block lookahead: up to 3072 frames ~= 60 ms at 51 200 Hz, \
             which is LONGER than the whole ~44 ms record: a shock abort is a promise about the \
             NEXT pulse."
        );
        // The second service is built while `first` is STILL ALIVE.
        match build(manual_cfg(-12.0), "classical-rearm", false) {
            Err(e) => rows.row(
                false,
                "classical abort -> re-arm -> fire (estop-unreserve zombie check)",
                format!(
                    "the post-abort station/arm REFUSED while the aborted service is still \
                     alive - a task was left reserved: {e}"
                ),
            ),
            Ok(Armed { mut svc, .. }) => {
                let fs = svc.fs_hz();
                let want = svc.config().spec.synthesize(fs).waveform_g.len()
                    + 2 * quiet_lead_frames(DURATION_MS / 1000.0, fs);
                match svc.fire_next_pulse() {
                    Err(e) => rows.row(
                        false,
                        "classical abort -> re-arm -> fire (estop-unreserve zombie check)",
                        format!("the post-abort fire refused: {e:?}"),
                    ),
                    Ok(_) => {
                        let got = svc
                            .pulse_store()
                            .last()
                            .map(|r| r.time_history.len())
                            .unwrap_or(0);
                        rows.row(
                            got == want,
                            "classical abort -> re-arm -> fire (estop-unreserve zombie check)",
                            format!(
                                "aborted at {aborted:?} ({trip}); a NEW station armed and fired \
                                 while the aborted service was still alive, ingesting {got} frames \
                                 vs the {want} the profile predicts - not a zombie that consumes \
                                 nothing"
                            ),
                        );
                    }
                }
                let _ = svc.stop();
            }
        }
        drop(first);
    }

    /// Helper struct so [`row_abort_rearm_fire`]'s destructuring reads
    /// straight; the second field of [`Armed`] is unused there.
    struct Armed2 {
        first: Svc,
    }

    /// ROW 13: STOP MID-TRAIN WITH DRIVE QUEUED.
    ///
    /// Authorities: the HOST's cancel timing (`Instant`, raised from another
    /// thread mid-train) vs the STORE's per-record lengths. The second side is
    /// what makes this a boundary test rather than a "did it stop" test: every
    /// pulse that DID fire must carry a FULL record, because Stop is defined as
    /// "complete the current pulse, cancel at the next inter-pulse boundary,
    /// park" - a short record would mean the cancel landed mid-record, which
    /// this chassis refuses anyway (-200289).
    ///
    /// Drive-side evidence is the DMM's: the trigger-file window is printed so
    /// the sitting can put `shock_dmm.py --expect absent` on the tail and prove
    /// the output node went quiet. The AI cannot testify to that.
    fn row_stop_mid_train(rows: &mut Rows) {
        let Armed { mut svc, .. } = match build(manual_cfg(-12.0), "classical-stop", false) {
            Ok(a) => a,
            Err(e) => {
                rows.row(false, "classical Stop mid-train: arm", e);
                return;
            }
        };
        let fs = svc.fs_hz();
        let want_len = svc.config().spec.synthesize(fs).waveform_g.len()
            + 2 * quiet_lead_frames(DURATION_MS / 1000.0, fs);
        // TRUE keeps the train going (the opposite sense to the play loop's
        // `cancel`, which is TRUE to stop - the two meet in this call path and
        // are not interchangeable).
        let keep_going = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&keep_going);
        let raise_at = Duration::from_secs_f64(TRAIN_SPACING_S * 1.25);
        let t0 = Instant::now();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(raise_at);
            flag.store(false, Ordering::SeqCst);
            Instant::now()
        });
        let spec = TrainSpec::delayed(TRAIN_COUNT, TRAIN_SPACING_S);
        let res = svc.fire_train_cancellable(spec, Some(keep_going.as_ref()));
        let raised = stopper.join().ok();
        let t1 = Instant::now();
        if dmm_enabled() {
            dmm_window("stop mid-train", t0, t1);
        }
        println!(
            "  DMM     stop mid-train: point `shock_dmm.py --expect absent` at the window AFTER \
             {:.3} s into this row (the cancel instant) - the AI is not evidence for a drive-off \
             claim, only the output node is",
            raised.map(|r| (r - t0).as_secs_f64()).unwrap_or(f64::NAN)
        );
        let plan = match res {
            Ok(p) => p,
            Err(e) => {
                rows.row(false, "classical Stop mid-train", format!("{e:?}"));
                return;
            }
        };
        let fired = svc.pulse_store().records().len();
        let all_full = svc
            .pulse_store()
            .records()
            .iter()
            .all(|r| r.time_history.len() == want_len);
        let cancelled_logged = svc.log().events().iter().any(|e| {
            matches!(
                e.kind,
                hertzforge_shock_host::service::report::EventKind::TrainCancelled { .. }
            )
        });
        let not_aborted = svc.state() != RunState::Aborted;
        rows.row(
            fired >= 1
                && (fired as u32) < plan.count
                && all_full
                && cancelled_logged
                && not_aborted,
            "classical Stop mid-train: the cancel lands at a PULSE boundary, not mid-record",
            format!(
                "{fired} of {} pulses fired before the host lowered should_continue at {:.3} s; \
                 every fired record is {want_len} frames: {all_full}; TrainCancelled logged: \
                 {cancelled_logged}; state {:?} (a Stop is not an Abort)",
                plan.count,
                raised.map(|r| (r - t0).as_secs_f64()).unwrap_or(f64::NAN),
                svc.state()
            ),
        );
        let _ = svc.stop();
        rows.row(
            svc.state() == RunState::Stopped,
            "classical Stop parks the run cleanly after the boundary cancel",
            format!("state {:?}", svc.state()),
        );
    }

    /// ROW 14 (INFO, NOT AN ASSERTION): THE ACHIEVABLE RATE-MODEL FLOOR.
    ///
    /// The number the gunfire question needs, and the reason it is not a
    /// row: `RateModel::conservative_default`'s floors are SELF-DECLARED
    /// PLACEHOLDERS (cooling 1.0 s, settling 0.5 s, compute 0.25 s), so
    /// asserting the measured turnaround against them would be asserting
    /// against a guess. This measures what the CURRENT cold-task shape can
    /// actually do - one full `arm -> preload -> start -> play -> stop -> park`
    /// cycle per pulse - and prints it, so the floors can be set FROM data in a
    /// later phase.
    ///
    /// The rate model is zeroed for the measurement and that is stated rather
    /// than hidden: the floors are the thing being characterized, and leaving
    /// them in would measure the placeholder rather than the hardware.
    fn info_rate_model_floor(rows: &mut Rows) {
        let mut cfg = manual_cfg(-12.0);
        cfg.rate_model = RateModel {
            cooling_floor_s: 0.0,
            settling_floor_s: 0.0,
            compute_floor_s: 0.0,
            device_recompute_floor_s: 0.0,
            read_cadence_ok: true,
        };
        // A tolerance abort would truncate the sample; the row reports how many
        // it actually got rather than pretending to ten.
        let Armed { mut svc, .. } = match build(cfg, "classical-floor", false) {
            Ok(a) => a,
            Err(e) => {
                println!("  INFO    classical rate-model floor: NOT MEASURED ({e})");
                return;
            }
        };
        let mut turnarounds: Vec<f64> = Vec::with_capacity(FLOOR_PULSES);
        for _ in 0..FLOOR_PULSES {
            let t = Instant::now();
            match svc.fire_next_pulse() {
                Ok(true) => turnarounds.push(t.elapsed().as_secs_f64()),
                Ok(false) => break,
                Err(_) => break,
            }
        }
        if turnarounds.len() < 3 {
            println!(
                "  INFO    classical rate-model floor: NOT MEASURED (only {} pulse(s) completed; \
                 state {:?})",
                turnarounds.len(),
                svc.state()
            );
            let _ = svc.stop();
            return;
        }
        let mut sorted = turnarounds.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let min = sorted[0];
        let med = sorted[sorted.len() / 2];
        let max = sorted[sorted.len() - 1];
        println!(
            "  INFO    classical per-pulse TURNAROUND over {} pulses with the rate model ZEROED \
             (arm -> preload -> start -> play -> stop -> park, {:.1} ms of which is the record \
             itself): min {:.4} s, median {:.4} s, max {:.4} s",
            turnarounds.len(),
            svc.config().spec.synthesize(svc.fs_hz()).waveform_g.len() as f64 * 1e3 / svc.fs_hz(),
            min,
            med,
            max
        );
        println!(
            "  INFO    => ACHIEVABLE FLOOR with the current cold-task shape: {:.4} s/pulse median \
             = {:.1} rounds/min sustained ({:.1} rounds/min at the best observed turnaround). The \
             SHIPPING floor is RateModel::conservative_default's 1.0 s = 60 rounds/min, a \
             self-declared placeholder; this measurement is what a data-set floor would be built \
             from. NOT asserted against - see the row doc.",
            med,
            60.0 / med,
            60.0 / min
        );
        let _ = svc.stop();
        let _ = rows;
    }

    // =======================================================================
    // SRS
    // =======================================================================

    /// ROWS 15-18: sysID coherence, the convergence trend, the review hold and
    /// the release.
    ///
    /// The SRS section runs on the SAME volts-exact table, so its "g" is a volt
    /// too and the same pseudo-kinematic caveat applies. `SrsTestConfig` has no
    /// `fs_hz` field at all - the rate comes from the plant and is adopted at
    /// arm - so this section prints the ACTUAL rate only, where classical
    /// prints requested-and-actual.
    pub fn srs(rows: &mut Rows) {
        println!();
        println!("--- SRS (chassis gate, NUMERIC) ---");
        mutation_guard_row(rows, "srs");

        // A modest flat target: on the volts-exact table this is 0.2 V of SRS,
        // and the synthesized drive to reach it is checked against the bench
        // ceiling BEFORE anything fires (the service has no drive-volts
        // concept - see the classical ceiling row's SKIP note).
        let mut cfg = hertzforge_srs::SrsTestConfig::flat(100.0, 2_000.0, 0.2);
        // MANUAL so the three convergence pulses stay on ONE rung: the trend
        // being asserted is the control law closing on the target, and an AUTO
        // ladder walking the level between pulses would confound it.
        cfg.schedule = ShockLevelSchedule::manual_default();
        cfg.schedule.mode = ShockRunMode::Manual;
        // The hold stays ON - it is the shipping default (`review_after_sysid`
        // is serde-defaulted to true so an older stored setup gets the safe
        // behavior). An instrument that switched a safety default off to make
        // its own rows easier would be measuring a configuration nobody ships.
        cfg.review_after_sysid = true;

        let st = match try_station() {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "srs station", e);
                return;
            }
        };
        let plant = match hertzforge_srs::HalPulsePlant::new(st.backend, volts_table(), st.station)
        {
            Ok(p) => p,
            Err(e) => {
                rows.row(false, "srs plant construct", e.to_string());
                return;
            }
        };
        let mut svc = match hertzforge_srs::SrsShockService::new(
            cfg,
            volts_table(),
            hertzforge_srs::FirePath::Host(Box::new(plant)),
            Some(tmp("srs")),
        ) {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "srs service construct", format!("{e:?}"));
                return;
            }
        };
        if let Err(e) = svc.assess_feasibility() {
            rows.row(false, "srs feasibility", format!("{e:?}"));
            return;
        }
        if let Err(e) = svc.arm() {
            rows.row(false, "srs arm", format!("{e:?}"));
            return;
        }
        println!(
            "  INFO    srs armed at the ADOPTED {:.4} Hz (requested {:.4} Hz; SrsTestConfig has no \
             fs_hz field - the plant owns the rate)",
            svc.fs_hz(),
            svc.requested_fs_hz()
        );

        // --- ROW 15: THE sysID BAND-COHERENCE FLOOR. ------------------------
        // Authorities: the MEASURED in-band coherence (Welch H/coherence over
        // the acquired response) vs a STATED floor. On a jumper the plant is a
        // wire, so this is close to a positive control: coherence lands at ~1.0
        // and anything below the floor is a broken excitation or a broken
        // acquisition, NOT a hard plant. Stated so a pass is not read as
        // evidence about a shaker.
        if let Err(e) = svc.run_sysid() {
            rows.row(false, "srs run_sysid", format!("{e:?}"));
            return;
        }
        let coh = svc.status().band_coherence;
        rows.row(
            coh >= SYSID_COHERENCE_FLOOR,
            "srs sysID in-band coherence at or above the floor",
            format!(
                "measured mean in-band coherence {coh:.7} vs the {SYSID_COHERENCE_FLOOR:.2} floor \
                 (jumper plant: a wire, so this is near a positive control - it says the \
                 excitation and the acquisition agree, not that a shaker is well behaved)"
            ),
        );

        // --- ROW 16: THE REVIEW HOLD IS ENTERED AND THE HARDWARE IS PARKED. -
        // Authorities: the SERVICE state vs the service's own refusal of a
        // driving verb. Two sides: a state token alone would not prove the hold
        // does anything, and a refusal alone would not prove the run reached
        // the hold rather than a fault. The fire-refusal probe is the parked-
        // hardware evidence this gate can produce; the DMM at 0.000 V through
        // the hold is the drive-side half and is owed to the sitting.
        let held = svc.state();
        let refusal = match svc.fire_pulse() {
            Err(e) => format!("{e:?}"),
            Ok(_) => "ACCEPTED (nothing refused the fire)".into(),
        };
        let named = refusal.contains("HOLDING for operator review")
            && refusal.contains("proceed_after_review");
        rows.row(
            held == hertzforge_srs::service::report::RunState::AwaitingReview && named,
            "srs review hold ENTERED and refuses a fire BY NAME (tasks parked)",
            format!("state {held:?}; fire_pulse -> {refusal}"),
        );
        if dmm_enabled() {
            println!(
                "  DMM     srs review hold: run `shock_dmm.py --expect absent` NOW - the hold \
                 parks both tasks and AO0 must read 0.000 V for as long as the hold stands"
            );
            std::thread::sleep(Duration::from_secs(6));
        }

        // --- ROW 17: PROCEED RELEASES THE HOLD AND A PULSE FIRES. -----------
        // Authorities: the SERVICE state after Proceed vs the STORE gaining a
        // record. Proceed re-arms the parked fire path (`ensure_open`) and
        // refuses by name if the re-bring-up came back on a different rate, so
        // a green here also says the FRF and drive table are still valid at the
        // rate they were built at.
        if let Err(e) = svc.proceed_after_review() {
            rows.row(false, "srs proceed after review", format!("{e:?}"));
            return;
        }

        // --- BENCH-CONTRACT CHECK after Proceed, BEFORE anything fires. -----
        // The service has no drive-volts ceiling (classical ceiling row's SKIP
        // note), so the gate checks the synthesized drive itself. This check
        // MUST sit after `proceed_after_review()`: `drive_components()` is the
        // ARMED drive table, which does not exist during the hold - run 1
        // placed it before Proceed and the None mapped to NaN, refusing a
        // healthy run ("the synthesized drive peaks at NaN V"). On a breach
        // the run is ABORTED (the drive is armed at this point; stop would
        // ramp a drive the gate just called over-ceiling).
        let drive_peak = svc
            .drive_components()
            .map(|e| {
                e.synthesize(svc.fs_hz())
                    .iter()
                    .fold(0.0f64, |m, &v| m.max(v.abs()))
            })
            .unwrap_or(f64::NAN)
            * 10.0f64.powf(svc.current_level_db() / 20.0);
        if !(drive_peak.is_finite() && drive_peak <= BENCH_MAX_DRIVE_V) {
            rows.row(
                false,
                "srs proceed + fire completes",
                format!(
                    "REFUSED after proceeding, before firing: the armed drive peaks at \
                     {drive_peak:.4} V at {:.1} dB, above this gate's {BENCH_MAX_DRIVE_V:.2} V \
                     bench ceiling",
                    svc.current_level_db()
                ),
            );
            svc.abort_now("gate bench ceiling: armed drive over 0.5 V");
            return;
        }
        println!(
            "  INFO    srs armed drive peak {drive_peak:.4} V at {:.1} dB, inside the \
             {BENCH_MAX_DRIVE_V:.2} V bench ceiling",
            svc.current_level_db()
        );
        let dmm = wait_for_dmm("srs first pulse");
        if let Err(e) = svc.fire_pulse() {
            rows.row(false, "srs fire after proceed", format!("{e:?}"));
            return;
        }
        if !dmm {
            println!("  DMM     (srs first pulse fired without a drive-side witness)");
        }
        rows.row(
            svc.store().len() == 1
                && svc.state() != hertzforge_srs::service::report::RunState::AwaitingReview,
            "srs proceed releases the hold and a pulse fires + is captured",
            format!(
                "state {:?}, store holds {} pulse(s), last record {} frames at {:.4} Hz",
                svc.state(),
                svc.store().len(),
                svc.store()
                    .last()
                    .map(|r| r.time_history.len())
                    .unwrap_or(0),
                svc.fs_hz()
            ),
        );

        // --- ROW 18: THE 3-ITERATION CONVERGENCE TREND. ---------------------
        // Authorities: the MEASURED per-band SRS of each pulse (AI) vs the
        // TARGET grid (profile). `worst_err_db` is the level-normalized worst
        // signed per-band error, so pinning the ladder in MANUAL keeps every
        // iteration on the same rung and the trend is the control law's work
        // and nothing else.
        //
        // The assertion is a TREND, not a threshold: three iterations on a flat
        // unity plant is not enough to demand convergence to a band, and
        // demanding one would be an acceptance criterion invented to fit a
        // jumper. What must be true is that the error is coming DOWN.
        for i in 1..3 {
            if let Err(e) = svc.fire_pulse() {
                rows.row(
                    false,
                    "srs 3-iteration convergence trend: |worst err| decreasing",
                    format!("iteration {i} refused: {e:?}"),
                );
                let _ = svc.stop();
                return;
            }
        }
        let errs: Vec<f64> = svc
            .report_data()
            .pulses
            .iter()
            .filter_map(|p| p.worst_err_db)
            .map(|e| e.abs())
            .collect();
        if errs.len() < 3 {
            rows.row(
                false,
                "srs 3-iteration convergence trend: |worst err| decreasing",
                format!(
                    "only {} iteration(s) carry a worst-band error; three are needed for a trend",
                    errs.len()
                ),
            );
        } else {
            let monotone = errs.windows(2).all(|w| w[1] <= w[0] + 1e-9);
            let improved = errs[errs.len() - 1] < errs[0];
            rows.row(
                monotone && improved,
                "srs 3-iteration convergence trend: |worst band error| decreasing",
                format!(
                    "|worst err| per iteration: {:?} dB; monotone non-increasing: {monotone}; \
                     first {:.4} -> last {:.4} dB",
                    errs.iter()
                        .map(|e| (e * 1e4).round() / 1e4)
                        .collect::<Vec<_>>(),
                    errs[0],
                    errs[errs.len() - 1]
                ),
            );
        }
        let _ = svc.stop();
    }
}

// ---------------------------------------------------------------------------
// 5. SINE - the two chassis gates.
//
// This section used to end at `coherence_gate()`: arm + sysID + a coherence
// number, and nothing driven. That was the whole of Sine's hardware evidence,
// and an earlier "runs closed-loop" status claim was corrected against it -
// NO ENGAGED SINE CONTROL TICK HAS EVER EXECUTED ON THIS CHASSIS. Both sections
// below drive one.
//
// `sine-baseline` (opt-in, PRE-conversion) is the discriminating observation the
// conversion is built on: a ~6 minute engaged run whose chunk boundary is
// EXPECTED to show taper + silence on the DMM. It exists so the post-conversion
// continuity row proves an ABSENCE against an instrument already shown to see the
// defect when it is present (a result that cannot fail is not evidence).
//
// `sine` (POST-conversion) is the numeric gate: every row states a measured
// quantity, a commanded one, the tolerance between them, and - in its comment -
// WHAT WOULD MAKE IT FAIL on this plant. That last part matters more here than
// anywhere else in this file, because Sine's bench plant is a JUMPER: a flat
// unity plant on which a servo that is doing nothing at all lands on target by
// accident. Where a row could pass that way it carries a positive or negative
// control that removes the accident.
//
// Bench contract, the same one every other section carries: AO0 jumpered to
// AI0, NOTHING downstream, IEPE OFF. Drive tops out at 0.2 V here and the
// profile's own Vpk abort sits at 0.8 V.
// ---------------------------------------------------------------------------
fn sine(rows: &mut Rows) {
    sine_rig::gate(rows);
}

fn sine_baseline(rows: &mut Rows) {
    sine_rig::baseline(rows);
}

mod sine_rig {
    use super::{station_with_outputs, tmp, Rows};
    use hertzforge_dsp::tracking::BandwidthMode;
    use hertzforge_sine::service::config::{
        CaptureConfig, ColaConfig, ControlChannel, ControlSet, SineMode, SineTestConfig,
    };
    use hertzforge_sine::{
        BidirSweepConfig, CompositeMode, CompressionPreset, DwellFor, DwellPlan, FeedforwardConfig,
        LadderConfig, NotchGovernorConfig, ProfileRow, RunState, RungFor, SineProfile, SineService,
        SineStatus, SweepDirection, SweepRate, SweepTarget,
    };
    use std::time::{Instant, SystemTime};

    /// The rate REQUESTED of the chassis. 51 200 Hz is 13.1072 MHz / 256, i.e.
    /// exactly ON the 9234's ladder, so this request is NOT coerced - which is
    /// why the coercion-warning evidence lives in a unit test against a mock and
    /// not in a row here. The gate still prints requested and actual side by side:
    /// a row that only ever prints one number cannot show a coercion the day the
    /// bench asks for a rate off the ladder.
    const REQUEST_HZ: f64 = 51_200.0;
    /// The control target, in the VOLTS-EXACT units the table below establishes.
    /// 0.2 V on the wire, an order of magnitude inside the +/-1 V bench contract.
    const TARGET_G: f64 = 0.2;
    /// The run band. Wide enough that the sweep row covers three octaves, low
    /// enough that the sysID's `1600 / f_lo` burst stays inside a bench sitting.
    const BAND: (f64, f64) = (60.0, 500.0);
    /// Chunk budget for any one wait. A chunk is CHUNK_SECONDS (0.25 s) of TEST
    /// time, so 2400 chunks is ~10 minutes: long enough that a slow servo is not
    /// a spurious FAIL, short enough that a hung one does not sit on the bench.
    const MAX_CHUNKS: u32 = 2_400;
    /// The engaged-run length the continuity row spans. Chosen against the
    /// PRE-conversion boundary period (~256 s on this chassis: 12 800 blocks x
    /// 1024 frames / 51 200 Hz), so the window contains at least one boundary the
    /// baseline showed the defect at. A shorter row could pass by never reaching
    /// one.
    const CONTINUITY_S: f64 = 360.0;

    // -----------------------------------------------------------------------
    // The rig
    // -----------------------------------------------------------------------

    /// A VOLTS-EXACT control row: `sensitivity_mv_per_eu = 1000` makes the EU
    /// conversion the identity, so every "g" in this section IS a volt on the
    /// wire and a commanded-vs-measured row compares two numbers in one unit.
    /// The shared `table()` helper's 100 mV/EU accel scaling is correct for the
    /// lifecycle sections and useless for an accuracy assertion.
    fn volts_table(cola: bool) -> hertzforge_channel_table::ChannelTable {
        use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        ctrl.units = Units::AccelG;
        ctrl.sensitivity_mv_per_eu = 1000.0;
        ctrl.range_max = 1.0;
        t.channels.push(ctrl);
        let mut drv = Channel::new(2);
        drv.role = ChannelRole::Drive;
        drv.units = Units::VoltageV;
        t.channels.push(drv);
        if cola {
            let mut aux = Channel::new(3);
            aux.role = ChannelRole::Drive;
            aux.units = Units::VoltageV;
            aux.description = "COLA (aux)".into();
            t.channels.push(aux);
        }
        t
    }

    /// The shared config. `mode` and `cola` are the only axes the rows vary.
    fn cfg(mode: SineMode, cola: bool) -> SineTestConfig {
        let mut profile = SineProfile::new("ni-gate");
        profile.push(ProfileRow::accel(BAND.0, TARGET_G));
        profile.push(ProfileRow::accel(BAND.1, TARGET_G));
        SineTestConfig {
            profile,
            mode,
            control: ControlSet {
                channels: vec![ControlChannel {
                    index: 1,
                    weight: 1.0,
                }],
                mode: CompositeMode::WeightedAvg,
            },
            limit_profiles: Vec::new(),
            blend_w: 1.0,
            compression: CompressionPreset::Fast,
            tracking_bw: BandwidthMode::Proportional {
                pct: 10.0,
                max_hz: 100.0,
            },
            feedforward: FeedforwardConfig::default(),
            notch: NotchGovernorConfig::default(),
            ladder: LadderConfig {
                sysid_level_db: -6.0,
                start_db: -12.0,
                table_db: vec![-12.0, -6.0, 0.0],
                per_rung: RungFor::Seconds(3.0),
                full_for: RungFor::Seconds(3.0),
            },
            // -6 dB, not -20 (2026-08-08 investigation): the burst base is a
            // hardcoded 0.05, so -20 dB drove 5 mV pk into the FIXED +/-10 V
            // 16-bit AO span = 9.5 LSB rms - a quantization-limited coherence
            // ceiling of ~0.9985-0.9991, a coin flip against this gate's 0.999
            // bar even with the lag alignment WORKING. -6 dB puts the burst at
            // 25 mV (44 dB SNR, ceiling 0.99996) and still 20x under the
            // 0.50 V drive clamp. A red on the coherence row now means the
            // ALIGNMENT, not the DAC's resolution.
            sysid_level_db: -6.0,
            coherence_floor: 0.5,
            // REAL numbers, not the 1e4/1e5 the old lifecycle row disabled them
            // with. The drive scale IS volts on this station (the step writes it
            // straight into the AO frame), so these are the bench contract
            // expressed as a run-time protection: clamp at 0.5 V, abort at 0.8 V.
            drive_vpk_ceiling: 0.5,
            drive_vpk_abort: 0.8,
            fault_debounce_ticks: 20,
            kinematic_monitor: false,
            capture: CaptureConfig::default(),
            cola: ColaConfig {
                enabled: cola,
                volts_pk: 0.1,
                waveform: hertzforge_sine::ColaWaveform::Sine,
            },
            shaker: None,
            moving_mass: Default::default(),
            // The gate RELEASES the review park itself (`arm_to_engaged` calls
            // `proceed_after_review`), so the hold stays ON here: an instrument
            // that switched a safety default off to make its own rows easier
            // would be measuring a configuration nobody ships.
            review_hold: true,
            // 512 frames = 10 ms at the adopted rate: comfortably inside the
            // post-conversion bound (control_tick_frames <= TICKS_PER_BATCH x
            // block = 2048 at a 1024-frame block) with room for a smaller block.
            control_tick_frames: 512,
            fs_hz: None,
            metadata: Vec::new(),
        }
    }

    fn build(cfg: SineTestConfig, cola: bool, tag: &str) -> Result<SineService, String> {
        // COLA needs a second AO channel; the plain rows keep the 1-AO shape so
        // their station matches what the GUI's single-drive factory derives.
        let st = station_with_outputs(if cola { 2 } else { 1 });
        SineService::new(
            cfg,
            volts_table(cola),
            st.backend,
            st.station,
            0x51_4E_45_01,
            Some(tmp(tag)),
        )
        .map_err(|e| format!("{e:?}"))
    }

    /// A wall-clock marker for the DMM sitting. The DMM digitize BLOCKS for its
    /// whole window and must be driven by a separate process
    /// (`tools/probes/p050_dmm.py`), so the only thing this run can do for it is
    /// say WHEN each seam happened, in the same clock the operator's shell sees.
    /// Printed as INFO, never a row: a wall time is evidence for a human, not a
    /// predicate this process can evaluate.
    fn dmm_mark(t0: Instant, label: &str) {
        let unix = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        println!(
            "  DMM     {label} at unix {unix:.3} (+{:.3} s into the section)",
            t0.elapsed().as_secs_f64()
        );
    }

    /// Pump chunks until `pred` holds, the run ends, or `limit` chunks elapse.
    /// Returns `(predicate met, chunks pumped, last status)`.
    /// Trip + event-log tail, appended to any pump error so an unexpected
    /// Aborted carries its own explanation off the chassis. A bare "run_chunk
    /// from Aborted" cost a full re-run to diagnose on 2026-08-08 (the settle
    /// row's abort left no trip, no reason, nothing).
    fn failure_context(svc: &SineService) -> String {
        let trip = svc
            .trip()
            .map(|t| format!("{t:?}"))
            .unwrap_or_else(|| "none".into());
        let tail: Vec<String> = svc
            .log()
            .events()
            .iter()
            .rev()
            .take(6)
            .map(|ev| format!("{:?}", ev.kind))
            .collect();
        format!(
            "trip: {trip}; log tail (newest first): {}",
            tail.join(" | ")
        )
    }

    /// The sysID align note (lag, peak |r|, lags searched) - the discriminating
    /// observation for any coherence-row red. Without it a chassis FAIL is
    /// unexplainable after the run (2026-08-08: 0.990133 measured, and whether
    /// the shift even applied was invisible).
    fn last_align_note(svc: &SineService) -> Option<String> {
        svc.log()
            .events()
            .iter()
            .rev()
            .find_map(|ev| match &ev.kind {
                hertzforge_sine::EventKind::Note { text } if text.starts_with("sysID align") => {
                    Some(text.clone())
                }
                _ => None,
            })
    }

    fn pump_until<F>(
        svc: &mut SineService,
        limit: u32,
        mut pred: F,
    ) -> Result<(bool, u32, SineStatus), String>
    where
        F: FnMut(&SineStatus) -> bool,
    {
        let mut n = 0u32;
        loop {
            let alive = match svc.run_chunk() {
                Ok(a) => a,
                Err(e) => return Err(format!("{e}; {}", failure_context(svc))),
            };
            n += 1;
            let s = svc.status();
            if pred(&s) {
                return Ok((true, n, s));
            }
            if !alive || n >= limit {
                return Ok((false, n, s));
            }
        }
    }

    /// Pump for `seconds` of TEST time (the chunk is 0.25 s of it), calling
    /// `each` with every status. Returns the chunk count and the last status.
    fn pump_for<F>(
        svc: &mut SineService,
        seconds: f64,
        mut each: F,
    ) -> Result<(u32, SineStatus), String>
    where
        F: FnMut(&SineStatus),
    {
        let limit = ((seconds / 0.25).ceil() as u32).max(1);
        let mut n = 0u32;
        let mut last = svc.status();
        while n < limit {
            let alive = match svc.run_chunk() {
                Ok(a) => a,
                Err(e) => return Err(format!("{e}; {}", failure_context(svc))),
            };
            n += 1;
            last = svc.status();
            each(&last);
            if !alive {
                break;
            }
        }
        Ok((n, last))
    }

    /// Level error in dB of the measured composite against the commanded target.
    fn level_err_db(s: &SineStatus) -> f64 {
        let m = s.measured_g.unwrap_or(0.0);
        if m <= 0.0 || s.target_g <= 0.0 {
            return f64::INFINITY;
        }
        20.0 * (m / s.target_g).log10()
    }

    /// Arm -> sysID -> engage, shared by both sections. `ff_error_db` injects the
    /// negative control between sysID and the gate (`None` for a normal run).
    /// Returns the band coherence on success.
    fn arm_to_engaged(
        svc: &mut SineService,
        rows: &mut Rows,
        t0: Instant,
        ff_error_db: Option<f64>,
        prefix: &str,
    ) -> Option<f64> {
        if let Err(e) = svc.assess_feasibility() {
            rows.row(false, &format!("{prefix} feasibility"), format!("{e:?}"));
            return None;
        }
        if let Err(e) = svc.arm() {
            rows.row(
                false,
                &format!("{prefix} arm (needs the proven Shared clock)"),
                format!("{e:?}"),
            );
            return None;
        }
        dmm_mark(t0, &format!("{prefix} ARM (AO clock starts here)"));
        let actual = svc.sample_rate_hz();
        rows.row(
            true,
            &format!("{prefix} ARMED closed-loop on NI-REAL"),
            format!(
                "requested {REQUEST_HZ:.1} Hz, adopted {actual:.3} Hz; the proven-Shared \
                 entry is what permits a per-tick host correction here"
            ),
        );
        if let Err(e) = svc.run_sysid(0) {
            rows.row(false, &format!("{prefix} sysid"), format!("{e:?}"));
            return None;
        }
        dmm_mark(t0, &format!("{prefix} sysID BURST END -> gate compute"));
        let coh = svc.band_coherence();
        // The align note prints on PASS and FAIL alike: a coherence number
        // without the lag/peak behind it is not evidence either way.
        if let Some(t) = last_align_note(svc) {
            println!("  INFO    {prefix}: {t}");
        }
        if let Some(db) = ff_error_db {
            if let Err(e) = svc.inject_feedforward_error_db(db) {
                rows.row(
                    false,
                    &format!("{prefix} feedforward negative control"),
                    format!("{e:?}"),
                );
                return None;
            }
        }
        // A CoherenceHold must NOT be releasable, and the gate proves it before it
        // proves anything else: `proceed_after_review` is the AwaitingReview verb,
        // and pointing it at a FAILED gate has to be a typed refusal rather than a
        // one-click bypass. Asserted here on the real chassis as well as in the
        // unit suite, because this is the panel path an operator would actually
        // click.
        match svc.coherence_gate() {
            Ok(true) => {}
            Ok(false) => {
                let bypass = svc.proceed_after_review();
                rows.row(
                    bypass.is_err(),
                    &format!("{prefix} CoherenceHold is NOT releasable by the review verb"),
                    match bypass {
                        Err(e) => format!("refused: {e}"),
                        Ok(()) => "ENGAGED past a FAILED coherence gate".into(),
                    },
                );
                rows.row(
                    false,
                    &format!("{prefix} coherence gate"),
                    format!(
                        "HELD at {coh:.4} (floor {:.3})",
                        svc.config().coherence_floor
                    ),
                );
                return None;
            }
            Err(e) => {
                rows.row(false, &format!("{prefix} coherence gate"), format!("{e:?}"));
                return None;
            }
        }
        // THE REVIEW PARK. `review_hold` is ON in this rig, so a passing gate
        // parks rather than engaging - the row asserts the park happened, and the
        // release is what drives.
        let parked = matches!(svc.state(), RunState::AwaitingReview);
        rows.row(
            parked,
            &format!("{prefix} passing gate PARKS at AwaitingReview (review_hold ON)"),
            format!(
                "state {:?} after a gate that passed at {coh:.4}",
                svc.state()
            ),
        );
        if !parked {
            return None;
        }
        if let Err(e) = svc.proceed_after_review() {
            rows.row(
                false,
                &format!("{prefix} proceed_after_review"),
                format!("{e}"),
            );
            return None;
        }
        dmm_mark(t0, &format!("{prefix} ENGAGE (first closed-loop drive)"));
        Some(coh)
    }

    // -----------------------------------------------------------------------
    // BASELINE (opt-in): the pre-conversion boundary defect, observed.
    // -----------------------------------------------------------------------

    /// `cargo run --release -p hertzforge-station --example ni_module_sweep sine-baseline`
    ///
    /// Run this ONLY with `tools/probes/p050_dmm.py --mode window` armed on AO0
    /// by a second process, centred on the boundary wall-times this prints.
    pub fn baseline(rows: &mut Rows) {
        println!();
        println!("--- SINE BASELINE (PRE-conversion; one row is EXPECTED to FAIL) ---");
        let t0 = Instant::now();

        let mut svc = match build(
            cfg(
                SineMode::Sweep(BidirSweepConfig {
                    band: BAND,
                    rate: SweepRate::LogOctPerMin(1.0),
                    start_hz: BAND.0,
                    initial_direction: SweepDirection::Up,
                    target: SweepTarget::DurationS(CONTINUITY_S * 2.0),
                }),
                false,
            ),
            false,
            "sine-baseline",
        ) {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "sine baseline construct", e);
                return;
            }
        };
        let Some(_coh) = arm_to_engaged(&mut svc, rows, t0, None, "sine baseline") else {
            return;
        };

        // The boundary CADENCE, stated before the run so the operator can arm the
        // DMM against it. Pre-conversion `run_chunk` counts `fs * 0.25` ENGINE
        // TICKS - and one tick is one BLOCK, not one frame. At a 1024-frame block
        // that is 12 800 blocks = 13.1 M frames = ~256 s of test time per chunk,
        // not the 0.25 s the constant's name implies. The frames-as-ticks
        // conflation IS the defect; this line is what makes it visible.
        let fs = svc.sample_rate_hz();
        let block = svc.configured().map(|a| a.block_frames).unwrap_or(1);
        let chunk_s = (fs * 0.25).max(256.0) * block as f64 / fs;
        println!(
            "  INFO    sine baseline: chunk = {:.0} blocks x {block} frames = {chunk_s:.1} s of \
             test time; the FIRST boundary lands ~{chunk_s:.0} s after the ENGAGE mark above",
            (fs * 0.25).max(256.0)
        );

        // Pump the run. Every chunk RETURN is a boundary: mark it, so the DMM
        // capture can be lined up against a wall time rather than a guess.
        let mut boundaries = 0u32;
        let mut worst_err_db = 0.0f64;
        let start = Instant::now();
        let mut fault = None;
        while start.elapsed().as_secs_f64() < CONTINUITY_S {
            match svc.run_chunk() {
                Ok(alive) => {
                    boundaries += 1;
                    dmm_mark(
                        t0,
                        &format!(
                            "sine baseline CHUNK BOUNDARY #{boundaries} (taper + UNRESERVE + \
                             silence re-prefill happen HERE)"
                        ),
                    );
                    let d = level_err_db(&svc.status()).abs();
                    if d.is_finite() {
                        worst_err_db = worst_err_db.max(d);
                    }
                    if !alive {
                        break;
                    }
                }
                Err(e) => {
                    fault = Some(format!("{e}"));
                    break;
                }
            }
        }
        let engaged_s = start.elapsed().as_secs_f64();

        rows.row(
            fault.is_none() && boundaries >= 1,
            "sine baseline: the FIRST engaged sine control ticks on this chassis",
            format!(
                "{engaged_s:.0} s engaged, {boundaries} chunk boundary/ies crossed, worst level \
                 error {worst_err_db:.2} dB{}",
                fault
                    .as_ref()
                    .map(|f| format!(", FAULT: {f}"))
                    .unwrap_or_default()
            ),
        );

        // THE EXPECTED-FAIL ROW. This process cannot see the AO node - only the
        // DMM can - so the row is scored on the DEFECT'S PRESENCE IN THE CODE,
        // which is unconditional pre-conversion: `Engine::run`'s epilogue stops
        // the backend at every chunk end, `NiBackend::stop` tapers the drive over
        // CONTROLLED_STOP_RAMP_MS and UNRESERVEs the AO task, and the next chunk's
        // `start` re-prefills the lookahead with SILENCE. A FAIL here is the
        // discriminating observation the conversion is built on; a PASS would mean
        // this row is looking at the wrong code and nothing downstream should be
        // believed. Confirm it on the DMM window centred on a boundary mark above
        // and record the measured quiet duration beside this line.
        rows.row(
            false,
            "sine baseline: drive continuity ACROSS the chunk boundary (EXPECTED FAIL)",
            format!(
                "pre-conversion `run_chunk` calls `Engine::run`, whose epilogue stops the \
                 backend; every one of the {boundaries} boundary/ies above pays a \
                 taper + UNRESERVE + silence re-prefill. Score the DMM window against the \
                 boundary wall-times printed above."
            ),
        );

        // The stop + sysID->engage seams, marked for the same DMM sitting. Today the
        // sysID->engage gap is quiet by construction (the burst ends, the FRF and
        // feedforward compute, then the step's first frame) - it is the seam the
        // conversion turns into a deliberate cold start, so the baseline records what
        // it looked like BEFORE.
        dmm_mark(t0, "sine baseline STOP SEAM (ramp begins)");
        if let Err(e) = svc.stop() {
            rows.row(false, "sine baseline stop", format!("{e}"));
            return;
        }
        dmm_mark(t0, "sine baseline STOP COMPLETE");
        rows.row(
            matches!(svc.state(), RunState::Stopped),
            "sine baseline reaches Stopped",
            format!("state {:?}", svc.state()),
        );
    }

    // -----------------------------------------------------------------------
    // GATE: the numeric, post-conversion gate. PASS x2 before anything is
    // declared.
    // -----------------------------------------------------------------------

    pub fn gate(rows: &mut Rows) {
        println!();
        println!("--- SINE GATE (closed loop on the proven clock, NUMERIC) ---");
        let t0 = Instant::now();
        sweep_rows(rows, t0);
        negative_control_row(rows, t0);
        dwell_row(rows, t0);
        cola_stop_row(rows, t0);
    }

    /// The main session: coherence, engage, ladder, level accuracy, retune echo,
    /// the sweep, THD, the continuity soak, compute margin, faults, ramped stop.
    fn sweep_rows(rows: &mut Rows, t0: Instant) {
        let mut svc = match build(
            cfg(
                SineMode::Sweep(BidirSweepConfig {
                    band: BAND,
                    rate: SweepRate::LogOctPerMin(4.0),
                    start_hz: BAND.0,
                    initial_direction: SweepDirection::Up,
                    target: SweepTarget::DurationS(CONTINUITY_S * 3.0),
                }),
                false,
            ),
            false,
            "sine-gate",
        ) {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "sine construct", e);
                return;
            }
        };
        let Some(coh) = arm_to_engaged(&mut svc, rows, t0, None, "sine") else {
            return;
        };

        // --- ROW: sysID COHERENCE on the wire. -----------------------------
        // FAILS if the drive/response lag alignment is missing: every nfft window
        // then pairs the drive with a different slice of the response and a
        // perfectly coherent jumper reads far below 1 (Random measured 0.04 on
        // this exact plant before its xcorr port; Sine's un-ported ceiling was
        // 0.990). 0.999 is the post-port bar; a result in [0.99, 0.999) means the
        // port did not take and is a defect to record, not a green.
        rows.row(
            coh >= 0.999,
            "sine sysID band coherence >= 0.999 on the WIRE loop",
            format!(
                "band coherence {coh:.6} (a jumper is a perfectly coherent plant; anything \
                 below this is the drive/response lag alignment, not the plant)"
            ),
        );

        // --- ROW: ENGAGE + the ladder reaches full level. ------------------
        // FAILS if the ladder never advances (a `Seconds` rung whose clock is not
        // fed) or if a protection trips on the way up. `FullLevel` is the state
        // the service enters only when `ladder_done()`.
        let (full, chunks, s) = match pump_until(&mut svc, MAX_CHUNKS, |s| {
            matches!(s.state, RunState::FullLevel)
        }) {
            Ok(v) => v,
            Err(e) => {
                rows.row(false, "sine ladder to full level", e);
                return;
            }
        };
        rows.row(
            full && (s.level_db - 0.0).abs() < 1e-9,
            "sine ladder climbs -12 -> -6 -> 0 dB and reaches FullLevel",
            format!(
                "state {:?} at {:.1} dB after {chunks} chunk(s) ({:.1} s of test time)",
                s.state,
                s.level_db,
                chunks as f64 * 0.25
            ),
        );
        if !full {
            return;
        }

        // --- ROW: CLOSED-LOOP LEVEL at three spot frequencies. -------------
        // Hold the sweep, walk the tone to each spot, let the servo settle, and
        // compare measured against commanded. On a flat unity plant this row can
        // pass with a servo that does nothing - which is what the -6 dB negative
        // control below exists to rule out. What it catches HERE is the opposite
        // failure: a servo that moves and lands somewhere else (a rung scale
        // applied twice, a feedforward with the wrong sign, an EU conversion that
        // is not the identity the volts-exact table promises).
        if let Err(e) = svc.hold() {
            rows.row(false, "sine hold", format!("{e}"));
            return;
        }
        let mut spot_detail = String::new();
        let mut spots_ok = true;
        for target_hz in [80.0f64, 200.0, 450.0] {
            dmm_mark(t0, &format!("sine RETUNE -> {target_hz:.0} Hz"));
            if let Err(e) = go_to(&svc, target_hz) {
                rows.row(false, "sine retune", e);
                return;
            }
            let arrived = match pump_until(&mut svc, MAX_CHUNKS, |s| {
                (s.freq_hz - target_hz).abs() / target_hz < 0.02
            }) {
                Ok(v) => v,
                Err(e) => {
                    rows.row(false, "sine retune pump", e);
                    return;
                }
            };
            if !arrived.0 {
                rows.row(
                    false,
                    "sine retune echo within 2%",
                    format!(
                        "commanded {target_hz:.1} Hz, echo {:.2} Hz",
                        arrived.2.freq_hz
                    ),
                );
                return;
            }
            // Let the servo settle AT the new tone before measuring the level.
            let (_, s) = match pump_for(&mut svc, 3.0, |_| {}) {
                Ok(v) => v,
                Err(e) => {
                    rows.row(false, "sine settle", e);
                    return;
                }
            };
            let m = s.measured_g.unwrap_or(0.0);
            let err = if s.target_g > 0.0 {
                (m - s.target_g).abs() / s.target_g
            } else {
                f64::INFINITY
            };
            spots_ok &= err < 0.05;
            spot_detail.push_str(&format!(
                "{target_hz:.0} Hz: measured {m:.4} V vs target {:.4} V ({:.2}%); ",
                s.target_g,
                err * 100.0
            ));
        }
        rows.row(
            spots_ok,
            "sine closed-loop level within 5% at 80 / 200 / 450 Hz",
            spot_detail,
        );

        // --- ROW: the RETUNE ECHO itself. ----------------------------------
        // FAILS if the scheduler clamp and the NCO disagree - the panel renders
        // the SERVICE's echo, so a frequency clamped in the
        // oscillator but not in the scheduler reads back a number the device is
        // not playing.
        let echo_target = 300.0f64;
        if let Err(e) = go_to(&svc, echo_target) {
            rows.row(false, "sine go-to", e);
            return;
        }
        let (echoed, _, s) = match pump_until(&mut svc, MAX_CHUNKS, |s| {
            (s.freq_hz - echo_target).abs() / echo_target < 0.02
        }) {
            Ok(v) => v,
            Err(e) => {
                rows.row(false, "sine go-to pump", e);
                return;
            }
        };
        rows.row(
            echoed,
            "sine GoTo echo within 2% (scheduler == NCO == status)",
            format!(
                "commanded {echo_target:.1} Hz, echoed {:.2} Hz ({:.2}% off)",
                s.freq_hz,
                (s.freq_hz - echo_target).abs() / echo_target * 100.0
            ),
        );

        // --- ROW: a LEVEL STEP, marked for the DMM. ------------------------
        dmm_mark(t0, "sine LEVEL STEP -3 dB");
        if let Err(e) = svc.step_level(-3.0) {
            rows.row(false, "sine step_level", format!("{e}"));
            return;
        }
        let (stepped, _, s) =
            match pump_until(&mut svc, MAX_CHUNKS, |s| (s.level_db + 3.0).abs() < 1e-9) {
                Ok(v) => v,
                Err(e) => {
                    rows.row(false, "sine level step pump", e);
                    return;
                }
            };
        rows.row(
            stepped,
            "sine manual level step lands on the commanded rung",
            format!("commanded -3.0 dB, echoed {:.2} dB", s.level_db),
        );
        let _ = svc.step_level(3.0);

        // --- ROW: SWEEP UP AND DOWN, with the band-edge bounce. ------------
        // FAILS if the sweep never reaches an edge (a rate derived from the
        // pre-adoption rate sweeps 6.25x slow or fast) or if the bounce does not
        // count - `sweeps_done` only increments on a band-edge arrival.
        if let Err(e) = svc.resume() {
            rows.row(false, "sine resume", format!("{e}"));
            return;
        }
        let (bounced, chunks, s) = match pump_until(&mut svc, MAX_CHUNKS, |s| s.sweeps_done >= 2) {
            Ok(v) => v,
            Err(e) => {
                rows.row(false, "sine sweep pump", e);
                return;
            }
        };
        rows.row(
            bounced,
            "sine sweep completes UP and DOWN with a band-edge bounce",
            format!(
                "{} sweep(s) completed in {chunks} chunk(s) ({:.1} s of test time), tone at \
                 {:.1} Hz",
                s.sweeps_done,
                chunks as f64 * 0.25,
                s.freq_hz
            ),
        );

        // --- INFO: THD. Recorded as DATA, never a pass criterion. ----------
        // The bench plant is a piece of WIRE: its distortion is the 9263's and
        // the 9234's, not the article's, so a threshold here would be pinning a
        // number that means nothing about any real test. Recorded so the value
        // exists the first time a real article makes it interesting.
        println!(
            "  INFO    sine THD at {:.1} Hz: {}",
            s.freq_hz,
            s.thd
                .map(|t| format!(
                    "{:.4}% (wire-plant bound; DATA, not a criterion)",
                    t * 100.0
                ))
                .unwrap_or_else(|| "not available".into())
        );

        // --- ROW: THE CONTINUITY SOAK. -------------------------------------
        // This row proves an ABSENCE, which is exactly the shape of a result that
        // cannot fail on its own - so it is only evidence PAIRED with the baseline
        // section, which showed the same instrument catching the defect when it was
        // there. Post-conversion there is no boundary left to find: `run_chunk` fills
        // CHUNK_SECONDS of TIME out of bounded `run_ticks` batches over ONE
        // continuously running backend, so nothing stops the AO between them. FAILS if
        // the conversion regressed to `Engine::run` (the DMM would show the taper
        // again) or if the run faulted inside the soak.
        dmm_mark(t0, "sine CONTINUITY SOAK BEGINS (no boundary should exist)");
        let soak_start = Instant::now();
        let mut worst_err_db = 0.0f64;
        let mut soak_chunks = 0u32;
        let mut soak_fault = None;
        while soak_start.elapsed().as_secs_f64() < CONTINUITY_S {
            match svc.run_chunk() {
                Ok(alive) => {
                    soak_chunks += 1;
                    let d = level_err_db(&svc.status()).abs();
                    if d.is_finite() {
                        worst_err_db = worst_err_db.max(d);
                    }
                    if !alive {
                        break;
                    }
                }
                Err(e) => {
                    soak_fault = Some(format!("{e}"));
                    break;
                }
            }
        }
        let soak_s = soak_start.elapsed().as_secs_f64();
        dmm_mark(t0, "sine CONTINUITY SOAK ENDS");
        rows.row(
            soak_fault.is_none() && soak_s >= CONTINUITY_S * 0.95,
            "sine drives >= 6 min engaged with no processing boundary (DMM continuity)",
            format!(
                "{soak_s:.0} s engaged over {soak_chunks} chunk(s), worst level error \
                 {worst_err_db:.2} dB{}. Pair with the baseline section's EXPECTED-FAIL row: \
                 that is what proves the DMM sees the defect when it is present.",
                soak_fault
                    .as_ref()
                    .map(|f| format!(", FAULT: {f}"))
                    .unwrap_or_default()
            ),
        );

        // --- ROW: THE RAMPED STOP. -----------------------------------------
        // FAILS if the drive is still non-zero when the tasks stop (the device
        // holds its last sample forever) or if the state machine does not land on
        // Stopped. The COLA half of the predicate is asserted in its own session
        // below, where a second output row actually exists.
        dmm_mark(t0, "sine RAMPED STOP begins");
        if let Err(e) = svc.request_stop() {
            rows.row(false, "sine request_stop", format!("{e}"));
            return;
        }
        let stopped = pump_until(&mut svc, MAX_CHUNKS, |s| {
            matches!(s.state, RunState::Stopped | RunState::Aborted)
        });
        let _ = svc.stop();
        dmm_mark(t0, "sine RAMPED STOP complete (tasks stopped)");
        let drive = svc.drive_scale();
        rows.row(
            matches!(svc.state(), RunState::Stopped) && drive == 0.0,
            "sine ramped stop lands the drive at 0.000 V before the tasks stop",
            format!(
                "state {:?}, drive scale {drive:.6} V{}",
                svc.state(),
                stopped
                    .as_ref()
                    .err()
                    .map(|e| format!(", pump error: {e}"))
                    .unwrap_or_default()
            ),
        );

        // --- ROW: THE COMPUTE MARGIN. --------------------------------------
        // The one row no HERTZFORGE-SIM run can produce: the Sim path
        // arms TEST_RELAX_COMPUTE_BUDGET by name, so the fraction there is
        // meaningless. On this station the budget IS the tick period.
        // FAILS if a per-tick allocation, an unbounded `surface_events` scan, or
        // a display FFT drifted onto the control tick.
        match svc.engine_health() {
            Some(h) => {
                let frac = if h.compute_budget.as_secs_f64() > 0.0 {
                    h.worst_compute.as_secs_f64() / h.compute_budget.as_secs_f64()
                } else {
                    f64::INFINITY
                };
                rows.row(
                    frac < 0.5,
                    "sine worst-tick compute < 0.5 x budget",
                    format!(
                        "{:.3} ms of {:.3} ms ({:.1}%)",
                        h.worst_compute.as_secs_f64() * 1e3,
                        h.compute_budget.as_secs_f64() * 1e3,
                        frac * 100.0
                    ),
                );
                // --- ROW: ZERO ENGINE FAULTS ACROSS THE SECTION. -----------
                rows.row(
                    h.health_faults == 0 && h.ring_overruns == 0,
                    "sine ran with zero engine faults",
                    format!(
                        "{} scheduling fault(s), {} raw-ring drop(s), {} display-ring block(s) \
                         dropped, {} tick(s), {} frame(s) in",
                        h.health_faults,
                        h.ring_overruns,
                        svc.blocks_dropped(),
                        h.ticks,
                        h.frames_in
                    ),
                );
            }
            None => rows.row(
                false,
                "sine engine health available",
                "no engine was built - the run never reached a batch".into(),
            ),
        }
    }

    /// THE NEGATIVE CONTROL. A flat unity plant makes the feedforward
    /// prediction and the servo's answer the same number, so the level row above
    /// would pass with the feedback path severed. Offset the sysID table by
    /// -6 dB and the two disagree by exactly that; the row passes only if the
    /// FEEDBACK walked the drive back onto target.
    ///
    /// FAILS if the servo is not closing: the drive stays 6 dB low and the
    /// measured level sits ~6 dB (a factor of 2) under target.
    fn negative_control_row(rows: &mut Rows, t0: Instant) {
        let mut svc = match build(
            cfg(
                SineMode::Dwell(DwellPlan {
                    freq_hz: 200.0,
                    for_: DwellFor::Seconds(60.0),
                }),
                false,
            ),
            false,
            "sine-negctl",
        ) {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "sine negative-control construct", e);
                return;
            }
        };
        if arm_to_engaged(&mut svc, rows, t0, Some(-6.0), "sine negative-control").is_none() {
            return;
        }
        // 12 s: past the last ladder rung entry (rungs at 3 / 6 / 9 s), so the row
        // measures a settled loop rather than one mid-climb.
        let (_, s) = match pump_for(&mut svc, 12.0, |_| {}) {
            Ok(v) => v,
            Err(e) => {
                rows.row(false, "sine negative-control pump", e);
                return;
            }
        };
        let err = level_err_db(&s);
        rows.row(
            err.abs() < 0.5,
            "sine servo CLOSES a deliberate -6 dB feedforward error (negative control)",
            format!(
                "measured {:.4} V vs target {:.4} V = {err:+.2} dB after 12 s; an open loop \
                 would sit at -6.00 dB",
                s.measured_g.unwrap_or(0.0),
                s.target_g
            ),
        );
        let _ = svc.stop();
    }

    /// The dwell-hold stability row. FAILS if the servo hunts (a compression
    /// preset whose ki is wrong for the adopted rate) or if the tracking filter's
    /// bandwidth was derived from a rate the device is not running at.
    ///
    /// The threshold is stated against the ESTIMATOR: the tracked amplitude is a
    /// single-pole demodulation, not an average of windows, so its own variance
    /// on a clean wire is far below 0.1 dB. 0.5 dB is therefore a control-loop
    /// criterion, not an estimator one.
    fn dwell_row(rows: &mut Rows, t0: Instant) {
        let mut svc = match build(
            cfg(
                SineMode::Dwell(DwellPlan {
                    freq_hz: 200.0,
                    for_: DwellFor::Seconds(120.0),
                }),
                false,
            ),
            false,
            "sine-dwell",
        ) {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "sine dwell construct", e);
                return;
            }
        };
        if arm_to_engaged(&mut svc, rows, t0, None, "sine dwell").is_none() {
            return;
        }
        // Settle first; the row is about the HOLD, not the approach.
        if let Err(e) = pump_for(&mut svc, 10.0, |_| {}).map(|_| ()) {
            rows.row(false, "sine dwell settle", e);
            return;
        }
        let mut samples: Vec<f64> = Vec::new();
        if let Err(e) = pump_for(&mut svc, 10.0, |s| {
            let d = level_err_db(s);
            if d.is_finite() {
                samples.push(d);
            }
        })
        .map(|_| ())
        {
            rows.row(false, "sine dwell hold", e);
            return;
        }
        let n = samples.len().max(1) as f64;
        let mean = samples.iter().sum::<f64>() / n;
        let sigma = (samples.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n).sqrt();
        rows.row(
            !samples.is_empty() && sigma < 0.5,
            "sine dwell hold: sigma < 0.5 dB over 10 s",
            format!(
                "sigma {sigma:.3} dB, mean {mean:+.3} dB over {} sample(s) at 200 Hz",
                samples.len()
            ),
        );
        // --- ROW: THE ABORT TAPER. ------------------------------------------
        // FAILS if Abort is answered as a ramped Stop (which is what `SineState::abort`
        // was, verbatim, before the abort fix) or if the emergency stop never reaches
        // the backend. The 25 ms figure is the BACKEND's taper
        // (`CONTROLLED_STOP_RAMP_MS` on the NI path), not a host ramp - the point of an
        // abort is that the host does NOT ramp. This process cannot see the taper; the
        // DMM window centred on the mark below is what measures it, and the row asserts
        // the state machine took the abort path.
        dmm_mark(t0, "sine ABORT (25 ms backend taper expected on the DMM)");
        let before = Instant::now();
        let aborted = svc.abort("gate: abort taper row");
        let took = before.elapsed();
        rows.row(
            aborted.is_ok() && matches!(svc.state(), RunState::Aborted),
            "sine abort takes the emergency-stop path (not a ramp)",
            format!(
                "state {:?} in {:.1} ms, drive scale {:.6} V. A ramped stop from this level \
                 would take ~{:.2} s at the {} dB/s law - score the DMM window for the \
                 backend's own 25 ms taper.",
                svc.state(),
                took.as_secs_f64() * 1e3,
                svc.drive_scale(),
                hertzforge_sine::service::engine::SineProcessStep::safe_stop_seconds(TARGET_G),
                hertzforge_sine::service::engine::RAMP_DB_PER_S,
            ),
        );
    }

    /// The COLA half of the stop predicate. FAILS at `out_channels = 2` if the
    /// stop only checks ch0: the aux Drive-role output would still be holding its
    /// last full-amplitude sample on a second physical DAC when the tasks stop.
    fn cola_stop_row(rows: &mut Rows, t0: Instant) {
        let mut svc = match build(
            cfg(
                SineMode::Dwell(DwellPlan {
                    freq_hz: 200.0,
                    for_: DwellFor::Seconds(60.0),
                }),
                true,
            ),
            true,
            "sine-cola",
        ) {
            Ok(s) => s,
            Err(e) => {
                rows.row(false, "sine COLA construct", e);
                return;
            }
        };
        if arm_to_engaged(&mut svc, rows, t0, None, "sine COLA").is_none() {
            return;
        }
        if let Err(e) = pump_for(&mut svc, 5.0, |_| {}).map(|_| ()) {
            rows.row(false, "sine COLA settle", e);
            return;
        }
        dmm_mark(
            t0,
            "sine COLA RAMPED STOP begins (tee the DMM on AO1 for this row)",
        );
        if let Err(e) = svc.request_stop() {
            rows.row(false, "sine COLA request_stop", format!("{e}"));
            return;
        }
        let _ = pump_until(&mut svc, MAX_CHUNKS, |s| {
            matches!(s.state, RunState::Stopped | RunState::Aborted)
        });
        let _ = svc.stop();
        dmm_mark(t0, "sine COLA RAMPED STOP complete");
        rows.row(
            matches!(svc.state(), RunState::Stopped) && svc.drive_scale() == 0.0,
            "sine ramped stop at out_channels = 2: ch0 AND the COLA aux reach zero",
            format!(
                "state {:?}, control drive {:.6} V, COLA enabled = {}",
                svc.state(),
                svc.drive_scale(),
                svc.config().cola.enabled
            ),
        );
    }

    /// Absolute frequency command, through the service's own `go_to` verb. Returns
    /// the CLAMPED value the service says it adopted; the caller compares that
    /// against what the status echoes once the command reaches a control tick,
    /// which is what makes "scheduler == NCO == echo" checkable rather than
    /// assumed.
    fn go_to(svc: &SineService, hz: f64) -> Result<f64, String> {
        svc.go_to(hz).map_err(|e| format!("{e}"))
    }
}

// ---------------------------------------------------------------------------
// 6. EXPLORER (host control core) - THE CHASSIS GATE.
//
// The only section whose rows are NUMERIC assertions on the acquired signal
// rather than "it ran". That is deliberate: Explorer is the first module whose
// drive an operator moves BY HAND mid-run, so what has to be proven is not that
// the lifecycle completes but that the number on the panel is the number on the
// wire. Every row below states a measured quantity, a commanded one, and the
// tolerance between them.
//
// Bench contract, the same one every other section in this file carries: AO0
// jumpered to AI0, NOTHING downstream, IEPE OFF. Drive tops out at 0.2 V here.
// ---------------------------------------------------------------------------
fn explorer(rows: &mut Rows) {
    println!();
    println!("--- EXPLORER (host-synthesized open-loop sine, NUMERIC gate) ---");
    use hertzforge_explorer_host::service::{ExplorerHostConfig, ExplorerHostService, RunState};
    use hertzforge_explorer_host::slew::SlewLimits;
    use hertzforge_explorer_host::step::{AmplitudeRamp, SweepDirection};

    /// The rate REQUESTED. The 9234 coerces up its 13.1072 MHz/(256n) ladder
    /// (8192 -> 8533.33), and the whole point of the adopt-then-gate sequence is
    /// that everything downstream runs at what came back, not at this.
    const REQUEST_HZ: f64 = 8_192.0;
    /// Commanded drive for the level row. Well inside the +/-1 V bench contract.
    const DRIVE_VPK: f64 = 0.2;
    /// Batch budget for any one wait. At 2 blocks per batch and ~20 ms a block,
    /// 4000 batches is ~2.5 minutes: long enough that a slow ramp is not a
    /// spurious FAIL, short enough that a hung one does not sit on the bench.
    const MAX_BATCHES: u32 = 4_000;

    let st = station();

    // A VOLTS-EXACT control row: `sensitivity_mv_per_eu = 1000` makes the host's
    // EU conversion the identity (`eu = volts * 1000 / 1000`), so every measured
    // number below is in VOLTS and compares directly against the commanded
    // volts. The shared `table()` helper uses the 100 mV/EU accel scaling every
    // other section wants, under which a 0.2 V tone reads 2.0 EU - correct, and
    // useless for a commanded-vs-measured assertion.
    let table = {
        use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
        let mut t = ChannelTable::new();
        let mut ctrl = Channel::new(1);
        ctrl.role = ChannelRole::Control;
        ctrl.units = Units::VoltageV;
        ctrl.sensitivity_mv_per_eu = 1000.0;
        ctrl.range_max = 1.0;
        t.channels.push(ctrl);
        let mut drv = Channel::new(2);
        drv.role = ChannelRole::Drive;
        drv.units = Units::VoltageV;
        t.channels.push(drv);
        t
    };

    let cfg = ExplorerHostConfig {
        fs_hz: REQUEST_HZ,
        band_hz: (50.0, 2_000.0),
        start_hz: 100.0,
        max_drive_vpk: 0.5,
        // Faster than the operator defaults (1 oct/s, 6 dB/s) so the gate fits a
        // bench sitting: a 40 dB ramp at 30 dB/s is 1.3 s rather than 6.7 s.
        // Both are still SLEWS - these rows prove travel is BOUNDED, not slow.
        slew: SlewLimits {
            freq_oct_per_s: 8.0,
            drive_db_per_s: 30.0,
        },
        ramp: AmplitudeRamp::default(),
        display_nfft: 2_048,
        ring_blocks: 16,
    };

    let mut svc = match ExplorerHostService::new(cfg, table, st.backend, st.station) {
        Ok(s) => s.with_block_frames(hertzforge_hal::default_block_frames(REQUEST_HZ)),
        Err(e) => {
            rows.row(false, "explorer construct", format!("{e}"));
            return;
        }
    };
    if let Err(e) = svc.arm() {
        rows.row(
            false,
            "explorer arm (adopt-actual + arm gate)",
            format!("{e}"),
        );
        return;
    }
    let fs = svc.actual_fs_hz();
    rows.row(
        true,
        "explorer ARMED open-loop on NI-REAL",
        format!(
            "requested {REQUEST_HZ:.1} Hz, driver programmed {fs:.3} Hz; band adopted as \
             {:.1}..{:.1} Hz",
            svc.band_hz().0,
            svc.band_hz().1
        ),
    );
    if let Err(e) = svc.start() {
        rows.row(false, "explorer start", format!("{e}"));
        return;
    }

    // One batch, then one analysis pass. `None` on a run fault (the FAIL row is
    // printed by the macro; the caller returns).
    macro_rules! pump {
        ($svc:expr) => {{
            match $svc.run_batches(1) {
                Ok(alive) => Some(($svc.service_analysis(), alive)),
                Err(e) => {
                    rows.row(false, "explorer pump", format!("{e}"));
                    None
                }
            }
        }};
    }

    // --- ROW 1: the 0.000 V ARM POSTURE, MEASURED. --------------------------
    let mut now;
    {
        let mut s = svc.service_analysis();
        for _ in 0..8 {
            let Some((next, _)) = pump!(svc) else { return };
            s = next;
        }
        now = s;
    }
    let armed_rms = now.per_channel_rms.first().copied().unwrap_or(f64::NAN);
    rows.row(
        now.commanded_vpk == 0.0 && armed_rms < 0.02,
        "explorer arms at 0.000 V (measured RMS < 0.02 V)",
        format!(
            "commanded {:.4} V, measured {armed_rms:.5} V RMS",
            now.commanded_vpk
        ),
    );

    // --- ROW 2: RAMP TO 0.2 Vpk, measured against the command. --------------
    if let Err(e) = svc.set_amplitude_vpk(DRIVE_VPK) {
        rows.row(false, "explorer set_amplitude_vpk", format!("{e}"));
        return;
    }
    let mut arrived = false;
    for _ in 0..MAX_BATCHES {
        let Some((next, alive)) = pump!(svc) else {
            return;
        };
        now = next;
        if (now.commanded_vpk - DRIVE_VPK).abs() < 1e-9 {
            arrived = true;
            break;
        }
        if !alive {
            break;
        }
    }
    if !arrived {
        rows.row(
            false,
            "explorer drive ramp reaches the command",
            "timed out".into(),
        );
        return;
    }
    // Let the analysis window fill AT the settled level before measuring it.
    for _ in 0..16 {
        let Some((next, _)) = pump!(svc) else { return };
        now = next;
    }
    let measured_vpk =
        now.per_channel_rms.first().copied().unwrap_or(f64::NAN) * std::f64::consts::SQRT_2;
    let level_err = (measured_vpk - DRIVE_VPK).abs() / DRIVE_VPK;
    rows.row(
        level_err < 0.05,
        "explorer 0.2 Vpk: measured RMS x sqrt(2) within 5%",
        format!(
            "measured {measured_vpk:.4} Vpk vs commanded {DRIVE_VPK:.4} Vpk ({:.2}% off)",
            level_err * 100.0
        ),
    );

    // --- ROW 3: RETUNE 100 -> 400 Hz, measured against the command. ---------
    let commanded_hz = match svc.set_frequency(400.0) {
        Ok(hz) => hz,
        Err(e) => {
            rows.row(false, "explorer set_frequency", format!("{e}"));
            return;
        }
    };
    let mut measured_hz = None;
    for _ in 0..MAX_BATCHES {
        let Some((next, alive)) = pump!(svc) else {
            return;
        };
        now = next;
        // A manual retune lands in ONE generated frame (no slew), but the drive is
        // written ahead of the acquisition by the pipeline (551 frames here). The
        // measured window resets on the retune and opens only once that pipeline
        // has drained, so the FIRST reading to mature is the new tone's, measured
        // off the wire - which is why this row proves the retune ARRIVED, not
        // merely that it was accepted. (2026-11-01, before the window
        // waited for the pipeline: 379.41 Hz, a blend with the 100 Hz in flight.)
        if let Some(hz) = now.measured_freq_hz {
            measured_hz = Some(hz);
            break;
        }
        if !alive {
            break;
        }
    }
    match measured_hz {
        Some(hz) => {
            let err = (hz - commanded_hz).abs() / commanded_hz;
            rows.row(
                err < 0.02,
                "explorer retune 100 -> 400 Hz: measured within 2%",
                format!(
                    "measured {hz:.2} Hz vs commanded {commanded_hz:.2} Hz ({:.2}% off)",
                    err * 100.0
                ),
            );
        }
        None => rows.row(
            false,
            "explorer retune 100 -> 400 Hz",
            "no frequency measurement matured (drive at zero, or the tone never settled)".into(),
        ),
    }

    // --- ROW 4: A DECADE SWEEP, UP AND DOWN, COMPLETING. --------------------
    // Park at 200 Hz first so the up sweep covers a clean decade to the 2 kHz
    // band edge; the down sweep then covers more than a decade back to 50 Hz.
    if svc.set_frequency(200.0).is_err() {
        rows.row(
            false,
            "explorer sweep setup",
            "set_frequency(200) refused".into(),
        );
        return;
    }
    let mut sweep_ok = true;
    let mut detail = String::new();
    for (dir, from, edge, label) in [
        (SweepDirection::Up, 200.0f64, 2_000.0f64, "up"),
        (SweepDirection::Down, 2_000.0f64, 50.0f64, "down"),
    ] {
        // Wait for the tone to be AT the start before timing the sweep.
        for _ in 0..MAX_BATCHES {
            let Some((next, _)) = pump!(svc) else { return };
            now = next;
            if (now.commanded_freq_hz - from).abs() < 1.0 {
                break;
            }
        }
        if svc.start_sweep(dir, 240.0).is_err() {
            rows.row(false, "explorer start_sweep", format!("{label} refused"));
            return;
        }
        let mut halted = false;
        for _ in 0..MAX_BATCHES {
            let Some((next, alive)) = pump!(svc) else {
                return;
            };
            now = next;
            if now.sweep.map(|w| w.halted).unwrap_or(false) {
                halted = true;
                break;
            }
            if !alive {
                break;
            }
        }
        let decades = (from / edge).max(edge / from).log10();
        let at_edge = (now.target_freq_hz - edge).abs() < 1e-6;
        sweep_ok &= halted && at_edge && decades >= 1.0;
        detail.push_str(&format!(
            "{label}: halted={halted} at {:.1} Hz (edge {edge:.1}, {decades:.2} decade(s)); ",
            now.target_freq_hz
        ));
    }
    rows.row(
        sweep_ok,
        "explorer decade sweep up + down completes and halts at the edge",
        detail,
    );

    // --- ROW 5: THE RAMPED STOP. --------------------------------------------
    svc.request_stop();
    let mut stopped = false;
    for _ in 0..MAX_BATCHES {
        let Some((next, alive)) = pump!(svc) else {
            return;
        };
        now = next;
        if now.state == RunState::Stopped {
            stopped = true;
            break;
        }
        if !alive {
            break;
        }
    }
    let stop_rms = now.per_channel_rms.first().copied().unwrap_or(f64::NAN);
    rows.row(
        stopped && now.commanded_vpk == 0.0,
        "explorer ramped stop reaches 0.000 V and stops the tasks",
        format!(
            "state {:?}, commanded {:.5} V, last measured {stop_rms:.5} V RMS, reason: {}",
            now.state,
            now.commanded_vpk,
            now.end
                .as_ref()
                .map(|e| e.reason.clone())
                .unwrap_or_else(|| "(none)".into())
        ),
    );

    // --- ROW 6: THE COMPUTE MARGIN. -----------------------------------------
    // The one row here no Sim run can produce: on HERTZFORGE-SIM the
    // compute budget is explicitly RELAXED and the jitter guard explicitly
    // DISABLED, so a green there says nothing at all. On this station the budget
    // IS the tick period, which is what makes the fraction meaningful.
    let worst = now.worst_tick_compute;
    let budget = now.compute_budget;
    let frac = if budget.as_secs_f64() > 0.0 {
        worst.as_secs_f64() / budget.as_secs_f64()
    } else {
        f64::INFINITY
    };
    println!(
        "  INFO    explorer worst-tick compute {:.3} ms vs budget {:.3} ms ({:.1}% of budget)",
        worst.as_secs_f64() * 1e3,
        budget.as_secs_f64() * 1e3,
        frac * 100.0
    );
    rows.row(
        frac < 0.5,
        "explorer worst-tick compute < 0.5 x budget",
        format!(
            "{:.3} ms of {:.3} ms ({:.1}%); the display FFT is OFF this tick by construction",
            worst.as_secs_f64() * 1e3,
            budget.as_secs_f64() * 1e3,
            frac * 100.0
        ),
    );

    // --- ROW 7: NO ENGINE FAULTS ACROSS THE WHOLE SECTION. ------------------
    rows.row(
        now.health_faults == 0 && now.ring_overruns == 0 && now.observed_trip.is_none(),
        "explorer ran with zero engine faults",
        format!(
            "{} scheduling fault(s), {} raw-ring drop(s), {} analysis block(s) dropped, \
             {} tick(s), {} frame(s) generated",
            now.health_faults,
            now.ring_overruns,
            now.blocks_dropped,
            now.engine_ticks,
            now.frames_generated
        ),
    );

    // --- ROW 8: DRIVER DIAGNOSTICS REACHED A LOG. ---------------------------
    // Not a pass/fail on the COUNT - a coercion warning is expected here, since
    // the 9234 coerces the requested rate up its ladder. What is being proven is
    // that the driver's own words reached a human-readable surface at all: they
    // are drained from the backend every batch and would otherwise be discarded.
    if now.driver_warnings.is_empty() {
        println!("  INFO    explorer driver diagnostics: none reported this run");
    } else {
        println!(
            "  INFO    explorer driver diagnostics ({}):",
            now.driver_warnings.len()
        );
        for w in &now.driver_warnings {
            println!("            {w}");
        }
    }
}
