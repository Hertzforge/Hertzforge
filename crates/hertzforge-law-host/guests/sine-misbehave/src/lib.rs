//! A Sine law that misbehaves on purpose, for the host's and the core's tests.
//! Its engage, and every call in `ok` mode, is the built-in `servo_blend` law;
//! the `mode` parameter picks what goes wrong on `tick`:
//!
//! - `ok`: the built-in law.
//! - `trap`: panics (a trap, with a message on stderr).
//! - `spin`: loops forever.
//! - `grow`: allocates until memory runs out.
//! - `sleep`: sleeps 30 seconds, then answers (under the host a sleep returns
//!   at once, so this one answers).
//! - `sleep_after`: the first `after` ticks answer; every later one runs past
//!   any budget (a busy loop).
//! - `open_file`: tries to read, list and write files and open a network
//!   connection, and returns an error if every attempt fails.
//! - `nan`, `negative`, `undeclared`: returns a NaN drive, a drive below
//!   zero, or a value the manifest does not declare.
//! - `law_error`: returns an error.
//! - `print`: writes 64 KiB to each of stdout and stderr, then answers.
//! - `adjust_error`: every live change is refused by the law.

use hertzforge_law_sdk::sine::native::{servo_blend_manifest, ServoBlendFile};
use hertzforge_law_sdk::sine::*;
use hertzforge_law_sdk::{
    export_sine_law, ChoiceSpec, IntegerSpec, Param, ParamKind, ParamSpec, Params, SineSingleLaw,
};
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
    "negative",
    "undeclared",
    "law_error",
    "print",
    "adjust_error",
];

/// The fixture's state.
pub struct SineMisbehave {
    mode: String,
    after: i64,
    law: ServoBlendFile,
    ticks: i64,
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

impl SineSingleLaw for SineMisbehave {
    fn describe() -> SineManifest {
        let mut m = servo_blend_manifest();
        m.law.name = "misbehaving Sine fixture".into();
        m.law.version = "0.0.0".into();
        m.law.summary = "A test fixture that misbehaves in the way its mode names.".into();
        m.law.params.push(ParamSpec {
            key: "mode".into(),
            label: "Mode".into(),
            help: "What goes wrong.".into(),
            kind: ParamKind::Choice(ChoiceSpec {
                options: MODES.iter().map(|m| m.to_string()).collect(),
                default: "ok".into(),
            }),
        });
        m.law.params.push(ParamSpec {
            key: "after".into(),
            label: "Good ticks first".into(),
            help: "With sleep_after: how many ticks answer before every call runs late.".into(),
            kind: ParamKind::Integer(IntegerSpec {
                unit: "ticks".into(),
                min: 0,
                max: 100_000,
                step: 1,
                default: 1,
            }),
        });
        m
    }

    fn new(_setup: SineSetup, params: Params) -> Result<Self, String> {
        let mode = params.choice("mode")?.to_string();
        let after = params.integer("after")?;
        let own: Vec<Param> = params
            .0
            .into_iter()
            .filter(|p| p.key != "mode" && p.key != "after")
            .collect();
        Ok(SineMisbehave {
            mode,
            after,
            law: ServoBlendFile::build(&Params(own))?,
            ticks: 0,
        })
    }

    fn engage(&mut self, inputs: &TickInputs) -> Result<TickOutputs, String> {
        self.law.call(true, inputs)
    }

    fn tick(&mut self, inputs: &TickInputs) -> Result<TickOutputs, String> {
        self.ticks += 1;
        match self.mode.as_str() {
            "trap" => panic!("deliberate trap from the misbehaving Sine fixture"),
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
            "sleep_after" if self.ticks > self.after => {
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
        let mut o = self.law.call(false, inputs)?;
        match self.mode.as_str() {
            "nan" => o.drive = f64::NAN,
            "negative" => o.drive = -0.1,
            "undeclared" => o.values.push(hertzforge_law_sdk::DiagnosticValue {
                key: "not_declared".into(),
                value: 1.0,
            }),
            _ => {}
        }
        Ok(o)
    }

    fn adjust(&mut self, p: &Param) -> Result<(), String> {
        if self.mode == "adjust_error" {
            return Err("deliberate refusal from the misbehaving fixture".into());
        }
        self.law.adjust_param(p)
    }
}

export_sine_law!(SineMisbehave);
