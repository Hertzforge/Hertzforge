//! The 1 ms system timer the NI host loop needs on Windows.
//!
//! ## Why this exists
//!
//! DAQmx's default read wait mode is SLEEP with a 1 ms `ReadSleepTime`: while a
//! fixed-N read waits for its block, the driver sleeps 1 ms between buffer polls.
//! Linux honors a 1 ms sleep. Windows rounds every sleep UP to its system timer
//! period, which is 15.625 ms by default - so a read that should return at the
//! block boundary returns at the next 15.6 ms tick instead.
//!
//! At block 1024 (51.2 kHz, a 20 ms tick) that turns the read cadence into
//! alternating ~15.6 ms and ~31.3 ms gaps. Every third or fourth read lands more
//! than 1.5 periods after the last one, and the engine's period-jitter check stops
//! the run on `SchedulingFault(PeriodJitter)` by its third tick, every time.
//! Measured on a Windows 11 cDAQ-9178 bench (9234 AI only, no AO task, 30 s each,
//! 2026-11-01):
//!
//! | read wait                          | block | gaps over 1.5x | worst gap | CPU    |
//! |------------------------------------|-------|----------------|-----------|--------|
//! | Sleep (driver default)             | 1024  | 422 of 1500    | 32.6 ms   | 1.0 %  |
//! | Poll / Yield                       | 1024  | 0 of 1500      | 20.8 ms   | ~99 %  |
//! | Sleep + `timeBeginPeriod(1)`       | 1024  | 0 of 1500      | 22.5 ms   | 1.7 %  |
//! | Sleep + `timeBeginPeriod(1)`       | 512   | 0 of 3000      | 12.4 ms   | 1.4 %  |
//!
//! Poll and Yield fix it by spinning a whole core for the life of the run. A
//! per-process 1 ms timer fixes it at the CPU cost of the default, so that is the
//! lever: the read wait mode stays at the DRIVER DEFAULT, and the backend holds a
//! 1 ms timer request while its tasks are armed or running. The shutdown taper's
//! 200 us polls (`NiBackend::ramp_drive_to_zero`) round the same way, and run
//! inside the same hold.
//!
//! ## The two calls
//!
//! - `timeBeginPeriod(1)` / `timeEndPeriod(1)` (winmm). Since Windows 10 2004 the
//!   request is PER PROCESS, not a system-wide setting, and Windows REFCOUNTS it:
//!   every successful begin needs exactly one matching end, or the process keeps
//!   the raised resolution after the backend is gone. [`TimerHold`] owns one
//!   successful begin and ends it exactly once, on drop - and a begin that FAILED
//!   produces no hold, so nothing is ever ended that was not begun.
//! - `SetProcessInformation(ProcessPowerThrottling)` with `ControlMask =
//!   PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION` and `StateMask = 0`, once per
//!   process. Windows 11 may otherwise IGNORE a raised resolution while the
//!   process's window is minimized or fully occluded - and the desktop app runs
//!   the loop in-process, under its window. The bench measurement above came from
//!   a console process with no window, so it does not cover that case; this call
//!   is what makes the windowed case the same as the measured one. Older Windows
//!   does not know the information class; that failure is reported as a warning
//!   and the hold proceeds.
//!
//! Neither call changes anything outside this process. No power plan, no
//! system-wide timer setting, no priority class.
//!
//! The module is compiled on Windows and in tests only: the pairing rule is
//! proven on every host through [`TimerApi`], and no other platform carries any
//! of it.

/// The process-wide timer-period request, as two calls. A trait so the pairing
/// rule ([`TimerHold`]) is provable on a host without winmm.
pub(crate) trait TimerApi {
    /// `timeBeginPeriod(ms)`: `0` (`TIMERR_NOERROR`) on success, the MMRESULT
    /// error code otherwise.
    fn begin(&self, ms: u32) -> u32;
    /// `timeEndPeriod(ms)`: same return convention.
    fn end(&self, ms: u32) -> u32;
}

/// One SUCCESSFUL `begin(ms)`, ended exactly once when dropped.
///
/// Held in an `Option` by the backend: replacing or clearing the option drops the
/// old hold, so every path that releases it - stop, abort, close, a rebuild, a
/// `Drop` of the backend itself - ends exactly the begin it owned. There is no
/// separate "release" call to forget on an error path.
pub(crate) struct TimerHold<A: TimerApi> {
    api: A,
    ms: u32,
}

impl<A: TimerApi> TimerHold<A> {
    /// Request a `ms` timer period. `Err(code)` means the request was REFUSED:
    /// nothing is held, and nothing will be ended.
    pub(crate) fn acquire(api: A, ms: u32) -> Result<Self, u32> {
        match api.begin(ms) {
            0 => Ok(TimerHold { api, ms }),
            code => Err(code),
        }
    }

    /// The period this hold requested, in milliseconds.
    pub(crate) fn period_ms(&self) -> u32 {
        self.ms
    }
}

impl<A: TimerApi> Drop for TimerHold<A> {
    fn drop(&mut self) {
        // Best effort and panic-free. An end that fails cannot be retried
        // usefully (the refcount is the OS's), and a Drop must not panic.
        let _ = self.api.end(self.ms);
    }
}

