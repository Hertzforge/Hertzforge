//! A backend seen from the run: the station's backend with every call passed
//! through unchanged, and the three things a transient run needs to see at every
//! block of a fire it does not otherwise reach.
//!
//! - **The stop.** The run's cancel flag (raised by Stop, Abort and the safe stop
//!   without the run thread) is copied into the controller's own cancel flag at
//!   every block. The two are kept apart because the controller raises and
//!   consumes its own flag as it ends a fire; shared, the run would read the
//!   controller's own end as the operator's stop.
//! - **The fire's progress**, for the live frame: frames played of the record.
//! - **The simulator's pace**: on the simulator, input is read no faster than
//!   `speed` times real time from the fire's start, so a simulated run takes the
//!   time a real one would.
//! - **The abort's taper**, when the run asks for it ([`Watch::abort`]): a fire
//!   cancelled by an abort is brought to rest by the backend's emergency stop
//!   rather than its controlled stop.
//!
//! Nothing here changes a sample, a refusal or an order of calls: what the
//! controller asks of the backend is what the backend does.

use super::rails::Progress;
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The longest one pacing sleep, so a stop is seen within it.
const MAX_PACE_SLEEP: Duration = Duration::from_millis(25);

/// What the run hands an observed backend.
#[derive(Clone)]
pub struct Watch {
    /// The run's cancel flag.
    pub asked: Arc<AtomicBool>,
    /// The controller's own cancel flag, raised when `asked` is.
    pub own: Arc<AtomicBool>,
    pub progress: Arc<Progress>,
    /// Frames the next fire is expected to play; 0 when not known.
    pub expected: Arc<AtomicU64>,
    /// Simulated seconds per wall second; 0 runs unpaced.
    pub speed: f64,
    /// The run's abort flag, for a controller whose own stop at the end of a
    /// cancelled fire would take the controlled stop's taper: while it is raised,
    /// the backend's stop is its emergency stop. `None` leaves every stop as asked.
    pub abort: Option<Arc<AtomicBool>>,
}

impl Watch {
    /// Copy a raised stop into the controller's flag.
    pub fn bridge(&self) {
        if self.asked.load(Ordering::SeqCst) {
            self.own.store(true, Ordering::SeqCst);
        }
    }
}

/// The station's backend under a [`Watch`].
pub struct Observed {
    inner: Box<dyn Backend>,
    watch: Watch,
    rate_hz: f64,
    started: Option<Instant>,
    read_frames: u64,
    written_frames: u64,
}

impl Observed {
    pub fn new(inner: Box<dyn Backend>, watch: Watch) -> Observed {
        let rate_hz = inner.caps().sample_rate_hz;
        Observed {
            inner,
            watch,
            rate_hz,
            started: None,
            read_frames: 0,
            written_frames: 0,
        }
    }

    fn report(&self) {
        let of = self
            .watch
            .expected
            .load(Ordering::Relaxed)
            .max(self.written_frames);
        self.watch.progress.block(self.written_frames, of);
    }

    fn pace(&self) {
        let (Some(t0), true) = (self.started, self.watch.speed > 0.0 && self.rate_hz > 0.0) else {
            return;
        };
        let due = self.read_frames as f64 / self.rate_hz / self.watch.speed;
        let ahead = due - t0.elapsed().as_secs_f64();
        if ahead > 0.002 {
            std::thread::sleep(Duration::from_secs_f64(ahead).min(MAX_PACE_SLEEP));
        }
    }
}

impl Backend for Observed {
    fn caps(&self) -> Caps {
        self.inner.caps()
    }

    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }

    fn close(&mut self) -> Result<(), HalError> {
        self.inner.close()
    }

    fn take_warnings(&mut self) -> Vec<String> {
        self.inner.take_warnings()
    }

    fn configure(
        &mut self,
        table: &ChannelTable,
        acq: &AcquisitionConfig,
    ) -> Result<ConfiguredActual, HalError> {
        let actual = self.inner.configure(table, acq)?;
        self.rate_hz = actual.actual_rate_hz;
        Ok(actual)
    }

    fn arm(&mut self) -> Result<(), HalError> {
        self.inner.arm()
    }

    fn start(&mut self) -> Result<(), HalError> {
        self.watch.bridge();
        self.started = Some(Instant::now());
        self.read_frames = 0;
        self.written_frames = 0;
        self.watch
            .progress
            .start(self.watch.expected.load(Ordering::Relaxed));
        self.inner.start()
    }

    fn stop(&mut self) -> Result<(), HalError> {
        let aborting = self
            .watch
            .abort
            .as_ref()
            .is_some_and(|a| a.load(Ordering::SeqCst));
        let r = if aborting {
            self.inner.emergency_stop()
        } else {
            self.inner.stop()
        };
        self.started = None;
        self.watch.progress.end();
        r
    }

    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        self.watch.bridge();
        let r = self.inner.read_input(out)?;
        if let InputRead::Frames(n) = r {
            self.read_frames += n as u64;
            self.pace();
        }
        Ok(r)
    }

    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.watch.bridge();
        let n = self.inner.write_drive(drive)?;
        self.written_frames += n as u64;
        self.report();
        Ok(n)
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
        let r = self.inner.emergency_stop();
        self.started = None;
        self.watch.progress.end();
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_station::{BackendKind, StationSpec};

    fn watch(speed: f64) -> Watch {
        Watch {
            asked: Arc::new(AtomicBool::new(false)),
            own: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Progress::default()),
            expected: Arc::new(AtomicU64::new(0)),
            speed,
            abort: None,
        }
    }

    #[test]
    fn a_raised_stop_reaches_the_controllers_flag_at_the_next_block_and_progress_counts() {
        let sim = hertzforge_station::try_build_station_backend(
            BackendKind::Sim,
            &StationSpec::default(),
        )
        .unwrap();
        let w = watch(0.0);
        w.expected.store(64, Ordering::SeqCst);
        let mut b = Observed::new(sim.backend, w.clone());
        let mut t = crate::table::Table::new().table;
        let mut c = crate::table::new_row(1);
        c.role = hertzforge_channel_table::ChannelRole::Control;
        let mut d = crate::table::new_row(2);
        d.role = hertzforge_channel_table::ChannelRole::Drive;
        t.channels = vec![c, d];
        b.open().unwrap();
        let acq = AcquisitionConfig::new(b.caps().sample_rate_hz, 16);
        b.configure(&t, &acq).unwrap();
        b.arm().unwrap();
        b.start().unwrap();
        assert_eq!(w.progress.now(), Some((0, 64)));
        assert_eq!(b.write_drive(&[0.0; 16]).unwrap(), 16);
        assert_eq!(w.progress.now(), Some((16, 64)));
        assert!(!w.own.load(Ordering::SeqCst));
        w.asked.store(true, Ordering::SeqCst);
        let _ = b.write_drive(&[0.0; 16]);
        assert!(w.own.load(Ordering::SeqCst), "the stop was bridged");
        b.stop().unwrap();
        assert_eq!(w.progress.now(), None);
    }
}
