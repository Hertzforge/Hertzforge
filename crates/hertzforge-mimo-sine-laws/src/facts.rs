//! The catalog entries of the MIMO Sine world and what the law picker shows
//! for each law: name, version, where it comes from, its parameters, the
//! shapes and test types it accepts, whether it takes an amplitude-only
//! reference, the values it reports, where it stands on the check, and for a
//! built-in law its limits as the research testbed measured them.
//!
//! The shared catalog entry has no field for the shapes, the test types or
//! the amplitude-only flag, so [`SineMimoEntry`] carries them beside it; a
//! `laws.list` answer for this world emits them (`test_types`,
//! `amplitude_only`) from here.

use crate::native::core_test_type;
use hertzforge_law_host::{
    ChoiceSpec, DiagnosticSpec, IntegerSpec, LawIdentity, LoadedSineMimoLaw, Manifest, NumberSpec,
    ParamKind, ParamSpec,
};
use hertzforge_law_sdk::sine_mimo::builtin::{self, Knob, LawDecl};
use hertzforge_mimo_sine::{shape_name, MimoShape, TestType};
use hertzforge_random_laws::{CatalogEntry, CheckStatus, LawCatalog, LawWorld};

/// A built-in declaration as the host's manifest.
pub fn host_manifest(d: &LawDecl) -> Manifest {
    Manifest {
        name: d.name.into(),
        version: d.version.into(),
        author: builtin::AUTHOR.into(),
        summary: d.summary.into(),
        params: d
            .params
            .iter()
            .map(|p| ParamSpec {
                key: p.key.into(),
                label: p.label.into(),
                help: p.help.into(),
                kind: match p.knob {
                    Knob::Number {
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
                    Knob::Integer {
                        unit,
                        min,
                        max,
                        default,
                    } => ParamKind::Integer(IntegerSpec {
                        unit: unit.into(),
                        min,
                        max,
                        step: 1,
                        default,
                    }),
                    Knob::Toggle(d) => ParamKind::Toggle(d),
                    Knob::Choice { options, default } => ParamKind::Choice(ChoiceSpec {
                        options: options.iter().map(|o| (*o).to_string()).collect(),
                        default: default.into(),
                    }),
                },
            })
            .collect(),
        diagnostics: d
            .values
            .iter()
            .map(|v| DiagnosticSpec {
                key: v.key.into(),
                label: v.label.into(),
                unit: v.unit.into(),
                help: v.help.into(),
            })
            .collect(),
    }
}

/// One MIMO Sine law in the catalog: the shared entry, and what this world
/// adds to it.
#[derive(Debug, Clone, PartialEq)]
pub struct SineMimoEntry {
    /// The shared catalog entry (id, identity, manifest, check, world).
    pub entry: CatalogEntry,
    /// The shapes the law accepts.
    pub shapes: Vec<MimoShape>,
    /// The test types the law accepts.
    pub test_types: Vec<TestType>,
    /// Whether the law accepts an amplitude-only reference.
    pub amplitude_only: bool,
}

impl SineMimoEntry {
    /// The shapes' wire names.
    pub fn shape_names(&self) -> Vec<&'static str> {
        self.shapes.iter().map(|s| shape_name(*s)).collect()
    }

    /// The test types' wire names (`sweep`, `dwell`, `stepped`).
    pub fn test_type_names(&self) -> Vec<&'static str> {
        self.test_types.iter().map(|t| t.as_str()).collect()
    }
}

/// The catalog entry of a built-in law.
pub fn builtin_entry(d: &LawDecl) -> SineMimoEntry {
    let manifest = host_manifest(d);
    SineMimoEntry {
        entry: CatalogEntry {
            id: d.id.into(),
            identity: LawIdentity {
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                author: manifest.author.clone(),
                sha256: String::new(),
            },
            manifest,
            builtin: true,
            check: CheckStatus::BuiltIn,
            world: LawWorld::SineMimo,
            mimo: None,
            live: Vec::new(),
        },
        shapes: d
            .shapes
            .iter()
            .map(|s| crate::native::core_shape(*s))
            .collect(),
        test_types: d.test_types().into_iter().map(core_test_type).collect(),
        amplitude_only: d.amplitude_only,
    }
}

fn file_entry(e: CatalogEntry, law: &LoadedSineMimoLaw) -> SineMimoEntry {
    let m = law.manifest();
    SineMimoEntry {
        entry: e,
        shapes: m
            .shapes
            .iter()
            .map(|s| crate::wasm::core_shape(*s))
            .collect(),
        test_types: m
            .test_types
            .iter()
            .map(|t| crate::wasm::core_test_type(*t))
            .collect(),
        amplitude_only: m.amplitude_only,
    }
}

/// Every MIMO Sine law in `catalog`: the built-ins first (`matrix`, the
/// default, then `paired`, `steepest_descent`, `secant`, `ratio`), then the
/// imported law files.
pub fn entries(catalog: &LawCatalog) -> Vec<SineMimoEntry> {
    let mut out: Vec<SineMimoEntry> = builtin::ALL.iter().map(builtin_entry).collect();
    for e in catalog.sine_mimo_entries() {
        if let Some(l) = catalog.sine_mimo_law(&e.id) {
            out.push(file_entry(e, l));
        }
    }
    out
}

