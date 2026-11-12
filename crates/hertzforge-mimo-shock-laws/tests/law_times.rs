//! The time a built-in law takes, natively and as its law file, to be built
//! and seeded (the seed fits the modal model when the model is tracked) and to
//! answer one pulse, at 16384 points for 4 x 4 and 16 x 8. A measurement, run
//! on demand in release: `cargo test --release -p hertzforge-mimo-shock-laws
//! --features reference-plant --test law_times -- --ignored --nocapture`.

#![cfg(feature = "reference-plant")]

use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_law_sdk::shock_mimo::math::reference_plant::{
    reference_shock, srs_grid_hz, FS_HZ, RECORD_LEN,
};
use hertzforge_law_sdk::shock_mimo::math::research_plant::LevelPlant;
use hertzforge_law_sdk::shock_mimo::math::{level_scale, srs};
use hertzforge_mimo_shock::{
    ControlVerdict, MimoShockLawFactory, PlantStack, PulseInputs, Setup, SrsTarget, Target, Timing,
    WaveformTarget,
};
use hertzforge_mimo_shock_laws::{load_law_file, resolve_builtin_with, WasmMimoShockLawFactory};
use std::time::{Duration, Instant};

fn setup(plant: &LevelPlant, srs_target: bool) -> Setup {
    let frf = plant.frf(0.1, FS_HZ, RECORD_LEN);
    let (lines, nc, nd) = frf.dims();
    let r = reference_shock(FS_HZ, RECORD_LEN);
    let rows: Vec<Vec<f64>> = (0..nc)
        .map(|c| r.iter().map(|v| if c % 2 == 1 { -v } else { *v }).collect())
        .collect();
    let waveforms: Vec<f64> = rows.iter().flatten().copied().collect();
    let target = if srs_target {
        let grid = srs_grid_hz();
        let points = srs::srs_rows(&rows, FS_HZ, &grid, 10.0, 1.0);
        Target::Srs(SrsTarget {
            natural_hz: grid,
            q: 10.0,
            points: points.into_iter().flatten().collect(),
            tol_lo_db: -1.5,
            tol_hi_db: 3.0,
            waveforms,
            components: vec![Vec::new(); nc],
        })
    } else {
        Target::Waveform(WaveformTarget { records: waveforms })
    };
    Setup {
        fs_hz: FS_HZ,
        record_len: RECORD_LEN,
        n_drives: nd,
        n_controls: nc,
        band_lo_hz: 80.0,
        band_hi_hz: 2400.0,
        target,
        timing: Timing::Free { window_s: 0.002 },
        plant: PlantStack {
            frf,
            coherence: vec![1.0; nc * lines],
            trusted: vec![true; lines],
        },
        drive_ceiling_v: vec![1.0e3; nd],
        ladder_db: vec![-12.0, -9.0, -6.0, -3.0, 0.0],
        identification_level_db: Some(-20.0),
    }
}

fn inputs(setup: &Setup, plant: &LevelPlant, drives: &[f64], level_db: f64) -> PulseInputs {
    let n = setup.record_len;
    let rows: Vec<Vec<f64>> = drives.chunks(n).map(<[f64]>::to_vec).collect();
    let y = plant.respond(&rows, level_scale(level_db), FS_HZ).unwrap();
    let srs = match &setup.target {
        Target::Srs(t) => Some(srs::srs_rows(&y, setup.fs_hz, &t.natural_hz, t.q, 1.0).concat()),
        Target::Waveform(_) => None,
    };
    PulseInputs {
        pulse_index: 1,
        level_db,
        next_level_db: level_db + 3.0,
        drives: drives.to_vec(),
        responses: y.concat(),
        srs,
        verdicts: vec![
            ControlVerdict {
                passed: false,
                error_db: 0.0,
                delay_s: 0.0,
                peak_ratio: 1.0,
            };
            setup.n_controls
        ],
        limited: false,
    }
}

/// `(build and seed, one iteration)`, seconds.
fn time(f: &dyn MimoShockLawFactory, setup: &Setup, plant: &LevelPlant) -> (f64, f64) {
    let t0 = Instant::now();
    let mut law = f.build(setup).expect("built");
    let seed = law.seed(-12.0).expect("seeded");
    let t_seed = t0.elapsed().as_secs_f64();
    let i = inputs(setup, plant, &seed.drives, -12.0);
    let t1 = Instant::now();
    law.iterate(&i).expect("answered");
    (t_seed, t1.elapsed().as_secs_f64())
}

fn law_file(name: &str) -> std::path::PathBuf {
    let file = format!("{name}.wasm");
    let exe = std::env::current_exe().unwrap();
    exe.ancestors()
        .skip(1)
        .take(4)
        .map(|d| d.join("wasm32-wasip2").join("release").join(&file))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{file} is not built; run scripts/build_laws.sh"))
}

#[test]
#[ignore = "a measurement: run on demand in release"]
fn mimo_shock_x_law_call_times() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let plants = [
        ("4 x 4", LevelPlant::four_by_four_damped(0.08, 0.0, 0.02)),
        ("16 x 8", LevelPlant::sixteen_by_eight(0.08, 0.0, 11)),
    ];
    let cases = [
        ("shape", "frozen"),
        ("shape", "track, clamp"),
        ("shape", "track_secant, clamp"),
        ("inverse", "frozen"),
        ("inverse", "track"),
        ("inverse", "track_secant, step"),
    ];
    println!(
        "| plant | law | model | native seed s | native pulse s | file seed s | file pulse s |"
    );
    println!("|---|---|---|---|---|---|---|");
    for (pname, plant) in &plants {
        for (law, label) in cases {
            let model = label.split(',').next().unwrap().trim();
            let mut params = vec![Param {
                key: "model".into(),
                value: ParamValue::Choice(model.into()),
            }];
            if label.contains("clamp") {
                params.push(Param {
                    key: "gain_clamp_db".into(),
                    value: ParamValue::Number(12.0),
                });
            }
            if label.contains("step") {
                params.push(Param {
                    key: "predicted_step".into(),
                    value: ParamValue::Toggle(true),
                });
            }
            let s = setup(plant, law == "shape");
            let native = resolve_builtin_with(law, &params).unwrap();
            let (ns, ni) = time(native.as_ref(), &s, plant);
            let loaded = load_law_file(
                &host,
                &law_file(&format!("hertzforge_law_mimo_shock_{law}")),
            )
            .unwrap();
            let file = WasmMimoShockLawFactory::new(loaded, params, Duration::from_secs(600));
            let (fs, fi) = time(&file, &s, plant);
            println!("| {pname} | {law} | {label} | {ns:.3} | {ni:.3} | {fs:.3} | {fi:.3} |");
        }
    }
}
