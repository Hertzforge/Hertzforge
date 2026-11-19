//! Gate fixtures for `shock::compensate_pulse`.
//!
//! The load-bearing gate: a 20 g / 11 ms half-sine, compensated double-sided,
//! must leave residual velocity `< 1e-6 in/s` and residual displacement
//! `< 1e-8 in` when re-integrated through `shock::pulse_kinematics` - the PURE
//! (no-high-pass) integrator, because a high-pass would mask exactly the
//! residuals being verified. Plus the OEM-shape sanity comparison: the decoded
//! OEM compensation for the same pulse used a ~-24% trough with +12% shoulders;
//! the solved shapes here need not match byte-for-byte (the device computes its
//! OWN compensation on Path A) but must land in the same order of magnitude.

use hertzforge_dsp::integrate::G_TO_IN_S2;
use hertzforge_dsp::shock::{
    compensate_pulse, compensation_shape, pulse, pulse_kinematics, CompensatedPulse,
    CompensationConfig, CompensationShape,
};

/// The decoded Shock config sample rate ("SR 5120").
const FS_HZ: f64 = 5120.0;

/// Residual gates: velocity < 1e-6 in/s, displacement < 1e-8 in.
const V_GATE_IPS: f64 = 1e-6;
const D_GATE_IN: f64 = 1e-8;

fn oem_pulse() -> Vec<f64> {
    pulse::half_sine(20.0, 11.0, FS_HZ)
}

/// The solved shoulder amplitude as a fraction of the main peak: the most
/// negative sample in the PRE region over the main-pulse peak.
fn pre_shoulder_frac(comp: &CompensatedPulse, main_peak_g: f64) -> f64 {
    comp.waveform_g[..comp.pre_len]
        .iter()
        .fold(0.0_f64, |m, &v| m.min(v))
        / main_peak_g
}

// ---------------------------------------------------------------------------
// THE gate: 20 g / 11 ms half-sine, default double-sided compensation.
// ---------------------------------------------------------------------------

#[test]
fn gate_20g_11ms_half_sine_double_sided_residuals() {
    let main = oem_pulse();
    let cfg = CompensationConfig::default();
    let comp = compensate_pulse(&main, FS_HZ, &cfg);

    // Structure: [pre | main | post], symmetric geometry mirrored from pre.
    assert_eq!(comp.main_len, main.len());
    assert_eq!(comp.pre_len, comp.post_len, "symmetric default mirrors pre");
    assert_eq!(
        comp.waveform_g.len(),
        comp.pre_len + comp.main_len + comp.post_len
    );
    // The main pulse rides unmodified in the middle of the composite.
    assert_eq!(
        &comp.waveform_g[comp.pre_len..comp.pre_len + comp.main_len],
        &main[..]
    );

    // Residuals verified through the pure integrator: the reported fields
    // and an independent re-integration must both clear the gates.
    let kin = pulse_kinematics(&comp.waveform_g, FS_HZ);
    for (label, v, d) in [
        (
            "reported",
            comp.residual_velocity_ips,
            comp.residual_displacement_in,
        ),
        (
            "reintegrated",
            kin.residual_velocity_ips,
            kin.residual_disp_in,
        ),
    ] {
        assert!(
            v.abs() < V_GATE_IPS,
            "{label} residual velocity {v} in/s >= gate {V_GATE_IPS}"
        );
        assert!(
            d.abs() < D_GATE_IN,
            "{label} residual displacement {d} in >= gate {D_GATE_IN}"
        );
    }

    // Peak displacement is reported, finite, and physically nonzero (the table
    // moves and returns to center).
    assert!(comp.peak_displacement_in.is_finite());
    assert!(comp.peak_displacement_in > 0.0);
    assert!(
        (comp.peak_displacement_in - kin.peak_disp_in.abs().max(kin.min_disp_in.abs())).abs()
            < 1e-12
    );

    // Contrast: the UNCOMPENSATED half-sine leaves dV = 2*A*T/pi (in g*s) - the
    // physical residual compensation exists to null. ~53 in/s for 20 g at the
    // grid-realized duration T = (N-1)/fs (the generator spans N samples with
    // both endpoints on the grid, so the realized lobe is (N-1)/fs ~ 10.74 ms).
    let unc = pulse_kinematics(&main, FS_HZ);
    let t_realized = (main.len() - 1) as f64 / FS_HZ;
    let analytic_dv = G_TO_IN_S2 * 2.0 * 20.0 * t_realized / std::f64::consts::PI;
    assert!(
        (unc.residual_velocity_ips - analytic_dv).abs() / analytic_dv < 0.02,
        "uncompensated dV {} vs analytic {analytic_dv}",
        unc.residual_velocity_ips
    );
    assert!(unc.residual_velocity_ips > 50.0, "sanity: ~54 in/s");
}

