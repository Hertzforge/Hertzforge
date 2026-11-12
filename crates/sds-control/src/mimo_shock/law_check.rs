//! The MIMO Shock law check's runner: each case of
//! `hertzforge_mimo_shock_laws::check` played through the engine on the
//! simulator's shock plants, judged by that crate's `judge`.
//!
//! What each case's perturbation does here:
//!
//! - **Model delay** and the **identification** gain error: the law is built
//!   on the identified plant moved by the delay (`H exp(-j 2 pi f delay)`) or
//!   scaled by the gain; the core's own guards keep the plant as identified.
//!   The identification is at 20 dB under the reference, the engine's own
//!   level rule.
//! - **Response noise**: independent Gaussian noise on every measured control
//!   row, its rms the stated fraction of that control's full-level target rms,
//!   from the case's seed.
//! - **Limit channel**: the plant's last output is a monitored row, not a
//!   control. A first run without a bound measures its peak at full level; the
//!   case runs again with the abort at the stated fraction of that peak.
//!
//! A run that holds before its first pulse because a control cannot reach its
//! target (more controls than drives) is acknowledged: the case judges the law.

use super::config::PLANTS;
use crate::protocol::MethodError;
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_dsp::shock::{CompensationConfig, CompensationShape};
use hertzforge_dsp::srs::{octave_freqs, OctaveFraction};
use hertzforge_hal::Backend;
use hertzforge_mimo_shock::ident::Rng;
use hertzforge_mimo_shock::{
    ClassicalControl, DriveChannel, EndOutcome, GuardPlan, IdentificationPlan, LadderPlan,
    LimitChannel, MimoShockConfig, MimoShockEngine, MimoShockLaw, MimoShockLawFactory, Setup,
    ShotError, ShotPath, SrsSpec, Target, TargetKind, TargetSpec, Timing, WaveformTarget,
};
use hertzforge_mimo_shock_laws::{CaseRun, CaseSpec, CaseVerdict, Perturbation};
use hertzforge_module_abi::StationClass;
use hertzforge_sdm::C64;
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault};
use hertzforge_shock_common::shot::ShotHooks;
use hertzforge_shock_host::service::firer::{BackendPulseFirer, FireControl, PulseRows};
use hertzforge_shock_host::spec::{ClassicalPulseSpec, ClassicalShape};
use hertzforge_shock_host::tolerance::ToleranceStandard;
use hertzforge_shock_host::{PulseFirer, ServiceError};
use hertzforge_srs::iterate::{SynthesisEngine, SynthesisOpts};
use hertzforge_srs::table::WaveletTable;
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// The rate every case runs at, Hz (the shock plants' calibration rate).
pub const CHECK_FS_HZ: f64 = 12_800.0;
/// Classical records.
pub const CHECK_LEN_CLASSICAL: usize = 8192;
/// SRS records.
pub const CHECK_LEN_SRS: usize = 16384;

fn plant(name: &str) -> hertzforge_hal_sim::plants::ShockPlant {
    hertzforge_hal_sim::plants::ShockPlant::from_name(name).expect("a standard shock plant")
}

/// A simulator station of the shock plant `name`.
fn station(name: &str, seed: u64) -> Box<dyn Backend + Send> {
    Box::new(
        hertzforge_hal_sim::SimConfig::new()
            .sample_rate_hz(CHECK_FS_HZ)
            .noise_level(1e-4)
            .shared_clock(true)
            .seed(seed)
            .plant(plant(name).build().model)
            .build(),
    )
}

/// `nc` control rows, `nl` monitor rows, then `nd` drive rows, from index 1.
fn table(nc: usize, nl: usize, nd: usize) -> ChannelTable {
    let mut t = ChannelTable::new();
    for k in 0..nc + nl + nd {
        let mut c = Channel::new(k as u32 + 1);
        c.role = if k < nc {
            ChannelRole::Control
        } else if k < nc + nl {
            ChannelRole::Monitor
        } else {
            ChannelRole::Drive
        };
        c.range_max = 1e9;
        t.channels.push(c);
    }
    t
}

