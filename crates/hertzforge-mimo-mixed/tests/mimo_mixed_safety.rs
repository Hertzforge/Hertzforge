//! The run's protections on the simulator: every abort takes the drive to
//! zero and names its part (and control), a law's failure, trap or late
//! answer in either role ends in the controlled stop, the stop and abort
//! ramps, and the real-time start refusal.
#![allow(clippy::needless_range_loop)]

mod common;
mod service_common;

use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_mimo_mixed::config::MimoPartBody;
use hertzforge_mimo_mixed::service::*;
use service_common::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Run to the end (or `limit_s` of station time), the drive's peak per chunk
/// over every drive.
fn run_out(svc: &mut MimoMixedService, limit_s: f64) -> Vec<f64> {
    let mut peaks = Vec::new();
    while svc.status().t_s < limit_s {
        let going = svc.run_chunk().expect("chunk");
        peaks.push(
            svc.status()
                .drives
                .iter()
                .map(|d| d.peak_v)
                .fold(0.0, f64::max),
        );
        if !going {
            break;
        }
    }
    peaks
}

fn ended(svc: &MimoMixedService) -> RunEnd {
    let e = svc.end().cloned().expect("the run ended");
    println!(
        "end: {} part {:?} control {:?}: {}",
        e.name, e.part, e.control, e.words
    );
    e
}

/// The checks every abort shares: it is an abort and the drive is at zero.
fn assert_aborted_to_zero(svc: &MimoMixedService, e: &RunEnd) {
    assert!(e.aborted, "{}", e.name);
    assert!(svc.drive_is_zero(), "{}: drive left playing", e.name);
}

fn two_tone_config(full_s: f64) -> hertzforge_mimo_mixed::config::MimoMixedConfig {
    let s = SineStructure::Linear4x4;
    run_config(
        s,
        0.5,
        vec![tone("t1", fixed(150.0), 1.0), tone("t2", fixed(700.0), 1.0)],
        full_s,
    )
}

/// Requirement: a tone whose law, once the run is in, asks 0.5 dB less
/// drive every tick (under the step cap, so never bound) ends the run
/// `tone_abort` naming that tone and a control, with the drive at zero.
#[test]
fn mimo_mixed_tone_abort_names_its_part_and_ramps_to_zero() {
    let s = SineStructure::Linear4x4;
    let cfg = two_tone_config(30.0);
    let mut laws = MimoMixedLaws::builtin(&cfg).expect("laws");
    laws.tones = Arc::new(FaultyTone {
        inner: laws.tones.clone(),
        fault: Fault::Fade,
        after: 50,
    });
    let mut svc = MimoMixedService::new(
        cfg,
        table(s),
        backend(s, 7),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        laws,
    )
    .expect("service");
    engage(&mut svc);
    run_out(&mut svc, 80.0);
    let e = ended(&svc);
    assert_eq!(e.name, "tone_abort");
    assert!(
        matches!(e.part.as_deref(), Some("t1") | Some("t2")),
        "{:?}",
        e.part
    );
    assert!(e.control.is_some());
    assert_aborted_to_zero(&svc, &e);
}

/// Requirement: a floor whose abort band is 0.05 dB either side ends the run
/// `floor_envelope`, naming the floor and a control, with the drive at zero.
#[test]
fn mimo_mixed_floor_envelope_names_the_floor_and_ramps_to_zero() {
    let s = SineStructure::Linear4x4;
    let mut cfg = two_tone_config(30.0);
    if let MimoPartBody::Floor(f) = &mut cfg.parts[0].body {
        f.alarm_db = hertzforge_mimo_mixed::config::PerControl::All((0.02, 0.02));
        f.abort_db = hertzforge_mimo_mixed::config::PerControl::All((0.05, 0.05));
    }
    cfg.soft_start_relax_db = 0.0;
    let mut svc = service(s, cfg);
    engage(&mut svc);
    run_out(&mut svc, 80.0);
    let e = ended(&svc);
    assert_eq!(e.name, "floor_envelope");
    assert_eq!(e.part.as_deref(), Some("floor"));
    assert!(e.control.is_some());
    assert_aborted_to_zero(&svc, &e);
}

/// Requirement: a floor given 0.5 s to equalize ends the run
/// `floor_not_equalized`, naming the floor, with the drive at zero.
#[test]
fn mimo_mixed_floor_not_equalized_names_the_floor_and_ramps_to_zero() {
    let s = SineStructure::Linear4x4;
    let mut cfg = two_tone_config(30.0);
    cfg.floor_settle_s = 0.5;
    let mut svc = service(s, cfg);
    engage(&mut svc);
    run_out(&mut svc, 80.0);
    let e = ended(&svc);
    assert_eq!(e.name, "floor_not_equalized");
    assert_eq!(e.part.as_deref(), Some("floor"));
    assert_aborted_to_zero(&svc, &e);
}

/// Requirement: a servoed band whose abort band is 0.01 dB either side ends
/// the run `band_abort`, naming the band and a control, with the drive at
/// zero.
#[test]
fn mimo_mixed_band_abort_names_its_part_and_ramps_to_zero() {
    let s = SineStructure::Linear4x4;
    let mut b = band("nb", fixed(900.0), 40.0, added(3e-3, 0.5));
    if let MimoPartBody::Narrowband(nb) = &mut b.body {
        nb.alarm_db = hertzforge_mimo_mixed::config::PerControl::All((0.005, 0.005));
        nb.abort_db = hertzforge_mimo_mixed::config::PerControl::All((0.01, 0.01));
    }
    let mut cfg = run_config(s, 0.5, vec![b], 30.0);
    cfg.law.bands = hertzforge_mixed::LawPick::new("per_control");
    let mut svc = service(s, cfg);
    engage(&mut svc);
    run_out(&mut svc, 80.0);
    let e = ended(&svc);
    assert_eq!(e.name, "band_abort");
    assert_eq!(e.part.as_deref(), Some("nb"));
    assert!(e.control.is_some());
    assert_aborted_to_zero(&svc, &e);
}

/// Requirement: a control whose input range is 0.5 g (the tones alone peak
/// near 2 g) ends the run `input_overload`, naming the channel's control,
/// with the drive at zero.
#[test]
fn mimo_mixed_input_overload_names_the_control_and_ramps_to_zero() {
    let s = SineStructure::Linear4x4;
    let cfg = two_tone_config(30.0);
    let mut t = table(s);
    t.channels[1].range_max = 0.5;
    let laws = MimoMixedLaws::builtin(&cfg).expect("laws");
    let mut svc = MimoMixedService::new(
        cfg,
        t,
        backend(s, 7),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        laws,
    )
    .expect("service");
    svc.arm().expect("arm");
    let r = svc.identify().map(|_| ()).and_then(|()| {
        svc.coherence_gate()?;
        svc.proceed()
    });
    if r.is_ok() {
        run_out(&mut svc, 80.0);
    }
    let e = ended(&svc);
    assert_eq!(e.name, "input_overload");
    assert_eq!(e.control, Some(1));
    assert_aborted_to_zero(&svc, &e);
}

