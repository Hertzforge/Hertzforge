//! The SRS shock control laws an operator can choose, and the check a law file
//! passes before it drives hardware.
//!
//! - [`resolve_srs`] turns the operator's [`LawChoice`] into the
//!   [`SrsLawFactory`] the SRS service builds its law with
//!   (`SrsShockService::with_law_factory`): the built-in `srs_ratio`
//!   ([`DEFAULT_SRS_LAW_ID`]) or a `shock-srs` law file from the catalog. A law
//!   of another world is refused ([`ChoiceError::WrongTestType`]).
//! - [`SrsCheckCase`] is the check: the fixed set of simulated test articles,
//!   what a run on each is measured by ([`SrsCaseMeasure`]) and the rules that
//!   make the measure a passed or failed case ([`judge_srs`]).
//!   [`run_srs_check_case`] runs one case under the core's guards on the
//!   backend it is handed; [`srs_check_config`] is the run each case is.
//!
//! Every case is a whole run under the core's guards on a simulated station: a
//! flat [`SRS_CHECK_TARGET_G`] g target on a 1/3-octave grid from 100 to
//! 2000 Hz, Q 10, synthesized from odd-half-cycle wavelets; AUTO -12, -6 and
//! 0 dB with 2, 2 and then the case's pulses at full level, advancing on a
//! passing pulse, at most [`SRS_CHECK_MAX_PULSES_PER_LEVEL`] pulses a level,
//! the abort band [`SRS_CHECK_ABORT_DB`] either way; the drive ceiling
//! [`SRS_CHECK_DRIVE_CEILING_V`] and abort [`SRS_CHECK_DRIVE_ABORT_V`]; the
//! identification in blocks of [`SRS_CHECK_SYSID_BLOCK_S`], then proceeding
//! past the review hold. The spacing is the rate model's, on a virtual clock:
//! a simulated station needs no cooling. Every case asks the same of the law
//! first: every call answered, in time, with zero misses and zero refused
//! outputs. Then each case asks for its [`SrsRequirement`].
//!
//! [`LawChoice`]: hertzforge_random_laws::LawChoice
//! [`ChoiceError::WrongTestType`]: hertzforge_random_laws::ChoiceError::WrongTestType

use crate::check::CheckRunError;
use crate::srs_wasm::WasmSrsLawFactory;
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::Backend;
use hertzforge_law_host::{validate, LawError, ParamValue};
use hertzforge_random_laws::{
    srs_builtin_manifest, CheckCase, CheckRecord, ChoiceError, LawCatalog, LawChoice, LawWorld,
};
use hertzforge_shock_common::{LadderAdvance, ShockRunMode};
use hertzforge_shock_law_math::srs::SrsRatioParams;
use hertzforge_srs::iterate::{synthesize_to_target, SynthesisOpts};
use hertzforge_srs::service::config::{FrfSpec, LimitChannelSpec, LimitSet, LimitTarget};
use hertzforge_srs::service::pacer::VirtualPacer;
use hertzforge_srs::service::report::{RunState, TripKind};
use hertzforge_srs::{
    FirePath, HalPulsePlant, SrsControl, SrsLawFactory, SrsRatioFactory, SrsShockService,
    SrsTestConfig,
};
use std::fmt;
use std::sync::Arc;

/// The catalog id of the built-in SRS law, `srs_ratio`: the default.
pub const DEFAULT_SRS_LAW_ID: &str = hertzforge_shock_law_math::srs::ID;

/// The version of the SRS check written into its records. A change to the
/// plant set or to the rules changes it.
pub const SRS_CHECK_VERSION: &str = "shock-srs-check 1";

/// The check target, g, flat over the band grid.
pub const SRS_CHECK_TARGET_G: f64 = 1.0;
/// The check runs' drive ceiling, volts peak.
pub const SRS_CHECK_DRIVE_CEILING_V: f64 = 10.0;
/// The check runs' drive abort, volts peak.
pub const SRS_CHECK_DRIVE_ABORT_V: f64 = 20.0;
/// The limit case's flat bound, g: 2.5 times the target.
pub const SRS_CHECK_LIMIT_BOUND_G: f64 = 2.5 * SRS_CHECK_TARGET_G;
/// The limit case's headroom, dB.
pub const SRS_CHECK_LIMIT_HEADROOM_DB: f64 = 1.0;
/// The schedule's levels, dB.
pub const SRS_CHECK_LEVELS_DB: [f64; 3] = [-12.0, -6.0, 0.0];
/// Pulses a level may take without a pass before the run ends not converged.
/// More than a test's usual 5: on a sharp mode the built-in law's first pulse
/// is several dB hot at the mode's band (the seed divides each component by
/// the gain at its own frequency, and its neighbours' tails ring the mode), and
/// the band comes down about 1 dB a pulse.
pub const SRS_CHECK_MAX_PULSES_PER_LEVEL: u32 = 12;
/// The identification's block, seconds at most: about one burst and its
/// ring-out.
pub const SRS_CHECK_SYSID_BLOCK_S: f64 = 0.2;
/// The check's envelope abort band, dB either way. The alarm band stays the
/// test's ±3 dB (the pass); the abort is wide because the article is
/// simulated and the check judges the law's convergence, not the first
/// pulse.
pub const SRS_CHECK_ABORT_DB: f64 = 12.0;

