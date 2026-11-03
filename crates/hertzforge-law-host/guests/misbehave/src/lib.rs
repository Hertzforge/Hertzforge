//! A law that misbehaves on purpose, for the host's tests. The `mode`
//! parameter picks what `iterate` does; `seed` and `retarget-level` always
//! return the target at the scheduled level as the drive.
//!
//! - `ok`: return the target at the scheduled level as the drive.
//! - `trap`: panic (a trap, with a message on stderr).
//! - `spin`: loop forever.
//! - `grow`: allocate until memory runs out.
//! - `open_file`: try to read, list and write files and open a network
//!   connection; return an error if every attempt fails.
//! - `sleep`: sleep for 30 seconds, then return the target.
//! - `probe`: report the wall clock, the time a busy loop took, a hash keyed
//!   by the random source, and the counts of environment variables and
//!   arguments.
//! - `nan`, `short`, `undeclared`: return a drive with a NaN, a drive one
//!   line short, or an undeclared diagnostic.
//! - `law_error`: return an error.
//! - `print`: write 64 KiB to each of stdout and stderr, then return the
//!   target.

use hertzforge_law_sdk::{
    band_range, export_law, no_diagnostics, Band, ChoiceSpec, DiagnosticSpec, DiagnosticValue,
    LoopInputs, LoopOutputs, Manifest, ParamKind, ParamSpec, Params, RandomLaw, Setup,
};
use std::hash::BuildHasher;
use std::hint::black_box;
use std::io::Write;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MODES: &[&str] = &[
    "ok",
    "trap",
    "spin",
    "grow",
    "open_file",
    "sleep",
    "probe",
    "nan",
    "short",
    "undeclared",
    "law_error",
    "print",
];

const PROBES: &[&str] = &[
    "wall_clock_ns",
    "busy_loop_ns",
    "hash_probe",
    "env_vars",
    "args",
    "escaped",
];

/// The fixture's state.
pub struct Misbehave {
    mode: String,
    target: Vec<f64>,
    band: Band,
    /// The scheduled level, dB re the full target.
    level_db: f64,
}

impl Misbehave {
    fn drive(&self) -> Vec<f64> {
        let power = 10f64.powf(self.level_db / 10.0);
        let mut d = vec![0.0; self.target.len()];
        for k in band_range(self.band) {
            d[k] = self.target[k] * power;
        }
        d
    }

    fn ok(&self, values: Vec<(&str, f64)>) -> LoopOutputs {
        let mut diagnostics = no_diagnostics();
        diagnostics.values = values
            .into_iter()
            .map(|(key, value)| DiagnosticValue {
                key: key.into(),
                value,
            })
            .collect();
        LoopOutputs {
            drive_psd: self.drive(),
            level_cmd_db: self.level_db,
            diagnostics,
        }
    }

    fn escape(&self) -> Result<LoopOutputs, String> {
        let attempts: [(&str, std::io::Result<()>); 4] = [
            ("read /etc/hosts", std::fs::read("/etc/hosts").map(drop)),
            ("list .", std::fs::read_dir(".").map(drop)),
            ("write escape.txt", std::fs::write("escape.txt", b"x")),
            (
                "connect 127.0.0.1:9",
                std::net::TcpStream::connect("127.0.0.1:9").map(drop),
            ),
        ];
        let escaped = attempts.iter().filter(|(_, r)| r.is_ok()).count();
        if escaped == 0 {
            let why: Vec<String> = attempts
                .iter()
                .map(|(what, r)| match r {
                    Ok(()) => format!("{what}: ok"),
                    Err(e) => format!("{what}: {e}"),
                })
                .collect();
            Err(format!("every escape failed ({})", why.join("; ")))
        } else {
            Ok(self.ok(vec![("escaped", escaped as f64)]))
        }
    }

    fn probe(&self) -> LoopOutputs {
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as f64)
            .unwrap_or(-1.0);
        let t0 = Instant::now();
        let mut x = 0u64;
        for i in 0..200_000u64 {
            x = black_box(x.wrapping_mul(31).wrapping_add(i));
        }
        let busy = t0.elapsed().as_nanos() as f64;
        let hash = std::collections::hash_map::RandomState::new().hash_one(0x5eed_u64) as f64;
        self.ok(vec![
            ("wall_clock_ns", wall),
            ("busy_loop_ns", busy),
            ("hash_probe", hash),
            ("env_vars", std::env::vars().count() as f64),
            ("args", std::env::args().count() as f64),
        ])
    }
}

impl RandomLaw for Misbehave {
    fn describe() -> Manifest {
        Manifest {
            name: "misbehaving fixture".into(),
            version: "0.0.0".into(),
            author: String::new(),
            summary: "A test fixture that misbehaves in the way its mode names.".into(),
            params: vec![ParamSpec {
                key: "mode".into(),
                label: "Mode".into(),
                help: "What iterate does.".into(),
                kind: ParamKind::Choice(ChoiceSpec {
                    options: MODES.iter().map(|m| m.to_string()).collect(),
                    default: "ok".into(),
                }),
            }],
            diagnostics: PROBES
                .iter()
                .map(|k| DiagnosticSpec {
                    key: k.to_string(),
                    label: k.to_string(),
                    unit: String::new(),
                    help: String::new(),
                })
                .collect(),
        }
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        Ok(Misbehave {
            mode: params.choice("mode")?.to_string(),
            target: setup.target_psd,
            band: setup.band,
            level_db: 0.0,
        })
    }

    fn seed(&mut self, _inputs: &LoopInputs) -> Result<LoopOutputs, String> {
        Ok(self.ok(Vec::new()))
    }

    fn iterate(&mut self, _inputs: &LoopInputs) -> Result<LoopOutputs, String> {
        match self.mode.as_str() {
            "trap" => panic!("deliberate trap from the misbehaving fixture"),
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
            "open_file" => self.escape(),
            "sleep" => {
                std::thread::sleep(Duration::from_secs(30));
                Ok(self.ok(Vec::new()))
            }
            "probe" => Ok(self.probe()),
            "nan" => {
                let mut out = self.ok(Vec::new());
                let lo = band_range(self.band).start;
                out.drive_psd[lo] = f64::NAN;
                Ok(out)
            }
            "short" => {
                let mut out = self.ok(Vec::new());
                out.drive_psd.pop();
                Ok(out)
            }
            "undeclared" => Ok(self.ok(vec![("not_declared", 1.0)])),
            "law_error" => Err("deliberate error from the misbehaving fixture".into()),
            "print" => {
                let chunk = [b'x'; 1024];
                for _ in 0..64 {
                    let _ = std::io::stdout().write_all(&chunk);
                    let _ = std::io::stderr().write_all(&chunk);
                }
                Ok(self.ok(Vec::new()))
            }
            _ => Ok(self.ok(Vec::new())),
        }
    }

    fn retarget_level(&mut self, level_db: f64, _target_rms: f64) -> Result<LoopOutputs, String> {
        self.level_db = level_db;
        Ok(self.ok(Vec::new()))
    }
}

export_law!(Misbehave);