/// Requirement: a stop at full level ramps the drive down over the shutdown
/// ramp (0.5 s): the chunk after the stop still plays, under the chunk
/// before it, and the drive is at zero within a second; the run ends
/// `stopped`, not an abort.
#[test]
fn mimo_mixed_stop_ramps_the_drive_down() {
    let s = SineStructure::Linear4x4;
    let mut svc = service(s, two_tone_config(30.0));
    engage(&mut svc);
    assert!(run_until(&mut svc, MimoMixedPhase::FullLevel, 60.0));
    let t1 = svc.status().t_s + 1.0;
    run_out(&mut svc, t1);
    let before = svc
        .status()
        .drives
        .iter()
        .map(|d| d.peak_v)
        .fold(0.0, f64::max);
    svc.stop().expect("stop");
    let t0 = svc.status().t_s;
    let peaks = run_out(&mut svc, t0 + 5.0);
    println!("before {before:.3} V; after the stop {peaks:.3?}");
    assert!(peaks[0] > 0.0 && peaks[0] < before, "a ramp, not a step");
    let at_zero = peaks.iter().position(|p| *p == 0.0).expect("reaches zero");
    assert!(at_zero as f64 * 0.25 <= 1.0, "zero after {at_zero} chunks");
    let e = ended(&svc);
    assert_eq!(e.name, "stopped");
    assert!(!e.aborted);
    assert!(svc.drive_is_zero());
}

/// Requirement: an operator abort at full level takes the drive to zero
/// within two chunks (0.5 s), names itself, and leaves the drive at zero.
#[test]
fn mimo_mixed_abort_takes_the_drive_to_zero() {
    let s = SineStructure::Linear4x4;
    let mut svc = service(s, two_tone_config(30.0));
    engage(&mut svc);
    assert!(run_until(&mut svc, MimoMixedPhase::FullLevel, 60.0));
    svc.abort("test").expect("abort");
    let t0 = svc.status().t_s;
    let peaks = run_out(&mut svc, t0 + 5.0);
    println!("after the abort {peaks:.3?}");
    let at_zero = peaks.iter().position(|p| *p == 0.0).expect("reaches zero");
    assert!(at_zero <= 2, "zero after {at_zero} chunks");
    let e = ended(&svc);
    assert_eq!(e.name, "operator_abort");
    assert_aborted_to_zero(&svc, &e);
}

/// Requirement: an operator abort while a 5 s stop ramps down is taken: it
/// replaces the stop, the drive is at zero within two chunks (0.5 s) of it,
/// and the run ends `operator_abort`, an abort.
#[test]
fn mimo_mixed_an_abort_during_a_stop_takes_over_at_the_abort_ramp() {
    let s = SineStructure::Linear4x4;
    let mut cfg = two_tone_config(30.0);
    cfg.shutdown_ramp_s = 5.0;
    let mut svc = service(s, cfg);
    engage(&mut svc);
    assert!(run_until(&mut svc, MimoMixedPhase::FullLevel, 60.0));
    svc.stop().expect("stop");
    assert!(svc.run_chunk().expect("chunk"));
    svc.abort("test").expect("an abort during the stop");
    let t0 = svc.status().t_s;
    let peaks = run_out(&mut svc, t0 + 10.0);
    println!("after the abort {peaks:.3?}");
    let at_zero = peaks.iter().position(|p| *p == 0.0).expect("reaches zero");
    assert!(at_zero <= 2, "zero after {at_zero} chunks");
    let e = ended(&svc);
    assert_eq!(e.name, "operator_abort");
    assert_aborted_to_zero(&svc, &e);
}

/// The simulator with control 1's input forced to 1000 once `spike` is set.
struct Spike {
    inner: Box<dyn hertzforge_hal::Backend>,
    n_in: usize,
    spike: Arc<std::sync::atomic::AtomicBool>,
}

impl hertzforge_hal::Backend for Spike {
    fn caps(&self) -> hertzforge_hal::Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.close()
    }
    fn configure(
        &mut self,
        table: &hertzforge_channel_table::ChannelTable,
        acq: &hertzforge_hal::AcquisitionConfig,
    ) -> Result<hertzforge_hal::ConfiguredActual, hertzforge_hal::HalError> {
        self.inner.configure(table, acq)
    }
    fn arm(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.stop()
    }
    fn read_input(
        &mut self,
        out: &mut [f32],
    ) -> Result<hertzforge_hal::InputRead, hertzforge_hal::HalError> {
        let r = self.inner.read_input(out)?;
        if let hertzforge_hal::InputRead::Frames(f) = r {
            if self.spike.load(Ordering::Relaxed) {
                for k in 0..f {
                    out[k * self.n_in] = 1000.0;
                }
            }
        }
        Ok(r)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, hertzforge_hal::HalError> {
        self.inner.write_drive(drive)
    }
    fn poll_status(&mut self) -> hertzforge_hal::BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.emergency_stop()
    }
}

/// Requirement: an input overload while a 5 s stop ramps down is reported
/// as the end (`input_overload`, an abort, naming control 0) and ramps at
/// the abort ramp: the drive is at zero within two chunks of it.
#[test]
fn mimo_mixed_a_trip_during_a_stop_is_the_end_and_ramps_at_once() {
    let s = SineStructure::Linear4x4;
    let mut cfg = two_tone_config(30.0);
    cfg.shutdown_ramp_s = 5.0;
    let mut t = table(s);
    t.channels[0].range_max = 100.0;
    let spike = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let laws = MimoMixedLaws::builtin(&cfg).expect("laws");
    let b = Spike {
        inner: backend(s, 7),
        n_in: s.dims().0,
        spike: spike.clone(),
    };
    let mut svc = MimoMixedService::new(
        cfg,
        t,
        Box::new(b),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        laws,
    )
    .expect("service");
    engage(&mut svc);
    assert!(run_until(&mut svc, MimoMixedPhase::FullLevel, 60.0));
    svc.stop().expect("stop");
    assert!(svc.run_chunk().expect("chunk"));
    spike.store(true, Ordering::Relaxed);
    let t0 = svc.status().t_s;
    let peaks = run_out(&mut svc, t0 + 10.0);
    println!("after the overload {peaks:.3?}");
    let at_zero = peaks.iter().position(|p| *p == 0.0).expect("reaches zero");
    assert!(at_zero <= 2, "zero after {at_zero} chunks");
    let e = ended(&svc);
    assert_eq!(e.name, "input_overload");
    assert_eq!(e.control, Some(0));
    assert_aborted_to_zero(&svc, &e);
}

