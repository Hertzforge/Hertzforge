//! The control thread's scheduling class on Windows: MMCSS "Pro Audio".
//!
//! ## Why this exists
//!
//! With the 1 ms timer held (`crate::timer_resolution`) the read cadence is right
//! on average, but on Windows a normal-priority reading thread is still preempted
//! by ordinary background work often enough to stop a long run. Measured on a
//! Windows 11 cDAQ-9178 bench (9234 AI only, block 1024 at 51.2 kHz, a 20 ms tick,
//! driver-default Sleep wait, 1 ms timer held, 30 minutes per row, 2026-11-01):
//!
//! | reading thread               | reads over 1.25x | reads over 1.5x | worst gap |
//! |------------------------------|------------------|-----------------|-----------|
//! | default priority             | 6                | 2               | 42.5 ms   |
//! | MMCSS "Pro Audio" registered | 0                | 0               | 22.6 ms   |
//!
//! The late reads lined up with Search indexing; the registered run was under
//! HEAVIER background load (disk cleanup, malware scan, native image generation)
//! and had none. A read later than 1.5 periods is the engine's
//! `SchedulingFault(PeriodJitter)` stop, so each of the default-priority row's two
//! would have ended an engine run. The late-tick policy is unchanged by this
//! module: it prevents the late reads, it does not forgive them.
//!
//! ## Which thread, and for how long
//!
//! The thread that READS is the control loop's thread: every module pulls its
//! input blocks on the thread that runs its loop (the engine's tick, the TWR
//! player, the SRS and shock firers), and the read wait is where the late ticks
//! were measured. So the backend registers the thread that makes the first read
//! of a run, while the tasks are running, and nothing earlier: arm, prefill and
//! start may run on a command thread that never reads.
//!
//! The registration ends when the run ends ON THAT THREAD (stop, abort, close, a
//! rebuild, or the backend's `Drop`) or when the thread exits, whichever comes
//! first. A run stopped from another thread (a daemon's stop command) keeps its
//! reading thread registered until that thread exits; in this tree a reading
//! thread either stops its own run or is a per-run pump that exits when the run
//! ends, so that is shortly after.
//!
//! ## The pairing rule
//!
//! `AvRevertMmThreadCharacteristics` must be called from the thread that called
//! `AvSetMmThreadCharacteristicsW`; from any other thread it fails (the function's
//! documentation says so). The backend is `Send` and its stop
//! or `Drop` may run anywhere, so the registration does NOT live in the backend.
//! It lives in a thread-local slot on the registering thread:
//!
//! - [`Registration`] owns one successful registration and reverts it exactly
//!   once, on drop. It is `!Send`, so it cannot leave its thread.
//! - [`ThreadSlot`] holds at most one, tagged with the run that owns it. A run's
//!   end reverts it only on the owning thread; signalled from another thread it
//!   does nothing there, and the slot's own drop at thread exit reverts it. A
//!   panic that ends the thread is the same thread exit.
//! - A refusal holds nothing and is never reverted. It is recorded for the run so
//!   the run warns once, not once per read.
//!
//! A failed registration never refuses a run: the run proceeds at the priority
//! it had before this existed, and the backend reports the refusal through its
//! warning channel.
//!
//! The module is compiled on Windows and in tests only. The pairing rule is
//! proven on every host through [`MmcssApi`]; no other platform carries any of it.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

/// The MMCSS task the control thread joins: the class the bench measured.
pub(crate) const MMCSS_TASK: &str = "Pro Audio";

/// The two MMCSS calls. A trait so the pairing rule is provable on a host
/// without avrt.
pub(crate) trait MmcssApi: Sync {
    /// `AvSetMmThreadCharacteristicsW(MMCSS_TASK)` for the CALLING thread:
    /// `Ok(handle)` on success, `Err(GetLastError())` otherwise.
    fn register(&self) -> Result<usize, u32>;
    /// `AvRevertMmThreadCharacteristics(handle)` on the CALLING thread, which
    /// must be the one that registered. `true` on success.
    fn revert(&self, handle: usize) -> bool;
}

/// One SUCCESSFUL registration of the current thread, reverted exactly once
/// when dropped.
///
/// `!Send` (the `PhantomData<*const ()>`): the revert is only valid on the
/// registering thread, so the type cannot be moved off it. A registration that
/// FAILED produces no value, so nothing is ever reverted that was not
/// registered.
pub(crate) struct Registration {
    api: &'static dyn MmcssApi,
    handle: usize,
    _thread_bound: PhantomData<*const ()>,
}

