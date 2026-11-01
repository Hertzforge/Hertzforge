//! Random's arm path CONSULTS the shared arm gate
//! (`hertzforge_module_abi::arm_gate`).
//!
//! One of six sibling tests (a `tests/arm_gate_consulted.rs` in each module
//! crate) that together answer the predicate "every arm consults the
//! resolver". Per-crate rather than one cross-module file, because the
//! falsifying mutation is per-module: bypass the helper in ONE module's arm
//! path and exactly that module's test reds, which is what makes the suite tell
//! you WHERE the gate went missing.
//!
//! RED-MUTATION for this file: delete the `hertzforge_module_abi::arm_gate(...)` call
//! from `PlantLoop::open` (in `src/service/engine.rs`) and the refusal test
//! below reds - the COLA loop would run its per-tick correction against a plant
//! whose clock domain is not proven Shared.
//!
//! ## Why the refusal lever here is the CLOCK, not a device-loop backend
//!
//! Random arms `{HostSynthesized, HostPerTick}`, and the gate's clock
//! precondition is the arm-time refusal that reaches this path. An unproven
//! clock routes through the same `arm_gate` call and yields the same typed
//! `UnsupportedReason` as the sibling tests' device-loop mocks.
//!
//! ## The mock moved DOWN a layer
//!
//! Originally this file could not build the backend it wanted to test against -
//! `PlantLoop` manufactured its own `SimBackend` from `PlantConfig`, so the
//! unproven clock had to be injected through a `PlantConfig::shared_clock`
//! FIELD that existed only to make this refusal reachable. That field is gone:
//! the loop now takes a finished `Box<dyn Backend>`, so the test builds a Sim
//! plant with `.shared_clock(false)` directly. Same property, same gate, one
//! fewer piece of production config that existed for a test's benefit.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal_sim::SimConfig;
use hertzforge_module_abi::StationClass;
use hertzforge_random::service::engine::PlantLoop;
use hertzforge_random::LoopConfig;

fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    t.channels.push(ctrl);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    t.channels.push(drv);
    t
}

/// The HERTZFORGE-SIM demo plant the Random gates were validated against, with the
/// shared-clock declaration as a parameter. `shared_clock(true)` is the physical
/// truth for a software plant (input and output advance on the same synthetic
/// sample index); `false` is the mock.
fn demo_plant(shared_clock: bool) -> Box<dyn hertzforge_hal::Backend> {
    Box::new(
        SimConfig::new()
            .sample_rate_hz(8_192.0)
            .resonances(vec![(200.0, 2.5), (550.0, 2.5)])
            .noise_level(2e-4)
            .feedthrough(0.35)
            .shared_clock(shared_clock)
            .seed(0x5A5A_1234)
            .max_input_channels(1)
            .max_output_channels(1)
            .build(),
    )
}

#[test]
fn random_arm_refuses_an_unproven_clock_with_the_gates_typed_reason() {
    let mut cfg = LoopConfig::demo_default();
    let text = match PlantLoop::open(
        demo_plant(false),
        StationClass::HertzforgeSim,
        &mut cfg,
        table(),
    ) {
        Err(e) => e,
        Ok(_) => panic!("Random corrects per tick; that needs a proven shared clock"),
    };

    // The gate's `ClockNotShared` text, reaching the service error verbatim
    // (surfacing it as a UI panel is later work; today it must at least survive).
    assert!(
        text.contains("per-tick host correction"),
        "the refusal must be the gate's, not a generic backend error: {text}"
    );
    assert!(
        text.contains("Shared"),
        "the refusal must name the precondition: {text}"
    );
    assert!(
        text.contains("drift"),
        "the refusal must say WHY it matters: {text}"
    );
    assert!(text.contains("arm refused"), "{text}");
}

/// The other half. A refusal that could not have been an acceptance proves
/// nothing about the gate, so the default plant - which declares the shared
/// clock every real use has - must open.
#[test]
fn random_arm_succeeds_on_a_plant_that_declares_a_shared_clock() {
    let mut cfg = LoopConfig::demo_default();
    assert!(
        PlantLoop::open(
            demo_plant(true),
            StationClass::HertzforgeSim,
            &mut cfg,
            table()
        )
        .is_ok(),
        "a proven shared clock satisfies the gate"
    );
}