/// Requirement: the run's step holds each drive to a peak abort above its
/// peak ceiling, so the clamp at the ceiling is a backstop under the abort:
/// the stated `abort_vpk` where one is stated, the ceiling plus 1 dB where
/// none is.
#[test]
fn mimo_mixed_each_drive_aborts_above_its_peak_ceiling() {
    let s = SineStructure::Linear4x4;
    let mut cfg = two_tone_config(30.0);
    let stated = cfg.drives[2].peak_v * 1.5;
    cfg.drives[2].abort_vpk = Some(stated);
    let peaks: Vec<f64> = cfg.drives.iter().map(|d| d.peak_v).collect();
    let mut svc = service(s, cfg);
    engage(&mut svc);
    let st = svc.status();
    for (j, d) in st.drives.iter().enumerate() {
        println!(
            "drive {j}: peak ceiling {} V, abort {} V",
            peaks[j], d.abort_vpk
        );
        assert!(d.abort_vpk > peaks[j], "drive {j}");
        let want = if j == 2 {
            stated
        } else {
            peaks[j] * 10f64.powf(1.0 / 20.0)
        };
        assert!((d.abort_vpk - want).abs() <= 1e-12 * want, "drive {j}");
    }
}

/// A run on BaseArticle6x4 (four controls, two monitors) with one 2 g tone
/// at 150 Hz and monitor channel 5 a limit with a flat ceiling of
/// `ceiling` and a 3 dB abort margin; the worst `over_db` it read, and the
/// end.
fn limit_run(ceiling: f64) -> (f64, RunEnd) {
    let s = SineStructure::BaseArticle6x4;
    let mut cfg = run_config(s, 0.5, vec![tone("t1", fixed(150.0), 2.0)], 10.0);
    cfg.limits = vec![hertzforge_mimo_random::config::LimitSpec {
        channel_index: 5,
        ceiling_psd: vec![ceiling],
        abort_margin_db: 3.0,
    }];
    let mut svc = service(s, cfg);
    engage(&mut svc);
    let mut worst = f64::NEG_INFINITY;
    while svc.status().t_s < 80.0 {
        let going = svc.run_chunk().expect("chunk");
        let st = svc.status();
        worst = worst.max(st.limits[0].over_db);
        if !going {
            break;
        }
    }
    (worst, ended(&svc))
}

/// Requirement: a limit channel is judged on what it carries, the tones
/// included (the residual the floor law sees has every tone subtracted).
/// A run whose limit ceiling is 1e3 reads the channel's worst line; with
/// the ceiling 6 dB under that reading the run ends `limit_abort` (past the
/// 3 dB margin), an abort, at zero; with it 6 dB over the run completes.
/// The tone alone carries that worst line: the random part's worst line on
/// the monitor reads 1.3e-2 g^2/Hz (measured with the tones left out of the
/// check), 14 dB under the tone's 0.37 g^2/Hz and 8 dB under the lower
/// ceiling, so judging the residual alone never ends this run.
#[test]
fn mimo_mixed_a_tone_over_a_limit_ceiling_ends_the_run() {
    let (probe_db, probe_end) = limit_run(1e3);
    assert_eq!(probe_end.name, "completed", "{}", probe_end.words);
    let carried = 1e3 * 10f64.powf(probe_db / 10.0);
    println!("probe: worst line carries {carried:.4e} (g^2/Hz)");
    assert!(
        carried > 1e-1,
        "the tone's line, not the floor: {carried:e}"
    );
    let (over_db, end) = limit_run(carried / 10f64.powf(0.6));
    println!(
        "ceiling 6 dB under: worst {over_db:.2} dB, end {}",
        end.name
    );
    assert_eq!(end.name, "limit_abort", "{}", end.words);
    assert!(end.aborted);
    let (under_db, end) = limit_run(carried * 10f64.powf(0.6));
    println!(
        "ceiling 6 dB over: worst {under_db:.2} dB, end {}",
        end.name
    );
    assert_eq!(end.name, "completed", "{}", end.words);
}

/// A floor-only run on Linear4x4 held 12 s at full level, each drive's rms
/// ceiling `ceiling_v` (its abort at the ceiling when `abort_at_ceiling`,
/// else the default ceiling plus 1 dB); the end and whether the drive is at
/// zero.
fn rms_run(ceiling_v: f64, abort_at_ceiling: bool) -> (RunEnd, bool) {
    let s = SineStructure::Linear4x4;
    let mut cfg = run_config(s, 0.5, vec![], 12.0);
    for d in &mut cfg.drives {
        d.ceiling_vrms = ceiling_v;
        d.abort_vrms = abort_at_ceiling.then_some(ceiling_v);
    }
    let mut svc = service(s, cfg);
    engage_past_the_prediction(&mut svc);
    run_out(&mut svc, 80.0);
    (ended(&svc), svc.drive_is_zero())
}

/// Requirement: each drive's random part is judged against its rms abort
/// (MIMO Random's `drive_vrms`, at `abort_vrms` or the ceiling plus 1 dB)
/// on every window of `nfft` frames. The floor alone on this fixture asks
/// 0.156 to 0.158 V rms of each drive at full level (its drive matrix's
/// rms, read while writing this test), so a 0.15 V ceiling holds every
/// drive at its ceiling, 0.4 dB under what it asks, with the floor still
/// equalizing. With the abort at the ceiling the windows scatter about
/// their abort and the run ends `drive_vrms`, an abort at zero naming a
/// drive; with the default abort 1 dB over the ceiling the same run
/// completes.
#[test]
fn mimo_mixed_a_drive_at_its_rms_abort_ends_the_run() {
    let (end, zero) = rms_run(0.15, true);
    assert_eq!(end.name, "drive_vrms", "{}", end.words);
    assert!(end.aborted && end.drive.is_some());
    assert!(zero, "drive left playing");
    let (end, _) = rms_run(0.15, false);
    assert_eq!(end.name, "completed", "{}", end.words);
}