/// The check's classical targets: half-sines of 20 and 18 g in turn, 11 ms,
/// rectangular shoulders four times the duration.
fn classical(nc: usize) -> TargetSpec {
    TargetSpec::Classical {
        controls: (0..nc)
            .map(|c| {
                let mut pulse = ClassicalPulseSpec::new(
                    ClassicalShape::HalfSine,
                    if c % 2 == 0 { 20.0 } else { 18.0 },
                    11.0,
                );
                pulse.compensation = CompensationConfig {
                    shape: CompensationShape::Rectangular,
                    pre_frac: 4.0,
                    post_frac: 4.0,
                    symmetric: true,
                };
                ClassicalControl {
                    pulse,
                    inverted: false,
                }
            })
            .collect(),
        standard: ToleranceStandard::Mil810_516,
        lead_s: 0.05,
    }
}

/// The check's SRS targets: +6 dB an octave from 50 Hz to 300 Hz, then flat at
/// 20 or 16 g in turn to 2 kHz, Q 10, third octaves.
fn srs(nc: usize, tol: (f64, f64)) -> TargetSpec {
    let f = octave_freqs(50.0, 2000.0, OctaveFraction::Third);
    TargetSpec::Srs {
        spec: SrsSpec {
            points: (0..nc)
                .map(|c| {
                    let a = if c % 2 == 0 { 20.0 } else { 16.0 };
                    f.iter()
                        .map(|&fz| if fz < 300.0 { a * fz / 300.0 } else { a })
                        .collect()
                })
                .collect(),
            natural_hz: f,
            q: 10.0,
            tol_lo_db: tol.0,
            tol_hi_db: tol.1,
            tables: (0..nc)
                .map(|_| {
                    SynthesisEngine::Wavsyn(
                        WaveletTable::default_over_band(50.0, 2000.0, OctaveFraction::Third)
                            .expect("a table"),
                    )
                })
                .collect(),
            synthesis: SynthesisOpts::default(),
            inverted: Vec::new(),
        },
        lead_s: 0.05,
    }
}

fn config(spec: &CaseSpec, nc: usize, nl: usize, nd: usize, limit: f64) -> MimoShockConfig {
    let len = match spec.kind {
        TargetKind::Waveform => CHECK_LEN_CLASSICAL,
        TargetKind::Srs => CHECK_LEN_SRS,
    };
    MimoShockConfig {
        fs_hz: CHECK_FS_HZ,
        record_len: len,
        band_lo_hz: 5.0,
        band_hi_hz: 2500.0,
        controls: (1..=nc as u32).collect(),
        drives: (0..nd)
            .map(|d| DriveChannel {
                channel: (nc + nl + d) as u32 + 1,
                ceiling_v: 10.0,
                abort_v: 15.0,
            })
            .collect(),
        limits: (0..nl)
            .map(|l| LimitChannel {
                channel: (nc + l) as u32 + 1,
                abort_peak: limit,
            })
            .collect(),
        target: Target::Waveform(WaveformTarget {
            records: vec![0.0; nc * len],
        }),
        timing: Timing::Free { window_s: 0.002 },
        ladder: LadderPlan {
            levels_db: spec.ladder_db.clone(),
            max_pulses_per_level: spec.pulses_per_level,
            ..LadderPlan::default()
        },
        identification: IdentificationPlan {
            drive_vrms: 1.0,
            frames: 40,
            coherence_floor: 0.5,
            review: false,
        },
        guards: GuardPlan::default(),
        min_spacing_s: 1.0,
    }
}

/// The law's model moved by a delay and scaled by a gain.
struct Perturbed {
    inner: Arc<dyn MimoShockLawFactory>,
    delay_s: f64,
    gain: f64,
}

impl MimoShockLawFactory for Perturbed {
    fn build(&self, setup: &Setup) -> Result<Box<dyn MimoShockLaw + Send>, LawFault> {
        let mut s = setup.clone();
        let df = s.fs_hz / s.record_len as f64;
        for f in 0..s.record_len / 2 + 1 {
            let w = -2.0 * std::f64::consts::PI * f as f64 * df * self.delay_s;
            let k = C64::new(w.cos(), w.sin()) * self.gain;
            for v in s.plant.frf.line_mut(f).iter_mut() {
                *v *= k;
            }
        }
        self.inner.build(&s)
    }

    fn describe(&self) -> LawDescriptor {
        self.inner.describe()
    }

    fn manifest(&self) -> hertzforge_mimo_shock::LawManifest {
        self.inner.manifest()
    }
}

/// The host firer on a simulator, adding noise to the control rows.
struct NoisyShot {
    firer: Option<BackendPulseFirer<Box<dyn Backend + Send>>>,
    /// Each control's channel and its noise rms, EU.
    noise: Vec<(u32, f64)>,
    rng: Rng,
}