/// The built-in law's factory: `srs_ratio` natively, with its parameter values.
pub type BuiltinSrsFactory = SrsRatioFactory;

/// The default law with its default parameters.
pub fn default_srs_choice() -> LawChoice {
    LawChoice::new(DEFAULT_SRS_LAW_ID, Vec::new())
}

/// Resolve `choice` against `catalog`: check its parameter values against the
/// law's manifest and return the factory an SRS run builds its law with: the
/// native built-in for `srs_ratio`, a [`WasmSrsLawFactory`] for a `shock-srs`
/// law file.
///
/// Refusals: [`ChoiceError::UnknownLaw`] for an id the catalog does not hold,
/// [`ChoiceError::WrongTestType`] for a law of another world,
/// [`ChoiceError::BadParam`] for a value its declaration refuses.
pub fn resolve_srs(
    catalog: &LawCatalog,
    choice: &LawChoice,
) -> Result<Arc<dyn SrsLawFactory>, ChoiceError> {
    let bad = |e: LawError| match e {
        LawError::ParamInvalid { key, reason } => ChoiceError::BadParam { key, reason },
        other => ChoiceError::BadConfig(other.to_string()),
    };
    if choice.id == DEFAULT_SRS_LAW_ID {
        let params =
            validate::resolve_params(&srs_builtin_manifest(), &choice.params, 1).map_err(bad)?;
        let mut p = SrsRatioParams::default();
        for param in &params {
            if let ParamValue::Number(v) = param.value {
                p.set(&param.key, v)
                    .map_err(|reason| ChoiceError::BadParam {
                        key: param.key.clone(),
                        reason,
                    })?;
            }
        }
        return Ok(Arc::new(BuiltinSrsFactory::new(p)));
    }
    match catalog.entry(&choice.id) {
        None => Err(ChoiceError::UnknownLaw(choice.id.clone())),
        Some(entry) => match entry.world {
            LawWorld::ShockSrs => {
                let law = catalog
                    .srs_law(&choice.id)
                    .ok_or_else(|| ChoiceError::UnknownLaw(choice.id.clone()))?;
                let params = law.resolve_params(&choice.params).map_err(bad)?;
                Ok(Arc::new(WasmSrsLawFactory::new(law.clone(), params)))
            }
            LawWorld::Single
            | LawWorld::Mimo
            | LawWorld::ShockClassical
            | LawWorld::Sine
            | LawWorld::Twr
            | LawWorld::ShockMimo => Err(ChoiceError::WrongTestType {
                id: choice.id.clone(),
                law_is_for: entry.world.test_type(),
            }),
        },
    }
}

/// What a case asks of the law beyond answering every call, in time, with
/// usable gains.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SrsRequirement {
    /// Every band within `db` of the target by full-level pulse `by_pulse`
    /// (counted from 1), and on every full-level pulse after it.
    WithinBy {
        /// The band, dB either way.
        db: f64,
        /// The full-level pulse by which every band must be within it.
        by_pulse: u32,
    },
    /// At every level of the schedule, every band within `db` of the target
    /// at that level after `after_pulses` pulses there.
    EachLevelWithin {
        /// The band, dB either way.
        db: f64,
        /// Pulses at a level before it is judged.
        after_pulses: u32,
    },
    /// The predicted limit-channel SRS at most `over_bound_db` over its bound
    /// on every pulse, and the control SRS within `db` of the target outside
    /// the notched bands.
    LimitHeld {
        /// How far over the bound a prediction may be, dB.
        over_bound_db: f64,
        /// The control band outside the notched bands, dB either way.
        db: f64,
    },
}

/// The simulated test article a case runs on. The controller builds a
/// one-drive, one-control simulated station from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SrsCheckPlant {
    /// A wire: feedthrough 1, no modes.
    Wire,
    /// One mode.
    OneMode {
        /// Natural frequency, Hz.
        f_hz: f64,
        /// Quality factor.
        q: f64,
    },
    /// Two close modes.
    TwoModes {
        /// The modes' natural frequencies, Hz.
        f_hz: [f64; 2],
        /// Their quality factor.
        q: f64,
    },
}

impl fmt::Display for SrsCheckPlant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SrsCheckPlant::Wire => f.write_str("a wire"),
            SrsCheckPlant::OneMode { f_hz, q } => write!(f, "one mode, {f_hz} Hz, Q {q}"),
            SrsCheckPlant::TwoModes { f_hz, q } => {
                write!(f, "two modes, {} and {} Hz, Q {q}", f_hz[0], f_hz[1])
            }
        }
    }
}

