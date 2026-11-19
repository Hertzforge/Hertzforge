//! A MIMO Shock law that misbehaves on purpose, for the host's and the core's
//! tests. In `ok` mode, and on every seed unless the mode is `seed_trap`, it
//! returns a small steady drive (a tenth of each drive's ceiling at its first
//! sample, zero elsewhere). The `mode` parameter picks what goes wrong on
//! `iterate`:
//!
//! - `ok`: nothing.
//! - `trap`: panics (a trap, with a message on stderr).
//! - `seed_trap`: the seed panics.
//! - `spin`: loops forever.
//! - `spin_after`: the first iteration answers; every later one loops forever.
//! - `grow`: allocates until memory runs out.
//! - `wrong_drive_count`: one drive record too few.
//! - `wrong_length`: every drive record one sample short.
//! - `nan`: a NaN drive sample.
//! - `infinite`: an infinite drive sample.
//! - `over_ceiling`: the last drive at twice its ceiling.
//! - `bad_delays`: one delay too few.
//! - `undeclared`: a value the manifest does not declare.
//! - `law_error`: returns an error.

use hertzforge_law_sdk::shock_mimo::*;
use hertzforge_law_sdk::{export_shock_mimo_law, Params};
use std::hint::black_box;

const MODES: &[&str] = &[
    "ok",
    "trap",
    "seed_trap",
    "spin",
    "spin_after",
    "grow",
    "wrong_drive_count",
    "wrong_length",
    "nan",
    "infinite",
    "over_ceiling",
    "bad_delays",
    "undeclared",
    "law_error",
];

/// The fixture's state.
pub struct Misbehave {
    mode: String,
    len: usize,
    controls: usize,
    ceiling: Vec<f64>,
    calls: u32,
}

impl Misbehave {
    fn steady(&self) -> Outputs {
        let drives = self
            .ceiling
            .iter()
            .map(|c| {
                let mut r = vec![0.0; self.len];
                r[0] = 0.1 * c;
                r
            })
            .collect();
        Outputs {
            drives,
            delays_s: Some(vec![0.0; self.controls]),
            values: vec![("calls".into(), f64::from(self.calls))],
        }
    }
}

fn spin() -> ! {
    let mut x = 0u64;
    loop {
        x = black_box(x.wrapping_add(1));
    }
}

impl ShockMimoLaw for Misbehave {
    fn describe() -> ShockMimoManifest {
        ManifestBuilder::new("misbehaving MIMO Shock fixture", "0.0.0")
            .summary("A test fixture that misbehaves in the way its mode names.")
            .targets(&[TargetKind::Waveform, TargetKind::Srs])
            .choice("mode", "Mode", "What goes wrong.", MODES, "ok")
            .value("calls", "Calls", "", "Iterations so far.")
            .build()
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        Ok(Misbehave {
            mode: params.choice("mode")?.to_string(),
            len: setup.record_len,
            controls: setup.n_controls,
            ceiling: setup.drive_ceiling_v,
            calls: 0,
        })
    }

    fn seed(&mut self, _level_db: f64) -> Result<Outputs, String> {
        if self.mode == "seed_trap" {
            panic!("the fixture panicked in its seed on purpose");
        }
        Ok(self.steady())
    }

    fn iterate(&mut self, _inputs: &Inputs) -> Result<Outputs, String> {
        self.calls += 1;
        let mut o = self.steady();
        match self.mode.as_str() {
            "trap" => panic!("the fixture panicked on purpose"),
            "spin" => spin(),
            "spin_after" if self.calls > 1 => spin(),
            "grow" => {
                let mut hoard: Vec<Vec<u8>> = Vec::new();
                loop {
                    hoard.push(vec![1u8; 16 << 20]);
                    black_box(&hoard);
                }
            }
            "wrong_drive_count" => {
                o.drives.pop();
            }
            "wrong_length" => {
                for d in &mut o.drives {
                    d.pop();
                }
            }
            "nan" => o.drives[0][1] = f64::NAN,
            "infinite" => o.drives[0][2] = f64::INFINITY,
            "over_ceiling" => {
                let last = o.drives.len() - 1;
                o.drives[last][3] = 2.0 * self.ceiling[last];
            }
            "bad_delays" => o.delays_s = Some(vec![0.0; self.controls - 1]),
            "undeclared" => o.values.push(("not_declared".into(), 1.0)),
            "law_error" => return Err("the fixture refused on purpose".into()),
            _ => {}
        }
        Ok(o)
    }
}

export_shock_mimo_law!(Misbehave);
