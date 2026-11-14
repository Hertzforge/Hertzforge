//! What a TWR capture leaves in its run folder: the channel table the run
//! plays with, and the reference written once beside the stream rather than
//! inside the sidecar that is rewritten through the run.

mod common;

use common::{config, field_like_reference, player, sim_backend, table, FS};
use hertzforge_capture::{read_binary_record, CaptureSidecar, SIDECAR_FILE};
use hertzforge_twr::service::config::TwrConfig;
use hertzforge_twr::{ReferenceWaveform, TwrService};
use std::path::{Path, PathBuf};

fn root(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "hertzforge_twr_run_records_{tag}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&p);
    p
}

fn sidecar(dir: &Path) -> (CaptureSidecar, u64) {
    let path = dir.join(SIDECAR_FILE);
    let len = std::fs::metadata(&path).unwrap().len();
    (
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap(),
        len,
    )
}

/// A capture started for `cfg`, stopped at once: its folder.
fn captured(cfg: TwrConfig, root: &Path) -> PathBuf {
    let mut svc = TwrService::new(player(sim_backend()), cfg).unwrap();
    let dir = svc.start_capture(root).unwrap();
    svc.stop_capture().unwrap();
    dir
}

#[test]
fn the_sidecar_carries_the_channel_table_the_run_plays_with() {
    let root = root("table");
    let dir = captured(config(), &root);
    let (side, _) = sidecar(&dir);
    assert_eq!(side.channel_table, table());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_reference_is_written_once_and_the_sidecar_does_not_grow_with_it() {
    let root = root("reference");
    let short = field_like_reference();
    let mut samples = short.samples.clone();
    for _ in 0..7 {
        samples.extend_from_slice(&short.samples);
    }
    let long = ReferenceWaveform::from_samples(&short.label, FS, samples).unwrap();
    let mut sizes = Vec::new();
    for reference in [short, long] {
        let mut cfg = config();
        cfg.reference = reference.clone();
        let dir = captured(cfg, &root);
        let (side, len) = sidecar(&dir);
        sizes.push(len);
        // The sidecar describes the reference and names its file; no samples.
        let run: serde_json::Value = serde_json::from_str(&side.run_config_json).unwrap();
        let described = &run["reference"];
        assert!(described.get("samples").is_none(), "{described}");
        assert_eq!(described["len"], reference.samples.len());
        assert_eq!(described["fs_hz"], reference.fs_hz);
        assert_eq!(described["eu"], reference.eu.as_str());
        assert_eq!(described["file"], "reference.bin");
        // The run folder's record gives the samples back bit for bit.
        let (header, bytes) = read_binary_record(&dir, "reference").unwrap();
        assert_eq!(&header, described);
        let back: Vec<f64> = bytes
            .chunks_exact(8)
            .map(|b| f64::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert_eq!(back.len(), reference.samples.len());
        assert!(back
            .iter()
            .zip(&reference.samples)
            .all(|(a, b)| a.to_bits() == b.to_bits()));
    }
    // Eight times the reference, the same sidecar to within the digits of its
    // length and hash.
    assert!(sizes[1].abs_diff(sizes[0]) < 16, "sidecar sizes {sizes:?}");
    let _ = std::fs::remove_dir_all(&root);
}
