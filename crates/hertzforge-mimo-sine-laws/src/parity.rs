//! Two laws over one recorded sequence of inputs: whether a law file and its
//! native form give the same drives on the same inputs, update after update.
//!
//! [`compare_laws`] builds both laws from their factories for one setup and
//! hands each the same [`RecordedInputs`], whole sequences long, through the
//! core's own law seam ([`MimoSineLaw`]), so what is compared is what the
//! engine would receive. With the `reference-structure` feature,
//! [`bench::record`] makes such a sequence: a built-in law run in closed loop
//! on the law mathematics' exact linear structure, every update's inputs
//! kept.

use hertzforge_mimo_sine::law::LawValues;
use hertzforge_mimo_sine::{
    Hold, LawFault, LawSetup, MimoSineLaw, MimoSineLawFactory, TickInputs, TickOutputs,
};
use hertzforge_sdm::C64;

/// One update's inputs, owned, as recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedInputs {
    /// Updates since the law was built.
    pub index: u64,
    /// Seconds since the last call.
    pub dt_s: f64,
    /// The tone's frequency, Hz.
    pub frequency_hz: f64,
    /// The sweep rate, Hz per second.
    pub sweep_hz_per_s: f64,
    /// The sweep rate, octaves per minute.
    pub sweep_oct_per_min: f64,
    /// The scheduled level.
    pub level: f64,
    /// Each control's reference.
    pub reference: Vec<C64>,
    /// Each control's tracked amplitude.
    pub controls: Vec<C64>,
    /// Each limit channel's tracked amplitude.
    pub limits: Vec<C64>,
    /// Each limit channel's bound.
    pub limit_bounds: Vec<Option<f64>>,
    /// The drive in force.
    pub drive_in_force: Vec<C64>,
    /// The core's hold.
    pub hold: Hold,
    /// The notch's scale.
    pub notch_scale: f64,
    /// Whether the settle window is open.
    pub settling: bool,
}

impl RecordedInputs {
    /// The inputs as the core hands them to a law.
    pub fn as_tick(&self) -> TickInputs<'_> {
        TickInputs {
            index: self.index,
            dt_s: self.dt_s,
            frequency_hz: self.frequency_hz,
            sweep_hz_per_s: self.sweep_hz_per_s,
            sweep_oct_per_min: self.sweep_oct_per_min,
            level: self.level,
            reference: &self.reference,
            controls: &self.controls,
            limits: &self.limits,
            limit_bounds: &self.limit_bounds,
            drive_in_force: &self.drive_in_force,
            hold: self.hold,
            notch_scale: self.notch_scale,
            settling: self.settling,
        }
    }
}

/// One update's answer as recorded: the drive and the values reported.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// The drive.
    pub drive: Vec<C64>,
    /// Each declared value, `None` where it was not reported.
    pub values: Vec<Option<f64>>,
    /// The request.
    pub request: Option<hertzforge_mimo_sine::Request>,
}

/// Run `law`, built for `setup`, over `seq`: `engage` on the first update,
/// `tick` on every one after. The first fault ends the run, with its update.
pub fn run_sequence(
    law: &mut dyn MimoSineLaw,
    setup: &LawSetup,
    declared: usize,
    seq: &[RecordedInputs],
) -> Result<Vec<Answer>, (usize, LawFault)> {
    let mut out = TickOutputs::new(setup.drives, setup.controls);
    let mut values = LawValues::new(declared);
    let mut answers = Vec::with_capacity(seq.len());
    for (k, x) in seq.iter().enumerate() {
        out.reset();
        values.clear();
        let r = if k == 0 {
            law.engage(&x.as_tick(), &mut out, &mut values)
        } else {
            law.tick(&x.as_tick(), &mut out, &mut values)
        };
        r.map_err(|f| (k, f))?;
        answers.push(Answer {
            drive: out.drive.clone(),
            values: (0..declared).map(|i| values.get(i)).collect(),
            request: out.request,
        });
    }
    Ok(answers)
}

/// How far two laws' drives were apart over one sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct ParityReport {
    /// Updates compared.
    pub updates: usize,
    /// The largest difference of a drive, over the largest drive of that
    /// update, over every update.
    pub worst_relative: f64,
    /// The update where it was.
    pub worst_update: usize,
    /// The largest difference of a reported value, relative to its size
    /// (at least 1).
    pub worst_value: f64,
    /// Updates where one law reported a value the other did not, or asked
    /// for a different request.
    pub mismatched_updates: usize,
}

