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
    IdentFacts, IdentFigures, LawReading, Prediction, PulseFigures, Row, StationRef, TimeFigures,
    Trip, TripFacts,
};
use crate::protocol::{ErrorCode, MethodError};
use crate::pulse::rails::{Progress, Signals};
use crate::station::{self, class_name, NiScanner, StationPlan};
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::Backend;
use hertzforge_shock_common::{ShockRunMode, TrainSpec};
use hertzforge_shock_host::service::report::PulseOrigin;
use hertzforge_shock_host::{
    BackendPulseFirer, ClassicalLawFactory, ClassicalShockService, PulseRow, RunState, TripKind,
};
use serde_json::json;
use std::path::PathBuf;
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
        // The drive's peak as played comes with the closed loop's row.
        drive_vpk: f64::NAN,
        peak_g: p.peak_g,
        peak_ratio: p
            .peak_g
            .filter(|_| p.commanded_amplitude_g > 0.0)
            .map(|g| g / p.commanded_amplitude_g),
        error_pct: None,
        velocity_change_ips: p.velocity_change_ips,
        velocity_change_ratio: None,
        verdicts,
        passed: p.tolerance_overall,
        srs: match (p.srs_bands_total, p.srs_bands_pass) {
            (Some(t), Some(ok)) => Some((t, ok)),
            _ => None,
        },
        at_s: p.at_wall_time_s,
        law: None,
    }
}

/// A controller trip on the wire: the end it makes, its words and its trip.
pub fn end_of_trip(t: &hertzforge_shock_host::TripRecord) -> EngineEnd {
    let at = Some(t.at_level_db);
    let trip = |facts| {
        Some(Trip {
            facts,
            pulse: t.at_pulse,
            at_level_db: at,
        })
    };
    match &t.kind {
        TripKind::Tolerance {
            envelope_pass,
            velocity_change_pass,
            error_frac,
            worst_deviation_frac_of_peak,
            ..
        } => EngineEnd {
            outcome: "aborted",
            message: format!(
                "Pulse {} failed the tolerance at {} dB.",
                t.at_pulse + 1,
                crate::protocol::num(config::round(t.at_level_db, 2))
            ),
            trip: trip(TripFacts::Tolerance {
                envelope_pass: *envelope_pass,
                velocity_change_pass: *velocity_change_pass,
                error_pct: 100.0 * error_frac.abs(),
                worst_deviation_frac: *worst_deviation_frac_of_peak,
            }),
        },
        TripKind::Kinematic {
            quantity,
            value,
            limit,
            ..
        } => EngineEnd {
            outcome: "aborted",
            message: format!(
                "Pulse {} passed the shaker's {quantity}: {} against {}.",
                t.at_pulse + 1,
                crate::protocol::num(super::view::sig(*value, 3)),
                crate::protocol::num(super::view::sig(*limit, 3))
            ),
            trip: trip(TripFacts::Kinematic {
                axis: if quantity.contains("velocity") {
                    "velocity"
                } else {
                    "displacement"
                },
                value: *value,
                limit: *limit,
                predicted: false,
            }),
        },
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
    }
}

// ---------------------------------------------------------------------------------
// The controller's engine.
// ---------------------------------------------------------------------------------

type Service = ClassicalShockService<BackendPulseFirer<Box<dyn Backend>>>;

/// The controller's classical shock service over the station's backend.
pub struct ServiceEngine {
    ctx: EngineContext,
    svc: Option<Service>,
    cfg: Option<Arc<WireShock>>,
    reference: Option<Reference>,
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

    /// The rows and figures fired since the last look, and the end the
    /// controller's state says.
    fn collect(&mut self) -> Fired {
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
        out.end = match svc.state() {
            RunState::Aborted => svc.trip().map(end_of_trip).or(Some(EngineEnd {
                outcome: "fault",
                message: "The controller ended the run without a trip record.".into(),
                trip: None,
            })),
            RunState::Stopped if svc.status().ladder_completed => Some(EngineEnd {
                outcome: "completed",
                message: "The schedule finished.".into(),
                trip: None,
            }),
            _ => None,
        };
        out
    }

