//! Running two MIMO Shock laws side by side: [`compare_laws`] plays a scripted
//! run (a ladder of levels through a plant the caller supplies) and feeds both
//! laws the same inputs after every pulse, then reports how far apart their
//! drives were. It is how a law file is shown to run exactly as the same
//! mathematics runs natively ([`MathLaw`]), and how a native built-in is shown
//! to match its law file.

use hertzforge_law_sdk::shock_mimo::math::{self, LawSteps};
use hertzforge_mimo_shock::{
    ControlVerdict, MimoShockLaw, PulseInputs, PulseOutputs, Setup, Target,
};
use hertzforge_shock_common::law_guard::{LawFault, LawValue};

/// `n` rows of `len` from a flat channel-major list.
fn rows(flat: &[f64], len: usize) -> Vec<Vec<f64>> {
    flat.chunks(len.max(1)).map(<[f64]>::to_vec).collect()
}

/// The core's setup as the law mathematics takes it.
pub fn math_setup(s: &Setup) -> math::Setup {
    math::Setup {
        fs_hz: s.fs_hz,
        record_len: s.record_len,
        n_drives: s.n_drives,
        n_controls: s.n_controls,
        band_lo_hz: s.band_lo_hz,
        band_hi_hz: s.band_hi_hz,
        reference: rows(s.target.waveforms(), s.record_len),
        srs: match &s.target {
            Target::Waveform(_) => None,
            Target::Srs(t) => Some(math::SrsTarget {
                natural_hz: t.natural_hz.clone(),
                q: t.q,
                points: rows(&t.points, t.natural_hz.len()),
            }),
        },
        plant: math::PlantModel {
            frf: s.plant.frf.clone(),
            trusted: s.plant.trusted.clone(),
        },
    }
}

/// The core's pulse inputs as the law mathematics takes them (whose pulse
/// index counts the pulse just played from 0).
pub fn math_inputs(i: &PulseInputs, s: &Setup) -> math::PulseInputs {
    let len = s.record_len;
    math::PulseInputs {
        pulse_index: i.pulse_index.saturating_sub(1) as usize,
        level_db: i.level_db,
        next_level_db: i.next_level_db,
        drives: rows(&i.drives, len),
        responses: rows(&i.responses, len),
        srs: match (&i.srs, &s.target) {
            (Some(m), Target::Srs(t)) => Some(rows(m, t.natural_hz.len())),
            _ => None,
        },
    }
}

fn outputs(o: math::PulseOutputs) -> PulseOutputs {
    PulseOutputs {
        drives: o.drives.into_iter().flatten().collect(),
        delays_s: o.delays_s,
        values: o
            .values
            .into_iter()
            .map(|(k, v)| LawValue::new(k.to_string(), v))
            .collect(),
    }
}

/// A law of the shared mathematics ([`LawSteps`]) called natively through the
/// core's law trait. A reference for parity tests: the native built-in laws
/// are the engine's own adapters, and the law files are compared with both.
pub struct MathLaw {
    name: String,
    setup: Setup,
    steps: Box<dyn LawSteps + Send>,
}

impl MathLaw {
    /// `steps` under `name`, for the run `setup`.
    pub fn new(name: &str, setup: &Setup, steps: Box<dyn LawSteps + Send>) -> Self {
        MathLaw {
            name: name.into(),
            setup: setup.clone(),
            steps,
        }
    }

    /// `inverse` with `params`.
    pub fn inverse(setup: &Setup, params: math::inverse::InverseParams) -> Result<Self, String> {
        let law = math::inverse::InverseLaw::new(math_setup(setup), params)?;
        Ok(Self::new("inverse", setup, Box::new(law)))
    }

    /// `shape` with `params`.
    pub fn shape(setup: &Setup, params: math::shape::ShapeParams) -> Result<Self, String> {
        let law = math::shape::ShapeLaw::new(math_setup(setup), params)?;
        Ok(Self::new("shape", setup, Box::new(law)))
    }

