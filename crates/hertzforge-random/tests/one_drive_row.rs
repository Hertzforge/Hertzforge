//! Single-output Random drives ONE output. Its playback step emits one drive
//! sample per frame while `PlantLoop::open` sizes the engine for every enabled
//! Drive row, so a table with two Drive rows would spread one stream across both
//! outputs and drop half the frames, and nothing would say so. Until the drive
//! path is generalized for multiple-output control, the arm refuses such a table
//! before any device is opened.
//!
//! RED-MUTATION: delete the drive-row count check at the top of `PlantLoop::open`
//! (in `src/service/engine.rs`) and `two_drive_rows_are_refused_before_the_device_opens`
//! reds: the open succeeds and the engine is sized for two outputs.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal_sim::SimConfig;
use hertzforge_module_abi::StationClass;
use hertzforge_random::service::engine::PlantLoop;
use hertzforge_random::LoopConfig;

fn table(drive_rows: usize) -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    t.channels.push(ctrl);
    for k in 0..drive_rows {
        let mut drv = Channel::new(2 + k as u32);
        drv.role = ChannelRole::Drive;
        t.channels.push(drv);
    }
    t
}

/// A plant that would ACCEPT two outputs, so the refusal cannot be the backend's.
fn plant() -> Box<dyn hertzforge_hal::Backend> {
    Box::new(
        SimConfig::new()
            .sample_rate_hz(8_192.0)
            .resonances(vec![(200.0, 2.5), (550.0, 2.5)])
            .noise_level(2e-4)
            .feedthrough(0.35)
            .shared_clock(true)
            .seed(0x5A5A_1234)
            .max_input_channels(1)
            .max_output_channels(2)
            .build(),
    )
}

#[test]
fn two_drive_rows_are_refused_before_the_device_opens() {
    let mut cfg = LoopConfig::demo_default();
    let text = match PlantLoop::open(plant(), StationClass::HertzforgeSim, &mut cfg, table(2)) {
        Err(e) => e,
        Ok(_) => panic!("single-output Random must refuse a table with two drive rows"),
    };
    assert!(text.contains("arm refused"), "{text}");
    assert!(
        text.contains("2 drive rows"),
        "the refusal must count the rows: {text}"
    );
    assert!(
        text.contains("one output"),
        "the refusal must say what this test type drives: {text}"
    );
}

/// The other half: one drive row still opens, so the check refuses only what it
/// must.
#[test]
fn one_drive_row_opens() {
    let mut cfg = LoopConfig::demo_default();
    assert!(
        PlantLoop::open(plant(), StationClass::HertzforgeSim, &mut cfg, table(1)).is_ok(),
        "one drive row is the single-output case and must open"
    );
}