/// A run on Linear4x4 with the floor and one 40 Hz band at 900 Hz, servoed
/// by `scalar_servo`, of `level` g^2/Hz added (abort band 40 dB either way, so the band's own
/// abort stays out of it), each drive's rms ceiling `ceiling_v` and the
/// change cap's rise `up_db`, held 12 s at full level; the end, whether the
/// band was ever shown limiting, and the band's error over the controls at
/// the end, dB.
fn band_run(level: f64, ceiling_v: f64, up_db: f64) -> (RunEnd, bool, f64) {
    let s = SineStructure::Linear4x4;
    let mut b = band("nb", fixed(900.0), 40.0, added(level, 0.5));
    if let MimoPartBody::Narrowband(nb) = &mut b.body {
        nb.alarm_db = hertzforge_mimo_mixed::config::PerControl::All((40.0, 40.0));
        nb.abort_db = hertzforge_mimo_mixed::config::PerControl::All((40.0, 40.0));
    }
    let mut cfg = run_config(s, 0.5, vec![b], 12.0);
    cfg.law.bands = hertzforge_mixed::LawPick::new("scalar_servo");
    for d in &mut cfg.drives {
        d.ceiling_vrms = ceiling_v;
    }
    cfg.caps.up_db = up_db;
    let mut svc = service(s, cfg);
    engage_past_the_prediction(&mut svc);
    let mut limiting = false;
    let mut err = f64::NAN;
    while svc.status().t_s < 80.0 {
        let st = svc.status();
        if st.phase == MimoMixedPhase::FullLevel {
            let e = &st.bands[0].error_db;
            err = e.iter().sum::<f64>() / e.len() as f64;
        }
        let going = svc.run_chunk().expect("chunk");
        limiting |= svc.status().bands.iter().any(|b| b.limiting);
        if !going {
            break;
        }
    }
    println!(
        "band {level} g^2/Hz, ceilings {ceiling_v} V, cap {up_db} dB: limiting {limiting}, \
         band error {err:.2} dB"
    );
    (ended(&svc), limiting, err)
}

/// Requirement: a servoed band's drive is held under the drives' rms
/// ceilings with the floor's (the bands' share cut to what the floor's
/// leaves) and its rise under the change cap, and the band is shown
/// limiting while either holds it, so its servo does not wind up. The
/// floor alone asks about 0.157 V rms of each drive and a band of 0.01
/// g^2/Hz over 40 Hz about 0.075 V more: with the default ceilings the band
/// reaches its level and is never shown limiting; with 0.16 V ceilings the
/// band gets what the floor leaves, reads well short of its level, and is
/// shown limiting. With the default ceilings and a 0.3 dB change cap (the
/// floor still equalizes under it) the servo's rises are capped: the band
/// is shown limiting there too.
#[test]
fn mimo_mixed_a_band_is_held_by_the_drive_ceilings_and_the_change_cap() {
    let (end, limiting, free_db) = band_run(1e-2, 2.0, 3.5);
    assert_eq!(end.name, "completed", "{}", end.words);
    assert!(!limiting, "shown limiting with nothing holding it");
    assert!(free_db.abs() < 1.5, "the free band read {free_db:.2} dB");
    let (end, limiting, held_db) = band_run(1e-2, 0.165, 3.5);
    assert_eq!(end.name, "completed", "{}", end.words);
    assert!(
        limiting,
        "the ceilings held the band but it was not shown limiting"
    );
    assert!(held_db < -3.0, "the held band read {held_db:.2} dB");
    let (_, limiting, _) = band_run(3e-3, 2.0, 0.3);
    assert!(
        limiting,
        "the change cap held the band but it was not shown limiting"
    );
}

/// Requirement: the identification holds MIMO Random's protections on
/// each segment: a drive whose identification rms reaches its rms abort
/// ends it `drive_vrms` (each ceiling here half the rms a probe
/// identification settles on), and a limit channel over its ceiling plus
/// its margin at full level ends it `limit_abort` (BaseArticle6x4's
/// monitor channel 5 under a ceiling of 1e-12 g^2/Hz); each an abort with
/// the drive at zero.
#[test]
fn mimo_mixed_the_identification_aborts_on_drive_rms_and_limits() {
    let s = SineStructure::Linear4x4;
    let cfg = run_config(s, 0.5, vec![], 6.0);
    let mut svc = service(s, cfg.clone());
    svc.arm().expect("arm");
    let used = svc.identify().expect("identify").drive_vrms.clone();
    println!("identified at {used:.4?} V rms");
    let mut cfg = cfg;
    for (d, v) in cfg.drives.iter_mut().zip(&used) {
        d.ceiling_vrms = v / 2.0;
    }
    let mut svc = service(s, cfg);
    svc.arm().expect("arm");
    assert!(svc.identify().is_err(), "identified past the rms abort");
    let e = ended(&svc);
    assert_eq!(e.name, "drive_vrms", "{}", e.words);
    assert!(e.drive.is_some());
    assert_aborted_to_zero(&svc, &e);

    let s = SineStructure::BaseArticle6x4;
    let mut cfg = run_config(s, 0.5, vec![], 6.0);
    cfg.limits = vec![hertzforge_mimo_random::config::LimitSpec {
        channel_index: 5,
        ceiling_psd: vec![1e-12],
        abort_margin_db: 3.0,
    }];
    let mut svc = service(s, cfg);
    svc.arm().expect("arm");
    assert!(svc.identify().is_err(), "identified past the limit");
    let e = ended(&svc);
    assert_eq!(e.name, "limit_abort", "{}", e.words);
    assert_aborted_to_zero(&svc, &e);
}

// =============================================================================
// Law faults in either role.
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fault {
    Fail,
    Trap,
    /// Asks 0.5 dB less than the drive in force every tick (tone role).
    Fade,
}

/// A floor law that answers as the built-in until its `after`th iterate,
/// then fails or panics on every call.
struct FaultyFloor {
    inner: Arc<dyn hertzforge_random_laws::MimoLawFactory>,
    fault: Fault,
    after: u64,
}

struct FaultyFloorLaw {
    inner: Box<dyn hertzforge_sdm::law::MimoControlLaw + Send>,
    fault: Fault,
    after: u64,
    calls: u64,
}

impl hertzforge_sdm::law::MimoControlLaw for FaultyFloorLaw {
    fn needs(&self) -> &[hertzforge_sdm::law::MimoInput] {
        self.inner.needs()
    }
    fn shapes(&self) -> &[hertzforge_sdm::law::MimoShape] {
        self.inner.shapes()
    }
    fn seed(
        &mut self,
        x: &hertzforge_sdm::law::MimoLoopInputs<'_>,
    ) -> Result<hertzforge_sdm::law::MimoLoopOutputs, hertzforge_sdm::law::LawError> {
        self.inner.seed(x)
    }
    fn iterate(
        &mut self,
        x: &hertzforge_sdm::law::MimoLoopInputs<'_>,
    ) -> Result<hertzforge_sdm::law::MimoLoopOutputs, hertzforge_sdm::law::LawError> {
        self.calls += 1;
        if self.calls > self.after {
            match self.fault {
                Fault::Fail | Fault::Fade => {
                    return Err(hertzforge_sdm::law::LawError("injected".into()))
                }
                Fault::Trap => panic!("injected floor law trap"),
            }
        }
        self.inner.iterate(x)
    }
}

