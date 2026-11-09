//! The closed-loop run of [`ClassicalShockService`]: identification, the
//! review hold, the seed, and the pulse loop under the core's guards.
//!
//! ```text
//! Armed -> identify -> SysId -> AwaitingReview -> proceed -> Firing -> ...
//!                         |                          |
//!                         v                          v
//!                       Held               Aborted (law_fault, drive_vpk)
//! ```
//!
//! # Each pulse
//!
//! 1. The run's pulse cap.
//! 2. The core's pre-fire checks on the drive in force at the pulse's level:
//!    the predicted response peak through the identified plant (the drive
//!    limit's ceiling), the drive's peak in volts against the drive abort
//!    (refuse) and the drive ceiling (scale down to it), and the predicted
//!    response's kinematics against the shaker's ratings (refuse).
//! 3. Fire the grid record in volts.
//! 4. Judge: the tolerance at the commanded amplitude on the sign-corrected
//!    record; an overshoot over `1 + overshoot_abort` aborts at any level; a
//!    tolerance failure aborts at full level (`full_level`) or at every level
//!    (`every_level`), never for an undershoot; measured kinematics and the
//!    between-pulse monitors as on the open loop.
//! 5. Divergence, then the ladder (`advance`, `max_pulses_per_level`).
//! 6. The law's next full-level drive, checked by the core before it is
//!    adopted. A late, failed, panicked or refused call is a miss: the next
//!    pulse refires the drive in force; `law_misses` in a row end the run.
//!
//! A train or a gunfire burst fires the drive in force, with no law call
//! inside it; each pulse is judged.

use super::closed::{
    build_law, check_output, fade_ends, guarded_call, peak, rms, summarize, ClassicalCallPayload,
    ClassicalCallRecord, GridPlant, LawRun, LawStats,
};
use super::*;
use crate::law::{FdCorrectFactory, PulseInputs, ToleranceAbort, Verdict, DIVERGENCE_RISE_DB};
use crate::service::report::{
    ConvergenceLimit, DrivePrediction, HeldReason, LawReport, LawStatus, PulseLawCall,
};
use hertzforge_dsp::shock::pulse_kinematics;
use hertzforge_shock_common::law_guard::{
    LawCall, LawCallRecord, LawFault, LawRecordRing, MissCounter, OutputRule,
};
use hertzforge_shock_common::LadderTick;
use hertzforge_twr::sysid::SysIdConfig;

/// What the identification view shows: the plant on the identification's own
/// bins, and the bins the core trusts on the pulse grid.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentificationView {
    /// The identification's bin spacing, Hz.
    pub df_hz: f64,
    /// `|H|`, EU per volt, on the identification's bins.
    pub h_mag: Vec<f64>,
    /// The phase of `H`, radians.
    pub h_phase: Vec<f64>,
    /// Coherence, 0 to 1.
    pub coherence: Vec<f64>,
    /// The pulse grid's bin spacing, Hz.
    pub grid_df_hz: f64,
    /// The bins the core trusts, on the pulse grid.
    pub trusted: Vec<bool>,
}

impl<F: PulseFirer> ClassicalShockService<F> {
    /// Scale the identified gain by `db` before it is inverted (the law
    /// check's identification-error case; 0 dB otherwise). The core's
    /// predictions use the scaled plant too: it is the plant the core believes.
    pub fn with_identification_scale_db(mut self, db: f64) -> Self {
        self.identification_scale_db = db;
        self
    }

    /// True when this run closes the loop (`ClassicalTestConfig::control` set).
    pub fn is_closed_loop(&self) -> bool {
        self.cfg.control.is_some()
    }

    pub(super) fn law_seeded(&self) -> bool {
        self.closed
            .as_ref()
            .is_some_and(|c| c.run.as_ref().is_some_and(|r| !r.drive.is_empty()))
    }

    pub(super) fn level_moves_allowed(&self) -> bool {
        self.state.is_running() || (self.closed.is_some() && self.state == RunState::AwaitingReview)
    }

    /// The drive's peak, volts, at `level_db` if the drive in force fired
    /// there. `None` before the law is seeded.
    pub fn predicted_drive_vpk_at(&self, level_db: f64) -> Option<f64> {
        let run = self.closed.as_ref()?.run.as_ref()?;
        (!run.drive.is_empty()).then(|| peak(&run.drive) * db_scale(level_db))
    }

    /// Refuse a level move whose drive would pass the drive abort.
    pub(super) fn check_level_move(&self, level_db: f64) -> Result<(), ServiceError> {
        let (Some(vpk), Some(control)) = (self.predicted_drive_vpk_at(level_db), &self.cfg.control)
        else {
            return Ok(());
        };
        if vpk > control.drive_vpk_abort {
            return Err(ServiceError::DriveCeiling {
                level_db,
                vpk,
                abort_vpk: control.drive_vpk_abort,
            });
        }
        Ok(())
    }

    /// The identification for the review view: `None` until identification
    /// is done.
    pub fn identification_view(&self) -> Option<IdentificationView> {
        let c = self.closed.as_ref()?;
        let sysid = c.sysid.as_ref()?;
        Some(IdentificationView {
            df_hz: sysid.df(),
            h_mag: sysid
                .h
                .iter()
                .map(|&(re, im)| (re * re + im * im).sqrt())
                .collect(),
            h_phase: sysid.h.iter().map(|&(re, im)| im.atan2(re)).collect(),
            coherence: sysid.coherence.clone(),
            grid_df_hz: c.grid.fs_hz / c.grid.n as f64,
            trusted: c
                .plant
                .as_ref()
                .map(|p| p.trusted.clone())
                .unwrap_or_default(),
        })
    }

