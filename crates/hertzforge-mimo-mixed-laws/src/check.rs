//! The cases the composed MIMO Mixed Mode run is held to, each on one of the
//! simulator's existing sine structures, with the built-in law in the role
//! not under check (a law file in one role is checked with the built-in in
//! the other).
//!
//! Every case runs the floor at 1e-3 g^2/Hz from 20 Hz to 1800 Hz on 4 Hz
//! lines (16384 Hz, 4096-sample frames), coherence 0.5 between every pair of
//! controls, 8 frames a loop, the level from -6 dB to full level, and the
//! case's parts. A case is judged from the run's own status, read after
//! every chunk.
//!
//! # The laws' time budget
//!
//! A check run on a user's machine ([`run_mimo_mixed_check_case`]) times every
//! law call on the wall clock against the run's own budget, as a run on
//! hardware does: a tone law late at its engage, or late too often after it,
//! ends the case on a law fault. A test harness
//! ([`run_mimo_mixed_check_case_with`]) hands the run [`MimoMixedCheckClocks`]
//! instead, so its verdicts do not depend on how busy the machine is: the
//! simulator's time stands still while a law computes, and one late reading
//! from a loaded machine at a tone's engage ended a case with nothing wrong
//! in it.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal::Backend;
use hertzforge_mimo_mixed::band::BandStatement;
use hertzforge_mimo_mixed::config::{
    MimoDrive, MimoFloorPart, MimoMixedConfig, MimoMixedLawChoice, MimoNarrowbandPart, MimoPart,
    MimoPartBody, MimoTonePart, PerControl, DEFAULT_BAND_ABORT_DB, DEFAULT_BAND_ALARM_DB,
    DEFAULT_TONE_ABORT_DB, DEFAULT_TONE_ALARM_DB, DEFAULT_TRACKING_HZ,
};
use hertzforge_mimo_mixed::service::{
    MimoMixedError, MimoMixedLaws, MimoMixedPhase, MimoMixedService, MimoMixedStatus,
};
use hertzforge_mixed::{
    BandWidth, Direction, LawPick, PartSchedule, PartSweepTarget, PartWindow, SweepLaw,
    ToneAmplitude,
};
use hertzforge_random::service::config::LevelSchedule;
use hertzforge_random::RunMode;
use hertzforge_random_laws::CheckCase;
use hertzforge_reference::{Method, Provenance, Reference};
use hertzforge_sdm::law::Tolerances;
use hertzforge_sdm::{HermitianStack, C64};
use std::sync::Arc;

/// One case of the MIMO Mixed check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MimoMixedCheckCase {
    /// A floor alone.
    FloorOnly,
    /// Two dwelling tones on the floor.
    FixedTonesOnFloor,
    /// A tone swept through the structure's modes on the floor.
    SweptToneThroughMode,
    /// A narrowband dwelling over the floor (folded into its reference).
    NarrowbandFixed,
    /// A narrowband swept over the floor (servoed by its own law).
    NarrowbandSwept,
    /// The floor and a tone moved through a level ladder.
    LevelLadder,
    /// Three tones sharing drives whose peak ceiling binds.
    SharedCeiling,
    /// Three controls by six drives.
    MoreDrivesThanControls,
    /// A flat wire behind a hardware station's lag.
    TransportDelay,
    /// The floor and a tone with a monitor channel held under a limit.
    LimitChannel,
}

impl MimoMixedCheckCase {
    /// Every case, in the order the check runs them.
    pub const ALL: [MimoMixedCheckCase; 10] = [
        MimoMixedCheckCase::FloorOnly,
        MimoMixedCheckCase::FixedTonesOnFloor,
        MimoMixedCheckCase::SweptToneThroughMode,
        MimoMixedCheckCase::NarrowbandFixed,
        MimoMixedCheckCase::NarrowbandSwept,
        MimoMixedCheckCase::LevelLadder,
        MimoMixedCheckCase::SharedCeiling,
        MimoMixedCheckCase::MoreDrivesThanControls,
        MimoMixedCheckCase::TransportDelay,
        MimoMixedCheckCase::LimitChannel,
    ];

