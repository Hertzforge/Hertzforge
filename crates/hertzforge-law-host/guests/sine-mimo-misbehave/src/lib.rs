//! A MIMO Sine law that misbehaves on purpose, for the host's and the core's
//! tests. In `ok` mode it returns a small steady drive (a tenth of each
//! drive's ceiling, in phase) with one declared value. The `mode` parameter
//! picks what goes wrong; unless the mode names the constructor or engage, it
//! goes wrong on `tick`:
//!
//! - `ok`: nothing.
//! - `new_error`: the constructor refuses the run.
//! - `new_spin`: the constructor loops forever.
//! - `engage_trap`: engage panics.
//! - `trap`: panics (a trap, with a message on stderr).
//! - `spin`: loops forever.
//! - `spin_after`: the first tick answers; every later one loops forever.
//! - `grow`: allocates until memory runs out.
//! - `wrong_drive_count`: one drive too few.
//! - `nan`: a NaN drive.
//! - `infinite`: an infinite drive.
//! - `too_many_values`: nine values.
//! - `undeclared`: a value the manifest does not declare.
//! - `bad_request`: a slow request at 1.5 times the scheduled rate.
//! - `wrong_reference_count`: a notched reference one value short.
//! - `over_ceiling`: the last drive at twice its ceiling.
//! - `law_error`: returns an error.

use hertzforge_law_sdk::sine_mimo::*;
use hertzforge_law_sdk::{export_sine_mimo_law, Params};
use std::hint::black_box;

const MODES: &[&str] = &[
    "ok",
    "new_error",
    "new_spin",
    "engage_trap",
    "trap",
    "spin",
    "spin_after",
    "grow",
    "wrong_drive_count",
    "nan",
    "infinite",
    "too_many_values",
    "undeclared",
    "bad_request",
    "wrong_reference_count",
    "over_ceiling",
    "law_error",
];

/// The fixture's state.
pub struct Misbehave {
    mode: String,
    controls: usize,
    ceiling: Vec<f64>,
    ticks: u32,
}

impl Misbehave {
    fn steady(&self) -> Outputs {
        let mut o = Outputs::drive(
            self.ceiling
                .iter()
                .map(|c| C64::new(0.1 * c, 0.0))
                .collect(),
        );
        o.values.push(("ticks".into(), f64::from(self.ticks)));
        o
    }
}

fn spin() -> ! {
    let mut x = 0u64;
    loop {
        x = black_box(x.wrapping_add(1));
    }
}

impl SineMimoLaw for Misbehave {
    fn describe() -> SineMimoManifest {
        ManifestBuilder::new("misbehaving MIMO Sine fixture", "0.0.0")
            .summary("A test fixture that misbehaves in the way its mode names.")
            .shapes(&ALL_SHAPES)
            .test_types(&TestType::ALL)
            .amplitude_only(true)
            .choice("mode", "Mode", "What goes wrong.", MODES, "ok")
            .value("ticks", "Ticks", "", "Ticks so far.")
            .build()
    }

    fn new(setup: Setup, params: Params) -> Result<Self, String> {
        let mode = params.choice("mode")?.to_string();
        match mode.as_str() {
            "new_error" => return Err("the fixture refused the run on purpose".into()),
            "new_spin" => spin(),
            _ => {}
        }
        Ok(Misbehave {
            mode,
            controls: setup.controls,
            ceiling: setup.drive_ceiling_v,
            ticks: 0,
        })
    }

    fn engage(&mut self, _inputs: &Inputs) -> Result<Outputs, String> {
        if self.mode == "engage_trap" {
            panic!("the fixture panicked in engage on purpose");
        }
        Ok(self.steady())
    }

    fn tick(&mut self, _inputs: &Inputs) -> Result<Outputs, String> {
        self.ticks += 1;
        let mut o = self.steady();
        match self.mode.as_str() {
            "trap" => panic!("the fixture panicked on purpose"),
            "spin" => spin(),
            "spin_after" if self.ticks > 1 => spin(),
            "grow" => {
                let mut hoard: Vec<Vec<u8>> = Vec::new();
                loop {
                    hoard.push(vec![1u8; 16 << 20]);
                    black_box(&hoard);
                }
            }
            "wrong_drive_count" => {
                o.drive.pop();
            }
            "nan" => o.drive[0].im = f64::NAN,
            "infinite" => o.drive[0].re = f64::INFINITY,
            "too_many_values" => o.values = vec![("ticks".into(), 1.0); 9],
            "undeclared" => o.values.push(("not_declared".into(), 1.0)),
            "bad_request" => o.request = Request::Slow(1.5),
            "wrong_reference_count" => {
                o.notched_reference = Some(vec![C64::new(0.1, 0.0); self.controls - 1])
            }
            "over_ceiling" => {
                let last = o.drive.len() - 1;
                o.drive[last] = C64::new(2.0 * self.ceiling[last], 0.0);
            }
            "law_error" => return Err("the fixture refused on purpose".into()),
            _ => {}
        }
        Ok(o)
    }
}

export_sine_mimo_law!(Misbehave);
