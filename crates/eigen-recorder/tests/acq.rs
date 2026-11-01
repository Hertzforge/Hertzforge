//! Gate tests: AcquisitionConfig defaults, validation rejections, and the
//! derived readouts (exact values on the canonical 2048-block native config).

use eigen_recorder::{AcquisitionConfig, AveragingKind, WindowKind};

#[test]
fn defaults_are_the_documented_selection() {
    let cfg = AcquisitionConfig::default();
    assert_eq!(cfg.native_rate_hz, 65_536.0);
    assert_eq!(cfg.analysis_decimation, 1);
    assert_eq!(cfg.block_size, 2048);
    assert_eq!(cfg.window, WindowKind::Hann);
    assert_eq!(cfg.overlap_pct, 50.0);
    assert_eq!(cfg.averaging, AveragingKind::Linear);
    assert_eq!(cfg.average_count, 8);
    cfg.validate().expect("defaults must validate");
}

#[test]
fn per_window_default_overlap() {
    assert_eq!(WindowKind::Hann.default_overlap_pct(), 50.0);
    assert_eq!(WindowKind::Flattop.default_overlap_pct(), 75.0);
    assert_eq!(WindowKind::Rectangular.default_overlap_pct(), 0.0);
    assert_eq!(WindowKind::BlackmanHarris.default_overlap_pct(), 66.7);
}

#[test]
fn derived_readouts_exact_on_the_canonical_config() {
    // 2048 block @ 65,536 native -> df 32 Hz, 800 lines, span 25,600 Hz.
    let cfg = AcquisitionConfig::default();
    assert_eq!(cfg.analysis_rate_hz(), 65_536.0);
    assert_eq!(cfg.df_hz(), 32.0);
    assert_eq!(cfg.spectral_lines(), 800);
    assert_eq!(cfg.span_hz(), 25_600.0);
    assert_eq!(cfg.block_seconds(), 2048.0 / 65_536.0);
}

#[test]
fn derived_readouts_follow_the_decimation_ladder() {
    let cfg = AcquisitionConfig {
        analysis_decimation: 4,
        ..AcquisitionConfig::default()
    };
    cfg.validate().unwrap();
    assert_eq!(cfg.analysis_rate_hz(), 16_384.0);
    assert_eq!(cfg.df_hz(), 8.0);
    assert_eq!(cfg.span_hz(), 6_400.0);
    assert_eq!(cfg.block_seconds(), 2048.0 / 16_384.0);
    // Lines depend only on the block size.
    assert_eq!(cfg.spectral_lines(), 800);
}

#[test]
fn validation_rejects_bad_native_rate() {
    for bad in [0.0f64, -1.0, f64::NAN] {
        let cfg = AcquisitionConfig {
            native_rate_hz: bad,
            ..AcquisitionConfig::default()
        };
        assert!(cfg.validate().is_err(), "rate {bad} must be rejected");
    }
}

#[test]
fn validation_rejects_bad_decimation() {
    for bad in [0u32, 3, 5, 6, 7, 12, 100, 512, 1024] {
        let cfg = AcquisitionConfig {
            analysis_decimation: bad,
            ..AcquisitionConfig::default()
        };
        assert!(cfg.validate().is_err(), "decimation {bad} must be rejected");
    }
    for good in [1u32, 2, 4, 8, 16, 32, 64, 128, 256] {
        let cfg = AcquisitionConfig {
            analysis_decimation: good,
            ..AcquisitionConfig::default()
        };
        assert!(cfg.validate().is_ok(), "decimation {good} must validate");
    }
}

#[test]
fn validation_rejects_bad_block_size() {
    for bad in [0usize, 100, 128, 255, 257, 3000, 131_072] {
        let cfg = AcquisitionConfig {
            block_size: bad,
            ..AcquisitionConfig::default()
        };
        assert!(cfg.validate().is_err(), "block {bad} must be rejected");
    }
    for good in [
        256usize, 512, 1024, 2048, 4096, 8192, 16_384, 32_768, 65_536,
    ] {
        let cfg = AcquisitionConfig {
            block_size: good,
            ..AcquisitionConfig::default()
        };
        assert!(cfg.validate().is_ok(), "block {good} must validate");
    }
}

#[test]
fn validation_rejects_bad_overlap() {
    for bad in [-0.1f64, -50.0, 95.1, 100.0, f64::NAN] {
        let cfg = AcquisitionConfig {
            overlap_pct: bad,
            ..AcquisitionConfig::default()
        };
        assert!(cfg.validate().is_err(), "overlap {bad} must be rejected");
    }
    for good in [0.0f64, 50.0, 66.7, 75.0, 95.0] {
        let cfg = AcquisitionConfig {
            overlap_pct: good,
            ..AcquisitionConfig::default()
        };
        assert!(cfg.validate().is_ok(), "overlap {good} must validate");
    }
}

#[test]
fn validation_rejects_zero_average_count() {
    let cfg = AcquisitionConfig {
        average_count: 0,
        ..AcquisitionConfig::default()
    };
    assert!(cfg.validate().is_err());
}

#[test]
fn serde_round_trip() {
    let cfg = AcquisitionConfig {
        analysis_decimation: 8,
        block_size: 4096,
        window: WindowKind::Flattop,
        overlap_pct: WindowKind::Flattop.default_overlap_pct(),
        averaging: AveragingKind::PeakHold,
        average_count: 16,
        ..AcquisitionConfig::default()
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let back: AcquisitionConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back, cfg);
}
