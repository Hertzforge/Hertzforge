//! The FRF plant: what it streams is the convolution of the impulse responses it reports,
//! and what it reports is the stack it was given wherever the approximation is exact.

mod common;

use common::{cdiv, mag, Rng};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal::{AcquisitionConfig, Backend};
use hertzforge_hal_sim::plants::StandardPlant;
use hertzforge_hal_sim::{FrfPlant, ModalPlant, Mode, PlantModel, SimConfig, C64};
use std::f64::consts::TAU;

fn table(n_in: usize, n_d: usize) -> ChannelTable {
    let mut t = ChannelTable::new();
    for k in 0..n_in + n_d {
        let mut c = Channel::new(k as u32 + 1);
        c.role = if k < n_in {
            ChannelRole::Control
        } else {
            ChannelRole::Drive
        };
        c.range_max = 1e9;
        t.channels.push(c);
    }
    t
}

fn small_modal() -> PlantModel {
    PlantModel::Modal(ModalPlant::new(
        2,
        2,
        vec![
            Mode::new(90.0, 6.0, vec![1.0, 0.5], vec![0.8, -0.4]),
            Mode::new(300.0, 10.0, vec![-0.3, 1.0], vec![0.6, 1.0]),
        ],
    ))
}

/// Streaming through the backend equals a direct convolution with the reported impulse
/// responses, delayed by the block latency and the transport delay, here with the
/// impulse response truncated and its tail tapered.
#[test]
fn the_frf_plant_streams_the_convolution_of_its_reported_impulse_responses() {
    let fs = 2048.0;
    let df = 8.0; // FIR grid 256 points
    let stack = small_modal().analytic_frf(129, df, fs);
    let plant = FrfPlant {
        max_taps: 128, // truncated: the last 32 taps are tapered
        block_frames: 32,
        transport_delay_samples: 2,
        ..FrfPlant::new(stack, df)
    };
    let taps = plant.impulse_responses(fs).unwrap();
    assert_eq!((taps.grid, taps.taps, taps.block), (256, 128, 32));
    let model = PlantModel::Frf(plant);
    let mut b = SimConfig::new()
        .sample_rate_hz(fs)
        .plant(model)
        .noise_level(0.0)
        .shared_clock(true)
        .build();
    b.open().unwrap();
    b.configure(&table(2, 2), &AcquisitionConfig::new(fs, 64))
        .unwrap();
    b.arm().unwrap();
    b.start().unwrap();

    let n = 3000;
    let mut rng = Rng::new(11);
    let x: Vec<[f32; 2]> = (0..n)
        .map(|_| [(rng.unit() - 0.5) as f32, (rng.unit() - 0.5) as f32])
        .collect();
    let mut y = vec![[0.0f32; 2]; n];
    let mut buf = [0.0f32; 2];
    for (xf, yf) in x.iter().zip(y.iter_mut()) {
        b.write_drive(xf).unwrap();
        b.read_input(&mut buf).unwrap();
        *yf = buf;
    }
    let lag = 32 + 2;
    let (mut worst, mut scale) = (0.0f64, 0.0f64);
    for (t, yt) in y.iter().enumerate() {
        for (i, &got) in yt.iter().enumerate() {
            let want: f64 = (0..2usize)
                .map(|j| {
                    taps.element(i, j)
                        .iter()
                        .enumerate()
                        .filter(|&(k, _)| t >= lag + k)
                        .map(|(k, &h)| h * x[t - lag - k][j] as f64)
                        .sum::<f64>()
                })
                .sum();
            worst = worst.max((got as f64 - want).abs());
            scale = scale.max(want.abs());
        }
    }
    assert!(scale > 0.1, "the plant produced almost nothing ({scale})");
    assert!(
        worst < 1e-5 * scale,
        "streamed output differs from the direct convolution by {worst} (scale {scale})"
    );
}

/// With `fs / df` a power of two and no truncation, the FIR the plant plays has exactly
/// the stack's response on the stack's lines (behind the block latency, which the exact
/// FRF reports).
#[test]
fn an_untruncated_frf_plant_plays_its_stack_exactly_on_the_stack_lines() {
    let fs = 8192.0;
    let df = 2.0;
    let lines = 2049;
    let stack = StandardPlant::Modal4x4
        .build()
        .model
        .analytic_frf(lines, df, fs);
    let plant = FrfPlant::new(stack.clone(), df);
    let block = plant.block_frames;
    let played = PlantModel::Frf(plant).analytic_frf(lines, df, fs);
    let peak = stack.data().iter().map(|&c| mag(c)).fold(0.0, f64::max);
    let mut worst = 0.0f64;
    for k in 0..lines {
        let f = k as f64 * df;
        let ph = -TAU * f * block as f64 / fs;
        let latency = C64::new(ph.cos(), ph.sin());
        for i in 0..4 {
            for j in 0..4 {
                let want = stack.get(k, i, j) * latency;
                worst = worst.max(mag(played.get(k, i, j) - want) / peak);
            }
        }
    }
    assert!(
        worst < 1e-9,
        "played FRF differs from the stack by {worst} of its peak"
    );
}

/// The exact FRF by one FFT per element (an FFT grid) and by the direct sum (any
/// frequencies) agree.
#[test]
fn the_frf_plants_two_exact_frf_paths_agree() {
    let fs = 2048.0;
    let df = 8.0;
    let model = PlantModel::Frf(FrfPlant {
        max_taps: 128,
        block_frames: 32,
        transport_delay_samples: 3,
        ..FrfPlant::new(small_modal().analytic_frf(129, df, fs), df)
    });
    let fast = model.analytic_frf(129, df, fs);
    let freqs: Vec<f64> = (0..129).map(|k| k as f64 * df).collect();
    let direct = model.analytic_frf_at(&freqs, fs);
    let worst = fast
        .data()
        .iter()
        .zip(direct.data())
        .map(|(&a, &b)| mag(a - b))
        .fold(0.0, f64::max);
    assert!(worst < 1e-10, "FFT and direct paths differ by {worst}");
    // And truncation is visible in what is reported: the played FRF is not the stack.
    let stack = small_modal().analytic_frf(129, df, fs);
    let near_peak = cdiv(fast.get(11, 0, 0), stack.get(11, 0, 0)); // 88 Hz
    assert!(
        (mag(near_peak) - 1.0).abs() > 1e-4,
        "a truncated, tapered FIR reported as if it were the stack"
    );
}

/// A stack the plant cannot play is refused at configure, with the reason.
#[test]
fn an_unplayable_stack_is_refused_at_configure() {
    let stack = small_modal().analytic_frf(129, 8.0, 2048.0);
    let bad = PlantModel::Frf(FrfPlant {
        max_taps: 100,
        ..FrfPlant::new(stack, 8.0)
    });
    let mut b = SimConfig::new().sample_rate_hz(2048.0).plant(bad).build();
    b.open().unwrap();
    let err = b
        .configure(&table(2, 2), &AcquisitionConfig::new(2048.0, 64))
        .unwrap_err();
    assert!(format!("{err:?}").contains("max_taps 100"), "{err:?}");
}
