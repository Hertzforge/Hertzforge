//! `followed_poles` against the research port it was written from: the port's
//! law run on a quasi-steady loop over a structure moved from its
//! identification (one run the poles follow through a change cap, one where
//! the gate falls back), every update replayed here with the port's inputs and
//! the answers, the values and the followed poles compared.
mod common;

use common::{b64_cplx, b64_f64s, identification, load, testbed_rule};
use hertzforge_mimo_sine_law_math::followed::{FollowedLaw, FollowedParams};
use hertzforge_mimo_sine_law_math::law::{Hold, Inputs, Outputs, SineLaw};
use hertzforge_mimo_sine_law_math::setup::{LoopDelay, Shape, TrackingRule};
use hertzforge_mimo_sine_law_math::{Setup, C64};
use serde_json::Value;

fn setup(v: &Value) -> Setup {
    let id = identification(&load("m7d_parity.json")["cases"][0]);
    let lines = id.grid_hz.len();
    let (n, m) = (4, 3);
    let by_line = b64_cplx(&v["reference"]);
    let reference = (0..n * lines)
        .map(|o| by_line[(o % lines) * n + o / lines])
        .collect();
    Setup {
        controls: n,
        drives: m,
        limits: 0,
        fs_hz: 8192.0,
        tick_s: v["tick_s"].as_f64().expect("tick"),
        band_lo_hz: 40.0,
        band_hi_hz: 160.0,
        drive_ceiling_v: vec![10.0; m],
        identification: id,
        identification_level_db: f64::NAN,
        reference,
        phase_specified: true,
        limit_bounds: Vec::new(),
        alarm_db: vec![3.0; n],
        abort_db: vec![6.0; n],
        shape: Shape::of(n, m),
        tracking: TrackingRule::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        loop_delay: LoopDelay {
            tracker_delay_factor: 0.625,
            transport_delay_s: 0.0,
        },
    }
}

fn params(v: &Value, c: &Value) -> FollowedParams {
    let mut p = FollowedParams {
        station_delay_s: v["tau_s"].as_f64().expect("tau"),
        gain_rule: testbed_rule(),
        ..Default::default()
    };
    if let Some(w) = c["params"]["window"].as_u64() {
        p.window = w as usize;
    }
    if let Some(r) = c["params"]["gate_ratio"].as_f64() {
        p.gate_ratio = r;
    }
    p
}

/// The worst disagreements over a run: the answers (each update's largest
/// over its largest), each value, the followed frequencies and dampings.
struct Worst {
    drive: f64,
    values: [f64; 7],
    fr: f64,
    zr: f64,
}

fn replay(v: &Value, c: &Value, p: FollowedParams) -> Worst {
    let s = setup(v);
    let (n, m) = (s.controls, s.drives);
    let mut law = FollowedLaw::new(&s, p).expect("law");
    let modes = c["modes"].as_u64().expect("modes") as usize;
    assert_eq!(law.model().modes.modes(), modes, "{}", c["name"]);
    let t = c["ticks"].as_u64().expect("ticks") as usize;
    let f = b64_f64s(c["f_hz"].as_str().expect("f"));
    let scale = b64_f64s(c["scale"].as_str().expect("scale"));
    let ctl = b64_cplx(&c["controls"]);
    let drive = b64_cplx(&c["drive"]);
    let vals = b64_f64s(c["values"].as_str().expect("values"));
    let fr = b64_f64s(c["fr"].as_str().expect("fr"));
    let zr = b64_f64s(c["zr"].as_str().expect("zr"));
    let holds = c["hold"].as_array().expect("hold");
    let oct = c["oct_min"].as_f64().expect("oct");
    let mut w = Worst {
        drive: 0.0,
        values: [0.0; 7],
        fr: 0.0,
        zr: 0.0,
    };
    let mut out = Outputs::new(m, 8);
    let mut r = vec![C64::new(0.0, 0.0); n];
    for k in 0..t {
        let rate = f[k] * 2f64.ln() * oct / 60.0;
        let d_in: Vec<C64> = if k == 0 {
            vec![C64::new(0.0, 0.0); m]
        } else {
            drive[(k - 1) * m..k * m]
                .iter()
                .map(|d| d * scale[k])
                .collect()
        };
        s.reference_at(f[k], &mut r);
        let hold = if holds[k].as_str() == Some("e") {
            Hold::Engage
        } else {
            Hold::Free
        };
        let x = Inputs {
            index: k as u64,
            dt_s: s.tick_s,
            f_hz: f[k],
            sweep_hz_s: rate,
            sweep_oct_min: oct,
            level: 1.0,
            reference: &r,
            controls: &ctl[k * n..(k + 1) * n],
            limits: &[],
            limit_bounds: &[],
            drive_in_force: &d_in,
            hold,
            notch_scale: 1.0,
            settle_open: hold == Hold::Engage,
        };
        if k == 0 {
            law.engage(&x, &mut out).expect("engage");
        } else {
            law.tick(&x, &mut out).expect("tick");
        }
        let want = &drive[k * m..(k + 1) * m];
        let big = want.iter().map(|d| d.norm()).fold(1e-300, f64::max);
        let e = out
            .drive
            .iter()
            .zip(want)
            .map(|(a, b)| (a - b).norm())
            .fold(0.0, f64::max)
            / big;
        w.drive = w.drive.max(e);
        for i in 0..7 {
            let (a, b) = (out.values[i], vals[k * 7 + i]);
            w.values[i] = w.values[i].max((a - b).abs() / b.abs().max(1.0));
        }
        let (gf, gz) = law.followed();
        for j in 0..modes {
            w.fr =
                w.fr.max((gf[j] - fr[k * modes + j]).abs() / fr[k * modes + j]);
            w.zr =
                w.zr.max((gz[j] - zr[k * modes + j]).abs() / zr[k * modes + j]);
        }
    }
    w
}

/// Agreement to rounding: the answers and the values to 1e-9, the followed
/// frequencies to 1e-12 and dampings to 1e-9 (the fit's own agreement).
fn agrees(w: &Worst) -> bool {
    w.drive < 1e-9 && w.values.iter().all(|e| *e < 1e-9) && w.fr < 1e-12 && w.zr < 1e-9
}

#[test]
fn the_law_is_the_research_ports() {
    let v = load("followed_poles_parity.json");
    for c in v["cases"].as_array().expect("cases") {
        let w = replay(&v, c, params(&v, c));
        eprintln!(
            "{}: drive {:.2e}, values {:?}, frequency {:.2e}, damping {:.2e}",
            c["name"],
            w.drive,
            w.values.map(|e| format!("{e:.1e}")),
            w.fr,
            w.zr
        );
        assert!(agrees(&w), "{}", c["name"]);
    }
}

#[test]
fn the_law_parity_check_fails_on_a_changed_law() {
    // The same replay with the fit's prior half as wide: the poles follow
    // differently and the check must see it.
    let v = load("followed_poles_parity.json");
    let c = &v["cases"][0];
    let p = FollowedParams {
        prior_bw: 2.0,
        ..params(&v, c)
    };
    let w = replay(&v, c, p);
    assert!(
        !agrees(&w),
        "a changed law passed: {:e} {:e}",
        w.drive,
        w.fr
    );
}
