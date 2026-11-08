//! The seam between the run thread and whatever runs the pulses: [`ShockEngine`].
//!
//! The run thread ([`super::run`]) owns the order of things on the wire (arm, start,
//! review, the time between pulses, the hold while nobody holds control, stop and
//! abort, every publish); an engine owns the pulses themselves. The engine a run
//! uses in service is [`ServiceEngine`], the controller's `ClassicalShockService`
//! over the station's backend: identification, the law, the guards, the ladder and
//! every judgement are the controller's. Wire tests that exercise the protocol
//! without a controller run give the service [`super::sim::SimulatedEngine`]
//! instead, as the MIMO wire tests give their simulated driver.

use super::config::{self, WireShock};
use super::preview::{self, Reference};
use super::view::{
    Band, IdentFacts, IdentFigures, LawCall, LawReading, Prediction, PulseFigures, Row, SrsFigures,
    StationRef, TimeFigures, Trip, TripFacts,
};
use crate::protocol::{ErrorCode, MethodError};
use crate::pulse::rails::{Progress, Signals};
use crate::station::{self, class_name, NiScanner, StationPlan};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_dsp::shock::pulse_kinematic_series;
use hertzforge_dsp::srs::srs_analyze;
use hertzforge_hal::Backend;
use hertzforge_law_host::DiagnosticSpec;
use hertzforge_shock_common::law_guard::LawFault;
use hertzforge_shock_common::pulse_store::DEFAULT_SRS_Q;
use hertzforge_shock_common::{ShockRunMode, TrainSpec};
use hertzforge_shock_host::service::report::{
    ConvergenceLimit, IdentificationSummary, PulseOrigin,
};
use hertzforge_shock_host::{
    BackendPulseFirer, ClassicalLawFactory, ClassicalShockService, PulseRow, RunState, TripKind,
};
use hertzforge_shock_law_math::spectrum::{abs, rfft_half};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

/// What a run arms on, taken from the service when the request arrives.
pub struct ArmPlan {
    pub cfg: Arc<WireShock>,
    pub table: ChannelTable,
    pub station: StationPlan,
    pub scanner: Arc<dyn NiScanner>,
    /// The service's local data directory: the hardware lock lives here.
    pub data_root: PathBuf,
    pub capture_root: Option<PathBuf>,
    pub bind: String,
    /// The run's law, resolved by the service from the catalog.
    pub law: Result<Arc<dyn ClassicalLawFactory>, MethodError>,
    /// The values the law reports, as its manifest declares them.
    pub law_values: Vec<DiagnosticSpec>,
    /// The operator's acknowledgement to arm past a feasibility exceedance.
    pub ack: Option<String>,
}

/// An armed station's facts.
pub struct Armed {
    pub fs_hz: f64,
    pub record_len: usize,
    pub station: StationRef,
    pub feasibility: (&'static str, String),
    /// Held while the run is live on hardware.
    pub lock: Option<hertzforge_station::lock::LockGuard>,
}

/// How an arm ended.
pub enum ArmOutcome {
    Armed(Armed),
    /// Feasibility exceeded and no acknowledgement was given.
    Blocked((&'static str, String)),
}

/// How identification ended.
pub enum Identified {
    /// Done: hold for review, or go on.
    Done { facts: IdentFacts, review: bool },
    /// The run is held before any pulse.
    Held {
        facts: Option<IdentFacts>,
        message: String,
    },
}

/// The engine ended the run: its outcome, words and trip.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineEnd {
    pub outcome: &'static str,
    pub message: String,
    pub trip: Option<Trip>,
}

/// How seeding the law ended.
pub enum Seeded {
    Ready(Prediction),
    Ended(EngineEnd),
}

/// What one fire (a pulse, a train or a burst) did.
#[derive(Default)]
pub struct Fired {
    pub rows: Vec<Row>,
    pub figures: Vec<PulseFigures>,
    /// The engine ended the run on it.
    pub end: Option<EngineEnd>,
}

/// A level move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LevelMove {
    /// One step, in dB.
    Step(f64),
    /// A level of the table, from 0.
    Rung(usize),
}

/// Where the ladder is.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Position {
    pub level_db: f64,
    pub rung: Option<usize>,
    pub at_level: u32,
    pub of_level: u32,
    pub passed: u64,
}

