//! Dispatcher tests: the liftability proof (a mock core driven through [`dispatch`]
//! with ZERO Tauri types anywhere in scope), the version gate, and the converging-
//! intent event flow.

use hertzforge_protocol::{
    dispatch, Basis, Command, CommandEnvelope, Completion, CoreApi, EngineeringUnit, Event,
    EventSink, IntentId, IntentOutcome, LevelTarget, Progress, ProtocolError, Response, VecSink,
    PROTOCOL_VERSION,
};

/// A mock core that behaves like the real split will:
///
/// - `SetLevel { Db }` applies instantly (the setpoint-hardware flavor, e.g. NI).
/// - `GoToFrequency` is accepted as CONVERGING (the paced-hardware flavor, e.g. a
///   delegated controller) and then advances one paced step per [`MockCore::tick`],
///   emitting progress - `tick` stands in for the core's background pacing loop,
///   which in production runs on its own thread at the OEM cadence (~200 ms) while
///   `handle` has long since returned.
/// - Retargeting mid-flight supersedes the old intent, exactly like the real pacer
///   converging on the latest target.
///
/// LIFTABILITY PROOF - the intent of this whole file: the mock is driven through the
/// SAME `dispatch` entry point the Tauri command layer calls, and there is no
/// `tauri::*` (nor any transport, nor any hardware crate) in scope. A socket server
/// feeding decoded envelopes into `dispatch` gets identical behavior; lifting the
/// core out of the Tauri process is a transport swap, not a redesign.
struct MockCore {
    /// Every command that actually reached the core (the version gate keeps
    /// mismatched envelopes out; a test asserts this stays empty for those).
    handled: Vec<Command>,
    /// Current (mock) measured frequency, Hz.
    frequency_hz: f64,
    /// Current (mock) level, dB.
    level_db: f64,
    /// The one in-flight frequency convergence, if any.
    in_flight: Option<InFlight>,
    /// Server-side intent id allocator.
    next_intent: u64,
}

struct InFlight {
    intent: IntentId,
    target_hz: f64,
}

/// The mock's per-tick frequency step (Hz) - the stand-in for one paced nudge burst.
const STEP_HZ: f64 = 100.0;
/// The mock's convergence deadband (Hz), mirroring the real converger's stop rule.
const DEADBAND_HZ: f64 = 1.0;

impl MockCore {
    fn new() -> Self {
        MockCore {
            handled: Vec::new(),
            frequency_hz: 100.0,
            level_db: 0.0,
            in_flight: None,
            next_intent: 1,
        }
    }

    /// One paced background step: move toward the target, emit progress, settle when
    /// inside the deadband. In production this is the delegate service's pacing
    /// thread; the trait's accept-and-return contract is what makes the equivalence
    /// hold.
    fn tick(&mut self, events: &mut dyn EventSink) {
        let Some(fly) = &self.in_flight else { return };
        let intent = fly.intent;
        let target = fly.target_hz;

        let gap = target - self.frequency_hz;
        let step = gap.abs().min(STEP_HZ) * gap.signum();
        self.frequency_hz += step;

        events.emit(Event::IntentProgress {
            intent,
            progress: Progress {
                current: self.frequency_hz,
                target,
                unit: EngineeringUnit::Hertz,
                basis: Basis::Measured,
                fraction: None,
            },
        });
        if (self.frequency_hz - target).abs() <= DEADBAND_HZ {
            self.in_flight = None;
            events.emit(Event::IntentSettled {
                intent,
                outcome: IntentOutcome::Completed,
            });
        }
    }
}

impl CoreApi for MockCore {
    fn handle(&mut self, cmd: Command, events: &mut dyn EventSink) -> Response {
        self.handled.push(cmd.clone());
        match cmd {
            // Paced flavor: accept, converge in the background.
            Command::GoToFrequency { hz } => {
                // A newer intent supersedes the in-flight one (the pacer converges
                // on the LATEST target; the old intent settles, honestly, as
                // Superseded rather than vanishing).
                if let Some(old) = self.in_flight.take() {
                    events.emit(Event::IntentSettled {
                        intent: old.intent,
                        outcome: IntentOutcome::Superseded,
                    });
                }
                let intent = IntentId(self.next_intent);
                self.next_intent += 1;
                self.in_flight = Some(InFlight {
                    intent,
                    target_hz: hz,
                });
                Response::Accepted {
                    completion: Completion::Converging { intent },
                }
            }
            // Setpoint flavor: applied before the response returns.
            Command::SetLevel {
                level: LevelTarget::Db(db),
            } => {
                self.level_db = db;
                Response::Accepted {
                    completion: Completion::Immediate,
                }
            }
            _ => Response::Accepted {
                completion: Completion::Immediate,
            },
        }
    }
}

/// The liftability proof itself (see the [`MockCore`] docs): the full command ->
/// dispatch -> response cycle with zero transport types in scope.
#[test]
fn mock_core_is_driven_through_the_dispatcher_with_zero_tauri_types() {
    let mut core = MockCore::new();
    let mut sink = VecSink::default();

    let reply = dispatch(
        &mut core,
        CommandEnvelope::new(
            42,
            Command::SetLevel {
                level: LevelTarget::Db(-6.0),
            },
        ),
        &mut sink,
    );

    // Correlation: the reply echoes the request id and carries OUR version.
    assert_eq!(reply.id, 42);
    assert_eq!(reply.v, PROTOCOL_VERSION);
    assert_eq!(
        reply.body,
        Response::Accepted {
            completion: Completion::Immediate
        }
    );
    // The intent reached the core in engineering units - no mechanics anywhere.
    assert_eq!(core.handled.len(), 1);
    assert_eq!(core.level_db, -6.0);
}

