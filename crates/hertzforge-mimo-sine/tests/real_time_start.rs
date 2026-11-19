//! Starting on a station whose output runs on its own clock.
//!
//! The simulator steps in lockstep with the service, so it cannot show what
//! happens when a station's output plays whether or not it is fed. The double
//! here does, with the measured behaviour of a non-regenerating output clocked
//! from its input: a bounded output buffer, silence added at the start up to
//! the lookahead when the caller wrote less, a module that takes frames into
//! its own store at once, an output that has run dry reporting no space with
//! no error and accepting nothing from then on, input that accumulates with
//! the clock, and a loopback from what was really played to what is read.
//! Time is the double's own frame clock, advanced by the reads, by the test,
//! and by a script per start; nothing here reads the wall clock.

mod common;

use common::{flat, law};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_mimo_sine::records::TickRecord;
use hertzforge_mimo_sine::service::{EngineOptions, MimoSineService, ServiceState};
use hertzforge_mimo_sine::*;
use hertzforge_module_abi::StationClass;
use hertzforge_sine::scheduler::{DwellFor, DwellPlan, LadderConfig, RungFor};

/// A station double that runs in real time.
mod double {
    use hertzforge_channel_table::{ChannelRole, ChannelTable};
    use hertzforge_hal::{
        AcquisitionConfig, Backend, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError,
        InputRead, LoopOwner, OutputBufferContract, UnderrunPolicy,
    };
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, MutexGuard};

    /// The chassis as measured: the rate, a 768 frame output buffer, a 512
    /// frame lookahead, 127 frames taken into the output module's own store at
    /// the start, and the loop's physical delay.
    #[derive(Debug, Clone, Copy)]
    pub struct Params {
        pub fs_hz: f64,
        pub buffer_frames: usize,
        pub lookahead_frames: usize,
        pub store_frames: usize,
        pub delay_frames: usize,
    }

    impl Default for Params {
        fn default() -> Self {
            Params {
                fs_hz: 12_800.0,
                buffer_frames: 768,
                lookahead_frames: 512,
                store_frames: 127,
                delay_frames: 41,
            }
        }
    }

    /// What happens at one start.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct StartScript {
        /// Frames of the station's clock that pass between the start and the
        /// next call made on the station.
        pub first_call_after: u64,
        /// The first read after the start finds no input yet.
        pub first_read_empty: bool,
        /// Writes made before the start are taken up to this many frames.
        pub prestart_cap: Option<usize>,
    }

    /// What the double saw at one start.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct StartSeen {
        /// Frames the caller wrote before the start.
        pub written_before: usize,
        /// Silence the start added to reach the lookahead.
        pub silence_added: usize,
        /// Whether this start's output ran dry.
        pub went_dry: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Task {
        Closed,
        Open,
        Configured,
        Armed,
        Running,
        Stopped,
    }

    struct State {
        p: Params,
        task: Task,
        block: usize,
        n_in: usize,
        n_out: usize,
        buffer: VecDeque<f32>,
        store: VecDeque<f32>,
        hold: Vec<f32>,
        dry: bool,
        line: VecDeque<Vec<f32>>,
        acquired: VecDeque<f32>,
        clock: u64,
        counter: u64,
        next_script: Option<StartScript>,
        first_call: Option<u64>,
        first_read_empty: bool,
        prestart_cap: Option<usize>,
        written_before: usize,
        starts: Vec<StartSeen>,
        running_at_return: Vec<&'static str>,
    }

    impl State {
        fn buffered(&self) -> usize {
            self.buffer.len() / self.n_out.max(1)
        }

        fn refill_store(&mut self) {
            let n = self.n_out;
            while self.store.len() < self.p.store_frames * n && self.buffer.len() >= n {
                for _ in 0..n {
                    let v = self.buffer.pop_front().expect("checked");
                    self.store.push_back(v);
                }
            }
        }

        /// Let the station's clock run `frames` frames.
        fn advance(&mut self, frames: u64) {
            if self.task != Task::Running {
                return;
            }
            let n = self.n_out;
            for _ in 0..frames {
                let frame: Vec<f32> = if self.dry {
                    self.hold.clone()
                } else if self.store.len() >= n && n > 0 {
                    let f: Vec<f32> = self.store.drain(..n).collect();
                    self.refill_store();
                    f
                } else {
                    self.dry = true;
                    if let Some(s) = self.starts.last_mut() {
                        s.went_dry = true;
                    }
                    self.hold.clone()
                };
                self.hold.clone_from(&frame);
                self.line.push_back(frame);
                let out = self.line.pop_front().expect("the line holds its delay");
                for i in 0..self.n_in {
                    self.acquired.push_back(out.get(i).copied().unwrap_or(0.0));
                }
                self.clock += 1;
            }
        }

        fn on_call(&mut self) {
            if let Some(f) = self.first_call.take() {
                self.advance(f);
            }
        }

        fn clear_output(&mut self) {
            self.buffer.clear();
            self.store.clear();
            self.acquired.clear();
            self.dry = false;
            self.hold = vec![0.0; self.n_out];
        }
    }

    /// The test's side of the double: its clock and what it saw.
    #[derive(Clone)]
    pub struct Handle(Arc<Mutex<State>>);

    impl Handle {
        fn lock(&self) -> MutexGuard<'_, State> {
            self.0.lock().expect("the double's lock")
        }

        /// Let `ms` of the station's time pass (the caller's own work).
        pub fn pass_ms(&self, ms: f64) {
            let mut s = self.lock();
            let frames = (ms * 1e-3 * s.p.fs_hz).round() as u64;
            s.advance(frames);
        }

        /// The script for the next start.
        pub fn script_next_start(&self, script: StartScript) {
            self.lock().next_script = Some(script);
        }

        /// Every start so far.
        pub fn starts(&self) -> Vec<StartSeen> {
            self.lock().starts.clone()
        }

        /// Whether the station's tasks are running now.
        pub fn running(&self) -> bool {
            self.lock().task == Task::Running
        }

        /// The milliseconds `frames` frames last at the double's rate.
        pub fn frames_of_ms(&self, ms: f64) -> u64 {
            (ms * 1e-3 * self.lock().p.fs_hz).round() as u64
        }

        /// Notes the test made of calls that returned with the station running.
        pub fn note_running_at_return(&self, what: &'static str) {
            let mut s = self.lock();
            if s.task == Task::Running {
                s.running_at_return.push(what);
            }
        }

        /// The calls that returned with the station running.
        pub fn running_at_return(&self) -> Vec<&'static str> {
            self.lock().running_at_return.clone()
        }
    }

    /// The station's side.
    pub struct Station(Arc<Mutex<State>>);

    /// A double with `p`, and its handle.
    pub fn station(p: Params) -> (Box<dyn Backend>, Handle) {
        let s = Arc::new(Mutex::new(State {
            p,
            task: Task::Closed,
            block: 256,
            n_in: 0,
            n_out: 0,
            buffer: VecDeque::new(),
            store: VecDeque::new(),
            hold: Vec::new(),
            dry: false,
            line: VecDeque::new(),
            acquired: VecDeque::new(),
            clock: 0,
            counter: 0,
            next_script: None,
            first_call: None,
            first_read_empty: false,
            prestart_cap: None,
            written_before: 0,
            starts: Vec::new(),
            running_at_return: Vec::new(),
        }));
        (Box::new(Station(s.clone())), Handle(s))
    }

    impl Station {
        fn lock(&self) -> MutexGuard<'_, State> {
            self.0.lock().expect("the double's lock")
        }
    }

    fn wrong(what: &str, task: Task) -> HalError {
        HalError::Config(format!("{what} in {task:?}"))
    }

    impl Backend for Station {
        fn caps(&self) -> Caps {
            let s = self.lock();
            Caps {
                loop_owner: LoopOwner::HostClosesLoop,
                native_modes: vec![],
                clock: ClockDomain::Shared,
                max_input_channels: 32,
                max_output_channels: 32,
                abort_latency_bound: None,
                output_buffer: Some(OutputBufferContract {
                    min_lookahead_frames: s.p.lookahead_frames as u32,
                    underrun_policy: UnderrunPolicy::RampDownThenAbort,
                }),
                sample_rate_hz: s.p.fs_hz,
                delegated_response_observable: false,
                clock_groups: vec![],
            }
        }

        fn open(&mut self) -> Result<(), HalError> {
            let mut s = self.lock();
            if s.task != Task::Closed {
                return Err(wrong("open", s.task));
            }
            s.task = Task::Open;
            Ok(())
        }

        fn close(&mut self) -> Result<(), HalError> {
            let mut s = self.lock();
            s.clear_output();
            s.task = Task::Closed;
            Ok(())
        }

        fn configure(
            &mut self,
            table: &ChannelTable,
            acq: &AcquisitionConfig,
        ) -> Result<ConfiguredActual, HalError> {
            let mut s = self.lock();
            if !matches!(s.task, Task::Open | Task::Configured | Task::Stopped) {
                return Err(wrong("configure", s.task));
            }
            let on = table.channels.iter().filter(|c| c.enabled);
            s.n_in = on.clone().filter(|c| c.role != ChannelRole::Drive).count();
            s.n_out = on.filter(|c| c.role == ChannelRole::Drive).count();
            s.block = acq.block_frames.max(1);
            s.clear_output();
            s.task = Task::Configured;
            Ok(ConfiguredActual {
                actual_rate_hz: s.p.fs_hz,
                block_frames: s.block,
                group_delay_frames: None,
            })
        }

        fn arm(&mut self) -> Result<(), HalError> {
            let mut s = self.lock();
            if !matches!(s.task, Task::Open | Task::Configured | Task::Stopped) {
                return Err(wrong("arm", s.task));
            }
            s.task = Task::Armed;
            Ok(())
        }

        fn start(&mut self) -> Result<(), HalError> {
            let mut s = self.lock();
            if s.task != Task::Armed {
                return Err(wrong("start", s.task));
            }
            let script = s.next_script.take().unwrap_or_default();
            let queued = s.buffered();
            let want = s.p.lookahead_frames;
            let silence = want.saturating_sub(queued);
            let n = s.n_out;
            s.buffer.extend(std::iter::repeat_n(0.0f32, silence * n));
            let written_before = std::mem::take(&mut s.written_before);
            s.starts.push(StartSeen {
                written_before,
                silence_added: silence,
                went_dry: false,
            });
            s.dry = false;
            s.hold = vec![0.0; n];
            let delay = s.p.delay_frames;
            s.line = std::iter::repeat_n(vec![0.0f32; n], delay).collect();
            s.acquired.clear();
            s.task = Task::Running;
            s.refill_store();
            s.first_call = Some(script.first_call_after);
            s.first_read_empty = script.first_read_empty;
            s.prestart_cap = None;
            Ok(())
        }

        fn stop(&mut self) -> Result<(), HalError> {
            let mut s = self.lock();
            s.on_call();
            if matches!(s.task, Task::Running | Task::Armed) {
                s.clear_output();
                s.task = Task::Stopped;
            }
            Ok(())
        }

        fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
            let mut s = self.lock();
            s.on_call();
            match s.task {
                Task::Running => {}
                Task::Stopped => return Ok(InputRead::NoDataYet),
                other => return Err(wrong("read", other)),
            }
            if std::mem::take(&mut s.first_read_empty) {
                return Ok(InputRead::NoDataYet);
            }
            let n_in = s.n_in.max(1);
            let frames = (out.len() / n_in).min(s.block);
            let have = s.acquired.len() / n_in;
            if have < frames {
                s.advance((frames - have) as u64);
            }
            for v in out.iter_mut().take(frames * n_in) {
                *v = s.acquired.pop_front().expect("acquired");
            }
            s.counter += frames as u64;
            Ok(InputRead::Frames(frames))
        }

        fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
            let mut s = self.lock();
            s.on_call();
            if matches!(s.task, Task::Closed | Task::Open) {
                return Err(HalError::NotConnected);
            }
            let n = s.n_out.max(1);
            if s.dry {
                return Ok(0);
            }
            let offered = drive.len() / n;
            let space = s.p.buffer_frames.saturating_sub(s.buffered());
            let mut accepted = offered.min(space);
            if s.task != Task::Running {
                let cap = s
                    .next_script
                    .and_then(|x| x.prestart_cap)
                    .or(s.prestart_cap)
                    .unwrap_or(usize::MAX);
                accepted = accepted.min(cap.saturating_sub(s.written_before));
                s.written_before += accepted;
            }
            s.buffer.extend(&drive[..accepted * n]);
            Ok(accepted)
        }

        fn output_space(&mut self) -> Option<usize> {
            let mut s = self.lock();
            s.on_call();
            if s.dry {
                return Some(0);
            }
            Some(s.p.buffer_frames.saturating_sub(s.buffered()))
        }

        fn poll_status(&mut self) -> BackendStatus {
            let s = self.lock();
            BackendStatus {
                connected: s.task != Task::Closed,
                faulted: false,
                message: format!("{:?}", s.task),
                sample_counter: s.counter,
                epoch: 0,
            }
        }

        fn emergency_stop(&mut self) -> Result<(), HalError> {
            let mut s = self.lock();
            if matches!(s.task, Task::Running | Task::Armed) {
                s.clear_output();
                s.task = Task::Stopped;
            }
            Ok(())
        }
    }
}