/// One simulated test article or condition of the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SrsCheckCase {
    /// One mode at 400 Hz, Q 10.
    OneMode,
    /// Two close modes at 240 and 260 Hz, Q 30.
    CloseModes,
    /// One mode with measurement noise 30 dB under the identification
    /// burst's response.
    Noise,
    /// A wire on a ladder of -12, -6 and 0 dB, with manual moves.
    Ladder,
    /// One mode, with the identified gain scaled +3 dB before the seed.
    IdentificationError,
    /// A wire for control, and one limit channel with a user-supplied FRF that
    /// resonates at 500 Hz under a flat bound.
    Limit,
}

impl SrsCheckCase {
    /// Every case, in the order the check runs them.
    pub const ALL: [SrsCheckCase; 6] = [
        SrsCheckCase::OneMode,
        SrsCheckCase::CloseModes,
        SrsCheckCase::Noise,
        SrsCheckCase::Ladder,
        SrsCheckCase::IdentificationError,
        SrsCheckCase::Limit,
    ];

    /// The case's name in a record and on screen.
    pub fn name(self) -> &'static str {
        match self {
            SrsCheckCase::OneMode => "one mode",
            SrsCheckCase::CloseModes => "close modes",
            SrsCheckCase::Noise => "measurement noise",
            SrsCheckCase::Ladder => "level ladder",
            SrsCheckCase::IdentificationError => "identification error",
            SrsCheckCase::Limit => "limit channel",
        }
    }

    /// The simulated test article.
    pub fn plant(self) -> SrsCheckPlant {
        match self {
            SrsCheckCase::CloseModes => SrsCheckPlant::TwoModes {
                f_hz: [240.0, 260.0],
                q: 30.0,
            },
            SrsCheckCase::Limit | SrsCheckCase::Ladder => SrsCheckPlant::Wire,
            _ => SrsCheckPlant::OneMode {
                f_hz: 400.0,
                q: 10.0,
            },
        }
    }

    /// The factor, dB, the core scales its identified gain by before the seed
    /// (`None`: the identification as measured).
    pub fn identification_scale_db(self) -> Option<f64> {
        match self {
            SrsCheckCase::IdentificationError => Some(3.0),
            _ => None,
        }
    }

    /// Measurement noise, dB under the response (`None`: the simulator's
    /// floor only).
    pub fn noise_below_db(self) -> Option<f64> {
        match self {
            SrsCheckCase::Noise => Some(30.0),
            _ => None,
        }
    }

    /// The resonance of the limit channel's user-supplied FRF, Hz (`None`: no
    /// limit channel).
    pub fn limit_resonance_hz(self) -> Option<f64> {
        match self {
            SrsCheckCase::Limit => Some(500.0),
            _ => None,
        }
    }

    /// Pulses at full level: the case's `by_pulse`, or 3.
    pub fn full_level_pulses(self) -> u32 {
        match self.requirement() {
            SrsRequirement::WithinBy { by_pulse, .. } => by_pulse,
            _ => 3,
        }
    }

    /// The measurement noise the simulated station adds, RMS EU, at the
    /// station's rate `fs_hz` (`None`: no noise). It is
    /// [`Self::noise_below_db`] under the peak of the run's quietest pulse,
    /// the identification burst: the check's synthesized pulse at the
    /// identification level.
    pub fn noise_rms_g(self, fs_hz: f64) -> Option<f64> {
        let db = self.noise_below_db()?;
        let cfg = srs_check_config(self, fs_hz);
        let level_db = cfg.sysid.map_or(SRS_CHECK_LEVELS_DB[0], |s| s.level_db);
        let seed = cfg.seed_table().ok()?;
        let fit = synthesize_to_target(
            &cfg.target_on_grid().ok()?,
            &cfg.band_freqs(),
            fs_hz,
            seed,
            cfg.synthesis,
        );
        let pk = fit.waveform.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        Some(pk * 10f64.powf((level_db - db) / 20.0))
    }

    /// What the case asks of the law.
    pub fn requirement(self) -> SrsRequirement {
        match self {
            SrsCheckCase::Ladder => SrsRequirement::EachLevelWithin {
                db: 3.0,
                after_pulses: 3,
            },
            SrsCheckCase::IdentificationError => SrsRequirement::WithinBy {
                db: 3.0,
                by_pulse: 8,
            },
            SrsCheckCase::Limit => SrsRequirement::LimitHeld {
                over_bound_db: 0.5,
                db: 3.0,
            },
            SrsCheckCase::OneMode | SrsCheckCase::CloseModes | SrsCheckCase::Noise => {
                SrsRequirement::WithinBy {
                    db: 3.0,
                    by_pulse: 6,
                }
            }
        }
    }
}