/// What a run's engine does. Every call is made on the run thread; a fire blocks it
/// for the whole record and reads [`Signals::cancel`] at every block.
pub trait ShockEngine: Send {
    /// Feasibility, the station, the arm gate and the grid at the rate the station
    /// programmed. Nothing is driven.
    fn arm(&mut self, plan: ArmPlan) -> Result<ArmOutcome, MethodError>;
    /// Identification: one cancellable fire.
    fn identify(&mut self) -> Result<Identified, MethodError>;
    /// Seed the law and predict the full-level drive.
    fn seed(&mut self) -> Result<Seeded, MethodError>;
    /// Fire one pulse at the present level.
    fn fire(&mut self) -> Result<Fired, MethodError>;
    /// Fire `count` pulses `spacing_s` apart at the present level, the drive in
    /// force.
    fn train(&mut self, count: u32, spacing_s: f64) -> Result<Fired, MethodError>;
    /// Fire the configured gunfire burst.
    fn burst(&mut self) -> Result<Fired, MethodError>;
    /// Move the level; the new level, dB.
    fn set_level(&mut self, to: LevelMove) -> Result<f64, MethodError>;
    /// The identified drive's peak at `level_db`, volts, once seeded.
    fn predicted_vpk_at(&self, level_db: f64) -> Option<f64>;
    /// The controlled stop: park, nothing more fires.
    fn stop(&mut self);
    /// The emergency stop.
    fn abort(&mut self, reason: &str);
    fn position(&self) -> Position;
    fn identification(&self) -> Option<IdentFigures>;
    fn law_values(&self) -> Vec<LawReading>;
    /// Unusable law calls in a row now.
    fn misses(&self) -> u32;
    /// Log lines since the last call.
    fn take_log(&mut self) -> Vec<(&'static str, String)>;
    /// When the device started the last pulse, as the controller stamped it once
    /// the station's clock ran: the moment the time between pulses is held from.
    /// `None` leaves the run thread its own moment of asking for the pulse.
    fn last_pulse_start(&self) -> Option<Instant> {
        None
    }
}

/// What an engine is built with: the run's signals and its progress cell, and the
/// simulator's pace.
#[derive(Clone)]
pub struct EngineContext {
    pub signals: Signals,
    pub progress: Arc<Progress>,
    /// Simulated seconds per wall second on the simulator; 0 runs unpaced.
    pub sim_speed: f64,
}

/// Makes a run's engine.
pub type EngineFactory = Arc<dyn Fn(&EngineContext) -> Box<dyn ShockEngine> + Send + Sync>;

/// The engine a run uses when the service is given none: the controller's.
pub fn service_engine_factory() -> EngineFactory {
    Arc::new(|ctx| Box::new(ServiceEngine::new(ctx.clone())))
}

fn arm_refused(words: impl std::fmt::Display) -> MethodError {
    MethodError::named(
        ErrorCode::Refused,
        "arm_refused",
        format!("The station refused to arm: {words}"),
    )
}

fn fault(words: impl std::fmt::Display) -> MethodError {
    MethodError::internal(format!("the classical shock service failed: {words}"))
}

/// The wire's name of a pulse's origin.
pub fn origin_name(o: PulseOrigin) -> &'static str {
    match o {
        PulseOrigin::Ladder => "ladder",
        PulseOrigin::Train => "train",
        PulseOrigin::Gunfire => "gunfire",
    }
}

/// A controller pulse row on the wire.
pub fn row_of(p: &PulseRow) -> Row {
    let verdicts = match (p.envelope_pass, p.velocity_change_pass, p.substitution_pass) {
        (Some(e), Some(v), Some(s)) => Some((e, v, s)),
        _ => None,
    };
    Row {
        index: p.pulse,
        origin: origin_name(p.origin),
        scheduled_db: p.scheduled_level_db,
        commanded_db: p.commanded_db,
        limit_scale: p.drive_limit_scale,
        drive_vpk: p.drive_vpk.unwrap_or(f64::NAN),
        peak_g: p.peak_g,
        peak_ratio: p.peak_ratio.or_else(|| {
            p.peak_g
                .filter(|_| p.commanded_amplitude_g > 0.0)
                .map(|g| g / p.commanded_amplitude_g)
        }),
        error_pct: p.error_pct,
        velocity_change_ips: p.velocity_change_ips,
        velocity_change_ratio: p.velocity_change_ratio,
        verdicts,
        passed: p.tolerance_overall,
        srs: match (p.srs_bands_total, p.srs_bands_pass) {
            (Some(t), Some(ok)) => Some((t, ok)),
            _ => None,
        },
        at_s: p.at_wall_time_s,
        law: p.law.as_ref().map(|l| LawCall {
            used: l.used,
            miss: l.miss.as_ref().map(LawFault::reason),
            capped_bins: l.capped_bins,
            peak_capped: l.peak_capped,
            call_ms: l.call_ms,
        }),
    }
}

/// A level in words: "full level" at 0 dB, else its dB.
fn level_words(db: f64) -> String {
    if db.abs() < 1e-9 {
        "full level".to_owned()
    } else {
        format!("{} dB", crate::protocol::num(config::round(db, 2)))
    }
}

/// `x` to three significant digits, for words.
fn sig3(x: f64) -> serde_json::Value {
    crate::protocol::num(super::view::sig(x, 3))
}

