//! What a law runs inside: one Wasmtime store per instance, holding a WASI
//! context that grants nothing, a memory cap, a per-call deadline, frozen
//! clocks, seeded random sources and a bounded capture of stdout and stderr.

use crate::error::LawError;
use crate::types::GuestOutput;
use rand::rngs::StdRng;
use rand::SeedableRng;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use wasmtime::component::{HasData, Linker, Resource, ResourceTable};
use wasmtime::{Engine, ResourceLimiter, Store, Trap, UpdateDeadline};
use wasmtime_wasi::cli::{IsTerminal, StdoutStream};
use wasmtime_wasi::clocks::{HostMonotonicClock, HostWallClock};
use wasmtime_wasi::p2::bindings::clocks::monotonic_clock;
use wasmtime_wasi::p2::{DynPollable, OutputStream, Pollable, StreamResult};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

/// The fixed seeds behind the guest's random sources. Any fixed values do;
/// what matters is that every instance gets the same ones.
const SECURE_SEED: u64 = 0x4865_7274_7a66_6f72;
const INSECURE_SEED: u64 = 0x6765_2d6c_6177_2d31;
const INSECURE_SEED_128: u128 = 0x636f_6e74_726f_6c2d_6c61_7773_2d30_2e31;
/// Largest single random request the host serves (it allocates the reply).
const MAX_RANDOM_BYTES: u64 = 1 << 20;
/// Largest table a law may grow, in elements.
const MAX_TABLE_ELEMENTS: usize = 100_000;
/// How much the capture accepts per write permit (it never blocks).
const WRITE_PERMIT: usize = 64 * 1024;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// Clocks: time does not pass for a law
// ---------------------------------------------------------------------------

/// The wall clock reads the Unix epoch, always.
struct FrozenWallClock;

impl HostWallClock for FrozenWallClock {
    fn resolution(&self) -> Duration {
        Duration::from_nanos(1)
    }
    fn now(&self) -> Duration {
        Duration::ZERO
    }
}

/// The monotonic clock reads zero, always.
struct FrozenMonotonicClock;

impl HostMonotonicClock for FrozenMonotonicClock {
    fn resolution(&self) -> u64 {
        1
    }
    fn now(&self) -> u64 {
        0
    }
}

/// A pollable that is ready at once.
struct Ready;

#[async_trait::async_trait]
impl Pollable for Ready {
    async fn ready(&mut self) {}
}

/// The `wasi:clocks/monotonic-clock` the host links in place of the stock
/// one. The stock one reads the frozen clock but sleeps in real time, inside
/// a host call where the epoch deadline cannot reach; a law that slept would
/// hold the loop thread past its budget. Here every sleep is ready at once,
/// which is also what a frozen clock implies.
struct FrozenMonotonic;

impl HasData for FrozenMonotonic {
    type Data<'a> = FrozenMonotonicView<'a>;
}

struct FrozenMonotonicView<'a> {
    table: &'a mut ResourceTable,
}

impl FrozenMonotonicView<'_> {
    fn ready(&mut self) -> wasmtime::Result<Resource<DynPollable>> {
        let r = self.table.push(Ready)?;
        wasmtime_wasi::p2::subscribe(self.table, r)
    }
}

impl monotonic_clock::Host for FrozenMonotonicView<'_> {
    fn now(&mut self) -> wasmtime::Result<monotonic_clock::Instant> {
        Ok(FrozenMonotonicClock.now())
    }
    fn resolution(&mut self) -> wasmtime::Result<monotonic_clock::Duration> {
        Ok(FrozenMonotonicClock.resolution())
    }
    fn subscribe_instant(
        &mut self,
        _when: monotonic_clock::Instant,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        self.ready()
    }
    fn subscribe_duration(
        &mut self,
        _duration: monotonic_clock::Duration,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        self.ready()
    }
}

// ---------------------------------------------------------------------------
// Bounded capture of stdout and stderr
// ---------------------------------------------------------------------------

struct CaptureBuf {
    /// The last `limit` bytes written.
    tail: VecDeque<u8>,
    limit: usize,
    /// Every byte ever written.
    written: u64,
}

/// A stdout or stderr that keeps the last `limit` bytes and drops older ones.
/// It never blocks and never fails, so a chatty law is never stopped by its
/// own printing.
#[derive(Clone)]
pub(crate) struct Capture {
    buf: Arc<Mutex<CaptureBuf>>,
}