    /// The full-level reference on the closed loop's grid, EU, and the main
    /// pulse's first sample and length. `None` before arm or on the open loop.
    pub fn grid_reference(&self) -> Option<(&[f64], usize, usize)> {
        let c = self.closed.as_ref()?;
        Some((&c.grid.reference, c.grid.main_start, c.grid.main_len))
    }

    /// Take the law-call records the run has not yet written: every call
    /// since the last drain, oldest first. With capture running the run writes
    /// them into the run folder itself after each call and this returns what
    /// is left (nothing, normally).
    pub fn drain_law_records(&mut self) -> Vec<ClassicalCallRecord> {
        self.closed
            .as_mut()
            .and_then(|c| c.run.as_mut())
            .map(|r| r.records.drain())
            .unwrap_or_default()
    }

    /// Run system identification: one cold fire of band-limited random in
    /// volts, its peak held under the drive ceiling. Then the run holds for
    /// review (`AwaitingReview`), or goes on as [`Self::proceed`] does when the
    /// review hold is off, or ends `Held` when the identification cannot be
    /// trusted. A cancel during the fire ends the run `Stopped`.
    pub fn identify(&mut self) -> Result<RunState, ServiceError> {
        let control = self.cfg.control.clone().ok_or_else(|| {
            ServiceError::BadState("identification is for a closed-loop run".into())
        })?;
        if self.state != RunState::Armed || self.closed.is_none() {
            return Err(ServiceError::BadState(format!(
                "identify from {:?} (arm first)",
                self.state
            )));
        }
        self.transition(RunState::SysId);
        let fs = self.fs_hz();
        let mut drive = control.sysid.build_excitation(fs);
        fade_ends(&mut drive, fs);
        let pk = peak(&drive);
        if pk > control.drive_vpk_ceiling {
            let s = control.drive_vpk_ceiling / pk;
            for v in drive.iter_mut() {
                *v *= s;
            }
            self.log.push(EventKind::Note {
                text: format!(
                    "identification drive peak {pk:.3} V held to the {:.3} V drive ceiling",
                    control.drive_vpk_ceiling
                ),
            });
        }

        let cancel = Arc::clone(&self.cancel);
        let mut observer = self.block_observer.take();
        let fired = {
            let mut ctl = FireControl {
                cancel: Some(cancel.as_ref()),
                observer: observer.as_deref_mut(),
                hooks: None,
            };
            self.firer.fire(&drive, 0.0, &mut ctl)
        };
        self.block_observer = observer;
        self.drain_warnings(None);
        let response = match fired {
            Ok(Some(r)) => r,
            Ok(None) => {
                return Err(ServiceError::Fire(
                    "identification needs the measured response".into(),
                ))
            }
            Err(ServiceError::Cancelled {
                frames_done,
                frames_committed,
                frames_expected,
            }) => {
                self.log.push(EventKind::FireCancelled {
                    pulse: 0,
                    frames_done,
                    frames_committed,
                    frames_expected,
                });
                let _ = self.stop_capture_inner();
                let _ = self.firer.stop();
                self.transition(RunState::Stopped);
                return Ok(self.state);
            }
            Err(e) => return Err(e),
        };
        if response.len() != drive.len() {
            return Err(ServiceError::Fire(format!(
                "identification played {} samples and measured {}",
                drive.len(),
                response.len()
            )));
        }

        // Estimate with no gate, then judge the coherence here, so a held
        // run still reports what it measured.
        let est = SysIdConfig {
            min_mean_coherence: f64::MIN_POSITIVE,
            ..control.sysid.clone()
        };
        let measured = est
            .estimate(&drive, &response, fs)
            .map_err(|e| ServiceError::Fire(format!("identification: {e}")))?;
        let mut believed = measured.clone();
        if self.identification_scale_db != 0.0 {
            let s = db_scale(self.identification_scale_db);
            for h in believed.h.iter_mut() {
                h.0 *= s;
                h.1 *= s;
            }
            self.log.push(EventKind::Note {
                text: format!(
                    "the identified gain is scaled {:+.1} dB before it is inverted",
                    self.identification_scale_db
                ),
            });
        }
        let mean = measured.in_band_mean_coherence;
        let closed = self
            .closed
            .as_mut()
            .ok_or_else(|| ServiceError::Internal("no closed loop at identification".into()))?;
        closed.sysid = Some(measured);
        let plant = if mean >= control.sysid.min_mean_coherence {
            Some(
                GridPlant::new(&believed, &control, &closed.grid)
                    .map_err(|e| ServiceError::Fire(format!("identification: {e}")))?,
            )
        } else {
            None
        };
        let summary = plant
            .as_ref()
            .map(|p| summarize(&drive, &believed, p, &control));
        let (trusted, coverage) = plant
            .as_ref()
            .map(|p| (p.trusted.iter().filter(|&&t| t).count(), p.coverage))
            .unwrap_or((0, 0.0));
        closed.summary = summary.or(Some(crate::service::report::IdentificationSummary {
            drive_vrms: rms(&drive),
            drive_vpk: peak(&drive),
            coherence_mean: mean,
            min_mean_coherence: control.sysid.min_mean_coherence,
            trusted_bins: 0,
            band_energy: 0.0,
            passed: false,
        }));
        closed.plant = plant;
        self.log.push(EventKind::Identified {
            coherence_mean: mean,
            min_mean_coherence: control.sysid.min_mean_coherence,
            trusted_bins: trusted,
            band_energy: coverage,
            drive_vpk: peak(&drive),
        });

        if mean < control.sysid.min_mean_coherence {
            self.hold(HeldReason::LowCoherence {
                mean,
                floor: control.sysid.min_mean_coherence,
            });
            return Ok(self.state);
        }
        if coverage < control.min_band_energy {
            self.hold(HeldReason::ReferenceOutOfBand {
                coverage,
                min: control.min_band_energy,
            });
            return Ok(self.state);
        }
        if control.review_hold {
            self.transition(RunState::AwaitingReview);
            Ok(self.state)
        } else {
            self.proceed()
        }
    }