    /// `paired` with `params`.
    pub fn paired(setup: &Setup, params: math::paired::PairedParams) -> Result<Self, String> {
        let law = math::paired::PairedLaw::new(math_setup(setup), params)?;
        Ok(Self::new("paired", setup, Box::new(law)))
    }
}

impl MimoShockLaw for MathLaw {
    fn name(&self) -> &str {
        &self.name
    }

    fn seed(&mut self, level_db: f64) -> Result<PulseOutputs, LawFault> {
        self.steps
            .seed(level_db)
            .map(outputs)
            .map_err(LawFault::Failed)
    }

    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, LawFault> {
        self.steps
            .iterate(&math_inputs(inputs, &self.setup))
            .map(outputs)
            .map_err(LawFault::Failed)
    }
}

/// How far apart two laws' drives were over a scripted run.
#[derive(Debug, Clone, PartialEq)]
pub struct ParityReport {
    /// Calls compared: the seed and every iteration.
    pub calls: usize,
    /// The largest difference of any drive sample on any call, relative to
    /// the largest drive sample of that call.
    pub worst_rel: f64,
    /// The call it happened on (0 is the seed).
    pub worst_call: usize,
    /// The largest difference of any reported value or delay, relative to its
    /// size (or absolute under 1).
    pub worst_value_rel: f64,
    /// Whether both reported the same value keys and delay presence on every
    /// call.
    pub same_keys: bool,
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / a.abs().max(1.0)
}

/// Each control's waveform error against its target at the played level, dB:
/// the deterministic stand-in verdict a scripted run hands both laws.
fn verdicts(setup: &Setup, responses: &[f64], level_db: f64) -> Vec<ControlVerdict> {
    let s = 10f64.powf(level_db / 20.0);
    let len = setup.record_len;
    let r = setup.target.waveforms();
    (0..setup.n_controls)
        .map(|c| {
            let (y, t) = (&responses[c * len..][..len], &r[c * len..][..len]);
            let e: f64 = y.iter().zip(t).map(|(y, t)| (y - s * t).powi(2)).sum();
            let n: f64 = t.iter().map(|t| (s * t).powi(2)).sum();
            let error_db = 10.0 * (e.max(1e-300) / n.max(1e-300)).log10();
            let peak = |x: &[f64]| x.iter().fold(0.0f64, |a, v| a.max(v.abs()));
            ControlVerdict {
                passed: error_db < -20.0,
                error_db,
                delay_s: 0.0,
                peak_ratio: peak(y) / (s * peak(t)).max(1e-300),
            }
        })
        .collect()
}

/// The plant of a scripted run: the flat drives as played (drive after
/// drive) and their level in dB to the flat responses (control after control).
pub type ScriptedPlant<'a> = dyn FnMut(&[f64], f64) -> Result<Vec<f64>, String> + 'a;

/// Play `levels` (one pulse per entry) through `plant` with law `a`'s drives,
/// hand both laws the same inputs after every pulse (`b`'s own drives are
/// compared, never played) and report how far apart the drives were.
///
/// `plant` maps the flat drives as played (drive after drive) and the level
/// they were played at to the flat responses (control after control). For an
/// SRS target the measured SRS is computed from the responses on the target's
/// frequencies; the verdicts are each control's waveform error.
pub fn compare_laws(
    a: &mut dyn MimoShockLaw,
    b: &mut dyn MimoShockLaw,
    setup: &Setup,
    levels: &[f64],
    plant: &mut ScriptedPlant<'_>,
) -> Result<ParityReport, String> {
    let first = *levels.first().ok_or("the script has no pulse")?;
    let fault = |w: &str, f: LawFault| format!("law {w}: {} ({})", f.reason(), f_text(&f));
    let mut oa = a.seed(first).map_err(|f| fault("a", f))?;
    let mut ob = b.seed(first).map_err(|f| fault("b", f))?;
    let mut report = ParityReport {
        calls: 0,
        worst_rel: 0.0,
        worst_call: 0,
        worst_value_rel: 0.0,
        same_keys: true,
    };
    for k in 0..levels.len() {
        report.compare(k, &oa, &ob)?;
        if k + 1 == levels.len() {
            break;
        }
        let responses = plant(&oa.drives, levels[k])?;
        let srs = match &setup.target {
            Target::Waveform(_) => None,
            Target::Srs(t) => Some(
                math::srs::srs_rows(
                    &rows(&responses, setup.record_len),
                    setup.fs_hz,
                    &t.natural_hz,
                    t.q,
                    1.0,
                )
                .into_iter()
                .flatten()
                .collect(),
            ),
        };
        let inputs = PulseInputs {
            pulse_index: k as u64 + 1,
            level_db: levels[k],
            next_level_db: levels[k + 1],
            verdicts: verdicts(setup, &responses, levels[k]),
            drives: oa.drives.clone(),
            responses,
            srs,
            limited: false,
        };
        oa = a.iterate(&inputs).map_err(|f| fault("a", f))?;
        ob = b.iterate(&inputs).map_err(|f| fault("b", f))?;
    }
    Ok(report)
}