    /// The cases a law must pass, in the order the check runs them.
    pub const REQUIRED: [MimoMixedCheckCase; 9] = [
        MimoMixedCheckCase::FloorOnly,
        MimoMixedCheckCase::FixedTonesOnFloor,
        MimoMixedCheckCase::NarrowbandFixed,
        MimoMixedCheckCase::NarrowbandSwept,
        MimoMixedCheckCase::LevelLadder,
        MimoMixedCheckCase::SharedCeiling,
        MimoMixedCheckCase::MoreDrivesThanControls,
        MimoMixedCheckCase::TransportDelay,
        MimoMixedCheckCase::LimitChannel,
    ];

    /// The cases run and recorded for information only. A tone swept
    /// through a mode faster than the tracker follows is beyond the method
    /// (the built-in laws read about 3.6 dB under target there), so no law
    /// is held to it.
    pub const INFORMATIONAL: [MimoMixedCheckCase; 1] = [MimoMixedCheckCase::SweptToneThroughMode];

    /// Whether a law must pass this case.
    pub fn required(self) -> bool {
        !Self::INFORMATIONAL.contains(&self)
    }

    /// The stable record and wire name.
    pub fn name(self) -> &'static str {
        match self {
            MimoMixedCheckCase::FloorOnly => "floor_only",
            MimoMixedCheckCase::FixedTonesOnFloor => "fixed_tones_on_floor",
            MimoMixedCheckCase::SweptToneThroughMode => "swept_tone_through_mode",
            MimoMixedCheckCase::NarrowbandFixed => "narrowband_fixed",
            MimoMixedCheckCase::NarrowbandSwept => "narrowband_swept",
            MimoMixedCheckCase::LevelLadder => "level_ladder",
            MimoMixedCheckCase::SharedCeiling => "shared_ceiling",
            MimoMixedCheckCase::MoreDrivesThanControls => "more_drives_than_controls",
            MimoMixedCheckCase::TransportDelay => "transport_delay",
            MimoMixedCheckCase::LimitChannel => "limit_channel",
        }
    }

    /// The case with `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The simulated structure and parts, in words.
    pub fn plant(self) -> &'static str {
        match self {
            MimoMixedCheckCase::FloorOnly => "sine-linear-4x4; the floor alone",
            MimoMixedCheckCase::FixedTonesOnFloor => {
                "sine-linear-4x4; 1 g tones dwelling at 150 Hz and 700 Hz"
            }
            MimoMixedCheckCase::SweptToneThroughMode => {
                "sine-linear-4x4; a 1 g tone swept 200 Hz to 360 Hz at 2 Hz/s through its modes"
            }
            MimoMixedCheckCase::NarrowbandFixed => {
                "sine-linear-4x4; a 40 Hz band at 300 Hz adding 3e-3 g^2/Hz"
            }
            MimoMixedCheckCase::NarrowbandSwept => {
                "sine-linear-4x4; a 40 Hz band swept 800 Hz to 1000 Hz at 5 Hz/s adding 3e-3 g^2/Hz"
            }
            MimoMixedCheckCase::LevelLadder => {
                "sine-linear-4x4; a 1 g tone at 240 Hz through -9, -6, -3 and 0 dB"
            }
            MimoMixedCheckCase::SharedCeiling => {
                "sine-linear-4x4; 2 g tones at 150, 240 and 700 Hz, every drive's peak ceiling 1.4 V, common_scale"
            }
            MimoMixedCheckCase::MoreDrivesThanControls => {
                "sine-wide-3x6; a 1 g tone at 240 Hz"
            }
            MimoMixedCheckCase::TransportDelay => {
                "sine-loopback-4x4 behind 553 frames of lag; the floor at 1e-4 g^2/Hz and a 0.3 g tone at 240 Hz"
            }
            MimoMixedCheckCase::LimitChannel => {
                "sine-base-article-6x4; a 1 g tone at 150 Hz; monitor channel 5 a limit, its flat ceiling 0.2 g^2/Hz, abort 3 dB over it"
            }
        }
    }

    /// What the case asks of the laws, in words.
    pub fn requirement(self) -> &'static str {
        match self {
            MimoMixedCheckCase::SweptToneThroughMode => {
                "the run completes; from 5 s into full level every control's floor within 1 dB and the tone, whenever usable, within its abort band"
            }
            MimoMixedCheckCase::SharedCeiling => {
                "the run completes with no drive frame clamped; from 5 s into full level every control's floor within 1 dB, no drive's peak over its ceiling, every tone at one common scale, and every tone, whenever usable, within its alarm band of its target as the ceiling rule scales it"
            }
            MimoMixedCheckCase::LimitChannel => {
                "the run completes; from 5 s into full level every control's floor within 1 dB, the tone, whenever usable, within its alarm band, and the limit channel never over its ceiling"
            }
            MimoMixedCheckCase::NarrowbandFixed | MimoMixedCheckCase::NarrowbandSwept => {
                "the run completes; from 5 s into full level every control's floor within 1 dB; from 20 s every control's band level within its alarm band"
            }
            MimoMixedCheckCase::LevelLadder => {
                "the run completes through every rung; 3 s into each rung every control's floor within 1 dB and the tone, whenever usable, within its alarm band"
            }
            _ => {
                "the run completes; from 5 s into full level every control's floor within 1 dB and every tone, whenever usable, within its alarm band"
            }
        }
    }
}