impl Registration {
    /// Register the calling thread. `Err(code)` means the OS refused: nothing is
    /// held, and nothing will be reverted.
    fn acquire(api: &'static dyn MmcssApi) -> Result<Self, u32> {
        api.register().map(|handle| Registration {
            api,
            handle,
            _thread_bound: PhantomData,
        })
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // Best effort and panic-free: a failed revert cannot be retried usefully,
        // and a Drop must not panic (this one also runs during thread exit).
        let _ = self.api.revert(self.handle);
    }
}

/// What a thread holds, and for which run.
struct Held {
    run: u64,
    /// `None`: this run's registration on this thread was REFUSED. Kept so the
    /// run does not retry (and warn) on every read.
    registration: Option<Registration>,
}

/// The per-thread state: at most one registration, tagged with its run.
#[derive(Default)]
pub(crate) struct ThreadSlot {
    held: Option<Held>,
}

/// What [`ThreadSlot::enter`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Entered {
    /// This call registered the thread.
    Registered,
    /// The thread was already registered: by this run, or by an earlier run whose
    /// end was signalled from another thread (this run adopts it).
    AlreadyRegistered,
    /// This run was already refused on this thread; not retried.
    AlreadyRefused,
    /// The OS refused this call (the code is `GetLastError()`).
    Refused(u32),
}

impl ThreadSlot {
    /// The read path: make sure the current thread is registered for `run`.
    /// At most one registration attempt per run per thread.
    pub(crate) fn enter(&mut self, run: u64, api: &'static dyn MmcssApi) -> Entered {
        if let Some(held) = &mut self.held {
            if held.registration.is_some() {
                // Already elevated. Adopting is the same effect with no second
                // registration on the thread.
                held.run = run;
                return Entered::AlreadyRegistered;
            }
            if held.run == run {
                return Entered::AlreadyRefused;
            }
            // A refusal recorded for an EARLIER run: this run gets its own try.
        }
        let (registration, entered) = match Registration::acquire(api) {
            Ok(r) => (Some(r), Entered::Registered),
            Err(code) => (None, Entered::Refused(code)),
        };
        self.held = Some(Held { run, registration });
        entered
    }

    /// The end of `run`, signalled on the current thread. Returns what this thread
    /// held for `run` (the caller drops it, which reverts a registration), or
    /// `None` when this thread holds nothing for that run.
    fn leave(&mut self, run: u64) -> Option<Held> {
        if self.held.as_ref().is_some_and(|h| h.run == run) {
            self.held.take()
        } else {
            None
        }
    }
}

thread_local! {
    /// This thread's registration. Its drop at thread exit is the revert for a run
    /// whose end was signalled elsewhere, and for a thread a panic ended.
    static SLOT: RefCell<ThreadSlot> = const { RefCell::new(ThreadSlot { held: None }) };
}

/// [`ThreadSlot::enter`] on the current thread. `None` only while the thread is
/// being torn down (the slot is gone), in which case nothing is registered.
pub(crate) fn enter_this_thread(run: u64, api: &'static dyn MmcssApi) -> Option<Entered> {
    SLOT.try_with(|slot| slot.try_borrow_mut().ok().map(|mut s| s.enter(run, api)))
        .ok()
        .flatten()
}

/// The end of `run`, on the current thread: reverts the registration if this
/// thread holds it for `run`, and does nothing otherwise. Returns whether this
/// thread held anything for `run`.
pub(crate) fn leave_this_thread(run: u64) -> bool {
    let held = SLOT
        .try_with(|slot| slot.try_borrow_mut().ok().and_then(|mut s| s.leave(run)))
        .ok()
        .flatten();
    // Dropped here, outside the slot's borrow: the revert runs now, on this thread.
    held.is_some()
}

/// A process-unique run id. Never 0.
fn next_run() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The backend's side: which run is live, and which API registers it.
///
/// The run id is taken at the run's first running read and released at its end
/// (from any thread), so the next run is a new id and gets its own attempt.
pub(crate) struct RunPriority {
    api: Option<&'static dyn MmcssApi>,
    run: Option<u64>,
}

impl RunPriority {
    /// The real MMCSS calls on Windows. In this crate's own tests, nothing:
    /// a test installs a counting API with [`RunPriority::with_api`].
    pub(crate) fn system() -> Self {
        RunPriority {
            api: system_api(),
            run: None,
        }
    }

    /// A backend hook over `api` (tests).
    #[cfg(test)]
    pub(crate) fn with_api(api: &'static dyn MmcssApi) -> Self {
        RunPriority {
            api: Some(api),
            run: None,
        }
    }

