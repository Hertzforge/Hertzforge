//! The seam between the run thread and whatever plays the records: [`TwrEngine`].
//!
//! The run thread ([`super::run`]) owns the order of things on the wire (arm, start,
//! review, the plays between verbs, the pause while nobody holds control, stop and
//! abort, every publish); an engine owns the plays themselves. The engine a run
//! uses in service is [`ServiceEngine`], the controller's `TwrService` over the
//! station's backend: identification, the inverse, the law, the guards, the ladder
//! and every judgement are the controller's. Wire tests that exercise the protocol
//! without a controller run give the service [`super::sim::SimulatedEngine`]
//! instead, as the shock wire tests give theirs.

use super::config::{excitation_name, WireTwr};
use super::reference::Stored;
use super::view::{
    IdentFacts, IdentFigures, MaskFigures, PlayRecords, PlayRow, Prediction, Spectra, StationRef,
    Trip, TripFacts,
};
use crate::protocol::{num, ErrorCode, MethodError};
use crate::pulse::observe::{Observed, Watch};
use crate::pulse::rails::{Progress, Signals};
use crate::shock::config::LawFacts;
use crate::shock::view::sig;
use crate::station::{self, class_name, NiScanner, StationPlan};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_twr::control::PlayKind;
use hertzforge_twr::grid::rfft_half;
use hertzforge_twr::player::{BackendReferencePlayer, LagSource, PlayProgress, SampleRing};
use hertzforge_twr::service::Advanced;
use hertzforge_twr::{TripKind, TwrError, TwrLawFactory, TwrService, TwrState, TwrTrip};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};

/// What a run arms on, taken from the service when the request arrives.
pub struct ArmPlan {
    pub cfg: Arc<WireTwr>,
    /// The stored reference the run plays.
    pub stored: Arc<Stored>,
    pub facts: LawFacts,
    pub table: ChannelTable,
    pub station: StationPlan,
    pub scanner: Arc<dyn NiScanner>,
    /// The service's local data directory: the hardware lock lives here.
    pub data_root: PathBuf,
    /// Where the run's records are kept (`None`: the controller's local default).
    pub capture_root: Option<PathBuf>,
    pub bind: String,
    /// The run's law, resolved by the service from the catalog.
    pub law: Result<Arc<dyn TwrLawFactory>, MethodError>,
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
    /// The control channel's display tap, drained while a play is in flight.
    pub tap: Option<Arc<Mutex<SampleRing>>>,
    pub units: String,
}

/// How an arm ended.
pub enum ArmOutcome {
    Armed(Box<Armed>),
    /// Feasibility exceeded and no acknowledgement was given.
    Blocked((&'static str, String)),
}

/// The engine ended the run: its outcome, words and trip.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineEnd {
    pub outcome: &'static str,
    pub message: String,
    pub trip: Option<Trip>,
}

/// How identification ended.
pub enum Identified {
    /// Passed: hold for review, or go on.
    Done { facts: IdentFacts, review: bool },
    /// The coherence hold: nothing drives until a reset.
    Held {
        facts: Option<IdentFacts>,
        message: String,
    },
    /// A protection ended the run (the coverage floor, the drive ceiling).
    Ended {
        facts: Option<IdentFacts>,
        end: EngineEnd,
    },
}

/// How proceeding ended.
pub enum Proceeded {
    Ready(Option<Prediction>),
    Ended(EngineEnd),
}

/// What one play did.
#[derive(Default)]
pub struct Played {
    pub row: Option<PlayRow>,
    pub records: Option<PlayRecords>,
    /// The engine ended the run on it.
    pub end: Option<EngineEnd>,
    /// The play was cancelled after this many frames.
    pub cancelled_at: Option<u64>,
}

/// A level move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LevelMove {
    /// One step, in dB.
    Step(f64),
    /// A level of the table, from 0.
    Rung(usize),
}

/// Where the run is.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Position {
    pub level_db: f64,
    pub rung: Option<usize>,
    pub at_level: u32,
    pub of_level: u32,
    pub playback: u32,
    pub of_playback: u32,
    /// The present level has gated.
    pub gated: bool,
    /// The next play is a playback play.
    pub playing: bool,
    /// A level failed its gate under `hold`.
    pub holding: bool,
    pub frozen: bool,
    pub alarm: bool,
}

