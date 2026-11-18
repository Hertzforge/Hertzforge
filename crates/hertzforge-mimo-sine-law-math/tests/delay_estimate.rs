//! The structure delay the loop-speed rule reads: the `principal` form is not
//! set by a noisy small element and reads a resonance's delay; `largest`
//! stays the research testbed's form.

use hertzforge_mimo_sine_law_math::reference_structure::{ModalStructure, Scatter};
use hertzforge_mimo_sine_law_math::setup::{DelayEstimate, GroupDelay};
use hertzforge_mimo_sine_law_math::C64;
use std::f64::consts::PI;

/// Two modes far apart, 2 controls by 2 drives, light damping.
fn two_modes() -> ModalStructure {
    ModalStructure {
        f_hz: vec![100.0, 300.0],
        zeta: vec![0.01, 0.02],
        drive_shape: vec![1.0, 0.6, 0.5, -1.0],
        output_shape: vec![1.0, 0.7, -0.4, 1.0],
        drives: 2,
        controls: 2,
    }
}

/// An exact identification with one element given a noisy phase where it is
/// small (an eighth of the line's largest, above the present form's tenth).
#[test]
fn mimo_sine_principal_delay_is_not_set_by_a_noisy_small_element() {
    let st = two_modes();
    let band = (150.0, 250.0); // between the modes: the true delay is a few ms
    let exact = st.identification(band, 0.25, false);
    let mut noisy = exact.clone();
    let (l, n, m) = noisy.response.dims();
    let mut data = noisy.response.data().to_vec();
    for k in 0..l {
        let line = &mut data[k * n * m..(k + 1) * n * m];
        let big = line.iter().map(|v| v.norm()).fold(0.0, f64::max);
        // Element (1, 0) replaced by an eighth of the largest with a phase
        // that jumps by up to 0.3 rad from line to line: noise, not structure.
        let ph = 0.3 * (2.3 * k as f64).sin();
        line[m] = C64::from_polar(0.125 * big, ph);
    }
    noisy.response = hertzforge_mimo_sine_law_math::MatrixStack::from_vec(l, n, m, data).unwrap();
    let truth = GroupDelay::of(&exact).at(200.0, 0.0);
    let present = GroupDelay::of_with(&noisy, DelayEstimate::Largest).at(200.0, 0.0);
    let principal = GroupDelay::of_with(&noisy, DelayEstimate::Principal).at(200.0, 0.0);
    eprintln!(
        "true {:.1} ms, largest {:.1} ms, principal {:.1} ms",
        truth * 1e3,
        present * 1e3,
        principal * 1e3
    );
    // The present form reads the noise's phase slope (about 0.1 s); the
    // principal form reads the element's share of it, under the tracking
    // filter's own delay at 200 Hz (1 / (pi 20 Hz), 16 ms), so the noise does
    // not set the loop's speed.
    let tracker = 1.0 / (PI * 20.0);
    assert!(
        present > 5.0 * tracker,
        "the case must fool the present form"
    );
    assert!(
        principal < 0.5 * tracker && principal < 0.1 * present,
        "principal read {principal} s against a true {truth} s"
    );
}

/// At a light resonance the `principal` form reads the mode's own delay,
/// `1 / (zeta w)`, within 20 percent (the five-line smoothing takes a little
/// of the peak), with and without scatter.
#[test]
fn mimo_sine_principal_delay_reads_a_resonances_delay() {
    let st = two_modes();
    let band = (60.0, 340.0);
    for (r, fr) in st.f_hz.iter().enumerate() {
        let q = 1.0 / (st.zeta[r] * 2.0 * PI * fr);
        for sc in [None, Some(Scatter::of(0.02, 7)), Some(Scatter::of(0.05, 8))] {
            let id = match sc {
                None => st.identification(band, 0.25, false),
                Some(s) => st.identification_scattered(band, 0.25, s),
            };
            let got = GroupDelay::of_with(&id, DelayEstimate::Principal).at(*fr, 0.0);
            eprintln!(
                "{fr} Hz: 1/(zeta w) {:.1} ms, principal {:.1} ms",
                q * 1e3,
                got * 1e3
            );
            assert!(
                got > 0.8 * q && got < 1.25 * q,
                "{fr} Hz: principal {got} s against {q} s"
            );
        }
    }
}

/// The declared names round-trip and the present form is the default.
#[test]
fn mimo_sine_delay_estimate_names_round_trip() {
    for n in DelayEstimate::NAMES {
        assert_eq!(DelayEstimate::from_name(n).unwrap().name(), n);
    }
    assert_eq!(DelayEstimate::default(), DelayEstimate::Largest);
    assert!(DelayEstimate::from_name("smallest").is_none());
}
