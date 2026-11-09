//! The rails a transient run's thread runs on.
//!
//! The run thread owns the controller's service outright; the socket never reaches
//! it. Requests arrive on a [`Jobs`] queue the thread takes between pulses. The
//! thread publishes a view after every step into [`Shared`], whose every publish
//! reaches the service in order. Three things must work without the run thread,
//! because a fire blocks it for the whole record: Stop and Abort raise
//! [`Signals::cancel`], which the fire reads at every block; an absent controller
//! raises [`Signals::hold`], which keeps the next pulse from starting; and the fire's
//! progress is written into [`Progress`] from the per-block observer, for the live
//! frame. [`guarded`] runs each step under `catch_unwind`, so a panic in a step ends
//! the run in the emergency stop instead of taking the thread down with the drive
//! open.

use crate::protocol::{num, ErrorCode, MethodError};
use crate::random::run::LogEntry;
use serde_json::Value;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// How often a wait between pulses looks at its signals and its queue.
pub const POLL: Duration = Duration::from_millis(20);

/// A publish, as the service hears it: the run id and the view published.
pub type Notice<V> = (String, Arc<V>);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// The signals raised from outside the run thread.
#[derive(Debug, Clone, Default)]
pub struct Signals {
    /// Stop or abort: nothing more fires, and a fire in flight stops at its next
    /// block. Never lowered during a run.
    pub cancel: Arc<AtomicBool>,
    /// Abort rather than stop.
    pub abort: Arc<AtomicBool>,
    /// Nobody holds control: no pulse starts until it is lowered.
    pub hold: Arc<AtomicBool>,
    /// The inverse of `hold`, for the controller's own between-pulse checks, which
    /// read true to go on.
    pub go: Arc<AtomicBool>,
}

impl Signals {
    pub fn new() -> Signals {
        let s = Signals::default();
        s.go.store(true, Ordering::SeqCst);
        s
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn aborting(&self) -> bool {
        self.abort.load(Ordering::SeqCst)
    }

    pub fn held(&self) -> bool {
        self.hold.load(Ordering::SeqCst)
    }

    /// Raise the stop; with `abort`, the abort as well. An abort always wins.
    pub fn raise(&self, abort: bool) {
        if abort {
            self.abort.store(true, Ordering::SeqCst);
        }
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Hold firing (nobody holds control) or release it.
    pub fn set_hold(&self, hold: bool) {
        self.hold.store(hold, Ordering::SeqCst);
        self.go.store(!hold, Ordering::SeqCst);
    }
}

/// The fire in flight: frames played of the record, written from the per-block
/// observer.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicU64,
    of: AtomicU64,
    active: AtomicBool,
}

impl Progress {
    pub fn start(&self, of: u64) {
        self.done.store(0, Ordering::SeqCst);
        self.of.store(of, Ordering::SeqCst);
        self.active.store(true, Ordering::SeqCst);
    }

    pub fn block(&self, done: u64, of: u64) {
        self.done.store(done, Ordering::Relaxed);
        self.of.store(of, Ordering::Relaxed);
    }

    pub fn end(&self) {
        self.active.store(false, Ordering::SeqCst);
    }