/// What a run's engine does. Every call is made on the run thread; a play blocks
/// it for the whole record and reads [`Signals::cancel`] at every block.
pub trait TwrEngine: Send {
    /// Feasibility, the station and the record at the station's rate. Nothing is
    /// driven.
    fn arm(&mut self, plan: ArmPlan) -> Result<ArmOutcome, MethodError>;
    /// Identification: one cancellable play.
    fn identify(&mut self) -> Result<Identified, MethodError>;
    /// Build the law and seed it.
    fn proceed(&mut self) -> Result<Proceeded, MethodError>;
    /// One play at the present level: a correction play, or a playback play once
    /// playback has begun.
    fn play(&mut self) -> Result<Played, MethodError>;
    /// Begin playback from a level that has gated.
    fn begin_playback(&mut self) -> Result<(), MethodError>;
    /// Move the level; the new level, dB.
    fn set_level(&mut self, to: LevelMove) -> Result<f64, MethodError>;
    /// The drive in force's peak at `level_db`, volts, once seeded.
    fn predicted_vpk_at(&self, level_db: f64) -> Option<f64>;
    /// Hold the law's corrections off, or resume them.
    fn freeze(&mut self, frozen: bool);
    /// The controlled stop: nothing more plays.
    fn stop(&mut self);
    /// The emergency stop.
    fn abort(&mut self, reason: &str);
    fn position(&self) -> Position;
    fn identification(&self) -> Option<IdentFigures>;
    fn mask(&self) -> Option<MaskFigures>;
    /// The level target now, for a plot that shows it before a play.
    fn level_target(&self) -> Option<PlayRecords>;
    /// Unusable law calls in a row now.
    fn misses(&self) -> u32;
    /// Frames the station has clocked over every play and pad since the run armed.
    fn frames_clocked(&self) -> u64;
    /// Log lines since the last call, driver warnings among them.
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
pub type EngineFactory = Arc<dyn Fn(&EngineContext) -> Box<dyn TwrEngine> + Send + Sync>;

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
    MethodError::internal(format!("the TWR service failed: {words}"))
}

/// A sentence from the controller's words: a capital first letter and a full
/// stop.
pub fn sentence(s: &str) -> String {
    let t = s.trim().trim_end_matches('.');
    let mut c = t.chars();
    match c.next() {
        Some(f) => format!("{}{}.", f.to_uppercase(), c.as_str()),
        None => String::new(),
    }
}

/// The display tap's length, samples of the control channel.
pub const TAP_SAMPLES: usize = 8192;

type Service = TwrService<BackendReferencePlayer<Observed>>;

/// The controller's TWR service over the station's backend.
pub struct ServiceEngine {
    ctx: EngineContext,
    svc: Option<Service>,
    cfg: Option<Arc<WireTwr>>,
    units: String,
    log: Vec<(&'static str, String)>,
    log_seen: usize,
    capture_root: Option<PathBuf>,
    capturing: bool,
    #[cfg(test)]
    without_law: bool,
}

impl ServiceEngine {
    pub fn new(ctx: EngineContext) -> ServiceEngine {
        ServiceEngine {
            ctx,
            svc: None,
            cfg: None,
            units: "g".into(),
            log: Vec::new(),
            log_seen: 0,
            capture_root: None,
            capturing: false,
            #[cfg(test)]
            without_law: false,
        }
    }

    /// A test engine whose run is the core's own iteration, without the law seam.
    #[cfg(test)]
    pub fn without_law(ctx: EngineContext) -> ServiceEngine {
        let mut e = ServiceEngine::new(ctx);
        e.without_law = true;
        e
    }

    fn svc(&mut self) -> Result<&mut Service, MethodError> {
        self.svc
            .as_mut()
            .ok_or_else(|| fault("the run is not armed"))
    }

    fn forward_log(&mut self) {
        let Some(svc) = self.svc.as_mut() else {
            return;
        };
        let events = svc.event_log();
        for e in events.iter().skip(self.log_seen) {
            // The core stamps its own clock in front: the log has its own time.
            let text = match e.find("] ") {
                Some(i) if e.starts_with('[') => &e[i + 2..],
                _ => e.as_str(),
            };
            self.log.push(("note", text.to_owned()));
        }
        self.log_seen = events.len();
        for w in svc.take_driver_warnings() {
            self.log.push(("driver", w));
        }
    }