impl hertzforge_random_laws::MimoLawFactory for FaultyFloor {
    fn identity(&self) -> hertzforge_law_host::LawIdentity {
        self.inner.identity()
    }
    fn params(&self) -> &[hertzforge_law_host::Param] {
        self.inner.params()
    }
    fn build(
        &self,
        setup: &hertzforge_sdm::law::MimoSetup,
        budget: std::time::Duration,
    ) -> Result<
        Box<dyn hertzforge_sdm::law::MimoControlLaw + Send>,
        hertzforge_random_laws::ChoiceError,
    > {
        Ok(Box::new(FaultyFloorLaw {
            inner: self.inner.build(setup, budget)?,
            fault: self.fault,
            after: self.after,
            calls: 0,
        }))
    }
}

/// A tone law that answers as the built-in until its `after`th tick, then
/// fails or panics on every call.
struct FaultyTone {
    inner: Arc<dyn hertzforge_mimo_sine::law::MimoSineLawFactory>,
    fault: Fault,
    after: u64,
}

struct FaultyToneLaw {
    inner: Box<dyn hertzforge_mimo_sine::law::MimoSineLaw + Send>,
    fault: Fault,
    after: u64,
    calls: u64,
}

impl hertzforge_mimo_sine::law::MimoSineLaw for FaultyToneLaw {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn engage(
        &mut self,
        x: &hertzforge_mimo_sine::tick::TickInputs<'_>,
        o: &mut hertzforge_mimo_sine::tick::TickOutputs,
        v: &mut hertzforge_mimo_sine::law::LawValues,
    ) -> Result<(), hertzforge_mimo_sine::tick::LawFault> {
        self.inner.engage(x, o, v)
    }
    fn tick(
        &mut self,
        x: &hertzforge_mimo_sine::tick::TickInputs<'_>,
        o: &mut hertzforge_mimo_sine::tick::TickOutputs,
        v: &mut hertzforge_mimo_sine::law::LawValues,
    ) -> Result<(), hertzforge_mimo_sine::tick::LawFault> {
        self.calls += 1;
        if self.calls > self.after {
            match self.fault {
                Fault::Fail => {
                    return Err(hertzforge_mimo_sine::tick::LawFault::Failed(
                        "injected".into(),
                    ))
                }
                Fault::Trap => panic!("injected tone law trap"),
                Fault::Fade => {
                    let k = 10f64.powf(-0.5 / 20.0);
                    for (d, f) in o.drive.iter_mut().zip(x.drive_in_force) {
                        *d = *f * k;
                    }
                    return Ok(());
                }
            }
        }
        self.inner.tick(x, o, v)
    }
}

impl hertzforge_mimo_sine::law::MimoSineLawFactory for FaultyTone {
    fn build(
        &self,
        setup: &hertzforge_mimo_sine::law::LawSetup,
    ) -> Result<
        Box<dyn hertzforge_mimo_sine::law::MimoSineLaw + Send>,
        hertzforge_mimo_sine::tick::LawFault,
    > {
        Ok(Box::new(FaultyToneLaw {
            inner: self.inner.build(setup)?,
            fault: self.fault,
            after: self.after,
            calls: 0,
        }))
    }
    fn describe(&self) -> hertzforge_mimo_sine::law::LawDescriptor {
        self.inner.describe()
    }
    fn manifest(&self) -> hertzforge_mimo_sine::law::LawManifest {
        self.inner.manifest()
    }
}

/// A clock that runs true until its `after`th reading, then jumps a second
/// on every reading: every call measured after that is a second long.
struct LateClock {
    reads: AtomicU64,
    after: u64,
    origin: std::time::Instant,
}

impl LateClock {
    fn new(after: u64) -> Arc<Self> {
        Arc::new(LateClock {
            reads: AtomicU64::new(0),
            after,
            origin: std::time::Instant::now(),
        })
    }
    fn now(&self) -> f64 {
        let k = self.reads.fetch_add(1, Ordering::Relaxed);
        self.origin.elapsed().as_secs_f64() + k.saturating_sub(self.after) as f64
    }
}

impl hertzforge_random::service::law::LawClock for LateClock {
    fn now_s(&self) -> f64 {
        self.now()
    }
}

impl hertzforge_sine::law::LawClock for LateClock {
    fn now_s(&self) -> f64 {
        self.now()
    }
}

#[derive(Clone, Copy, Debug)]
enum Role {
    Floor,
    Tones,
}

/// A two-tone run whose `role` law fails, traps or (`None`) answers late;
/// the end and whether the drive is at zero.
fn faulty_run(role: Role, fault: Option<Fault>) -> (RunEnd, bool) {
    let s = SineStructure::Linear4x4;
    let cfg = two_tone_config(30.0);
    let mut laws = MimoMixedLaws::builtin(&cfg).expect("laws");
    if let Some(f) = fault {
        match role {
            Role::Floor => {
                laws.floor = Arc::new(FaultyFloor {
                    inner: laws.floor.clone(),
                    fault: f,
                    after: 4,
                });
            }
            Role::Tones => {
                laws.tones = Arc::new(FaultyTone {
                    inner: laws.tones.clone(),
                    fault: f,
                    after: 50,
                });
            }
        }
    }
    let mut svc = MimoMixedService::new(
        cfg,
        table(s),
        backend(s, 7),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        laws,
    )
    .expect("service");
    if fault.is_none() {
        // Late: the role's clock jumps a second a reading once the run is in.
        let real = LateClock::new(u64::MAX);
        svc = match role {
            Role::Floor => svc.with_law_clocks(LateClock::new(40), real),
            Role::Tones => svc.with_law_clocks(real, LateClock::new(400)),
        };
    }
    engage(&mut svc);
    run_out(&mut svc, 80.0);
    let e = ended(&svc);
    let zero = svc.drive_is_zero();
    (e, zero)
}

