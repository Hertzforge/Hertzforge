//! The job engine (stated in three sentences there): ONE job at a
//! time; records are built OFF-lock and the session mutex is taken only at
//! commit; job status lives OUTSIDE the session in its own mutex. Progress
//! flows through a callback the tauri layer adapts onto `tauri::Emitter`
//! (`device_status.rs` precedent); bulk data never rides progress. Cancel is
//! a shared AtomicBool checked at record boundaries. Locks recover from
//! poisoning.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// The size policy.
pub const PROMPT_ABOVE_BYTES: u64 = 1 << 30; // 1 GiB
pub const REFUSE_ABOVE_BYTES: u64 = 4 << 30; // 4 GiB

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobPhase {
    Idle,
    Running,
    Cancelling,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatus {
    pub id: u64,
    pub phase: JobPhase,
    pub label: String,
    pub records_done: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub error: Option<String>,
}

impl Default for JobStatus {
    fn default() -> Self {
        JobStatus {
            id: 0,
            phase: JobPhase::Idle,
            label: String::new(),
            records_done: 0,
            bytes_done: 0,
            bytes_total: 0,
            error: None,
        }
    }
}

/// Recover a poisoned lock: the data under these locks
/// is status/session state whose invariants are re-validated on use; refusing
/// forever because a worker panicked once is worse than recovering.
pub fn lock_recover<'a, T>(m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

pub struct JobEngine {
    status: Arc<Mutex<JobStatus>>,
    cancel: Arc<AtomicBool>,
    handle: Mutex<Option<JoinHandle<()>>>,
    next_id: Mutex<u64>,
}

impl Default for JobEngine {
    fn default() -> Self {
        JobEngine {
            status: Arc::new(Mutex::new(JobStatus::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            handle: Mutex::new(None),
            next_id: Mutex::new(0),
        }
    }
}

impl JobEngine {
    pub fn status(&self) -> JobStatus {
        lock_recover(&self.status).clone()
    }

    pub fn cancel(&self) {
        let mut st = lock_recover(&self.status);
        if st.phase == JobPhase::Running {
            st.phase = JobPhase::Cancelling;
            self.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// Start a job unless one is running. `work` receives (progress, cancel)
    /// and returns Ok(records_done) or Err(message). `on_change` fires on
    /// every status transition and throttled progress (the Emitter adapter).
    pub fn start<W, C>(
        &self,
        label: &str,
        work: W,
        on_change: C,
    ) -> Result<u64, crate::jobs::BusyError>
    where
        W: FnOnce(&mut dyn FnMut(usize, u64, u64), &dyn Fn() -> bool) -> Result<usize, String>
            + Send
            + 'static,
        C: Fn(JobStatus) + Send + Sync + 'static,
    {
        {
            let st = lock_recover(&self.status);
            if matches!(st.phase, JobPhase::Running | JobPhase::Cancelling) {
                return Err(BusyError);
            }
        }
        // Reap the previous worker so handles never accumulate.
        if let Some(h) = lock_recover(&self.handle).take() {
            let _ = h.join();
        }
        let id = {
            let mut n = lock_recover(&self.next_id);
            *n += 1;
            *n
        };
        self.cancel.store(false, Ordering::SeqCst);
        {
            let mut st = lock_recover(&self.status);
            *st = JobStatus {
                id,
                phase: JobPhase::Running,
                label: label.into(),
                ..JobStatus::default()
            };
            on_change(st.clone());
        }
        let status = Arc::clone(&self.status);
        let cancel = Arc::clone(&self.cancel);
        let on_change = Arc::new(on_change);
        let oc = Arc::clone(&on_change);
        let handle = std::thread::Builder::new()
            .name(format!("insight-job-{id}"))
            .spawn(move || {
                let mut last_emit = std::time::Instant::now();
                let mut progress = |records: usize, done: u64, total: u64| {
                    let mut st = lock_recover(&status);
                    st.records_done = records;
                    st.bytes_done = done;
                    st.bytes_total = total;
                    // Throttle progress emissions to ~10 Hz.
                    if last_emit.elapsed().as_millis() >= 100 {
                        last_emit = std::time::Instant::now();
                        oc(st.clone());
                    }
                };
                let cancelled = || cancel.load(Ordering::SeqCst);
                let result = work(&mut progress, &cancelled);
                let mut st = lock_recover(&status);
                match result {
                    Ok(n) => {
                        st.records_done = n;
                        st.phase = if cancel.load(Ordering::SeqCst) {
                            JobPhase::Failed
                        } else {
                            JobPhase::Done
                        };
                        if st.phase == JobPhase::Failed {
                            st.error = Some("cancelled".into());
                        }
                    }
                    Err(e) => {
                        st.phase = JobPhase::Failed;
                        st.error = Some(e);
                    }
                }
                oc(st.clone());
            })
            .map_err(|_| BusyError)?;
        *lock_recover(&self.handle) = Some(handle);
        Ok(id)
    }

    /// Test/shutdown helper: wait for the current worker.
    pub fn join(&self) {
        if let Some(h) = lock_recover(&self.handle).take() {
            let _ = h.join();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusyError;

impl std::fmt::Display for BusyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "another job is already running")
    }
}

impl std::error::Error for BusyError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn single_job_policy_and_lifecycle() {
        let engine = JobEngine::default();
        let emissions = Arc::new(AtomicUsize::new(0));
        let e2 = Arc::clone(&emissions);
        let id = engine
            .start(
                "import",
                move |progress, _cancel| {
                    for i in 0..5 {
                        progress(i, i as u64 * 100, 500);
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Ok(5)
                },
                move |_st| {
                    e2.fetch_add(1, Ordering::SeqCst);
                },
            )
            .unwrap();
        assert!(id > 0);
        // Second start while running -> Busy.
        let busy = engine.start("again", |_, _| Ok(0), |_| {});
        assert!(busy.is_err());
        engine.join();
        let st = engine.status();
        assert_eq!(st.phase, JobPhase::Done);
        assert_eq!(st.records_done, 5);
        assert!(
            emissions.load(Ordering::SeqCst) >= 2,
            "start + terminal at minimum"
        );
        // After completion a new job starts fine.
        engine.start("next", |_, _| Ok(0), |_| {}).unwrap();
        engine.join();
    }

    #[test]
    fn cancel_reaches_the_worker_and_marks_failed() {
        let engine = JobEngine::default();
        engine
            .start(
                "long",
                |progress, cancel| {
                    for i in 0..1000 {
                        if cancel() {
                            return Ok(i);
                        }
                        progress(i, i as u64, 1000);
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                    Ok(1000)
                },
                |_| {},
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        engine.cancel();
        engine.join();
        let st = engine.status();
        assert_eq!(st.phase, JobPhase::Failed);
        assert_eq!(st.error.as_deref(), Some("cancelled"));
        assert!(st.records_done < 1000);
    }

    #[test]
    fn worker_error_reports_failed_with_message() {
        let engine = JobEngine::default();
        engine
            .start("bad", |_, _| Err("boom at byte 42".into()), |_| {})
            .unwrap();
        engine.join();
        let st = engine.status();
        assert_eq!(st.phase, JobPhase::Failed);
        assert!(st.error.as_deref().unwrap().contains("boom"));
    }
}
