//! The classical closed loop on the simulator: identification in volts, the
//! review hold, the built-in law's seed and corrections, the ladder advancing
//! on a passing pulse, and the drive in volts.
//!
//! The plants are the law check's: a wire, one mode at 400 Hz (Q 10), the
//! simulated station's two modes (200 and 550 Hz, Q 2.5) under a terminal-peak
//! sawtooth of 6 ms, one mode behind a 2 ms delay, and a wire whose identified
//! gain the core scales +3 dB before inverting it. Every run is AUTO
//! `[-12, -9, -6, -3, 0]` dB with `[1, 1, 1, 1, 3]` pulses, advancing on a
//! pass, at most 5 pulses a rung, MIL-STD-810 Method 516.

use hertzforge_channel_table::ChannelTable;
use hertzforge_dsp::shock::{CompensationConfig, CompensationShape};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_shock_common::{LadderAdvance, ShockLevelSchedule};
use hertzforge_shock_host::service::pacer::VirtualPacer;
use hertzforge_shock_host::{
    BackendPulseFirer, ClassicalControl, ClassicalPulseSpec, ClassicalShape, ClassicalShockService,
    ClassicalTestConfig, HeldReason, Polarity, RunState,
};

const FS: f64 = 5120.0;

type Svc = ClassicalShockService<BackendPulseFirer<SimBackend>>;

fn spec(shape: ClassicalShape, ms: f64) -> ClassicalPulseSpec {
    let mut spec = ClassicalPulseSpec::new(shape, 20.0, ms);
    spec.compensation = CompensationConfig {
        shape: CompensationShape::Rectangular,
        pre_frac: 4.0,
        post_frac: 4.0,
        symmetric: true,
    };
    spec
}

fn config(spec: ClassicalPulseSpec) -> ClassicalTestConfig {
    let mut cfg = ClassicalTestConfig::new(spec);
    cfg.fs_hz = FS;
    // The plants are about 1 g per volt: the 20 g pulse needs about 20 V.
    let mut control = ClassicalControl::new(FS, 30.0, 40.0);
    control.sysid.duration_s = 2.0;
    cfg.control = Some(control);
    cfg.schedule = ShockLevelSchedule {
        pulses_per_level: vec![1, 1, 1, 1, 3],
        advance: LadderAdvance::OnPass,
        ..ShockLevelSchedule::auto_default()
    };
    cfg
}

