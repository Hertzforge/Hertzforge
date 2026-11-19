//! The built-in law follows the drive the core played. When the core holds the
//! drive under what the law asked (the notch ceiling clamping a request, a cap,
//! the drive ceiling), the servo's own level follows the drive down instead of
//! climbing on, so it is never pinned at its ceiling with the drive frozen when
//! the hold ends.
//!
//! Found by the check's limit-channel case: past the resonance the notch
//! ceiling rises tick by tick, the core told the law the drive was free (the
//! drive in force sat under the new ceiling) and clamped every request back to
//! the ceiling. The servo climbed 0.1 to 0.2 dB a tick for 9 s, reached its
//! +120 dB ceiling, and from then on asked for no change at all: the drive
//! froze at 0.32 V while the response fell away, and the run aborted on the
//! low side at 483 Hz.

use hertzforge_sine::law::servo_blend::{ServoBlendLaw, ServoBlendParams, V_SERVO_LEVEL};
use hertzforge_sine::law::{Hold, LawValues, SineControlLaw, TickInputs};

const AMP: [f64; 1] = [0.5];
const PHASE: [f64; 1] = [0.0];

fn inputs(tick_index: u64, drive: f64) -> TickInputs<'static> {
    TickInputs {
        tick_index,
        dt_s: 0.01,
        frequency_hz: 300.0,
        sweep_rate_hz_per_s: 0.0,
        level_db: 0.0,
        target: 1.0,
        // 6 dB under the target throughout: the servo always asks for more.
        measured: 0.5,
        channel_amplitude: &AMP,
        channel_phase_deg: &PHASE,
        drive,
        prediction_db: None,
        hold: Hold::Free,
        notch_ceiling: None,
        settling: false,
    }
}

#[test]
fn a_drive_the_core_holds_under_the_request_does_not_wind_the_servo_up() {
    let mut law = ServoBlendLaw::new(&ServoBlendParams::default()).unwrap();
    let mut values = LawValues::new(4);
    let held = 1.0;
    law.engage(&inputs(0, 0.0), &mut values).unwrap();
    // Ten seconds of ticks on which the core plays `held` whatever the law
    // asks (6 dB low at the slow preset, the servo asks 0.12 dB a tick: 120 dB
    // in the ten seconds).
    for k in 1..=1000 {
        let out = law.tick(&inputs(k, held), &mut values).unwrap();
        assert!(
            out.drive > held,
            "tick {k}: 6 dB low, the law asks for more"
        );
    }
    let level = values.get(V_SERVO_LEVEL).unwrap();
    assert!(
        level.abs() < 1.0,
        "the servo's level follows the drive played: {level:+.2} dB"
    );
    // The hold ends: the core plays what the law asks, and the law keeps
    // raising the drive while the response is still low.
    let mut drive = held;
    for k in 1001..=1010 {
        let out = law.tick(&inputs(k, drive), &mut values).unwrap();
        assert!(
            out.drive > drive * 10f64.powf(0.1 / 20.0),
            "tick {k}: the law raises the drive after the hold ({} from {drive})",
            out.drive
        );
        drive = out.drive;
    }
}
