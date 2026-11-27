//! The modal fit against the research port it was written from: its poles,
//! dampings and residues on the port's own identifications (a synthetic 4 x 3
//! with a close pair at two noise levels, and a 6 x 6 frame's band with a
//! near-repeated pair), each with its known delay.
mod common;

use common::{f64s, identification, load};
use hertzforge_mimo_sine_law_math::modal_fit::fit_known_delay;
use hertzforge_mimo_sine_law_math::C64;
use serde_json::Value;

/// The largest relative errors of a fit against the port's: frequency,
/// damping, and the residues' (Frobenius over all modes).
fn errors(c: &Value, tau_s: f64) -> (usize, usize, f64, f64, f64) {
    let id = identification(c);
    let b = f64s(&c["band"]);
    let fit = fit_known_delay(&id, (b[0], b[1]), tau_s).expect("fit");
    let (f, z) = (f64s(&c["f_hz"]), f64s(&c["zeta"]));
    let a: Vec<C64> = f64s(&c["a_re"])
        .into_iter()
        .zip(f64s(&c["a_im"]))
        .map(|(r, i)| C64::new(r, i))
        .collect();
    if fit.f_hz.len() != f.len() {
        return (
            fit.f_hz.len(),
            f.len(),
            f64::INFINITY,
            f64::INFINITY,
            f64::INFINITY,
        );
    }
    let ef = fit
        .f_hz
        .iter()
        .zip(&f)
        .map(|(x, y)| (x - y).abs() / y)
        .fold(0.0, f64::max);
    let ez = fit
        .zeta
        .iter()
        .zip(&z)
        .map(|(x, y)| (x - y).abs() / y)
        .fold(0.0, f64::max);
    let num: f64 = fit
        .residues
        .iter()
        .zip(&a)
        .map(|(x, y)| (x - y).norm_sqr())
        .sum();
    let den: f64 = a.iter().map(|y| y.norm_sqr()).sum();
    (fit.f_hz.len(), f.len(), ef, ez, (num / den).sqrt())
}

#[test]
fn the_fit_is_the_research_ports() {
    let v = load("m7d_parity.json");
    let cases = v["cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 3);
    for c in cases {
        let tau = c["tau_s"].as_f64().expect("tau");
        let (got, want, ef, ez, ea) = errors(c, tau);
        eprintln!(
            "{}: modes {got} of {want}, frequency {ef:.2e}, damping {ez:.2e}, residues {ea:.2e}",
            c["name"]
        );
        assert_eq!(got, want, "{}", c["name"]);
        assert!(ef < 1e-9, "{}: frequency {ef:e}", c["name"]);
        assert!(ez < 1e-7, "{}: damping {ez:e}", c["name"]);
        assert!(ea < 1e-7, "{}: residues {ea:e}", c["name"]);
    }
}

#[test]
fn the_parity_check_fails_on_a_wrong_delay() {
    // The same comparison with the delay off by one sample at the port's rate:
    // the residues turn and the check must see it.
    let v = load("m7d_parity.json");
    let c = &v["cases"][0];
    let tau = c["tau_s"].as_f64().expect("tau") + 1.0 / 8192.0;
    let (got, want, ef, ez, ea) = errors(c, tau);
    assert!(
        got != want || ef >= 1e-9 || ez >= 1e-7 || ea >= 1e-7,
        "a wrong delay passed: {ef:e} {ez:e} {ea:e}"
    );
}
