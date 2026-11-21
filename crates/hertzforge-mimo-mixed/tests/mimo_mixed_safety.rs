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