fn service(cfg: ClassicalTestConfig, plant: SimConfig, tag: &str) -> Svc {
    #[allow(deprecated)]
    let table = ChannelTable::nonuniform_default_8ch();
    let backend = plant
        .sample_rate_hz(FS)
        .noise_level(0.0)
        .shared_clock(true)
        .build();
    let firer = BackendPulseFirer::new(
        backend,
        table.clone(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap();
    let mut s = ClassicalShockService::new(
        cfg,
        table,
        firer,
        Some(std::env::temp_dir().join(format!(
            "hertzforge-shock-closed-{tag}-{}",
            std::process::id()
        ))),
    )
    .unwrap();
    s.set_pacer(Box::new(VirtualPacer::new(0.010)));
    s
}

/// Arm, identify, review, proceed, run the schedule.
fn run(mut s: Svc) -> Svc {
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    assert_eq!(s.identify().unwrap(), RunState::AwaitingReview);
    assert_eq!(s.proceed().unwrap(), RunState::Firing);
    s.run_auto(100).unwrap();
    s
}

/// The first passing pulse (from 1) and its level, and the full-level verdicts.
fn summary(s: &Svc) -> (Option<(usize, f64)>, Vec<bool>) {
    let rows = s.report_data().pulses;
    let first = rows
        .iter()
        .position(|r| r.tolerance_overall == Some(true))
        .map(|i| (i + 1, rows[i].scheduled_level_db));
    let full = rows
        .iter()
        .filter(|r| r.scheduled_level_db == 0.0)
        .map(|r| r.tolerance_overall == Some(true))
        .collect();
    (first, full)
}

fn assert_converges(s: &Svc, by_pulse: usize, what: &str) {
    assert_eq!(s.state(), RunState::Stopped, "{what}: {:?}", s.trip());
    let (first, full) = summary(s);
    let (n, level) = first.unwrap_or_else(|| panic!("{what}: no pulse passed"));
    assert!(
        n <= by_pulse && level == -12.0,
        "{what}: first pass {n} at {level}"
    );
    assert_eq!(full, vec![true; 3], "{what}: every full-level pulse passes");
    let law = s.report_data().law.unwrap();
    assert_eq!((law.misses, law.refused), (0, 0), "{what}");
}

#[test]
fn on_a_wire_every_pulse_passes_the_first_included() {
    let s = run(service(
        config(spec(ClassicalShape::HalfSine, 11.0)),
        SimConfig::new().resonances(Vec::new()).feedthrough(1.0),
        "wire",
    ));
    assert_eq!(s.state(), RunState::Stopped);
    let rows = s.report_data().pulses;
    assert_eq!(rows.len(), 7);
    assert!(rows.iter().all(|r| r.tolerance_overall == Some(true)));
    // Full level on a 1 g per volt wire is 20 V: the drive is in volts.
    let last = rows.last().unwrap();
    assert!((last.drive_vpk.unwrap() - 20.0).abs() < 0.2, "{last:?}");
    assert!(last.error_pct.unwrap() < 5.0, "{last:?}");
}

#[test]
fn one_mode_passes_by_the_third_pulse() {
    let s = run(service(
        config(spec(ClassicalShape::HalfSine, 11.0)),
        SimConfig::new().resonance(400.0, 10.0).feedthrough(1.0),
        "one-mode",
    ));
    assert_converges(&s, 3, "one mode");
}

#[test]
fn the_station_plant_passes_a_haversine_by_the_third_pulse() {
    let s = run(service(
        config(spec(ClassicalShape::Haversine, 6.0)),
        SimConfig::new()
            .resonances(vec![(200.0, 2.5), (550.0, 2.5)])
            .feedthrough(1.0),
        "station",
    ));
    assert_converges(&s, 3, "station plant");
}

#[test]
fn a_transport_delay_passes_by_the_third_pulse() {
    let s = run(service(
        config(spec(ClassicalShape::HalfSine, 11.0)),
        SimConfig::new()
            .resonance(400.0, 10.0)
            .feedthrough(1.0)
            .transport_delay_samples((0.002 * FS).round() as usize),
        "delay",
    ));
    assert_converges(&s, 3, "delay");
}

#[test]
fn an_identification_error_is_corrected_within_the_first_rung() {
    // The case gives its first rung eight pulses: at the default beta of 0.5
    // a 3 dB error leaves 0.65 of the error after each pulse, and a pulse
    // passes once the error is under about 4% (the compensated pulse's own
    // velocity change sits 6% under the nominal, inside the 10% tolerance).
    let mut cfg = config(spec(ClassicalShape::HalfSine, 11.0));
    cfg.schedule.max_pulses_per_level = 8;
    let s = run(service(
        cfg,
        SimConfig::new().resonances(Vec::new()).feedthrough(1.0),
        "id-error",
    )
    .with_identification_scale_db(3.0));
    let rows = s.report_data().pulses;
    // The core believes the plant 3 dB hotter: the seed plays 29% low.
    assert!((rows[0].peak_ratio.unwrap() - 10f64.powf(-3.0 / 20.0)).abs() < 0.01);
    assert_eq!(rows[0].tolerance_overall, Some(false));
    let (first, _) = summary(&s);
    let (n, level) = first.expect("the law corrects it");
    assert!(n <= 8 && level == -12.0, "first pass {n} at {level}");
    assert!(n > 1, "the first pulse is the uncorrected one");
    assert_eq!(s.state(), RunState::Stopped, "{:?}", s.trip());
}

#[test]
fn a_negative_pulse_is_driven_negative_and_judged_sign_corrected() {
    let mut cfg = config(spec(ClassicalShape::HalfSine, 11.0));
    cfg.control.as_mut().unwrap().polarity = Polarity::Negative;
    let s = run(service(
        cfg,
        SimConfig::new().resonance(400.0, 10.0).feedthrough(1.0),
        "negative",
    ));
    assert_converges(&s, 3, "negative");
    let v = s.pulse_view(0).unwrap();
    let (lo, hi) = v
        .measured
        .iter()
        .fold((0.0f64, 0.0f64), |(a, b), &x| (a.min(x), b.max(x)));
    assert!(-lo > 2.0 * hi, "the measured pulse points down: {lo} {hi}");
}

#[test]
fn identification_holds_when_coherence_is_under_the_floor() {
    // Noise far over the identification drive.
    #[allow(deprecated)]
    let table = ChannelTable::nonuniform_default_8ch();
    let noisy = SimConfig::new()
        .resonances(Vec::new())
        .feedthrough(1.0)
        .sample_rate_hz(FS)
        .noise_level(0.1)
        .shared_clock(true)
        .build();
    let firer = BackendPulseFirer::new(
        noisy,
        table.clone(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap();
    let mut cfg = config(spec(ClassicalShape::HalfSine, 11.0));
    cfg.control.as_mut().unwrap().sysid.drive_rms = 1e-6;
    let mut s = ClassicalShockService::new(
        cfg,
        table,
        firer,
        Some(std::env::temp_dir().join("hertzforge-shock-closed-held")),
    )
    .unwrap();
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    assert_eq!(s.identify().unwrap(), RunState::Held);
    match s.status().held {
        Some(HeldReason::LowCoherence { mean, floor }) => assert!(mean < floor),
        h => panic!("{h:?}"),
    }
    assert!(s.report_data().pulses.is_empty());
    assert!(s.proceed().is_err(), "a held run does not proceed");
}

#[test]
fn identification_holds_when_the_trusted_bins_miss_the_reference() {
    // A correction band of 1-2 kHz leaves the 11 ms half-sine's energy out.
    let mut cfg = config(spec(ClassicalShape::HalfSine, 11.0));
    let c = cfg.control.as_mut().unwrap();
    c.band_lo_hz = 1000.0;
    c.band_hi_hz = 2000.0;
    c.sysid.f_lo_hz = 1000.0;
    c.sysid.f_hi_hz = 2000.0;
    let mut s = service(
        cfg,
        SimConfig::new().resonances(Vec::new()).feedthrough(1.0),
        "out-of-band",
    );
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    assert_eq!(s.identify().unwrap(), RunState::Held);
    assert!(matches!(
        s.status().held,
        Some(HeldReason::ReferenceOutOfBand { coverage, min }) if coverage < min
    ));
}