/// Requirement: in the floor role and in the tone role, a law that fails,
/// traps (panics) or answers late, once the run is in, ends the run
/// `law_fault` naming the role's part (the floor, or a tone), with the drive
/// at zero.
#[test]
fn mimo_mixed_law_faults_in_either_role_give_the_controlled_stop() {
    for role in [Role::Floor, Role::Tones] {
        for fault in [Some(Fault::Fail), Some(Fault::Trap), None] {
            let (e, zero) = faulty_run(role, fault);
            let what = format!(
                "{role:?}/{}",
                fault.map_or("late".into(), |f| format!("{f:?}"))
            );
            println!("{what}: {} {:?}: {}", e.name, e.part, e.words);
            assert_eq!(e.name, "law_fault", "{what}: {}", e.words);
            assert!(e.aborted, "{what}");
            match role {
                Role::Floor => assert_eq!(e.part.as_deref(), Some("floor"), "{what}"),
                Role::Tones => assert!(
                    matches!(e.part.as_deref(), Some("t1") | Some("t2")),
                    "{what}: {:?}",
                    e.part
                ),
            }
            assert!(zero, "{what}: drive left playing");
        }
    }
}

// =============================================================================
// The real-time start.
// =============================================================================

/// The simulator, stating a 512-frame output lookahead, taking only 100
/// frames of drive before it is started; it counts its starts.
struct Stingy {
    inner: Box<dyn hertzforge_hal::Backend>,
    m: usize,
    taken: usize,
    started: Arc<AtomicU64>,
}

impl hertzforge_hal::Backend for Stingy {
    fn caps(&self) -> hertzforge_hal::Caps {
        let mut c = self.inner.caps();
        c.output_buffer = Some(hertzforge_hal::OutputBufferContract {
            min_lookahead_frames: 512,
            underrun_policy: hertzforge_hal::UnderrunPolicy::RampDownThenAbort,
        });
        c
    }
    fn open(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.close()
    }
    fn configure(
        &mut self,
        table: &hertzforge_channel_table::ChannelTable,
        acq: &hertzforge_hal::AcquisitionConfig,
    ) -> Result<hertzforge_hal::ConfiguredActual, hertzforge_hal::HalError> {
        self.inner.configure(table, acq)
    }
    fn arm(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.started.fetch_add(1, Ordering::Relaxed);
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.stop()
    }
    fn read_input(
        &mut self,
        out: &mut [f32],
    ) -> Result<hertzforge_hal::InputRead, hertzforge_hal::HalError> {
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, hertzforge_hal::HalError> {
        if self.started.load(Ordering::Relaxed) == 0 {
            let frames = (drive.len() / self.m).min(100usize.saturating_sub(self.taken));
            self.taken += frames;
            if frames == 0 {
                return Ok(0);
            }
            return self.inner.write_drive(&drive[..frames * self.m]);
        }
        self.inner.write_drive(drive)
    }
    fn poll_status(&mut self) -> hertzforge_hal::BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.emergency_stop()
    }
}

/// Requirement: on a real-time station that states a 512-frame lookahead
/// and takes only 100 frames before its start, the identification is
/// refused with the station's count named, the station is never started,
/// and nothing is driven.
#[test]
fn mimo_mixed_real_time_start_is_refused_short_of_the_lookahead() {
    let s = SineStructure::Linear4x4;
    let cfg = two_tone_config(30.0);
    let laws = MimoMixedLaws::builtin(&cfg).expect("laws");
    let started = Arc::new(AtomicU64::new(0));
    let b = Stingy {
        inner: backend(s, 7),
        m: cfg.drives.len(),
        taken: 0,
        started: started.clone(),
    };
    let mut svc = MimoMixedService::new(
        cfg,
        table(s),
        Box::new(b),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        laws,
    )
    .expect("service")
    .with_real_time(true);
    svc.arm().expect("arm");
    let r = svc.identify().map(|_| ());
    println!("identify: {r:?}");
    match r {
        Err(MimoMixedError::Plant(w)) => assert!(w.contains("the station took 100 of"), "{w}"),
        other => panic!("not refused: {other:?}"),
    }
    assert_eq!(
        started.load(Ordering::Relaxed),
        0,
        "the station was started"
    );
    assert!(svc.drive_is_zero());
    assert_eq!(svc.end().map(|e| e.name), Some("start_refused"));
}

/// The simulator, stating a 512-frame output lookahead, that from its
/// second start on takes only 100 frames of the engine's zero prefill (so
/// the engaged start is counted short of the identification's and the run
/// errs on its first batch, the station started); it counts its starts and
/// its emergency stops.
struct Shifty {
    inner: Box<dyn hertzforge_hal::Backend>,
    m: usize,
    writes_since_start: usize,
    started: Arc<AtomicU64>,
    stopped: Arc<AtomicU64>,
}

impl hertzforge_hal::Backend for Shifty {
    fn caps(&self) -> hertzforge_hal::Caps {
        let mut c = self.inner.caps();
        c.output_buffer = Some(hertzforge_hal::OutputBufferContract {
            min_lookahead_frames: 512,
            underrun_policy: hertzforge_hal::UnderrunPolicy::RampDownThenAbort,
        });
        c
    }
    fn open(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.close()
    }
    fn configure(
        &mut self,
        table: &hertzforge_channel_table::ChannelTable,
        acq: &hertzforge_hal::AcquisitionConfig,
    ) -> Result<hertzforge_hal::ConfiguredActual, hertzforge_hal::HalError> {
        self.inner.configure(table, acq)
    }
    fn arm(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.started.fetch_add(1, Ordering::Relaxed);
        self.writes_since_start = 0;
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.stop()
    }
    fn read_input(
        &mut self,
        out: &mut [f32],
    ) -> Result<hertzforge_hal::InputRead, hertzforge_hal::HalError> {
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, hertzforge_hal::HalError> {
        self.writes_since_start += 1;
        if self.started.load(Ordering::Relaxed) >= 2 {
            match self.writes_since_start {
                1 => {
                    let frames = (drive.len() / self.m).min(100);
                    return self.inner.write_drive(&drive[..frames * self.m]);
                }
                2 => return Ok(0),
                _ => {}
            }
        }
        self.inner.write_drive(drive)
    }
    fn poll_status(&mut self) -> hertzforge_hal::BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.stopped.fetch_add(1, Ordering::Relaxed);
        self.inner.emergency_stop()
    }
}