impl NoisyShot {
    fn gauss(&mut self) -> f64 {
        let u1 = self.rng.uniform().max(1e-300);
        let u2 = self.rng.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

impl ShotPath for NoisyShot {
    fn arm(&mut self, fs_hz: f64) -> Result<f64, String> {
        let f = self.firer.take().ok_or("the station is already armed")?;
        let mut f = f.with_requested_rate(fs_hz);
        f.arm().map_err(|e| e.to_string())?;
        let fs = f
            .actual_fs_hz()
            .ok_or("the station did not say what rate it runs at")?;
        self.firer = Some(f);
        Ok(fs)
    }

    fn set_quiet_frames(&mut self, frames: usize) {
        if let Some(f) = self.firer.as_mut() {
            f.set_quiet_frames(frames);
        }
    }

    fn fire<'a>(
        &mut self,
        drive: &[f64],
        n_drive: usize,
        cancel: Option<&'a AtomicBool>,
        hooks: Option<&'a mut dyn ShotHooks>,
    ) -> Result<PulseRows, ShotError> {
        let f = self
            .firer
            .as_mut()
            .ok_or(ShotError::Failed("the station is not armed".into()))?;
        let mut ctl = FireControl {
            cancel,
            observer: None,
            hooks,
        };
        let mut rows = f.fire_rows(drive, n_drive, &mut ctl).map_err(|e| match e {
            ServiceError::Cancelled { .. } => ShotError::Cancelled,
            other => ShotError::Failed(other.to_string()),
        })?;
        for (ch, rms) in self.noise.clone() {
            if let Some(slot) = rows.channels.iter().position(|c| *c == ch) {
                let n = rows.rows[slot].len();
                for i in 0..n {
                    let g = self.gauss();
                    rows.rows[slot][i] += rms * g;
                }
            }
        }
        Ok(rows)
    }

    fn stop(&mut self) -> Result<(), String> {
        let r = match self.firer.as_mut() {
            Some(f) => f.stop().map_err(|e| e.to_string()),
            None => Ok(()),
        };
        self.firer = None;
        r
    }

    fn emergency_stop(&mut self) -> Result<(), String> {
        let r = match self.firer.as_mut() {
            Some(f) => f.emergency_stop().map_err(|e| e.to_string()),
            None => Ok(()),
        };
        self.firer = None;
        r
    }
}

/// One run of `spec` with `law`, the limit (if any) at `limit`.
fn run_once(
    spec: &CaseSpec,
    law: Arc<dyn MimoShockLawFactory>,
    limit: f64,
) -> Result<(CaseRun, MimoShockEngine), String> {
    if !PLANTS.contains(&spec.plant) {
        return Err(format!("no simulator plant \"{}\"", spec.plant));
    }
    let (n_in, nd) = plant(spec.plant).dims();
    let nl = usize::from(matches!(
        spec.perturbation,
        Perturbation::LimitChannel { .. }
    ));
    let nc = n_in - nl;
    let target = match spec.kind {
        TargetKind::Waveform => classical(nc),
        TargetKind::Srs => srs(nc, spec.srs_tolerance_db),
    };
    let law: Arc<dyn MimoShockLawFactory> = match spec.perturbation {
        Perturbation::ModelDelay { delay_s } => Arc::new(Perturbed {
            inner: law,
            delay_s,
            gain: 1.0,
        }),
        Perturbation::Identification { gain_db, .. } => Arc::new(Perturbed {
            inner: law,
            delay_s: 0.0,
            gain: 10f64.powf(gain_db / 20.0),
        }),
        _ => law,
    };
    let cfg = config(spec, nc, nl, nd, limit);
    let len = cfg.record_len;
    let noise: Vec<(u32, f64)> = match spec.perturbation {
        Perturbation::ResponseNoise { rms_rel } => {
            let built = target.build(CHECK_FS_HZ, len).map_err(|e| e.to_string())?;
            let w = built.target.waveforms();
            (0..nc)
                .map(|c| {
                    let r = &w[c * len..(c + 1) * len];
                    let rms = (r.iter().map(|v| v * v).sum::<f64>() / len as f64).sqrt();
                    (c as u32 + 1, rms_rel * rms)
                })
                .collect()
        }
        _ => Vec::new(),
    };
    let firer = BackendPulseFirer::new(
        station(spec.plant, spec.seed),
        table(nc, nl, nd),
        StationClass::HertzforgeSim,
    )
    .map_err(|e| e.to_string())?;
    let mut e = MimoShockEngine::new(cfg, law)
        .map_err(|e| e.to_string())?
        .with_target_spec(target)
        .with_seed(spec.seed)
        .with_pacing(false);
    e.arm_with(Box::new(NoisyShot {
        firer: Some(firer),
        noise,
        rng: Rng::new(spec.seed ^ 0x006e_6f69_7365),
    }))
    .map_err(|e| e.to_string())?;
    e.identify().map_err(|e| e.to_string())?;
    if e.end().is_none() {
        if e.unreachable().is_some() {
            e.acknowledge_unreachable("the check judges the law");
        }
        e.proceed().map_err(|e| e.to_string())?;
        if e.end().is_none() {
            e.run_to_end(None).map_err(|e| e.to_string())?;
        }
    }
    let end = e.end().map_or(EndOutcome::Fault, |x| x.outcome);
    Ok((
        CaseRun {
            pulses: e.pulses().to_vec(),
            end,
        },
        e,
    ))
}

/// Play `spec` with `law` on the simulator.
pub fn run_case(spec: &CaseSpec, law: Arc<dyn MimoShockLawFactory>) -> Result<CaseRun, String> {
    match spec.perturbation {
        Perturbation::LimitChannel { abort_rel } => {
            let (_, e) = run_once(spec, law.clone(), f64::INFINITY)?;
            let n = e.config().record_len;
            let nc = e.config().controls.len();
            let peak = e
                .records()
                .pulses
                .iter()
                .filter_map(|q| {
                    let slot = q.channels.iter().position(|&c| c == nc as u32 + 1)?;
                    let row = &q.rows[slot];
                    let a = q.quiet_frames.min(row.len());
                    let pk = row[a..(a + n).min(row.len())]
                        .iter()
                        .fold(0.0f64, |m, v| m.max(v.abs()));
                    Some(pk / 10f64.powf(q.level_db / 20.0))
                })
                .fold(0.0f64, f64::max);
            if peak <= 0.0 {
                return Err("the limit channel measured nothing".into());
            }
            run_once(spec, law, abort_rel * peak).map(|(r, _)| r)
        }
        _ => run_once(spec, law, f64::INFINITY).map(|(r, _)| r),
    }
}

/// Run the whole check for `law` on the target kinds it accepts, telling
/// `progress` as each case starts and ends.
pub fn run_check(
    law_id: &str,
    law: Arc<dyn MimoShockLawFactory>,
    progress: &mut dyn FnMut(Value),
) -> Result<Vec<CaseVerdict>, MethodError> {
    let kinds = law.manifest().targets;
    let of = hertzforge_mimo_shock_laws::MimoShockCheckCase::ALL.len() * kinds.len();
    let mut index = 0usize;
    let verdicts = hertzforge_mimo_shock_laws::run_check(&kinds, |spec| {
        progress(json!({
            "law_id": law_id, "index": index, "of": of,
            "case": spec.case.name(), "kind": spec.kind.as_str(),
            "state": "running", "result": null,
        }));
        let r = run_case(spec, law.clone());
        index += 1;
        r
    });
    for (i, v) in verdicts.iter().enumerate() {
        progress(json!({
            "law_id": law_id, "index": i, "of": of,
            "case": v.case.name(), "kind": v.kind.as_str(),
            "state": "done", "result": case_json(v),
        }));
    }
    Ok(verdicts)
}

/// One case's line in the check report.
pub fn case_json(v: &CaseVerdict) -> Value {
    json!({
        "name": v.record_name(),
        "case": v.case.name(),
        "kind": v.kind.as_str(),
        "plant": v.case.describe(),
        "passed": v.passed,
        "failed": v.failed,
        "number": crate::protocol::num(crate::views::round_to(v.number, 3)),
        "detail": v.detail,
    })
}

/// The `laws.check` report.
pub fn report_json(verdicts: &[CaseVerdict], recorded: bool) -> Value {
    json!({
        "passed": !verdicts.is_empty() && verdicts.iter().all(|v| v.passed),
        "recorded": recorded,
        "tool_version": hertzforge_mimo_shock_laws::MIMO_SHOCK_CHECK_VERSION,
        "cases": verdicts.iter().map(case_json).collect::<Vec<_>>(),
    })
}
