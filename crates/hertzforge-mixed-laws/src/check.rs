//! The cases the composed Mixed Mode core is held to, each on a simulated
//! station, with the built-in laws in both roles (a law file in one role is
//! checked with the built-in in the other).

use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal::Backend;
use hertzforge_mixed::parts::{
    BandWidth, Direction, FloorPart, MixedIdentification, MixedLimits, NarrowbandPart, Part,
    PartBody, PartSchedule, PartSweepTarget, PartWindow, SweepLaw, ToneAmplitude, TonePart,
};
use hertzforge_mixed::{
    MixedConfig, MixedLawFactories, MixedPhase, MixedService, PartKind, DEFAULT_TRACKING_HZ,
    MIN_TONE_SPACING_WIDTHS,
};
use hertzforge_random::service::config::{ControlSet, LevelSchedule};
use hertzforge_random_laws::CheckCase;
use std::fmt;

/// One case of the Mixed Mode check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MixedCheckCase {
    /// A floor alone: parity with a Random run on the same plant.
    FloorOnly,
    /// One dwelling tone on the floor.
    FixedToneOnFloor,
    /// A tone swept through the plant's mode on the floor.
    SweptToneThroughMode,
    /// Two tones at the closest spacing allowed.
    TwoTonesClose,
    /// A narrowband dwelling over the floor.
    NarrowbandFixed,
    /// A narrowband swept over the floor.
    NarrowbandSwept,
    /// Every part moved through a level ladder.
    LevelLadder,
    /// A flat wire behind a hardware station's lag.
    TransportDelay,
    /// A tone far over a low floor (a low-level loopback's ratio, A^2 / G
    /// about 3e4 Hz), swept from 100 Hz at 2 oct/min.
    ToneFarOverFloor,
}

impl MixedCheckCase {
    /// Every case, in the order the check runs them.
    pub const ALL: [MixedCheckCase; 9] = [
        MixedCheckCase::FloorOnly,
        MixedCheckCase::FixedToneOnFloor,
        MixedCheckCase::SweptToneThroughMode,
        MixedCheckCase::TwoTonesClose,
        MixedCheckCase::NarrowbandFixed,
        MixedCheckCase::NarrowbandSwept,
        MixedCheckCase::LevelLadder,
        MixedCheckCase::TransportDelay,
        MixedCheckCase::ToneFarOverFloor,
    ];

    /// The stable record and wire name.
    pub fn name(self) -> &'static str {
        match self {
            MixedCheckCase::FloorOnly => "floor_only",
            MixedCheckCase::FixedToneOnFloor => "fixed_tone_on_floor",
            MixedCheckCase::SweptToneThroughMode => "swept_tone_through_mode",
            MixedCheckCase::TwoTonesClose => "two_tones_close",
            MixedCheckCase::NarrowbandFixed => "narrowband_fixed",
            MixedCheckCase::NarrowbandSwept => "narrowband_swept",
            MixedCheckCase::LevelLadder => "level_ladder",
            MixedCheckCase::TransportDelay => "transport_delay",
            MixedCheckCase::ToneFarOverFloor => "tone_far_over_floor",
        }
    }

    /// The case with `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The simulated article or condition, in words.
    pub fn plant(self) -> &'static str {
        match self {
            MixedCheckCase::FloorOnly => "one mode on a flat floor, no tones or bands",
            MixedCheckCase::FixedToneOnFloor => "one mode; a tone dwelling below it",
            MixedCheckCase::SweptToneThroughMode => "one mode; a tone swept through it",
            MixedCheckCase::TwoTonesClose => {
                "one mode; two dwelling tones two tracking widths apart"
            }
            MixedCheckCase::NarrowbandFixed => "one mode; a band dwelling away from it",
            MixedCheckCase::NarrowbandSwept => "one mode; a band swept through it",
            MixedCheckCase::LevelLadder => "one mode; a tone and a band through the level ladder",
            MixedCheckCase::TransportDelay => {
                "a flat wire behind a hardware station's lag; a tone and a band"
            }
            MixedCheckCase::ToneFarOverFloor => {
                "one mode; a 0.2 g tone swept over a floor of 1.3e-6 g^2/Hz"
            }
        }
    }

    /// What the case asks of the laws, in words.
    pub fn requirement(self) -> &'static str {
        match self {
            MixedCheckCase::FloorOnly => {
                "the floor's rms over its clean lines within 1 dB of its reference throughout"
            }
            MixedCheckCase::FixedToneOnFloor => {
                "the tone within its alarm band and the floor's clean lines within theirs at full level"
            }
            MixedCheckCase::SweptToneThroughMode => {
                "the tone within its alarm band through the mode; the floor within its alarm band"
            }
            MixedCheckCase::TwoTonesClose => {
                "each tone within its alarm band; neither tone's reading carries the other"
            }
            MixedCheckCase::NarrowbandFixed => {
                "the band's level within its alarm band widened to its scatter; the floor's clean lines within theirs"
            }
            MixedCheckCase::NarrowbandSwept => {
                "the band's moving-frame level within its widened alarm band through the mode"
            }
            MixedCheckCase::LevelLadder => {
                "at each level every part within its alarm band after its settle time"
            }
            MixedCheckCase::TransportDelay => {
                "the run completes and every part settles within its alarm band"
            }
            MixedCheckCase::ToneFarOverFloor => {
                "the floor's rms within 1 dB with no abort while the tone sweeps; the tone within its alarm band"
            }
        }
    }
}