    /// The end a terminal core state means, with its words: completion, or the
    /// core's own trip by its kind.
    fn ended(&self) -> Option<EngineEnd> {
        let svc = self.svc.as_ref()?;
        let st = svc.status();
        match st.state {
            TwrState::Complete => Some(EngineEnd {
                outcome: "completed",
                message: format!(
                    "The playback schedule finished: {} play{} at {}.",
                    st.plays_completed,
                    if st.plays_completed == 1 { "" } else { "s" },
                    level_words(st.level_db)
                ),
                trip: None,
            }),
            TwrState::Aborted => {
                let message = sentence(st.abort_reason.as_deref().unwrap_or("the run aborted"));
                Some(match st.trip.as_ref() {
                    Some(t) => end_of_trip(t, message),
                    None => EngineEnd {
                        outcome: "aborted",
                        message,
                        trip: None,
                    },
                })
            }
            _ => None,
        }
    }

    fn ident_facts(&self, passed: bool) -> Option<IdentFacts> {
        let svc = self.svc.as_ref()?;
        let cfg = svc.config();
        let st = svc.status();
        let fs = st.actual_fs_hz.unwrap_or(svc.grid().fs_hz);
        let peak = {
            let mut x = cfg.sysid.build_excitation(fs);
            cfg.limits.apply_drive_clip(&mut x);
            x.iter().fold(0.0f64, |m, &v| m.max(v.abs()))
        };
        let lag = svc.lag_frames();
        let source = match svc.lag_provenance() {
            Some(LagSource::Measured) => "measured",
            _ => "derived",
        };
        Some(IdentFacts {
            excitation: excitation_name(cfg.sysid.excitation),
            drive_vrms: cfg.sysid.drive_rms,
            drive_vpk: peak,
            coherence_mean: st.sysid_mean_coherence.unwrap_or(f64::NAN),
            min_mean_coherence: cfg.sysid.min_mean_coherence,
            coverage: st.mask_energy_coverage,
            lag_frames: lag.map(|l| l as f64),
            lag_source: source,
            passed,
        })
    }

    /// The level target at `level_db`.
    fn target_at(&self, level_db: f64) -> Vec<f64> {
        let Some(svc) = self.svc.as_ref() else {
            return Vec::new();
        };
        let now = svc.status().level_db;
        let k = 10f64.powf((level_db - now) / 20.0);
        svc.target_time().iter().map(|v| v * k).collect()
    }

    fn band(&self) -> (f64, f64) {
        self.svc.as_ref().map_or((0.0, 0.0), |s| {
            (s.config().law.f_lo_hz, s.config().law.f_hi_hz)
        })
    }

    fn records(&self, level_db: f64) -> Option<PlayRecords> {
        let svc = self.svc.as_ref()?;
        let response = svc.achieved_time().to_vec();
        if response.is_empty() {
            return None;
        }
        let target = self.target_at(level_db);
        let drive = svc.drive_time().to_vec();
        let grid = svc.grid();
        let (lo, hi) = self.band();
        let spectra = (target.len() == grid.n && drive.len() == grid.n).then(|| {
            let mag = |x: &[f64]| -> Vec<f64> {
                rfft_half(x, grid.n)
                    .iter()
                    .map(|&(re, im)| (re * re + im * im).sqrt())
                    .collect()
            };
            let (r, y, d) = (mag(&target), mag(&response), mag(&drive));
            let keep: Vec<usize> = (0..r.len())
                .filter(|&k| {
                    let f = grid.freq(k);
                    f >= lo && f <= hi
                })
                .collect();
            let pick = |v: &[f64]| keep.iter().map(|&k| v[k]).collect::<Vec<f64>>();
            Spectra {
                freqs_hz: keep.iter().map(|&k| grid.freq(k)).collect(),
                target: pick(&r),
                response: pick(&y),
                drive: pick(&d),
            }
        });
        let l = &svc.config().limits;
        Some(PlayRecords {
            fs_hz: grid.fs_hz,
            target,
            response,
            drive,
            alarm_frac: l.envelope_alarm_frac,
            abort_frac: l.envelope_abort_frac,
            spectra,
        })
    }