/// The version gate: a mismatched envelope is rejected loudly, with both versions
/// named, and the payload NEVER reaches the core.
#[test]
fn version_mismatch_is_rejected_before_the_core_sees_it() {
    let mut core = MockCore::new();
    let mut sink = VecSink::default();

    let mut request = CommandEnvelope::new(7, Command::Start);
    request.v = PROTOCOL_VERSION + 1; // a peer from the future

    let reply = dispatch(&mut core, request, &mut sink);

    assert_eq!(reply.id, 7, "even a rejection correlates to its request");
    assert_eq!(
        reply.v, PROTOCOL_VERSION,
        "the rejection speaks OUR dialect"
    );
    assert_eq!(
        reply.body,
        Response::Rejected {
            error: ProtocolError::VersionMismatch {
                ours: PROTOCOL_VERSION,
                theirs: PROTOCOL_VERSION + 1,
            }
        }
    );
    // Loudly and EARLY: nothing was parsed into the core's world.
    assert!(
        core.handled.is_empty(),
        "mismatched payload must never reach the core"
    );
    assert!(sink.events.is_empty());
}

/// The converging-intent contract end to end: accept as Converging, stream
/// monotonic progress toward the target, then settle Completed - the shape a UI
/// needs so a seconds-long paced convergence (delegated flavor) neither blocks the
/// response nor looks broken.
#[test]
fn converging_intent_emits_progress_then_completes() {
    let mut core = MockCore::new();
    let mut sink = VecSink::default();

    let reply = dispatch(
        &mut core,
        CommandEnvelope::new(1, Command::GoToFrequency { hz: 500.0 }),
        &mut sink,
    );
    let Response::Accepted {
        completion: Completion::Converging { intent },
    } = reply.body
    else {
        panic!(
            "paced hardware must answer Converging, got {:?}",
            reply.body
        );
    };
    assert!(
        sink.events.is_empty(),
        "no progress before the pacing loop runs"
    );

    // Drive the background pacing loop (bounded, like the real converger).
    for _ in 0..64 {
        core.tick(&mut sink);
        if core.in_flight.is_none() {
            break;
        }
    }

    let (settle, progress): (Vec<_>, Vec<_>) = sink
        .events
        .iter()
        .partition(|e| matches!(e, Event::IntentSettled { .. }));

    // Multiple paced progress samples, all correlated, monotone toward the target.
    assert!(
        progress.len() >= 2,
        "expected paced progress, got {progress:?}"
    );
    let mut last = 100.0;
    for ev in &progress {
        let Event::IntentProgress {
            intent: pid,
            progress: p,
        } = ev
        else {
            unreachable!()
        };
        assert_eq!(*pid, intent);
        assert_eq!(p.unit, EngineeringUnit::Hertz);
        assert_eq!(p.target, 500.0);
        assert!(
            p.current > last,
            "progress must advance: {} -> {}",
            last,
            p.current
        );
        last = p.current;
    }

    // Exactly one terminal event, after the progress, and it is Completed.
    assert_eq!(settle.len(), 1);
    assert_eq!(
        settle[0],
        &Event::IntentSettled {
            intent,
            outcome: IntentOutcome::Completed
        }
    );
    assert!(
        matches!(sink.events.last().unwrap(), Event::IntentSettled { .. }),
        "the terminal event ends the stream for this intent"
    );
    assert!((core.frequency_hz - 500.0).abs() <= DEADBAND_HZ);
}

/// Retargeting mid-convergence: the old intent settles as Superseded (a normal
/// outcome, not an error) and the new one carries on to completion - the operator
/// turning the dial twice must not produce failure noise.
#[test]
fn retargeting_supersedes_the_in_flight_intent() {
    let mut core = MockCore::new();
    let mut sink = VecSink::default();

    let first = dispatch(
        &mut core,
        CommandEnvelope::new(1, Command::GoToFrequency { hz: 500.0 }),
        &mut sink,
    );
    let Response::Accepted {
        completion: Completion::Converging { intent: first_id },
    } = first.body
    else {
        panic!("expected Converging");
    };
    core.tick(&mut sink); // partway there...

    let second = dispatch(
        &mut core,
        CommandEnvelope::new(2, Command::GoToFrequency { hz: 250.0 }),
        &mut sink,
    );
    let Response::Accepted {
        completion: Completion::Converging { intent: second_id },
    } = second.body
    else {
        panic!("expected Converging");
    };
    assert_ne!(first_id, second_id);

    // The old intent settled Superseded the moment the new one was accepted.
    assert!(sink.events.contains(&Event::IntentSettled {
        intent: first_id,
        outcome: IntentOutcome::Superseded
    }));

    for _ in 0..64 {
        core.tick(&mut sink);
        if core.in_flight.is_none() {
            break;
        }
    }
    assert!(sink.events.contains(&Event::IntentSettled {
        intent: second_id,
        outcome: IntentOutcome::Completed
    }));
    assert!((core.frequency_hz - 250.0).abs() <= DEADBAND_HZ);
}
