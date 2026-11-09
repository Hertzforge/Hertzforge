//! A shot whose wait is stopped or held never starts.
//!
//! The service brings each shot up (arms the station, commits the whole record)
//! before it waits out the floor, so a Stop or a lowered `should_continue`
//! during that wait finds an ARMED station holding an unplayed record. It must
//! park it (stop and unreserve) without ever starting it: a ladder pulse comes
//! back `false` with nothing fired, a train ends there with what it fired, and
//! after a hold the next pulse still keeps the floor.
//!
//! The firer is the shipping `BackendPulseFirer` over the simulator, wrapped to
//! record `arm`, `start`, `stop` and each read that delivered frames (`play`);
//! the pacer moves the flag under test at the first wait it sleeps in.

use hertzforge_channel_table::ChannelTable;
use hertzforge_dsp::shock::{CompensationConfig, CompensationShape};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_shock_common::{RateModel, ShockLevelSchedule, TrainSpec};
use hertzforge_shock_host::service::pacer::TrainPacer;
use hertzforge_shock_host::service::report::EventKind;
use hertzforge_shock_host::{
    BackendPulseFirer, ClassicalPulseSpec, ClassicalShape, ClassicalShockService,
    ClassicalTestConfig,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const FS: f64 = 5120.0;

type Calls = Arc<Mutex<Vec<&'static str>>>;

struct Recorded {
    inner: SimBackend,
    calls: Calls,
}

impl Backend for Recorded {
    fn caps(&self) -> Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }
    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
    }
    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        self.inner.configure(table, acq)
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.calls.lock().unwrap().push("arm");
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.calls.lock().unwrap().push("start");
        self.inner.start()
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.calls.lock().unwrap().push("stop");
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let r = self.inner.read_input(out)?;
        if matches!(r, InputRead::Frames(_)) {
            let mut calls = self.calls.lock().unwrap();
            if calls.last() != Some(&"play") {
                calls.push("play");
            }
        }
        Ok(r)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.inner.write_drive(drive)
    }
    fn output_space(&mut self) -> Option<usize> {
        self.inner.output_space()
    }
    fn input_overloads(&mut self) -> Vec<bool> {
        self.inner.input_overloads()
    }
    fn poll_status(&mut self) -> BackendStatus {
        self.inner.poll_status()
    }
    fn emergency_stop(&mut self) -> Result<(), HalError> {
        self.calls.lock().unwrap().push("emergency_stop");
        self.inner.emergency_stop()
    }
}

/// A virtual pacer that sets `flag` to `to` at the first wait it sleeps in once
/// it is armed (`armed`) and at least `after` pulses have started, and records
/// every pulse start.
struct FlagAtSleep {
    now: f64,
    flag: Arc<AtomicBool>,
    to: bool,
    armed: Arc<AtomicBool>,
    after: Arc<Mutex<usize>>,
    starts: Arc<Mutex<Vec<f64>>>,
}

