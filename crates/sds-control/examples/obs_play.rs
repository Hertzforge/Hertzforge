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
use hertzforge_hal_sim::{JointLaw, JointedMode, JointedPlant, JointedStep, LevelRule};
use serde_json::{Map, Value};
use std::io::Write;
use std::path::Path;

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("obs_play: {msg}");
    std::process::exit(2);
}

type Obj = Map<String, Value>;

fn num(o: &Obj, k: &str) -> Option<f64> {
    match o.get(k) {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_f64()
                .unwrap_or_else(|| die(format!("{k} is not a number"))),
        ),
    }
}
fn need(o: &Obj, k: &str) -> f64 {
    num(o, k).unwrap_or_else(|| die(format!("missing {k}")))
}
fn vecf(o: &Obj, k: &str) -> Vec<f64> {
    match o.get(k) {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => v
            .as_array()
            .unwrap_or_else(|| die(format!("{k} is not an array")))
            .iter()
            .map(|x| {
                x.as_f64()
                    .unwrap_or_else(|| die(format!("{k} holds a non-number")))
            })
            .collect(),
    }
}
fn load(path: &Path) -> Value {
    let t =
        std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("{}: {e}", path.display())));
    serde_json::from_str(&t).unwrap_or_else(|e| die(format!("{}: {e}", path.display())))
}

fn plant_of(p: &Obj) -> JointedPlant {
    if p.get("kind").and_then(Value::as_str) != Some("jointed") {
        die("only a jointed structure is played");
    }
    let n_o = need(p, "n_outputs") as usize;
    let n_d = need(p, "n_drives") as usize;
    let modes: Vec<JointedMode> = p["modes"]
        .as_array()
        .unwrap_or_else(|| die("no modes"))
        .iter()
        .map(|m| {
            let m = m
                .as_object()
                .unwrap_or_else(|| die("a mode is not an object"));
            let level = match m.get("level") {
                None | Some(Value::Null) => None,
                Some(l) => {
                    let l = l
                        .as_object()
                        .unwrap_or_else(|| die("level is not an object"));
                    Some(LevelRule {
                        drop: need(l, "drop"),
                        drop_exponent: need(l, "drop_exponent"),
                        rise: need(l, "rise"),
                        rise_exponent: need(l, "rise_exponent"),
                        rise_cap: need(l, "rise_cap"),
                        damping_gain: need(l, "damping_gain"),
                        damping_exponent: need(l, "damping_exponent"),
                        damping_cap: num(l, "damping_cap").unwrap_or_else(|| {
                            die("a mode's level needs \"damping_cap\" (the amplitude ratio \
                                 above which its damping stops growing, required since \
                                 2026-11-14)")
                        }),
                    })
                }
            };
            JointedMode {
                f_hz: need(m, "f_hz"),
                zeta: need(m, "zeta"),
                input_shape: vecf(m, "drive_shape"),
                output_shape: vecf(m, "output_shape"),
                reference_amplitude: need(m, "reference_amplitude"),
                level,
            }
        })
        .collect();
    let mut jp = JointedPlant::new(n_o, n_d, modes);
    jp.joint = match p.get("joint") {
        None | Some(Value::Null) => None,
        Some(j) => {
            let j = j
                .as_object()
                .unwrap_or_else(|| die("joint is not an object"));
            let mut law = JointLaw::new(need(j, "delta"), need(j, "damping_gain"));
            if let Some(v) = num(j, "freq_exponent") {
                law.freq_exponent = v;
            }
            if let Some(v) = num(j, "damping_exponent") {
                law.damping_exponent = v;
            }
            if let Some(v) = num(j, "decay_cycles") {
                law.decay_cycles = v;
            }
            Some(law)
        }
    };
    jp.feedthrough = num(p, "feedthrough").unwrap_or(0.0);
    jp.residual = vecf(p, "residual");
    jp.transport_delay_samples = num(p, "transport_delay_samples").unwrap_or(0.0) as usize;
    jp.output_delays_s = vecf(p, "output_delays_s");
    if let Some(Value::Object(s)) = p.get("step") {
        jp.step = match s.get("kind").and_then(Value::as_str) {
            Some("triangle_hold") => JointedStep::TriangleHold,
            Some("equalized") => JointedStep::Equalized {
                band_hz: need(s, "band_hz"),
                half_taps: num(s, "half_taps").unwrap_or(2.0) as usize,
            },
            _ => die("unknown step kind"),
        };
    }
    jp
}

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
