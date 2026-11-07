//! A simulated classical shock engine for wire tests: the protocol's states, verbs,
//! timings and refusals without the controller's service, as the MIMO wire tests
//! run on their simulated driver. A run never uses it unless the service is given
//! it ([`crate::server::ServiceOptions::shock_engine`]).
//!
//! The plant is a wire at [`GAIN_V_PER_G`]: every pulse measures its reference at
//! the level fired, a hair low, and passes. Identification takes its configured time
//! at the given pace; a fire takes its record's. The ladder is the controller's own
//! (`ShockLadderState`), so levels, steps, rungs and completion are the
//! controller's arithmetic.

use super::config::WireShock;
use super::engine::{
    ArmOutcome, ArmPlan, Armed, EngineContext, EngineEnd, Fired, Identified, LevelMove, Position,
    Seeded, ShockEngine,
};
use super::preview::{self, Reference};
use super::view::{
    IdentFacts, IdentFigures, LawCall, LawReading, Prediction, PulseFigures, Row, StationRef, Trip,
    TripFacts,
};
use crate::protocol::{ErrorCode, MethodError};
use hertzforge_shock_common::{
    pulse_feasibility, FeasibilityTolerances, ShockLadderState, ShockRunMode,
};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The simulated plant's drive per g of response.
pub const GAIN_V_PER_G: f64 = 0.02;
/// Each pulse measures its reference times this.
const MEASURED_RATIO: f64 = 0.998;

/// The simulated engine.
pub struct SimulatedEngine {
    ctx: EngineContext,
    cfg: Option<Arc<WireShock>>,
    reference: Option<Reference>,
    ladder: Option<ShockLadderState>,
    vpk_full: Option<f64>,
    identified: bool,
    fired: u64,
    passed: u64,
    armed_at: Option<Instant>,
    log: Vec<(&'static str, String)>,
}

impl SimulatedEngine {
    pub fn new(ctx: EngineContext) -> SimulatedEngine {
        SimulatedEngine {
            ctx,
            cfg: None,
            reference: None,
            ladder: None,
            vpk_full: None,
            identified: false,
            fired: 0,
            passed: 0,
            armed_at: None,
            log: Vec::new(),
        }
    }

    fn cfg(&self) -> Result<&WireShock, MethodError> {
        self.cfg
            .as_deref()
            .ok_or_else(|| MethodError::internal("the simulated run is not armed"))
    }

    /// Sleep `s` simulated seconds at the context's pace, in short steps; false
    /// when the run's cancel was raised.
    fn pace(&self, s: f64) -> bool {
        if self.ctx.sim_speed <= 0.0 || s <= 0.0 {
            return !self.ctx.signals.cancelled();
        }
        let end = Instant::now() + Duration::from_secs_f64(s / self.ctx.sim_speed);
        while Instant::now() < end {
            if self.ctx.signals.cancelled() {
                return false;
            }
            std::thread::sleep(
                end.saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(10)),
            );
        }
        !self.ctx.signals.cancelled()
    }

    /// One pulse at the present level.
    fn one(&mut self, origin: &'static str, law: bool) -> Result<(Row, PulseFigures), MethodError> {
        let (amplitude, ceiling, fs) = {
            let c = self.cfg()?;
            (c.amplitude_g, c.drive_vpk_ceiling, c.fs_hz)
        };
        let vpk_full = self
            .vpk_full
            .ok_or_else(|| MethodError::internal("the simulated law is not seeded"))?;
        let level = self
            .ladder
            .as_ref()
            .ok_or_else(|| MethodError::internal("the simulated run has no ladder"))?
            .current_level_db();
        let scale = 10f64.powf(level / 20.0);
        let wanted = vpk_full * scale;
        let limit_scale = if wanted > ceiling {
            ceiling / wanted
        } else {
            1.0
        };
        let reference = self
            .reference
            .as_ref()
            .ok_or_else(|| MethodError::internal("the simulated run has no reference"))?;
        let n = reference.record_len as u64;
        self.ctx.progress.start(n);
        let blocks = 8u64;
        for b in 1..=blocks {
            // A pulse in flight plays out whole: the committed queue is longer.
            let _ = self.pace(reference.record_len as f64 / fs / blocks as f64);
            self.ctx.progress.block(n * b / blocks, n);
        }
        self.ctx.progress.end();
        let commanded = scale * limit_scale;
        let mut figures = reference.figures(commanded);
        figures.measured = Some(
            figures
                .reference
                .iter()
                .map(|v| v * MEASURED_RATIO)
                .collect(),
        );
        let drive: Vec<f64> = figures.reference.iter().map(|v| v * GAIN_V_PER_G).collect();
        let (vel, disp) = reference.kinematics(commanded);
        let index = self.fired;
        self.fired += 1;
        self.passed += 1;
        if let Some(l) = self.ladder.as_mut() {
            l.on_pulse_fired();
        }
        let row = Row {
            index,
            origin,
            scheduled_db: level,
            commanded_db: level + 20.0 * limit_scale.log10(),
            limit_scale,
            drive_vpk: wanted * limit_scale,
            peak_g: Some(amplitude * commanded * MEASURED_RATIO),
            peak_ratio: Some(MEASURED_RATIO),
            error_pct: Some(100.0 * (1.0 - MEASURED_RATIO)),
            velocity_change_ips: Some(
                reference.facts.velocity_change_ips * commanded * MEASURED_RATIO,
            ),
            velocity_change_ratio: Some(MEASURED_RATIO),
            verdicts: Some((true, true, true)),
            passed: Some(true),
            srs: Some((21, 21)),
            at_s: self.armed_at.map_or(0.0, |t| t.elapsed().as_secs_f64()),
            law: law.then_some(LawCall {
                used: true,
                miss: None,
                capped_bins: 0,
                peak_capped: false,
                call_ms: 0.1,
            }),
        };
        let figures = PulseFigures {
            drive,
            velocity: Some((vel.iter().map(|v| v * MEASURED_RATIO).collect(), vel)),
            displacement: Some((disp.iter().map(|v| v * MEASURED_RATIO).collect(), disp)),
            time: figures,
            srs: None,
            spectrum: None,
        };
        self.log.push((
            "pulse",
            format!(
                "pulse {} at {} dB: {} g, passed",
                index + 1,
                crate::protocol::num(super::config::round(level, 2)),
                crate::protocol::num(super::config::round(
                    amplitude * commanded * MEASURED_RATIO,
                    3
                ))
            ),
        ));
        Ok((row, figures))
    }

