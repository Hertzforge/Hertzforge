//! Single-output Sine laws: the `sine-single` world.
//!
//! A Sine law decides one number per control tick: the amplitude of the one
//! tone the controller plays, in drive units peak (volts peak on hardware).
//! The controller keeps everything else: the frequency, the sweep and its
//! direction, the tone's phase, the level schedule, the second reference
//! output, and every protection. The law is built once per run after system
//! identification ([`SineSingleLaw::new`], with the identification in
//! [`SineSetup`]), called once for the starting drive
//! ([`SineSingleLaw::engage`]) and then once a tick
//! ([`SineSingleLaw::tick`]), on the controller's real-time thread: each call
//! has a time budget of a fraction of the tick (2.5 ms at the default 10 ms
//! tick), and a call past it is not used.
//!
//! What the controller does with the drive a law returns, whatever the law:
//! it refuses one that is not a finite number at least zero (a miss; the
//! drive in force is held, and a run of misses ends the run with the
//! controlled stop), caps its change per tick and its rise per second, bounds
//! the first drive near the identification's prediction, holds it under the
//! limit channels and the drive ceiling, and aborts on the drive abort. While
//! [`Hold::Notch`] the law must not raise the drive (the controller holds it at
//! the notch ceiling); while [`Hold::Probe`] its drive is not used.
//!
//! A number parameter the manifest declares live
//! ([`SineManifestBuilder::live`]) can be changed by the operator during the
//! run: the controller checks the key and the range, then calls
//! [`SineSingleLaw::adjust`], and the value applies from the next tick.
//!
//! ```ignore
//! use hertzforge_law_sdk::sine::*;
//! use hertzforge_law_sdk::{Params, SineSingleLaw};
//!
//! struct Hold { drive: f64 }
//!
//! impl SineSingleLaw for Hold {
//!     fn describe() -> SineManifest {
//!         SineManifestBuilder::new("Hold", "0.1.0").build()
//!     }
//!     fn new(_setup: SineSetup, _params: Params) -> Result<Self, String> {
//!         Ok(Hold { drive: 0.0 })
//!     }
//!     fn engage(&mut self, i: &TickInputs) -> Result<TickOutputs, String> {
//!         self.drive = i.prediction_db.map(|db| 10f64.powf((db + i.level_db) / 20.0)).unwrap_or(0.01);
//!         Ok(drive_only(self.drive))
//!     }
//!     fn tick(&mut self, _: &TickInputs) -> Result<TickOutputs, String> {
//!         Ok(drive_only(self.drive))
//!     }
//! }
//! ```

use crate::__sine_bindings::exports::hertzforge::control_law::sine_single_law as wit_law;
use crate::{DiagnosticSpec, DiagnosticValue, Manifest, Param, ParamSpec, Params};
use core::cell::RefCell;

pub use crate::__sine_bindings::hertzforge::control_law::sine_types::{
    Hold, Identification, SineManifest, SineSetup, TickInputs, TickOutputs,
};

/// The most values a Sine law may report on one call.
pub const MAX_SINE_VALUES: usize = 8;

/// A single-output Sine control law.
///
/// The host builds one value per run with [`Self::new`], after system
/// identification, calls [`Self::engage`] once for the starting drive, then
/// [`Self::tick`] every control tick, and [`Self::adjust`] when the operator
/// changes a live parameter. Calls never overlap.
pub trait SineSingleLaw: Sized + 'static {
    /// The law's name, version, parameters, values and live parameters. Read
    /// once, when the file is loaded; it must not depend on anything but the
    /// code.
    fn describe() -> SineManifest;

    /// Build the law for a run. `params` holds every declared parameter,
    /// already checked against the manifest. An error refuses the run.
    fn new(setup: SineSetup, params: Params) -> Result<Self, String>;

    /// The first tick after identification: the starting drive.
    fn engage(&mut self, inputs: &TickInputs) -> Result<TickOutputs, String>;

    /// One control tick: the drive.
    fn tick(&mut self, inputs: &TickInputs) -> Result<TickOutputs, String>;

    /// The operator changed the live parameter `p` (its key declared live, its
    /// value within range). The default refuses: a law with live parameters
    /// overrides it. An error keeps the value in force and refuses the change.
    fn adjust(&mut self, p: &Param) -> Result<(), String> {
        Err(format!("`{}` cannot be changed while the test runs", p.key))
    }
}

