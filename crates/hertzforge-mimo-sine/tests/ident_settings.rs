//! The identification's line spacing and its default record count: minutes,
//! not nine, with the trust mask and the sweep-rate warning still right on
//! the linear 4 by 4 (its lightest mode, 40 Hz at Q 50, is 0.8 Hz wide).

mod common;

use common::*;
use hertzforge_hal_sim::plants::SineStructure;
use hertzforge_mimo_sine::service::{EngineOptions, MimoSineService};
use hertzforge_mimo_sine::*;
use hertzforge_module_abi::StationClass;
use hertzforge_sine::scheduler::{BidirSweepConfig, SweepDirection, SweepTarget};
use hertzforge_sine::SweepRate;

#[test]
fn mimo_sine_the_default_identification_takes_minutes_and_still_resolves_the_lightest_mode() {
    let s = SineStructure::Linear4x4;
    let mut st = setup(
        s,
        TestPlan::Sweep(BidirSweepConfig {
            band: (30.0, 300.0),
            rate: SweepRate::LogOctPerMin(1.0),
            start_hz: 30.0,
            initial_direction: SweepDirection::Up,
            target: SweepTarget::Sweeps(1),
        }),
        "matrix",
    );
    st.identification = IdentificationSettings::default();
    assert_eq!(st.identification.resolution_hz, 0.25);
    assert_eq!(st.identification.frames, None);
    let mut svc = MimoSineService::new(
        st,
        table(s),
        backend(s, NOISE, 7),
        law("matrix"),
        // On the simulator the law's time budget is relaxed, as the other simulator
        // harnesses relax theirs, so a call a busy machine made late is not a miss here.
        EngineOptions {
            law_budget: Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET),
            ..EngineOptions::new(StationClass::HertzforgeSim)
        },
    )
    .unwrap();
    svc.arm().unwrap();
    let rep = svc.identify().unwrap().clone();
    let id = &rep.identification;
    let inside: Vec<usize> = (0..id.frequency_hz.len())
        .filter(|&k| id.frequency_hz[k] >= 30.0 && id.frequency_hz[k] <= 300.0)
        .collect();
    let trusted = inside.iter().filter(|&&k| id.trusted[k]).count() as f64 / inside.len() as f64;
    let at40 = rep
        .sweep_rate_warning
        .iter()
        .find(|r| (r.f_hz - 40.0).abs() < 1.0)
        .copied();
    println!(
        "{} records of {} samples ({} Hz lines): {:.0} s; trusted {:.3} of the band; 40 Hz mode {:?}; modes {}",
        rep.realizations, rep.record_len, rep.line_spacing_hz, rep.duration_s, trusted, at40, rep.modes_found
    );
    assert_eq!(rep.realizations, 24);
    assert_eq!(rep.line_spacing_hz, 0.25);
    assert!((rep.duration_s - 200.0).abs() < 1.0, "{}", rep.duration_s);
    assert!(trusted > 0.99, "{trusted}");
    let m = at40.expect("the 40 Hz mode is found");
    assert!((m.q - 50.0).abs() < 2.5, "{}", m.q);
    assert!(!m.flagged);
}

#[test]
fn mimo_sine_a_line_spacing_outside_its_range_is_refused_by_name() {
    let s = SineStructure::Linear4x4;
    for bad in [0.0, 0.01, 10.0, f64::NAN] {
        let mut st = setup(s, dwell(96.0, 1.0), "matrix");
        st.identification.resolution_hz = bad;
        let e = st.validate().unwrap_err();
        assert_eq!(e.rule, SetupRule::IdentificationInvalid);
        assert_eq!(e.field, "identification.resolution_hz");
    }
    assert_eq!(IdentificationSettings::default_frames(2), 24);
    assert_eq!(IdentificationSettings::default_frames(8), 48);
}
