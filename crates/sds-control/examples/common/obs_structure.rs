//! The Open Benchmark Set's jointed structure as the simulator runs it: the loader the
//! open-loop player (`obs_play`) has always used, shared with the MIMO Sine benchmark runner
//! (`msnb_run`) so both build the structure from the same keys by the same code (case schema
//! `mm-x4-case/2`). Moved here from `obs_play.rs` unchanged; an error ends the program with
//! status 2 and the program's name in front of the message, as `obs_play` did.
use hertzforge_hal_sim::{JointLaw, JointedMode, JointedPlant, JointedStep, LevelRule};
use serde_json::{Map, Value};
use std::path::Path;

/// The running program's file name, for the messages.
fn program() -> String {
    std::env::args()
        .next()
        .and_then(|a| {
            Path::new(&a)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "obs_play".into())
}

pub fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("{}: {msg}", program());
    std::process::exit(2);
}

pub type Obj = Map<String, Value>;

pub fn num(o: &Obj, k: &str) -> Option<f64> {
    match o.get(k) {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_f64()
                .unwrap_or_else(|| die(format!("{k} is not a number"))),
        ),
    }
}
pub fn need(o: &Obj, k: &str) -> f64 {
    num(o, k).unwrap_or_else(|| die(format!("missing {k}")))
}
pub fn vecf(o: &Obj, k: &str) -> Vec<f64> {
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
pub fn load(path: &Path) -> Value {
    let t =
        std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("{}: {e}", path.display())));
    serde_json::from_str(&t).unwrap_or_else(|e| die(format!("{}: {e}", path.display())))
}

pub fn plant_of(p: &Obj) -> JointedPlant {
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