    /// Called on every read while the tasks are RUNNING, on the reading thread.
    /// Registers that thread on the run's first read there; `Some(warning)` when
    /// the OS refused (once per run per thread).
    pub(crate) fn on_running_read(&mut self) -> Option<String> {
        let api = self.api?;
        let run = *self.run.get_or_insert_with(next_run);
        match enter_this_thread(run, api)? {
            Entered::Refused(code) => Some(refusal_warning(code)),
            Entered::Registered | Entered::AlreadyRegistered | Entered::AlreadyRefused => None,
        }
    }

    /// The run has ended (stop, abort, close, rebuild, `Drop`), on whatever
    /// thread called it. On the registering thread this reverts now; elsewhere
    /// the registering thread reverts when it exits.
    pub(crate) fn end_run(&mut self) {
        if let Some(run) = self.run.take() {
            leave_this_thread(run);
        }
    }
}

/// The warning a refused registration puts in the backend's warning channel.
pub(crate) fn refusal_warning(code: u32) -> String {
    format!(
        "the control thread could not join the Windows multimedia scheduling class \
         (AvSetMmThreadCharacteristicsW(\"{MMCSS_TASK}\") refused: {}); the run continues \
         at normal priority, where background load can delay a read past 1.5 block \
         periods and stop the run on period jitter",
        std::io::Error::from_raw_os_error(code as i32)
    )
}

#[cfg(all(windows, not(test)))]
fn system_api() -> Option<&'static dyn MmcssApi> {
    Some(&Avrt)
}

#[cfg(test)]
fn system_api() -> Option<&'static dyn MmcssApi> {
    None
}

/// The real avrt calls.
#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Avrt;

#[cfg(windows)]
impl MmcssApi for Avrt {
    fn register(&self) -> Result<usize, u32> {
        use windows_sys::Win32::System::Threading::AvSetMmThreadCharacteristicsW;
        let name: Vec<u16> = MMCSS_TASK
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // Zero on input: this is a first registration of the thread, not a join
        // of an existing task index.
        let mut task_index: u32 = 0;
        // SAFETY: `name` is NUL-terminated UTF-16 that outlives the call, and
        // `task_index` is a valid, initialized out-pointer.
        let handle = unsafe { AvSetMmThreadCharacteristicsW(name.as_ptr(), &mut task_index) };
        if handle.is_null() {
            Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32)
        } else {
            Ok(handle.expose_provenance())
        }
    }

    fn revert(&self, handle: usize) -> bool {
        use windows_sys::Win32::System::Threading::AvRevertMmThreadCharacteristics;
        // SAFETY: `handle` came from a successful AvSetMmThreadCharacteristicsW on
        // THIS thread (a `Registration` is !Send and lives in this thread's slot),
        // and each one is reverted once, by its drop.
        unsafe {
            AvRevertMmThreadCharacteristics(std::ptr::with_exposed_provenance_mut(handle)) != 0
        }
    }
}

/// A counting MMCSS API for this crate's tests (here and the backend's wiring
/// tests).
#[cfg(test)]
pub(crate) mod fake {
    use super::MmcssApi;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;
    use std::thread::ThreadId;

    /// Counts registrations and reverts and records the thread of each. `refuse`
    /// makes every registration fail with that code.
    #[derive(Default)]
    pub(crate) struct Counting {
        registers: AtomicU32,
        reverts: AtomicU32,
        refuse: u32,
        pub(crate) registered_on: Mutex<Vec<ThreadId>>,
        pub(crate) reverted_on: Mutex<Vec<ThreadId>>,
    }