/// The largest `|a - b|` over the largest `|b|`.
pub fn relative_difference(a: &[C64], b: &[C64]) -> f64 {
    let scale = b.iter().map(|v| v.norm()).fold(1e-300, f64::max);
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).norm())
        .fold(0.0, f64::max)
        / scale
}

/// Compare two sets of answers to one sequence.
pub fn compare_answers(a: &[Answer], b: &[Answer]) -> ParityReport {
    let mut r = ParityReport {
        updates: a.len().min(b.len()),
        worst_relative: 0.0,
        worst_update: 0,
        worst_value: 0.0,
        mismatched_updates: a.len().abs_diff(b.len()),
    };
    for (k, (x, y)) in a.iter().zip(b).enumerate() {
        let d = if x.drive.len() == y.drive.len() {
            relative_difference(&x.drive, &y.drive)
        } else {
            f64::INFINITY
        };
        if d > r.worst_relative {
            r.worst_relative = d;
            r.worst_update = k;
        }
        let mut mismatch = x.request != y.request || x.values.len() != y.values.len();
        for (u, v) in x.values.iter().zip(&y.values) {
            match (u, v) {
                (Some(u), Some(v)) => {
                    r.worst_value = r.worst_value.max((u - v).abs() / v.abs().max(1.0));
                }
                (None, None) => {}
                _ => mismatch = true,
            }
        }
        if mismatch {
            r.mismatched_updates += 1;
        }
    }
    r
}

/// Build `a` and `b` for `setup` and run both over `seq`; the report of how
/// far their answers were apart, or the first law that failed and why.
pub fn compare_laws(
    a: &dyn MimoSineLawFactory,
    b: &dyn MimoSineLawFactory,
    setup: &LawSetup,
    seq: &[RecordedInputs],
) -> Result<ParityReport, String> {
    let run = |f: &dyn MimoSineLawFactory, which: &str| -> Result<Vec<Answer>, String> {
        let declared = f.manifest().values.len();
        let mut law = f
            .build(setup)
            .map_err(|e| format!("{which} was not built: {e}"))?;
        run_sequence(law.as_mut(), setup, declared, seq)
            .map_err(|(k, e)| format!("{which} failed at update {k}: {e}"))
    };
    Ok(compare_answers(
        &run(a, "the first law")?,
        &run(b, "the second law")?,
    ))
}

/// Recorded sequences on the law mathematics' exact linear structure.
#[cfg(feature = "reference-structure")]
pub mod bench {
    use super::RecordedInputs;
    use hertzforge_law_sdk::sine_mimo::builtin::{config, BuiltLaw, LawDecl};
    use hertzforge_law_sdk::Params;
    use hertzforge_mimo_sine::{
        Hold, LawIdentification, LawSetup, LawTolerance, MimoShape, TestType, Tracking,
    };
    use hertzforge_mimo_sine_law_math as math;
    use math::law::{Inputs, Outputs, SineLaw};
    pub use math::reference_structure::ModalStructure;
    use math::reference_structure::{run_closed_loop, Dynamics, Played, Schedule};

    /// The core's law setup of a mathematics setup, for a run of `test_type`.
    /// Each control's alarm and abort band is its band on every line.
    pub fn law_setup(s: &math::Setup, test_type: TestType) -> LawSetup {
        let id = &s.identification;
        let lines = id.grid_hz.len();
        let per = |v: &[f64]| -> Vec<f64> {
            v.iter()
                .flat_map(|x| std::iter::repeat_n(*x, lines))
                .collect()
        };
        LawSetup {
            controls: s.controls,
            drives: s.drives,
            limits: s.limits,
            fs_hz: s.fs_hz,
            tick_s: s.tick_s,
            band_lo_hz: s.band_lo_hz,
            band_hi_hz: s.band_hi_hz,
            test_type,
            shape: MimoShape::of(s.controls, s.drives),
            drive_ceiling_v: s.drive_ceiling_v.clone(),
            identification: LawIdentification {
                frequency_hz: id.grid_hz.clone(),
                response: id.response.clone(),
                limit_response: id.limit_response.clone(),
                coherence: id.coherence.clone(),
                limit_coherence: id.limit_coherence.clone(),
                trusted: id.trusted.clone(),
            },
            identification_level_db: Some(s.identification_level_db).filter(|v| v.is_finite()),
            reference: s.reference.clone(),
            phase_specified: s.phase_specified,
            limit_bounds: s
                .limit_bounds
                .iter()
                .map(|b| Some(*b).filter(|b| b.is_finite()))
                .collect(),
            tolerance: LawTolerance {
                alarm_low_db: per(&s.alarm_db),
                alarm_high_db: per(&s.alarm_db),
                abort_low_db: per(&s.abort_db),
                abort_high_db: per(&s.abort_db),
            },
            tracking: match s.tracking {
                math::TrackingRule::Proportional { pct, max_hz } => {
                    Tracking::Proportional { pct, max_hz }
                }
                math::TrackingRule::Fixed { hz } => Tracking::Fixed { hz },
            },
            tracker_delay_factor: s.loop_delay.tracker_delay_factor,
            transport_delay_s: s.loop_delay.transport_delay_s,
        }
    }

