//! What the law picker shows for a MIMO Shock law: name, version, where it
//! comes from, its parameters, the target kinds it accepts, the values it
//! reports, and where it stands on the check, including the check cases a
//! built-in law is known to fail.

use crate::check::MimoShockCheckCase;
use crate::BuiltinMimoShock;
use hertzforge_law_host::{
    ChoiceSpec, DiagnosticSpec, IntegerSpec, LawIdentity, LoadedShockMimoLaw, Manifest, NumberSpec,
    ParamKind, ParamSpec,
};
use hertzforge_law_sdk::shock_mimo::builtin::{self, Knob, LawDecl};
use hertzforge_law_sdk::shock_mimo::TargetKind as SdkKind;
use hertzforge_mimo_shock::TargetKind;
use hertzforge_random_laws::{CatalogEntry, CheckStatus, LawCatalog, LawWorld};

/// A check case a built-in law is expected to fail, and the evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct KnownFailure {
    /// The case.
    pub case: MimoShockCheckCase,
    /// The target kind.
    pub kind: TargetKind,
    /// Why it fails or is expected to, in words.
    pub why: String,
}

/// What the law picker shows for one law.
#[derive(Debug, Clone, PartialEq)]
pub struct LawFacts {
    /// What a choice names the law by: a built-in id or a file's SHA-256.
    pub id: String,
    /// Name, version, author and file hash (empty for a built-in).
    pub identity: LawIdentity,
    /// The world: always `shock_mimo`.
    pub world: LawWorld,
    /// Whether the law is built into the program.
    pub builtin: bool,
    /// One paragraph on what it does.
    pub summary: String,
    /// The parameters the operator may set.
    pub params: Vec<ParamSpec>,
    /// The target kinds it accepts.
    pub targets: Vec<TargetKind>,
    /// The values it reports.
    pub values: Vec<String>,
    /// Where it stands on its check.
    pub check: CheckStatus,
    /// Check cases it is known to fail, with the evidence.
    pub known_failures: Vec<KnownFailure>,
}

fn core_kind(k: SdkKind) -> TargetKind {
    match k {
        SdkKind::Waveform => TargetKind::Waveform,
        SdkKind::Srs => TargetKind::Srs,
    }
}

/// A built-in declaration as the host's manifest.
pub fn decl_manifest(d: &LawDecl) -> Manifest {
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

/// The declaration of a built-in law.
pub fn decl(b: BuiltinMimoShock) -> &'static LawDecl {
    match b {
        BuiltinMimoShock::Inverse => &builtin::INVERSE,
        BuiltinMimoShock::Shape => &builtin::SHAPE,
        BuiltinMimoShock::Paired => &builtin::PAIRED,
    }
}

const NO_LIMITS: &str =
    "a MIMO Shock law is told nothing of the limit channels, so it cannot hold \
                         one under its line; the controller stops the run when one would cross it";

/// The check cases a built-in law fails at its defaults, as the check ran them
/// on the simulator. The cases are written for a good law; these are where
/// today's built-ins fall short of one.
pub fn known_failures(b: BuiltinMimoShock) -> Vec<KnownFailure> {
    use MimoShockCheckCase::*;
    use TargetKind::{Srs, Waveform};
    let mut out = Vec::new();
    let mut add = |case: MimoShockCheckCase, kinds: &[TargetKind], why: &str| {
        for &kind in kinds {
            out.push(KnownFailure {
                case,
                kind,
                why: why.into(),
            });
        }
    };
    match b {
        BuiltinMimoShock::Inverse => {
            add(
                Jointed6x3,
                &[Waveform, Srs],
                "with more controls than drives the targets are out of reach, so no pulse passes \
                 and the run stays at the first level",
            );
            add(
                DelayError,
                &[Waveform],
                "with every path's delay 0.5 ms wrong in its model it does not pass the -9 dB level",
            );
            add(
                DelayError,
                &[Srs],
                "with every path's delay wrong its first correction is refused by the drive growth \
                 cap",
            );
            add(
                MeasurementNoise,
                &[Srs],
                "it corrects the noise too, and its first correction is refused by the band growth \
                 cap",
            );
            add(LimitChannel, &[Waveform, Srs], NO_LIMITS);
        }
        BuiltinMimoShock::Shape => {
            add(
                Jointed6x3,
                &[Srs],
                "with more controls than drives its first correction is refused by the band growth \
                 cap",
            );
            add(
                DelayError,
                &[Srs],
                "with every path's delay wrong it does not pass the first level",
            );
            add(
                MeasurementNoise,
                &[Srs],
                "under measurement noise it does not pass the first level",
            );
            add(LimitChannel, &[Srs], NO_LIMITS);
        }
        // No cross-path model: the pairs fight on any coupled structure.
        BuiltinMimoShock::Paired => {
            for case in MimoShockCheckCase::ALL {
                add(
                    case,
                    &[Waveform, Srs],
                    "uses no cross path of the plant, and every check plant is coupled",
                );
            }
        }
    }
    out
}

