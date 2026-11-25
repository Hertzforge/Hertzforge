//! The largest setup the engine accepts, 24 controls by 12 drives, run whole
//! on the simulator: arm, identify, engage and a dwell and a sweep with
//! `matrix` to their ends on a linear plate, every control and drive carried
//! in every update's record; and one control or one drive more refused by
//! name before anything is opened.
//!
//! The plate is built here, not taken from the simulator's named structures:
//! it is the sine plates' recipe ([`SineStructure`]'s eight elastic modes,
//! mass line and participation) with 24 controls on a six by four grid and
//! 12 drives on a four by three grid. With more controls than drives the
//! reference cannot be met at every control, so the error is judged against
//! the response the drives can reach (the least-squares fit through the
//! identification), as `closed_loop_linear` judges `sine-tall-6x3`, to the
//! same bounds: dwell 0.5 dB, sweep 4 dB worst and 0.5 dB median.
//!
//! [`SineStructure`]: hertzforge_hal_sim::plants::SineStructure

mod common;

use common::*;
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal_sim::plants::SINE_BAND_HZ;
use hertzforge_hal_sim::{
    JointedMode, JointedPlant, JointedStep, PlantModel, SimBackend, SimConfig,
};
use hertzforge_mimo_sine::records::{Outcome, TickRecord};
use hertzforge_mimo_sine::service::{EngineOptions, MimoSineService, ServiceError};
use hertzforge_mimo_sine::*;
use hertzforge_module_abi::StationClass;
use hertzforge_sine::scheduler::{BidirSweepConfig, SweepDirection, SweepTarget};
use hertzforge_sine::SweepRate;

const NC: usize = 24;
const ND: usize = 12;

/// The sine plates' elastic modes: frequency, damping, shape and weight.
const MODES: [(f64, f64, usize, f64); 8] = [
    (40.0, 0.010, 0, 0.67),
    (96.0, 0.020, 1, 1.0),
    (165.0, 0.015, 2, 0.6),
    (270.0, 0.025, 3, 1.0),
    (430.0, 0.020, 4, 1.0),
    (690.0, 0.030, 5, 1.0),
    (1080.0, 0.025, 6, 1.0),
    (1620.0, 0.030, 7, 1.0),
];

/// The plate's flexible shapes, `x` and `y` in half-widths (the simulator's
/// sine plates use the same eight).
fn plate_shape(k: usize, x: f64, y: f64) -> f64 {
    use std::f64::consts::PI;
    match k {
        0 => x * y,
        1 => x,
        2 => y,
        3 => x * x + y * y - 0.8,
        4 => (PI * x).cos() * y,
        5 => (PI * y).cos() * x,
        6 => (PI * x).cos() * (PI * y).cos(),
        _ => (0.5 * PI * x).sin() * (PI * y).cos(),
    }
}

/// `nx` by `ny` points spread over the plate, each moved a little off the
/// grid so no two share a symmetry.
fn grid(nx: usize, ny: usize, span: f64, tilt: f64) -> Vec<[f64; 2]> {
    let at = |i: usize, n: usize| -span + 2.0 * span * i as f64 / (n - 1) as f64;
    (0..ny)
        .flat_map(|j| {
            (0..nx).map(move |i| {
                let k = (j * nx + i) as f64;
                [
                    at(i, nx) + tilt * (1.7 * k).sin(),
                    at(j, ny) + tilt * (2.3 * k).cos(),
                ]
            })
        })
        .collect()
}

/// The 24 by 12 linear plate: mass line 8 g/V, participation 0.15, the
/// equalized step to the top of the sine band.
fn plate() -> JointedPlant {
    let controls = grid(6, 4, 0.75, 0.04);
    let drives = grid(4, 3, 0.8, 0.03);
    let (mass_line, participation) = (8.0, 0.15);
    let modes = MODES
        .iter()
        .map(|&(f_hz, zeta, k, weight)| JointedMode {
            f_hz,
            zeta,
            input_shape: drives
                .iter()
                .map(|p| weight * participation * mass_line * plate_shape(k, p[0], p[1]))
                .collect(),
            output_shape: controls
                .iter()
                .map(|p| plate_shape(k, p[0], p[1]))
                .collect(),
            reference_amplitude: 1.0,
            level: None,
        })
        .collect();
    let residual = controls
        .iter()
        .flat_map(|c| {
            drives.iter().map(move |d| {
                let d2 = (c[0] - d[0]).powi(2) + (c[1] - d[1]).powi(2);
                mass_line * (-d2).exp()
            })
        })
        .collect();
    JointedPlant {
        residual,
        step: JointedStep::equalized(SINE_BAND_HZ.1),
        ..JointedPlant::new(NC, ND, modes)
    }
}