    /// A law that keeps every update's inputs and drive.
    struct Recorder<'a> {
        law: BuiltLaw,
        seq: &'a mut Vec<RecordedInputs>,
        asked: &'a mut Vec<Vec<math::C64>>,
    }

    impl Recorder<'_> {
        fn keep(&mut self, x: &Inputs) {
            self.seq.push(RecordedInputs {
                index: x.index,
                dt_s: x.dt_s,
                frequency_hz: x.f_hz,
                sweep_hz_per_s: x.sweep_hz_s,
                sweep_oct_per_min: x.sweep_oct_min,
                level: x.level,
                reference: x.reference.to_vec(),
                controls: x.controls.to_vec(),
                limits: x.limits.to_vec(),
                limit_bounds: x
                    .limit_bounds
                    .iter()
                    .map(|b| Some(*b).filter(|b| b.is_finite()))
                    .collect(),
                drive_in_force: x.drive_in_force.to_vec(),
                hold: match x.hold {
                    math::Hold::Free => Hold::Free,
                    math::Hold::Notch => Hold::Notch,
                    math::Hold::Held => Hold::Held,
                    math::Hold::Probe => Hold::Probe,
                    math::Hold::Engage => Hold::Engage,
                },
                notch_scale: x.notch_scale,
                settling: x.settle_open,
            });
        }
    }

    impl SineLaw for Recorder<'_> {
        fn engage(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
            self.keep(x);
            self.law.engage(x, out)?;
            self.asked.push(out.drive.clone());
            Ok(())
        }

        fn tick(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
            self.keep(x);
            self.law.tick(x, out)?;
            self.asked.push(out.drive.clone());
            Ok(())
        }

        fn value_names(&self) -> &'static [&'static str] {
            self.law.value_names()
        }
    }

    /// A recorded run: the setup, every update's inputs, and the drive the
    /// mathematics asked for on each.
    #[derive(Debug, Clone)]
    pub struct Recording {
        /// The setup, as the core hands it to a law.
        pub setup: LawSetup,
        /// Every update's inputs.
        pub inputs: Vec<RecordedInputs>,
        /// The drive the law's mathematics asked for on each update.
        pub asked: Vec<Vec<math::C64>>,
    }

    /// Run the built-in law `decl` with `params` on `structure` through
    /// `schedule` (closed loop, no guards, the structure in steady state with
    /// the tracking filter's lag), keeping every update's inputs: at most
    /// `max_updates` updates.
    pub fn record(
        decl: &LawDecl,
        params: &Params,
        structure: &ModalStructure,
        setup: &math::Setup,
        schedule: Schedule,
        test_type: TestType,
        max_updates: usize,
    ) -> Result<Recording, String> {
        let law = config(decl.id, params)?.build(setup)?;
        let names = law.value_names().len();
        let (mut seq, mut asked) = (Vec::new(), Vec::new());
        let mut rec = Recorder {
            law,
            seq: &mut seq,
            asked: &mut asked,
        };
        run_closed_loop(
            &mut rec,
            structure,
            setup,
            schedule,
            Played::AsAsked,
            Dynamics::QuasiSteady,
            names,
            max_updates,
        )?;
        Ok(Recording {
            setup: law_setup(setup, test_type),
            inputs: seq,
            asked,
        })
    }
}