    /// `(done, of)` while a fire is in flight.
    pub fn now(&self) -> Option<(u64, u64)> {
        self.active.load(Ordering::SeqCst).then(|| {
            (
                self.done.load(Ordering::Relaxed),
                self.of.load(Ordering::Relaxed),
            )
        })
    }
}

/// What the run thread and the service share.
pub struct Shared<V> {
    view: Mutex<Arc<V>>,
    log: Mutex<Vec<LogEntry>>,
    pub signals: Signals,
    pub progress: Arc<Progress>,
    /// Who sent the stop or abort, set with the signal so a run that ends inside a
    /// long call can name them.
    pub ended_by: Mutex<Option<String>>,
    /// How a stop raised from outside ends the run (`operator_stop`, `safe_stop`),
    /// set with the signal: the run thread may see the signal before it takes the
    /// queued request.
    stop_outcome: Mutex<Option<&'static str>>,
    /// Set once the service has started publishing this run's live frames.
    pub live_started: AtomicBool,
    notify: tokio::sync::mpsc::UnboundedSender<Notice<V>>,
}

fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl<V> Shared<V> {
    pub fn new(initial: V, notify: tokio::sync::mpsc::UnboundedSender<Notice<V>>) -> Shared<V> {
        Shared {
            view: Mutex::new(Arc::new(initial)),
            log: Mutex::new(Vec::new()),
            signals: Signals::new(),
            progress: Arc::new(Progress::default()),
            ended_by: Mutex::new(None),
            stop_outcome: Mutex::new(None),
            live_started: AtomicBool::new(false),
            notify,
        }
    }

    pub fn view(&self) -> Arc<V> {
        lock(&self.view).clone()
    }

    /// Publish `view` and tell the service, in order.
    pub fn publish(&self, run_id: &str, view: V) {
        let view = Arc::new(view);
        *lock(&self.view) = view.clone();
        let _ = self.notify.send((run_id.to_owned(), view));
    }

    pub fn add_log(&self, kind: &'static str, text: impl Into<String>) {
        let mut log = lock(&self.log);
        let seq = log.len() as u64;
        log.push(LogEntry {
            seq,
            at_unix: now_unix(),
            kind,
            text: text.into(),
        });
    }

    pub fn log_since(&self, since: u64) -> (Vec<LogEntry>, u64) {
        let log = lock(&self.log);
        let next = log.len() as u64;
        let from = (since as usize).min(log.len());
        (log[from..].to_vec(), next)
    }

    pub fn set_ended_by(&self, by: &str) {
        let mut b = lock(&self.ended_by);
        if b.is_none() {
            *b = Some(by.to_owned());
        }
    }

    pub fn ended_by(&self) -> Option<String> {
        lock(&self.ended_by).clone()
    }

    /// Name how a stop ends the run, before its signal is raised (the first wins).
    pub fn set_stop_outcome(&self, outcome: &'static str) {
        let mut o = lock(&self.stop_outcome);
        if o.is_none() {
            *o = Some(outcome);
        }
    }

    pub fn stop_outcome(&self) -> Option<&'static str> {
        *lock(&self.stop_outcome)
    }
}

/// The run thread's job queue.
pub struct Jobs<C> {
    run_id: String,
    tx: Mutex<mpsc::Sender<C>>,
}

impl<C> Jobs<C> {
    pub fn new(run_id: &str, tx: mpsc::Sender<C>) -> Jobs<C> {
        Jobs {
            run_id: run_id.to_owned(),
            tx: Mutex::new(tx),
        }
    }

    pub fn send(&self, cmd: C) -> Result<(), MethodError> {
        lock(&self.tx)
            .send(cmd)
            .map_err(|_| MethodError::internal(format!("run {} is no longer running", self.run_id)))
    }

    /// Queue a request and wait for its answer, at most `wait`.
    pub async fn ask<T: Send + 'static>(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<T, MethodError>>) -> C,
        wait: Duration,
    ) -> Result<T, MethodError> {
        let (tx, rx) = oneshot::channel();
        self.send(make(tx))?;
        match tokio::time::timeout(wait, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(MethodError::internal(format!(
                "run {} ended before it answered",
                self.run_id
            ))),
            Err(_) => Err(MethodError::new(
                ErrorCode::Internal,
                format!(
                    "run {} did not answer within {} s; the request stands",
                    self.run_id,
                    num(wait.as_secs_f64().round())
                ),
            )),
        }
    }
}

/// Run `f`, turning a panic into its message.
pub fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        p.downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic with no message".to_owned())
    })
}

/// How far ahead of an `auto` pulse's due time a run thread asks the controller
/// for it, from the last pulse's bring-up: twice the work the controller does
/// before its own wait (the drive's checks, the arm, the commit) and 50 ms,
/// held between 50 ms and 1 s. The controller then waits out the rest, so the
/// bring-up is done inside the time between pulses and only the start lands on
/// the due time. Nothing is known before the first pulse, which is due at once.
pub fn lead_for(bring_up: Option<hertzforge_shock_common::BringUp>) -> Duration {
    match bring_up {
        Some(b) => {
            let ms = (2.0 * b.ahead_ms() + 50.0).clamp(50.0, 1000.0);
            Duration::from_micros((ms * 1e3).round() as u64)
        }
        None => Duration::ZERO,
    }
}