/// Controls 1 to 24, then drives 25 to 36.
fn plate_table() -> ChannelTable {
    let mut t = ChannelTable::new();
    for k in 0..NC + ND {
        let mut c = Channel::new(k as u32 + 1);
        c.role = if k < NC {
            ChannelRole::Control
        } else {
            ChannelRole::Drive
        };
        c.range_max = 1e9;
        t.channels.push(c);
    }
    t
}

/// The common setup (every control at 1 g in phase, amplitude only, ceilings
/// 2 V) at `nc` controls and `nd` drives, identified on 1 Hz lines.
fn plate_setup(nc: usize, nd: usize, test: TestPlan) -> MimoSineSetup {
    let mut s = setup(
        hertzforge_hal_sim::plants::SineStructure::Linear4x4,
        test,
        "matrix",
    );
    let flat = s.controls[0].profile.clone();
    s.controls = (0..nc)
        .map(|i| ControlChannel {
            channel: 1 + i,
            profile: flat.clone(),
            phase: vec![],
        })
        .collect();
    s.drives = (0..nd)
        .map(|i| DriveChannel {
            channel: 1 + nc + i,
            ceiling_vpk: 2.0,
            abort_vpk: 4.0,
        })
        .collect();
    s.identification.resolution_hz = 1.0;
    s.identification.frames = Some(nd as u32 + 2);
    s
}

fn plate_service(setup: MimoSineSetup) -> Result<MimoSineService, ServiceError> {
    let b: SimBackend = SimConfig::new()
        .sample_rate_hz(FS)
        .plant(PlantModel::Jointed(plate()))
        .noise_level(NOISE)
        .seed(7)
        .shared_clock(true)
        .build();
    MimoSineService::new(
        setup,
        plate_table(),
        Box::new(b),
        law("matrix"),
        EngineOptions {
            law_budget: Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET),
            ..EngineOptions::new(StationClass::HertzforgeSim)
        },
    )
}

/// Run `test` on the plate with `matrix` to its end; the outcome, the
/// records, and the median and worst error against the reachable response.
fn plate_run(test: TestPlan, max_s: f64) -> (Option<Outcome>, Vec<TickRecord>, f64, f64) {
    let mut svc = plate_service(plate_setup(NC, ND, test)).expect("the setup is accepted");
    svc.arm().expect("arms");
    let rep = svc.identify().expect("identifies");
    assert!(rep.gate_passed, "the identification's gate");
    let id = svc.identification().unwrap().identification.clone();
    assert_eq!(id.response.dims().1, NC);
    assert_eq!(id.response.dims().2, ND);
    svc.engage().expect("engages");
    let r = run(svc, max_s);
    let reach = |t: &TickRecord| -> Vec<f64> {
        let mut h = vec![C64::new(0.0, 0.0); NC * ND];
        hertzforge_mimo_sine_law_math::setup::interp_stack(
            &id.frequency_hz,
            id.response.data(),
            NC * ND,
            t.frequency_hz,
            &mut h,
        );
        let (z, _) = hertzforge_mimo_sine_law_math::linalg::reg_pinv(&h, NC, ND, 1e-3).unwrap();
        let r: Vec<C64> = t.reference[..NC]
            .iter()
            .map(|v| C64::new(v.norm(), 0.0))
            .collect();
        let mut d = vec![C64::new(0.0, 0.0); ND];
        hertzforge_mimo_sine_law_math::linalg::mat_vec(&z, ND, NC, &r, &mut d);
        let mut c = vec![C64::new(0.0, 0.0); NC];
        hertzforge_mimo_sine_law_math::linalg::mat_vec(&h, NC, ND, &d, &mut c);
        (0..NC)
            .map(|k| (20.0 * (t.controls()[k].norm() / c[k].norm()).log10()).abs())
            .collect()
    };
    let mut v: Vec<f64> = r
        .ticks
        .iter()
        .filter(|t| !t.settling && t.miss.is_none())
        .flat_map(reach)
        .collect();
    assert!(!v.is_empty(), "no update was judged");
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (med, worst) = (v[v.len() / 2], *v.last().unwrap());
    println!(
        "24 x 12 {:?}: outcome {:?}, {} updates, median {med:.3} dB, worst {worst:.3} dB",
        r.service.setup().test_type(),
        r.outcome,
        r.ticks.len()
    );
    (r.outcome, r.ticks, med, worst)
}