/// The catalog entry of a built-in law.
pub fn builtin_entry(b: BuiltinMimoShock) -> CatalogEntry {
    let manifest = decl_manifest(decl(b));
    CatalogEntry {
        id: b.id().into(),
        identity: LawIdentity {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            author: manifest.author.clone(),
            sha256: String::new(),
        },
        manifest,
        builtin: true,
        check: CheckStatus::BuiltIn,
        world: LawWorld::ShockMimo,
        mimo: None,
        live: Vec::new(),
    }
}

/// Every MIMO Shock law in `catalog`: the built-ins first (`inverse`, the
/// default for a waveform target, then `shape`, the default for an SRS
/// target, then `paired`), then the imported law files.
pub fn entries(catalog: &LawCatalog) -> Vec<CatalogEntry> {
    BuiltinMimoShock::ALL
        .into_iter()
        .map(builtin_entry)
        .chain(catalog.shock_mimo_entries())
        .collect()
}

/// The facts of a built-in law.
pub fn builtin_facts(b: BuiltinMimoShock) -> LawFacts {
    let e = builtin_entry(b);
    let d = decl(b);
    LawFacts {
        id: e.id,
        summary: e.manifest.summary.clone(),
        params: e.manifest.params.clone(),
        values: e
            .manifest
            .diagnostics
            .iter()
            .map(|d| d.key.clone())
            .collect(),
        identity: e.identity,
        world: LawWorld::ShockMimo,
        builtin: true,
        targets: d.targets.iter().map(|k| core_kind(*k)).collect(),
        check: CheckStatus::BuiltIn,
        known_failures: known_failures(b),
    }
}

/// The facts of a law file, with its check status.
pub fn file_facts(law: &LoadedShockMimoLaw, check: CheckStatus) -> LawFacts {
    let m = law.manifest();
    let failures = match &check {
        CheckStatus::Checked(r) => r
            .cases
            .iter()
            .filter(|c| !c.passed)
            .filter_map(|c| {
                let (name, kind) = c.name.split_once(" (")?;
                let kind = match kind.trim_end_matches(')') {
                    "waveform" => TargetKind::Waveform,
                    "srs" => TargetKind::Srs,
                    _ => return None,
                };
                Some(KnownFailure {
                    case: MimoShockCheckCase::from_name(name)?,
                    kind,
                    why: c.detail.clone(),
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    LawFacts {
        id: law.identity().sha256.clone(),
        identity: law.identity().clone(),
        world: LawWorld::ShockMimo,
        builtin: false,
        summary: m.law.summary.clone(),
        params: m.law.params.clone(),
        targets: m
            .targets
            .iter()
            .map(|k| crate::wasm::core_kind(*k))
            .collect(),
        values: m.law.diagnostics.iter().map(|d| d.key.clone()).collect(),
        check,
        known_failures: failures,
    }
}

/// The facts of every MIMO Shock law in `catalog`, built-ins first.
pub fn all_facts(catalog: &LawCatalog) -> Vec<LawFacts> {
    let mut out: Vec<LawFacts> = BuiltinMimoShock::ALL
        .into_iter()
        .map(builtin_facts)
        .collect();
    for e in catalog.shock_mimo_entries() {
        if let Some(l) = catalog.shock_mimo_law(&e.id) {
            out.push(file_facts(l, e.check.clone()));
        }
    }
    out
}