/// How a wait for the next pulse ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waited {
    /// The pulse is due and firing is not held.
    Due,
    /// Stop or abort was raised.
    Cancelled,
    /// `between` asked to stop waiting (a request it took changed the run).
    Interrupted,
}

/// Wait until `deadline`, and past it while firing is held, looking at the signals
/// and calling `between` every [`POLL`]. `between` takes the run's queued requests;
/// it returns false to end the wait.
pub fn wait_until(
    deadline: Instant,
    signals: &Signals,
    mut between: impl FnMut() -> bool,
) -> Waited {
    loop {
        if signals.cancelled() {
            return Waited::Cancelled;
        }
        if !between() {
            return Waited::Interrupted;
        }
        if signals.cancelled() {
            return Waited::Cancelled;
        }
        let now = Instant::now();
        if now >= deadline && !signals.held() {
            return Waited::Due;
        }
        let left = deadline.saturating_duration_since(now);
        std::thread::sleep(if left.is_zero() { POLL } else { left.min(POLL) });
    }
}

/// A value for a log line or message, rounded for a person.
pub fn words(x: f64, digits: i32) -> Value {
    let p = 10f64.powi(digits);
    num((x * p).round() / p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lead_is_twice_the_work_before_the_wait_and_50_ms_held_to_50_ms_and_1_s() {
        use hertzforge_shock_common::BringUp;
        let b = |ahead_ms: f64| BringUp {
            checks_ms: 0.0,
            arm_ms: ahead_ms,
            commit_ms: 0.0,
            wait_ms: 4000.0,
            start_ms: 2.0,
        };
        assert_eq!(lead_for(None), Duration::ZERO);
        assert_eq!(lead_for(Some(b(125.0))), Duration::from_millis(300));
        assert_eq!(lead_for(Some(b(0.0))), Duration::from_millis(50));
        assert_eq!(lead_for(Some(b(2000.0))), Duration::from_secs(1));
    }

    #[test]
    fn a_wait_ends_on_its_deadline_on_a_stop_or_when_its_caller_asks() {
        let s = Signals::new();
        let t0 = Instant::now();
        assert_eq!(
            wait_until(t0 + Duration::from_millis(30), &s, || true),
            Waited::Due
        );
        assert!(t0.elapsed() >= Duration::from_millis(30));
        let mut n = 0;
        assert_eq!(
            wait_until(t0 + Duration::from_secs(10), &s, || {
                n += 1;
                n < 3
            }),
            Waited::Interrupted
        );
        let s2 = s.clone();
        let raiser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            s2.raise(false);
        });
        let t1 = Instant::now();
        assert_eq!(
            wait_until(t1 + Duration::from_secs(10), &s, || true),
            Waited::Cancelled
        );
        assert!(t1.elapsed() < Duration::from_secs(1));
        raiser.join().unwrap();
    }

    #[test]
    fn a_held_wait_does_not_fire_past_its_deadline_until_released() {
        let s = Signals::new();
        s.set_hold(true);
        assert!(!s.go.load(Ordering::SeqCst));
        let s2 = s.clone();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            s2.set_hold(false);
        });
        let t0 = Instant::now();
        assert_eq!(wait_until(t0, &s, || true), Waited::Due);
        assert!(t0.elapsed() >= Duration::from_millis(80));
        releaser.join().unwrap();
        assert!(s.go.load(Ordering::SeqCst));
    }

    #[test]
    fn a_panic_is_caught_with_its_words() {
        let r: Result<(), String> = guarded(|| panic!("the drive went wrong"));
        assert_eq!(r.unwrap_err(), "the drive went wrong");
        assert_eq!(guarded(|| 7).unwrap(), 7);
    }

    #[test]
    fn the_progress_is_none_between_fires() {
        let p = Progress::default();
        assert_eq!(p.now(), None);
        p.start(8192);
        p.block(1024, 8192);
        assert_eq!(p.now(), Some((1024, 8192)));
        p.end();
        assert_eq!(p.now(), None);
    }
}
