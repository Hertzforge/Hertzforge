//! Stations and configurations the closed-loop tests share.
#![allow(dead_code)]

use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal_sim::SimConfig;
use hertzforge_mixed::{
    parts::{
        BandWidth, FloorPart, MixedIdentification, MixedLawChoice, MixedLimits, NarrowbandPart,
        Part, PartBody, PartSchedule, PartWindow, ToneAmplitude, TonePart,
    },
    MixedConfig, MixedService,
};
use hertzforge_random::service::config::{ControlSet, LevelSchedule};

/// The run's rate, Hz.
pub const FS: f64 = 4096.0;
/// The plant's resonance and its Q.
pub const FN: f64 = 300.0;
pub const Q: f64 = 8.0;
/// The plant's transport delay, frames.
pub const DELAY: usize = 40;
/// The station's block, frames.
pub const BLOCK: usize = 64;

pub fn table() -> ChannelTable {
    let mut table = ChannelTable::new();
    let mut c = Channel::new(0);
    c.role = ChannelRole::Control;
    c.units = Units::AccelG;
    table.channels.push(c);
    let mut d = Channel::new(1);
    d.role = ChannelRole::Drive;
    d.units = Units::VoltageV;
    table.channels.push(d);
    table
}

/// The lagged resonant plant every closed-loop test states: one mode at
/// [`FN`] Hz, Q [`Q`], feedthrough 1, noise 1e-4, [`DELAY`] frames of
/// transport delay.
pub fn station(delay: usize) -> Box<dyn hertzforge_hal::Backend> {
    Box::new(
        SimConfig::new()
            .sample_rate_hz(FS)
            .resonance(FN, Q)
            .feedthrough(1.0)
            .noise_level(1e-4)
            .transport_delay_samples(delay)
            .shared_clock(true)
            .seed(0x4D49_5845)
            .build(),
    )
}

pub fn floor(level: f64) -> Part {
    let mut t = BreakpointTable::new("floor");
    t.push(BreakpointRow::psd(40.0, level));
    t.push(BreakpointRow::psd(1000.0, level));
    Part {
        id: "floor".into(),
        name: String::new(),
        enabled: true,
        window: PartWindow::default(),
        body: PartBody::Floor(FloorPart {
            breakpoints: t,
            alarm_db: (3.0, 3.0),
            abort_db: (6.0, 6.0),
        }),
    }
}

pub fn tone(id: &str, schedule: PartSchedule, g_pk: f64) -> Part {
    Part {
        id: id.into(),
        name: String::new(),
        enabled: true,
        window: PartWindow::default(),
        body: PartBody::Tone(TonePart {
            schedule,
            amplitude: ToneAmplitude::Constant { g_pk },
            alarm_db: (1.5, 1.5),
            abort_db: (3.0, 3.0),
        }),
    }
}

pub fn band(id: &str, schedule: PartSchedule, level: f64, width_hz: f64) -> Part {
    Part {
        id: id.into(),
        name: String::new(),
        enabled: true,
        window: PartWindow::default(),
        body: PartBody::Narrowband(NarrowbandPart {
            schedule,
            level_g2_hz: level,
            width: BandWidth::Hz { hz: width_hz },
            alarm_db: (3.0, 3.0),
            abort_db: (6.0, 6.0),
        }),
    }
}

pub fn fixed(hz: f64) -> PartSchedule {
    PartSchedule::Fixed {
        hz,
        for_s: None,
        for_cycles: None,
    }
}

/// 4 Hz lines (1024-point frames at 4096 Hz), 8 frames a loop (one second),
/// 60 degrees of freedom, a 6 dB soft start, one-second part ramps.
pub fn config(parts: Vec<Part>) -> MixedConfig {
    MixedConfig {
        df_hz: 4.0,
        lines: 256,
        nfft: Some(1024),
        blocks_per_window: 8,
        control_dof: 60.0,
        control: ControlSet::single(0),
        parts,
        exclusion_hz: 8.0,
        tracking_hz: 0.5,
        schedule: LevelSchedule {
            start_db: 0.0,
            table_db: vec![0.0],
            ..LevelSchedule::auto_default()
        },
        full_level_duration_s: None,
        level_ramp_s: 1.0,
        shutdown_ramp_s: 1.0,
        part_ramp_s: 1.0,
        floor_settle_s: 20.0,
        soft_start_db: 6.0,
        soft_start_relax_db: 6.0,
        shaker: None,
        moving_mass: Default::default(),
        crest_factor: 3.0,
        identification: MixedIdentification {
            sysid_level_db: -12.0,
            ..MixedIdentification::default()
        },
        limits: MixedLimits {
            drive_vpk_ceiling: 20.0,
            ..MixedLimits::default()
        },
        capture: Default::default(),
        law: MixedLawChoice::default(),
        law_guards: None,
        control_tick_s: None,
        metadata: Vec::new(),
    }
}

/// Build, arm, identify, gate and engage.
pub fn engaged(cfg: MixedConfig, delay: usize) -> MixedService {
    engaged_with(cfg, delay, hertzforge_mixed::MixedLawFactories::builtin())
}

/// As [`engaged`], with `laws`.
pub fn engaged_with(
    cfg: MixedConfig,
    delay: usize,
    laws: hertzforge_mixed::MixedLawFactories,
) -> MixedService {
    let mut s = MixedService::new(
        cfg,
        table(),
        station(delay),
        hertzforge_module_abi::StationClass::HertzforgeSim,
        1,
        None,
    )
    .unwrap()
    .with_laws(laws)
    .with_block_frames(BLOCK);
    s.arm().unwrap();
    s.run_sysid(0).unwrap();
    assert!(s.coherence_gate().unwrap());
    s.proceed_after_review().unwrap();
    s
}

/// Run `seconds` of station time.
pub fn run_for(s: &mut MixedService, seconds: f64) -> bool {
    let chunks = (seconds / 0.25).ceil() as usize;
    for _ in 0..chunks {
        if !s.run_chunk().unwrap() {
            return false;
        }
    }
    true
}
