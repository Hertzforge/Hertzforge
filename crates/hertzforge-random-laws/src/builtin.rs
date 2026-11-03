//! The laws built into the program, described by the same [`Manifest`] type a
//! law file declares, so a form or a list treats every law alike.
//!
//! Each manifest is built from the one declaration next to the law's math:
//! [`HinvConfig::KNOBS`] for `hinv` and `hertzforge_law_ratio_math::PARAMS` for
//! `ratio`. The law files `laws/hinv` and `laws/ratio` declare their manifests
//! from the same place, and a test holds each built-in manifest equal to its
//! law file's.

use hertzforge_law_host::{
    DiagnosticSpec, IntegerSpec, LawIdentity, Manifest, NumberSpec, ParamKind, ParamSpec,
};
use hertzforge_law_ratio_math as ratio;
use hertzforge_random::control::{HinvConfig, HinvLaw, KnobKind};

/// The id of the default law.
pub const DEFAULT_LAW_ID: &str = "hinv";

/// A law built into the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinLaw {
    /// H-inverse with a level servo and a per-line shape trim: the default.
    Hinv,
    /// Per-line ratio correction.
    Ratio,
}

impl BuiltinLaw {
    /// Every built-in law, the default first.
    pub const ALL: [BuiltinLaw; 2] = [BuiltinLaw::Hinv, BuiltinLaw::Ratio];

    /// The stable id a choice of law names it by.
    pub fn id(self) -> &'static str {
        match self {
            BuiltinLaw::Hinv => DEFAULT_LAW_ID,
            BuiltinLaw::Ratio => "ratio",
        }
    }

    /// The built-in law with `id`, if any.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.id() == id)
    }

    /// The law's manifest.
    pub fn manifest(self) -> Manifest {
        match self {
            BuiltinLaw::Hinv => hinv_manifest(),
            BuiltinLaw::Ratio => ratio_manifest(),
        }
    }

    /// The law's identity: its manifest's name, version and author. The hash is
    /// empty: a built-in law is part of the program, and the program's own
    /// build identifies it.
    pub fn identity(self) -> LawIdentity {
        let m = self.manifest();
        LawIdentity {
            name: m.name,
            version: m.version,
            author: m.author,
            sha256: String::new(),
        }
    }
}

fn hinv_manifest() -> Manifest {
    let defaults = HinvConfig::default();
    Manifest {
        name: HinvLaw::TITLE.into(),
        version: HinvLaw::VERSION.into(),
        author: HinvLaw::AUTHOR.into(),
        summary: HinvLaw::SUMMARY.into(),
        params: HinvConfig::KNOBS
            .iter()
            .map(|k| {
                let default = defaults.knob(k.key).unwrap_or(k.min);
                ParamSpec {
                    key: k.key.into(),
                    label: k.label.into(),
                    help: k.help.into(),
                    kind: match k.kind {
                        KnobKind::Number => ParamKind::Number(NumberSpec {
                            unit: k.unit.into(),
                            min: k.min,
                            max: k.max,
                            step: k.step,
                            default,
                        }),
                        // Whole-number knobs are declared with whole bounds.
                        KnobKind::Integer => ParamKind::Integer(IntegerSpec {
                            unit: k.unit.into(),
                            min: k.min as i64,
                            max: k.max as i64,
                            step: k.step as i64,
                            default: default as i64,
                        }),
                    },
                }
            })
            .collect(),
        diagnostics: Vec::new(),
    }
}

fn ratio_manifest() -> Manifest {
    Manifest {
        name: ratio::NAME.into(),
        version: ratio::VERSION.into(),
        author: ratio::AUTHOR.into(),
        summary: ratio::SUMMARY.into(),
        params: ratio::PARAMS
            .iter()
            .map(|p| ParamSpec {
                key: p.key.into(),
                label: p.label.into(),
                help: p.help.into(),
                kind: ParamKind::Number(NumberSpec {
                    unit: p.unit.into(),
                    min: p.min,
                    max: p.max,
                    step: p.step,
                    default: p.default,
                }),
            })
            .collect(),
        diagnostics: ratio::DIAGNOSTICS
            .iter()
            .map(|d| DiagnosticSpec {
                key: d.key.into(),
                label: d.label.into(),
                unit: d.unit.into(),
                help: d.help.into(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_law_host::validate::validate_manifest;

    #[test]
    fn every_built_in_manifest_passes_the_rules_an_imported_one_must() {
        for b in BuiltinLaw::ALL {
            validate_manifest(&b.manifest()).unwrap_or_else(|e| panic!("{b:?}: {e}"));
            assert_eq!(BuiltinLaw::from_id(b.id()), Some(b));
        }
        assert_eq!(BuiltinLaw::ALL[0].id(), DEFAULT_LAW_ID);
    }

    #[test]
    fn the_hinv_manifest_declares_its_knobs_with_todays_defaults() {
        let m = BuiltinLaw::Hinv.manifest();
        let d = HinvConfig::default();
        assert_eq!(m.params.len(), HinvConfig::KNOBS.len());
        for p in &m.params {
            let want = d.knob(&p.key).expect("every parameter is a knob");
            match &p.kind {
                ParamKind::Number(s) => assert_eq!(s.default, want, "{}", p.key),
                ParamKind::Integer(s) => assert_eq!(s.default as f64, want, "{}", p.key),
                other => panic!("{}: {other:?}", p.key),
            }
        }
    }
}
