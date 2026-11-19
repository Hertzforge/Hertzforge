//! The multi-drive shot: one record `n_d` drives wide, played on one clock, every
//! input row returned whole and in table order. Run on the simulator with a sized
//! plant that routes each drive to exactly one input, so where a drive's energy
//! lands says which row is which.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal_sim::{FrfPlant, MatrixStack, PlantModel, SimBackend, SimConfig, C64};
use hertzforge_module_abi::StationClass;
use hertzforge_shock_host::service::firer::FireControl;
use hertzforge_shock_host::{BackendPulseFirer, PulseFirer, ServiceError};

const FS: f64 = 8192.0;
const QUIET: usize = 64;

/// A table of `n` Control rows (indices 0..n) followed by `n` Drive rows.
fn table(n: usize) -> ChannelTable {
    let mut t = ChannelTable::new();
    for i in 0..n {
        let mut c = Channel::new(i as u32);
        c.role = ChannelRole::Control;
        t.channels.push(c);
    }
    for d in 0..n {
        let mut c = Channel::new((n + d) as u32);
        c.role = ChannelRole::Drive;
        c.units = Units::VoltageV;
        t.channels.push(c);
    }
    t
}

/// A flat, unit-gain plant where drive `d` reaches input `route[d]` and nothing else.
fn routed(route: &[usize]) -> SimBackend {
    let n = route.len();
    let lines = 257;
    let mut stack = MatrixStack::zeros(lines, n, n);
    for line in 0..lines {
        for (d, &i) in route.iter().enumerate() {
            stack.set(line, i, d, C64::new(1.0, 0.0));
        }
    }
    let mut frf = FrfPlant::new(stack, FS / 2.0 / (lines - 1) as f64);
    frf.block_frames = 32;
    SimConfig::new()
        .sample_rate_hz(FS)
        .noise_level(0.0)
        .shared_clock(true)
        .plant(PlantModel::Frf(frf))
        .build()
}

fn armed(n: usize, route: &[usize]) -> BackendPulseFirer<SimBackend> {
    let mut f = BackendPulseFirer::new(routed(route), table(n), StationClass::HertzforgeSim)
        .unwrap()
        .with_quiet_frames(QUIET);
    f.arm().unwrap();
    f
}

/// A half-sine of `len` frames and peak `amp` on drive `d` of `n`, silence elsewhere.
fn pulses(n: usize, len: usize, amps: &[f64]) -> Vec<f64> {
    let mut rec = vec![0.0; n * len];
    for k in 0..len {
        let s = (std::f64::consts::PI * k as f64 / (len - 1) as f64).sin();
        for d in 0..n {
            rec[k * n + d] = amps[d] * s;
        }
    }
    rec
}

fn peak(x: &[f64]) -> f64 {
    x.iter().fold(0.0f64, |m, v| m.max(v.abs()))
}

#[test]
fn mimo_shock_two_drives_return_every_row_whole_and_in_order() {
    // Drive 0 reaches input 1 and drive 1 reaches input 0.
    let mut f = armed(2, &[1, 0]);
    assert_eq!((f.n_drives(), f.n_inputs()), (2, 2));
    let len = 400;
    let rec = pulses(2, len, &[0.25, 0.75]);
    let shot = f.fire_rows(&rec, 2, &mut FireControl::none()).unwrap();
    assert_eq!(shot.frames, len + 2 * QUIET);
    assert_eq!(shot.channels, vec![0, 1]);
    assert_eq!(shot.rows.len(), 2);
    for row in &shot.rows {
        assert_eq!(row.len(), shot.frames);
    }
    assert!(
        (peak(&shot.rows[0]) - 0.75).abs() < 0.05,
        "{}",
        peak(&shot.rows[0])
    );
    assert!(
        (peak(&shot.rows[1]) - 0.25).abs() < 0.05,
        "{}",
        peak(&shot.rows[1])
    );
    assert_eq!(shot.row_of_channel(1).map(peak), Some(peak(&shot.rows[1])));
}

#[test]
fn mimo_shock_four_drives_put_one_drive_only_where_the_plant_puts_it() {
    let route = [2, 0, 3, 1];
    let mut f = armed(4, &route);
    let len = 300;
    for d in 0..4 {
        let mut amps = [0.0; 4];
        amps[d] = 0.5;
        let shot = f
            .fire_rows(&pulses(4, len, &amps), 4, &mut FireControl::none())
            .unwrap();
        assert_eq!(shot.rows.len(), 4);
        for (i, row) in shot.rows.iter().enumerate() {
            assert_eq!(row.len(), len + 2 * QUIET);
            if i == route[d] {
                assert!((peak(row) - 0.5).abs() < 0.05, "drive {d} input {i}");
            } else {
                assert!(
                    peak(row) < 1e-9,
                    "drive {d} leaked {} onto input {i}",
                    peak(row)
                );
            }
        }
    }
}

#[test]
fn mimo_shock_a_record_of_the_wrong_width_is_refused_before_anything_is_armed() {
    let mut f = armed(2, &[0, 1]);
    let rec = pulses(2, 100, &[0.1, 0.1]);
    // One drive wide on a two-drive table.
    match f.fire_rows(&rec, 1, &mut FireControl::none()) {
        Err(ServiceError::Fire(m)) => assert!(m.contains("2 enabled drive rows"), "{m}"),
        other => panic!("{other:?}"),
    }
    // Not a whole number of frames.
    match f.fire_rows(&rec[..199], 2, &mut FireControl::none()) {
        Err(ServiceError::Fire(m)) => assert!(m.contains("whole number"), "{m}"),
        other => panic!("{other:?}"),
    }
    // Neither refusal fired anything: the next shot plays normally.
    let shot = f.fire_rows(&rec, 2, &mut FireControl::none()).unwrap();
    assert_eq!(shot.frames, 100 + 2 * QUIET);
}