/// How a case's run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum SrsCaseEnd {
    /// The schedule completed (or, for the ladder case, its moves did).
    Completed,
    /// The core's iteration limit ended it.
    NotConverged,
    /// A protection aborted it; the trip kind's wire name (`envelope`,
    /// `drive_vpk`, `kinematic`, ...).
    Aborted(String),
    /// The law's misses ended it; the reason's wire name.
    LawFault(String),
    /// Identification held the run (coherence).
    Held,
    /// The run failed; the controller's words.
    Fault(String),
}

/// What one case's run is judged by.
#[derive(Debug, Clone, PartialEq)]
pub struct SrsCaseMeasure {
    /// How the run ended.
    pub end: SrsCaseEnd,
    /// Pulses fired.
    pub pulses: u32,
    /// Pulses fired at full level.
    pub full_level_pulses: u32,
    /// The first full-level pulse, counted from 1, with every band within the
    /// case's band; `None` when none was.
    pub first_within_pulse: Option<u32>,
    /// Whether every full-level pulse after the first within was within too.
    pub within_after_first: bool,
    /// The worst band's error on the last pulse, dB, signed.
    pub final_worst_band_db: Option<f64>,
    /// The worst band's error on the last pulse outside the bands the limit
    /// channels hold down (`SrsShockService::limit_held_bands`), dB, signed;
    /// `None` when nothing fired.
    pub final_worst_outside_notch_db: Option<f64>,
    /// Per level of the schedule, in order: the level, dB, and the worst
    /// band's error on the case's judged pulse there (the third for the ladder
    /// case, the last otherwise), dB (`None` when the level was not reached).
    pub levels: Vec<(f64, Option<f64>)>,
    /// The largest predicted limit-channel SRS over its bound on any pulse,
    /// dB; `None` without a limit channel.
    pub worst_limit_over_db: Option<f64>,
    /// Law calls made, the seed included.
    pub calls: u32,
    /// Calls whose output was not used.
    pub misses: u32,
    /// Calls whose output the core refused (a subset of `misses`).
    pub refused: u32,
    /// Component gains the core's limit cap held back, over the run.
    pub limited_components: u64,
    /// The slowest call, ms; `None` when no call was made.
    pub worst_call_ms: Option<f64>,
    /// One call's budget, ms.
    pub budget_ms: f64,
    /// The largest drive commanded, volts peak.
    pub max_drive_vpk: f64,
    /// The drive ceiling, volts peak.
    pub drive_ceiling_vpk: f64,
}

/// The limit case's supplied drive-to-channel gain: 1, rising to 4 at
/// 500 Hz (`1 + 3 / (1 + ((f - 500) / 100)^2)`), every 25 Hz to `fs_hz / 2`.
pub fn srs_check_limit_frf(fs_hz: f64) -> FrfSpec {
    let n = (fs_hz / 2.0 / 25.0).floor() as usize;
    let freqs_hz: Vec<f64> = (0..=n).map(|i| i as f64 * 25.0).collect();
    let mag = freqs_hz
        .iter()
        .map(|&f| 1.0 + 3.0 / (1.0 + ((f - 500.0) / 100.0).powi(2)))
        .collect();
    FrfSpec { freqs_hz, mag }
}

/// The run a case is, at the station's rate `fs_hz` (see the module docs).
pub fn srs_check_config(case: SrsCheckCase, fs_hz: f64) -> SrsTestConfig {
    let mut cfg = SrsTestConfig::flat(100.0, 2000.0, SRS_CHECK_TARGET_G);
    cfg.synthesis = SynthesisOpts {
        max_iters: 120,
        tol_db: 1.5,
        max_gain_db_per_iter: 6.0,
        q: 10.0,
    };
    cfg.schedule.table_db = SRS_CHECK_LEVELS_DB.to_vec();
    cfg.schedule.start_db = SRS_CHECK_LEVELS_DB[0];
    cfg.schedule.pulses_per_level = vec![2, 2, case.full_level_pulses()];
    cfg.schedule.advance = LadderAdvance::OnPass;
    cfg.schedule.max_pulses_per_level = SRS_CHECK_MAX_PULSES_PER_LEVEL;
    if case == SrsCheckCase::Ladder {
        cfg.schedule.mode = ShockRunMode::Manual;
    }
    cfg.compute_pvss = false;
    cfg.review_after_sysid = false;
    // The identification's block: the largest power of two within
    // SRS_CHECK_SYSID_BLOCK_S at the station's rate (4096 at 20 480 Hz, 8192
    // at 51 200 Hz). A block shorter than one burst and its ring-out costs the
    // estimate its coherence.
    if let Some(spec) = cfg.sysid.as_mut() {
        let n = (SRS_CHECK_SYSID_BLOCK_S * fs_hz).max(64.0) as usize;
        spec.nfft = 1usize << (usize::BITS - 1 - n.leading_zeros());
    }
    cfg.envelope.abort_db = (SRS_CHECK_ABORT_DB, SRS_CHECK_ABORT_DB);
    cfg.control = Some(SrsControl::new(
        SRS_CHECK_DRIVE_CEILING_V,
        SRS_CHECK_DRIVE_ABORT_V,
    ));
    if case.limit_resonance_hz().is_some() {
        cfg.limits = Some(LimitSet {
            channels: vec![LimitChannelSpec {
                name: "limit".into(),
                limit: LimitTarget::Flat {
                    accel_g: SRS_CHECK_LIMIT_BOUND_G,
                },
                frf: Some(srs_check_limit_frf(fs_hz)),
            }],
            headroom_db: SRS_CHECK_LIMIT_HEADROOM_DB,
        });
    }
    cfg
}

