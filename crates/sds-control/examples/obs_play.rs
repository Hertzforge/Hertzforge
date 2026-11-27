//! The benchmark study's open-loop player: play a drive record through a jointed
//! structure given as data, from rest, with no noise, by the simulator's own step, and
//! write every output sample (and, if asked, every mode's envelope after each sample).
//! It is the player the Hertzforge Journal's benchmark study used to check that the
//! Open Benchmark Set's reference model and the simulator give the same samples.
//!
//! ```text
//! cargo run --release -p sds-control --example obs_play -- <case-or-structure.json> \
//!     --drive <drive.f8> --out <out.f8> [--env <env.f8>] [--fs 8192] \
//!     [--step hold | --step equalized:<band_hz>:<half_taps>]
//! ```
//!
//! The JSON is a case file of the benchmark study's run program (`structure`, or
//! `structure_file`) or a bare jointed structure object, read with the keys of case
//! schema `mm-x4-case/2`; `to_runner_structure` in the set's `code/reference_model.py`
//! writes one from a problem. The drive is raw little-endian float64,
//! `(samples, n_drives)` row-major. The output is `(samples, n_outputs)`, the envelope
//! `(samples, n_modes)`, both the same type. `--step` overrides the structure's own
//! `step`. Nothing in the structure's behavior is changed here: the samples come from
//! `JointedPlant::stepper`. The set's `code/tests` run this player when the environment
//! variable `OBS_PLAY` names its build.
use hertzforge_hal_sim::JointedStep;
use serde_json::Value;
use std::io::Write;
use std::path::Path;

#[allow(dead_code)]
#[path = "common/obs_structure.rs"]
mod obs_structure;
use obs_structure::{die, load, plant_of};

fn main() {
    let mut args = std::env::args().skip(1);
    let json_path = args
        .next()
        .unwrap_or_else(|| die("usage: see the file's docs"));
    let (mut drive, mut out, mut env, mut fs, mut step) = (None, None, None, 8192.0, None);
    while let Some(a) = args.next() {
        let v = args
            .next()
            .unwrap_or_else(|| die(format!("{a} needs a value")));
        match a.as_str() {
            "--drive" => drive = Some(v),
            "--out" => out = Some(v),
            "--env" => env = Some(v),
            "--fs" => fs = v.parse().unwrap_or_else(|_| die("bad --fs")),
            "--step" => step = Some(v),
            _ => die(format!("unknown flag {a}")),
        }
    }
    let root = load(Path::new(&json_path));
    let root = root
        .as_object()
        .unwrap_or_else(|| die("the JSON is not an object"));
    let pv = if root.contains_key("kind") {
        Value::Object(root.clone())
    } else if let Some(Value::String(f)) = root.get("structure_file") {
        load(Path::new(f))
    } else {
        root.get("structure")
            .cloned()
            .unwrap_or_else(|| die("no structure"))
    };
    let mut plant = plant_of(
        pv.as_object()
            .unwrap_or_else(|| die("the structure is not an object")),
    );
    if let Some(s) = step {
        plant.step = if s == "hold" {
            JointedStep::TriangleHold
        } else {
            let parts: Vec<&str> = s.split(':').collect();
            if parts.len() != 3 || parts[0] != "equalized" {
                die("--step hold | equalized:<band_hz>:<half_taps>");
            }
            JointedStep::Equalized {
                band_hz: parts[1].parse().unwrap_or_else(|_| die("bad band")),
                half_taps: parts[2].parse().unwrap_or_else(|_| die("bad half_taps")),
            }
        };
    }
    let bytes = std::fs::read(drive.unwrap_or_else(|| die("--drive is required")))
        .unwrap_or_else(|e| die(e));
    let x: Vec<f64> = bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| f64::from_le_bytes(*c))
        .collect();
    let (nd, no, nm) = (plant.n_drives, plant.n_inputs, plant.modes.len());
    if !x.len().is_multiple_of(nd) {
        die("the drive length is not a whole number of frames");
    }
    let n = x.len() / nd;
    let mut st = plant.stepper(fs).unwrap_or_else(|e| die(e));
    let mut y = vec![0.0; no];
    let mut ybuf: Vec<u8> = Vec::with_capacity(n * no * 8);
    let mut ebuf: Vec<u8> = Vec::with_capacity(if env.is_some() { n * nm * 8 } else { 0 });
    for t in 0..n {
        st.step(&x[t * nd..(t + 1) * nd], &mut y);
        for v in &y {
            ybuf.extend_from_slice(&v.to_le_bytes());
        }
        if env.is_some() {
            for v in st.envelope() {
                ebuf.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    let write = |p: &str, b: &[u8]| {
        std::fs::File::create(p)
            .and_then(|mut f| f.write_all(b))
            .unwrap_or_else(|e| die(format!("{p}: {e}")))
    };
    write(&out.unwrap_or_else(|| die("--out is required")), &ybuf);
    if let Some(e) = env {
        write(&e, &ebuf);
    }
    eprintln!(
        "obs_play: {n} samples, {nd} drives, {no} outputs, {nm} modes, step {:?}, step delay {}",
        plant.step,
        plant.step_delay_samples()
    );
}