impl Capture {
    fn new(limit: usize) -> Self {
        Capture {
            buf: Arc::new(Mutex::new(CaptureBuf {
                tail: VecDeque::with_capacity(limit.min(4096)),
                limit,
                written: 0,
            })),
        }
    }

    fn push(&self, data: &[u8]) {
        let mut b = lock(&self.buf);
        b.written = b.written.saturating_add(data.len() as u64);
        let keep = data.len().min(b.limit);
        b.tail.extend(&data[data.len() - keep..]);
        let excess = b.tail.len().saturating_sub(b.limit);
        b.tail.drain(..excess);
    }

    /// Total bytes written so far: a mark for [`Self::since`].
    fn mark(&self) -> u64 {
        lock(&self.buf).written
    }

    /// What was written after `mark`, as far as it is still kept.
    fn since(&self, mark: u64) -> String {
        let b = lock(&self.buf);
        let new = usize::try_from(b.written.saturating_sub(mark)).unwrap_or(usize::MAX);
        let n = new.min(b.tail.len());
        let bytes: Vec<u8> = b.tail.iter().skip(b.tail.len() - n).copied().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// The kept tail and how many older bytes were dropped.
    fn snapshot(&self) -> (String, u64) {
        let b = lock(&self.buf);
        let bytes: Vec<u8> = b.tail.iter().copied().collect();
        let dropped = b.written.saturating_sub(b.tail.len() as u64);
        (String::from_utf8_lossy(&bytes).into_owned(), dropped)
    }
}

impl IsTerminal for Capture {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for Capture {
    fn p2_stream(&self) -> Box<dyn OutputStream> {
        Box::new(self.clone())
    }
    fn async_stream(&self) -> Box<dyn tokio::io::AsyncWrite + Send + Sync> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl Pollable for Capture {
    async fn ready(&mut self) {}
}

impl OutputStream for Capture {
    fn write(&mut self, bytes: bytes::Bytes) -> StreamResult<()> {
        self.push(&bytes);
        Ok(())
    }
    fn flush(&mut self) -> StreamResult<()> {
        Ok(())
    }
    fn check_write(&mut self) -> StreamResult<usize> {
        Ok(WRITE_PERMIT)
    }
}

impl tokio::io::AsyncWrite for Capture {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.push(buf);
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

// ---------------------------------------------------------------------------
// Memory cap
// ---------------------------------------------------------------------------

/// Caps the total linear memory of every memory in the store. A refused
/// growth is recorded so the trap that usually follows (a Rust law aborts
/// when an allocation fails) is reported as a memory error.
struct Limiter {
    limit: usize,
    total: usize,
    refused: Option<usize>,
}

impl ResourceLimiter for Limiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let total = self.total.saturating_sub(current).saturating_add(desired);
        if total > self.limit {
            self.refused = Some(total);
            return Ok(false);
        }
        self.total = total;
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= MAX_TABLE_ELEMENTS)
    }
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

pub(crate) struct HostState {
    wasi: WasiCtx,
    table: ResourceTable,
    limiter: Limiter,
    /// When the call in flight must be stopped.
    deadline: Instant,
    stdout: Capture,
    stderr: Capture,
    /// `stderr`'s mark when the call in flight began.
    stderr_mark: u64,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// The linker every law is instantiated against: WASI 0.2, with the
/// monotonic clock replaced by [`FrozenMonotonic`]. What WASI could reach is
/// withheld in [`Sandbox::new`]'s context: no directories, no network, no
/// environment, no arguments, no stdin.
pub(crate) fn linker(engine: &Engine) -> wasmtime::Result<Linker<HostState>> {
    let mut linker = Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
    linker.allow_shadowing(true);
    monotonic_clock::add_to_linker::<HostState, FrozenMonotonic>(&mut linker, |s| {
        FrozenMonotonicView {
            table: &mut s.table,
        }
    })?;
    linker.allow_shadowing(false);
    Ok(linker)
}

/// One law's store, with the deadline and memory bookkeeping around calls.
pub(crate) struct Sandbox {
    pub(crate) store: Store<HostState>,
    budget: Duration,
}

impl Sandbox {
    pub(crate) fn new(engine: &Engine, memory_limit: usize, capture_bytes: usize) -> Self {
        let stdout = Capture::new(capture_bytes);
        let stderr = Capture::new(capture_bytes);
        let mut b = WasiCtxBuilder::new();
        // Nothing is inherited: the builder starts with no preopened
        // directories, no environment, no arguments and an empty stdin, and
        // these lines only add the deterministic replacements.
        b.stdout(stdout.clone())
            .stderr(stderr.clone())
            .wall_clock(FrozenWallClock)
            .monotonic_clock(FrozenMonotonicClock)
            .secure_random(StdRng::seed_from_u64(SECURE_SEED))
            .insecure_random(StdRng::seed_from_u64(INSECURE_SEED))
            .insecure_random_seed(INSECURE_SEED_128)
            .max_random_size(MAX_RANDOM_BYTES)
            .allow_tcp(false)
            .allow_udp(false)
            .allow_ip_name_lookup(false);
        let state = HostState {
            wasi: b.build(),
            table: ResourceTable::new(),
            limiter: Limiter {
                limit: memory_limit,
                total: 0,
                refused: None,
            },
            deadline: Instant::now(),
            stdout,
            stderr,
            stderr_mark: 0,
        };
        let mut store = Store::new(engine, state);
        store.limiter(|s| &mut s.limiter);
        // Checked once per epoch tick while a call runs: stop the law when the
        // call's real-time deadline has passed, otherwise look again next
        // tick. Comparing real time (rather than counting ticks) keeps the
        // deadline honest when the ticker thread is late.
        store.epoch_deadline_callback(|ctx| {
            if Instant::now() >= ctx.data().deadline {
                Ok(UpdateDeadline::Interrupt)
            } else {
                Ok(UpdateDeadline::Continue(1))
            }
        });
        Sandbox {
            store,
            budget: Duration::ZERO,
        }
    }