    fn completed(&self) -> Option<EngineEnd> {
        let auto = self
            .cfg
            .as_ref()
            .is_some_and(|c| c.mode == ShockRunMode::Auto);
        (auto && self.ladder.as_ref().is_some_and(|l| l.completed())).then(|| EngineEnd {
            outcome: "completed",
            message: "The schedule finished.".into(),
            trip: None,
        })
    }

    fn several(
        &mut self,
        count: u32,
        spacing_s: f64,
        origin: &'static str,
    ) -> Result<Fired, MethodError> {
        let mut out = Fired::default();
        for i in 0..count {
            if i > 0 && !self.pace(spacing_s) {
                break;
            }
            if self.ctx.signals.cancelled() || !self.ctx.signals.go.load(Ordering::SeqCst) {
                break;
            }
            let (row, fig) = self.one(origin, false)?;
            out.rows.push(row);
            out.figures.push(fig);
        }
        out.end = self.completed();
        Ok(out)
    }
}

impl ShockEngine for SimulatedEngine {
    fn arm(&mut self, plan: ArmPlan) -> Result<ArmOutcome, MethodError> {
        let cfg = plan.cfg.clone();
        let reference = preview::reference(&cfg.pulse(), cfg.standard, cfg.fs_hz);
        let feasibility = match &cfg.feasibility {
            None => (
                "not_evaluated",
                "No shaker was given, so the pulse was not checked against shaker ratings."
                    .to_owned(),
            ),
            Some(f) => super::config::verdict(&pulse_feasibility(
                &f.shaker,
                &f.moving_mass,
                reference.composite_g(),
                cfg.fs_hz,
                FeasibilityTolerances {
                    residual_velocity_ips: f.residual_velocity_ips,
                    residual_disp_in: f.residual_disp_in,
                },
            )),
        };
        if feasibility.0 == "exceeded" && plan.ack.is_none() {
            return Ok(ArmOutcome::Blocked(feasibility));
        }
        let ladder = ShockLadderState::new(cfg.schedule())
            .map_err(|e| MethodError::named(ErrorCode::Refused, "arm_refused", e.to_string()))?;
        let record_len = reference.record_len;
        self.ladder = Some(ladder);
        self.reference = Some(reference);
        self.cfg = Some(cfg.clone());
        self.armed_at = Some(Instant::now());
        Ok(ArmOutcome::Armed(Armed {
            fs_hz: cfg.fs_hz,
            record_len,
            station: StationRef {
                kind: "sim",
                class: "sim",
                devices: Vec::new(),
            },
            feasibility,
            lock: None,
        }))
    }

    fn identify(&mut self) -> Result<Identified, MethodError> {
        let (secs, vrms, ceiling, floor, review) = {
            let c = self.cfg()?;
            (
                c.sysid_duration_s,
                c.sysid_drive_vrms,
                c.drive_vpk_ceiling,
                c.sysid_min_mean_coherence,
                c.review,
            )
        };
        self.pace(secs);
        self.identified = true;
        Ok(Identified::Done {
            facts: IdentFacts {
                drive_vrms: vrms,
                drive_vpk: (3.5 * vrms).min(ceiling),
                coherence_mean: 0.998,
                min_mean_coherence: floor,
                passed: true,
            },
            review,
        })
    }

