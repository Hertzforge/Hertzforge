//! A shot whose wait is stopped or held never starts.
//!
//! The core brings each shot up (arms the device, commits the record's head)
//! before it waits out the time between pulses, so a Stop or a lowered
//! `should_continue` during that wait finds an ARMED device holding an unplayed
//! record. It must park it (stop and unreserve) without ever starting it. On a
//! Stop the run ends; on a hold the run goes on and the next call fires the
//! pulse, still no sooner than the time between pulses.
//!
//! The plant is the shipping `HalPulsePlant` over a 1 g/V loopback that records
//! its lifecycle calls and every frame it plays; the pacer raises the flag under
//! test at the first wait it sleeps in.

mod common;

use common::*;
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
    InputRead, LoopOwner,
};
use hertzforge_srs::service::pacer::PulsePacer;
use hertzforge_srs::service::plant::HalPulsePlant;
use hertzforge_srs::service::report::{EventKind, RunState};
use hertzforge_srs::{FirePath, SrsShockService};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

type Calls = Arc<Mutex<Vec<&'static str>>>;

/// A 1 g/V loopback that records `arm`, `start`, `stop` and each read that
/// delivered frames (`play`).
struct Loopback {
    queued: VecDeque<f32>,
    calls: Calls,
}

impl Backend for Loopback {
    fn caps(&self) -> Caps {
        Caps {
            loop_owner: LoopOwner::HostClosesLoop,
            native_modes: vec![],
            clock: ClockDomain::Shared,
            max_input_channels: 4,
            max_output_channels: 1,
            abort_latency_bound: None,
            output_buffer: None,
            sample_rate_hz: FS,
            delegated_response_observable: false,
            clock_groups: vec![],
        }
    }
    fn open(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), HalError> {
        Ok(())
    }
    fn configure(
        &mut self,
        _t: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        Ok(ConfiguredActual {
            actual_rate_hz: acq.requested_rate_hz,
            block_frames: acq.block_frames,
            group_delay_frames: None,
        })
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.calls.lock().unwrap().push("arm");
        Ok(())
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.calls.lock().unwrap().push("start");
        Ok(())
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.calls.lock().unwrap().push("stop");
        // Stopped and unreserved: whatever was committed is gone, unplayed.
        self.queued.clear();
        Ok(())
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.calls.lock().unwrap().push("emergency_stop");
        self.queued.clear();
        Ok(())
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let frames = out.len().min(self.queued.len());
        if frames == 0 {
            return Ok(InputRead::NoDataYet);
        }
        for v in out.iter_mut().take(frames) {
            *v = self.queued.pop_front().unwrap_or(0.0);
        }
        let mut calls = self.calls.lock().unwrap();
        if calls.last() != Some(&"play") {
            calls.push("play");
        }
        Ok(InputRead::Frames(frames))
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.queued.extend(drive.iter().copied());
        Ok(drive.len())
    }
    fn poll_status(&mut self) -> BackendStatus {
        BackendStatus::default()
    }
}

/// A virtual pacer that lowers or raises `flag` (to `to`) the first time a
/// wait sleeps on it, and records every pulse start.
struct FlagAtFirstSleep {
    now: f64,
    flag: Arc<AtomicBool>,
    to: bool,
    done: bool,
    starts: Arc<Mutex<Vec<f64>>>,
}

impl PulsePacer for FlagAtFirstSleep {
    fn now_s(&mut self) -> f64 {
        self.now
    }
    fn sleep_until(&mut self, deadline_s: f64) {
        if !self.done {
            self.done = true;
            self.flag.store(self.to, Ordering::SeqCst);
        }
        self.now = self.now.max(deadline_s);
    }
    fn pulse_start_s(&mut self) -> f64 {
        self.starts.lock().unwrap().push(self.now);
        let t = self.now;
        self.now += 0.01;
        t
    }
}

