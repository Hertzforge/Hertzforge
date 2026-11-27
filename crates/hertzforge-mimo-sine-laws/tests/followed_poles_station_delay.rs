//! The station delay the law check tells `followed_poles`, against the
//! identifications the engine measures on the check's own station. On the
//! plate without its modes (a real response, flat in frequency, behind the
//! station's delay) the identification turned back by the station delay less
//! the engine's lag is real at every line: the check's station delay is the
//! delay the identification holds, and one sample more, or the lag left in,
//! is not. On the plate with its modes the engine's lag, a cross-correlation's
//! peak, is not that delay: a law that took the lag for the whole delay, and
//! took nothing more out, would fit with the wrong one.

use hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET;
use hertzforge_mimo_sine::laws::{builtin, sdk_setup};
use hertzforge_mimo_sine::service::{EngineOptions, MimoSineService};
use hertzforge_mimo_sine_law_math::{Setup, C64};
use hertzforge_mimo_sine_laws::check::{
    check_table, plan_setup, sim_station, station_delay_s, MimoSineCheckCase, StationSpec,
    SIM_FRAMES_AHEAD,
};
use hertzforge_module_abi::StationClass;
use std::f64::consts::{PI, TAU};

/// The identification the engine gives the law for the slow sweep on the
/// station `spec`, as the law's mathematics takes it.
fn identified(spec: &StationSpec) -> (Setup, usize) {
    let plan = MimoSineCheckCase::SlowSweep.run_plan();
    let f = builtin("followed_poles").unwrap();
    let setup = plan_setup(&plan, "followed_poles", &f.manifest());
    let mut svc = MimoSineService::new(
        setup,
        check_table(plan.structure),
        sim_station(spec).unwrap(),
        f,
        EngineOptions {
            law_budget: Some(TEST_RELAX_COMPUTE_BUDGET),
            ..EngineOptions::new(StationClass::HertzforgeSim)
        },
    )
    .unwrap();
    svc.arm().unwrap();
    let rep = svc.identify().unwrap();
    assert!(rep.gate_passed);
    assert_eq!(rep.frames_ahead, SIM_FRAMES_AHEAD);
    let lag = rep.lag_frames;
    (sdk_setup(&svc.make_law_setup().unwrap()).to_math(), lag)
}

/// The furthest any line of the identification in the band is from real once
/// turned back by `tau_s`, rad: each line's common phase read from its
/// elements' squares' sum, up to their signs.
fn off_real(s: &Setup, tau_s: f64) -> f64 {
    let id = &s.identification;
    let mut worst: f64 = 0.0;
    for (k, f) in id.grid_hz.iter().enumerate() {
        if *f < s.band_lo_hz || *f > s.band_hi_hz {
            continue;
        }
        let back = C64::from_polar(1.0, TAU * f * tau_s);
        let sq: C64 = id
            .response
            .line(k)
            .iter()
            .map(|h| (h * back) * (h * back))
            .sum();
        let p = (0.5 * sq.arg()).rem_euclid(PI);
        worst = worst.max(p.min(PI - p));
    }
    worst
}

fn check(transport_delay_s: f64) {
    let flat = StationSpec {
        transport_delay_s,
        ..MimoSineCheckCase::TransportDelay.station()
    };
    let (s, lag) = identified(&flat);
    let station = station_delay_s(&flat).unwrap();
    let told = station - s.loop_delay.transport_delay_s;
    let right = off_real(&s, told);
    let one_more = off_real(&s, told + 1.0 / flat.fs_hz);
    let lag_left_in = off_real(&s, station);
    let plate = StationSpec {
        transport_delay_s,
        ..MimoSineCheckCase::SlowSweep.station()
    };
    let (_, plate_lag) = identified(&plate);
    let at = |d: f64| (d * flat.fs_hz).round() as i64;
    println!(
        "transport {transport_delay_s} s: the station's delay {} samples; without the modes \
         the lag is {lag} and the law takes out {:.1} samples, {right:.2e} rad off real (one \
         sample more {one_more:.2e}, the lag left in {lag_left_in:.2e}); with them the lag is \
         {plate_lag}",
        at(station),
        told * flat.fs_hz,
    );
    assert_eq!(station_delay_s(&plate), Some(station));
    assert!(right < 0.005, "told the check's delay: {right} rad");
    // Each wrong delay must show, or the check above proves nothing.
    for wrong in [one_more, lag_left_in] {
        assert!(wrong > 5.0 * right && wrong > 0.02, "{wrong} rad");
    }
    let off = plate_lag as i64 - at(station);
    assert!(
        off.abs() >= 8,
        "the lag is the delay on the plate: {off} samples"
    );
}

#[test]
fn mimo_sine_followed_poles_is_told_the_delay_the_identification_holds() {
    check(0.0);
}

#[test]
fn mimo_sine_followed_poles_is_told_the_delay_with_a_transport_delay() {
    check(0.05);
}