use double::{Params, StartScript};

const FS: f64 = 12_800.0;
const BLOCK: usize = 256;

/// Four controls (channels 1 to 4) and four drives (5 to 8), each drive
/// looped back to the control of its own slot.
fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    for k in 0..8u32 {
        let mut c = Channel::new(k + 1);
        c.role = if k < 4 {
            ChannelRole::Control
        } else {
            ChannelRole::Drive
        };
        c.range_max = 1e9;
        t.channels.push(c);
    }
    t
}

/// A dwell at `hz` for `s` seconds at 0.5 g, the phases 0, 30, 60 and 90
/// degrees, ceilings 0.8 V and aborts 0.95 V: the bench's four-jumper dwell.
fn setup(hz: f64, s: f64) -> MimoSineSetup {
    MimoSineSetup {
        controls: (0..4)
            .map(|i| ControlChannel {
                channel: 1 + i,
                profile: flat(0.5, 1.0, 6.0),
                phase: vec![PhasePoint {
                    freq_hz: 5.0,
                    phase_deg: 30.0 * i as f64,
                }],
            })
            .collect(),
        drives: (0..4)
            .map(|i| DriveChannel {
                channel: 5 + i,
                ceiling_vpk: 0.8,
                abort_vpk: 0.95,
            })
            .collect(),
        limits: vec![],
        phase_specified: true,
        test: TestPlan::Dwell(DwellPlan {
            freq_hz: hz,
            for_: DwellFor::Seconds(s),
        }),
        schedule: Schedule {
            mode: LevelMode::Auto,
            ladder: LadderConfig {
                sysid_level_db: -20.0,
                start_db: 0.0,
                table_db: vec![0.0],
                per_rung: RungFor::Seconds(1e6),
                full_for: RungFor::Seconds(1e6),
            },
        },
        fs_hz: FS,
        tick_s: 0.01,
        tracking: Tracking::default(),
        identification: IdentificationSettings {
            level_db: -20.0,
            resolution_hz: 2.0,
            frames: Some(6),
            trust_coherence: 0.5,
            gate_coherence: 0.5,
            review: false,
        },
        law: LawChoice {
            id: "matrix".into(),
            params: Default::default(),
        },
        requests: RequestSettings::default(),
        guards: GuardSettings::default(),
    }
}