/// Requirement: an error inside the run's loop once the station is
/// started (here the engaged start counted short of the identification's
/// frames ahead) stops the station at once and ends the run `run_error`,
/// naming the error, with the drive at zero; the error is still returned.
#[test]
fn mimo_mixed_an_error_mid_run_stops_the_station_and_names_the_end() {
    let s = SineStructure::Linear4x4;
    let cfg = two_tone_config(30.0);
    let laws = MimoMixedLaws::builtin(&cfg).expect("laws");
    let started = Arc::new(AtomicU64::new(0));
    let stopped = Arc::new(AtomicU64::new(0));
    let b = Shifty {
        inner: backend(s, 7),
        m: cfg.drives.len(),
        writes_since_start: 0,
        started: started.clone(),
        stopped: stopped.clone(),
    };
    let mut svc = MimoMixedService::new(
        cfg,
        table(s),
        Box::new(b),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        laws,
    )
    .expect("service");
    engage(&mut svc);
    let r = svc.run_chunk();
    println!(
        "run_chunk: {r:?}; starts {}, emergency stops {}",
        started.load(Ordering::Relaxed),
        stopped.load(Ordering::Relaxed)
    );
    match r {
        Err(MimoMixedError::Plant(w)) => assert!(w.contains("frames ahead"), "{w}"),
        other => panic!("the run did not err: {other:?}"),
    }
    let e = svc
        .end()
        .cloned()
        .expect("the run left running after its error");
    println!("end: {}: {}", e.name, e.words);
    assert_eq!(e.name, "run_error");
    assert!(e.words.contains("frames ahead"), "{}", e.words);
    assert_aborted_to_zero(&svc, &e);
    assert!(
        stopped.load(Ordering::Relaxed) >= 1,
        "the station was not stopped"
    );
    assert!(!svc.run_chunk().expect("an ended run"), "the run went on");
}

/// The simulator, every drive frame written to it kept.
struct Taping {
    inner: Box<dyn hertzforge_hal::Backend>,
    m: usize,
    tape: Arc<std::sync::Mutex<Vec<f32>>>,
}

impl hertzforge_hal::Backend for Taping {
    fn caps(&self) -> hertzforge_hal::Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.close()
    }
    fn configure(
        &mut self,
        table: &hertzforge_channel_table::ChannelTable,
        acq: &hertzforge_hal::AcquisitionConfig,
    ) -> Result<hertzforge_hal::ConfiguredActual, hertzforge_hal::HalError> {
        self.inner.configure(table, acq)
    }
    fn arm(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.stop()
    }
    fn read_input(
        &mut self,
        out: &mut [f32],
    ) -> Result<hertzforge_hal::InputRead, hertzforge_hal::HalError> {
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, hertzforge_hal::HalError> {
        let k = self.inner.write_drive(drive)?;
        let m = self.m;
        self.tape
            .lock()
            .expect("tape")
            .extend_from_slice(&drive[..(k * m).min(drive.len())]);
        Ok(k)
    }
    fn poll_status(&mut self) -> hertzforge_hal::BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), hertzforge_hal::HalError> {
        self.inner.emergency_stop()
    }
}

/// Requirement: a level step of the identification's search never splices
/// the drive: the drive in force ramps out to zero, then the new level
/// ramps in over the random part's ramp. Started 20 dB short, so the first
/// step is the search's largest (ten times the drive). Each drive's rms
/// over 256-frame windows, normalized to its level in the record and
/// averaged over the four independent drives: a dip is a window under a
/// quarter of the level half a second (twice the ramp) before it. Every
/// step and the closing park show one; after each step's dip the drive
/// takes at least three quarters of the ramp to reach 90 % of its new
/// level (a linear ramp takes 90 % of it, less a window; a new level put
/// in at once reaches it within the synthesis's crossfade, 2048 frames or
/// fewer here, half the ramp).
#[test]
fn mimo_mixed_the_identification_level_steps_ramp_out_and_in() {
    let s = SineStructure::Linear4x4;
    let mut cfg = two_tone_config(30.0);
    let mut idc = cfg.identification();
    idc.start_vrms /= 10.0;
    cfg.identification = Some(idc);
    let m = cfg.drives.len();
    let laws = MimoMixedLaws::builtin(&cfg).expect("laws");
    let tape = Arc::new(std::sync::Mutex::new(Vec::new()));
    let b = Taping {
        inner: backend(s, 7),
        m,
        tape: tape.clone(),
    };
    let mut svc = MimoMixedService::new(
        cfg,
        table(s),
        Box::new(b),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        laws,
    )
    .expect("service");
    svc.arm().expect("arm");
    let id = svc.identify().expect("identify");
    let steps = id.steps;
    println!("identified at {:.2} dB in {steps} steps", id.level_db);
    assert!(steps >= 2, "no level step to judge");
    let fs = svc.sample_rate_hz();
    let tape = tape.lock().expect("tape").clone();
    let (w, out) = (256, tape.len() / m);
    let windows = out / w;
    let mut mean = vec![0.0f64; windows];
    for j in 0..m {
        let rms: Vec<f64> = (0..windows)
            .map(|k| {
                let ss: f64 = (k * w..(k + 1) * w)
                    .map(|f| (tape[f * m + j] as f64).powi(2))
                    .sum();
                (ss / w as f64).sqrt()
            })
            .collect();
        // The record's level: the median over the windows before the
        // park (the last third of the tape is the record).
        let mut rec: Vec<f64> = rms[windows / 2..windows * 9 / 10].to_vec();
        rec.sort_by(f64::total_cmp);
        let full = rec[rec.len() / 2];
        assert!(full > 0.0, "drive {j} never played");
        for (a, r) in mean.iter_mut().zip(&rms) {
            *a += r / full / m as f64;
        }
    }
    let ramp = RANDOM_RAMP_S * fs;
    let back = (2.0 * ramp) as usize / w;
    let mut dips: Vec<usize> = Vec::new();
    let mut k = back;
    while k < windows {
        if mean[k] < 0.25 * mean[k - back] {
            let mut bottom = k;
            while k < windows && mean[k] < 0.25 * mean[k - back] {
                if mean[k] < mean[bottom] {
                    bottom = k;
                }
                k += 1;
            }
            dips.push(bottom);
        }
        k += 1;
    }
    println!(
        "{} dips over {windows} windows of {w} frames, at frames {:?}",
        dips.len(),
        dips.iter().map(|d| d * w).collect::<Vec<_>>()
    );
    assert_eq!(
        dips.len(),
        steps,
        "a level step played without ramping the drive out (one dip per step and the park)"
    );
    for &d in &dips[..steps - 1] {
        let after = (2.0 * ramp) as usize / w;
        let mut lvl: Vec<f64> = mean[d + after..(d + 2 * after).min(windows)].to_vec();
        lvl.sort_by(f64::total_cmp);
        let new = lvl[lvl.len() / 2];
        let reach = (d..windows)
            .find(|&k| mean[k] >= 0.9 * new)
            .expect("the new level reached");
        let frames = (reach - d) * w;
        println!(
            "the step at frame {}: from {:.3} to {new:.3} of full level in {frames} frames",
            d * w,
            mean[d]
        );
        assert!(
            frames as f64 >= 0.75 * ramp,
            "the new level came in over {frames} frames, under 3/4 of the {ramp} frame ramp"
        );
    }
}