/// The rate the check's stations run at, Hz (the sine structures'
/// calibration rate).
pub const MIMO_MIXED_CHECK_FS_HZ: f64 = 16_384.0;

/// The transport-delay case's lag, frames.
pub const MIMO_MIXED_CHECK_DELAY_FRAMES: usize = 553;

/// The simulated station a case runs on, from the case's own facts: the
/// caller builds it (this crate names no simulator).
#[derive(Debug, Clone, PartialEq)]
pub struct MimoMixedCheckStation {
    /// The simulator's sine structure, by its name (`sine-linear-4x4`, ...).
    pub structure: &'static str,
    /// Its outputs (controls first).
    pub n_outputs: usize,
    /// Its controls.
    pub n_controls: usize,
    /// Its drives.
    pub n_drives: usize,
    /// Rate, Hz.
    pub fs_hz: f64,
    /// Measurement noise, g rms.
    pub noise_rms_g: f64,
    /// Transport delay, frames.
    pub transport_delay_samples: usize,
    /// Seed of the station's noise.
    pub seed: u64,
}

/// The station `case` runs on.
pub fn mimo_mixed_check_station(case: MimoMixedCheckCase) -> MimoMixedCheckStation {
    let (structure, dims, delay) = match case {
        MimoMixedCheckCase::MoreDrivesThanControls => ("sine-wide-3x6", (3, 3, 6), 0),
        MimoMixedCheckCase::LimitChannel => ("sine-base-article-6x4", (6, 4, 4), 0),
        MimoMixedCheckCase::TransportDelay => (
            "sine-loopback-4x4",
            (4, 4, 4),
            MIMO_MIXED_CHECK_DELAY_FRAMES,
        ),
        _ => ("sine-linear-4x4", (4, 4, 4), 0),
    };
    MimoMixedCheckStation {
        structure,
        n_outputs: dims.0,
        n_controls: dims.1,
        n_drives: dims.2,
        fs_hz: MIMO_MIXED_CHECK_FS_HZ,
        noise_rms_g: 1e-4,
        transport_delay_samples: delay,
        seed: 7,
    }
}

/// The range of every acceleration channel of a check's station, g: a
/// common accelerometer range, about four times the largest peak a case
/// carries (the shared-ceiling case's three 2 g tones on its floor), so a
/// law that runs away meets the run's overload watch.
pub const MIMO_MIXED_CHECK_RANGE_G: f64 = 50.0;

/// The range of every drive channel of a check's station, volts.
pub const MIMO_MIXED_CHECK_RANGE_V: f64 = 10.0;