// ---------------------------------------------------------------------------
// OEM shape sanity comparison (-24% trough / +12% shoulders).
// ---------------------------------------------------------------------------

#[test]
fn oem_shape_order_of_magnitude_comparison() {
    let main = oem_pulse();

    // Half-sine shoulders at 1.5x duration: continuous-time closed form solves
    // to a = -A/(2k) = -33.3% of the peak. Same order as the OEM -24% trough
    // (ratio ~1.4x); the OEM's own +12%/-24% multi-lobe optimum is the DEVICE's
    // shape and is explicitly not byte-matched.
    let hs = compensate_pulse(&main, FS_HZ, &CompensationConfig::default());
    let hs_frac = pre_shoulder_frac(&hs, 20.0);
    assert!(
        (-0.45..=-0.22).contains(&hs_frac),
        "half-sine shoulder fraction {hs_frac} not ~-0.33 (closed form -1/(2*1.5))"
    );

    // Rectangular shoulders at the same duration: a = -A/(pi*k) = -21.2%,
    // essentially the OEM trough magnitude (-24%).
    let rect_cfg = CompensationConfig {
        shape: CompensationShape::Rectangular,
        ..CompensationConfig::default()
    };
    let rect = compensate_pulse(&main, FS_HZ, &rect_cfg);
    let rect_frac = pre_shoulder_frac(&rect, 20.0);
    assert!(
        (-0.30..=-0.15).contains(&rect_frac),
        "rectangular shoulder fraction {rect_frac} not ~-0.21 (closed form -1/(pi*1.5))"
    );

    // Both solutions still clear the residual gates.
    for comp in [&hs, &rect] {
        assert!(comp.residual_velocity_ips.abs() < V_GATE_IPS);
        assert!(comp.residual_displacement_in.abs() < D_GATE_IN);
    }
}

// ---------------------------------------------------------------------------
// Every shape x asymmetric geometry x asymmetric MAIN pulse nulls both
// residuals (the 2x2 solve is exact for the sampled geometry).
// ---------------------------------------------------------------------------

#[test]
fn all_shapes_null_residuals_even_for_asymmetric_mains() {
    // Terminal-peak sawtooth: an asymmetric main, so equal-amplitude symmetric
    // shoulders would NOT null displacement; the independent 2x2 solve must.
    let main = pulse::terminal_peak_sawtooth(30.0, 6.0, FS_HZ);
    for shape in [
        CompensationShape::HalfSine,
        CompensationShape::Rectangular,
        CompensationShape::RoundedRectangular,
    ] {
        for (pre, post, symmetric) in [(1.5, 1.5, true), (1.0, 2.5, false), (2.0, 0.75, false)] {
            let cfg = CompensationConfig {
                shape,
                pre_frac: pre,
                post_frac: post,
                symmetric,
            };
            let comp = compensate_pulse(&main, FS_HZ, &cfg);
            assert!(
                comp.residual_velocity_ips.abs() < V_GATE_IPS,
                "{shape:?} pre {pre} post {post} sym {symmetric}: residual v {}",
                comp.residual_velocity_ips
            );
            assert!(
                comp.residual_displacement_in.abs() < D_GATE_IN,
                "{shape:?} pre {pre} post {post} sym {symmetric}: residual d {}",
                comp.residual_displacement_in
            );
            if symmetric {
                assert_eq!(comp.pre_len, comp.post_len);
            } else {
                let m = main.len() as f64;
                assert_eq!(comp.pre_len, (m * pre).round() as usize);
                assert_eq!(comp.post_len, (m * post).round() as usize);
            }
        }
    }
}