// =============================================================================
// Each drive to its own shaker.
// =============================================================================

/// The identification of a two-tone run on Linear4x4 whose drives are the
/// table's drive rows `rows` (channel indices), in that order.
fn identified_h(rows: &[u32]) -> hertzforge_sdm::MatrixStack {
    let s = SineStructure::Linear4x4;
    let mut cfg = two_tone_config(30.0);
    let proto = cfg.drives[0].clone();
    cfg.drives = rows
        .iter()
        .map(|&r| {
            let mut d = proto.clone();
            d.channel_index = r;
            d
        })
        .collect();
    let mut svc = service(s, cfg);
    svc.arm().expect("arm");
    let id = svc.identify().expect("identify");
    println!(
        "drives {rows:?}: identified at {:.2} dB in {} steps",
        id.level_db, id.steps
    );
    id.h.clone()
}

/// The median over the band's lines of `|a[:, ja] - b[:, jb]| / |b[:, jb]|`
/// on the controls.
fn column_mismatch(
    a: &hertzforge_sdm::MatrixStack,
    ja: usize,
    b: &hertzforge_sdm::MatrixStack,
    jb: usize,
) -> f64 {
    let (lo, hi) = BAND;
    let mut r: Vec<f64> = (lo..hi)
        .map(|l| {
            let (mut num, mut den) = (0.0, 0.0);
            for i in 0..4 {
                num += (a.get(l, i, ja) - b.get(l, i, jb)).norm_sqr();
                den += b.get(l, i, jb).norm_sqr();
            }
            (num / den).sqrt()
        })
        .collect();
    r.sort_by(f64::total_cmp);
    r[r.len() / 2]
}

/// Requirement: drive `j` of the test reaches the shaker on its own table
/// row whatever order the test lists its drives in and whether it uses
/// every drive row. On Linear4x4 (drive rows 5 to 8), the identification of
/// a test listing its drives as [8, 7, 6, 5], and of one using only [7, 5],
/// gives in each column the response of that column's own row in the
/// identification with [5, 6, 7, 8] and of no other row. The bands are from
/// the scatter: two identifications of the same row on independent random
/// streams differ by a median 0.35 over the band, and the four rows' own
/// responses differ from each other by 1.2 or more; so a column is its own
/// row's under 0.5 and every other row's over 1.0. Before drives were mapped
/// to their rows the reversed test's column 0 matched row 5's response, not
/// row 8's.
#[test]
fn mimo_mixed_each_drive_reaches_its_own_shaker_in_any_order_or_subset() {
    let base = identified_h(&[5, 6, 7, 8]);
    let rev = identified_h(&[8, 7, 6, 5]);
    let sub = identified_h(&[7, 5]);
    let pairs: [(&str, &hertzforge_sdm::MatrixStack, usize, usize); 6] = [
        ("reversed col 0 (row 8)", &rev, 0, 3),
        ("reversed col 1 (row 7)", &rev, 1, 2),
        ("reversed col 2 (row 6)", &rev, 2, 1),
        ("reversed col 3 (row 5)", &rev, 3, 0),
        ("subset col 0 (row 7)", &sub, 0, 2),
        ("subset col 1 (row 5)", &sub, 1, 0),
    ];
    for (what, h, j, row) in pairs {
        let v: Vec<f64> = (0..4).map(|r| column_mismatch(h, j, &base, r)).collect();
        println!("{what}: mismatch against the base's columns {v:.3?}");
        assert!(
            v[row] < 0.5,
            "{what}: {:.3} from its own row's response",
            v[row]
        );
        for (r, x) in v.iter().enumerate().filter(|&(r, _)| r != row) {
            assert!(*x > 1.0, "{what}: {x:.3} from row {r}'s response");
        }
    }
}

/// Requirement: a sweeping tone under the dwell-only tone law `paired` is
/// refused `law_refuses_tone`, naming the tone, before anything is opened.
#[test]
fn mimo_mixed_a_tone_the_law_refuses_is_refused_before_the_station_opens() {
    let s = SineStructure::Linear4x4;
    let sweep = hertzforge_mixed::PartSchedule::Sweep {
        lo_hz: 150.0,
        hi_hz: 500.0,
        start_hz: 150.0,
        direction: hertzforge_mixed::Direction::Up,
        law: hertzforge_mixed::SweepLaw::LinearHzPerS,
        rate: 4.0,
        target: hertzforge_mixed::PartSweepTarget::Sweeps { n: 1 },
    };
    let mut cfg = run_config(s, 0.5, vec![tone("sw", sweep, 1.0)], 30.0);
    cfg.law.tones = hertzforge_mixed::LawPick::new("paired");
    let r = MimoMixedLaws::builtin(&cfg).and_then(|laws| {
        MimoMixedService::new(
            cfg,
            table(s),
            backend(s, 7),
            hertzforge_module_abi::StationClass::HertzforgeSim,
            laws,
        )
        .map(|_| ())
    });
    match r {
        Err(e) => {
            println!("refused: {} ({e})", e.name());
            assert_eq!(e.name(), "law_refuses_tone");
            assert!(e.to_string().contains("sw"), "{e}");
        }
        Ok(()) => panic!("not refused"),
    }
}

/// Requirement: three 2 g tones on drives whose peak ceiling is 0.3 V are
/// refused `budget_cannot_fit` at proceed, naming a drive, with nothing
/// driven after the identification.
#[test]
fn mimo_mixed_a_budget_that_cannot_fit_is_refused_at_proceed() {
    let s = SineStructure::Linear4x4;
    let parts = vec![
        tone("t1", fixed(150.0), 2.0),
        tone("t2", fixed(240.0), 2.0),
        tone("t3", fixed(700.0), 2.0),
    ];
    let mut cfg = run_config(s, 0.5, parts, 30.0);
    for d in cfg.drives.iter_mut() {
        d.peak_v = 0.3;
    }
    let mut svc = service(s, cfg);
    svc.arm().expect("arm");
    svc.identify().expect("identify");
    assert!(svc.coherence_gate().expect("gate"));
    match svc.proceed() {
        Err(e) => {
            println!("refused: {} ({e})", e.name());
            assert_eq!(e.name(), "budget_cannot_fit");
            assert!(e.to_string().contains("drive"), "{e}");
        }
        Ok(()) => panic!("not refused"),
    }
    assert!(svc.drive_is_zero());
    assert!(!svc.run_chunk().unwrap_or(false), "a refused run went on");
}