/// A controller trip on the wire: the end it makes, its words and its trip.
/// `predicted_db` is the level a drive found over the abort at the seed names
/// (the top of an automatic schedule, the present manual level).
pub fn end_of_trip(t: &hertzforge_shock_host::TripRecord, predicted_db: f64) -> EngineEnd {
    let trip = |facts, pulse: u64, at: f64| {
        Some(Trip {
            facts,
            pulse,
            at_level_db: Some(at),
        })
    };
    let at = t.at_level_db;
    let aborted = |message: String, trip| EngineEnd {
        outcome: "aborted",
        message,
        trip,
    };
    match &t.kind {
        TripKind::Tolerance {
            pulse,
            envelope_pass,
            velocity_change_pass,
            error_frac,
            worst_deviation_frac_of_peak,
        } => aborted(
            format!(
                "Pulse {} failed the tolerance at {}.",
                pulse + 1,
                level_words(at)
            ),
            trip(
                TripFacts::Tolerance {
                    envelope_pass: *envelope_pass,
                    velocity_change_pass: *velocity_change_pass,
                    error_pct: 100.0 * error_frac.abs(),
                    worst_deviation_frac: *worst_deviation_frac_of_peak,
                },
                *pulse,
                at,
            ),
        ),
        TripKind::Kinematic {
            quantity,
            value,
            limit,
            pulse,
            predicted,
        } => aborted(
            if *predicted {
                format!(
                    "Pulse {} at {} would pass the shaker's {quantity}: {} predicted against \
                     {}. Nothing was fired for it.",
                    pulse + 1,
                    level_words(at),
                    sig3(*value),
                    sig3(*limit)
                )
            } else {
                format!(
                    "Pulse {} passed the shaker's {quantity}: {} against {}.",
                    pulse + 1,
                    sig3(*value),
                    sig3(*limit)
                )
            },
            trip(
                TripFacts::Kinematic {
                    axis: if quantity.contains("velocity") {
                        "velocity"
                    } else {
                        "displacement"
                    },
                    value: *value,
                    limit: *limit,
                    predicted: *predicted,
                },
                *pulse,
                at,
            ),
        ),
        TripKind::PreFireRefused { reason } => EngineEnd {
            outcome: "fault",
            message: format!("The pre-fire check refused to fire: {reason}"),
            trip: None,
        },
        TripKind::OperatorAbort { .. } => EngineEnd {
            outcome: "operator_abort",
            message: "The operator aborted the run.".into(),
            trip: None,
        },
        TripKind::Overshoot {
            pulse,
            peak_ratio,
            limit_ratio,
        } => aborted(
            format!(
                "Pulse {} overshot at {}: its peak was {} of the commanded peak, over the {} \
                 that aborts.",
                pulse + 1,
                level_words(at),
                sig3(*peak_ratio),
                sig3(*limit_ratio)
            ),
            trip(
                TripFacts::Overshoot {
                    peak_ratio: *peak_ratio,
                    limit_ratio: *limit_ratio,
                },
                *pulse,
                at,
            ),
        ),
        TripKind::DriveVpk {
            pulse,
            vpk,
            abort_vpk,
            predicted,
        } => {
            let level = if *predicted { predicted_db } else { at };
            let v = crate::protocol::num(config::round(*vpk, 3));
            let a = crate::protocol::num(config::round(*abort_vpk, 3));
            aborted(
                if *predicted {
                    format!(
                        "The identified drive needs {v} V pk at {}; the drive abort is {a} V pk. \
                         Nothing was fired.",
                        level_words(level)
                    )
                } else {
                    format!(
                        "Pulse {}'s drive would have been {v} V pk at {}, over the {a} V pk drive \
                         abort; it was not played.",
                        pulse + 1,
                        level_words(level)
                    )
                },
                trip(
                    TripFacts::DriveVpk {
                        vpk: *vpk,
                        abort_vpk: *abort_vpk,
                        predicted: *predicted,
                    },
                    *pulse,
                    level,
                ),
            )
        }
        TripKind::Divergence {
            pulse,
            pulses,
            error_pct,
        } => aborted(
            format!(
                "The error rose on {pulses} pulses in a row at {}; pulse {} was {} % off the \
                 reference.",
                level_words(at),
                pulse + 1,
                sig3(*error_pct)
            ),
            trip(
                TripFacts::Divergence {
                    pulses: *pulses,
                    error_pct: *error_pct,
                },
                *pulse,
                at,
            ),
        ),
        TripKind::NotConverged { limit, pulses } => EngineEnd {
            outcome: "not_converged",
            message: match limit {
                ConvergenceLimit::PerLevel => format!(
                    "{pulses} pulses at {} without a pass; the run did not converge.",
                    level_words(at)
                ),
                ConvergenceLimit::Run => {
                    format!("The run's limit of {pulses} pulses came before the schedule finished.")
                }
            },
            trip: trip(
                TripFacts::NotConverged {
                    limit: limit.as_str(),
                    pulses: *pulses,
                },
                t.at_pulse.saturating_sub(1),
                at,
            ),
        },
        TripKind::LawFault { fault, consecutive } => EngineEnd {
            outcome: "law_fault",
            message: if t.at_pulse == 0 {
                format!("The control law's first drive was not usable: {fault}. Nothing was fired.")
            } else {
                format!(
                    "The control law gave no usable drive {consecutive} times in a row: {fault}. \
                     Nothing more was fired."
                )
            },
            trip: trip(
                TripFacts::LawFault {
                    reason: fault.reason(),
                    consecutive: *consecutive,
                },
                t.at_pulse,
                at,
            ),
        },
    }
}