    fn seed(&mut self) -> Result<Seeded, MethodError> {
        let c = self.cfg()?;
        let vpk = c.amplitude_g * GAIN_V_PER_G;
        let (ceiling, abort) = (c.drive_vpk_ceiling, c.drive_vpk_abort);
        let full = c.levels_db.last().copied().unwrap_or(0.0);
        let r = self
            .reference
            .as_ref()
            .ok_or_else(|| MethodError::internal("the simulated run has no reference"))?;
        let at_full = 10f64.powf(full / 20.0);
        // In auto the schedule will reach full level, so its drive is judged now; in
        // manual each move to a level is (`shock.level`, `drive_ceiling`).
        if c.mode == ShockRunMode::Auto && vpk * at_full > abort {
            return Ok(Seeded::Ended(EngineEnd {
                outcome: "aborted",
                message: format!(
                    "The identified drive needs {} V pk at full level; the drive abort is {} V \
                     pk. Nothing was fired.",
                    crate::protocol::num(super::config::round(vpk * at_full, 3)),
                    crate::protocol::num(abort)
                ),
                trip: Some(Trip {
                    facts: TripFacts::DriveVpk {
                        vpk: vpk * at_full,
                        abort_vpk: abort,
                        predicted: true,
                    },
                    pulse: 0,
                    at_level_db: Some(full),
                }),
            }));
        }
        let peak = r.facts.peak_g * at_full;
        let prediction = Prediction {
            drive_vpk: vpk * at_full,
            drive_vpk_ceiling: ceiling,
            drive_vpk_abort: abort,
            response_peak_g: peak,
            velocity_pk_ips: r.facts.velocity_change_ips.abs() * at_full,
            displacement_pk_pk_in: r.facts.displacement_pk_pk_in * at_full,
        };
        self.vpk_full = Some(vpk);
        Ok(Seeded::Ready(prediction))
    }

    fn fire(&mut self) -> Result<Fired, MethodError> {
        let (row, fig) = self.one("ladder", true)?;
        Ok(Fired {
            rows: vec![row],
            figures: vec![fig],
            end: self.completed(),
        })
    }

    fn train(&mut self, count: u32, spacing_s: f64) -> Result<Fired, MethodError> {
        self.several(count, spacing_s, "train")
    }

    fn burst(&mut self) -> Result<Fired, MethodError> {
        let g = self
            .cfg()?
            .gunfire
            .ok_or_else(|| MethodError::internal("no gunfire is configured"))?;
        self.several(g.rounds, 60.0 / g.rounds_per_min, "gunfire")
    }

    fn set_level(&mut self, to: LevelMove) -> Result<f64, MethodError> {
        let ladder = self
            .ladder
            .as_mut()
            .ok_or_else(|| MethodError::internal("the simulated run has no ladder"))?;
        match to {
            LevelMove::Step(d) if d > 0.0 => ladder.step_up(),
            LevelMove::Step(_) => ladder.step_down(),
            LevelMove::Rung(i) => ladder.jump_to_level(i).map_err(|e| {
                MethodError::named(ErrorCode::BadRequest, "not_a_rung", e.to_string())
            })?,
        }
        Ok(ladder.current_level_db())
    }

    fn predicted_vpk_at(&self, level_db: f64) -> Option<f64> {
        self.vpk_full.map(|v| v * 10f64.powf(level_db / 20.0))
    }

    fn stop(&mut self) {}

    fn abort(&mut self, _reason: &str) {}

    fn position(&self) -> Position {
        let Some(l) = &self.ladder else {
            return Position::default();
        };
        let s = l.schedule();
        let level = l.current_level_db();
        let on = s
            .table_db
            .get(l.level_index())
            .is_some_and(|x| (x - level).abs() < 1e-9);
        Position {
            level_db: level,
            rung: on.then_some(l.level_index()),
            at_level: l.pulses_at_level(),
            of_level: if s.mode == ShockRunMode::Auto {
                s.pulses_for(l.level_index())
            } else {
                0
            },
            passed: self.passed,
        }
    }

    fn identification(&self) -> Option<IdentFigures> {
        let c = self.cfg.as_deref()?;
        if !self.identified {
            return None;
        }
        let (lo, hi) = c.band_hz;
        let n = 32;
        let freqs: Vec<f64> = (0..n)
            .map(|k| lo * (hi / lo).powf(k as f64 / (n - 1) as f64))
            .collect();
        Some(IdentFigures {
            gain: vec![1.0 / GAIN_V_PER_G; n],
            phase_deg: freqs.iter().map(|f| -360.0 * f * 1e-5).collect(),
            coherence: vec![0.998; n],
            trusted: vec![true; n],
            floor: c.coherence_floor,
            unit: "g".into(),
            freqs_hz: freqs,
        })
    }

    fn law_values(&self) -> Vec<LawReading> {
        if self.vpk_full.is_none() {
            return Vec::new();
        }
        vec![LawReading {
            label: "Drive change".into(),
            value: 0.0,
            unit: "dB".into(),
        }]
    }

    fn misses(&self) -> u32 {
        0
    }

    fn take_log(&mut self) -> Vec<(&'static str, String)> {
        std::mem::take(&mut self.log)
    }
}
