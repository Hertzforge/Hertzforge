//! The epoch ticker: one background thread per [`crate::LawHost`] that
//! advances the engine's epoch every tick while a law call is in flight, so
//! the per-call deadline check runs. It parks when no call is active, so an
//! idle host costs no wakeups.

use crate::error::LawError;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;
use wasmtime::Engine;

#[derive(Default)]
struct State {
    active: usize,
    stop: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    // Nothing panics while holding this lock, but never let a poisoned lock
    // turn into a panic on a call path.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub(crate) struct Ticker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// Held for the length of one call; while any is held the ticker runs.
pub(crate) struct ActiveCall {
    shared: Arc<Shared>,
}

impl Drop for ActiveCall {
    fn drop(&mut self) {
        let mut st = self.shared.lock();
        st.active = st.active.saturating_sub(1);
    }
}

impl Ticker {
    pub(crate) fn start(engine: Engine, tick: Duration) -> Result<Self, LawError> {
        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("law-deadline-ticker".into())
            .spawn(move || run(&engine, &thread_shared, tick))
            .map_err(|e| LawError::Host(format!("could not start the deadline timer: {e}")))?;
        Ok(Ticker {
            shared,
            thread: Some(thread),
        })
    }

    /// Mark a call in flight until the returned guard drops.
    pub(crate) fn active(&self) -> ActiveCall {
        {
            let mut st = self.shared.lock();
            st.active = st.active.saturating_add(1);
        }
        self.shared.wake.notify_all();
        ActiveCall {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.wake.notify_all();
        if let Some(t) = self.thread.take() {
            // The thread only sleeps and increments; a join error would mean
            // it panicked, and there is nothing left to clean up.
            let _ = t.join();
        }
    }
}

fn run(engine: &Engine, shared: &Shared, tick: Duration) {
    loop {
        {
            let mut st = shared.lock();
            while st.active == 0 && !st.stop {
                st = shared.wake.wait(st).unwrap_or_else(PoisonError::into_inner);
            }
            if st.stop {
                return;
            }
        }
        std::thread::sleep(tick);
        engine.increment_epoch();
    }
}