/// A drive with no values.
pub fn drive_only(drive: f64) -> TickOutputs {
    TickOutputs {
        drive,
        faulted: false,
        values: Vec::new(),
    }
}

/// A drive with `values` as `(key, value)` pairs.
pub fn tick_outputs(drive: f64, values: &[(&str, f64)]) -> TickOutputs {
    TickOutputs {
        drive,
        faulted: false,
        values: values
            .iter()
            .map(|(k, v)| DiagnosticValue {
                key: (*k).to_string(),
                value: *v,
            })
            .collect(),
    }
}

/// Builds a [`SineManifest`]: the manifest every law declares (name,
/// version, author, summary, parameters, values) plus the parameters the
/// operator may change while the test runs.
#[derive(Debug, Clone)]
pub struct SineManifestBuilder {
    law: Manifest,
    live: Vec<String>,
}

impl SineManifestBuilder {
    /// A manifest named `name` at `version`.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        SineManifestBuilder {
            law: Manifest {
                name: name.into(),
                version: version.into(),
                author: String::new(),
                summary: String::new(),
                params: Vec::new(),
                diagnostics: Vec::new(),
            },
            live: Vec::new(),
        }
    }

    /// Who wrote the law.
    pub fn author(mut self, author: impl Into<String>) -> Self {
        self.law.author = author.into();
        self
    }

    /// One paragraph on what the law does.
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.law.summary = summary.into();
        self
    }

    /// Declare a parameter.
    pub fn param(mut self, spec: ParamSpec) -> Self {
        self.law.params.push(spec);
        self
    }

    /// Declare a value the law reports on each call (at most
    /// [`MAX_SINE_VALUES`]).
    pub fn value(
        mut self,
        key: impl Into<String>,
        label: impl Into<String>,
        unit: impl Into<String>,
        help: impl Into<String>,
    ) -> Self {
        self.law.diagnostics.push(DiagnosticSpec {
            key: key.into(),
            label: label.into(),
            unit: unit.into(),
            help: help.into(),
        });
        self
    }

    /// Mark the number parameter `key` live: the operator may change it while
    /// the test runs. Declare the parameter itself with [`Self::param`].
    pub fn live(mut self, key: impl Into<String>) -> Self {
        let key = key.into();
        if !self.live.contains(&key) {
            self.live.push(key);
        }
        self
    }

    /// The manifest.
    pub fn build(self) -> SineManifest {
        SineManifest {
            law: self.law,
            live: self.live,
        }
    }
}

/// The resource the interface exports: a [`SineSingleLaw`] behind a
/// `RefCell`, because the interface hands the law shared references.
#[doc(hidden)]
pub struct SineLawResource<L: SineSingleLaw> {
    law: RefCell<L>,
}

const REENTERED: &str = "the law was called while a call was in flight";

impl<L: SineSingleLaw> wit_law::GuestLaw for SineLawResource<L> {
    fn new(setup: SineSetup, params: Vec<Param>) -> Result<Self, String> {
        L::new(setup, Params(params)).map(|law| SineLawResource {
            law: RefCell::new(law),
        })
    }

    fn engage(&self, inputs: TickInputs) -> Result<TickOutputs, String> {
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .engage(&inputs)
    }

    fn tick(&self, inputs: TickInputs) -> Result<TickOutputs, String> {
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .tick(&inputs)
    }