fn service(setup: MimoSineSetup, backend: Box<dyn hertzforge_hal::Backend>) -> MimoSineService {
    let factory = law(&setup.law.id);
    MimoSineService::new(
        setup,
        table(),
        backend,
        factory,
        EngineOptions {
            block_frames: Some(BLOCK),
            law_budget: Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET),
            ..EngineOptions::new(StationClass::HertzforgeSim)
        },
    )
    .expect("the setup is accepted")
}

/// Run an engaged service to its end and keep its update records.
fn run_to_end(svc: &mut MimoSineService, max_s: f64) -> Vec<TickRecord> {
    let (rt, _re) = svc.take_records().expect("records");
    let mut ticks = Vec::new();
    for _ in 0..((max_s * FS / BLOCK as f64) as u64 + 64) {
        let alive = svc.run_batches(8).expect("runs");
        ticks.extend(rt.try_iter());
        if !alive {
            break;
        }
    }
    ticks.extend(rt.try_iter());
    ticks
}

/// The worst phase error over the judged records after `from_s`, degrees.
fn worst_phase_deg(ticks: &[TickRecord], from_s: f64) -> f64 {
    ticks
        .iter()
        .filter(|t| t.time_s >= from_s && !t.settling && t.miss.is_none())
        .flat_map(|t| {
            (0..t.controls as usize).map(move |c| {
                let e = t.control[c] * t.judged_reference[c].conj();
                e.arg().to_degrees().abs()
            })
        })
        .fold(0.0, f64::max)
}