    /// One pulse's figures: the measured record from the pulse store, the
    /// reference at the level fired on the same record.
    fn figures_of(&self, svc: &Service, p: &PulseRow) -> PulseFigures {
        let measured = p
            .store_seq
            .and_then(|s| svc.pulse_store().get(s))
            .map(|r| r.time_history.clone());
        let scale = 10f64.powf(p.commanded_db / 20.0);
        let time = match &self.reference {
            Some(r) => {
                let mut f = r.figures(scale);
                f.measured = measured;
                f
            }
            None => TimeFigures {
                start: 0,
                fs_hz: svc.fs_hz(),
                reference: Vec::new(),
                band: None,
                measured,
            },
        };
        PulseFigures {
            time,
            drive: Vec::new(),
            velocity: None,
            displacement: None,
            srs: None,
            spectrum: None,
        }
    }
}

/// The controller's notes since `seen`, as log lines.
fn forward_log(seen: &mut usize, log: &mut Vec<(&'static str, String)>, svc: &Service) {
    let events = svc.log().events();
    for e in events.iter().skip(*seen) {
        if let hertzforge_shock_host::service::report::EventKind::Note { text } = &e.kind {
            log.push(("note", text.clone()));
        }
    }
    *seen = events.len();
}

impl ServiceEngine {}

impl ShockEngine for ServiceEngine {
    fn arm(&mut self, plan: ArmPlan) -> Result<ArmOutcome, MethodError> {
        let law = plan.law?;
        let mut cfg = plan.cfg.build();
        if plan.capture_root.is_some() {
            cfg.capture.auto_start = cfg.capture.auto_start && plan.capture_root.is_some();
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
        let firer = BackendPulseFirer::new(built.backend, plan.table.clone(), class)
            .map_err(arm_refused)?;
        let mut svc = ClassicalShockService::new(cfg, plan.table, firer, plan.capture_root)
            .map_err(arm_refused)?
            .with_cancel_flag(self.ctx.signals.cancel.clone())
            .with_law_factory(law);
        let progress = self.ctx.progress.clone();
        let speed = self.ctx.sim_speed;
        let fs = plan.cfg.fs_hz;
        let paced = !hardware && speed > 0.0;
        // On the simulator a fire is paced to `speed` times real time from its own
        // first block, as Random's is.
        let mut t0: Option<Instant> = None;
        svc.set_block_observer(Box::new(move |done, of| {
            progress.block(done as u64, of as u64);
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
        // The controller's identification (band-limited random, one cancellable
        // fire, the coherence hold) arrives with its closed loop.
        Err(fault(
            "identification is not available from this controller",
        ))
    }

    fn seed(&mut self) -> Result<Seeded, MethodError> {
        Err(fault("the control law cannot be seeded by this controller"))
    }

    fn fire(&mut self) -> Result<Fired, MethodError> {
        let go = self.ctx.signals.go.clone();
        let r = self.svc()?.fire_next_pulse_cancellable(Some(&go));
        match r {
            Ok(_) | Err(hertzforge_shock_host::ServiceError::Cancelled { .. }) => {}
            Err(e) => return Err(fault(e)),
        }
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
        self.svc()?
            .fire_train_cancellable(TrainSpec::delayed(count, spacing_s), Some(&go))
            .map_err(fault)?;
        Ok(self.collect())
    }

    fn burst(&mut self) -> Result<Fired, MethodError> {
        let go = self.ctx.signals.go.clone();
        self.svc()?
            .fire_gunfire_burst_cancellable(Some(&go))
            .map_err(fault)?;
        Ok(self.collect())
    }

    fn set_level(&mut self, to: LevelMove) -> Result<f64, MethodError> {
        let svc = self.svc()?;
        let r = match to {
            LevelMove::Step(d) if d > 0.0 => svc.step_up(),
            LevelMove::Step(_) => svc.step_down(),
            LevelMove::Rung(i) => svc.jump_to_level(i),
        };
        r.map_err(|e| MethodError::named(ErrorCode::Refused, "wrong_state", e.to_string()))
    }

    fn predicted_vpk_at(&self, _level_db: f64) -> Option<f64> {
        // The identified drive at a level is the closed loop's prediction.
        None
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
            passed: svc
                .report_data()
                .pulses
                .iter()
                .filter(|p| p.tolerance_overall == Some(true))
                .count() as u64,
        }
    }

    fn identification(&self) -> Option<IdentFigures> {
        None
    }

    fn law_values(&self) -> Vec<LawReading> {
        Vec::new()
    }

    fn misses(&self) -> u32 {
        0
    }

    fn take_log(&mut self) -> Vec<(&'static str, String)> {
        std::mem::take(&mut self.log)
    }
}