    /// Prepare for one call with `budget` of wall-clock time.
    pub(crate) fn arm(&mut self, budget: Duration) {
        self.budget = budget;
        let now = Instant::now();
        let st = self.store.data_mut();
        st.deadline = now
            .checked_add(budget)
            .unwrap_or_else(|| now + Duration::from_secs(365 * 24 * 3600));
        st.limiter.refused = None;
        st.stderr_mark = st.stderr.mark();
        self.store.set_epoch_deadline(1);
    }

    /// Name the failure of the call last armed.
    pub(crate) fn classify(&self, e: &wasmtime::Error) -> LawError {
        let st = self.store.data();
        if let Some(requested_bytes) = st.limiter.refused {
            return LawError::Memory {
                limit_bytes: st.limiter.limit,
                requested_bytes,
            };
        }
        if matches!(e.downcast_ref::<Trap>(), Some(Trap::Interrupt)) {
            return LawError::Deadline {
                budget: self.budget,
            };
        }
        let mut msg = match e.downcast_ref::<Trap>() {
            Some(t) => t.to_string(),
            None => e
                .chain()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(": "),
        };
        let said = st.stderr.since(st.stderr_mark);
        let said = said.trim();
        if !said.is_empty() {
            msg.push_str("; the law wrote: ");
            msg.push_str(said);
        }
        LawError::Trap(msg)
    }

    pub(crate) fn output(&self) -> GuestOutput {
        let st = self.store.data();
        let (stdout, stdout_dropped) = st.stdout.snapshot();
        let (stderr, stderr_dropped) = st.stderr.snapshot();
        GuestOutput {
            stdout,
            stderr,
            stdout_dropped,
            stderr_dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capture_keeps_the_tail_and_counts_what_it_dropped() {
        let c = Capture::new(8);
        c.push(b"hello ");
        let mark = c.mark();
        c.push(b"world, again");
        let (kept, dropped) = c.snapshot();
        assert_eq!(kept, "d, again");
        assert_eq!(dropped, 18 - 8);
        // Only what came after the mark, and only as far as it is kept.
        assert_eq!(c.since(mark), "d, again");
        let mark = c.mark();
        c.push(b"!");
        assert_eq!(c.since(mark), "!");
    }

    #[test]
    fn the_limiter_counts_every_memory_against_one_cap() {
        let mut l = Limiter {
            limit: 100,
            total: 0,
            refused: None,
        };
        assert!(l.memory_growing(0, 60, None).unwrap_or(false));
        // A second memory may not take the total past the cap.
        assert!(!l.memory_growing(0, 50, None).unwrap_or(true));
        assert_eq!(l.refused, Some(110));
        // Growing the first memory is counted from its current size.
        assert!(l.memory_growing(60, 100, None).unwrap_or(false));
        assert!(!l.memory_growing(100, 101, None).unwrap_or(true));
    }
}
