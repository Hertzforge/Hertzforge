//! `paired`'s first drive is the drive its own loop would settle to on the
//! identification: on a structure whose pairs are coupled, its reference over
//! each pair's own path alone leaves the controls off by the cross paths.

use hertzforge_mimo_sine_law_math::law::{Hold, Inputs, Outputs, SineLaw};
use hertzforge_mimo_sine_law_math::paired::{PairedLaw, PairedParams, PAIRED_VALUES};
use hertzforge_mimo_sine_law_math::reference_structure::ModalStructure;
use hertzforge_mimo_sine_law_math::C64;

#[test]
fn mimo_sine_paired_first_drive_is_settled_on_the_identification() {
    let s = ModalStructure {
        f_hz: vec![4.0, 4.5, 60.0, 85.0],
        zeta: vec![0.05, 0.05, 0.02, 0.03],
        drive_shape: vec![1.0, 0.15, 0.15, 1.0, 0.8, 0.15, 0.15, 0.7],
        output_shape: vec![1.0, 0.15, 0.15, 1.0, 0.6, 0.15, 0.15, 0.5],
        drives: 2,
        controls: 2,
    }
    .setup((30.0, 120.0), false);
    let f = 70.0;
    let mut h = vec![C64::new(0.0, 0.0); 4];
    s.response_at(f, &mut h);
    let r = vec![C64::new(1.0, 0.0); 2];
    let zero = vec![C64::new(0.0, 0.0); 2];
    let x = Inputs {
        index: 0,
        dt_s: 0.0,
        f_hz: f,
        sweep_hz_s: 0.0,
        sweep_oct_min: 0.0,
        level: 1.0,
        reference: &r,
        controls: &zero,
        limits: &[],
        limit_bounds: &[],
        drive_in_force: &zero,
        hold: Hold::Engage,
        notch_scale: 1.0,
        settle_open: true,
    };
    let mut out = Outputs::new(2, PAIRED_VALUES.len());
    PairedLaw::new(&s, PairedParams::diagonal(2, 2))
        .unwrap()
        .engage(&x, &mut out)
        .unwrap();
    let read = |d: &[C64]| -> Vec<f64> {
        (0..2)
            .map(|i| 20.0 * (h[i * 2] * d[0] + h[i * 2 + 1] * d[1]).norm().log10())
            .collect()
    };
    // The plain start: each pair's reference over its own path.
    let plain: Vec<C64> = (0..2)
        .map(|i| C64::new(1.0 / h[i * 3].norm(), 0.0))
        .collect();
    let (got, start) = (read(&out.drive), read(&plain));
    println!("first drive reads {got:.3?} dB; the plain start {start:.3?} dB");
    assert!(start.iter().any(|e| e.abs() > 1.0), "{start:?}");
    // Within half a dB: with each drive's phase left where it starts, as the
    // law leaves it, this is as near as the pairs' own rule settles here.
    assert!(got.iter().all(|e| e.abs() < 0.5), "{got:?}");
}
