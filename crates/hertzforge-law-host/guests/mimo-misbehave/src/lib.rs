//! A multiple-output law that misbehaves on purpose, for the host's tests.
//! The `mode` parameter picks what `iterate` does; `seed` returns the good
//! drive in every mode but `spin_seed`.
//!
//! The fixture implements the interface's raw bindings rather than the SDK's
//! `RandomMimoLaw` trait: the SDK's types cannot express some of these
//! mistakes (a Hermitian stack keeps its diagonal real), and a law written in
//! another language, or against the bindings, can return anything the
//! interface carries. That is what the host has to be proof against.
//!
//! - `ok`: the good drive, 1e-3 V^2/Hz on each drive in band, uncorrelated,
//!   zero out of band.
//! - `nan`: the good drive with a NaN on the first in-band line.
//! - `non_hermitian`: the good drive with an imaginary part on a diagonal
//!   value (in packed form the only way to be non-Hermitian).
//! - `wrong_dims`: a drive one drive too wide.
//! - `out_of_band`: the good drive with a value on the first line out of
//!   band.
//! - `indefinite`: drives 0 and 1 given a cross term twice their PSDs, so
//!   each in-band line has an eigenvalue of -1e-3 against a largest of 3e-3:
//!   well formed, Hermitian, and not positive semi-definite.
//! - `spin`: loop forever.
//! - `spin_seed`: `seed` loops forever (and `iterate` returns the good drive).
//! - `trap`: panic (a trap, with a message on stderr).
//! - `law_error`: return an error.
//! - `undeclared`: report a diagnostic the manifest does not declare.
//! - `wrong_layout`: report the per-line diagnostic as a scalar.
//!
//! Every result reports which optional inputs arrived: the fixture declares it
//! needs the last drive and nothing else.

use hertzforge_law_sdk::__mimo_bindings::exports::hertzforge::control_law::random_mimo_law::{
    Guest as LawGuest, GuestLaw,
};
use hertzforge_law_sdk::__mimo_bindings::Guest;
use hertzforge_law_sdk::mimo::{
    hermitian_to_wit, setup_from_wit, wit, DiagnosticKind, HermitianStack, MimoInput, MimoManifest,
    MimoManifestBuilder, MimoShape, C64,
};
use hertzforge_law_sdk::{ChoiceSpec, Param, ParamKind, ParamSpec, ParamValue};
use std::hint::black_box;

const MODES: &[&str] = &[
    "ok",
    "nan",
    "non_hermitian",
    "wrong_dims",
    "out_of_band",
    "indefinite",
    "spin",
    "spin_seed",
    "trap",
    "law_error",
    "undeclared",
    "wrong_layout",
];

/// The fixture's state: fixed at construction.
pub struct MimoMisbehave {
    mode: String,
    lines: usize,
    drives: usize,
    band: (usize, usize),
}

impl MimoMisbehave {
    fn good(&self, drives: usize) -> wit::HermitianStack {
        let mut g = HermitianStack::zeros(self.lines, drives);
        for line in self.band.0..self.band.1 {
            for d in 0..drives {
                g.set(line, d, d, C64::new(1e-3, 0.0));
            }
        }
        hermitian_to_wit(&g)
    }

    /// The packed offset of `(r, c)`, `r <= c`, on `line`.
    fn at(&self, line: usize, r: usize, c: usize) -> usize {
        let n = self.drives;
        line * HermitianStack::packed_len(n) + HermitianStack::packed_index(n, r, c)
    }

    fn out(&self, drive: wit::HermitianStack, i: &wit::MimoInputs) -> wit::MimoOutputs {
        let flag = |present: bool| if present { 1.0 } else { 0.0 };
        let scalar = |key: &str, v: f64| wit::MimoDiagnostic {
            key: key.into(),
            data: wit::DiagnosticData::Scalar(v),
        };
        wit::MimoOutputs {
            drive,
            diagnostics: vec![
                scalar("has_last_drive", flag(i.last_drive.is_some())),
                scalar("has_sysid_noise", flag(i.sysid_noise.is_some())),
                scalar("has_sysid_response", flag(i.sysid_response.is_some())),
                wit::MimoDiagnostic {
                    key: "line_mark".into(),
                    data: wit::DiagnosticData::PerLine(vec![0.0; self.lines]),
                },
            ],
        }
    }
}