/// Run `case` with the law `factory` builds, under the core's guards, on
/// `backend` (the simulated station the controller built for
/// [`SrsCheckCase::plant`], with [`SrsCheckCase::noise_rms_g`]), and measure
/// it. The station is configured with one Drive and one Control channel, at
/// its own rate.
pub fn run_srs_check_case(
    case: SrsCheckCase,
    factory: Arc<dyn SrsLawFactory>,
    backend: Box<dyn Backend>,
) -> Result<SrsCaseMeasure, CheckRunError> {
    let setup = |e: &dyn fmt::Display| CheckRunError::Setup(e.to_string());
    let fs_hz = backend.caps().sample_rate_hz;
    let table = check_table();
    let plant = HalPulsePlant::new(
        backend,
        table.clone(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .map_err(|e| setup(&e))?;
    let cfg = srs_check_config(case, fs_hz);
    let root = std::env::temp_dir().join("hertzforge-srs-check");
    let mut s = SrsShockService::new(cfg, table, FirePath::Host(Box::new(plant)), Some(root))
        .map_err(|e| setup(&e))?
        .with_law_factory(factory)
        .with_identification_scale_db(case.identification_scale_db().unwrap_or(0.0));
    s.set_pacer(Box::new(VirtualPacer::new(0.0)));
    let ran = drive(case, &mut s);
    let fault = ran.err().map(|e| e.to_string());
    let _ = s.stop();
    Ok(measure(case, &s, fault))
}

/// One Drive channel and one Control channel.
fn check_table() -> ChannelTable {
    use hertzforge_channel_table::{Channel, ChannelRole};
    let mut t = ChannelTable::new();
    let mut drive = Channel::new(1);
    drive.role = ChannelRole::Drive;
    drive.description = "Drive".into();
    let mut control = Channel::new(2);
    control.role = ChannelRole::Control;
    control.description = "Control".into();
    t.channels.push(drive);
    t.channels.push(control);
    t
}

/// The case's run: identification, proceeding past the review hold, then the
/// schedule (AUTO), or the ladder case's manual moves: three pulses at -12 dB,
/// a jump to -6 dB and three pulses, six steps of +1 dB to 0 dB and three
/// pulses.
fn drive(case: SrsCheckCase, s: &mut SrsShockService) -> Result<(), hertzforge_srs::ServiceError> {
    s.assess_feasibility()?;
    s.arm()?;
    s.run_sysid()?;
    if s.state() != RunState::AwaitingReview {
        return Ok(());
    }
    s.proceed_after_review()?;
    let fire = |s: &mut SrsShockService, n: usize| -> Result<bool, hertzforge_srs::ServiceError> {
        for _ in 0..n {
            if !matches!(s.state(), RunState::Armed | RunState::Firing) || !s.fire_pulse()? {
                return Ok(false);
            }
        }
        Ok(matches!(s.state(), RunState::Armed | RunState::Firing))
    };
    if case == SrsCheckCase::Ladder {
        if !fire(s, 3)? {
            return Ok(());
        }
        s.jump_to_level(1)?;
        if !fire(s, 3)? {
            return Ok(());
        }
        for _ in 0..6 {
            s.step_up()?;
        }
        fire(s, 3)?;
        return Ok(());
    }
    let cap = s
        .config()
        .control
        .as_ref()
        .map_or(100, |c| c.guards.max_pulses as usize);
    fire(s, cap)?;
    Ok(())
}

fn measure(case: SrsCheckCase, s: &SrsShockService, fault: Option<String>) -> SrsCaseMeasure {
    let report = s.report_data();
    let rows = &report.pulses;
    let full_db = *SRS_CHECK_LEVELS_DB.last().unwrap_or(&0.0);
    let db = case.requirement().band_db();
    let notched = s.limit_held_bands();
    // Every band's error of a fired pulse, from its kept views.
    let errors = |seq: u64| -> Option<Vec<f64>> {
        let v = s.pulse_view(seq)?;
        Some(
            v.measured_srs
                .iter()
                .zip(&v.target_srs)
                .map(|(m, t)| {
                    if *m > 0.0 && *t > 0.0 {
                        20.0 * (m / t).log10()
                    } else {
                        0.0
                    }
                })
                .collect(),
        )
    };
    let within = |r: &hertzforge_srs::service::report::PulseRow| {
        r.worst_err_db.is_some_and(|e| e.abs() <= db)
    };
    let full: Vec<_> = rows
        .iter()
        .filter(|r| (r.scheduled_db - full_db).abs() < 1e-9)
        .collect();
    let first = full.iter().position(|r| within(r));
    let end = match (fault, s.state(), s.trip().map(|t| &t.kind)) {
        (Some(f), ..) => SrsCaseEnd::Fault(f),
        (None, RunState::Held, _) => SrsCaseEnd::Held,
        (None, RunState::NotConverged, _) => SrsCaseEnd::NotConverged,
        (None, _, Some(TripKind::LawFault { fault, .. })) => {
            SrsCaseEnd::LawFault(fault.reason().into())
        }
        (None, RunState::Aborted, Some(k)) => SrsCaseEnd::Aborted(k.name().into()),
        (None, _, _) => SrsCaseEnd::Completed,
    };
    let judged_at = |level: f64| -> Option<f64> {
        let at: Vec<_> = rows
            .iter()
            .filter(|r| (r.scheduled_db - level).abs() < 1e-9)
            .collect();
        let r = if case == SrsCheckCase::Ladder {
            at.get(2)
        } else {
            at.last()
        }?;
        r.worst_err_db
    };
    let law = report.law.as_ref();
    let headroom = s.config().limits.as_ref().map(|l| l.headroom_db);
    SrsCaseMeasure {
        end,
        pulses: rows.len() as u32,
        full_level_pulses: full.len() as u32,
        first_within_pulse: first.map(|i| i as u32 + 1),
        within_after_first: first.is_some_and(|i| full[i..].iter().all(|r| within(r))),
        final_worst_band_db: rows.last().and_then(|r| r.worst_err_db),
        final_worst_outside_notch_db: rows.last().and_then(|r| errors(r.seq)).map(|e| {
            e.iter()
                .enumerate()
                .filter(|(b, _)| !notched.contains(b))
                .map(|(_, &x)| x)
                .fold(0.0f64, |w, x| if x.abs() > w.abs() { x } else { w })
        }),
        levels: SRS_CHECK_LEVELS_DB
            .iter()
            .map(|&l| (l, judged_at(l)))
            .collect(),
        worst_limit_over_db: headroom.and_then(|h| {
            rows.iter()
                .filter_map(|r| r.worst_limit_db)
                .map(|x| x - h)
                .reduce(f64::max)
        }),
        calls: law.map_or(0, |l| l.calls as u32),
        misses: law.map_or(0, |l| l.misses as u32),
        refused: law.map_or(0, |l| l.refused as u32),
        limited_components: law.map_or(0, |l| l.limited_components),
        worst_call_ms: law.filter(|l| l.calls > 0).map(|l| l.worst_call_ms),
        budget_ms: law.map_or(0.0, |l| l.budget_ms),
        max_drive_vpk: rows
            .iter()
            .filter_map(|r| r.drive_vpk)
            .fold(0.0f64, f64::max),
        drive_ceiling_vpk: s
            .config()
            .control
            .as_ref()
            .map_or(0.0, |c| c.drive_vpk_ceiling),
    }
}

impl SrsRequirement {
    /// The case's band, dB either way.
    pub fn band_db(self) -> f64 {
        match self {
            SrsRequirement::WithinBy { db, .. }
            | SrsRequirement::EachLevelWithin { db, .. }
            | SrsRequirement::LimitHeld { db, .. } => db,
        }
    }

    /// The requirement's wire name: `within_by`, `each_level_within` or
    /// `limit_held`.
    pub fn as_str(self) -> &'static str {
        match self {
            SrsRequirement::WithinBy { .. } => "within_by",
            SrsRequirement::EachLevelWithin { .. } => "each_level_within",
            SrsRequirement::LimitHeld { .. } => "limit_held",
        }
    }
}

fn end_words(e: &SrsCaseEnd) -> String {
    match e {
        SrsCaseEnd::Completed => "completed".into(),
        SrsCaseEnd::NotConverged => "ended not converged".into(),
        SrsCaseEnd::Aborted(k) => format!("aborted ({k})"),
        SrsCaseEnd::LawFault(r) => format!("ended on a law fault ({r})"),
        SrsCaseEnd::Held => "held at identification".into(),
        SrsCaseEnd::Fault(f) => format!("failed: {f}"),
    }
}

/// Judge `measure` against `case`: a passed or failed case, with what was
/// measured in words.
pub fn judge_srs(case: SrsCheckCase, measure: &SrsCaseMeasure) -> CheckCase {
    let m = measure;
    let mut why: Vec<String> = Vec::new();
    if m.calls == 0 {
        why.push("the law was never called".into());
    }
    if m.misses > 0 {
        why.push(format!(
            "{} call(s) not used, {} of them refused",
            m.misses, m.refused
        ));
    }
    if let Some(w) = m.worst_call_ms {
        if w > m.budget_ms {
            why.push(format!(
                "the slowest call took {w:.1} ms, over its {:.1} ms budget",
                m.budget_ms
            ));
        }
    }
    if m.end != SrsCaseEnd::Completed {
        why.push(format!("the run {}", end_words(&m.end)));
    }
    match case.requirement() {
        SrsRequirement::WithinBy { db, by_pulse } => match m.first_within_pulse {
            Some(n) if n <= by_pulse && m.within_after_first => {}
            Some(n) if n <= by_pulse => why.push(format!(
                "every band was within {db} dB at full-level pulse {n}, but not on every one \
                 after it"
            )),
            Some(n) => why.push(format!(
                "every band was within {db} dB first at full-level pulse {n}, not by pulse \
                 {by_pulse}"
            )),
            None => why.push(format!(
                "no full-level pulse had every band within {db} dB ({} fired at full level)",
                m.full_level_pulses
            )),
        },
        SrsRequirement::EachLevelWithin { db, after_pulses } => {
            for (level, err) in &m.levels {
                match err {
                    Some(e) if e.abs() <= db => {}
                    Some(e) => why.push(format!(
                        "at {level:+.0} dB the worst band was {e:+.2} dB after {after_pulses} \
                         pulses"
                    )),
                    None => why.push(format!(
                        "{after_pulses} pulses were not fired at {level:+.0} dB"
                    )),
                }
            }
        }
        SrsRequirement::LimitHeld { over_bound_db, db } => {
            match m.worst_limit_over_db {
                Some(o) if o <= over_bound_db => {}
                Some(o) => why.push(format!(
                    "the limit channel's prediction reached {o:+.2} dB over its bound"
                )),
                None => why.push("no limit-channel prediction was made".into()),
            }
            match m.final_worst_outside_notch_db {
                Some(e) if e.abs() <= db => {}
                Some(e) => why.push(format!(
                    "outside the notched bands the worst band was {e:+.2} dB on the last pulse"
                )),
                None => why.push("no pulse was fired".into()),
            }
        }
    }
    if m.max_drive_vpk > m.drive_ceiling_vpk + 1e-12 {
        why.push(format!(
            "the drive reached {:.3} V pk, over the {:.3} V pk ceiling",
            m.max_drive_vpk, m.drive_ceiling_vpk
        ));
    }
    let worst = m
        .worst_call_ms
        .map(|w| format!("{w:.2} ms"))
        .unwrap_or_else(|| "no call".into());
    let summary = format!(
        "{}; {} pulses, {} at full level{}; last pulse's worst band {}; slowest call {worst} \
         of {:.0} ms; {} limited components",
        end_words(&m.end),
        m.pulses,
        m.full_level_pulses,
        m.first_within_pulse
            .map(|n| format!(", every band within at full-level pulse {n}"))
            .unwrap_or_default(),
        m.final_worst_band_db
            .map(|e| format!("{e:+.2} dB"))
            .unwrap_or_else(|| "not measured".into()),
        m.budget_ms,
        m.limited_components
    );
    CheckCase {
        name: case.name().to_string(),
        passed: why.is_empty(),
        detail: if why.is_empty() {
            summary
        } else {
            format!("{}. {summary}", why.join("; "))
        },
    }
}

/// The check record for the law file `law_sha256` from its judged cases.
pub fn srs_check_record(
    law_sha256: impl Into<String>,
    date: impl Into<String>,
    cases: Vec<CheckCase>,
) -> CheckRecord {
    CheckRecord::new(law_sha256, SRS_CHECK_VERSION, date, cases)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_is_listed_once_with_a_unique_name() {
        let mut names: Vec<&str> = SrsCheckCase::ALL.iter().map(|c| c.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SrsCheckCase::ALL.len());
    }

    #[test]
    fn the_cases_carry_the_plan_plants_and_requirements() {
        use SrsCheckCase as C;
        assert_eq!(
            C::OneMode.requirement(),
            SrsRequirement::WithinBy {
                db: 3.0,
                by_pulse: 6
            }
        );
        assert!(matches!(
            C::IdentificationError.requirement(),
            SrsRequirement::WithinBy { by_pulse: 8, .. }
        ));
        assert_eq!(C::IdentificationError.identification_scale_db(), Some(3.0));
        assert_eq!(C::Noise.noise_below_db(), Some(30.0));
        assert_eq!(C::Limit.limit_resonance_hz(), Some(500.0));
        assert!(matches!(
            C::CloseModes.plant(),
            SrsCheckPlant::TwoModes { f_hz, q } if f_hz == [240.0, 260.0] && q == 30.0
        ));
    }

    fn good() -> SrsCaseMeasure {
        SrsCaseMeasure {
            end: SrsCaseEnd::Completed,
            pulses: 10,
            full_level_pulses: 6,
            first_within_pulse: Some(1),
            within_after_first: true,
            final_worst_band_db: Some(0.4),
            final_worst_outside_notch_db: Some(0.4),
            levels: vec![(-12.0, Some(0.5)), (-6.0, Some(0.4)), (0.0, Some(0.4))],
            worst_limit_over_db: Some(-1.0),
            calls: 11,
            misses: 0,
            refused: 0,
            limited_components: 0,
            worst_call_ms: Some(0.1),
            budget_ms: 250.0,
            max_drive_vpk: 0.4,
            drive_ceiling_vpk: 10.0,
        }
    }

    #[test]
    fn a_clean_run_passes_every_case() {
        for case in SrsCheckCase::ALL {
            let j = judge_srs(case, &good());
            assert!(j.passed, "{}: {}", case.name(), j.detail);
        }
    }

    #[test]
    fn each_rule_fails_a_case_by_name() {
        let fails = |case: SrsCheckCase, m: SrsCaseMeasure, words: &str| {
            let j = judge_srs(case, &m);
            assert!(!j.passed, "{}: {}", case.name(), j.detail);
            assert!(j.detail.contains(words), "{}: {}", case.name(), j.detail);
        };
        use SrsCheckCase as C;
        fails(
            C::OneMode,
            SrsCaseMeasure {
                misses: 1,
                refused: 1,
                ..good()
            },
            "not used",
        );
        fails(
            C::OneMode,
            SrsCaseMeasure {
                worst_call_ms: Some(300.0),
                ..good()
            },
            "over its",
        );
        fails(
            C::OneMode,
            SrsCaseMeasure {
                end: SrsCaseEnd::NotConverged,
                ..good()
            },
            "not converged",
        );
        fails(
            C::OneMode,
            SrsCaseMeasure {
                first_within_pulse: Some(7),
                ..good()
            },
            "not by pulse 6",
        );
        fails(
            C::OneMode,
            SrsCaseMeasure {
                within_after_first: false,
                ..good()
            },
            "not on every one",
        );
        fails(
            C::IdentificationError,
            SrsCaseMeasure {
                first_within_pulse: None,
                ..good()
            },
            "no full-level pulse",
        );
        fails(
            C::Ladder,
            SrsCaseMeasure {
                levels: vec![(-12.0, Some(3.5)), (-6.0, Some(0.4)), (0.0, None)],
                ..good()
            },
            "-12 dB the worst band",
        );
        fails(
            C::Limit,
            SrsCaseMeasure {
                worst_limit_over_db: Some(0.8),
                ..good()
            },
            "over its bound",
        );
        fails(
            C::Limit,
            SrsCaseMeasure {
                final_worst_outside_notch_db: Some(-3.4),
                ..good()
            },
            "outside the notched bands",
        );
        fails(
            C::OneMode,
            SrsCaseMeasure {
                max_drive_vpk: 10.5,
                ..good()
            },
            "over the 10.000 V pk ceiling",
        );
        fails(
            C::OneMode,
            SrsCaseMeasure { calls: 0, ..good() },
            "never called",
        );
    }

    #[test]
    fn the_record_carries_the_check_version_and_the_verdict() {
        let cases: Vec<CheckCase> = SrsCheckCase::ALL
            .iter()
            .map(|&c| judge_srs(c, &good()))
            .collect();
        let r = srs_check_record("ab".repeat(32), "2026-11-07T00:00:00Z", cases);
        assert!(r.passed);
        assert_eq!(r.tool_version, SRS_CHECK_VERSION);
    }

    #[test]
    fn the_built_in_resolves_with_checked_parameters_and_other_worlds_are_refused() {
        let root =
            std::env::temp_dir().join(format!("hertzforge-shock-laws-srs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cat = LawCatalog::open(&root).expect("an empty catalog opens");
        let f = resolve_srs(&cat, &default_srs_choice()).unwrap();
        assert_eq!(f.describe().id, "srs_ratio");
        assert_eq!(f.describe().params["max_db_per_pulse"], 3.0);
        let two = LawChoice::new(
            DEFAULT_SRS_LAW_ID,
            vec![hertzforge_law_host::Param::new(
                "max_db_per_pulse",
                ParamValue::Number(2.0),
            )],
        );
        assert_eq!(
            resolve_srs(&cat, &two).unwrap().describe().params["max_db_per_pulse"],
            2.0
        );
        let six = LawChoice::new(
            DEFAULT_SRS_LAW_ID,
            vec![hertzforge_law_host::Param::new(
                "max_db_per_pulse",
                ParamValue::Number(6.0),
            )],
        );
        assert!(matches!(
            resolve_srs(&cat, &six),
            Err(ChoiceError::BadParam { .. })
        ));
        assert!(matches!(
            resolve_srs(&cat, &LawChoice::new("fd_correct", Vec::new())),
            Err(ChoiceError::WrongTestType {
                law_is_for: "classical shock",
                ..
            })
        ));
        assert!(matches!(
            resolve_srs(&cat, &LawChoice::new("no_such_law", Vec::new())),
            Err(ChoiceError::UnknownLaw(_))
        ));
        let _ = std::fs::remove_dir_all(root);
    }
}
