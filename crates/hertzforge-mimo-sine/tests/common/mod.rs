//! Shared fixtures: a sine structure on the simulator station, a setup for
//! it, and a run driven to its end with its records collected.

#![allow(dead_code)]

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal_sim::plants::{SineStructure, SINE_CALIBRATION_RATE_HZ};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_mimo_sine::records::{Event, Outcome, TickRecord};
use hertzforge_mimo_sine::service::{EngineOptions, MimoSineService};
use hertzforge_mimo_sine::*;
use hertzforge_module_abi::StationClass;
use hertzforge_sine::profile::{ProfileRow, SineProfile};
use hertzforge_sine::scheduler::{DwellFor, DwellPlan, LadderConfig, RungFor};
use std::sync::Arc;

#[allow(unused_imports)]
pub use hertzforge_sdm::C64;

pub const FS: f64 = SINE_CALIBRATION_RATE_HZ;

/// The table: the structure's controls, its other responses (limit
/// channels), then its drives; channel `k` has index `k + 1`.
pub fn table(s: SineStructure) -> ChannelTable {
    let (n_out, n_d) = s.dims();
    let mut t = ChannelTable::new();
    for k in 0..n_out + n_d {
        let mut c = Channel::new(k as u32 + 1);
        c.role = if k < s.n_controls() {
            ChannelRole::Control
        } else if k < n_out {
            ChannelRole::Monitor
        } else {
            ChannelRole::Drive
        };
        c.range_max = 1e9;
        t.channels.push(c);
    }
    t
}

pub fn backend(s: SineStructure, noise: f64, seed: u64) -> Box<dyn hertzforge_hal::Backend> {
    let b: SimBackend = SimConfig::new()
        .sample_rate_hz(FS)
        .plant(s.build().model)
        .noise_level(noise)
        .seed(seed)
        .shared_clock(true)
        .build();
    Box::new(b)
}

/// A flat acceleration profile at `g` over 5 to 2000 Hz, alarm and abort
/// bands as given.
pub fn flat(g: f64, alarm_db: f64, abort_db: f64) -> SineProfile {
    let mut p = SineProfile::new("flat");
    p.global_alarm_db = (alarm_db, alarm_db);
    p.global_abort_db = (abort_db, abort_db);
    p.push(ProfileRow::accel(5.0, g));
    p.push(ProfileRow::accel(2000.0, g));
    p
}

/// A setup for structure `s`: every control at its full level in phase,
/// amplitude only, the article responses (if any) as limit channels with
/// bound `limit_g`, every drive with ceiling 2 V and abort 4 V.
pub fn setup(s: SineStructure, test: TestPlan, law: &str) -> MimoSineSetup {
    let (n_out, n_d) = s.dims();
    let nc = s.n_controls();
    let g = s.full_level_g();
    MimoSineSetup {
        controls: (0..nc)
            .map(|i| ControlChannel {
                channel: 1 + i,
                profile: flat(g, 1.5, 6.0),
                phase: vec![],
            })
            .collect(),
        drives: (0..n_d)
            .map(|i| DriveChannel {
                channel: 1 + n_out + i,
                ceiling_vpk: 2.0,
                abort_vpk: 4.0,
            })
            .collect(),
        limits: vec![],
        phase_specified: false,
        test,
        schedule: Schedule {
            mode: LevelMode::Auto,
            ladder: LadderConfig {
                sysid_level_db: -20.0,
                start_db: 0.0,
                table_db: vec![0.0],
                per_rung: RungFor::Seconds(1e6),
                full_for: RungFor::Seconds(1e6),
            },
        },
        fs_hz: FS,
        tick_s: 0.01,
        tracking: Tracking::default(),
        identification: IdentificationSettings {
            level_db: -20.0,
            resolution_hz: 0.25,
            frames: Some(6.max(n_d as u32 + 2)),
            trust_coherence: 0.5,
            gate_coherence: 0.5,
            review: false,
        },
        law: LawChoice {
            id: law.into(),
            params: Default::default(),
        },
        requests: RequestSettings::default(),
        guards: GuardSettings::default(),
    }
}

pub fn dwell(f: f64, s: f64) -> TestPlan {
    TestPlan::Dwell(DwellPlan {
        freq_hz: f,
        for_: DwellFor::Seconds(s),
    })
}

/// A run's collected records.
pub struct Run {
    pub service: MimoSineService,
    pub outcome: Option<Outcome>,
    pub ticks: Vec<TickRecord>,
    pub events: Vec<Event>,
}

/// Arm, identify and engage `setup` on `s` under `factory`.
pub fn engaged(
    s: SineStructure,
    setup: MimoSineSetup,
    factory: Arc<dyn MimoSineLawFactory>,
    noise: f64,
) -> MimoSineService {
    let mut svc = MimoSineService::new(
        setup,
        table(s),
        backend(s, noise, 7),
        factory,
        EngineOptions::new(StationClass::HertzforgeSim),
    )
    .expect("the setup is accepted");
    svc.arm().expect("arms");
    let rep = svc.identify().expect("identifies");
    assert!(
        rep.gate_passed,
        "the identification's gate: {:?}",
        rep.band_coherence
    );
    svc.engage().expect("engages");
    svc
}

/// Run an engaged service to its end (at most `max_s` of test time) and
/// collect its records.
pub fn run(mut service: MimoSineService, max_s: f64) -> Run {
    let (rt, re) = service.take_records().expect("records");
    let mut ticks = Vec::new();
    let mut events = Vec::new();
    let batches = (max_s * FS / 512.0) as u64 + 64;
    let mut outcome = None;
    for _ in 0..batches {
        let alive = service.run_batches(16).expect("runs");
        ticks.extend(rt.try_iter());
        events.extend(re.try_iter());
        if !alive {
            break;
        }
    }
    if let service::ServiceState::Ended(o) = service.state() {
        outcome = Some(o);
    }
    Run {
        service,
        outcome,
        ticks,
        events,
    }
}

/// The built-in law `id` with its defaults.
pub fn law(id: &str) -> Arc<dyn MimoSineLawFactory> {
    laws::builtin(id).expect("a built-in law")
}

/// The worst |error| in dB over the records after `from_s`, judged records
/// only (outside settle windows).
pub fn worst_error_db(r: &Run, from_s: f64) -> f64 {
    r.ticks
        .iter()
        .filter(|t| t.time_s >= from_s && !t.settling && t.miss.is_none())
        .flat_map(|t| t.errors_db().iter().map(|e| e.abs()).collect::<Vec<_>>())
        .fold(0.0, f64::max)
}

/// The measurement noise on every input in the closed-loop tests, g rms per
/// sample: about 1 % of the sine structures' full level.
pub const NOISE: f64 = 0.01;