/// The largest drive any record shows, volts peak.
fn largest_drive_v(ticks: &[TickRecord]) -> f64 {
    ticks
        .iter()
        .flat_map(|t| (0..t.drives as usize).map(move |d| t.drive[d].norm()))
        .fold(0.0, f64::max)
}

/// The run's caller does its own work between the service's steps: opening
/// the run record after the arm, keeping the identification after it, the
/// screen after the engage. 90 ms of it is longer than the silence a start
/// puts in the output (40 ms at 12,800 Hz). None of it may leave the output
/// running unfed.
#[test]
fn mimo_sine_the_callers_work_between_steps_never_leaves_the_output_unfed() {
    let (backend, h) = double::station(Params::default());
    let mut svc = service(setup(60.0, 2.0), backend);
    svc.arm().expect("arms");
    h.note_running_at_return("arm");
    h.pass_ms(90.0);
    let rep = svc.identify().map(|r| r.gate_passed);
    h.note_running_at_return("identify");
    assert_eq!(rep, Ok(true), "identification: {rep:?}");
    h.pass_ms(90.0);
    svc.engage().expect("engages");
    h.note_running_at_return("engage");
    h.pass_ms(90.0);
    let ticks = run_to_end(&mut svc, 4.0);
    assert!(
        matches!(
            svc.state(),
            ServiceState::Ended(records::Outcome::Completed)
        ),
        "the run ended {:?}: {:?}",
        svc.state(),
        svc.station_note()
    );
    assert!(!ticks.is_empty());
    assert!(
        h.starts().iter().all(|s| !s.went_dry),
        "a start ran dry: {:?}",
        h.starts()
    );
    assert!(
        h.running_at_return().is_empty(),
        "the station was left running unfed after: {:?}",
        h.running_at_return()
    );
}

