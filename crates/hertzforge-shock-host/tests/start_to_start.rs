//! The rate-model floor is a floor on the PHYSICAL start-to-start: the time
//! between the instants the device's clock started two pulses, not between two
//! host timestamps taken somewhere near them.
//!
//! The firer here is the shipping [`BackendPulseFirer`] over the simulator,
//! wrapped so that it runs on the same virtual clock as the service's pacer.
//! Each arm costs 15 ms and 0 ms in turn, and the start itself takes 0.5 ms
//! before the clock runs and 0.5 ms after, so the work between the service's
//! decision to fire and the clock actually running is a different length on
//! alternate pulses, as it is on the real chassis. The wrapper records the
//! instant the clock started.
//!
//! The run fires ladder pulses, then a train straight after the last of them,
//! then one more ladder pulse. What must hold for every pair of pulses: the
//! physical start-to-start is at least the floor, and the start time the
//! service reports is the physical start give or take the start call itself
//! (never before it).

use hertzforge_channel_table::ChannelTable;
use hertzforge_dsp::shock::{CompensationConfig, CompensationShape};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use hertzforge_hal_sim::{SimBackend, SimConfig};
use hertzforge_shock_common::{DrivePath, RateModel, ShockLevelSchedule, TrainSpec};
use hertzforge_shock_host::service::pacer::TrainPacer;
use hertzforge_shock_host::{
    BackendPulseFirer, ClassicalPulseSpec, ClassicalShape, ClassicalShockService,
    ClassicalTestConfig,
};
use std::sync::{Arc, Mutex};

const FS: f64 = 5120.0;

/// One virtual clock, seconds, read by the pacer and moved by the backend.
type Clock = Arc<Mutex<f64>>;

/// The service's pacer on the shared clock: a wait jumps to its deadline, and a
/// pulse start costs nothing of its own (the backend charges the fire's time).
struct SharedPacer(Clock);

impl TrainPacer for SharedPacer {
    fn now_s(&mut self) -> f64 {
        *self.0.lock().unwrap()
    }
    fn sleep_until(&mut self, deadline_s: f64) {
        let mut t = self.0.lock().unwrap();
        if deadline_s > *t {
            *t = deadline_s;
        }
    }
}

/// Each arm's cost in turn, seconds.
const ARM_COST_S: [f64; 2] = [0.015, 0.0];
/// The start call's time before the clock runs, and after.
const START_PRE_S: f64 = 0.0005;
const START_POST_S: f64 = 0.0005;

/// The simulator on the shared clock, recording when its clock started.
struct Clocked {
    inner: SimBackend,
    clock: Clock,
    arms: usize,
    starts: Arc<Mutex<Vec<f64>>>,
}

impl Clocked {
    fn advance(&self, s: f64) {
        *self.clock.lock().unwrap() += s;
    }
}

impl Backend for Clocked {
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
        self.advance(ARM_COST_S[self.arms % ARM_COST_S.len()]);
        self.arms += 1;
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.advance(START_PRE_S);
        let r = self.inner.start();
        let t = *self.clock.lock().unwrap();
        self.starts.lock().unwrap().push(t);
        self.advance(START_POST_S);
        r
    }
    fn stop(&mut self) -> Result<(), HalError> {
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let r = self.inner.read_input(out)?;
        if let InputRead::Frames(n) = r {
            self.advance(n as f64 / FS);
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
        self.inner.emergency_stop()
    }
}

fn spec_20g_11ms() -> ClassicalPulseSpec {
    let mut spec = ClassicalPulseSpec::new(ClassicalShape::HalfSine, 20.0, 11.0);
    spec.compensation = CompensationConfig {
        shape: CompensationShape::Rectangular,
        pre_frac: 4.0,
        post_frac: 4.0,
        symmetric: true,
    };
    spec
}

#[test]
fn physical_start_to_start_is_never_under_the_floor_on_the_ladder_or_into_a_train() {
    let clock: Clock = Arc::default();
    let starts = Arc::new(Mutex::new(Vec::new()));
    let backend = Clocked {
        inner: SimConfig::new()
            .sample_rate_hz(FS)
            .resonances(Vec::new())
            .noise_level(0.0)
            .feedthrough(1.0)
            .shared_clock(true)
            .build(),
        clock: Arc::clone(&clock),
        arms: 0,
        starts: Arc::clone(&starts),
    };
    #[allow(deprecated)]
    let table = ChannelTable::nonuniform_default_8ch();
    let firer = BackendPulseFirer::new(
        backend,
        table.clone(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .unwrap();
    let mut cfg = ClassicalTestConfig::new(spec_20g_11ms());
    cfg.schedule = ShockLevelSchedule::manual_default();
    cfg.rate_model = RateModel {
        cooling_floor_s: 0.5,
        settling_floor_s: 0.2,
        compute_floor_s: 0.1,
        device_recompute_floor_s: 20.0,
        read_cadence_ok: true,
    };
    let floor = cfg.rate_model.effective_min_spacing(DrivePath::HostDac);
    let mut s = ClassicalShockService::new(
        cfg,
        table,
        firer,
        Some(
            std::env::temp_dir().join(format!("hertzforge-shock-host-s2s-{}", std::process::id())),
        ),
    )
    .unwrap();
    s.set_pacer(Box::new(SharedPacer(Arc::clone(&clock))));
    s.assess_feasibility().unwrap();
    s.arm().unwrap();
    for _ in 0..4 {
        assert!(
            s.fire_next_pulse().unwrap(),
            "trip: {:?}",
            s.report_data().trip
        );
    }
    let plan = s.fire_train_with(TrainSpec::max_rate(3)).unwrap();
    assert_eq!(plan.realized_spacing_s, floor);
    assert!(s.fire_next_pulse().unwrap());
    s.stop().unwrap();

    let physical = starts.lock().unwrap().clone();
    let report = s.report_data();
    let reported: Vec<f64> = report.pulses.iter().map(|r| r.at_wall_time_s).collect();
    assert_eq!(
        physical.len(),
        8,
        "four ladder pulses, a train of three, one more"
    );
    assert_eq!(reported.len(), physical.len());

    for (k, w) in physical.windows(2).enumerate() {
        assert!(
            w[1] - w[0] >= floor - 1e-9,
            "pulses {k} and {} started {:.4} s apart on the device clock, under the \
             {floor} s floor (starts {physical:?})",
            k + 1,
            w[1] - w[0]
        );
    }
    for (k, (p, r)) in physical.iter().zip(&reported).enumerate() {
        assert!(
            *r >= p - 1e-9 && *r - p <= START_POST_S + 1e-9,
            "pulse {k}: reported start {r:.4} s against the device's {p:.4} s"
        );
    }
    let train = &report.trains[0];
    for (n, w) in train.pulse_numbers.iter().zip(&train.wall_times_s) {
        assert_eq!(
            *w, reported[*n as usize],
            "the train's wall time is the row's"
        );
    }
}