/// Why a check case could not be run.
#[derive(Debug, Clone, PartialEq)]
pub enum MixedCheckError {
    /// The composed core cannot run a check yet.
    NotBuilt,
    /// The case's run failed; the core's words.
    Run(String),
}

impl fmt::Display for MixedCheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MixedCheckError::NotBuilt => f.write_str("the Mixed Mode check cannot run yet"),
            MixedCheckError::Run(s) => write!(f, "the check case failed to run: {s}"),
        }
    }
}

impl std::error::Error for MixedCheckError {}

/// The rate the check's stations run at, Hz.
pub const MIXED_CHECK_FS_HZ: f64 = 4096.0;
/// The transport-delay case's lag, frames: the NI loopback chassis's measured
/// lag.
pub const MIXED_CHECK_DELAY_FRAMES: usize = 553;
/// The resonant cases' mode.
pub const MIXED_CHECK_MODE: (f64, f64) = (300.0, 8.0);

/// The simulated station a case runs on, from the case's own facts: the
/// controller builds the station from this (the core names no simulator).
#[derive(Debug, Clone, PartialEq)]
pub struct MixedCheckStation {
    /// Rate, Hz.
    pub fs_hz: f64,
    /// Resonator modes `(f_n, Q)`; empty for a flat wire.
    pub modes: Vec<(f64, f64)>,
    /// Broadband feedthrough.
    pub feedthrough: f64,
    /// Measurement noise, g rms.
    pub noise_rms_g: f64,
    /// Transport delay, frames.
    pub transport_delay_samples: usize,
    /// The block the station is pumped in, frames.
    pub block_frames: usize,
    /// Seed of the station's noise.
    pub seed: u64,
}

/// The station `case` runs on.
pub fn mixed_check_station(case: MixedCheckCase) -> MixedCheckStation {
    let (modes, delay) = match case {
        MixedCheckCase::TransportDelay => (Vec::new(), MIXED_CHECK_DELAY_FRAMES),
        _ => (vec![MIXED_CHECK_MODE], 40),
    };
    MixedCheckStation {
        fs_hz: MIXED_CHECK_FS_HZ,
        modes,
        feedthrough: 1.0,
        noise_rms_g: 1e-4,
        transport_delay_samples: delay,
        block_frames: 64,
        seed: 0x4D49_5843,
    }
}