fn f_text(f: &LawFault) -> String {
    match f {
        LawFault::Failed(s) | LawFault::Crashed(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

impl ParityReport {
    fn compare(&mut self, call: usize, a: &PulseOutputs, b: &PulseOutputs) -> Result<(), String> {
        if a.drives.len() != b.drives.len() {
            return Err(format!(
                "call {call}: {} drive samples against {}",
                a.drives.len(),
                b.drives.len()
            ));
        }
        let peak = a.drives.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let diff = a
            .drives
            .iter()
            .zip(&b.drives)
            .fold(0.0f64, |m, (x, y)| m.max((x - y).abs()));
        let r = if peak > 0.0 { diff / peak } else { diff };
        if r > self.worst_rel || r.is_nan() {
            self.worst_rel = r;
            self.worst_call = call;
        }
        let keys = |o: &PulseOutputs| o.values.iter().map(|v| v.key.clone()).collect::<Vec<_>>();
        if keys(a) != keys(b) || a.delays_s.is_some() != b.delays_s.is_some() {
            self.same_keys = false;
        }
        for (x, y) in a.values.iter().zip(&b.values) {
            self.worst_value_rel = self.worst_value_rel.max(rel(x.value, y.value));
        }
        if let (Some(x), Some(y)) = (&a.delays_s, &b.delays_s) {
            for (x, y) in x.iter().zip(y) {
                self.worst_value_rel = self.worst_value_rel.max(rel(*x, *y));
            }
        }
        self.calls += 1;
        Ok(())
    }
}

/// A scripted run on the law mathematics' exact reference plant (feature
/// `reference-plant`): a 4 x 4 coupled structure whose modes soften and damp
/// with level, a target at every control, and a law file run beside a native
/// law over the same pulses.
#[cfg(feature = "reference-plant")]
pub mod bench {
    use super::{compare_laws, ParityReport};
    use crate::{load_law_file, WasmMimoShockLawFactory};
    use hertzforge_law_host::{HostConfig, LawHost, Param};
    use hertzforge_law_sdk::shock_mimo::math::reference_plant::{
        reference_shock, srs_grid_hz, ModalPlant,
    };
    use hertzforge_law_sdk::shock_mimo::math::{level_scale, spectrum, srs};
    use hertzforge_mimo_shock::{
        MimoShockLaw, MimoShockLawFactory, PlantStack, Setup, SrsTarget, Target, TargetKind,
        Timing, WaveformTarget,
    };
    use std::path::PathBuf;
    use std::time::Duration;

    /// Sample rate of the bench, Hz.
    pub const FS_HZ: f64 = 8192.0;
    /// Record length of the bench.
    pub const RECORD_LEN: usize = 2048;
    /// The pulses of a parity run, dB: two at each of three levels, so the
    /// run holds a level and steps it.
    pub const PARITY_LEVELS: [f64; 6] = [-12.0, -12.0, -6.0, -6.0, 0.0, 0.0];

    /// The bench: a run's setup and the plant behind it.
    pub struct Bench {
        /// The setup a law is built with.
        pub setup: Setup,
        /// The plant the pulses are played through.
        pub plant: ModalPlant,
    }

    impl Bench {
        /// The 4 x 4 reference plant (frequencies down 5 % and damping up by
        /// half at full level), identified exactly at -12 dB, with the
        /// reference shock as every control's target, for `kind`.
        pub fn four_by_four(kind: TargetKind) -> Bench {
            let plant = ModalPlant::four_by_four(0.05, 2.0);
            let r = reference_shock(FS_HZ, RECORD_LEN);
            let (nc, nd) = (plant.n_c, plant.n_d);
            // Each control a little different, so a law cannot pass by
            // treating them alike.
            let rows: Vec<Vec<f64>> = (0..nc)
                .map(|c| r.iter().map(|v| v * (1.0 - 0.1 * c as f64)).collect())
                .collect();
            let waveforms: Vec<f64> = rows.iter().flatten().copied().collect();
            let lines = RECORD_LEN / 2 + 1;
            let target = match kind {
                TargetKind::Waveform => Target::Waveform(WaveformTarget { records: waveforms }),
                TargetKind::Srs => {
                    let grid = srs_grid_hz();
                    let points = srs::srs_rows(&rows, FS_HZ, &grid, 10.0, 1.0);
                    Target::Srs(SrsTarget {
                        natural_hz: grid,
                        q: 10.0,
                        points: points.into_iter().flatten().collect(),
                        tol_lo_db: -1.5,
                        tol_hi_db: 3.0,
                        waveforms,
                        components: vec![Vec::new(); nc],
                    })
                }
            };
            let setup = Setup {
                fs_hz: FS_HZ,
                record_len: RECORD_LEN,
                n_drives: nd,
                n_controls: nc,
                band_lo_hz: 50.0,
                band_hi_hz: 3000.0,
                target,
                timing: Timing::Free { window_s: 0.002 },
                plant: PlantStack {
                    frf: plant.frf(level_scale(-12.0), FS_HZ, RECORD_LEN),
                    coherence: vec![1.0; nc * lines],
                    trusted: vec![true; lines],
                },
                drive_ceiling_v: vec![1.0e3; nd],
                ladder_db: vec![-12.0, -6.0, 0.0],
            };
            Bench { setup, plant }
        }

        /// The plant as [`compare_laws`] calls it: flat drives as played and
        /// their level to flat responses.
        pub fn respond(&self, drives: &[f64], level_db: f64) -> Result<Vec<f64>, String> {
            let rows: Vec<Vec<f64>> = drives
                .chunks(self.setup.record_len)
                .map(<[f64]>::to_vec)
                .collect();
            let y = self.plant.respond(&rows, level_scale(level_db), FS_HZ)?;
            Ok(y.into_iter().flatten().collect())
        }

        /// The rms of every target together, at full level.
        pub fn target_rms(&self) -> f64 {
            let rows: Vec<Vec<f64>> = self
                .setup
                .target
                .waveforms()
                .chunks(self.setup.record_len)
                .map(<[f64]>::to_vec)
                .collect();
            spectrum::rms_all(&rows)
        }
    }

    /// What one pulse of [`play`] measured.
    #[derive(Debug, Clone, PartialEq)]
    pub struct BenchRow {
        /// The level played, dB.
        pub level_db: f64,
        /// Every control's waveform error, response over target at the played
        /// level: `20 log10(|y - s r| / |s r|)` over every control together.
        pub waveform_error_db: f64,
        /// For an SRS target, the largest SRS deviation over every control and
        /// frequency, dB.
        pub worst_srs_db: Option<f64>,
        /// Rms of every drive together, referred to full level, volts.
        pub drive_rms_full_level: f64,
    }

    /// Play `levels` on `bench` with `law` alone, each pulse with the drives
    /// it returned, and measure each pulse.
    pub fn play(
        law: &mut dyn MimoShockLaw,
        bench: &Bench,
        levels: &[f64],
    ) -> Result<Vec<BenchRow>, String> {
        let s = &bench.setup;
        let len = s.record_len;
        let rows =
            |flat: &[f64]| -> Vec<Vec<f64>> { flat.chunks(len).map(<[f64]>::to_vec).collect() };
        let fault =
            |f: hertzforge_shock_common::law_guard::LawFault| format!("{} ({f:?})", f.reason());
        let mut out = law.seed(levels[0]).map_err(fault)?;
        let mut table = Vec::new();
        for (k, &level) in levels.iter().enumerate() {
            let sc = level_scale(level);
            let y = bench.respond(&out.drives, level)?;
            let r = s.target.waveforms();
            let e: f64 = y.iter().zip(r).map(|(y, r)| (y - sc * r).powi(2)).sum();
            let n: f64 = r.iter().map(|r| (sc * r).powi(2)).sum();
            let srs = match &s.target {
                Target::Srs(t) => Some(srs::srs_rows(&rows(&y), s.fs_hz, &t.natural_hz, t.q, 1.0)),
                Target::Waveform(_) => None,
            };
            let worst_srs_db = match (&srs, &s.target) {
                (Some(m), Target::Srs(t)) => {
                    let target: Vec<Vec<f64>> = t
                        .points
                        .chunks(t.natural_hz.len())
                        .map(|p| p.iter().map(|v| v * sc).collect())
                        .collect();
                    Some(srs::worst_error_db(m, &target))
                }
                _ => None,
            };
            table.push(BenchRow {
                level_db: level,
                waveform_error_db: 10.0 * (e / n).log10(),
                worst_srs_db,
                drive_rms_full_level: spectrum::rms_all(&rows(&out.drives)) / sc,
            });
            if k + 1 == levels.len() {
                break;
            }
            let inputs = hertzforge_mimo_shock::PulseInputs {
                pulse_index: k as u64 + 1,
                level_db: level,
                next_level_db: levels[k + 1],
                drives: out.drives.clone(),
                srs: srs.map(|m| m.into_iter().flatten().collect()),
                verdicts: vec![
                    hertzforge_mimo_shock::ControlVerdict {
                        passed: false,
                        error_db: 0.0,
                        delay_s: 0.0,
                        peak_ratio: 1.0,
                    };
                    s.n_controls
                ],
                responses: y,
                limited: false,
            };
            out = law.iterate(&inputs).map_err(fault)?;
        }
        Ok(table)
    }

    /// The built law file `<crate_name>.wasm` (as `scripts/build_laws.sh`
    /// leaves it under the target directory), found from the running test.
    pub fn law_file(crate_name: &str) -> PathBuf {
        let file = format!("{crate_name}.wasm");
        let exe = std::env::current_exe().expect("the test binary knows its own path");
        for dir in exe.ancestors().skip(1).take(4) {
            let p = dir.join("wasm32-wasip2").join("release").join(&file);
            if p.is_file() {
                return p;
            }
        }
        panic!("the law file {file} is not built. Run `bash scripts/build_laws.sh`.");
    }

    /// Run the law file `crate_name` with `params` beside `native` over
    /// [`PARITY_LEVELS`] on `bench`.
    pub fn law_file_against(
        crate_name: &str,
        params: Vec<Param>,
        native: &mut dyn MimoShockLaw,
        bench: &Bench,
    ) -> Result<ParityReport, String> {
        let host = LawHost::new(HostConfig::default()).map_err(|e| e.to_string())?;
        let law = load_law_file(&host, &law_file(crate_name)).map_err(|e| e.to_string())?;
        let factory = WasmMimoShockLawFactory::new(law, params, Duration::from_secs(60));
        let mut file = factory
            .build(&bench.setup)
            .map_err(|f| format!("{} ({f:?})", f.reason()))?;
        compare_laws(
            file.as_mut(),
            native,
            &bench.setup,
            &PARITY_LEVELS,
            &mut |d, l| bench.respond(d, l),
        )
    }
}
