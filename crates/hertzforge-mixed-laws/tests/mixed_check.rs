//! The mixed check: the built-in pair passes all eight cases, each on the
//! station its own facts describe (the transport-delay case at the chassis's
//! 553 frames of lag), and a tone law past its stability bound fails.

use hertzforge_hal_sim::SimConfig;
use hertzforge_mixed::MixedLawFactories;
use hertzforge_mixed_laws::{mixed_check_station, run_mixed_check_case, MixedCheckCase};
use hertzforge_sine::control::CompressionPreset;
use hertzforge_sine::law::servo_blend::{ServoBlendFactory, ServoBlendParams};
use std::sync::Arc;

fn station(case: MixedCheckCase) -> Box<dyn hertzforge_hal::Backend> {
    let st = mixed_check_station(case);
    Box::new(
        SimConfig::new()
            .sample_rate_hz(st.fs_hz)
            .resonances(st.modes)
            .feedthrough(st.feedthrough)
            .noise_level(st.noise_rms_g)
            .transport_delay_samples(st.transport_delay_samples)
            .shared_clock(true)
            .seed(st.seed)
            .build(),
    )
}

#[test]
fn the_built_in_pair_passes_all_eight_cases() {
    let laws = MixedLawFactories::builtin();
    let mut failed = Vec::new();
    for case in MixedCheckCase::ALL {
        let r = run_mixed_check_case(case, &laws, station(case)).unwrap();
        eprintln!(
            "{} {}: {}",
            if r.passed { "PASS" } else { "FAIL" },
            r.name,
            r.detail
        );
        if !r.passed {
            failed.push(r.name);
        }
    }
    assert!(failed.is_empty(), "{failed:?}");
}

#[test]
fn a_tone_law_past_its_bound_fails_the_check() {
    let mut laws = MixedLawFactories::builtin();
    laws.tones = Arc::new(ServoBlendFactory {
        params: ServoBlendParams {
            compression: CompressionPreset::Custom {
                ki: 8.0,
                max_rate_db_per_s: 60.0,
            },
            ..ServoBlendParams::default()
        },
    });
    let case = MixedCheckCase::FixedToneOnFloor;
    let r = run_mixed_check_case(case, &laws, station(case)).unwrap();
    eprintln!("ringing law: {}", r.detail);
    assert!(!r.passed);
}