/// The channel table every case runs on: a control accelerometer on 0, the
/// drive on 1.
pub fn mixed_check_table() -> ChannelTable {
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

fn part(id: &str, body: PartBody) -> Part {
    Part {
        id: id.into(),
        name: String::new(),
        enabled: true,
        window: PartWindow::default(),
        body,
    }
}

fn tone(id: &str, schedule: PartSchedule, g_pk: f64) -> Part {
    part(
        id,
        PartBody::Tone(TonePart {
            schedule,
            amplitude: ToneAmplitude::Constant { g_pk },
            alarm_db: (1.5, 1.5),
            abort_db: (3.0, 3.0),
        }),
    )
}

fn band(id: &str, schedule: PartSchedule) -> Part {
    part(
        id,
        PartBody::Narrowband(NarrowbandPart {
            schedule,
            level_g2_hz: 0.05,
            width: BandWidth::Hz { hz: 40.0 },
            alarm_db: (3.0, 3.0),
            abort_db: (6.0, 6.0),
        }),
    )
}

fn fixed(hz: f64) -> PartSchedule {
    PartSchedule::Fixed {
        hz,
        for_s: None,
        for_cycles: None,
    }
}

fn sweep(lo: f64, hi: f64, rate: f64) -> PartSchedule {
    PartSchedule::Sweep {
        lo_hz: lo,
        hi_hz: hi,
        start_hz: lo,
        direction: Direction::Up,
        law: SweepLaw::LinearHzPerS,
        rate,
        target: PartSweepTarget::Sweeps { n: 2 },
    }
}

/// How long the case runs once its parts are in, seconds.
pub fn mixed_check_run_s(case: MixedCheckCase) -> f64 {
    match case {
        MixedCheckCase::SweptToneThroughMode => 140.0,
        MixedCheckCase::NarrowbandSwept => 140.0,
        MixedCheckCase::ToneFarOverFloor => 60.0,
        MixedCheckCase::LevelLadder => 30.0,
        _ => 25.0,
    }
}

/// The configuration `case` runs: 4 Hz lines, 60 degrees of freedom, a flat
/// 0.01 g^2/Hz floor from 40 to 1000 Hz, and the case's parts.
pub fn mixed_check_config(case: MixedCheckCase) -> MixedConfig {
    let level = if case == MixedCheckCase::ToneFarOverFloor {
        1.3e-6
    } else {
        0.01
    };
    let mut t = BreakpointTable::new("floor");
    t.push(BreakpointRow::psd(40.0, level));
    t.push(BreakpointRow::psd(1000.0, level));
    let floor = part(
        "floor",
        PartBody::Floor(FloorPart {
            breakpoints: t,
            alarm_db: (3.0, 3.0),
            abort_db: (6.0, 6.0),
        }),
    );
    let tracking = DEFAULT_TRACKING_HZ;
    let mut parts = vec![floor];
    match case {
        MixedCheckCase::FloorOnly => {}
        MixedCheckCase::FixedToneOnFloor => parts.push(tone("t1", fixed(250.0), 2.0)),
        MixedCheckCase::SweptToneThroughMode => {
            parts.push(tone("t1", sweep(150.0, 450.0, 4.0), 2.0))
        }
        MixedCheckCase::TwoTonesClose => {
            parts.push(tone("t1", fixed(250.0), 2.0));
            parts.push(tone(
                "t2",
                fixed(250.0 + MIN_TONE_SPACING_WIDTHS * tracking),
                2.0,
            ));
        }
        MixedCheckCase::NarrowbandFixed => parts.push(band("b1", fixed(600.0))),
        MixedCheckCase::NarrowbandSwept => parts.push(band("b1", sweep(150.0, 450.0, 4.0))),
        MixedCheckCase::ToneFarOverFloor => parts.push(tone(
            "t1",
            PartSchedule::Sweep {
                lo_hz: 100.0,
                hi_hz: 400.0,
                start_hz: 100.0,
                direction: Direction::Up,
                law: SweepLaw::LogOctPerMin,
                rate: 2.0,
                target: PartSweepTarget::Sweeps { n: 2 },
            },
            0.2,
        )),
        MixedCheckCase::LevelLadder | MixedCheckCase::TransportDelay => {
            parts.push(tone("t1", fixed(250.0), 2.0));
            parts.push(band("b1", fixed(600.0)));
        }
    }
    let ladder = case == MixedCheckCase::LevelLadder;
    MixedConfig {
        df_hz: 4.0,
        lines: 256,
        nfft: Some(1024),
        blocks_per_window: 8,
        control_dof: 60.0,
        control: ControlSet::single(0),
        parts,
        exclusion_hz: 8.0,
        tracking_hz: tracking,
        schedule: LevelSchedule {
            start_db: if ladder { -12.0 } else { 0.0 },
            table_db: if ladder {
                vec![-12.0, -6.0, 0.0]
            } else {
                vec![0.0]
            },
            dwell_s: vec![10.0],
            ..LevelSchedule::auto_default()
        },
        full_level_duration_s: Some(mixed_check_run_s(case)),
        level_ramp_s: 1.0,
        shutdown_ramp_s: 1.0,
        part_ramp_s: 1.0,
        floor_settle_s: 15.0,
        soft_start_db: 0.0,
        soft_start_relax_db: 0.0,
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
        law: Default::default(),
        law_guards: None,
        control_tick_s: None,
        metadata: Vec::new(),
    }
}

/// Run `case` with `laws` on `backend` (the station the controller built from
/// [`mixed_check_station`]) and judge it against its requirement. A run that
/// trips fails the case, naming the trip.
pub fn run_mixed_check_case(
    case: MixedCheckCase,
    laws: &MixedLawFactories,
    backend: Box<dyn Backend>,
) -> Result<CheckCase, MixedCheckError> {
    let spec = mixed_check_station(case);
    let run = |e: hertzforge_mixed::MixedError| MixedCheckError::Run(e.to_string());
    let mut s = MixedService::new(
        mixed_check_config(case),
        mixed_check_table(),
        backend,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        spec.seed,
        None,
    )
    .map_err(run)?
    .with_laws(laws.clone())
    .with_block_frames(spec.block_frames);
    s.arm().map_err(run)?;
    s.run_sysid(0).map_err(run)?;
    if !s.coherence_gate().map_err(run)? {
        return Ok(verdict(
            case,
            false,
            "the identification's coherence gate failed".into(),
        ));
    }
    s.proceed_after_review().map_err(run)?;
    // (part id, error dB) every half second once every part is in and settled.
    let mut readings: Vec<(String, f64, f64)> = Vec::new();
    let mut floor_errs: Vec<f64> = Vec::new();
    let mut at_rung_end: Vec<(f64, Vec<Option<f64>>)> = Vec::new();
    let mut since_running = 0.0;
    let mut alive = true;
    while alive {
        for _ in 0..2 {
            alive = s.run_chunk().map_err(run)?;
            if !alive {
                break;
            }
        }
        let st = s.live_status();
        if st.phase != MixedPhase::Running {
            continue;
        }
        since_running += 0.5;
        if since_running < 6.0 {
            continue;
        }
        for r in &st.parts {
            if let Some(e) = r.error_db {
                match r.kind {
                    PartKind::Floor => floor_errs.push(e),
                    _ => readings.push((r.id.clone(), r.freq_hz.unwrap_or(0.0), e)),
                }
            }
        }
        if case == MixedCheckCase::LevelLadder {
            at_rung_end.push((st.level_db, st.parts.iter().map(|r| r.error_db).collect()));
        }
    }
    if let Some(t) = s.trip() {
        return Ok(verdict(
            case,
            false,
            format!("the run tripped: {}", t.words()),
        ));
    }
    let rms = |v: &[f64]| (v.iter().map(|x| x * x).sum::<f64>() / v.len().max(1) as f64).sqrt();
    let floor_ok = !floor_errs.is_empty() && floor_errs.iter().all(|e| e.abs() <= 1.0);
    let mut detail = format!(
        "floor rms error worst {:+.2} dB over {} readings",
        floor_errs
            .iter()
            .fold(0.0f64, |m, e| if e.abs() > m.abs() { *e } else { m }),
        floor_errs.len()
    );
    let mut ok = floor_ok;
    let ids: Vec<String> = s
        .config()
        .parts
        .iter()
        .skip(1)
        .map(|p| p.id.clone())
        .collect();
    for id in &ids {
        let v: Vec<f64> = readings
            .iter()
            .filter(|r| &r.0 == id)
            .map(|r| r.2)
            .collect();
        let kind_alarm = if id.starts_with('t') { 1.5 } else { 3.0 };
        let within = v.iter().filter(|e| e.abs() <= kind_alarm).count();
        let share = within as f64 / v.len().max(1) as f64;
        let worst = v.iter().fold(0.0f64, |m, e| m.max(e.abs()));
        let part_ok = !v.is_empty() && share >= 0.95 && worst <= 2.0 * kind_alarm;
        ok &= part_ok;
        detail.push_str(&format!(
            "; {id}: {} readings, rms {:.2} dB, worst {worst:.2} dB, {:.0} % within its alarm",
            v.len(),
            rms(&v),
            share * 100.0
        ));
    }
    if case == MixedCheckCase::LevelLadder {
        let levels: Vec<f64> = at_rung_end.iter().map(|x| x.0).collect();
        let climbed =
            levels.iter().any(|l| (l + 6.0).abs() < 0.1) && levels.iter().any(|l| l.abs() < 0.1);
        ok &= climbed;
        detail.push_str(if climbed {
            "; climbed -12, -6, 0 dB"
        } else {
            "; did not climb the ladder"
        });
    }
    Ok(verdict(case, ok, detail))
}

fn verdict(case: MixedCheckCase, passed: bool, detail: String) -> CheckCase {
    CheckCase {
        name: case.name().to_owned(),
        passed,
        detail: format!("{}: {detail}", case.requirement()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_cases_with_unique_names_that_read_back() {
        let mut names: Vec<_> = MixedCheckCase::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), 9);
        for c in MixedCheckCase::ALL {
            assert_eq!(MixedCheckCase::from_name(c.name()), Some(c));
            assert!(!c.plant().is_empty() && !c.requirement().is_empty());
        }
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 9);
        assert_eq!(MixedCheckCase::from_name("nope"), None);
    }

    #[test]
    fn every_case_states_a_valid_configuration_and_a_station() {
        for c in MixedCheckCase::ALL {
            mixed_check_config(c)
                .validate(Some(MIXED_CHECK_FS_HZ))
                .unwrap();
            let st = mixed_check_station(c);
            assert_eq!(st.fs_hz, MIXED_CHECK_FS_HZ);
        }
        assert_eq!(
            mixed_check_station(MixedCheckCase::TransportDelay).transport_delay_samples,
            553
        );
    }
}