// ---------------------------------------------------------------------------------
// The controller's engine.
// ---------------------------------------------------------------------------------

type Service = ClassicalShockService<BackendPulseFirer<Box<dyn Backend>>>;

/// The controller's classical shock service over the station's backend.
///
/// The controller keeps its own cancel flag rather than the run's: it raises its
/// flag itself when it ends a run (a trip, a hold), while the run's flag means the
/// operator or the service asked, and the run thread reads it that way. The run's
/// flag reaches the controller's at every block of a fire and every poll of a wait.
pub struct ServiceEngine {
    ctx: EngineContext,
    svc: Option<Service>,
    cfg: Option<Arc<WireShock>>,
    reference: Option<Reference>,
    /// The values the law reports, as its manifest declares them.
    law_values: Vec<DiagnosticSpec>,
    /// The control channel's unit.
    unit: String,
    rows_seen: usize,
    log_seen: usize,
    log: Vec<(&'static str, String)>,
}

impl ServiceEngine {
    pub fn new(ctx: EngineContext) -> ServiceEngine {
        ServiceEngine {
            ctx,
            svc: None,
            cfg: None,
            reference: None,
            law_values: Vec::new(),
            unit: "g".into(),
            rows_seen: 0,
            log_seen: 0,
            log: Vec::new(),
        }
    }

    fn svc(&mut self) -> Result<&mut Service, MethodError> {
        self.svc
            .as_mut()
            .ok_or_else(|| fault("the run lost its station"))
    }

    /// The level a drive over the abort at the seed is predicted at: the top of an
    /// automatic schedule, the present manual level.
    fn predicted_db(&self) -> f64 {
        match &self.svc {
            Some(s) if s.config().schedule.mode == ShockRunMode::Auto => {
                s.config().schedule.full_db()
            }
            Some(s) => s.current_level_db(),
            None => 0.0,
        }
    }

    /// How the controller ended the run, when it has.
    fn ended(&self) -> Option<EngineEnd> {
        let svc = self.svc.as_ref()?;
        match svc.state() {
            RunState::Aborted | RunState::NotConverged => Some(
                svc.trip()
                    .map(|t| end_of_trip(t, self.predicted_db()))
                    .unwrap_or_else(|| EngineEnd {
                        outcome: "fault",
                        message: "The controller ended the run without a trip record.".into(),
                        trip: None,
                    }),
            ),
            RunState::Stopped if svc.status().ladder_completed => Some(EngineEnd {
                outcome: "completed",
                message: "The schedule finished.".into(),
                trip: None,
            }),
            _ => None,
        }
    }

    /// The rows and figures fired since the last look, and the end the
    /// controller's state says.
    fn collect(&mut self) -> Fired {
        self.ctx.progress.end();
        let Some(svc) = self.svc.as_mut() else {
            return Fired::default();
        };
        // With capture running the controller writes each law call into the run
        // folder; without it the records have nowhere to go and are let go here
        // rather than held for the whole run.
        if !svc.status().capturing {
            let _ = svc.drain_law_records();
        }
        let Some(svc) = self.svc.as_ref() else {
            return Fired::default();
        };
        let report = svc.report_data();
        let mut out = Fired::default();
        for p in report.pulses.iter().skip(self.rows_seen) {
            out.rows.push(row_of(p));
            out.figures.push(self.figures_of(svc, p));
        }
        self.rows_seen = report.pulses.len();
        forward_log(&mut self.log_seen, &mut self.log, svc);
        out.end = self.ended();
        out
    }

