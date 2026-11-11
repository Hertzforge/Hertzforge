//! Builders the parts and schedule tests share.

use super::*;
use hertzforge_breakpoints::BreakpointRow;
use hertzforge_random::service::config::{ControlSet, LevelSchedule};

pub(crate) fn floor(id: &str) -> Part {
    let mut t = BreakpointTable::new("floor");
    t.push(BreakpointRow::psd(20.0, 0.01));
    t.push(BreakpointRow::psd(2000.0, 0.01));
    Part {
        id: id.into(),
        name: String::new(),
        enabled: true,
        window: PartWindow::default(),
        body: PartBody::Floor(FloorPart {
            breakpoints: t,
            alarm_db: DEFAULT_FLOOR_ALARM_DB,
            abort_db: DEFAULT_FLOOR_ABORT_DB,
        }),
    }
}

pub(crate) fn tone(id: &str, schedule: PartSchedule) -> Part {
    Part {
        id: id.into(),
        name: String::new(),
        enabled: true,
        window: PartWindow::default(),
        body: PartBody::Tone(TonePart {
            schedule,
            amplitude: ToneAmplitude::Constant { g_pk: 1.0 },
            alarm_db: DEFAULT_TONE_ALARM_DB,
            abort_db: DEFAULT_TONE_ABORT_DB,
        }),
    }
}

pub(crate) fn band(id: &str, schedule: PartSchedule, width: BandWidth) -> Part {
    Part {
        id: id.into(),
        name: String::new(),
        enabled: true,
        window: PartWindow::default(),
        body: PartBody::Narrowband(NarrowbandPart {
            schedule,
            level_g2_hz: 0.05,
            width,
            alarm_db: DEFAULT_BAND_ALARM_DB,
            abort_db: DEFAULT_BAND_ABORT_DB,
        }),
    }
}

pub(crate) fn fixed(hz: f64) -> PartSchedule {
    PartSchedule::Fixed { hz, for_s: None }
}

pub(crate) fn sweep(lo: f64, hi: f64, start: f64, target: PartSweepTarget) -> PartSchedule {
    PartSchedule::Sweep {
        lo_hz: lo,
        hi_hz: hi,
        start_hz: start,
        direction: Direction::Up,
        law: SweepLaw::LinearHzPerS,
        rate: 10.0,
        target,
    }
}

pub(crate) fn follow(of: &str, ratio: f64) -> PartSchedule {
    PartSchedule::Follow {
        of: of.into(),
        ratio,
    }
}

pub(crate) fn config(parts: Vec<Part>) -> MixedConfig {
    MixedConfig {
        df_hz: 1.0,
        lines: 2048,
        control_dof: 120.0,
        control: ControlSet::single(0),
        parts,
        exclusion_hz: DEFAULT_EXCLUSION_HZ,
        tracking_hz: DEFAULT_TRACKING_HZ,
        schedule: LevelSchedule::auto_default(),
        full_level_duration_s: Some(60.0),
        level_ramp_s: 0.0,
        shutdown_ramp_s: 0.0,
        identification: MixedIdentification::default(),
        limits: MixedLimits::default(),
        capture: Default::default(),
        law: MixedLawChoice::default(),
        law_guards: None,
        control_tick_s: None,
        metadata: Vec::new(),
    }
}