impl GuestLaw for MimoMisbehave {
    fn new(setup: wit::MimoSetup, params: Vec<Param>) -> Result<Self, String> {
        let setup = setup_from_wit(setup)?;
        let mode = match params.iter().find(|p| p.key == "mode").map(|p| &p.value) {
            Some(ParamValue::Choice(m)) => m.clone(),
            _ => return Err("parameter `mode` is missing".into()),
        };
        Ok(MimoMisbehave {
            mode,
            lines: setup.lines,
            drives: setup.drives,
            band: setup.band,
        })
    }

    fn seed(&self, inputs: wit::MimoInputs) -> Result<wit::MimoOutputs, String> {
        if self.mode == "spin_seed" {
            let mut x = 0u64;
            loop {
                x = black_box(x.wrapping_add(1));
            }
        }
        Ok(self.out(self.good(self.drives), &inputs))
    }

    fn iterate(&self, inputs: wit::MimoInputs) -> Result<wit::MimoOutputs, String> {
        let lo = self.band.0;
        let mut good = self.good(self.drives);
        match self.mode.as_str() {
            "nan" => {
                good.data[self.at(lo, 0, 0)].re = f64::NAN;
                Ok(self.out(good, &inputs))
            }
            "non_hermitian" => {
                good.data[self.at(lo, 0, 0)].im = 5e-4;
                Ok(self.out(good, &inputs))
            }
            "wrong_dims" => Ok(self.out(self.good(self.drives + 1), &inputs)),
            "out_of_band" => {
                let line = if lo > 0 { 0 } else { self.band.1 };
                good.data[self.at(line, 0, 0)].re = 1e-3;
                Ok(self.out(good, &inputs))
            }
            "indefinite" => {
                for line in self.band.0..self.band.1 {
                    good.data[self.at(line, 0, 1)].re = 2e-3;
                }
                Ok(self.out(good, &inputs))
            }
            "spin" => {
                let mut x = 0u64;
                loop {
                    x = black_box(x.wrapping_add(1));
                }
            }
            "trap" => panic!("deliberate trap from the multiple-output fixture"),
            "law_error" => Err("deliberate error from the multiple-output fixture".into()),
            "undeclared" => {
                let mut out = self.out(good, &inputs);
                out.diagnostics.push(wit::MimoDiagnostic {
                    key: "not_declared".into(),
                    data: wit::DiagnosticData::Scalar(1.0),
                });
                Ok(out)
            }
            "wrong_layout" => {
                let mut out = self.out(good, &inputs);
                for d in &mut out.diagnostics {
                    if d.key == "line_mark" {
                        d.data = wit::DiagnosticData::Scalar(0.0);
                    }
                }
                Ok(out)
            }
            _ => Ok(self.out(good, &inputs)),
        }
    }
}

/// The export.
pub struct Export;

impl Guest for Export {
    fn describe() -> MimoManifest {
        MimoManifestBuilder::new("misbehaving multiple-output fixture", "0.0.0")
            .summary("A test fixture that misbehaves in the way its mode names.")
            .param(ParamSpec {
                key: "mode".into(),
                label: "Mode".into(),
                help: "What iterate does.".into(),
                kind: ParamKind::Choice(ChoiceSpec {
                    options: MODES.iter().map(|m| m.to_string()).collect(),
                    default: "ok".into(),
                }),
            })
            .shapes(&[
                MimoShape::Square,
                MimoShape::MoreControlsThanDrives,
                MimoShape::FewerControlsThanDrives,
            ])
            .needs(&[MimoInput::LastDrive])
            .diagnostic(
                "has_last_drive",
                "Last drive given",
                "",
                "",
                DiagnosticKind::Scalar,
            )
            .diagnostic(
                "has_sysid_noise",
                "Noise given",
                "",
                "",
                DiagnosticKind::Scalar,
            )
            .diagnostic(
                "has_sysid_response",
                "Identification response given",
                "",
                "",
                DiagnosticKind::Scalar,
            )
            .diagnostic("line_mark", "Line mark", "", "", DiagnosticKind::PerLine)
            .build()
    }
}

impl LawGuest for Export {
    type Law = MimoMisbehave;
}

hertzforge_law_sdk::__mimo_bindings::__export_random_mimo!(
    Export with_types_in hertzforge_law_sdk::__mimo_bindings
);