    /// Seed the law after identification (from the review hold): build the
    /// law for the run, take its first drive through the core's checks (the
    /// first-drive bound included; a seed that is not usable ends the run
    /// `law_fault` with nothing fired), and refuse the run before any pulse
    /// when the drive at the highest level the run is scheduled to reach (the
    /// top of an AUTO table, the present MANUAL level) would pass the drive
    /// abort. Then the run fires: [`Self::run_auto`] in AUTO,
    /// [`Self::fire_next_pulse`] per pulse in MANUAL.
    pub fn proceed(&mut self) -> Result<RunState, ServiceError> {
        let control = self
            .cfg
            .control
            .clone()
            .ok_or_else(|| ServiceError::BadState("proceed is for a closed-loop run".into()))?;
        if !matches!(self.state, RunState::AwaitingReview | RunState::SysId) {
            return Err(ServiceError::BadState(format!(
                "proceed from {:?} (identify first)",
                self.state
            )));
        }
        let spacing = self.cfg.rate_model.effective_min_spacing(self.firer.path());
        let budget_s = control.guards.call_budget_s(spacing);
        let factory: Arc<dyn ClassicalLawFactory> = self
            .law_factory
            .clone()
            .unwrap_or_else(|| Arc::new(FdCorrectFactory::default()));
        let descriptor = factory.describe();
        // The core's caps are never tighter than the built-in's own clamp
        // plus 1 dB at the run's settings.
        let (mut bin_cap_db, mut peak_cap_db) = (control.max_bin_rise_db, control.max_peak_rise_db);
        if descriptor.id == hertzforge_shock_law_math::classical::ID {
            if let Some(m) = descriptor
                .params
                .get("max_db_per_pulse")
                .and_then(|v| v.as_f64())
            {
                bin_cap_db = bin_cap_db.max(m + 1.0);
                peak_cap_db = peak_cap_db.max(m + 1.0);
            }
        }
        let built = {
            let closed = self
                .closed
                .as_ref()
                .ok_or_else(|| ServiceError::Internal("no closed loop at proceed".into()))?;
            let plant = closed
                .plant
                .as_ref()
                .ok_or_else(|| ServiceError::Internal("no plant at proceed".into()))?;
            let setup = closed.setup(
                plant,
                control.drive_vpk_ceiling,
                budget_s,
                (control.band_lo_hz, control.band_hi_hz),
            );
            build_law(factory.as_ref(), &setup)
        };
        let law = match built {
            Ok(l) => l,
            Err(fault) => {
                self.log.push(EventKind::LawCalled {
                    pulse: 0,
                    call: LawCall::Seed,
                    used: false,
                    reason: Some(fault.reason().into()),
                    detail: Some(fault.to_string()),
                    call_ms: 0.0,
                });
                self.end_run(
                    TripKind::LawFault {
                        fault,
                        consecutive: 1,
                    },
                    RunState::Aborted,
                );
                return Ok(self.state);
            }
        };
        self.log.push(EventKind::Note {
            text: format!("control law: {}", LawReport::line_for(&descriptor)),
        });
        if let Some(closed) = self.closed.as_mut() {
            closed.run = Some(LawRun {
                law,
                descriptor,
                drive: Vec::new(),
                drive_mags: Vec::new(),
                predicted: Vec::new(),
                misses: MissCounter::new(control.guards.max_misses),
                stats: LawStats::default(),
                records: LawRecordRing::default(),
                last_values: Vec::new(),
                pending: None,
                budget_s,
                bin_cap_db,
                peak_cap_db,
            });
        }
        if !self.call_law(LawCall::Seed, &[], &[], None, false)? {
            return Ok(self.state);
        }
        let top_db = if self.cfg.schedule.mode == ShockRunMode::Auto {
            self.cfg.schedule.full_db()
        } else {
            self.current_level_db()
        };
        let vpk = self.predicted_drive_vpk_at(top_db).unwrap_or(0.0);
        if vpk > control.drive_vpk_abort {
            self.latch_trip(TripKind::DriveVpk {
                pulse: 0,
                vpk,
                abort_vpk: control.drive_vpk_abort,
                predicted: true,
            });
            return Ok(self.state);
        }
        self.transition(RunState::Firing);
        Ok(self.state)
    }