/// The slip: the engaged run's first tick comes at its start and finds no
/// input, so a prefill made then lands in a buffer still holding the start's
/// silence; the identification's start went the usual way. Away from 100 Hz
/// (where 128 frames is one whole cycle at this rate) every phase reading is
/// then off by 2 pi f 129 / fs (142.3 degrees the other way at 60 Hz): the
/// default law corrects the wrong way to its ceiling, and `ratio`, which holds
/// amplitude only, carries the error.
#[test]
fn mimo_sine_an_early_first_tick_at_engage_does_not_move_the_phase() {
    let mut failed = Vec::new();
    for id in ["matrix", "ratio"] {
        let (backend, h) = double::station(Params::default());
        let mut s = setup(60.0, 3.0);
        s.law.id = id.into();
        let mut svc = service(s, backend);
        svc.arm().expect("arms");
        let rep = svc.identify().map(|r| r.gate_passed);
        assert_eq!(rep, Ok(true), "identification: {rep:?}");
        h.script_next_start(StartScript {
            first_call_after: 0,
            first_read_empty: true,
            prestart_cap: None,
        });
        svc.engage().expect("engages");
        let ticks = run_to_end(&mut svc, 5.0);
        let phase = worst_phase_deg(&ticks, 1.5);
        let drive = largest_drive_v(&ticks);
        println!(
            "{id}: worst phase error {phase:.2} deg, largest drive {drive:.4} V, starts {:?}",
            h.starts()
        );
        if phase >= 5.0 || drive >= 0.7 {
            failed.push(format!(
                "{id}: worst phase error {phase:.2} degrees, largest drive {drive:.4} V"
            ));
        }
    }
    assert!(failed.is_empty(), "{failed:?}");
}
