//! `followed_poles` and its modal fit: the delay they are told, the refusal
//! without it, and a fit that keeps each mode once whatever the rounding. Each
//! check is shown to fail on a case built to break it.
use hertzforge_mimo_sine_law_math::followed::{FollowedLaw, FollowedParams};
use hertzforge_mimo_sine_law_math::modal_fit::{
    cluster_orders, fit_known_delay, Clustering, OrderPoles,
};
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

/// A pole at `f_hz` with damping ratio `zeta`.
fn pole(f_hz: f64, zeta: f64) -> C64 {
    let w = TAU * f_hz / (1.0 - zeta * zeta).sqrt();
    C64::new(-zeta * w, TAU * f_hz)
}

/// Orders 24 down to 12, each holding `poles(order)` with one participation
/// vector for all.
fn orders_of(poles: impl Fn(usize) -> Vec<C64>) -> Vec<OrderPoles> {
    let l = [C64::new(1.0, 0.0), C64::new(0.5, 0.0)];
    (12..=24)
        .rev()
        .map(|p| {
            let poles = poles(p);
            let participation = poles.iter().flat_map(|_| l).collect();
            OrderPoles {
                order: p,
                poles,
                participation,
            }
        })
        .collect()
}

#[test]
fn a_mode_split_over_two_clusters_is_kept_once() {
    // A mode at 42 Hz at every order; at order 20 a spurious pole 0.5 % above
    // it comes first and takes the mode's place in its cluster, so the mode's
    // own pole there starts a second cluster 1e-6 above the first. Orders 19
    // and 18 lie on the first, 17 to 12 on the second: each holds 7 of the 13
    // orders, enough to be kept. The same mode is one pole.
    let orders = orders_of(|p| match p {
        20 => vec![pole(42.21, 0.03), pole(42.0 * (1.0 + 1e-6), 0.03)],
        18..=24 => vec![pole(42.0, 0.03)],
        _ => vec![pole(42.0 * (1.0 + 1e-6), 0.03)],
    });
    let (lam, _) = cluster_orders(&orders, 2, Clustering::default());
    let f: Vec<f64> = lam.iter().map(|l| l.im / TAU).collect();
    assert_eq!(f.len(), 1, "the mode kept {} times: {f:?}", f.len());
    assert!((f[0] - 42.0).abs() < 1e-4, "{f:?}");
}

#[test]
fn a_close_pair_stays_two_modes() {
    // The pair of the research fixture: 0.78 % apart (inside the clustering's
    // 1 %), the same damping and here the same participation. Half a
    // bandwidth apart, they are two modes, kept both.
    let orders = orders_of(|_| vec![pole(90.0, 0.008), pole(90.7, 0.008)]);
    let (lam, _) = cluster_orders(&orders, 2, Clustering::default());
    let f: Vec<f64> = lam.iter().map(|l| l.im / TAU).collect();
    assert_eq!(f.len(), 2, "{f:?}");
}

#[test]
fn the_fit_keeps_each_mode_once_whatever_the_rounding() {
    // The law file parity suite's wide structure (2 controls, 3 drives), its
    // identification exact, so the orders above the three modes' are made of
    // rounding. Changed in the last bits, as another platform's arithmetic
    // changes it, the fit keeps the same three modes, moved by rounding only.
    // Before each mode was kept once, this found a mode kept twice on a few
    // of every hundred.
    let shape = |k: usize, n: usize| -> Vec<f64> {
        (0..k * n)
            .map(|i| {
                let (r, c) = (i / n, i % n);
                (1.3 * r as f64 + 0.7 * c as f64 + 0.4).sin() + if r == c { 1.5 } else { 0.0 }
            })
            .collect()
    };
    let st = ModalStructure {
        f_hz: vec![42.0, 63.0, 91.0],
        zeta: vec![0.03, 0.02, 0.04],
        drive_shape: shape(3, 3),
        output_shape: shape(3, 2),
        drives: 3,
        controls: 2,
    };
    let s = st.setup((30.0, 120.0), false);
    let (band, tau) = ((30.0, 120.0), 1e-6);
    let base = fit_known_delay(&s.identification, band, tau).expect("fit");
    assert_eq!(base.f_hz.len(), 3, "{:?}", base.f_hz);
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut jitter = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    };
    let mut moved: f64 = 0.0;
    for eps in [1e-15, 1e-13] {
        for t in 0..50 {
            let mut id = s.identification.clone();
            for v in id.response.data_mut() {
                *v *= C64::new(1.0 + eps * jitter(), eps * jitter());
            }
            let fit = fit_known_delay(&id, band, tau).expect("fit");
            assert_eq!(
                fit.f_hz.len(),
                3,
                "changed by {eps:e} (trial {t}): {:?}",
                fit.f_hz
            );
            for (a, b) in fit.f_hz.iter().zip(&base.f_hz) {
                moved = moved.max((a - b).abs() / b);
            }
        }
    }
    eprintln!("100 changed identifications: 3 modes each, frequencies moved {moved:.2e}");
    assert!(moved < 1e-6, "{moved:e}");
}