#[test]
fn symmetric_flag_ignores_post_frac() {
    let main = oem_pulse();
    let cfg = CompensationConfig {
        shape: CompensationShape::HalfSine,
        pre_frac: 2.0,
        post_frac: 0.5, // ignored: symmetric mirrors the pre geometry
        symmetric: true,
    };
    let comp = compensate_pulse(&main, FS_HZ, &cfg);
    assert_eq!(comp.pre_len, (main.len() as f64 * 2.0).round() as usize);
    assert_eq!(comp.post_len, comp.pre_len);
}

// ---------------------------------------------------------------------------
// The compensation-shape helper.
// ---------------------------------------------------------------------------

#[test]
fn compensation_shape_geometry() {
    // Half-sine: zero endpoints, unit peak.
    let hs = compensation_shape(CompensationShape::HalfSine, 101);
    assert_eq!(hs.len(), 101);
    assert!(hs[0].abs() < 1e-12 && hs[100].abs() < 1e-12);
    assert!((hs[50] - 1.0).abs() < 1e-12);

    // Rectangular: all ones.
    let rect = compensation_shape(CompensationShape::Rectangular, 8);
    assert!(rect.iter().all(|&v| v == 1.0));

    // Rounded rectangular: zero endpoints, flat unit middle, monotone tapers.
    let rr = compensation_shape(CompensationShape::RoundedRectangular, 100);
    assert!(rr[0].abs() < 1e-12);
    assert!(rr[99].abs() < 1e-12);
    assert!((rr[50] - 1.0).abs() < 1e-12, "flat middle is 1.0");
    for w in rr[..25].windows(2) {
        assert!(w[1] >= w[0] - 1e-12, "ramp-up monotone");
    }

    // Degenerate lengths.
    assert!(compensation_shape(CompensationShape::HalfSine, 0).is_empty());
    assert_eq!(
        compensation_shape(CompensationShape::RoundedRectangular, 1),
        vec![1.0]
    );
}

// ---------------------------------------------------------------------------
// Edge cases and contract panics.
// ---------------------------------------------------------------------------

#[test]
fn empty_main_or_bad_fs_returns_zeroed_result() {
    let z = compensate_pulse(&[], FS_HZ, &CompensationConfig::default());
    assert!(z.waveform_g.is_empty());
    assert_eq!(z.pre_len + z.main_len + z.post_len, 0);
    assert_eq!(z.residual_velocity_ips, 0.0);

    let main = oem_pulse();
    let z2 = compensate_pulse(&main, 0.0, &CompensationConfig::default());
    assert!(z2.waveform_g.is_empty());
}

#[test]
#[should_panic(expected = "pre_frac")]
fn zero_pre_frac_panics() {
    let cfg = CompensationConfig {
        pre_frac: 0.0,
        ..CompensationConfig::default()
    };
    let _ = compensate_pulse(&oem_pulse(), FS_HZ, &cfg);
}

#[test]
#[should_panic(expected = "post_frac")]
fn asymmetric_zero_post_frac_panics() {
    let cfg = CompensationConfig {
        post_frac: 0.0,
        symmetric: false,
        ..CompensationConfig::default()
    };
    let _ = compensate_pulse(&oem_pulse(), FS_HZ, &cfg);
}