    fn start_capture_if_asked(&mut self) {
        if self.capturing {
            return;
        }
        let auto = self
            .svc
            .as_ref()
            .is_some_and(|s| s.config().capture.auto_start);
        if !auto {
            return;
        }
        let root = self
            .capture_root
            .clone()
            .unwrap_or_else(hertzforge_capture::default_data_root);
        if let Some(svc) = self.svc.as_mut() {
            match svc.start_capture(&root) {
                Ok(dir) => {
                    self.capturing = true;
                    self.log
                        .push(("note", format!("recording to {}", dir.display())));
                }
                Err(e) => self
                    .log
                    .push(("note", format!("the recording did not start: {e}"))),
            }
        }
    }

    fn finish_capture(&mut self) {
        if self.capturing {
            if let Some(svc) = self.svc.as_mut() {
                let _ = svc.stop_capture();
            }
            self.capturing = false;
        }
    }
}

/// A level in words: "full level" at 0 dB, else its dB.
pub fn level_words(level_db: f64) -> String {
    if level_db.abs() < 1e-9 {
        "full level".into()
    } else {
        format!("{} dB", num(crate::shock::config::round(level_db, 2)))
    }
}

/// The end a core trip makes, with its facts on the wire.
pub fn end_of_trip(t: &TwrTrip, message: String) -> EngineEnd {
    let trip = |facts| {
        Some(Trip {
            facts,
            play: t.play,
            at_level_db: Some(t.at_level_db),
        })
    };
    let (outcome, trip) = match &t.kind {
        TripKind::Envelope {
            in_play,
            worst_frac,
            abort_frac,
            samples,
            at_sample,
        } => (
            "aborted",
            trip(TripFacts::Envelope {
                in_play: *in_play,
                worst_frac: *worst_frac,
                abort_frac: *abort_frac,
                samples: *samples as u64,
                at_sample: *at_sample as u64,
            }),
        ),
        TripKind::DriveVpk {
            vpk,
            ceiling_vpk,
            predicted,
        } => (
            "aborted",
            trip(TripFacts::DriveVpk {
                vpk: *vpk,
                ceiling_vpk: *ceiling_vpk,
                predicted: *predicted,
            }),
        ),
        TripKind::DriveRms { vrms, limit_vrms } => (
            "aborted",
            trip(TripFacts::DriveRms {
                vrms: *vrms,
                limit_vrms: *limit_vrms,
            }),
        ),
        TripKind::ResponseRms { rms, limit } => (
            "aborted",
            trip(TripFacts::ResponseRms {
                rms: *rms,
                limit: *limit,
            }),
        ),
        TripKind::Coverage {
            coverage,
            min_band_energy,
            error_floor_pct,
        } => (
            "aborted",
            trip(TripFacts::Coverage {
                coverage: *coverage,
                min_band_energy: *min_band_energy,
                error_floor_pct: *error_floor_pct,
            }),
        ),
        TripKind::Divergence { plays, error_pct } => (
            "aborted",
            trip(TripFacts::Divergence {
                plays: *plays as u64,
                error_pct: *error_pct,
            }),
        ),
        TripKind::NotConverged {
            iterations,
            error_pct,
            gate_pct,
        } => (
            "not_converged",
            trip(TripFacts::NotConverged {
                iterations: u64::from(*iterations),
                error_pct: *error_pct,
                gate_pct: *gate_pct,
            }),
        ),
        TripKind::Alignment {
            lag_frames,
            identification_lag_frames,
            drift_frames,
            limit_frames,
        } => (
            "aborted",
            trip(TripFacts::Alignment {
                lag_frames: *lag_frames,
                identification_lag_frames: *identification_lag_frames,
                drift_frames: *drift_frames,
                limit_frames: *limit_frames,
            }),
        ),
        TripKind::LawFault {
            reason,
            consecutive,
        } => (
            "law_fault",
            trip(TripFacts::LawFault {
                reason: reason.reason(),
                consecutive: u64::from(*consecutive),
            }),
        ),
        TripKind::FirstDrive { .. } => (
            "law_fault",
            trip(TripFacts::LawFault {
                reason: "first_drive",
                consecutive: 1,
            }),
        ),
        TripKind::NoResponse => ("fault", None),
    };
    EngineEnd {
        outcome,
        message,
        trip,
    }
}

/// A core play row on the wire.
pub fn row_of(p: &hertzforge_twr::PlayRow) -> PlayRow {
    PlayRow {
        index: p.index,
        playback: p.kind == PlayKind::Playback,
        level_db: p.level_db,
        rung: p.rung,
        iteration: p.iteration,
        rms_error_pct: p.metrics.rms_error_pct,
        peak_error_pct: p.metrics.peak_error_pct,
        spectral_error_db: p.metrics.spectral_error_db,
        gated: p.gated,
        clipped_samples: p.clipped_samples as u64,
        clamped_bins: p.clamped_bins as u64,
        drive_vpk: p.drive_vpk,
        response_peak: p.response_peak,
        lag_frames: p.lag_frames,
        drift_frames: p.drift_frames,
        start_frame: p.start_frame,
        frames: p.frames,
        at_s: p.at_s,
        law: None,
    }
}

impl TwrEngine for ServiceEngine {
    fn arm(&mut self, plan: ArmPlan) -> Result<ArmOutcome, MethodError> {
        let law = plan.law?;
        #[allow(unused_mut)]
        let mut cfg = plan.cfg.build(&plan.stored, &plan.facts);
        #[cfg(test)]
        if self.without_law {
            cfg.control = None;
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
        self.units = plan
            .table
            .channels
            .iter()
            .find(|c| c.enabled && c.role == ChannelRole::Control)
            .map_or_else(|| "g".to_owned(), |c| crate::table::unit_name(&c.units));
        let own = Arc::new(AtomicBool::new(false));
        let watch = Watch {
            asked: self.ctx.signals.cancel.clone(),
            own: own.clone(),
            // The play's progress is the player's own (record and rest pad).
            progress: Arc::new(Progress::default()),
            expected: Arc::new(AtomicU64::new(0)),
            speed: if hardware { 0.0 } else { self.ctx.sim_speed },
            abort: Some(self.ctx.signals.abort.clone()),
        };
        let progress = self.ctx.progress.clone();
        let observer = Arc::new(move |p: PlayProgress| {
            progress.block(p.frames_done as u64, p.frames_expected as u64);
        });
        let player = BackendReferencePlayer::new(
            Observed::new(built.backend, watch),
            plan.table.clone(),
            class,
        )
        .map_err(arm_refused)?
        .with_cancel_flag(own)
        .with_display_tap(TAP_SAMPLES)
        .with_play_observer(observer);
        let tap = player.display_tap_handle();
        let mut svc = TwrService::new(player, cfg)
            .map_err(arm_refused)?
            .with_law_factory(law);
        let feas =
            super::reference::feasibility(svc.feasibility(), plan.cfg.shape.feasibility.any());
        let verdict = (feas.verdict, feas.message.clone());
        if svc.state() == TwrState::FeasibilityBlocked {
            match &plan.ack {
                Some(a) => svc.arm_override(a).map_err(arm_refused)?,
                None => return Ok(ArmOutcome::Blocked(verdict)),
            }
        }
        let grid = svc.grid().clone();
        self.cfg = Some(plan.cfg.clone());
        self.capture_root = plan.capture_root.clone();
        self.svc = Some(svc);
        self.forward_log();
        Ok(ArmOutcome::Armed(Box::new(Armed {
            fs_hz: grid.fs_hz,
            record_len: grid.n,
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
            tap,
            units: self.units.clone(),
        })))
    }

    fn identify(&mut self) -> Result<Identified, MethodError> {
        let review = self
            .cfg
            .as_deref()
            .ok_or_else(|| fault("the run is not armed"))?
            .review;
        self.ctx.progress.start(0);
        let r = self.svc()?.advance();
        self.ctx.progress.end();
        self.forward_log();
        let state = self.svc()?.state();
        match (r, state) {
            (_, TwrState::CoherenceHold) => {
                let min = self
                    .svc
                    .as_ref()
                    .map_or(0.0, |s| s.config().sysid.min_mean_coherence);
                Ok(Identified::Held {
                    facts: self.ident_facts(false),
                    message: format!(
                        "Identification did not reach its coherence ({} in-band mean); raise its \
                         drive or its length, or check the response path, then reset.",
                        num(min)
                    ),
                })
            }
            (Err(TwrError::Cancelled { .. }), _) | (_, TwrState::Stopped) => {
                // The run thread ends the run on its signal.
                Err(fault("identification was stopped"))
            }
            (_, TwrState::Aborted) => Ok(Identified::Ended {
                facts: self.ident_facts(false),
                end: self
                    .ended()
                    .ok_or_else(|| fault("the run ended without a reason"))?,
            }),
            (Err(e), _) => Err(fault(e)),
            (Ok(Advanced::SysId { .. }), _) => Ok(Identified::Done {
                facts: self
                    .ident_facts(true)
                    .ok_or_else(|| fault("identification left no facts"))?,
                review,
            }),
            (Ok(other), s) => Err(fault(format!("identification ended in {s:?} ({other:?})"))),
        }
    }

    fn proceed(&mut self) -> Result<Proceeded, MethodError> {
        // The core seeds its drive at the end of identification today; once it
        // holds at review itself, this is where the law is built and seeded.
        if let Some(end) = self.ended() {
            return Ok(Proceeded::Ended(end));
        }
        self.start_capture_if_asked();
        let level = self.position().level_db;
        Ok(Proceeded::Ready(self.predicted_vpk_at(level).map(|vpk| {
            Prediction {
                drive_vpk: vpk,
                drive_vpk_ceiling: self.cfg.as_ref().map_or(0.0, |c| c.drive_vpk_ceiling),
                level_db: level,
            }
        })))
    }

    fn play(&mut self) -> Result<Played, MethodError> {
        let rows_before = self.svc()?.plays().len();
        self.start_capture_if_asked();
        self.ctx.progress.start(0);
        let r = self.svc()?.advance();
        self.ctx.progress.end();
        self.forward_log();
        if let Err(TwrError::Cancelled { frames_done, .. }) = &r {
            return Ok(Played {
                cancelled_at: Some(*frames_done as u64),
                ..Played::default()
            });
        }
        if let Err(e) = &r {
            if !matches!(e, TwrError::Aborted(_)) {
                return Err(fault(e));
            }
        }
        let svc = self
            .svc
            .as_ref()
            .ok_or_else(|| fault("the run lost its station"))?;
        let mut played = Played::default();
        if let Some(p) = svc.plays().get(rows_before) {
            let row = row_of(p);
            played.records = self.records(row.level_db);
            played.row = Some(row);
        }
        played.end = self.ended();
        if played.end.is_some() {
            self.finish_capture();
        }
        Ok(played)
    }

    fn begin_playback(&mut self) -> Result<(), MethodError> {
        self.svc()?.begin_playback().map_err(|e| {
            MethodError::named(
                ErrorCode::Refused,
                "not_converged",
                format!(
                    "Playback begins once the present level has gated ({}).",
                    sentence(&e.to_string()).trim_end_matches('.')
                ),
            )
        })
    }

    fn set_level(&mut self, to: LevelMove) -> Result<f64, MethodError> {
        let ceiling = self.cfg.as_ref().map_or(0.0, |c| c.drive_vpk_ceiling);
        let target = match to {
            LevelMove::Step(d) => self.position().level_db + d,
            LevelMove::Rung(i) => self
                .svc
                .as_ref()
                .and_then(|s| s.config().schedule.table_db.get(i).copied())
                .unwrap_or(f64::NAN),
        };
        let predicted = self.predicted_vpk_at(target);
        let svc = self.svc()?;
        let r = match to {
            LevelMove::Step(d) if d > 0.0 => svc.step_level_up(),
            LevelMove::Step(_) => svc.step_level_down(),
            LevelMove::Rung(i) => svc.jump_to_level(i),
        };
        self.forward_log();
        match r {
            Ok(()) => Ok(self.position().level_db),
            Err(e) => {
                let words = e.to_string();
                if words.contains("refused") && words.contains("ceiling") {
                    Err(MethodError::named(
                        ErrorCode::Refused,
                        "drive_ceiling",
                        format!(
                            "The drive in force would pass the drive ceiling of {} V pk at {}; \
                             the level and the drive are unchanged.",
                            num(ceiling),
                            level_words(target)
                        ),
                    )
                    .with_detail(json!({
                        "vpk": predicted.map(|v| num(sig(v, 4))),
                        "ceiling_vpk": num(ceiling),
                        "level_db": num(crate::shock::config::round(target, 2)),
                    })))
                } else {
                    Err(MethodError::named(
                        ErrorCode::Refused,
                        "wrong_state",
                        sentence(&words),
                    ))
                }
            }
        }
    }

    fn predicted_vpk_at(&self, _level_db: f64) -> Option<f64> {
        // The drive in force is the core's; until it states its peak, there is no
        // prediction to give.
        None
    }

    fn freeze(&mut self, frozen: bool) {
        if let Some(svc) = self.svc.as_mut() {
            svc.freeze_adaptation(frozen);
        }
        self.forward_log();
    }

    fn stop(&mut self) {
        if let Some(svc) = self.svc.as_mut() {
            let _ = svc.stop();
        }
        self.finish_capture();
        self.forward_log();
    }

    fn abort(&mut self, reason: &str) {
        if let Some(svc) = self.svc.as_mut() {
            svc.abort(reason);
        }
        self.finish_capture();
        self.forward_log();
    }

    fn position(&self) -> Position {
        let Some(svc) = self.svc.as_ref() else {
            return Position::default();
        };
        let st = svc.status();
        let schedule = &svc.config().schedule;
        Position {
            level_db: st.level_db,
            rung: st.rung,
            at_level: st.iterations_at_level,
            of_level: schedule.max_iterations_per_level,
            playback: st.plays_completed,
            of_playback: st.plays_target,
            gated: st.converged_at_level,
            playing: st.state == TwrState::Playing,
            holding: st.state == TwrState::Holding,
            frozen: st.adapt_frozen,
            alarm: st.alarm_active,
        }
    }

    fn identification(&self) -> Option<IdentFigures> {
        let svc = self.svc.as_ref()?;
        let cfg = svc.config();
        let mag = svc.frf_mag();
        if mag.is_empty() {
            return None;
        }
        let coh = svc.coherence();
        let phase = svc.frf_phase_deg();
        let (lo, hi) = (cfg.sysid.f_lo_hz, cfg.sysid.f_hi_hz);
        let mut f = IdentFigures {
            freqs_hz: Vec::new(),
            gain: Vec::new(),
            phase_deg: Vec::new(),
            coherence: Vec::new(),
            coherence_floor: cfg.inverse.coherence_floor,
            min_mean_coherence: cfg.sysid.min_mean_coherence,
            unit: format!("{}/V", self.units),
        };
        for (k, &(fk, g)) in mag.iter().enumerate() {
            if fk < lo || fk > hi {
                continue;
            }
            f.freqs_hz.push(fk);
            f.gain.push(g);
            f.phase_deg.push(phase.get(k).map_or(0.0, |p| p.1));
            f.coherence.push(coh.get(k).map_or(0.0, |c| c.1));
        }
        Some(f)
    }

    fn mask(&self) -> Option<MaskFigures> {
        let svc = self.svc.as_ref()?;
        let mask = svc.trust_mask();
        if mask.is_empty() {
            return None;
        }
        let (lo, hi) = self.band();
        let reference = svc.target_spectrum_mag();
        let peak = reference
            .iter()
            .map(|r| r.1)
            .fold(0.0f64, f64::max)
            .max(f64::MIN_POSITIVE);
        let mut f = MaskFigures {
            freqs_hz: Vec::new(),
            trusted: Vec::new(),
            reference: Vec::new(),
        };
        // The correction band and an octave either side of it.
        for (k, &(fk, m)) in mask.iter().enumerate() {
            if fk < lo / 2.0 || fk > 2.0 * hi || k == 0 {
                continue;
            }
            f.freqs_hz.push(fk);
            f.trusted.push(m);
            f.reference
                .push(reference.get(k).map_or(0.0, |r| r.1) / peak);
        }
        Some(f)
    }

    fn level_target(&self) -> Option<PlayRecords> {
        let svc = self.svc.as_ref()?;
        let l = &svc.config().limits;
        Some(PlayRecords {
            fs_hz: svc.grid().fs_hz,
            target: svc.target_time(),
            response: Vec::new(),
            drive: Vec::new(),
            alarm_frac: l.envelope_alarm_frac,
            abort_frac: l.envelope_abort_frac,
            spectra: None,
        })
    }

    fn misses(&self) -> u32 {
        0
    }

    fn frames_clocked(&self) -> u64 {
        self.svc.as_ref().map_or(0, |s| s.station_frames())
    }

    fn take_log(&mut self) -> Vec<(&'static str, String)> {
        std::mem::take(&mut self.log)
    }
}
