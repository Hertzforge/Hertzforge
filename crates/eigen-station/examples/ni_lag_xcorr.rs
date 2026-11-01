//! Measure the ACTUAL drive->response lag through `PlantLoop` on the real chassis
//! by cross-correlation, and report what the loop DERIVES for comparison.
//!
//! Written 2026-08-01 while chasing a remote Random defect: sysID alignment by the
//! derived lag (lookahead + group delay) left band coherence at 0.0425, so the
//! derivation is wrong somewhere and the argument stops here - the xcorr peak is
//! the truth.

use eigen_random::service::engine::PlantLoop;
use eigen_random::LoopConfig;
use eigen_station::{
    try_build_station_backend, BackendKind, NiSelection, SimPlantSpec, StationSpec,
};

fn main() {
    let mut cfg = LoopConfig::demo_default();
    cfg.band_lo_hz = 20.0;
    cfg.band_hi_hz = 2000.0;

    let spec = StationSpec {
        sim: SimPlantSpec::default(),
        ni: NiSelection {
            derive_inputs: 1,
            derive_outputs: 1,
            iepe: false,
            ..Default::default()
        },
        ni_block_frames: eigen_hal::default_block_frames(cfg.fs_hz),
    };
    let station = try_build_station_backend(BackendKind::Ni, &spec).expect("station");
    println!("station: {}", station.provenance.log_line());

    let table = probe_table();
    let mut lp = PlantLoop::open(station.backend, station.station, &mut cfg, table).expect("open");
    println!("derived pipeline_lag_frames: {}", lp.pipeline_lag_frames());
    println!(
        "fs {} nfft {} segment {} frames",
        cfg.fs_hz,
        cfg.nfft,
        cfg.window_len()
    );

    // A few segments of white noise; xcorr the LAST pair so the pipeline is warm.
    let n = cfg.window_len();
    let mut seed = 0x1234_5678_u64;
    let mut noise = || {
        // xorshift - deterministic, no external crates.
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed as f64 / u64::MAX as f64) - 0.5
    };
    let mut last_drive: Vec<f64> = Vec::new();
    let mut last_resp: Vec<f64> = Vec::new();
    for s in 0..4 {
        let drive: Vec<f64> = (0..n).map(|_| noise() * 0.1).collect();
        let seg = lp.run_segment(Vec::new(), drive.clone());
        if let Some(f) = &seg.faulted {
            eprintln!("segment {s} faulted: {f}");
            std::process::exit(1);
        }
        let n_ch = seg.n_ch.max(1);
        let resp: Vec<f64> = seg.interleaved.chunks(n_ch).map(|fr| fr[0]).collect();
        println!(
            "  seg {s}: drive {} frames rms {:.4}, resp {} frames rms {:.4}",
            drive.len(),
            rms(&drive),
            resp.len(),
            rms(&resp)
        );
        last_drive = drive;
        last_resp = resp;
    }

    // Cross-correlate: response vs drive over lags 0..=MAX_LAG.
    const MAX_LAG: usize = 6000;
    let len = last_drive.len().min(last_resp.len());
    let max_lag = MAX_LAG.min(len / 2);
    let mut best = (0usize, f64::MIN);
    let mut top: Vec<(usize, f64)> = Vec::new();
    for lag in 0..max_lag {
        let m = len - lag;
        let mut acc = 0.0;
        for i in 0..m {
            acc += last_drive[i] * last_resp[i + lag];
        }
        let r = acc / m as f64;
        if r > best.1 {
            best = (lag, r);
        }
        top.push((lag, r));
    }
    top.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap());
    println!("\nxcorr peak: lag {} frames (r {:.6})", best.0, best.1);
    println!("top |r| lags: {:?}", &top[..8.min(top.len())]);
    println!("derived lag was: {}", lp.pipeline_lag_frames());
}

fn rms(v: &[f64]) -> f64 {
    (v.iter().map(|x| x * x).sum::<f64>() / v.len().max(1) as f64).sqrt()
}

fn probe_table() -> eigen_channel_table::ChannelTable {
    use eigen_channel_table::{Channel, ChannelRole, ChannelTable, Units};
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 10.0;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(drv);
    t
}