/// The limit-channel case's limit channel (the base article's first
/// monitor).
pub const MIMO_MIXED_CHECK_LIMIT_CHANNEL: u32 = 5;

/// The limit-channel case's flat ceiling, g^2/Hz: about 3 dB over what the
/// built-in pair puts on the channel's worst line (the tone's, 0.097 g^2/Hz
/// there), so a law that puts 3 dB more on it fails the case.
pub const MIMO_MIXED_CHECK_LIMIT_CEILING: f64 = 0.2;

/// The channel table `station` runs on: its outputs on channels 1 up
/// (controls first), its drives after them.
pub fn mimo_mixed_check_table(station: &MimoMixedCheckStation) -> ChannelTable {
    let mut t = ChannelTable::new();
    for k in 0..station.n_outputs + station.n_drives {
        let mut c = Channel::new(k as u32 + 1);
        if k < station.n_controls {
            c.role = ChannelRole::Control;
            c.units = Units::AccelG;
            c.range_max = MIMO_MIXED_CHECK_RANGE_G;
        } else if k < station.n_outputs {
            c.role = ChannelRole::Monitor;
            c.units = Units::AccelG;
            c.range_max = MIMO_MIXED_CHECK_RANGE_G;
        } else {
            c.role = ChannelRole::Drive;
            c.units = Units::VoltageV;
            c.range_max = MIMO_MIXED_CHECK_RANGE_V;
        }
        t.channels.push(c);
    }
    t
}

const DF_HZ: f64 = 4.0;
const LINES: usize = 2049;
/// 20 Hz to 1800 Hz.
const BAND: (usize, usize) = (5, 451);
const GAMMA: f64 = 0.5;

fn reference(n: usize, level: f64) -> Reference {
    let mut sdm = HermitianStack::zeros(LINES, n);
    for l in BAND.0..BAND.1 {
        for r in 0..n {
            for c in r..n {
                let g = if r == c { 1.0 } else { GAMMA };
                sdm.set(l, r, c, C64::new(g * level, 0.0));
            }
        }
    }
    Reference {
        sdm,
        df_hz: DF_HZ,
        band: BAND,
        labels: (0..n).map(|i| format!("c{i}")).collect(),
        unit: "g".into(),
        tolerances: Tolerances::none(LINES, n),
        response_transformation: None,
        drive_transformation: None,
        provenance: Provenance::new(Method::Breakpoints, "mimo-mixed check"),
    }
}

fn part(id: &str, body: MimoPartBody) -> MimoPart {
    MimoPart {
        id: id.into(),
        name: String::new(),
        enabled: true,
        window: PartWindow::default(),
        body,
    }
}

fn tone(id: &str, schedule: PartSchedule, g_pk: f64) -> MimoPart {
    part(
        id,
        MimoPartBody::Tone(MimoTonePart {
            schedule,
            amplitude: PerControl::All(ToneAmplitude::Constant { g_pk }),
            phases_stated: false,
            phase_deg: Vec::new(),
            alarm_db: PerControl::All(DEFAULT_TONE_ALARM_DB),
            abort_db: PerControl::All(DEFAULT_TONE_ABORT_DB),
        }),
    )
}

