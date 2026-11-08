//! What the Sine law tests share: the simulated station for each check case,
//! built from the check's own station facts, and where the built law files
//! are.

#![allow(dead_code)]

use hertzforge_hal::Backend;
use hertzforge_hal_sim::plant::{FrfPlant, PlantModel};
use hertzforge_hal_sim::SimConfig;
use hertzforge_sdm::{MatrixStack, C64};
use hertzforge_sine_laws::{sine_check_station, SineCheckCase, SineCheckStation, SINE_CHECK_FS_HZ};
use std::path::PathBuf;

/// The simulated station `case` runs on, from [`sine_check_station`].
pub fn station_for(case: SineCheckCase) -> Box<dyn Backend> {
    let spec = sine_check_station(case, SINE_CHECK_FS_HZ);
    let cfg = SimConfig::new()
        .sample_rate_hz(SINE_CHECK_FS_HZ)
        .shared_clock(true)
        .max_output_channels(1)
        .seed(0x51E5_5EED);
    let cfg = match spec.station {
        SineCheckStation::Resonators { modes, feedthrough } => {
            cfg.resonances(modes).feedthrough(feedthrough)
        }
        SineCheckStation::Frf {
            df_hz,
            lines,
            block_frames,
        } => {
            let n_in = lines.len();
            let n_lines = lines[0].len();
            let mut stack = MatrixStack::zeros(n_lines, n_in, 1);
            for (i, ch) in lines.iter().enumerate() {
                for (k, &(re, im)) in ch.iter().enumerate() {
                    stack.set(k, i, 0, C64::new(re, im));
                }
            }
            cfg.plant(PlantModel::Frf(FrfPlant {
                block_frames,
                ..FrfPlant::new(stack, df_hz)
            }))
        }
    };
    // The lag goes on last: it is set on whichever plant the station plays.
    Box::new(
        cfg.noise_level(spec.noise_rms_g)
            .transport_delay_samples(spec.transport_delay_samples)
            .build(),
    )
}

/// A built law file, `<target>/wasm32-wasip2/release/<name>.wasm`.
pub fn law_file(name: &str) -> PathBuf {
    let file = format!("{name}.wasm");
    let exe = std::env::current_exe().expect("the test binary has a path");
    for dir in exe.ancestors().skip(1).take(4) {
        let p = dir.join("wasm32-wasip2").join("release").join(&file);
        if p.is_file() {
            return p;
        }
    }
    panic!("the law file {file} is not built. Run `bash scripts/build_laws.sh`.");
}
