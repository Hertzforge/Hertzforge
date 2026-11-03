//! What the catalog's tests share: the Sim plant and run configurations of
//! hertzforge-random's own guard tests, the built law files, a scratch data
//! root, and a law wrapper that records every call.

#![allow(dead_code)]

use hertzforge_breakpoints::{BreakpointRow, BreakpointTable};
use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_random::control::{
    ControlError, ControlLaw, LawFault, LevelCommand, LoopInputs, LoopOutputs,
};
use hertzforge_random::service::config::{LevelSchedule, RandomTestConfig};
use hertzforge_random::service::RandomService;
use hertzforge_random::{LawFactory, LawSetup, LoopConfig, RunState};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const CEILING_VRMS: f64 = 10.0;

/// The Sim plant the acceptance suite runs on: two in-band modes, healthy
/// feedthrough, a fixed noise seed.
pub fn backend(cfg: &LoopConfig) -> Box<dyn hertzforge_hal::Backend> {
    Box::new(
        hertzforge_hal_sim::SimConfig::new()
            .sample_rate_hz(cfg.fs_hz)
            .resonances(vec![(200.0, 2.5), (550.0, 2.5)])
            .noise_level(2e-4)
            .feedthrough(0.35)
            .shared_clock(true)
            .seed(0x5A5A_1234)
            .max_input_channels(1)
            .max_output_channels(1)
            .build(),
    )
}

/// Control on channel 1 (accel g), drive on channel 2.
pub fn siso_table() -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut ctrl = Channel::new(1);
    ctrl.role = ChannelRole::Control;
    ctrl.units = Units::AccelG;
    ctrl.range_max = 100.0;
    ctrl.abort.accel_g = Some(500.0);
    let mut drv = Channel::new(2);
    drv.role = ChannelRole::Drive;
    drv.units = Units::VoltageV;
    t.channels.push(ctrl);
    t.channels.push(drv);
    t
}

/// The decoded demo profile (20/0.01, 80/0.04, 350/0.04, 2000/0.007).
pub fn demo_profile() -> BreakpointTable {
    let mut bt = BreakpointTable::new("demo-profile");
    bt.push(BreakpointRow::psd(20.0, 0.01));
    bt.push(BreakpointRow::psd(80.0, 0.04));
    bt.push(BreakpointRow::psd(350.0, 0.04));
    bt.push(BreakpointRow::psd(2000.0, 0.007));
    bt
}

/// A full AUTO ladder: the soft start, every rung with a dwell of several
/// loops, level ramps, full level, and a ramped controlled stop at the end.
pub fn ladder_cfg() -> RandomTestConfig {
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.schedule = LevelSchedule::auto_default();
    cfg.schedule.dwell_s = vec![6.0];
    cfg.full_level_duration_s = 12.0;
    cfg.drive_ceiling_vrms = CEILING_VRMS;
    cfg.alarm_db = (6.0, 6.0);
    cfg.abort_db = (12.0, 12.0);
    cfg.level_ramp_s = 2.0;
    cfg.shutdown_ramp_s = 2.0;
    cfg
}

/// A MANUAL run with the envelope out of the way (the guards, not the
/// envelope, are under test) and a shutdown ramp so a controlled stop ramps.
pub fn manual_cfg() -> RandomTestConfig {
    let mut cfg = RandomTestConfig::siso(demo_profile(), 1);
    cfg.schedule = LevelSchedule::manual_default();
    cfg.drive_ceiling_vrms = CEILING_VRMS;
    cfg.alarm_db = (48.0, 48.0);
    cfg.abort_db = (60.0, 60.0);
    cfg.shutdown_ramp_s = 2.0;
    cfg
}

pub fn service(
    cfg: RandomTestConfig,
    factory: Arc<dyn LawFactory>,
    data: &Path,
    soft_start: bool,
) -> RandomService {
    let loop_cfg = LoopConfig::demo_default();
    let b = backend(&loop_cfg);
    let mut svc = RandomService::new(
        cfg,
        siso_table(),
        loop_cfg,
        b,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        Some(data.join("runs")),
    )
    .expect("service builds")
    .with_law_factory(factory);
    svc.set_review_after_sysid(false);
    if !soft_start {
        svc.set_soft_start_db(0.0);
    }
    svc
}

/// Feasibility, arm, identification and the gate: engaged when this returns.
pub fn engage(svc: &mut RandomService) {
    svc.assess_feasibility().expect("feasibility");
    svc.arm().expect("arm");
    svc.run_sysid(3).expect("sysid");
    assert!(svc.coherence_gate().expect("gate"), "the gate passes");
    assert_eq!(svc.state(), RunState::Engaged);
}