    /// Call the law once and take its output through the core's checks.
    /// `Ok(true)` when the run goes on (the output adopted, or a miss under the
    /// limit); `Ok(false)` when the call ended the run (a seed miss, or
    /// `law_misses` in a row).
    fn call_law(
        &mut self,
        which: LawCall,
        response: &[f64],
        drive_in: &[f64],
        verdict: Option<Verdict>,
        limited: bool,
    ) -> Result<bool, ServiceError> {
        let control = self
            .cfg
            .control
            .clone()
            .ok_or_else(|| ServiceError::Internal("a law call on the open loop".into()))?;
        let clock = Arc::clone(&self.law_clock);
        let pulse_index = self.pulses_fired;
        let fs = self.fs_hz();
        let closed = self
            .closed
            .as_mut()
            .ok_or_else(|| ServiceError::Internal("no closed loop at a law call".into()))?;
        let (grid, plant, run) = match (&closed.grid, closed.plant.as_ref(), closed.run.as_mut()) {
            (g, Some(p), Some(r)) => (g, p, r),
            _ => return Err(ServiceError::Internal("a law call before its build".into())),
        };
        let inputs = PulseInputs {
            pulse_index,
            response,
            drive: drive_in,
            judged: verdict,
            limited,
        };
        let (result, took_s) = guarded_call(
            run.law.as_mut(),
            which,
            &inputs,
            clock.as_ref(),
            run.budget_s,
        );
        run.stats.calls += 1;
        run.stats.worst_call_s = run.stats.worst_call_s.max(took_s);
        let last =
            (!run.drive.is_empty()).then_some((run.drive.as_slice(), run.drive_mags.as_slice()));
        let (outcome, out_drive, values) = match result {
            Err(f) => (Err((f, Default::default())), Vec::new(), Vec::new()),
            Ok(out) => {
                let mut candidate = out.drive.clone();
                if candidate.len() == grid.n {
                    grid.taper(&mut candidate);
                }
                let checked = check_output(
                    &out,
                    &candidate,
                    grid.n,
                    if which == LawCall::Seed { None } else { last },
                    run.peak_cap_db,
                    run.bin_cap_db,
                )
                .and_then(|(chk, mags)| {
                    let predicted = plant
                        .predict(&candidate)
                        .map_err(|e| (LawFault::Failed(e), Default::default()))?;
                    if which == LawCall::Seed {
                        let bound = control.first_drive_ratio * peak(&grid.reference);
                        if peak(&predicted) > bound {
                            return Err((LawFault::BadOutput(OutputRule::FirstDrive), chk));
                        }
                    }
                    Ok((chk, mags, predicted))
                });
                (
                    checked.map(|(chk, mags, predicted)| (chk, mags, predicted, candidate)),
                    out.drive,
                    out.values,
                )
            }
        };
        let call_ms = took_s * 1e3;
        let (miss, chk) = match &outcome {
            Ok((chk, ..)) => (None, *chk),
            Err((f, chk)) => (Some(f.clone()), *chk),
        };
        run.records.push(LawCallRecord {
            pulse_index,
            call: which,
            miss: miss.clone(),
            call_us: (took_s * 1e6).round() as u64,
            payload: ClassicalCallPayload {
                response: response.iter().map(|&v| v as f32).collect(),
                drive_in: drive_in.iter().map(|&v| v as f32).collect(),
                drive_out: out_drive.iter().map(|&v| v as f32).collect(),
                values: values.clone(),
                verdict,
                limited,
                capped_bins: chk.capped_bins,
                peak_capped: chk.peak_capped,
            },
        });
        let ended = match outcome {
            Ok((_, mags, predicted, candidate)) => {
                run.drive = candidate;
                run.drive_mags = mags;
                run.predicted = predicted;
                run.last_values = values;
                run.misses.used();
                run.pending = Some(PulseLawCall {
                    used: true,
                    miss: None,
                    capped_bins: 0,
                    peak_capped: false,
                    call_ms,
                });
                None
            }
            Err((fault, chk)) => {
                run.stats.misses += 1;
                if matches!(fault, LawFault::BadOutput(_)) {
                    run.stats.refused += 1;
                }
                run.stats.capped_bins += chk.capped_bins;
                run.stats.capped_peaks += u64::from(chk.peak_capped);
                run.pending = Some(PulseLawCall {
                    used: false,
                    miss: Some(fault.clone()),
                    capped_bins: chk.capped_bins,
                    peak_capped: chk.peak_capped,
                    call_ms,
                });
                let tripped = run.misses.miss();
                (which == LawCall::Seed || tripped).then(|| (fault, run.misses.in_a_row()))
            }
        };
        let _ = fs;
        self.log.push(EventKind::LawCalled {
            pulse: pulse_index,
            call: which,
            used: miss.is_none(),
            reason: miss.as_ref().map(|f| f.reason().to_string()),
            detail: miss.as_ref().map(|f| f.to_string()),
            call_ms,
        });
        self.persist_law_records();
        if let Some((fault, consecutive)) = ended {
            self.end_run(TripKind::LawFault { fault, consecutive }, RunState::Aborted);
            return Ok(false);
        }
        Ok(true)
    }

