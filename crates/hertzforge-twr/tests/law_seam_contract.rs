//! The control-law seam as a contract: the service stores a law factory and a
//! clock without calling either, and refuses a configuration that sets
//! `control`, until the seam is built.
//!
//! The refusal matters for the same reason classical shock's and Sine's did: a
//! configuration that asks for the core's guards (the call budget, the miss
//! limit, the per-call caps, the first-drive bound, the alignment check, the
//! at-rest taper) must not quietly run the built-in iteration without them.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal_sim::SimConfig;
use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault, MonotonicClock};
use hertzforge_twr::service::config::TwrConfig;
use hertzforge_twr::service::TwrState;
use hertzforge_twr::{
    BackendReferencePlayer, ReferenceWaveform, TwrControl, TwrError, TwrLaw, TwrLawFactory,
    TwrLawSetup, TwrService,
};
use std::sync::Arc;

const FS: f64 = 8192.0;

fn reference() -> ReferenceWaveform {
    let n = 4096usize;
    let samples: Vec<f64> = (0..n)
        .map(|i| {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
            w * (std::f64::consts::TAU * 200.0 * i as f64 / FS).sin()
        })
        .collect();
    ReferenceWaveform::from_samples("burst 200 Hz", FS, samples).unwrap()
}

fn player() -> BackendReferencePlayer<hertzforge_hal_sim::SimBackend> {
    let backend = SimConfig::new()
        .sample_rate_hz(FS)
        .resonances(vec![(300.0, 5.0)])
        .feedthrough(0.5)
        .shared_clock(true)
        .build();
    let mut table = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    table.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    table.channels.push(drv);
    BackendReferencePlayer::new(
        backend,
        table,
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap()
}

fn config() -> TwrConfig {
    TwrConfig::for_reference(reference())
}

/// A factory that is never called: building it is the only thing a test may
/// see happen.
struct Never;

impl TwrLawFactory for Never {
    fn build(&self, _: &TwrLawSetup<'_>) -> Result<Box<dyn TwrLaw + Send>, LawFault> {
        Err(LawFault::Failed("never built".into()))
    }
    fn describe(&self) -> LawDescriptor {
        LawDescriptor {
            id: "never".into(),
            name: "never".into(),
            version: "0".into(),
            sha256: None,
            params: serde_json::json!({}),
        }
    }
}

#[test]
fn a_configuration_that_sets_control_is_refused_until_the_seam_is_built() {
    let mut c = config();
    c.control = Some(TwrControl::default());
    match TwrService::new(player(), c) {
        Err(TwrError::Config(m)) => {
            assert!(m.contains("not built yet"), "{m}");
            assert!(m.contains("control"), "{m}");
        }
        Err(e) => panic!("refused for the wrong reason: {e}"),
        Ok(_) => panic!("a configuration asking for the core's law guards ran without them"),
    }
}

#[test]
fn a_bad_control_is_refused_by_its_field_before_the_seam_refusal() {
    let mut c = config();
    c.control = Some(TwrControl {
        first_drive_ratio: 0.5,
        ..TwrControl::default()
    });
    let e = TwrService::new(player(), c).err().expect("refused");
    assert!(e.to_string().contains("control.first_drive_ratio"), "{e}");
}

#[test]
fn without_control_the_service_builds_and_stores_a_factory_and_a_clock() {
    let svc = TwrService::new(player(), config()).expect("the open configuration builds");
    assert!(!svc.has_law_factory());
    let svc = svc
        .with_law_factory(Arc::new(Never))
        .with_law_clock(Arc::new(MonotonicClock::new()));
    assert!(svc.has_law_factory());
    assert_eq!(svc.state(), TwrState::Configured);
}

#[test]
fn a_record_without_control_serializes_without_it_and_both_round_trip() {
    // Samples that print and parse exactly: the workspace's JSON does not
    // promise a bit-exact round trip of every float, and this test is about the
    // field, not the reader.
    let flat = ReferenceWaveform::from_samples("flat", FS, vec![0.5; 4096]).unwrap();
    let config = || TwrConfig::for_reference(flat.clone());
    let open = config();
    let text = serde_json::to_string(&open).unwrap();
    assert!(
        !text.contains("\"control\""),
        "an open configuration's record must read as it did before the field existed"
    );
    let back: TwrConfig = serde_json::from_str(&text).unwrap();
    assert_eq!(back, open);

    let mut closed = config();
    closed.control = Some(TwrControl::default());
    let text = serde_json::to_string(&closed).unwrap();
    assert!(text.contains("\"control\""));
    let back: TwrConfig = serde_json::from_str(&text).unwrap();
    assert_eq!(back, closed);
}