fn service(calls: &Calls) -> SrsShockService {
    let plant = HalPulsePlant::new(
        Loopback {
            queued: VecDeque::new(),
            calls: Arc::clone(calls),
        },
        siso_table(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap()
    .with_block_frames(512);
    SrsShockService::new(
        closed_cfg(1.0),
        siso_table(),
        FirePath::Host(Box::new(plant)),
        Some(std::env::temp_dir().join(format!("hertzforge-srs-held-{}", std::process::id()))),
    )
    .unwrap()
}

/// The calls since the last `start`'s pulse was parked: what the next shot did.
fn after_last_play(calls: &Calls) -> Vec<&'static str> {
    let c = calls.lock().unwrap();
    let i = c
        .iter()
        .rposition(|x| *x == "play")
        .expect("a pulse played");
    c[i + 1..].to_vec()
}

fn notes(s: &SrsShockService) -> Vec<String> {
    s.log()
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Note { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_stop_in_the_wait_parks_the_armed_shot_and_nothing_plays() {
    let calls: Calls = Arc::default();
    let mut s = service(&calls);
    let cancel = Arc::new(AtomicBool::new(false));
    s.set_cancel_handle(Arc::clone(&cancel));
    s.set_pacer(Box::new(FlagAtFirstSleep {
        now: 0.0,
        flag: Arc::clone(&cancel),
        to: true,
        done: false,
        starts: Arc::default(),
    }));
    assert_eq!(identify(&mut s), RunState::AwaitingReview);
    s.proceed_after_review().unwrap();
    assert!(s.fire_pulse().unwrap(), "pulse 0 has no pulse before it");
    // Pulse 1 is brought up, then its wait is stopped.
    assert!(!s.fire_pulse().unwrap());
    assert_eq!(s.state(), RunState::Stopped);
    assert_eq!(
        after_last_play(&calls),
        ["stop", "arm", "stop"],
        "pulse 0 parked; pulse 1 armed with its record committed, then parked unstarted"
    );
    assert_eq!(s.report_data().pulses.len(), 1);
    assert!(
        notes(&s)
            .iter()
            .any(|n| n == "stopped in the wait before pulse 1; nothing more fired"),
        "{:?}",
        notes(&s)
    );
}

#[test]
fn a_hold_in_the_wait_parks_the_shot_and_the_next_call_fires_it_on_time() {
    let calls: Calls = Arc::default();
    let mut s = service(&calls);
    let go = Arc::new(AtomicBool::new(true));
    let starts = Arc::new(Mutex::new(Vec::new()));
    s.set_pacer(Box::new(FlagAtFirstSleep {
        now: 0.0,
        flag: Arc::clone(&go),
        to: false,
        done: false,
        starts: Arc::clone(&starts),
    }));
    let floor = closed_cfg(1.0).control.unwrap().min_spacing_s();
    assert_eq!(identify(&mut s), RunState::AwaitingReview);
    s.proceed_after_review().unwrap();
    assert!(s.fire_pulse_cancellable(Some(&go)).unwrap());
    // Pulse 1's wait is held: parked unstarted, the run goes on.
    assert!(!s.fire_pulse_cancellable(Some(&go)).unwrap());
    assert_eq!(s.state(), RunState::Firing);
    assert_eq!(after_last_play(&calls), ["stop", "arm", "stop"]);
    assert_eq!(s.report_data().pulses.len(), 1);
    assert!(
        notes(&s)
            .iter()
            .any(|n| n.starts_with("held in the wait before pulse 1")),
        "{:?}",
        notes(&s)
    );
    // Released, the next call fires pulse 1, the time between pulses after 0.
    go.store(true, Ordering::SeqCst);
    assert!(s.fire_pulse_cancellable(Some(&go)).unwrap());
    let rows = s.report_data().pulses;
    assert_eq!(rows.len(), 2);
    let t = starts.lock().unwrap().clone();
    assert_eq!(t.len(), 2, "two pulses started: {t:?}");
    assert!(t[1] - t[0] >= floor - 1e-9, "{t:?}");
    let b = rows[1]
        .bring_up
        .expect("a closed-loop pulse carries its bring-up");
    assert!(b.wait_ms > 0.0, "the wait is timed: {b:?}");
}