    /// Write the law's identity once and every call record since the last
    /// write into the run folder, when capture is running. Between pulses,
    /// after the pulse is judged; never during a fire.
    fn persist_law_records(&mut self) {
        let Some(dir) = self.capture.as_ref().map(|s| s.run_dir().to_path_buf()) else {
            return;
        };
        let control = self.cfg.control.clone();
        let Some(closed) = self.closed.as_mut() else {
            return;
        };
        let Some(run) = closed.run.as_mut() else {
            return;
        };
        if !closed.descriptor_written {
            let doc = serde_json::json!({
                "law": run.descriptor,
                "guards": control.as_ref().map(|c| &c.guards),
                "call_budget_s": run.budget_s,
                "max_peak_rise_db": run.peak_cap_db,
                "max_bin_rise_db": run.bin_cap_db,
            });
            let text = serde_json::to_string_pretty(&doc).unwrap_or_default();
            closed.descriptor_written = std::fs::write(dir.join("law.json"), text).is_ok();
        }
        for r in run.records.drain() {
            let mut index = serde_json::Map::new();
            index.insert("pulse_index".into(), r.pulse_index.into());
            index.insert(
                "call".into(),
                serde_json::to_value(r.call).unwrap_or_default(),
            );
            index.insert("used".into(), r.miss.is_none().into());
            index.insert("miss".into(), r.miss.as_ref().map(|f| f.reason()).into());
            index.insert("call_us".into(), r.call_us.into());
            index.insert("capped_bins".into(), r.payload.capped_bins.into());
            index.insert("peak_capped".into(), r.payload.peak_capped.into());
            index.insert("limited".into(), r.payload.limited.into());
            index.insert(
                "verdict".into(),
                serde_json::to_value(r.payload.verdict).unwrap_or_default(),
            );
            index.insert(
                "values".into(),
                serde_json::to_value(&r.payload.values).unwrap_or_default(),
            );
            index.insert("response_len".into(), r.payload.response.len().into());
            index.insert("drive_in_len".into(), r.payload.drive_in.len().into());
            index.insert("drive_out_len".into(), r.payload.drive_out.len().into());
            let mut bytes = Vec::with_capacity(
                4 * (r.payload.response.len()
                    + r.payload.drive_in.len()
                    + r.payload.drive_out.len()),
            );
            for v in r
                .payload
                .response
                .iter()
                .chain(&r.payload.drive_in)
                .chain(&r.payload.drive_out)
            {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            if hertzforge_capture::append_indexed_record(&dir, "law_pulses", &index, &bytes)
                .is_err()
            {
                // Keep it for the host rather than lose it.
                run.records.push(r);
                break;
            }
        }
    }

    /// End the run with `kind` and land in `to`: the controlled stop, nothing
    /// more fires.
    fn end_run(&mut self, kind: TripKind, to: RunState) {
        let record = TripRecord {
            kind,
            at_level_db: self.current_level_db(),
            at_pulse: self.pulses_fired,
        };
        self.log.push(EventKind::Trip {
            record: record.clone(),
        });
        self.trip = Some(record);
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.stop_capture_inner();
        let _ = self.firer.stop();
        self.drain_warnings(None);
        self.transition(to);
    }

    /// Identification held the run: nothing fires.
    fn hold(&mut self, reason: HeldReason) {
        self.log.push(EventKind::Held {
            reason: reason.clone(),
        });
        self.held = Some(reason);
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.stop_capture_inner();
        let _ = self.firer.stop();
        self.drain_warnings(None);
        self.transition(RunState::Held);
    }

    pub(super) fn prediction(&self) -> Option<DrivePrediction> {
        let control = self.cfg.control.as_ref()?;
        let run = self.closed.as_ref()?.run.as_ref()?;
        if run.drive.is_empty() {
            return None;
        }
        let k = pulse_kinematics(&run.predicted, self.fs_hz());
        Some(DrivePrediction {
            drive_vpk: peak(&run.drive),
            drive_vpk_ceiling: control.drive_vpk_ceiling,
            drive_vpk_abort: control.drive_vpk_abort,
            response_peak_g: peak(&run.predicted),
            velocity_pk_ips: k.peak_velocity_ips,
            displacement_pk_pk_in: k.pk_pk_disp_in,
        })
    }

    pub(super) fn law_status(&self) -> Option<LawStatus> {
        let run = self.closed.as_ref()?.run.as_ref()?;
        Some(LawStatus {
            descriptor: run.descriptor.clone(),
            misses_in_a_row: run.misses.in_a_row(),
            misses: run.stats.misses,
            calls: run.stats.calls,
            values: run.last_values.clone(),
        })
    }

    pub(super) fn law_report(&self) -> Option<LawReport> {
        let run = self.closed.as_ref()?.run.as_ref()?;
        Some(LawReport {
            line: LawReport::line_for(&run.descriptor),
            descriptor: run.descriptor.clone(),
            calls: run.stats.calls,
            misses: run.stats.misses,
            refused: run.stats.refused,
            capped_bins: run.stats.capped_bins,
            capped_peaks: run.stats.capped_peaks,
            worst_call_ms: run.stats.worst_call_s * 1e3,
            budget_ms: run.budget_s * 1e3,
        })
    }

    /// One closed-loop pulse (see the module docs). Returns `true` if the run
    /// is still running.
    pub(super) fn fire_one_closed(
        &mut self,
        scheduled_level_db: f64,
        spacing_s: f64,
        tick_ladder: bool,
        origin: PulseOrigin,
        slot: super::Slot<'_>,
    ) -> Result<bool, ServiceError> {
        let checks_from = std::time::Instant::now();
        if !self.state.is_running() {
            return Err(ServiceError::BadState(format!(
                "fire from {:?} (arm first)",
                self.state
            )));
        }
        if !spacing_s.is_finite() {
            return Err(ServiceError::BadState(
                "rate model refuses all spacing (read cadence unsustainable)".into(),
            ));
        }
        let control =
            self.cfg.control.clone().ok_or_else(|| {
                ServiceError::Internal("a closed-loop pulse on the open loop".into())
            })?;
        let (drive_full, predicted_full, sign, reference_full, window) = {
            let closed = self
                .closed
                .as_ref()
                .ok_or_else(|| ServiceError::Internal("no closed loop at a pulse".into()))?;
            let run = closed
                .run
                .as_ref()
                .filter(|r| !r.drive.is_empty())
                .ok_or_else(|| {
                    ServiceError::BadState("fire before the control law is seeded".into())
                })?;
            let p = self.cfg.standard.params();
            (
                run.drive.clone(),
                run.predicted.clone(),
                closed.grid.sign,
                closed.grid.reference.clone(),
                closed
                    .grid
                    .judged_window(p.window_pre_frac, p.window_post_frac),
            )
        };
        if self.state == RunState::Armed {
            self.transition(RunState::Firing);
        }
        // 1. The run's pulse cap.
        if self.pulses_fired >= control.guards.max_pulses {
            self.end_run(
                TripKind::NotConverged {
                    limit: ConvergenceLimit::Run,
                    pulses: self.pulses_fired,
                },
                RunState::NotConverged,
            );
            return Ok(false);
        }

        let ladder_scale = db_scale(scheduled_level_db);
        let carried = self.next_pulse_scale;
        let mut extra_scale = carried;
        self.next_pulse_scale = 1.0;
        let fs = self.fs_hz();

        // 2a. The predicted response peak through the identified plant.
        let predicted_peak = peak(&predicted_full) * ladder_scale * extra_scale;
        let decision = self.cfg.drive_limit.pre_fire_accel(predicted_peak);
        if !decision.allowed {
            if decision.scale_factor > 0.0 {
                extra_scale *= decision.scale_factor;
                self.drive_limit_action(
                    self.pulses_fired,
                    "pre-fire",
                    decision.scale_factor,
                    decision.reason,
                );
            } else {
                self.latch_trip(TripKind::PreFireRefused {
                    reason: decision.reason,
                });
                return Ok(false);
            }
        }
        // 2b. The drive in volts: over the abort it is refused, over the
        // ceiling it is scaled to it.
        let vpk = peak(&drive_full) * ladder_scale * extra_scale;
        if vpk > control.drive_vpk_abort {
            self.latch_trip(TripKind::DriveVpk {
                pulse: self.pulses_fired,
                vpk,
                abort_vpk: control.drive_vpk_abort,
                predicted: false,
            });
            return Ok(false);
        }
        if vpk > control.drive_vpk_ceiling {
            let s = control.drive_vpk_ceiling / vpk;
            extra_scale *= s;
            self.drive_limit_action(
                self.pulses_fired,
                "drive-ceiling",
                s,
                format!(
                    "the drive's {vpk:.4} V pk is over the {:.4} V pk drive ceiling; scaled to it",
                    control.drive_vpk_ceiling
                ),
            );
        }
        let total_scale = ladder_scale * extra_scale;
        let commanded_db = scheduled_level_db + 20.0 * extra_scale.log10();
        let commanded_amplitude_g = self.cfg.spec.amplitude_g * total_scale;
        // 2c. The predicted response's kinematics at the commanded scale.
        if let Some(shaker) = &self.cfg.shaker {
            let response: Vec<f64> = predicted_full.iter().map(|v| v * total_scale).collect();
            let k = pulse_kinematics(&response, fs);
            let breach = shaker
                .max_velocity_ips
                .as_ref()
                .filter(|v| k.peak_velocity_ips > v.value)
                .map(|v| ("peak velocity (in/s)", k.peak_velocity_ips, v.value))
                .or_else(|| {
                    shaker
                        .max_displacement_in_pp
                        .as_ref()
                        .filter(|d| k.pk_pk_disp_in > d.value)
                        .map(|d| ("pk-pk displacement (in)", k.pk_pk_disp_in, d.value))
                });
            if let Some((quantity, value, limit)) = breach {
                self.latch_trip(TripKind::Kinematic {
                    quantity: quantity.into(),
                    value,
                    limit,
                    pulse: self.pulses_fired,
                    predicted: true,
                });
                return Ok(false);
            }
        }

        // 3. Fire the grid record in volts.
        let drive: Vec<f64> = drive_full.iter().map(|&v| v * total_scale).collect();
        let fired_at_s = self.run_clock_s;
        let fired = self.fire_paced(&drive, commanded_db, spacing_s, slot, checks_from);
        if matches!(fired, Err(ServiceError::Withheld)) {
            self.withhold(carried);
            return Ok(false);
        }
        let fired_at_wall_s = self.last_pulse_wall_s.unwrap_or(0.0);
        let pulse_no = self.pulses_fired;
        self.drain_warnings(Some(pulse_no));
        let record = match fired {
            Ok(Some(m)) => m,
            Ok(None) => {
                return Err(ServiceError::Fire(
                    "the closed loop needs the measured response".into(),
                ))
            }
            Err(ServiceError::Cancelled {
                frames_done,
                frames_committed,
                frames_expected,
            }) => {
                self.log.push(EventKind::FireCancelled {
                    pulse: pulse_no,
                    frames_done,
                    frames_committed,
                    frames_expected,
                });
                let _ = self.stop_capture_inner();
                let _ = self.firer.stop();
                if self.state != RunState::Aborted {
                    self.transition(RunState::Stopped);
                }
                return Ok(false);
            }
            Err(e) => return Err(e),
        };
        self.pulses_fired += 1;
        self.run_clock_s += spacing_s;

        // 4. Measure and judge.
        let judged: Vec<f64> = record.iter().map(|v| v * sign).collect();
        let reference_level: Vec<f64> = reference_full.iter().map(|v| v * total_scale).collect();
        let fired_at_unix = self.base_unix + fired_at_s.max(0.0) as u64;
        let seq = self.store.ingest(
            record.clone(),
            fs,
            self.srs_freqs.clone(),
            commanded_db,
            self.cfg.compute_pvss,
            fired_at_unix,
        );
        let (peak_g, peak_velocity_ips, pk_pk_disp_in, meas_srs, record_len) = {
            let rec = self.store.get(seq).ok_or_else(|| {
                ServiceError::Internal(format!(
                    "pulse store lost record {seq} between ingest and read"
                ))
            })?;
            if let Some(sink) = self.capture.as_ref() {
                let _ = capture_pulse(sink, rec, 1);
            }
            (
                rec.kinematics.peak_accel_g,
                rec.kinematics.peak_velocity_ips,
                rec.kinematics.pk_pk_disp_in,
                rec.srs.clone(),
                rec.time_history.len(),
            )
        };
        let (band_pass, worst_db) = self.srs_band_check(&meas_srs, &reference_level, record_len);
        let mut scaled_spec: ClassicalPulseSpec = self.cfg.spec.clone();
        scaled_spec.amplitude_g = commanded_amplitude_g;
        let tol = check_tolerance(&judged, &scaled_spec, fs, self.cfg.standard);
        let (w0, w1) = window;
        let target: Vec<f64> = reference_level[w0..w1].iter().map(|v| v * sign).collect();
        let err: Vec<f64> = judged[w0..w1.min(judged.len())]
            .iter()
            .zip(&target)
            .map(|(m, t)| m - t)
            .collect();
        let error_pct = 100.0 * rms(&err) / rms(&target).max(f64::MIN_POSITIVE);
        let peak_ratio = judged.iter().fold(f64::NEG_INFINITY, |m, &v| m.max(v))
            / commanded_amplitude_g.max(f64::MIN_POSITIVE);
        let velocity_change_ratio = if tol.velocity_change.nominal_dv_ips.abs() > 0.0 {
            tol.velocity_change.measured_dv_ips / tol.velocity_change.nominal_dv_ips
        } else {
            0.0
        };
        let passed = tol.overall_pass;
        self.tolerance_checked += 1;
        if passed {
            self.tolerance_passed += 1;
            self.alarm_active = false;
        }

        let law = if tick_ladder {
            self.closed
                .as_mut()
                .and_then(|c| c.run.as_mut())
                .and_then(|r| r.pending.take())
        } else {
            None
        };
        let row = PulseRow {
            pulse: pulse_no,
            store_seq: Some(seq),
            origin,
            scheduled_level_db,
            commanded_db,
            commanded_amplitude_g,
            drive_limit_scale: extra_scale,
            at_run_time_s: fired_at_s,
            at_wall_time_s: fired_at_wall_s,
            bring_up: self.last_bring_up.take(),
            measured: true,
            peak_g: Some(peak_g),
            velocity_change_ips: Some(tol.velocity_change.measured_dv_ips),
            pk_pk_disp_in: Some(pk_pk_disp_in),
            tolerance_overall: Some(passed),
            envelope_pass: Some(tol.amplitude_envelope.pass),
            velocity_change_pass: Some(tol.velocity_change.pass),
            substitution_pass: Some(tol.substitution.pass),
            srs_bands_total: Some(band_pass.len()),
            srs_bands_pass: Some(band_pass.iter().filter(|&&b| b).count()),
            srs_worst_dev_db: Some(worst_db),
            srs_band_pass: Some(band_pass),
            drive_vpk: Some(peak(&drive)),
            error_pct: Some(error_pct),
            peak_ratio: Some(peak_ratio),
            velocity_change_ratio: Some(velocity_change_ratio),
            law,
        };

        // The judgement, in order: overshoot, measured kinematics, the
        // between-pulse monitors, then the tolerance verdict.
        let mut abort_after: Option<TripKind> = None;
        let limit_ratio = 1.0 + control.overshoot_abort_frac(self.cfg.standard);
        if peak_ratio > limit_ratio {
            abort_after = Some(TripKind::Overshoot {
                pulse: pulse_no,
                peak_ratio,
                limit_ratio,
            });
        }
        if abort_after.is_none() {
            if let Some(shaker) = &self.cfg.shaker {
                if let Some(v) = shaker
                    .max_velocity_ips
                    .as_ref()
                    .filter(|v| peak_velocity_ips > v.value)
                {
                    abort_after = Some(TripKind::Kinematic {
                        quantity: "peak velocity (in/s)".into(),
                        value: peak_velocity_ips,
                        limit: v.value,
                        pulse: pulse_no,
                        predicted: false,
                    });
                } else if let Some(d) = shaker
                    .max_displacement_in_pp
                    .as_ref()
                    .filter(|d| pk_pk_disp_in > d.value)
                {
                    abort_after = Some(TripKind::Kinematic {
                        quantity: "pk-pk displacement (in)".into(),
                        value: pk_pk_disp_in,
                        limit: d.value,
                        pulse: pulse_no,
                        predicted: false,
                    });
                }
            }
        }
        let caps = self.firer.path().capabilities();
        let peaks = vec![MeasuredPeak {
            channel: self.control_channel_name.clone(),
            peak_eu: peak_g,
        }];
        let bp = self.cfg.drive_limit.between_pulse(&caps, &peaks);
        if bp.scale_factor < 1.0 {
            self.next_pulse_scale = bp.scale_factor;
        }
        for a in bp.actions {
            if a.scale_factor < 1.0 {
                self.drive_limit_action(pulse_no, "between-pulse", a.scale_factor, a.reason);
            }
        }
        if abort_after.is_none() && !passed {
            let at_full = scheduled_level_db >= self.cfg.schedule.full_db() - 1e-9;
            let undershoot = peak_ratio < 1.0 && velocity_change_ratio < 1.0;
            let aborts = !undershoot
                && match control.tolerance_abort {
                    ToleranceAbort::EveryLevel => true,
                    ToleranceAbort::FullLevel => at_full,
                };
            if aborts {
                abort_after = Some(TripKind::Tolerance {
                    pulse: pulse_no,
                    envelope_pass: tol.amplitude_envelope.pass,
                    velocity_change_pass: tol.velocity_change.pass,
                    error_frac: tol.velocity_change.error_frac,
                    worst_deviation_frac_of_peak: tol
                        .amplitude_envelope
                        .worst_deviation_frac_of_peak,
                });
            } else {
                self.alarm_active = true;
                self.log.push(EventKind::ToleranceAlarm {
                    pulse: pulse_no,
                    error_frac: tol.velocity_change.error_frac,
                    worst_deviation_frac_of_peak: tol
                        .amplitude_envelope
                        .worst_deviation_frac_of_peak,
                });
            }
        }
        let aligned = tol.aligned_pulse_start_index;
        self.last_tolerance = Some(tol);
        self.log.push(EventKind::PulseFired {
            pulse: pulse_no,
            scheduled_level_db,
            commanded_db,
            measured: true,
            peak_g: Some(peak_g),
            velocity_change_ips: row.velocity_change_ips,
            tolerance_pass: Some(passed),
        });
        self.pulse_rows.push(row);
        self.played.push(PlayedPulse {
            pulse: pulse_no,
            commanded_db,
            commanded_amplitude_g,
            drive,
            lead_frames: 0,
            aligned_start: Some(aligned),
            scale: total_scale,
        });
        if let Some(kind) = abort_after {
            self.latch_trip(kind);
            return Ok(false);
        }
        // A train fires the drive in force: no ladder, no law.
        if !tick_ladder {
            return Ok(true);
        }

        // 5. Divergence at this level, then the ladder.
        let rises = self
            .closed
            .as_mut()
            .map(|c| {
                c.diverging(
                    scheduled_level_db,
                    error_pct,
                    DIVERGENCE_RISE_DB,
                    control.divergence_pulses,
                )
            })
            .unwrap_or(0);
        if rises > 0 {
            self.latch_trip(TripKind::Divergence {
                pulse: pulse_no,
                pulses: rises,
                error_pct,
            });
            return Ok(false);
        }
        let tick = {
            let ladder = self.ladder.as_mut().ok_or_else(|| {
                ServiceError::Internal("no ladder while running (ladder tick)".into())
            })?;
            let before = ladder.current_level_db();
            let tick = ladder.on_pulse_judged(passed);
            (tick, before, ladder.current_level_db())
        };
        match tick {
            (LadderTick::NotConverged { pulses }, ..) => {
                self.end_run(
                    TripKind::NotConverged {
                        limit: ConvergenceLimit::PerLevel,
                        pulses: u64::from(pulses),
                    },
                    RunState::NotConverged,
                );
                return Ok(false);
            }
            (LadderTick::Completed, ..) => return Ok(true),
            (LadderTick::Advanced, from_db, to_db) => {
                self.level_history.push(LevelHistoryEntry {
                    level_db: to_db,
                    auto: true,
                    at_pulse: self.pulses_fired,
                });
                self.log.push(EventKind::LevelChange {
                    from_db,
                    to_db,
                    auto: true,
                });
            }
            (LadderTick::Held, ..) => {}
        }

        // 6. The law's next full-level drive.
        let response_full: Vec<f64> = record.iter().map(|v| v / total_scale).collect();
        let verdict = Verdict {
            passed,
            error_pct,
            peak_ratio,
            velocity_change_ratio,
        };
        self.call_law(
            LawCall::Iterate,
            &response_full,
            &drive_full,
            Some(verdict),
            extra_scale < 1.0 - 1e-12,
        )
    }

    fn drive_limit_action(&mut self, pulse: u64, phase: &str, scale: f64, reason: String) {
        self.drive_limit_actions.push(DriveLimitActionRecord {
            pulse,
            phase: phase.into(),
            scale,
            reason: reason.clone(),
        });
        self.log.push(EventKind::DriveLimitAction {
            pulse,
            phase: phase.into(),
            scale,
            reason,
        });
    }
}
