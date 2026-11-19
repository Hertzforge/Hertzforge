//! A TWR law that misbehaves on purpose, for the host's and the core's tests.
//! Its seed, and every call in `ok` mode, is the built-in frequency-domain
//! correction (`twr_correct`); the `mode` parameter picks what goes wrong:
//!
//! - `ok`: the built-in law.
//! - `trap`: `iterate` panics (a trap, with a message on stderr).
//! - `spin`: `iterate` loops forever.
//! - `grow`: `iterate` allocates until memory runs out.
//! - `sleep`: `iterate` sleeps 30 seconds, then answers.
//! - `sleep_after`: the first correction answers; every later one runs past
//!   any budget (a busy loop: under the host a sleep returns at once).
//! - `open_file`: `iterate` tries to read, list and write files and open a
//!   network connection, and returns an error if every attempt fails.
//! - `nan`, `wrong_length`, `undeclared`: `iterate` returns a drive with a NaN,
//!   a drive one sample short, or a value the manifest does not declare.
//! - `law_error`: `iterate` returns an error.
//! - `print`: `iterate` writes 64 KiB to each of stdout and stderr, then
//!   answers.

use hertzforge_law_sdk::twr::*;
use hertzforge_law_sdk::{
    export_twr_law, ChoiceSpec, DiagnosticSpec, Manifest, ParamKind, ParamSpec, Params,
    TwrSingleLaw,
};
use hertzforge_shock_law_math::classical::Step;
use hertzforge_shock_law_math::twr::{TwrCorrect, TwrCorrectParams, DIAGNOSTICS};
use std::hint::black_box;
use std::io::Write;
use std::time::Duration;

const MODES: &[&str] = &[
    "ok",
    "trap",
    "spin",
    "grow",
    "sleep",
    "sleep_after",
    "open_file",
    "nan",
    "wrong_length",
    "undeclared",
    "law_error",
    "print",
];

/// The fixture's state.
pub struct TwrMisbehave {
    mode: String,
    math: TwrCorrect,
    calls: u32,
}

fn out(step: Step) -> PlayOutputs {
    outputs(step.drive, &step.report.values())
}

fn escape() -> Result<(), String> {
    let attempts: [(&str, std::io::Result<()>); 4] = [
        ("read /etc/hosts", std::fs::read("/etc/hosts").map(drop)),
        ("list .", std::fs::read_dir(".").map(drop)),
        ("write escape.txt", std::fs::write("escape.txt", b"x")),
        (
            "connect 127.0.0.1:9",
            std::net::TcpStream::connect("127.0.0.1:9").map(drop),
        ),
    ];
    if attempts.iter().all(|(_, r)| r.is_err()) {
        let why: Vec<String> = attempts
            .iter()
            .map(|(what, r)| match r {
                Ok(()) => format!("{what}: ok"),
                Err(e) => format!("{what}: {e}"),
            })
            .collect();
        Err(format!("every escape failed ({})", why.join("; ")))
    } else {
        Ok(())
    }
}

impl TwrSingleLaw for TwrMisbehave {
    fn describe() -> Manifest {
        Manifest {
            name: "misbehaving TWR fixture".into(),
            version: "0.0.0".into(),
            author: String::new(),
            summary: "A test fixture that misbehaves in the way its mode names.".into(),
            params: vec![ParamSpec {
                key: "mode".into(),
                label: "Mode".into(),
                help: "What goes wrong.".into(),
                kind: ParamKind::Choice(ChoiceSpec {
                    options: MODES.iter().map(|m| m.to_string()).collect(),
                    default: "ok".into(),
                }),
            }],
            diagnostics: DIAGNOSTICS
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

    fn new(setup: TwrSetup, params: Params) -> Result<Self, String> {
        let math = TwrCorrect::new(
            TwrCorrectParams::default(),
            setup.record_len as usize,
            &setup.reference,
            &inverse(&setup.plant),
            &setup.plant.trusted,
        )?;
        Ok(TwrMisbehave {
            mode: params.choice("mode")?.to_string(),
            math,
            calls: 0,
        })
    }

    fn seed(&mut self, _inputs: &PlayInputs) -> Result<PlayOutputs, String> {
        Ok(out(self.math.seed()?))
    }

    fn iterate(&mut self, inputs: &PlayInputs) -> Result<PlayOutputs, String> {
        self.calls += 1;
        match self.mode.as_str() {
            "trap" => panic!("deliberate trap from the misbehaving TWR fixture"),
            "spin" => {
                let mut x = 0u64;
                loop {
                    x = black_box(x.wrapping_add(1));
                }
            }
            "grow" => {
                let mut hoard: Vec<Vec<u8>> = Vec::new();
                loop {
                    hoard.push(vec![1u8; 8 << 20]);
                    black_box(&hoard);
                }
            }
            "sleep" => std::thread::sleep(Duration::from_secs(30)),
            "sleep_after" if self.calls > 1 => {
                let mut x = 0u64;
                loop {
                    x = black_box(x.wrapping_add(1));
                }
            }
            "open_file" => escape()?,
            "law_error" => return Err("deliberate error from the misbehaving fixture".into()),
            "print" => {
                let chunk = [b'x'; 1024];
                for _ in 0..64 {
                    let _ = std::io::stdout().write_all(&chunk);
                    let _ = std::io::stderr().write_all(&chunk);
                }
            }
            _ => {}
        }
        let clipped = inputs.limited.then_some(inputs.drive.as_slice());
        let mut o = out(self.math.iterate(&inputs.response, clipped)?);
        match self.mode.as_str() {
            "nan" => {
                let mid = o.drive.len() / 2;
                o.drive[mid] = f64::NAN;
            }
            "wrong_length" => {
                o.drive.pop();
            }
            "undeclared" => o.values.push(hertzforge_law_sdk::DiagnosticValue {
                key: "not_declared".into(),
                value: 1.0,
            }),
            _ => {}
        }
        Ok(o)
    }
}

export_twr_law!(TwrMisbehave);