    fn adjust(&self, p: Param) -> Result<(), String> {
        self.law
            .try_borrow_mut()
            .map_err(|_| REENTERED.to_string())?
            .adjust(&p)
    }
}

/// Run a native Sine law (one written against `hertzforge_sine::law`, the
/// controller's own seam) as this law file: the built-in law's file and the
/// test fixtures that start from it are built this way, so the native law and
/// the file are one implementation.
pub mod native {
    use super::{Hold, SineSetup, TickInputs, TickOutputs};
    use crate::DiagnosticValue;
    use hertzforge_sine::law as n;

    /// The native law's view of the setup, borrowing from `s`.
    pub fn setup(s: &SineSetup, call_budget_s: f64) -> n::SineLawSetup<'_> {
        n::SineLawSetup {
            control_channels: s.control_channels as usize,
            tick_s: s.tick_s,
            band_hz: (s.band_lo_hz, s.band_hi_hz),
            drive_ceiling: s.drive_ceiling,
            identification: n::Identification {
                frequency_hz: &s.identification.frequency_hz,
                gain: &s.identification.gain,
                coherence: &s.identification.coherence,
                trusted: &s.identification.trusted,
                target: &s.identification.target,
            },
            call_budget_s,
        }
    }

    /// The native law's view of one tick, borrowing from `i`.
    pub fn inputs(i: &TickInputs) -> n::TickInputs<'_> {
        n::TickInputs {
            tick_index: i.tick_index,
            dt_s: i.dt_s,
            frequency_hz: i.frequency_hz,
            sweep_rate_hz_per_s: i.sweep_rate_hz_per_s,
            level_db: i.level_db,
            target: i.target,
            measured: i.measured,
            channel_amplitude: &i.channel_amplitude,
            channel_phase_deg: &i.channel_phase_deg,
            drive: i.drive,
            prediction_db: i.prediction_db,
            hold: match i.hold {
                Hold::Free => n::Hold::Free,
                Hold::Notch => n::Hold::Notch,
                Hold::Probe => n::Hold::Probe,
            },
            notch_ceiling: i.notch_ceiling,
            settling: i.settling,
        }
    }

    /// The built-in Sine law's manifest (`servo_blend`), from its declaration
    /// in `hertzforge_sine::law::servo_blend`: the one the controller's
    /// catalog lists for the built-in.
    pub fn servo_blend_manifest() -> super::SineManifest {
        use crate::{ChoiceSpec, DiagnosticSpec, Manifest, NumberSpec, ParamKind, ParamSpec};
        use hertzforge_sine::law::servo_blend as sb;
        super::SineManifest {
            law: Manifest {
                name: sb::NAME.into(),
                version: sb::VERSION.into(),
                author: sb::AUTHOR.into(),
                summary: sb::SUMMARY.into(),
                params: sb::KNOBS
                    .iter()
                    .map(|k| ParamSpec {
                        key: k.key.into(),
                        label: k.label.into(),
                        help: k.help.into(),
                        kind: match k.kind {
                            sb::KnobKind::Number {
                                unit,
                                min,
                                max,
                                step,
                                default,
                            } => ParamKind::Number(NumberSpec {
                                unit: unit.into(),
                                min,
                                max,
                                step,
                                default,
                            }),
                            sb::KnobKind::Choice { options, default } => {
                                ParamKind::Choice(ChoiceSpec {
                                    options: options.iter().map(|o| o.to_string()).collect(),
                                    default: default.into(),
                                })
                            }
                        },
                    })
                    .collect(),
                diagnostics: sb::DIAGNOSTICS
                    .iter()
                    .map(|d| DiagnosticSpec {
                        key: d.key.into(),
                        label: d.label.into(),
                        unit: d.unit.into(),
                        help: d.help.into(),
                    })
                    .collect(),
            },
            live: sb::KNOBS
                .iter()
                .filter(|k| k.live)
                .map(|k| k.key.to_string())
                .collect(),
        }
    }

    /// The parameter values the interface hands a law, in the native records'
    /// form.
    pub fn settings(params: &crate::Params) -> Vec<(String, n::ParamSetting)> {
        use crate::ParamValue;
        params
            .0
            .iter()
            .filter_map(|p| {
                let v = match &p.value {
                    ParamValue::Number(x) => n::ParamSetting::Number(*x),
                    ParamValue::Integer(i) => n::ParamSetting::Integer(*i),
                    ParamValue::Toggle(b) => n::ParamSetting::Toggle(*b),
                    ParamValue::Choice(c) => n::ParamSetting::Choice(c.clone()),
                    _ => return None,
                };
                Some((p.key.clone(), v))
            })
            .collect()
    }

    /// The built-in Sine law, `servo_blend`, behind the law interface: the
    /// controller's own `ServoBlendLaw`.
    pub struct ServoBlendFile {
        law: n::servo_blend::ServoBlendLaw,
        values: n::LawValues,
    }

    impl ServoBlendFile {
        /// The keys of the values, by index.
        pub fn value_keys() -> Vec<&'static str> {
            n::servo_blend::DIAGNOSTICS.iter().map(|d| d.key).collect()
        }

        /// The law for `params` (every declared parameter, as the host hands
        /// them).
        pub fn build(params: &crate::Params) -> Result<Self, String> {
            let p = n::servo_blend::ServoBlendParams::from_settings(&settings(params))?;
            Ok(ServoBlendFile {
                law: n::servo_blend::ServoBlendLaw::new(&p).map_err(|f| f.to_string())?,
                values: n::LawValues::new(n::servo_blend::DIAGNOSTICS.len()),
            })
        }

        /// One call of the native law through the interface's types.
        pub fn call(&mut self, engage: bool, i: &TickInputs) -> Result<TickOutputs, String> {
            use n::SineControlLaw;
            self.values.clear();
            let ni = inputs(i);
            let r = if engage {
                self.law.engage(&ni, &mut self.values)
            } else {
                self.law.tick(&ni, &mut self.values)
            };
            outputs(r, &self.values, &Self::value_keys())
        }

        /// A live change through the interface's types.
        pub fn adjust_param(&mut self, p: &crate::Param) -> Result<(), String> {
            use n::SineControlLaw;
            match p.value {
                crate::ParamValue::Number(x) => {
                    self.law.adjust(&p.key, x).map_err(|f| f.to_string())
                }
                _ => Err(format!("`{}` takes a number", p.key)),
            }
        }
    }

    impl super::SineSingleLaw for ServoBlendFile {
        fn describe() -> super::SineManifest {
            servo_blend_manifest()
        }

        fn new(_setup: SineSetup, params: crate::Params) -> Result<Self, String> {
            ServoBlendFile::build(&params)
        }

        fn engage(&mut self, i: &TickInputs) -> Result<TickOutputs, String> {
            self.call(true, i)
        }

        fn tick(&mut self, i: &TickInputs) -> Result<TickOutputs, String> {
            self.call(false, i)
        }

        fn adjust(&mut self, p: &crate::Param) -> Result<(), String> {
            self.adjust_param(p)
        }
    }

    /// The interface's result from a native call: the drive, the fault flag,
    /// and every value the law set, under its declared key.
    pub fn outputs(
        r: Result<n::TickOutputs, n::LawFault>,
        values: &n::LawValues,
        keys: &[&str],
    ) -> Result<TickOutputs, String> {
        let o = r.map_err(|f| f.to_string())?;
        Ok(TickOutputs {
            drive: o.drive,
            faulted: o.faulted,
            values: values
                .iter()
                .filter_map(|(i, v)| {
                    keys.get(i).map(|k| DiagnosticValue {
                        key: (*k).to_string(),
                        value: v,
                    })
                })
                .collect(),
        })
    }
}
