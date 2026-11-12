//! The standard shock plants at the levels of a shock test.
//!
//! For each plant: the reference pulse (a 20 g, 11 ms half-sine with its pre- and
//! post-pulses, at the first control) is played by the drives of least energy that make
//! the linear plant play it, at -20, -12, -6 and 0 dB. Printed per level: the peak drive,
//! the peak at the first control, and for each mode the largest envelope over its
//! reference amplitude with the frequency and damping the mode reaches at that envelope.
//! Last, the calibration: the modal velocity that brings the dominant mode to its
//! reference amplitude at full level.
//!
//! Run with `cargo run --release -p hertzforge-hal-sim --example shock_plants`.

use hertzforge_hal_sim::plants::{
    reference_pulse, ShockPlant, SHOCK_CALIBRATION_RATE_HZ, SHOCK_CALIBRATION_SAMPLES,
};

fn main() {
    let fs = SHOCK_CALIBRATION_RATE_HZ;
    let n = SHOCK_CALIBRATION_SAMPLES;
    println!(
        "Reference pulse: 20 g, 11 ms half-sine with 2 g, 55 ms pre- and post-pulses, at the \
         first control. {fs} Hz, {n} samples."
    );
    for sp in ShockPlant::ALL {
        let plant = sp.plant();
        println!(
            "\n{} ({} controls x {} drives)",
            sp.name(),
            plant.n_inputs,
            plant.n_drives
        );
        println!("  small-signal modes:");
        for (k, m) in plant.modes.iter().enumerate() {
            println!(
                "    mode {k}: {:7.1} Hz  damping {:.4}  reference amplitude {:.3e}",
                m.f_hz, m.zeta, m.reference_amplitude
            );
        }
        for level_db in [-20.0, -12.0, -6.0, 0.0] {
            let target = reference_pulse(fs, n, level_db);
            let drives = plant
                .least_drive_for(0, &target, fs)
                .expect("a drive for the pulse");
            let peak_drive = drives
                .iter()
                .flat_map(|d| d.iter())
                .fold(0.0f64, |a, v| a.max(v.abs()));
            let sim = plant.simulate(fs, &drives).expect("runs");
            let peak_out = sim.outputs[0].iter().fold(0.0f64, |a, v| a.max(v.abs()));
            let at = plant.modes_at_envelope(&sim.peak_envelope);
            println!(
                "  {level_db:+5.0} dB: peak drive {peak_drive:.3} V, peak at control 0 {peak_out:.2} g"
            );
            for (k, ((r, (f, z)), m)) in sim
                .peak_ratio(&plant)
                .iter()
                .zip(&at)
                .zip(&plant.modes)
                .enumerate()
            {
                println!(
                    "    mode {k}: envelope {r:6.3} of reference  {f:7.1} Hz ({:+6.2} %)  \
                     damping {z:.4} (x{:.2})",
                    100.0 * (f / m.f_hz - 1.0),
                    z / m.zeta
                );
            }
        }
        // The calibration: the linear plant at full level.
        let target = reference_pulse(fs, n, 0.0);
        let lin = plant.linear();
        let drives = lin.least_drive_for(0, &target, fs).expect("a drive");
        let sim = lin.simulate(fs, &drives).expect("runs");
        let (k, v) = sim
            .peak_envelope
            .iter()
            .zip(&plant.modes)
            .map(|(a, m)| a * std::f64::consts::TAU * m.f_hz)
            .enumerate()
            .fold((0, 0.0f64), |b, (k, v)| if v > b.1 { (k, v) } else { b });
        println!("  calibration: dominant mode {k}, modal velocity at full level {v:.6e}");
    }
}