/// Every update carries all 24 controls, their references and errors, and
/// all 12 drives, each one moving.
fn every_channel_carried(ticks: &[TickRecord]) {
    assert!(ticks.len() > 100, "{} updates", ticks.len());
    for t in ticks {
        assert_eq!((t.controls, t.drives, t.limits), (NC as u8, ND as u8, 0));
        assert_eq!(t.controls().len(), NC);
        assert_eq!(t.drives().len(), ND);
        assert_eq!(t.errors_db().len(), NC);
    }
    let last = ticks.last().unwrap();
    for k in 0..NC {
        assert!(
            last.control[k].norm() > 0.1,
            "control {k}: {}",
            last.control[k]
        );
        assert!(last.reference[k].norm() > 0.1, "reference {k}");
        assert!(last.error_db[k].is_finite(), "error {k}");
    }
    for (j, d) in last.drive[..ND].iter().enumerate() {
        assert!(d.norm() > 0.0, "drive {j}");
    }
}

#[test]
fn mimo_sine_matrix_dwell_on_24_controls_by_12_drives() {
    let (o, ticks, _, worst) = plate_run(dwell(96.0, 6.0), 10.0);
    assert_eq!(o, Some(Outcome::Completed));
    every_channel_carried(&ticks);
    assert!(worst < 0.5, "{worst}");
}

#[test]
fn mimo_sine_matrix_sweep_on_24_controls_by_12_drives() {
    let test = TestPlan::Sweep(BidirSweepConfig {
        band: (60.0, 180.0),
        rate: SweepRate::LogOctPerMin(2.0),
        start_hz: 60.0,
        initial_direction: SweepDirection::Up,
        target: SweepTarget::Sweeps(1),
    });
    let (o, ticks, med, worst) = plate_run(test, 80.0);
    assert_eq!(o, Some(Outcome::Completed));
    every_channel_carried(&ticks);
    assert!(worst < 4.0 && med < 0.5, "median {med}, worst {worst}");
}

#[test]
fn mimo_sine_one_control_or_one_drive_over_the_limit_is_refused_by_name() {
    let refused = |nc: usize, nd: usize| match plate_service(plate_setup(nc, nd, dwell(96.0, 1.0)))
    {
        Err(ServiceError::Setup(e)) => e,
        Ok(_) => panic!("{nc} by {nd} was accepted"),
        Err(e) => panic!("{nc} by {nd} was refused otherwise: {e}"),
    };
    let e = refused(NC + 1, ND);
    assert_eq!(e.rule, SetupRule::ControlsOutOfRange);
    assert_eq!(e.message, "25 control channels; 1 to 24 are accepted");
    let e = refused(NC, ND + 1);
    assert_eq!(e.rule, SetupRule::DrivesOutOfRange);
    assert_eq!(e.message, "13 drives; 1 to 12 are accepted");
}

/// The size of one update's record, which the loop copies into the record
/// channel on every update; printed for the record of the limits' cost.
#[test]
fn mimo_sine_an_update_record_holds_the_largest_setup() {
    println!(
        "TickRecord: {} bytes at {MAX_CONTROLS} controls, {MAX_DRIVES} drives, {MAX_LIMITS} limits",
        std::mem::size_of::<TickRecord>()
    );
    let r = TickRecord::empty(MAX_CONTROLS, MAX_DRIVES, MAX_LIMITS);
    assert_eq!(r.controls().len(), 24);
    assert_eq!(r.drives().len(), 12);
}