impl TrainPacer for FlagAtSleep {
    fn now_s(&mut self) -> f64 {
        self.now
    }
    fn sleep_until(&mut self, deadline_s: f64) {
        let due = self.starts.lock().unwrap().len() >= *self.after.lock().unwrap();
        if due && self.armed.swap(false, Ordering::SeqCst) {
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

type Svc = ClassicalShockService<BackendPulseFirer<Recorded>>;

fn service(calls: &Calls) -> Svc {
    #[allow(deprecated)]
    let table = ChannelTable::nonuniform_default_8ch();
    let backend = Recorded {
        inner: SimConfig::new()
            .sample_rate_hz(FS)
            .resonances(Vec::new())
            .noise_level(0.0)
            .feedthrough(1.0)
            .shared_clock(true)
            .build(),
        calls: Arc::clone(calls),
    };
    let firer = BackendPulseFirer::new(
        backend,
        table.clone(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap();
    let mut spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, 11.0);
    spec.compensation = CompensationConfig {
        shape: CompensationShape::Rectangular,
        pre_frac: 4.0,
        post_frac: 4.0,
        symmetric: true,
    };
    let mut cfg = ClassicalTestConfig::new(spec);
    cfg.schedule = ShockLevelSchedule::manual_default();
    cfg.rate_model = RateModel {
        cooling_floor_s: 0.5,
        settling_floor_s: 0.2,
        compute_floor_s: 0.1,
        device_recompute_floor_s: 20.0,
        read_cadence_ok: true,
    };
    let mut s = ClassicalShockService::new(
        cfg,
        table,
        firer,
        Some(std::env::temp_dir().join(format!("hertzforge-shock-held-{}", std::process::id()))),
    )
    .unwrap();
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    s
}

fn after_last_play(calls: &Calls) -> Vec<&'static str> {
    let c = calls.lock().unwrap();
    let i = c
        .iter()
        .rposition(|x| *x == "play")
        .expect("a pulse played");
    c[i + 1..].to_vec()
}

fn noted(s: &Svc, text: &str) -> bool {
    s.log()
        .events()
        .iter()
        .any(|e| matches!(&e.kind, EventKind::Note { text: t } if t == text))
}

#[test]
fn a_stop_in_the_ladders_wait_parks_the_armed_shot_and_nothing_plays() {
    let calls: Calls = Arc::default();
    let mut s = service(&calls);
    let armed = Arc::new(AtomicBool::new(false));
    s.set_pacer(Box::new(FlagAtSleep {
        now: 0.0,
        flag: s.cancel_flag(),
        to: true,
        armed: Arc::clone(&armed),
        after: Arc::default(),
        starts: Arc::default(),
    }));
    assert!(s.fire_next_pulse().unwrap());
    armed.store(true, Ordering::SeqCst);
    assert!(!s.fire_next_pulse().unwrap(), "stopped in the wait");
    assert_eq!(
        after_last_play(&calls),
        ["stop", "arm", "stop"],
        "pulse 0 parked; pulse 1 armed with its record committed, then parked unstarted"
    );
    assert_eq!(s.report_data().pulses.len(), 1);
    assert!(noted(
        &s,
        "ladder pulse cancelled during the inter-pulse wait; nothing fired"
    ));
}

#[test]
fn a_hold_parks_the_shot_ends_a_train_there_and_the_next_pulse_keeps_the_floor() {
    let calls: Calls = Arc::default();
    let mut s = service(&calls);
    let go = Arc::new(AtomicBool::new(true));
    let armed = Arc::new(AtomicBool::new(false));
    let after = Arc::new(Mutex::new(0usize));
    let starts = Arc::new(Mutex::new(Vec::new()));
    s.set_pacer(Box::new(FlagAtSleep {
        now: 0.0,
        flag: Arc::clone(&go),
        to: false,
        armed: Arc::clone(&armed),
        after: Arc::clone(&after),
        starts: Arc::clone(&starts),
    }));
    let floor = 0.5;

    // A ladder pulse held in its wait: parked unstarted, the run goes on.
    assert!(s.fire_next_pulse_cancellable(Some(&go)).unwrap());
    armed.store(true, Ordering::SeqCst);
    assert!(!s.fire_next_pulse_cancellable(Some(&go)).unwrap());
    assert!(s.state().is_running());
    assert_eq!(after_last_play(&calls), ["stop", "arm", "stop"]);
    assert_eq!(s.report_data().pulses.len(), 1);
    go.store(true, Ordering::SeqCst);
    assert!(s.fire_next_pulse_cancellable(Some(&go)).unwrap());
    assert_eq!(s.report_data().pulses.len(), 2);

    // A train held in the wait before its second pulse ends there, one fired.
    *after.lock().unwrap() = starts.lock().unwrap().len() + 1;
    armed.store(true, Ordering::SeqCst);
    s.fire_train_cancellable(TrainSpec::max_rate(3), Some(&go))
        .unwrap();
    let report = s.report_data();
    assert_eq!(report.pulses.len(), 3, "the train fired one pulse");
    assert_eq!(after_last_play(&calls), ["stop", "arm", "stop"]);
    assert!(s.log().events().iter().any(|e| matches!(
        e.kind,
        EventKind::TrainCancelled {
            fired: 1,
            of_count: 3
        }
    )));

    // Every pulse that started kept the floor from the one before, and every
    // row carries its bring-up.
    let t = starts.lock().unwrap().clone();
    assert_eq!(t.len(), 3, "{t:?}");
    for w in t.windows(2) {
        assert!(w[1] - w[0] >= floor - 1e-9, "{t:?}");
    }
    assert!(report.pulses.iter().all(|r| r.bring_up.is_some()));
}