impl<A: TimerApi> std::fmt::Debug for TimerHold<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimerHold").field("ms", &self.ms).finish()
    }
}

/// The real winmm calls.
#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Winmm;

#[cfg(windows)]
impl TimerApi for Winmm {
    fn begin(&self, ms: u32) -> u32 {
        // SAFETY: `timeBeginPeriod(UINT) -> MMRESULT` takes a plain integer and
        // touches no caller memory.
        unsafe { windows_sys::Win32::Media::timeBeginPeriod(ms) }
    }
    fn end(&self, ms: u32) -> u32 {
        // SAFETY: as above, `timeEndPeriod(UINT) -> MMRESULT`.
        unsafe { windows_sys::Win32::Media::timeEndPeriod(ms) }
    }
}

/// The hold the backend owns on Windows.
#[cfg(windows)]
pub(crate) type SystemTimerHold = TimerHold<Winmm>;

/// The period the backend requests: 1 ms, the value the bench measured.
pub(crate) const HOST_LOOP_TIMER_MS: u32 = 1;

/// Opt this process out of Windows 11's "ignore the timer resolution request
/// while the window is minimized or occluded" power throttling. Runs the call
/// ONCE per process and remembers the answer; `Err` carries the failure text for
/// the caller to report.
#[cfg(windows)]
pub(crate) fn honor_timer_resolution_when_occluded() -> Result<(), String> {
    static OUTCOME: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    OUTCOME
        .get_or_init(|| {
            use windows_sys::Win32::System::Threading::{
                GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation,
                PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
            };
            // ControlMask names the policy being set; StateMask 0 turns it OFF,
            // i.e. "always honor this process's timer resolution request".
            let state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
                StateMask: 0,
            };
            // SAFETY: the pseudo-handle from GetCurrentProcess needs no close;
            // `state` outlives the call and its size is passed exactly.
            let ok = unsafe {
                SetProcessInformation(
                    GetCurrentProcess(),
                    ProcessPowerThrottling,
                    std::ptr::from_ref(&state).cast(),
                    std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
                )
            };
            if ok != 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error().to_string())
            }
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// Counts begins and ends; `refuse` makes every begin fail with that code.
    #[derive(Clone, Default)]
    struct Counting {
        begins: Rc<Cell<u32>>,
        ends: Rc<Cell<u32>>,
        refuse: u32,
    }

    impl TimerApi for Counting {
        fn begin(&self, ms: u32) -> u32 {
            assert_eq!(ms, HOST_LOOP_TIMER_MS);
            self.begins.set(self.begins.get() + 1);
            self.refuse
        }
        fn end(&self, ms: u32) -> u32 {
            assert_eq!(ms, HOST_LOOP_TIMER_MS);
            self.ends.set(self.ends.get() + 1);
            0
        }
    }

    /// RED-MUTATION: delete `TimerHold`'s `Drop` and the end count stays 0 - the
    /// process would keep the 1 ms timer after every backend that ever armed.
    #[test]
    fn a_successful_begin_is_ended_exactly_once_on_drop() {
        let api = Counting::default();
        let hold = TimerHold::acquire(api.clone(), HOST_LOOP_TIMER_MS).expect("begin succeeds");
        assert_eq!(hold.period_ms(), 1);
        assert_eq!((api.begins.get(), api.ends.get()), (1, 0));
        drop(hold);
        assert_eq!((api.begins.get(), api.ends.get()), (1, 1));
    }

    /// RED-MUTATION: make `acquire` return a hold on a nonzero code and this reds -
    /// a refused begin would be "ended", decrementing a refcount some OTHER
    /// holder in the process owns.
    #[test]
    fn a_refused_begin_holds_nothing_and_ends_nothing() {
        let api = Counting {
            refuse: 97, // TIMERR_NOCANDO
            ..Default::default()
        };
        let r = TimerHold::acquire(api.clone(), HOST_LOOP_TIMER_MS);
        assert_eq!(r.err(), Some(97));
        assert_eq!((api.begins.get(), api.ends.get()), (1, 0));
    }

    /// The backend's shape: an `Option` that is set on arm and cleared on every
    /// release path, including being set again while still held. Whatever the
    /// sequence, begins and ends balance once the option is cleared.
    #[test]
    fn an_option_held_hold_balances_across_rearm_and_release() {
        fn arm(slot: &mut Option<TimerHold<Counting>>, api: &Counting) {
            *slot = TimerHold::acquire(api.clone(), HOST_LOOP_TIMER_MS).ok();
        }
        fn release(slot: &mut Option<TimerHold<Counting>>) {
            *slot = None;
        }
        let api = Counting::default();
        let mut slot: Option<TimerHold<Counting>> = None;
        // arm, stop, arm, re-arm over a live hold, abort, then close.
        arm(&mut slot, &api);
        release(&mut slot);
        arm(&mut slot, &api);
        arm(&mut slot, &api);
        assert_eq!((api.begins.get(), api.ends.get()), (3, 2));
        release(&mut slot);
        release(&mut slot);
        assert!(slot.is_none());
        assert_eq!((api.begins.get(), api.ends.get()), (3, 3));
    }
}