/// The limits of a built-in law, stated plainly from the research record
/// (the research testbed's simulated structures; no law has run on a
/// shaker). `None` for an id that is not a built-in law.
pub fn limits(id: &str) -> Option<&'static str> {
    Some(match id {
        "matrix" => {
            "This law assumes the test article responds at test level as it did during the \
             low-level identification. Where a resonance has moved by less than a quarter of its \
             half-power bandwidth, expect the same control as on a linear article. Between a \
             quarter and one bandwidth, expect errors over 3 dB at that resonance. Past about \
             one bandwidth the loop can be lost and the test will abort. A Q of 50 puts one \
             bandwidth at 2 percent of frequency."
        }
        "paired" => {
            "No model of the coupling between pairs. On the research testbed's coupled linear \
             four-by-four structure it completed none of 5 dwells and none of 5 stepped sines. \
             It is for structures whose pairs are weakly coupled, and is the template to start \
             a law from."
        }
        "steepest_descent" => {
            "On the research testbed's linear four-by-four structure it completed 5 of 5 dwells \
             at the lightest resonance, 2 of 5 stepped sines (12 steps an octave, 30 to 120 Hz) \
             and none of 20 sweeps at one octave a minute. It accepts dwells only."
        }
        "secant" => {
            "On the research testbed's linear four-by-four structure it completed 4 of 5 dwells \
             at the lightest resonance with either carry, none of 5 stepped sines and none of 20 \
             sweeps at one octave a minute: each cycle holds the drive for several settle waits \
             while the tone moves on. It accepts dwells only."
        }
        "ratio" => {
            "On the research testbed's linear four-by-four structure, with reference pairing it \
             completed 5 of 5 dwells, 5 of 5 stepped sines and 3 of 20 sweeps at one octave a \
             minute; with drive pairing, 1 of 5 dwells and none of 5 stepped sines. It accepts \
             dwells and stepped sines."
        }
        _ => return None,
    })
}

/// What the law picker shows for one law.
#[derive(Debug, Clone, PartialEq)]
pub struct LawFacts {
    /// What a choice names the law by: a built-in id or a file's SHA-256.
    pub id: String,
    /// Name, version, author and file hash (empty for a built-in).
    pub identity: LawIdentity,
    /// The world: always `sine_mimo`.
    pub world: LawWorld,
    /// Whether the law is built into the program.
    pub builtin: bool,
    /// One paragraph on what it does.
    pub summary: String,
    /// The parameters the operator may set.
    pub params: Vec<ParamSpec>,
    /// The shapes it accepts.
    pub shapes: Vec<MimoShape>,
    /// The test types it accepts.
    pub test_types: Vec<TestType>,
    /// Whether it accepts an amplitude-only reference.
    pub amplitude_only: bool,
    /// The values it reports.
    pub values: Vec<String>,
    /// Where it stands on its check.
    pub check: CheckStatus,
    /// Its limits, as measured, for a built-in law.
    pub limits: Option<&'static str>,
}

fn facts_of(e: SineMimoEntry, limits: Option<&'static str>) -> LawFacts {
    LawFacts {
        id: e.entry.id,
        summary: e.entry.manifest.summary.clone(),
        params: e.entry.manifest.params.clone(),
        values: e
            .entry
            .manifest
            .diagnostics
            .iter()
            .map(|d| d.key.clone())
            .collect(),
        identity: e.entry.identity,
        world: LawWorld::SineMimo,
        builtin: e.entry.builtin,
        shapes: e.shapes,
        test_types: e.test_types,
        amplitude_only: e.amplitude_only,
        check: e.entry.check,
        limits,
    }
}

/// The facts of a built-in law.
pub fn builtin_facts(d: &LawDecl) -> LawFacts {
    facts_of(builtin_entry(d), limits(d.id))
}

/// The facts of a law file, with its check status.
pub fn file_facts(law: &LoadedSineMimoLaw, check: CheckStatus) -> LawFacts {
    let m = law.manifest();
    LawFacts {
        id: law.identity().sha256.clone(),
        identity: law.identity().clone(),
        world: LawWorld::SineMimo,
        builtin: false,
        summary: m.law.summary.clone(),
        params: m.law.params.clone(),
        shapes: m
            .shapes
            .iter()
            .map(|s| crate::wasm::core_shape(*s))
            .collect(),
        test_types: m
            .test_types
            .iter()
            .map(|t| crate::wasm::core_test_type(*t))
            .collect(),
        amplitude_only: m.amplitude_only,
        values: m.law.diagnostics.iter().map(|d| d.key.clone()).collect(),
        check,
        limits: None,
    }
}

/// The facts of every MIMO Sine law in `catalog`, built-ins first.
pub fn all_facts(catalog: &LawCatalog) -> Vec<LawFacts> {
    entries(catalog)
        .into_iter()
        .map(|e| {
            let l = if e.entry.builtin {
                limits(&e.entry.id)
            } else {
                None
            };
            facts_of(e, l)
        })
        .collect()
}