    /// One pulse's figures, from the controller's view of it: the reference at the
    /// level fired, its band, the measured pulse and the drive as played, on the
    /// shown part of the record.
    fn figures_of(&self, svc: &Service, p: &PulseRow) -> PulseFigures {
        let fs = svc.fs_hz();
        let Some(v) = svc.pulse_view(p.pulse) else {
            // Not measured: the reference at the level fired alone.
            let scale = 10f64.powf(p.commanded_db / 20.0);
            let time = self.reference.as_ref().map_or_else(
                || TimeFigures {
                    start: 0,
                    fs_hz: fs,
                    reference: Vec::new(),
                    band: None,
                    measured: None,
                },
                |r| r.figures(scale),
            );
            return PulseFigures {
                time,
                drive: Vec::new(),
                velocity: None,
                displacement: None,
                srs: None,
                spectrum: None,
            };
        };
        let main = svc
            .grid_reference()
            .map_or(v.band.main_start.max(0) as usize, |(_, s, _)| s);
        let end = self
            .reference
            .as_ref()
            .map_or(v.reference.len(), |r| r.shown().1)
            .min(v.reference.len())
            .min(v.measured.len());
        let cut = |x: &[f64]| x[..end.min(x.len())].to_vec();
        let reference = cut(&v.reference);
        let measured = cut(&v.measured);
        let band = {
            let b0 = v.band.start;
            let lo = b0.max(0);
            let hi = (b0 + v.band.upper.len() as i64).min(end as i64);
            (lo < hi).then(|| Band {
                start: lo as usize,
                hi: v.band.upper[(lo - b0) as usize..(hi - b0) as usize].to_vec(),
                lo: v.band.lower[(lo - b0) as usize..(hi - b0) as usize].to_vec(),
            })
        };
        let rk = pulse_kinematic_series(&reference, fs);
        let mk = pulse_kinematic_series(&measured, fs);
        let srs = p
            .store_seq
            .and_then(|s| svc.pulse_store().get(s))
            .map(|rec| {
                let r = srs_analyze(&v.reference, fs, &rec.srs_freqs, DEFAULT_SRS_Q);
                let tol = 10f64.powf(svc.config().srs_band_tol_db / 20.0);
                SrsFigures {
                    freqs_hz: rec.srs_freqs.clone(),
                    band_hi: r.iter().map(|x| x * tol).collect(),
                    band_lo: r.iter().map(|x| x / tol).collect(),
                    reference: r,
                    measured: Some(rec.srs.clone()),
                }
            });
        let spectrum = self.cfg.as_deref().and_then(|c| {
            let (lo, hi) = c.band_hz;
            let n = v.reference.len().min(v.measured.len());
            let r = rfft_half(&v.reference[..n]).ok()?;
            let y = rfft_half(&v.measured[..n]).ok()?;
            let df = fs / n as f64;
            let bins: Vec<usize> = (1..r.len().min(y.len()))
                .filter(|&k| (lo..=hi).contains(&(k as f64 * df)))
                .collect();
            Some((
                bins.iter().map(|&k| k as f64 * df).collect(),
                bins.iter().map(|&k| abs(r[k])).collect(),
                bins.iter().map(|&k| abs(y[k])).collect(),
            ))
        });
        PulseFigures {
            time: TimeFigures {
                start: -(main as i64),
                fs_hz: fs,
                reference,
                band,
                measured: Some(measured),
            },
            drive: cut(&v.drive),
            velocity: Some((rk.velocity_ips, mk.velocity_ips)),
            displacement: Some((rk.disp_in, mk.disp_in)),
            srs,
            spectrum,
        }
    }
}

/// A controller's words as a sentence: a capital first, a stop at the end.
fn sentence(words: &str) -> String {
    let mut c = words.chars();
    let mut s = match c.next() {
        Some(f) => format!("{}{}", f.to_uppercase(), c.as_str()),
        None => return String::new(),
    };
    if !s.ends_with('.') {
        s.push('.');
    }
    s
}

/// The controller's events since `seen`, as log lines.
fn forward_log(seen: &mut usize, log: &mut Vec<(&'static str, String)>, svc: &Service) {
    use hertzforge_shock_host::service::report::EventKind;
    let level = |db: f64| crate::protocol::num(config::round(db, 2));
    let events = svc.log().events();
    for e in events.iter().skip(*seen) {
        let line = match &e.kind {
            EventKind::Note { text } => Some(("note", text.clone())),
            EventKind::PulseFired {
                pulse,
                commanded_db,
                peak_g,
                tolerance_pass,
                ..
            } => Some((
                "pulse",
                format!(
                    "pulse {} at {} dB: {}",
                    pulse + 1,
                    level(*commanded_db),
                    match (peak_g, tolerance_pass) {
                        (Some(g), Some(true)) => format!("{} g, passed", sig3(*g)),
                        (Some(g), Some(false)) => format!("{} g, failed", sig3(*g)),
                        (Some(g), None) => format!("{} g", sig3(*g)),
                        (None, _) => "not measured".to_owned(),
                    }
                ),
            )),
            EventKind::LawCalled {
                pulse,
                used: false,
                reason,
                detail,
                ..
            } => Some((
                "law",
                format!(
                    "{} was not used ({}){}",
                    if *pulse == 0 {
                        "the law's first drive".to_owned()
                    } else {
                        format!("the law's drive after pulse {pulse}")
                    },
                    reason.as_deref().unwrap_or("unusable"),
                    detail
                        .as_deref()
                        .map_or_else(String::new, |d| format!(": {d}"))
                ),
            )),
            EventKind::Identified {
                coherence_mean,
                trusted_bins,
                band_energy,
                drive_vpk,
                ..
            } => Some((
                "note",
                format!(
                    "identified: mean coherence {:.3}, {trusted_bins} trusted bins covering {:.1}% \
                     of the reference's energy, drive peak {:.3} V",
                    coherence_mean,
                    100.0 * band_energy,
                    drive_vpk
                ),
            )),
            EventKind::DriveLimitAction {
                pulse,
                phase,
                scale,
                reason,
            } => Some((
                "note",
                format!(
                    "drive limit ({phase}) on pulse {}: scale {:.3}, {reason}",
                    pulse + 1,
                    scale
                ),
            )),
            EventKind::ToleranceAlarm {
                pulse, error_frac, ..
            } => Some((
                "alarm",
                format!(
                    "pulse {} out of tolerance: {:.1}% velocity-change error",
                    pulse + 1,
                    100.0 * error_frac.abs()
                ),
            )),
            EventKind::DriverWarning { text, .. } => Some(("note", format!("driver: {text}"))),
            EventKind::RateAdopted {
                requested_hz,
                actual_hz,
                ..
            } => Some((
                "station",
                format!("the device runs at {actual_hz} Hz (asked for {requested_hz} Hz)"),
            )),
            _ => None,
        };
        if let Some(l) = line {
            log.push(l);
        }
    }
    *seen = events.len();
}

fn ident_facts(s: &IdentificationSummary) -> IdentFacts {
    IdentFacts {
        drive_vrms: s.drive_vrms,
        drive_vpk: s.drive_vpk,
        coherence_mean: s.coherence_mean,
        min_mean_coherence: s.min_mean_coherence,
        passed: s.passed,
    }
}

/// A fire the operator or the service stopped is not a fault: the run thread
/// ends the run on its signal.
fn fired_or_cancelled<T>(
    r: Result<T, hertzforge_shock_host::ServiceError>,
) -> Result<(), MethodError> {
    match r {
        Ok(_) | Err(hertzforge_shock_host::ServiceError::Cancelled { .. }) => Ok(()),
        Err(e) => Err(fault(e)),
    }
}

impl ShockEngine for ServiceEngine {
    fn arm(&mut self, plan: ArmPlan) -> Result<ArmOutcome, MethodError> {
        let law = plan.law?;
        let mut cfg = plan.cfg.build();
        if plan.capture_root.is_some() {
            cfg.capture.auto_start = cfg.capture.auto_start && plan.capture_root.is_some();
        }
        // The run thread holds for review itself (the protocol's hold) and calls
        // the seed when it is over, so the controller always stops there.
        if let Some(c) = cfg.control.as_mut() {
            c.review_hold = true;
        }
        let built = station::build(&plan.station, &*plan.scanner)?;
        let hardware = built.station.is_hardware();
        let lock = if hardware {
            Some(
                hertzforge_station::lock::acquire(
                    &plan.data_root,
                    &plan.bind,
                    &built.provenance.station_label,
                )
                .map_err(|e| match e {
                    hertzforge_station::lock::LockError::Held(h) => MethodError::named(
                        ErrorCode::Refused,
                        "hardware_busy",
                        format!("Another program holds the NI hardware: {}", h.advice()),
                    )
                    .with_detail(json!({"holder": h.advice()})),
                    hertzforge_station::lock::LockError::Io(m) => arm_refused(m),
                })?,
            )
        } else {
            None
        };
        let class = built.station;
        let provenance = built.provenance.clone();
        self.unit = plan
            .table
            .channels
            .iter()
            .find(|c| c.enabled && c.role == ChannelRole::Control)
            .map_or_else(|| "g".to_owned(), |c| crate::table::unit_name(&c.units));
        let firer = BackendPulseFirer::new(built.backend, plan.table.clone(), class)
            .map_err(arm_refused)?;
        let mut svc = ClassicalShockService::new(cfg, plan.table, firer, plan.capture_root)
            .map_err(arm_refused)?
            .with_law_factory(law);
        let own = svc.cancel_flag();
        let asked = self.ctx.signals.cancel.clone();
        let progress = self.ctx.progress.clone();
        let speed = self.ctx.sim_speed;
        let fs = plan.cfg.fs_hz;
        let paced = !hardware && speed > 0.0;
        // On the simulator a fire is paced to `speed` times real time from its own
        // first block, as Random's is.
        let mut t0: Option<Instant> = None;
        let bridge = own.clone();
        let asked_fire = asked.clone();
        svc.set_block_observer(Box::new(move |done, of| {
            if asked_fire.load(Ordering::SeqCst) {
                bridge.store(true, Ordering::SeqCst);
            }
            if progress.now().is_none() {
                progress.start(of as u64);
            }
            progress.block(done as u64, of as u64);
            if done >= of {
                progress.end();
            }
            if paced {
                let start = *t0.get_or_insert_with(Instant::now);
                if done >= of {
                    t0 = None;
                }
                let ahead = done as f64 / fs / speed - start.elapsed().as_secs_f64();
                if ahead > 0.005 {
                    std::thread::sleep(std::time::Duration::from_secs_f64(ahead.min(0.25)));
                }
            }
        }));
        svc.set_wait_observer(Box::new(move |_left| {
            if asked.load(Ordering::SeqCst) {
                own.store(true, Ordering::SeqCst);
            }
        }));
        let feas = svc.assess_feasibility().map_err(arm_refused)?;
        let verdict = config::verdict(&feas);
        if svc.state() == RunState::FeasibilityBlocked {
            match &plan.ack {
                Some(a) => svc.arm_with_override(a).map_err(arm_refused)?,
                None => return Ok(ArmOutcome::Blocked(verdict)),
            }
        } else {
            svc.arm().map_err(arm_refused)?;
        }
        let adopted = svc.fs_hz();
        let reference = preview::reference(&plan.cfg.pulse(), plan.cfg.standard, adopted);
        let record_len = reference.record_len;
        self.reference = Some(reference);
        self.cfg = Some(plan.cfg.clone());
        self.law_values = plan.law_values;
        forward_log(&mut self.log_seen, &mut self.log, &svc);
        self.svc = Some(svc);
        Ok(ArmOutcome::Armed(Armed {
            fs_hz: adopted,
            record_len,
            station: StationRef {
                kind: if matches!(plan.station, StationPlan::Sim { .. }) {
                    "sim"
                } else {
                    "ni"
                },
                class: class_name(class),
                devices: provenance
                    .device_ids
                    .iter()
                    .cloned()
                    .zip(provenance.serials.iter().cloned())
                    .collect(),
            },
            feasibility: verdict,
            lock,
        }))
    }

    fn identify(&mut self) -> Result<Identified, MethodError> {
        let review = self
            .cfg
            .as_deref()
            .ok_or_else(|| fault("the run is not armed"))?
            .review;
        let state = self.svc()?.identify().map_err(fault);
        self.ctx.progress.end();
        let state = state?;
        let svc = self
            .svc
            .as_ref()
            .ok_or_else(|| fault("the run lost its station"))?;
        forward_log(&mut self.log_seen, &mut self.log, svc);
        let status = svc.status();
        let facts = status.identification.as_ref().map(ident_facts);
        match state {
            RunState::AwaitingReview => Ok(Identified::Done {
                facts: facts.ok_or_else(|| fault("identification left no summary"))?,
                review,
            }),
            RunState::Held => Ok(Identified::Held {
                facts,
                message: status.held.map_or_else(
                    || "Identification held the run.".to_owned(),
                    |h| sentence(&h.to_string()),
                ),
            }),
            // A cancel ends identification `Stopped`; the run thread ends the run on
            // its signal before it reads this.
            other => Err(fault(format!("identification ended in {other:?}"))),
        }
    }

    fn seed(&mut self) -> Result<Seeded, MethodError> {
        let state = self.svc()?.proceed().map_err(fault)?;
        let svc = self
            .svc
            .as_ref()
            .ok_or_else(|| fault("the run lost its station"))?;
        forward_log(&mut self.log_seen, &mut self.log, svc);
        if let Some(end) = self.ended() {
            return Ok(Seeded::Ended(end));
        }
        if state != RunState::Firing {
            return Err(fault(format!("the law's seed left the run in {state:?}")));
        }
        let p = svc
            .status()
            .prediction
            .ok_or_else(|| fault("the law's seed made no prediction"))?;
        Ok(Seeded::Ready(Prediction {
            drive_vpk: p.drive_vpk,
            drive_vpk_ceiling: p.drive_vpk_ceiling,
            drive_vpk_abort: p.drive_vpk_abort,
            response_peak_g: p.response_peak_g,
            velocity_pk_ips: p.velocity_pk_ips,
            displacement_pk_pk_in: p.displacement_pk_pk_in,
        }))
    }

    fn fire(&mut self) -> Result<Fired, MethodError> {
        let go = self.ctx.signals.go.clone();
        fired_or_cancelled(self.svc()?.fire_next_pulse_cancellable(Some(&go)))?;
        let mut fired = self.collect();
        if fired.end.is_none() {
            let completed = self.svc.as_ref().is_some_and(|s| {
                s.config().schedule.mode == ShockRunMode::Auto && s.status().ladder_completed
            });
            if completed {
                let _ = self.svc()?.stop();
                fired.end = Some(EngineEnd {
                    outcome: "completed",
                    message: "The schedule finished.".into(),
                    trip: None,
                });
            }
        }
        Ok(fired)
    }

    fn train(&mut self, count: u32, spacing_s: f64) -> Result<Fired, MethodError> {
        let go = self.ctx.signals.go.clone();
        fired_or_cancelled(
            self.svc()?
                .fire_train_cancellable(TrainSpec::delayed(count, spacing_s), Some(&go)),
        )?;
        Ok(self.collect())
    }

    fn burst(&mut self) -> Result<Fired, MethodError> {
        let go = self.ctx.signals.go.clone();
        fired_or_cancelled(self.svc()?.fire_gunfire_burst_cancellable(Some(&go)))?;
        Ok(self.collect())
    }

    fn set_level(&mut self, to: LevelMove) -> Result<f64, MethodError> {
        let svc = self.svc()?;
        let r = match to {
            LevelMove::Step(d) if d > 0.0 => svc.step_up(),
            LevelMove::Step(_) => svc.step_down(),
            LevelMove::Rung(i) => svc.jump_to_level(i),
        };
        r.map_err(|e| match e {
            hertzforge_shock_host::ServiceError::DriveCeiling {
                level_db,
                vpk,
                abort_vpk,
            } => MethodError::named(
                ErrorCode::Refused,
                "drive_ceiling",
                format!(
                    "The identified drive needs {} V pk at {} dB; the drive abort is {} V pk.",
                    crate::protocol::num(config::round(vpk, 3)),
                    crate::protocol::num(config::round(level_db, 2)),
                    crate::protocol::num(abort_vpk)
                ),
            )
            .with_detail(json!({
                "vpk": crate::protocol::num(config::round(vpk, 4)),
                "abort_vpk": crate::protocol::num(abort_vpk),
                "level_db": crate::protocol::num(config::round(level_db, 2)),
            })),
            other => MethodError::named(ErrorCode::Refused, "wrong_state", other.to_string()),
        })
    }

    fn predicted_vpk_at(&self, level_db: f64) -> Option<f64> {
        self.svc.as_ref()?.predicted_drive_vpk_at(level_db)
    }

    fn stop(&mut self) {
        if let Some(svc) = self.svc.as_mut() {
            let _ = svc.stop();
        }
    }

    fn abort(&mut self, reason: &str) {
        if let Some(svc) = self.svc.as_mut() {
            let _ = svc.abort_with_reason(reason);
        }
    }

    fn position(&self) -> Position {
        let Some(svc) = self.svc.as_ref() else {
            return Position::default();
        };
        let st = svc.status();
        let schedule = &svc.config().schedule;
        let on_table = schedule
            .table_db
            .get(st.level_index)
            .is_some_and(|l| (l - st.current_level_db).abs() < 1e-9);
        Position {
            level_db: st.current_level_db,
            rung: on_table.then_some(st.level_index),
            at_level: st.pulses_at_level,
            of_level: if schedule.mode == ShockRunMode::Auto {
                schedule.pulses_for(st.level_index)
            } else {
                0
            },
            passed: st.pulses_passed,
        }
    }

    fn identification(&self) -> Option<IdentFigures> {
        let svc = self.svc.as_ref()?;
        let cfg = self.cfg.as_deref()?;
        let v = svc.identification_view()?;
        let (lo, hi) = cfg.band_hz;
        let mut f = IdentFigures {
            freqs_hz: Vec::new(),
            gain: Vec::new(),
            phase_deg: Vec::new(),
            coherence: Vec::new(),
            trusted: Vec::new(),
            floor: cfg.coherence_floor,
            unit: self.unit.clone(),
        };
        for (k, (&g, &ph)) in v.h_mag.iter().zip(&v.h_phase).enumerate().skip(1) {
            let fk = k as f64 * v.df_hz;
            if fk < lo || fk > hi {
                continue;
            }
            f.freqs_hz.push(fk);
            f.gain.push(g);
            f.phase_deg.push(ph.to_degrees());
            f.coherence.push(v.coherence.get(k).copied().unwrap_or(0.0));
            let on_grid = (fk / v.grid_df_hz).round() as usize;
            f.trusted
                .push(v.trusted.get(on_grid).copied().unwrap_or(false));
        }
        Some(f)
    }

    fn law_values(&self) -> Vec<LawReading> {
        let Some(law) = self.svc.as_ref().and_then(|s| s.status().law) else {
            return Vec::new();
        };
        law.values
            .iter()
            .map(|v| {
                let d = self.law_values.iter().find(|d| d.key == v.key);
                LawReading {
                    label: d.map_or_else(|| v.key.clone(), |d| d.label.clone()),
                    value: v.value,
                    unit: d.map_or_else(String::new, |d| d.unit.clone()),
                }
            })
            .collect()
    }

    fn misses(&self) -> u32 {
        self.svc
            .as_ref()
            .and_then(|s| s.status().law)
            .map_or(0, |l| l.misses_in_a_row)
    }

    fn take_log(&mut self) -> Vec<(&'static str, String)> {
        std::mem::take(&mut self.log)
    }

    fn last_pulse_start(&self) -> Option<Instant> {
        // The age first, then the instant: the instant read after the age can
        // only make the start later, never earlier, so a countdown from it is
        // never short.
        let age = self.svc.as_ref()?.since_last_pulse_start_s()?;
        Instant::now().checked_sub(std::time::Duration::from_secs_f64(age))
    }
}