fn band(id: &str, schedule: PartSchedule) -> MimoPart {
    part(
        id,
        MimoPartBody::Narrowband(MimoNarrowbandPart {
            schedule,
            width: BandWidth::Hz { hz: 40.0 },
            statement: BandStatement::Added {
                level_g2_hz: PerControl::All(3e-3),
                coherence: GAMMA,
            },
            alarm_db: PerControl::All(DEFAULT_BAND_ALARM_DB),
            abort_db: PerControl::All(DEFAULT_BAND_ABORT_DB),
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

fn sweep(lo: f64, hi: f64, hz_per_s: f64) -> PartSchedule {
    PartSchedule::Sweep {
        lo_hz: lo,
        hi_hz: hi,
        start_hz: lo,
        direction: Direction::Up,
        law: SweepLaw::LinearHzPerS,
        rate: hz_per_s,
        target: PartSweepTarget::Duration { s: 1e4 },
    }
}

/// How long `case` holds full level, seconds.
pub fn mimo_mixed_check_run_s(case: MimoMixedCheckCase) -> f64 {
    match case {
        MimoMixedCheckCase::SweptToneThroughMode => 80.0,
        MimoMixedCheckCase::NarrowbandFixed | MimoMixedCheckCase::NarrowbandSwept => 30.0,
        MimoMixedCheckCase::SharedCeiling => 20.0,
        MimoMixedCheckCase::LevelLadder => 6.0,
        _ => 15.0,
    }
}

/// The configuration `case` runs, with the built-in laws named (the role
/// under check takes its law from the factories handed to
/// [`run_mimo_mixed_check_case`]).
pub fn mimo_mixed_check_config(case: MimoMixedCheckCase) -> MimoMixedConfig {
    let st = mimo_mixed_check_station(case);
    let n = st.n_controls;
    let floor_level = if case == MimoMixedCheckCase::TransportDelay {
        1e-4
    } else {
        1e-3
    };
    let mut parts = vec![part("floor", MimoPartBody::Floor(MimoFloorPart::default()))];
    match case {
        MimoMixedCheckCase::FloorOnly => {}
        MimoMixedCheckCase::FixedTonesOnFloor => {
            parts.push(tone("t1", fixed(150.0), 1.0));
            parts.push(tone("t2", fixed(700.0), 1.0));
        }
        MimoMixedCheckCase::SweptToneThroughMode => {
            parts.push(tone("t1", sweep(200.0, 360.0, 2.0), 1.0))
        }
        MimoMixedCheckCase::NarrowbandFixed => parts.push(band("b1", fixed(300.0))),
        MimoMixedCheckCase::NarrowbandSwept => parts.push(band("b1", sweep(800.0, 1000.0, 5.0))),
        MimoMixedCheckCase::LevelLadder | MimoMixedCheckCase::MoreDrivesThanControls => {
            parts.push(tone("t1", fixed(240.0), 1.0))
        }
        MimoMixedCheckCase::LimitChannel => parts.push(tone("t1", fixed(150.0), 1.0)),
        MimoMixedCheckCase::SharedCeiling => {
            parts.push(tone("t1", fixed(150.0), 2.0));
            parts.push(tone("t2", fixed(240.0), 2.0));
            parts.push(tone("t3", fixed(700.0), 2.0));
        }
        MimoMixedCheckCase::TransportDelay => parts.push(tone("t1", fixed(240.0), 0.3)),
    }
    let mut drives: Vec<MimoDrive> = (0..st.n_drives as u32)
        .map(|j| MimoDrive::new(st.n_outputs as u32 + 1 + j, 2.0))
        .collect();
    let mut law = MimoMixedLawChoice::default();
    if case == MimoMixedCheckCase::SharedCeiling {
        for d in drives.iter_mut() {
            d.peak_v = 1.4;
        }
        law.ceiling_rule = LawPick::new("common_scale");
    }
    let ladder = case == MimoMixedCheckCase::LevelLadder;
    MimoMixedConfig {
        estimation: hertzforge_mimo_random::config::Estimation {
            frames_per_loop: 8,
            ..Default::default()
        },
        controls: (1..=n as u32).collect(),
        drives,
        limits: if case == MimoMixedCheckCase::LimitChannel {
            vec![hertzforge_mimo_random::config::LimitSpec {
                channel_index: MIMO_MIXED_CHECK_LIMIT_CHANNEL,
                ceiling_psd: vec![MIMO_MIXED_CHECK_LIMIT_CEILING],
                abort_margin_db: 3.0,
            }]
        } else {
            Vec::new()
        },
        reference: reference(n, floor_level),
        parts,
        // Two 4 Hz lines (the least the exclusion may cover).
        exclusion_hz: 2.0 * DF_HZ,
        tracking_hz: DEFAULT_TRACKING_HZ,
        schedule: LevelSchedule {
            mode: RunMode::Auto,
            start_db: if ladder { -9.0 } else { -6.0 },
            table_db: if ladder {
                vec![-9.0, -6.0, -3.0, 0.0]
            } else {
                vec![-6.0, -3.0, 0.0]
            },
            dwell_s: vec![if ladder { 6.0 } else { 2.0 }],
        },
        full_level_duration_s: Some(mimo_mixed_check_run_s(case)),
        level_ramp_s: 0.5,
        shutdown_ramp_s: 0.5,
        part_ramp_s: 0.5,
        floor_settle_s: 30.0,
        soft_start_db: 3.0,
        soft_start_relax_db: 6.0,
        identification: None,
        gates: Default::default(),
        caps: Default::default(),
        protections: Default::default(),
        frf_mode: hertzforge_mimo_random::config::FrfMode::Hold,
        crest_factor: 3.0,
        drive_clip_sigma: None,
        article_mass_lb: 0.0,
        capture: Default::default(),
        law,
        control_tick_s: None,
        metadata: Vec::new(),
        seed: 1,
    }
}

/// The worst reading seen of one quantity, by control.
#[derive(Default)]
struct Worst {
    readings: usize,
    worst: Vec<f64>,
    outside: usize,
}

impl Worst {
    fn see(&mut self, errs: &[f64], within: impl Fn(f64) -> bool) {
        if self.worst.len() < errs.len() {
            self.worst.resize(errs.len(), 0.0);
        }
        self.readings += 1;
        let mut out = false;
        for (w, &e) in self.worst.iter_mut().zip(errs) {
            if !e.is_finite() || !within(e) {
                out = true;
            }
            if !e.is_finite() || e.abs() > w.abs() {
                *w = e;
            }
        }
        self.outside += out as usize;
    }

    fn ok(&self) -> bool {
        self.readings > 0 && self.outside == 0
    }

    fn words(&self, what: &str) -> String {
        format!(
            "{what}: worst {:.2?} dB over {} readings, {} outside",
            self.worst, self.readings, self.outside
        )
    }
}

fn floor_within(e: f64) -> bool {
    e.abs() <= 1.0
}

fn band_within(a: (f64, f64)) -> impl Fn(f64) -> bool {
    move |e| e >= -a.0 && e <= a.1
}

/// Run `case` with `laws` on `backend` (the station the caller built from
/// [`mimo_mixed_check_station`], on the table [`mimo_mixed_check_table`])
/// and judge it against its requirement. A run that aborts fails the case,
/// naming its end.
pub fn run_mimo_mixed_check_case(
    case: MimoMixedCheckCase,
    laws: &MimoMixedLaws,
    backend: Box<dyn Backend>,
) -> Result<CheckCase, MimoMixedError> {
    run_mimo_mixed_check_case_with(case, laws, backend, None)
}

/// The clocks a check run's law calls are timed on in place of the wall
/// clock: a test harness's, never a user's check.
#[derive(Clone)]
pub struct MimoMixedCheckClocks {
    /// The floor law's.
    pub floor: Arc<dyn hertzforge_random::LawClock>,
    /// The tone laws'.
    pub tones: Arc<dyn hertzforge_sine::law::LawClock>,
}

/// As [`run_mimo_mixed_check_case`], the laws' calls timed on `clocks` when
/// given (the wall clock when `None`). The budget itself is the run's own
/// either way.
pub fn run_mimo_mixed_check_case_with(
    case: MimoMixedCheckCase,
    laws: &MimoMixedLaws,
    backend: Box<dyn Backend>,
    clocks: Option<&MimoMixedCheckClocks>,
) -> Result<CheckCase, MimoMixedError> {
    let st = mimo_mixed_check_station(case);
    let cfg = mimo_mixed_check_config(case);
    let mut svc = MimoMixedService::new(
        cfg,
        mimo_mixed_check_table(&st),
        backend,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        laws.clone(),
    )?;
    if let Some(c) = clocks {
        svc = svc.with_law_clocks(c.floor.clone(), c.tones.clone());
    }
    svc.arm()?;
    svc.identify()?;
    if !svc.coherence_gate()? {
        return Ok(verdict(
            case,
            false,
            "the identification's coherence gate failed".into(),
        ));
    }
    svc.proceed()?;
    let tone_abort = band_within(DEFAULT_TONE_ABORT_DB);
    let tone_alarm = band_within(DEFAULT_TONE_ALARM_DB);
    let band_alarm = band_within(DEFAULT_BAND_ALARM_DB);
    let mut floor = Worst::default();
    let mut tones: Vec<Worst> = Vec::new();
    let mut bands: Vec<Worst> = Vec::new();
    let mut full_t0: Option<f64> = None;
    let mut rung: (f64, f64) = (f64::NAN, 0.0);
    let mut rungs_seen: Vec<f64> = Vec::new();
    let ladder = case == MimoMixedCheckCase::LevelLadder;
    let shared = case == MimoMixedCheckCase::SharedCeiling;
    let peak_ceiling: Vec<f64> = mimo_mixed_check_config(case)
        .drives
        .iter()
        .map(|d| d.peak_v)
        .collect();
    // The limit channels' worst reading over their ceilings, dB.
    let mut limit_over = (f64::NEG_INFINITY, 0.0);
    // The shared ceiling: readings where the rule held a tone, the scales
    // seen (least, most), the widest spread of scale between tones in one
    // reading, and the largest peak played over each drive's ceiling.
    let mut held = 0usize;
    let mut scales = (f64::INFINITY, f64::NEG_INFINITY);
    let mut spread = 0.0f64;
    let mut peak_over = f64::NEG_INFINITY;
    let mut scaled: Vec<Worst> = Vec::new();
    while svc.run_chunk()? {
        let s: MimoMixedStatus = svc.status();
        if (s.level_db - rung.0).abs() > 0.01 {
            rung = (s.level_db, s.t_s);
        }
        let judge_now = if ladder {
            matches!(
                s.phase,
                MimoMixedPhase::Schedule | MimoMixedPhase::FullLevel
            ) && s.t_s - rung.1 >= 3.0
        } else if s.phase == MimoMixedPhase::FullLevel {
            let t0 = *full_t0.get_or_insert(s.t_s);
            s.t_s - t0 >= 5.0
        } else {
            false
        };
        if !judge_now {
            continue;
        }
        if ladder && !rungs_seen.iter().any(|l| (l - s.level_db).abs() < 0.01) {
            rungs_seen.push(s.level_db);
        }
        floor.see(&s.floor_error_db, floor_within);
        for l in &s.limits {
            if l.over_db > limit_over.0 {
                limit_over = (l.over_db, l.at_hz);
            }
        }
        if shared {
            for (d, c) in s.drives.iter().zip(&peak_ceiling) {
                peak_over = peak_over.max(d.peak_v - c);
            }
            let (lo, hi) = s
                .tones
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), t| {
                    (lo.min(t.scale), hi.max(t.scale))
                });
            held += s.tones.iter().any(|t| t.limiting || t.scale < 1.0) as usize;
            scales = (scales.0.min(lo), scales.1.max(hi));
            spread = spread.max(hi - lo);
            scaled.resize_with(s.tones.len(), Worst::default);
            for (w, t) in scaled.iter_mut().zip(&s.tones) {
                if t.usable {
                    let e: Vec<f64> = t
                        .error_db
                        .iter()
                        .map(|e| e - 20.0 * t.scale.log10())
                        .collect();
                    w.see(&e, &tone_alarm);
                }
            }
        }
        tones.resize_with(s.tones.len(), Worst::default);
        for (w, t) in tones.iter_mut().zip(&s.tones) {
            if !t.usable {
                continue;
            }
            if case == MimoMixedCheckCase::SweptToneThroughMode {
                w.see(&t.error_db, &tone_abort);
            } else {
                w.see(&t.error_db, &tone_alarm);
            }
        }
        let since_full = full_t0.map_or(0.0, |t0| s.t_s - t0);
        bands.resize_with(s.bands.len(), Worst::default);
        if since_full >= 20.0 {
            for (w, b) in bands.iter_mut().zip(&s.bands) {
                w.see(&b.error_db, &band_alarm);
            }
        }
    }
    let s = svc.status();
    let end = svc.end().map(|e| (e.name, e.words.clone()));
    let mut ok = matches!(&end, Some((name, _)) if *name == "completed");
    let mut detail = match &end {
        Some((name, words)) => format!("ended {name} ({words})"),
        None => "no end recorded".into(),
    };
    ok &= floor.ok();
    detail.push_str(&format!("; {}", floor.words("floor")));
    if case != MimoMixedCheckCase::SharedCeiling {
        for (w, t) in tones.iter().zip(&s.tones) {
            ok &= w.ok();
            detail.push_str(&format!("; {}", w.words(&format!("tone {}", t.id))));
        }
    } else {
        // The rule is judged on what it did, not only on the run not
        // aborting: one scale for every tone, every tone at its scaled
        // target, no peak over a ceiling and nothing clamped.
        ok &= s.clamped == 0;
        detail.push_str(&format!("; {} drive frames clamped", s.clamped));
        for (w, t) in scaled.iter().zip(&s.tones) {
            ok &= w.ok();
            detail.push_str(&format!(
                "; {}",
                w.words(&format!("tone {} of its scaled target", t.id))
            ));
        }
        ok &= scaled.len() == s.tones.len() && spread <= 1e-9 && peak_over <= 0.0;
        detail.push_str(&format!(
            "; scales {:.4} to {:.4}, spread {spread:.2e} between tones, held in {held} readings; largest peak {peak_over:+.3} V over its ceiling",
            scales.0, scales.1
        ));
    }
    if case == MimoMixedCheckCase::LimitChannel {
        ok &= limit_over.0 <= 0.0;
        detail.push_str(&format!(
            "; limit channel worst {:+.2} dB over its ceiling, at {:.0} Hz",
            limit_over.0, limit_over.1
        ));
    }
    for (w, b) in bands.iter().zip(&s.bands) {
        ok &= w.ok();
        detail.push_str(&format!("; {}", w.words(&format!("band {}", b.id))));
    }
    if ladder {
        let climbed = [-9.0, -6.0, -3.0, 0.0]
            .iter()
            .all(|r: &f64| rungs_seen.iter().any(|l| (l - r).abs() < 0.01));
        ok &= climbed;
        detail.push_str(&format!("; rungs judged {rungs_seen:.1?} dB"));
    }
    Ok(verdict(case, ok, detail))
}

fn verdict(case: MimoMixedCheckCase, passed: bool, detail: String) -> CheckCase {
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
    fn mimo_mixed_ten_cases_with_unique_names_that_read_back() {
        let mut names: Vec<_> = MimoMixedCheckCase::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), 10);
        for c in MimoMixedCheckCase::ALL {
            assert_eq!(MimoMixedCheckCase::from_name(c.name()), Some(c));
            assert!(!c.plant().is_empty() && !c.requirement().is_empty());
        }
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 10);
        assert_eq!(MimoMixedCheckCase::from_name("nope"), None);
    }

    #[test]
    fn mimo_mixed_every_case_is_required_or_informational_in_run_order() {
        let required: Vec<_> = MimoMixedCheckCase::ALL
            .into_iter()
            .filter(|c| c.required())
            .collect();
        assert_eq!(required, MimoMixedCheckCase::REQUIRED);
        let informational: Vec<_> = MimoMixedCheckCase::ALL
            .into_iter()
            .filter(|c| !c.required())
            .collect();
        assert_eq!(informational, MimoMixedCheckCase::INFORMATIONAL);
        assert!(!MimoMixedCheckCase::SweptToneThroughMode.required());
    }

    #[test]
    fn mimo_mixed_every_case_states_a_valid_configuration() {
        for c in MimoMixedCheckCase::ALL {
            mimo_mixed_check_config(c)
                .validate(Some(MIMO_MIXED_CHECK_FS_HZ))
                .unwrap_or_else(|e| panic!("{}: {e}", c.name()));
        }
    }
}