    impl Counting {
        /// A fresh counter with a `'static` lifetime (the API is held as one).
        pub(crate) fn leak(refuse: u32) -> &'static Counting {
            Box::leak(Box::new(Counting {
                refuse,
                ..Default::default()
            }))
        }
        /// `(registrations attempted, reverts)`.
        pub(crate) fn counts(&self) -> (u32, u32) {
            (
                self.registers.load(Ordering::SeqCst),
                self.reverts.load(Ordering::SeqCst),
            )
        }
    }

    impl MmcssApi for Counting {
        fn register(&self) -> Result<usize, u32> {
            let n = self.registers.fetch_add(1, Ordering::SeqCst) + 1;
            self.registered_on
                .lock()
                .unwrap()
                .push(std::thread::current().id());
            match self.refuse {
                0 => Ok(0x7000 + n as usize),
                code => Err(code),
            }
        }
        fn revert(&self, _handle: usize) -> bool {
            self.reverts.fetch_add(1, Ordering::SeqCst);
            self.reverted_on
                .lock()
                .unwrap()
                .push(std::thread::current().id());
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Counting;
    use super::*;

    /// RED-MUTATION (run 2026-11-01): make `ThreadSlot::enter` ignore what the
    /// thread already holds and the second read registers again; delete
    /// `Registration`'s `Drop` and the revert count stays 0.
    #[test]
    fn a_run_registers_its_thread_once_and_reverts_once() {
        let api = Counting::leak(0);
        let mut slot = ThreadSlot::default();
        assert_eq!(slot.enter(7, api), Entered::Registered);
        for _ in 1..1000 {
            assert_eq!(slot.enter(7, api), Entered::AlreadyRegistered);
        }
        assert_eq!(api.counts(), (1, 0));
        drop(slot.leave(7));
        assert_eq!(api.counts(), (1, 1));
        // A second end of the same run finds nothing to revert.
        assert!(slot.leave(7).is_none());
        assert_eq!(api.counts(), (1, 1));
    }

    /// RED-MUTATION (run 2026-11-01): make `Registration::acquire` return a value
    /// on a refusal and the first call reports Registered: the refusal is never
    /// seen, and a handle the OS never issued would later be "reverted".
    #[test]
    fn a_refusal_is_reported_once_holds_nothing_and_reverts_nothing() {
        let api = Counting::leak(1552); // ERROR_INVALID_TASK_NAME
        let mut slot = ThreadSlot::default();
        assert_eq!(slot.enter(3, api), Entered::Refused(1552));
        for _ in 0..99 {
            assert_eq!(slot.enter(3, api), Entered::AlreadyRefused);
        }
        assert_eq!(
            api.counts(),
            (1, 0),
            "one attempt per run, not one per read"
        );
        drop(slot.leave(3));
        assert_eq!(api.counts(), (1, 0));
    }

    /// Two runs back to back on one thread: one registration and one revert each.
    #[test]
    fn consecutive_runs_on_one_thread_pair_separately() {
        let api = Counting::leak(0);
        let mut slot = ThreadSlot::default();
        assert_eq!(slot.enter(1, api), Entered::Registered);
        drop(slot.leave(1));
        assert_eq!(slot.enter(2, api), Entered::Registered);
        drop(slot.leave(2));
        assert_eq!(api.counts(), (2, 2));
    }

    /// A run whose end was signalled on another thread leaves its registration in
    /// place. A later run on the same thread adopts it rather than registering a
    /// second time, and the earlier run's id no longer reverts it.
    #[test]
    fn a_later_run_adopts_a_registration_it_finds() {
        let api = Counting::leak(0);
        let mut slot = ThreadSlot::default();
        assert_eq!(slot.enter(10, api), Entered::Registered);
        assert_eq!(slot.enter(11, api), Entered::AlreadyRegistered);
        assert!(slot.leave(10).is_none(), "run 10 no longer owns it");
        assert_eq!(api.counts(), (1, 0));
        drop(slot.leave(11));
        assert_eq!(api.counts(), (1, 1));
    }

    /// A refusal recorded for one run does not stop the next run from trying.
    #[test]
    fn a_refusal_is_per_run() {
        let api = Counting::leak(5);
        let mut slot = ThreadSlot::default();
        assert_eq!(slot.enter(20, api), Entered::Refused(5));
        assert_eq!(slot.enter(21, api), Entered::Refused(5));
        assert_eq!(api.counts(), (2, 0));
    }

    /// The thread-exit path: a run that never signalled its end is reverted by the
    /// slot's drop when its thread exits, ON that thread.
    ///
    /// RED-MUTATION (run 2026-11-01): delete `Registration`'s `Drop` and the
    /// revert count after the join is 0.
    #[test]
    fn a_thread_that_exits_mid_run_reverts_on_its_way_out() {
        let api = Counting::leak(0);
        let t = std::thread::spawn(move || {
            assert_eq!(enter_this_thread(30, api), Some(Entered::Registered));
            std::thread::current().id()
        });
        let id = t.join().unwrap();
        assert_eq!(api.counts(), (1, 1));
        assert_eq!(*api.reverted_on.lock().unwrap(), vec![id]);
    }

    /// The panic path: a reading thread that panics mid-run is reverted on its
    /// way out, the same as any other thread exit.
    #[test]
    fn a_reading_thread_that_panics_is_reverted() {
        let api = Counting::leak(0);
        let t = std::thread::spawn(move || {
            assert_eq!(enter_this_thread(40, api), Some(Entered::Registered));
            panic!("a fault inside the control loop");
        });
        assert!(t.join().is_err());
        assert_eq!(api.counts(), (1, 1));
    }

    /// The pairing rule itself: the end of a run signalled from ANOTHER thread
    /// reverts nothing there (the OS would refuse that revert), and the registering
    /// thread reverts its own registration when it exits.
    ///
    /// The design this rules out, a registration kept in the `Send` backend and
    /// reverted by whichever thread stops it, does not compile: `Registration` is
    /// `!Send`. This test pins the behavior that design would get wrong.
    #[test]
    fn a_run_ended_from_another_thread_is_reverted_by_its_own_thread() {
        let api = Counting::leak(0);
        let (registered_tx, registered_rx) = std::sync::mpsc::channel();
        let (exit_tx, exit_rx) = std::sync::mpsc::channel::<()>();
        let reader = std::thread::spawn(move || {
            assert_eq!(enter_this_thread(50, api), Some(Entered::Registered));
            registered_tx.send(()).unwrap();
            exit_rx.recv().unwrap();
            std::thread::current().id()
        });
        registered_rx.recv().unwrap();
        // The stop arrives on this thread.
        assert!(
            !leave_this_thread(50),
            "this thread holds nothing for run 50"
        );
        assert_eq!(api.counts(), (1, 0), "no revert off the registering thread");
        exit_tx.send(()).unwrap();
        let reader_id = reader.join().unwrap();
        assert_eq!(api.counts(), (1, 1));
        assert_eq!(*api.registered_on.lock().unwrap(), vec![reader_id]);
        assert_eq!(*api.reverted_on.lock().unwrap(), vec![reader_id]);
    }

    /// The backend hook: a run id is taken at the first running read and released
    /// at the run's end, so each run registers once and reverts once, and a run
    /// refused on one thread warns once.
    #[test]
    fn the_run_hook_pairs_per_run_and_warns_once_per_refused_run() {
        let api = Counting::leak(0);
        let mut hook = RunPriority::with_api(api);
        for _ in 0..50 {
            assert_eq!(hook.on_running_read(), None);
        }
        hook.end_run();
        hook.end_run(); // a second end (stop, then Drop) is harmless
        assert_eq!(api.counts(), (1, 1));
        assert_eq!(hook.on_running_read(), None);
        hook.end_run();
        assert_eq!(api.counts(), (2, 2));

        let refusing = Counting::leak(1552);
        let mut hook = RunPriority::with_api(refusing);
        let warnings: Vec<String> = (0..50).filter_map(|_| hook.on_running_read()).collect();
        assert_eq!(warnings.len(), 1, "one warning per run, not per read");
        assert!(warnings[0].contains("Pro Audio"), "{}", warnings[0]);
        hook.end_run();
        assert!(hook.on_running_read().is_some(), "the next run tries again");
        hook.end_run();
        assert_eq!(refusing.counts(), (2, 0));
    }

    /// No API (this crate's own tests, unless a test installs one): nothing is
    /// registered and nothing warns.
    #[test]
    fn a_hook_without_an_api_does_nothing() {
        let mut hook = RunPriority::system();
        assert_eq!(hook.on_running_read(), None);
        hook.end_run();
    }

    /// WINDOWS: the real calls pair on one thread. Whether the OS grants the
    /// registration depends on the host (the multimedia scheduler service can be
    /// off on a server image), so a refusal passes; a granted registration must
    /// revert successfully on the thread that made it.
    #[cfg(windows)]
    #[test]
    fn the_real_registration_reverts_on_its_own_thread() {
        struct Recording {
            reverted_ok: std::sync::Mutex<Vec<bool>>,
        }
        impl MmcssApi for Recording {
            fn register(&self) -> Result<usize, u32> {
                Avrt.register()
            }
            fn revert(&self, handle: usize) -> bool {
                let ok = Avrt.revert(handle);
                self.reverted_ok.lock().unwrap().push(ok);
                ok
            }
        }
        let api: &'static Recording = Box::leak(Box::new(Recording {
            reverted_ok: std::sync::Mutex::new(Vec::new()),
        }));
        let entered = std::thread::spawn(move || {
            let entered = enter_this_thread(1, api);
            assert!(leave_this_thread(1));
            entered
        })
        .join()
        .unwrap();
        match entered {
            Some(Entered::Registered) => {
                assert_eq!(*api.reverted_ok.lock().unwrap(), vec![true]);
            }
            Some(Entered::Refused(_)) => {
                assert!(api.reverted_ok.lock().unwrap().is_empty());
            }
            other => panic!("unexpected outcome {other:?}"),
        }
    }
}