/// One tick as the core played it.
#[derive(Debug, Clone)]
pub struct Tick {
    pub state: RunState,
    pub level_db: f64,
    pub drive: Vec<f64>,
}

/// Tick the run until it ends or `max_ticks`, checking after every tick that
/// the drive the core synthesized is finite, non-negative and at or below the
/// drive ceiling.
pub fn run(svc: &mut RandomService, max_ticks: usize) -> Vec<Tick> {
    let df = svc.realized_df_hz();
    let ceiling = svc.config().drive_ceiling_vrms;
    let mut ticks = Vec::new();
    while ticks.len() < max_ticks {
        let more = svc.loop_tick().expect("loop_tick returns Ok");
        let drive = svc
            .last_drive_psd()
            .map(<[f64]>::to_vec)
            .unwrap_or_default();
        assert!(
            drive.iter().all(|p| p.is_finite() && *p >= 0.0),
            "tick {}: the synthesized drive holds a non-finite or negative line",
            ticks.len() + 1
        );
        let vrms = (drive.iter().sum::<f64>() * df).sqrt();
        assert!(
            vrms <= ceiling * (1.0 + 1e-9),
            "tick {}: drive {vrms} V rms over the {ceiling} V rms ceiling",
            ticks.len() + 1
        );
        ticks.push(Tick {
            state: svc.state(),
            level_db: svc.schedule_level_db(),
            drive,
        });
        if !more {
            break;
        }
    }
    ticks
}

/// A law call as the law answered it, before the core's checks.
#[derive(Debug, Clone)]
pub struct Call {
    pub kind: &'static str,
    pub out: Result<(Vec<f64>, f64), String>,
}

/// Wraps a law and records every call.
struct Recorder {
    inner: Box<dyn ControlLaw + Send>,
    log: Arc<Mutex<Vec<Call>>>,
}

impl Recorder {
    fn keep(
        &self,
        kind: &'static str,
        r: Result<LoopOutputs, LawFault>,
    ) -> Result<LoopOutputs, LawFault> {
        let out = match &r {
            Ok(o) => Ok((o.drive_psd.clone(), o.level_cmd_db)),
            Err(e) => Err(e.to_string()),
        };
        self.log.lock().unwrap().push(Call { kind, out });
        r
    }
}

impl ControlLaw for Recorder {
    fn init(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        let r = self.inner.init(inputs);
        self.keep("seed", r)
    }
    fn iterate(&mut self, inputs: &LoopInputs) -> Result<LoopOutputs, LawFault> {
        let r = self.inner.iterate(inputs);
        self.keep("iterate", r)
    }
    fn retarget_level(&mut self, cmd: LevelCommand) -> Result<LoopOutputs, LawFault> {
        let r = self.inner.retarget_level(cmd);
        self.keep("retarget", r)
    }
    fn name(&self) -> &str {
        self.inner.name()
    }
}

/// `factory`, with every call of the laws it builds recorded.
pub fn recording(factory: Arc<dyn LawFactory>) -> (Arc<dyn LawFactory>, Arc<Mutex<Vec<Call>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let l = log.clone();
    let f = Arc::new(
        move |s: &LawSetup<'_>| -> Result<Box<dyn ControlLaw + Send>, ControlError> {
            Ok(Box::new(Recorder {
                inner: factory.build(s)?,
                log: l.clone(),
            }))
        },
    );
    (f, log)
}

/// The path of `<name>.wasm`, built by scripts/build_laws.sh into
/// `<target>/wasm32-wasip2/release/`, found from this test binary's own path
/// (`<target>/<profile>/deps/<binary>`). `HERTZFORGE_LAW_WASM_DIR` overrides
/// the directory. Fails, naming the script, when the file is not built.
pub fn law_file(name: &str) -> PathBuf {
    let file = format!("{name}.wasm");
    let mut tried: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("HERTZFORGE_LAW_WASM_DIR") {
        tried.push(PathBuf::from(dir).join(&file));
    } else {
        let exe = std::env::current_exe().expect("the test binary knows its own path");
        for dir in exe.ancestors().skip(1).take(4) {
            tried.push(dir.join("wasm32-wasip2").join("release").join(&file));
        }
    }
    tried
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "the law file {file} is not built. Run `bash scripts/build_laws.sh` \
             (scripts/rust_tests.sh runs it first). Looked in: {tried:#?}"
            )
        })
}

pub const HINV: &str = "hertzforge_law_hinv";
pub const RATIO: &str = "hertzforge_law_ratio";
pub const MISBEHAVE: &str = "law_fixture_misbehave";
pub const BAD_MANIFEST: &str = "law_fixture_bad_manifest";

/// A scratch data root, removed when dropped.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "hertzforge-random-laws-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("scratch data root");
        Scratch(p)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
