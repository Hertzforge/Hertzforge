//! The station backend a Random run drives, with the operator's stop and abort
//! reaching it directly.
//!
//! The controller's system identification and each control loop are single blocking
//! calls that run many engine ticks. A stop or an abort from the shell must not wait
//! for one to finish, so the run hands the controller its station wrapped in
//! [`Interruptible`]: when the operator's signal is set, the next engine read takes
//! the drive down with the STATION's own stop (`emergency_stop` for an abort, the
//! controlled `stop` otherwise) and fails, which ends the controller's call within
//! one engine block. Nothing else changes: every other call passes straight
//! through, including the defaulted ones the HAL warns must be forwarded explicitly.

use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// No operator signal.
pub const NONE: u8 = 0;
/// The operator stopped the run: the station's controlled stop.
pub const STOP: u8 = 1;
/// The operator aborted the run, or a protection tripped: the station's own abort stop.
pub const ABORT: u8 = 2;

/// A station backend that honors the operator's signal at the next engine block.
pub struct Interruptible {
    inner: Box<dyn Backend>,
    signal: Arc<AtomicU8>,
    /// The signal has been acted on: the station is stopped and stays stopped.
    acted: bool,
}

impl Interruptible {
    pub fn wrap(inner: Box<dyn Backend>, signal: Arc<AtomicU8>) -> Box<dyn Backend> {
        Box::new(Interruptible {
            inner,
            signal,
            acted: false,
        })
    }

    fn refusal(&self) -> HalError {
        match self.signal.load(Ordering::SeqCst) {
            ABORT => HalError::Io("aborted by the operator".into()),
            _ => HalError::Io("stopped by the operator".into()),
        }
    }

    /// Act on a pending signal: take the station down with its own stop, once.
    fn check(&mut self) -> Result<(), HalError> {
        if self.acted {
            return Err(self.refusal());
        }
        match self.signal.load(Ordering::SeqCst) {
            NONE => Ok(()),
            ABORT => {
                self.acted = true;
                let _ = self.inner.emergency_stop();
                Err(self.refusal())
            }
            _ => {
                self.acted = true;
                let _ = self.inner.stop();
                Err(self.refusal())
            }
        }
    }
}

impl Backend for Interruptible {
    fn caps(&self) -> Caps {
        self.inner.caps()
    }
    fn open(&mut self) -> Result<(), HalError> {
        self.check()?;
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
        self.check()?;
        self.inner.configure(table, acq)
    }
    fn arm(&mut self) -> Result<(), HalError> {
        self.check()?;
        self.inner.arm()
    }
    fn start(&mut self) -> Result<(), HalError> {
        self.check()?;
        self.inner.start()
    }
    /// The controlled stop, unless an abort is signalled and not yet acted on: then
    /// the station's own abort stop. The run takes this path when it ends on a
    /// protection trip, so a trip never gets the gentler taper.
    fn stop(&mut self) -> Result<(), HalError> {
        if !self.acted && self.signal.load(Ordering::SeqCst) == ABORT {
            self.acted = true;
            return self.inner.emergency_stop();
        }
        self.inner.stop()
    }
    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        self.check()?;
        self.inner.read_input(out)
    }
    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        self.check()?;
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
        self.acted = true;
        self.inner.emergency_stop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records which stop the station was given.
    struct Probe {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Backend for Probe {
        fn caps(&self) -> Caps {
            hertzforge_hal_sim::SimConfig::default().build().caps()
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
            a: &AcquisitionConfig,
        ) -> Result<ConfiguredActual, HalError> {
            Ok(ConfiguredActual {
                actual_rate_hz: a.requested_rate_hz,
                block_frames: a.block_frames,
                group_delay_frames: None,
            })
        }
        fn arm(&mut self) -> Result<(), HalError> {
            Ok(())
        }
        fn start(&mut self) -> Result<(), HalError> {
            Ok(())
        }
        fn stop(&mut self) -> Result<(), HalError> {
            self.calls.lock().unwrap().push("stop");
            Ok(())
        }
        fn read_input(&mut self, _out: &mut [f32]) -> Result<InputRead, HalError> {
            self.calls.lock().unwrap().push("read");
            Ok(InputRead::Frames(1))
        }
        fn write_drive(&mut self, d: &[f32]) -> Result<usize, HalError> {
            Ok(d.len())
        }
        fn take_warnings(&mut self) -> Vec<String> {
            vec!["underrun".into()]
        }
        fn output_space(&mut self) -> Option<usize> {
            Some(512)
        }
        fn input_overloads(&mut self) -> Vec<bool> {
            vec![false, true]
        }
        fn poll_status(&mut self) -> BackendStatus {
            BackendStatus::default()
        }
        fn emergency_stop(&mut self) -> Result<(), HalError> {
            self.calls.lock().unwrap().push("emergency_stop");
            Ok(())
        }
    }

    type Calls = Arc<Mutex<Vec<&'static str>>>;

    fn probe() -> (Box<dyn Backend>, Arc<AtomicU8>, Calls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let signal = Arc::new(AtomicU8::new(NONE));
        let b = Interruptible::wrap(
            Box::new(Probe {
                calls: calls.clone(),
            }),
            signal.clone(),
        );
        (b, signal, calls)
    }

    #[test]
    fn with_no_signal_everything_passes_through_including_the_defaulted_methods() {
        let (mut b, _s, calls) = probe();
        let mut buf = [0f32; 1];
        assert!(matches!(b.read_input(&mut buf), Ok(InputRead::Frames(1))));
        assert_eq!(b.take_warnings(), vec!["underrun".to_owned()]);
        assert_eq!(b.output_space(), Some(512));
        assert_eq!(b.input_overloads(), vec![false, true]);
        assert_eq!(*calls.lock().unwrap(), ["read"]);
    }

    #[test]
    fn an_abort_takes_the_station_down_with_its_own_abort_stop_at_the_next_read() {
        let (mut b, s, calls) = probe();
        s.store(ABORT, Ordering::SeqCst);
        let mut buf = [0f32; 1];
        assert!(b.read_input(&mut buf).is_err());
        assert!(b.write_drive(&[0.0]).is_err(), "nothing more is driven");
        assert!(b.read_input(&mut buf).is_err());
        assert_eq!(*calls.lock().unwrap(), ["emergency_stop"]);
    }

    #[test]
    fn a_stop_uses_the_stations_controlled_stop() {
        let (mut b, s, calls) = probe();
        s.store(STOP, Ordering::SeqCst);
        let mut buf = [0f32; 1];
        assert!(b.read_input(&mut buf).is_err());
        assert_eq!(*calls.lock().unwrap(), ["stop"]);
    }

    #[test]
    fn ending_on_a_trip_gives_the_abort_stop_not_the_taper() {
        let (mut b, s, calls) = probe();
        s.store(ABORT, Ordering::SeqCst);
        b.stop().unwrap();
        b.stop().unwrap();
        assert_eq!(*calls.lock().unwrap(), ["emergency_stop", "stop"]);
    }
}
