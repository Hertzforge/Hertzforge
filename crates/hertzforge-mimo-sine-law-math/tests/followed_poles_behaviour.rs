//! `followed_poles` and its modal fit: the delay they are told, and the
//! refusal without it. Each check is shown to fail on a case built to break it.
use hertzforge_mimo_sine_law_math::followed::{FollowedLaw, FollowedParams};
use hertzforge_mimo_sine_law_math::modal_fit::fit_known_delay;
use hertzforge_mimo_sine_law_math::reference_structure::ModalStructure;
use hertzforge_mimo_sine_law_math::{Setup, C64};
use std::f64::consts::TAU;

const BAND: (f64, f64) = (40.0, 120.0);
const DELAY_SAMPLES: f64 = 6.0;

fn structure() -> ModalStructure {
    ModalStructure {
        f_hz: vec![52.0, 71.0, 97.0],
        zeta: vec![0.012, 0.010, 0.015],
        drive_shape: vec![1.0, 0.3, -0.4, 0.9, 0.5, 0.6],
        output_shape: vec![0.8, 0.2, 0.3, 1.0, -0.7, 0.5],
        drives: 2,
        controls: 2,
    }
}

/// A setup on the structure whose identification holds a pure delay of
/// `DELAY_SAMPLES` at its rate.
fn delayed_setup() -> Setup {
    let mut s = structure().setup(BAND, false);
    let tau = DELAY_SAMPLES / s.fs_hz;
    let g = s.identification.grid_hz.clone();
    for (k, f) in g.iter().enumerate() {
        let turn = C64::from_polar(1.0, -TAU * f * tau);
        s.identification
            .response
            .line_mut(k)
            .iter_mut()
            .for_each(|v| *v *= turn);
    }
    s
}

/// The largest error of the fitted residues against the structure's, each
/// mode's over its own size, the delay turn at the mode's frequency on the
/// structure's.
fn residue_error(setup: &Setup, tau_s: f64) -> f64 {
    let st = structure();
    let fit = fit_known_delay(&setup.identification, BAND, tau_s).expect("fit");
    let true_tau = DELAY_SAMPLES / setup.fs_hz;
    let (n, m) = (2, 2);
    let mut worst: f64 = 0.0;
    for r in 0..st.f_hz.len() {
        let Some(j) = (0..fit.f_hz.len())
            .min_by(|&a, &b| {
                (fit.f_hz[a] - st.f_hz[r])
                    .abs()
                    .total_cmp(&(fit.f_hz[b] - st.f_hz[r]).abs())
            })
            .filter(|&j| (fit.f_hz[j] - st.f_hz[r]).abs() < 0.01 * st.f_hz[r])
        else {
            return f64::INFINITY;
        };
        let turn = C64::from_polar(1.0, -TAU * st.f_hz[r] * true_tau);
        let (mut num, mut den) = (0.0, 0.0);
        for i in 0..n {
            for k in 0..m {
                let a = turn * (st.output_shape[r * n + i] * st.drive_shape[r * m + k]);
                num += (fit.residues[(j * n + i) * m + k] - a).norm_sqr();
                den += a.norm_sqr();
            }
        }
        worst = worst.max((num / den).sqrt());
    }
    worst
}

#[test]
fn the_fit_takes_the_known_delay_out_and_puts_it_back() {
    let s = delayed_setup();
    let tau = DELAY_SAMPLES / s.fs_hz;
    let told = residue_error(&s, tau);
    assert!(told < 0.02, "told the delay: {told:e}");
    // Not told it: a real residue cannot hold the turn, the check must see it.
    let untold = residue_error(&s, 0.0);
    eprintln!("residue error told the delay {told:.2e}, not told {untold:.2e}");
    assert!(untold > 0.10, "the check cannot fail: {untold:e}");
}

#[test]
fn the_law_removes_the_station_delay_less_the_transport() {
    // The identification a law is given has the transport delay out already:
    // the law fits with the station's delay less it.
    let mut s = delayed_setup();
    let tau = DELAY_SAMPLES / s.fs_hz;
    s.loop_delay.transport_delay_s = 0.004;
    let direct = fit_known_delay(&s.identification, BAND, tau).expect("fit");
    let law = |station: f64| {
        FollowedLaw::new(
            &s,
            FollowedParams {
                station_delay_s: station,
                ..Default::default()
            },
        )
        .expect("law")
        .model()
        .modes
        .clone()
    };
    assert_eq!(law(tau + 0.004), direct);
    // Told the station's delay as if it were the identification's, the law
    // takes out the wrong one.
    assert_ne!(law(tau), direct);
}

#[test]
fn the_law_is_refused_without_the_station_delay() {
    let s = delayed_setup();
    let e = FollowedLaw::new(&s, FollowedParams::default()).expect_err("refused");
    assert!(e.contains("station_delay_s"), "{e}");
    for bad in [-1e-3, f64::NAN, f64::INFINITY] {
        let p = FollowedParams {
            station_delay_s: bad,
            ..Default::default()
        };
        assert!(FollowedLaw::new(&s, p).is_err(), "{bad}");
    }
    // Set, it builds: the refusal is the delay's.
    let p = FollowedParams {
        station_delay_s: DELAY_SAMPLES / s.fs_hz,
        ..Default::default()
    };
    assert!(FollowedLaw::new(&s, p).is_ok());
}
