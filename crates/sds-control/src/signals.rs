//! Signals: the time history of every channel a run reads or drives, whatever the test
//! type (schema/PROTOCOL.md, "Signals").
//!
//! Every test builds its station through [`crate::station::build`], which wraps the
//! backend in a [`Tapped`] decorator. At `configure` the decorator learns which column
//! of the blocks the backend reads and writes is which channel-table row, by the rule
//! both backends use ([`columns`]): the enabled rows in table order, the drive rows the
//! outputs and every other row an input. It then copies each block read and each drive
//! block the backend accepts into a ring holding the last several seconds of every
//! channel ([`sizing`]). A connection reads windows of the ring through
//! `signals.subscribe` ([`subscribe`]).
//!
//! **The run is never made to wait.** The ring has one writer, the run, and any number
//! of readers. The writer stores samples into atomics and publishes how far it has
//! written; a reader copies a window and then checks that the writer did not come round
//! and overwrite what it copied, dropping (and counting) any frames it did. Nothing the
//! run touches on its read or write path is behind a lock, and nothing there allocates:
//! the ring is sized once, at configure. The hub's lock ([`Hub`]) is taken only when a
//! run configures or ends, and by the service to find the current run.
//!
//! **Units.** Both backends deliver inputs in engineering units: the NI backend converts
//! the driver's volts with each row's sensitivity inside `read_input`, and the
//! simulator's plant answers in the rows' units. The tap keeps inputs as read, so it
//! never scales twice, and drives as written, in volts.
//!
//! **Station time.** A frame's place is counted from the run's configure: an input frame
//! by the frames read before it, a drive frame by the frames written before it. A drive
//! written after the backend has stopped (the next pulse of a shock test, the next
//! segment of a run that suspended) starts again at the input's place, because the
//! backend dropped whatever it still held when it stopped.
//!
//! **Lifetime.** A run's configure replaces the previous run's tap; the run's end (its
//! backend closed or dropped) marks the tap idle, and the service lets it go once the
//! last frames of it have been sent.

pub mod subscribe;

use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_hal::{
    AcquisitionConfig, Backend, BackendStatus, Caps, ConfiguredActual, HalError, InputRead,
};
use std::sync::atomic::{fence, AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// The longest window a plot may ask for, seconds.
pub const MAX_WINDOW_S: f64 = 10.0;
/// The most samples one run's ring holds over all its channels: 4 Mi samples (16 MiB).
/// A run whose channels and rate would need more holds a shorter window.
pub const RING_SAMPLES: usize = 4 << 20;
/// The ring is longer than the longest window by one part in this many (5%): a reader
/// loses frames to the writer only when it takes longer than that to copy a window.
pub const READER_MARGIN_PARTS: u64 = 20;

/// Whether a channel is read or driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Input,
    Drive,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Input => "input",
            Kind::Drive => "drive",
        }
    }
}

/// One channel the tap holds: its table row and its column in the blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalChannel {
    /// The channel-table row's index.
    pub index: u32,
    /// The row's description, or its role when it has none.
    pub name: String,
    /// The row's unit for an input; volts for a drive.
    pub unit: String,
    pub kind: Kind,
    /// The row's role, by its protocol name.
    pub role: &'static str,
    /// Its column among the inputs (`read_input`) or the drives (`write_drive`).
    pub col: usize,
}

/// The channels a backend configured with `table` reads and drives, in table order, each
/// with its column: the enabled rows only, the drive rows the drive columns in order and
/// every other row (control, monitor, reference) the input columns in order. The rule of
/// the simulator (`SimBackend::configure`) and of the NI planner (`plan_from_table`).
///
/// A table with no enabled drive row has no drive channel here, even on the simulator,
/// which then drives a plant input of its own that no row names.
pub fn columns(table: &ChannelTable) -> Vec<SignalChannel> {
    let (mut inputs, mut drives) = (0usize, 0usize);
    table
        .channels
        .iter()
        .filter(|c| c.enabled)
        .map(|c| {
            let (kind, col) = if c.role == ChannelRole::Drive {
                drives += 1;
                (Kind::Drive, drives - 1)
            } else {
                inputs += 1;
                (Kind::Input, inputs - 1)
            };
            let role = crate::table::role_name(c.role);
            let name = match c.description.trim() {
                "" => role.to_owned(),
                d => d.to_owned(),
            };
            let unit = match kind {
                Kind::Input => crate::table::unit_name(&c.units),
                Kind::Drive => "V".to_owned(),
            };
            SignalChannel {
                index: c.index,
                name,
                unit,
                kind,
                role,
                col,
            }
        })
        .collect()
}

/// How long a run's ring is and the longest window it gives a reader, in frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sizing {
    pub ring_frames: u64,
    pub window_frames: u64,
}

/// The ring for `columns` channels at `fs_hz`: [`MAX_WINDOW_S`] and the reader's margin,
/// or as many frames as [`RING_SAMPLES`] allows over the columns when that is fewer (the
/// longest window then shrinks with it).
pub fn sizing(fs_hz: f64, columns: usize) -> Sizing {
    let fs = if fs_hz.is_finite() && fs_hz > 0.0 {
        fs_hz
    } else {
        1.0
    };
    let window = ((fs * MAX_WINDOW_S).ceil() as u64).max(1);
    let want = window + window.div_ceil(READER_MARGIN_PARTS);
    let room = (RING_SAMPLES / columns.max(1)) as u64;
    let ring_frames = want.min(room).max(2);
    let window_frames = if ring_frames == want {
        window
    } else {
        ring_frames * READER_MARGIN_PARTS / (READER_MARGIN_PARTS + 1)
    }
    .clamp(1, ring_frames - 1);
    Sizing {
        ring_frames,
        window_frames,
    }
}

/// One kind's ring: `width` channels interleaved, `cap` frames, one writer.
///
/// The writer stores `claimed` (the end of the block it is about to write), then the
/// samples, then `head` (the end of what it has written). A reader loads `head`, copies,
/// then loads `claimed`: every frame below `claimed - cap` may have been overwritten
/// while it copied, and is dropped. The fences make the reader's `claimed` at least as
/// new as any sample it copied.
struct Ring {
    width: usize,
    cap: u64,
    data: Box<[AtomicU32]>,
    head: AtomicU64,
    claimed: AtomicU64,
}

impl Ring {
    fn new(width: usize, cap: u64) -> Ring {
        let n = if width == 0 { 0 } else { cap as usize * width };
        Ring {
            width,
            cap,
            data: std::iter::repeat_with(|| AtomicU32::new(0))
                .take(n)
                .collect(),
            head: AtomicU64::new(0),
            claimed: AtomicU64::new(0),
        }
    }

    fn head(&self) -> u64 {
        self.head.load(Ordering::Acquire)
    }

    /// Writer only: begin overwriting up to frame `end`.
    fn claim(&self, end: u64) {
        if end > self.claimed.load(Ordering::Relaxed) {
            self.claimed.store(end, Ordering::Relaxed);
        }
        fence(Ordering::Release);
    }

    /// Writer only: `frames` frames of `block` at the head. Returns the frames that could
    /// not be kept (a block longer than the ring keeps its last `cap` frames).
    fn append(&self, block: &[f32], frames: usize) -> u64 {
        if self.width == 0 || frames == 0 {
            return 0;
        }
        let at = self.head.load(Ordering::Relaxed);
        let end = at + frames as u64;
        let skip = (frames as u64).saturating_sub(self.cap) as usize;
        self.claim(end);
        for f in skip..frames {
            let slot = ((at + f as u64) % self.cap) as usize * self.width;
            let src = &block[f * self.width..(f + 1) * self.width];
            for (d, x) in self.data[slot..slot + self.width].iter().zip(src) {
                d.store(x.to_bits(), Ordering::Relaxed);
            }
        }
        self.head.store(end, Ordering::Release);
        skip as u64
    }

    /// Writer only: put the head at frame `at`. Forward, the frames in between are
    /// silence (written as zeros, the last `cap` of them); back, the frames from `at` on
    /// are written again.
    fn move_to(&self, at: u64) {
        let head = self.head.load(Ordering::Relaxed);
        if self.width > 0 && at > head {
            let from = head.max(at.saturating_sub(self.cap));
            self.claim(at);
            for f in from..at {
                let slot = (f % self.cap) as usize * self.width;
                for d in &self.data[slot..slot + self.width] {
                    d.store(0f32.to_bits(), Ordering::Relaxed);
                }
            }
        }
        self.head.store(at, Ordering::Release);
    }

    /// Copy column `col` over frames `[lo, hi)` into `out` (cleared first); `hi` must be at
    /// most a head this reader has loaded. Returns the first frame copied, and the frames
    /// lost: those within the ring's length of `hi` that the writer overwrote after the
    /// reader took `hi` (frames further back than a ring holds are not lost, only old).
    fn copy(&self, col: usize, lo: u64, hi: u64, out: &mut Vec<f32>) -> (u64, u64) {
        out.clear();
        if self.width == 0 || col >= self.width || hi <= lo {
            return (hi.max(lo), 0);
        }
        let held = lo.max(hi.saturating_sub(self.cap));
        let mut start = held.max(
            self.claimed
                .load(Ordering::Acquire)
                .saturating_sub(self.cap),
        );
        if start < hi {
            out.reserve((hi - start) as usize);
            for f in start..hi {
                let i = (f % self.cap) as usize * self.width + col;
                out.push(f32::from_bits(self.data[i].load(Ordering::Relaxed)));
            }
            fence(Ordering::Acquire);
            let valid = self
                .claimed
                .load(Ordering::Relaxed)
                .saturating_sub(self.cap);
            if valid > start {
                let gone = (valid.min(hi) - start) as usize;
                out.drain(..gone);
                start += gone as u64;
            }
        }
        let first = start.min(hi);
        (first, first - held)
    }
}

/// One run's tap: its channels, its rate and its two rings.
pub struct TapRun {
    id: u64,
    fs_hz: f64,
    channels: Vec<SignalChannel>,
    inputs: Ring,
    drives: Ring,
    window_frames: u64,
    live: AtomicBool,
    lost: AtomicU64,
}

impl TapRun {
    /// A run's tap, its ring sized here for its channels at `fs_hz`.
    pub fn new(id: u64, fs_hz: f64, channels: Vec<SignalChannel>) -> TapRun {
        let width = |k: Kind| channels.iter().filter(|c| c.kind == k).count();
        let (n_in, n_out) = (width(Kind::Input), width(Kind::Drive));
        let size = sizing(fs_hz, n_in + n_out);
        TapRun {
            id,
            fs_hz,
            inputs: Ring::new(n_in, size.ring_frames),
            drives: Ring::new(n_out, size.ring_frames),
            channels,
            window_frames: size.window_frames,
            live: AtomicBool::new(true),
            lost: AtomicU64::new(0),
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// The rate the station programmed.
    pub fn fs_hz(&self) -> f64 {
        self.fs_hz
    }

    /// Every channel, in table order.
    pub fn channels(&self) -> &[SignalChannel] {
        &self.channels
    }

    /// The run is acquiring: configured, and not yet closed or dropped.
    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }

    /// The longest window this run's ring gives, in frames.
    pub fn window_frames(&self) -> u64 {
        self.window_frames
    }

    /// The longest window, seconds.
    pub fn max_window_s(&self) -> f64 {
        self.window_frames as f64 / self.fs_hz
    }

    /// Frames lost so far: a reader lapped by the writer, or a block longer than the ring.
    pub fn lost_frames(&self) -> u64 {
        self.lost.load(Ordering::Relaxed)
    }

    /// Station time: the frames read so far.
    pub fn now(&self) -> u64 {
        self.inputs.head()
    }

    /// The drive frames written so far, at their place in station time.
    pub fn drive_head(&self) -> u64 {
        self.drives.head()
    }

    /// `ch`'s samples over station frames `[lo, hi)` into `out`, from the first still
    /// held; drives up to what has been written. Returns the first frame in `out`. Load
    /// `hi` from [`TapRun::now`] before calling, so a drive written again after a stop is
    /// never read while it is being written.
    pub fn window(&self, ch: &SignalChannel, lo: u64, hi: u64, out: &mut Vec<f32>) -> u64 {
        let (ring, hi) = match ch.kind {
            Kind::Input => (&self.inputs, hi.min(self.inputs.head())),
            Kind::Drive => (&self.drives, hi.min(self.drives.head())),
        };
        let (first, lost) = ring.copy(ch.col, lo, hi, out);
        if lost > 0 {
            self.lost.fetch_add(lost, Ordering::Relaxed);
        }
        first
    }

    /// Writer: a block read.
    fn put_inputs(&self, block: &[f32], frames: usize) {
        self.put(&self.inputs, block, frames);
    }

    /// Writer: a drive block accepted.
    fn put_drives(&self, block: &[f32], frames: usize) {
        self.put(&self.drives, block, frames);
    }

    fn put(&self, ring: &Ring, block: &[f32], frames: usize) {
        if ring.width == 0 {
            return;
        }
        // A block narrower than its frames claim (a backend with another layout than
        // the table's) is not taken: it would put samples under the wrong rows.
        if block.len() < frames * ring.width {
            self.lost.fetch_add(frames as u64, Ordering::Relaxed);
            return;
        }
        let skipped = ring.append(block, frames);
        if skipped > 0 {
            self.lost.fetch_add(skipped, Ordering::Relaxed);
        }
    }

    fn end(&self) {
        self.live.store(false, Ordering::Release);
    }
}

/// Where the current run's tap is found: one per process ([`global`]).
pub struct Hub {
    current: Mutex<Option<Arc<TapRun>>>,
    next: AtomicU64,
}

impl Hub {
    pub fn new() -> Arc<Hub> {
        Arc::new(Hub {
            current: Mutex::new(None),
            next: AtomicU64::new(1),
        })
    }

    fn slot(&self) -> MutexGuard<'_, Option<Arc<TapRun>>> {
        self.current.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The current run's tap, live or ended and not yet let go.
    pub fn current(&self) -> Option<Arc<TapRun>> {
        self.slot().clone()
    }

    /// A new run's tap: it replaces the one before, which ends.
    fn install(&self, fs_hz: f64, channels: Vec<SignalChannel>) -> Arc<TapRun> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let run = Arc::new(TapRun::new(id, fs_hz, channels));
        let old = self.slot().replace(run.clone());
        if let Some(old) = old {
            old.end();
        }
        run
    }

    /// Run `id` has ended; it stays until [`Hub::release`], so its last frames can be sent.
    fn end(&self, id: u64) {
        if let Some(run) = self.slot().as_ref().filter(|r| r.id == id) {
            run.end();
        }
    }

    /// Let run `id` go, when it has ended and is still the current one.
    pub fn release(&self, id: u64) {
        let mut slot = self.slot();
        if slot.as_ref().is_some_and(|r| r.id == id && !r.is_live()) {
            *slot = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn hold_lock(&self) -> MutexGuard<'_, Option<Arc<TapRun>>> {
        self.slot()
    }
}

/// The process's hub, which every station built through [`crate::station::build`] taps.
pub fn global() -> Arc<Hub> {
    static HUB: OnceLock<Arc<Hub>> = OnceLock::new();
    HUB.get_or_init(Hub::new).clone()
}

/// A backend whose reads and accepted drives are copied into the current run's tap.
pub struct Tapped<B> {
    inner: B,
    hub: Arc<Hub>,
    run: Option<Arc<TapRun>>,
    /// The next drive block starts again at the input's place (set by a stop).
    rebase: bool,
}

impl<B: Backend> Tapped<B> {
    pub fn new(inner: B, hub: Arc<Hub>) -> Tapped<B> {
        Tapped {
            inner,
            hub,
            run: None,
            rebase: false,
        }
    }

    /// This backend's tap, once configured.
    pub fn run(&self) -> Option<&Arc<TapRun>> {
        self.run.as_ref()
    }

    fn finish(&mut self) {
        if let Some(run) = self.run.take() {
            run.end();
            self.hub.end(run.id);
        }
    }
}

impl<B> Drop for Tapped<B> {
    fn drop(&mut self) {
        if let Some(run) = self.run.take() {
            run.end();
            self.hub.end(run.id);
        }
    }
}

/// Every method is forwarded, the defaulted ones too (see the HAL's `impl Backend for
/// Box<T>`): a default left in place here would hide the inner backend's own answer.
impl<B: Backend> Backend for Tapped<B> {
    fn caps(&self) -> Caps {
        self.inner.caps()
    }

    fn open(&mut self) -> Result<(), HalError> {
        self.inner.open()
    }

    fn close(&mut self) -> Result<(), HalError> {
        let r = self.inner.close();
        self.finish();
        r
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
        self.finish();
        self.run = Some(self.hub.install(actual.actual_rate_hz, columns(table)));
        self.rebase = false;
        Ok(actual)
    }

    fn arm(&mut self) -> Result<(), HalError> {
        self.inner.arm()
    }

    fn start(&mut self) -> Result<(), HalError> {
        self.inner.start()
    }

    fn stop(&mut self) -> Result<(), HalError> {
        let r = self.inner.stop();
        self.rebase = true;
        r
    }

    fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
        let r = self.inner.read_input(out)?;
        if let (InputRead::Frames(n), Some(run)) = (r, &self.run) {
            run.put_inputs(out, n);
        }
        Ok(r)
    }

    fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
        let n = self.inner.write_drive(drive)?;
        if let Some(run) = &self.run {
            if n > 0 && run.drives.width > 0 {
                if std::mem::take(&mut self.rebase) {
                    run.drives.move_to(run.inputs.head());
                }
                run.put_drives(drive, n);
            }
        }
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
        self.inner.emergency_stop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_channel_table::{Channel, Units};
    use hertzforge_hal::{ClockDomain, LoopOwner};
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    fn row(index: u32, role: ChannelRole, description: &str, units: Units) -> Channel {
        let mut c = Channel::new(index);
        c.role = role;
        c.description = description.into();
        c.units = units;
        c
    }

    fn table(rows: Vec<Channel>) -> ChannelTable {
        let mut t = ChannelTable::new();
        t.channels = rows;
        t
    }

    /// A backend that reads frames whose every sample is its station frame plus the
    /// column's hundredth (exact in f32 well past these tests' frame counts), and
    /// accepts every drive frame.
    struct Counting {
        width: usize,
        frame: u64,
        reads: Arc<AtomicUsize>,
    }

    impl Counting {
        fn new(width: usize) -> Counting {
            Counting {
                width,
                frame: 0,
                reads: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl Backend for Counting {
        fn caps(&self) -> Caps {
            Caps {
                loop_owner: LoopOwner::HostClosesLoop,
                native_modes: vec![],
                clock: ClockDomain::Shared,
                max_input_channels: 64,
                max_output_channels: 64,
                abort_latency_bound: None,
                output_buffer: None,
                sample_rate_hz: 1000.0,
                delegated_response_observable: false,
                clock_groups: vec![],
            }
        }
        fn open(&mut self) -> Result<(), HalError> {
            Ok(())
        }
        fn close(&mut self) -> Result<(), HalError> {
            Ok(())
        }
        fn configure(
            &mut self,
            _: &ChannelTable,
            acq: &AcquisitionConfig,
        ) -> Result<ConfiguredActual, HalError> {
            Ok(ConfiguredActual {
                actual_rate_hz: acq.requested_rate_hz,
                block_frames: acq.block_frames,
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
            Ok(())
        }
        fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
            let frames = out.len() / self.width;
            for f in 0..frames {
                for c in 0..self.width {
                    out[f * self.width + c] = (self.frame + f as u64) as f32 + c as f32 / 100.0;
                }
            }
            self.frame += frames as u64;
            self.reads.fetch_add(1, Ordering::Relaxed);
            Ok(InputRead::Frames(frames))
        }
        fn write_drive(&mut self, drive: &[f32]) -> Result<usize, HalError> {
            Ok(drive.len())
        }
        fn poll_status(&mut self) -> BackendStatus {
            BackendStatus::default()
        }
        fn emergency_stop(&mut self) -> Result<(), HalError> {
            Ok(())
        }
    }

    fn input(index: u32, col: usize) -> SignalChannel {
        SignalChannel {
            index,
            name: String::new(),
            unit: "g".into(),
            kind: Kind::Input,
            role: "monitor",
            col,
        }
    }

    // --- the mapping ------------------------------------------------------------

    #[test]
    fn columns_follow_the_enabled_rows_in_table_order_drives_apart() {
        let mut off = row(3, ChannelRole::Monitor, "Off", Units::AccelG);
        off.enabled = false;
        let mut off_drive = row(6, ChannelRole::Drive, "", Units::VoltageV);
        off_drive.enabled = false;
        let t = table(vec![
            row(4, ChannelRole::Drive, "Shaker", Units::VoltageV),
            row(1, ChannelRole::Control, "Table", Units::AccelG),
            off,
            row(2, ChannelRole::Monitor, "  ", Units::ForceLbf),
            off_drive,
            row(5, ChannelRole::Reference, "Ref", Units::AccelG),
            row(7, ChannelRole::Drive, "Second", Units::AccelG),
        ]);
        let cols = columns(&t);
        let got: Vec<(u32, Kind, usize, &str, &str)> = cols
            .iter()
            .map(|c| (c.index, c.kind, c.col, c.name.as_str(), c.unit.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (4, Kind::Drive, 0, "Shaker", "V"),
                (1, Kind::Input, 0, "Table", "g"),
                (2, Kind::Input, 1, "monitor", "lbf"),
                (5, Kind::Input, 2, "Ref", "g"),
                // A drive is in volts whatever unit its row names.
                (7, Kind::Drive, 1, "Second", "V"),
            ]
        );
    }

    /// The simulator reads the columns this mapping names: as many inputs, and its
    /// reference row (third input here, after a disabled row and a drive) reads the drive.
    #[test]
    fn the_simulator_reads_the_columns_the_mapping_names() {
        use hertzforge_hal_sim::SimConfig;
        let mut off = row(2, ChannelRole::Monitor, "", Units::AccelG);
        off.enabled = false;
        let mut reference = row(6, ChannelRole::Reference, "", Units::AccelG);
        reference.range_max = 10.0;
        let t = table(vec![
            row(1, ChannelRole::Control, "", Units::AccelG),
            off,
            row(3, ChannelRole::Drive, "", Units::VoltageV),
            row(4, ChannelRole::Monitor, "", Units::AccelG),
            reference,
        ]);
        let cols = columns(&t);
        let inputs: Vec<u32> = cols
            .iter()
            .filter(|c| c.kind == Kind::Input)
            .map(|c| c.index)
            .collect();
        assert_eq!(inputs, [1, 4, 6]);
        let hub = Hub::new();
        let sim = SimConfig::new()
            .resonance(200.0, 2.0)
            .noise_level(0.0)
            .max_input_channels(8)
            .build();
        let mut b = Tapped::new(sim, hub.clone());
        b.open().unwrap();
        b.configure(&t, &AcquisitionConfig::new(1000.0, 16))
            .unwrap();
        b.arm().unwrap();
        b.start().unwrap();
        let mut out = vec![0f32; 3 * 16];
        for _ in 0..4 {
            assert_eq!(b.write_drive(&[0.5; 16]).unwrap(), 16);
            assert_eq!(b.read_input(&mut out).unwrap(), InputRead::Frames(16));
        }
        let run = hub.current().unwrap();
        let mut w = Vec::new();
        let now = run.now();
        assert_eq!(now, 64);
        let reference = cols.iter().find(|c| c.index == 6).unwrap();
        run.window(reference, 32, now, &mut w);
        assert!(
            w.iter().all(|x| (*x - 0.5).abs() < 1e-6),
            "row 6 reads the drive: {w:?}"
        );
        let drive = cols.iter().find(|c| c.index == 3).unwrap();
        run.window(drive, 0, now, &mut w);
        assert_eq!(w, vec![0.5; 64], "row 3 is the drive as written");
    }

    // --- units ------------------------------------------------------------------

    /// The tap keeps what the backend delivers: inputs already in their unit (whatever
    /// the row's sensitivity) and drives in volts. Scaling here as well would scale twice.
    #[test]
    fn inputs_keep_their_unit_and_drives_their_volts() {
        let mut a = row(1, ChannelRole::Control, "", Units::AccelG);
        a.sensitivity_mv_per_eu = 10.0;
        let mut d = row(2, ChannelRole::Drive, "", Units::VoltageV);
        d.sensitivity_mv_per_eu = 50.0;
        let hub = Hub::new();
        let mut b = Tapped::new(Counting::new(1), hub.clone());
        b.configure(&table(vec![a, d]), &AcquisitionConfig::new(1000.0, 4))
            .unwrap();
        let mut out = [0f32; 4];
        b.read_input(&mut out).unwrap();
        b.write_drive(&[0.25, -0.5, 1.0, 2.0]).unwrap();
        let run = hub.current().unwrap();
        let mut w = Vec::new();
        run.window(&run.channels()[0], 0, 4, &mut w);
        assert_eq!(w, [0.0, 1.0, 2.0, 3.0]);
        run.window(&run.channels()[1], 0, 4, &mut w);
        assert_eq!(w, [0.25, -0.5, 1.0, 2.0]);
        assert_eq!(run.channels()[0].unit, "g");
        assert_eq!(run.channels()[1].unit, "V");
    }

    // --- the ring ---------------------------------------------------------------

    #[test]
    fn the_ring_is_sized_at_configure_and_capped_in_samples() {
        let s = sizing(12_800.0, 2);
        assert_eq!(s.ring_frames, 134_400, "10 s and the reader's margin");
        assert_eq!(s.window_frames, 128_000, "10 s");
        let s = sizing(51_200.0, 96);
        assert_eq!(s.ring_frames, (RING_SAMPLES / 96) as u64);
        assert!(s.window_frames < s.ring_frames);
        assert!(s.ring_frames * 96 <= RING_SAMPLES as u64);
        let run = TapRun::new(1, 51_200.0, (0..8).map(|i| input(i, i as usize)).collect());
        assert_eq!(run.inputs.data.len() as u64, run.inputs.cap * 8);
        assert!(run.inputs.data.len() <= RING_SAMPLES);
        assert!(run.max_window_s() > 9.0 && run.max_window_s() <= MAX_WINDOW_S);
    }

    #[test]
    fn the_ring_wraps_and_keeps_the_last_frames() {
        let run = TapRun::new(1, 100.0, vec![input(1, 0), input(2, 1)]);
        let cap = run.inputs.cap;
        let mut frame = 0u64;
        // Blocks of 7 frames until the ring has wrapped twice and a half.
        while frame < cap * 5 / 2 {
            let block: Vec<f32> = (0..7)
                .flat_map(|f| {
                    let x = (frame + f) as f32;
                    [x, -x]
                })
                .collect();
            run.put_inputs(&block, 7);
            frame += 7;
        }
        assert_eq!(run.now(), frame);
        let mut w = Vec::new();
        // The last window, in order across the wrap.
        let lo = frame - run.window_frames();
        let first = run.window(&run.channels()[1], lo, frame, &mut w);
        assert_eq!(first, lo);
        let want: Vec<f32> = (lo..frame).map(|f| -(f as f32)).collect();
        assert_eq!(w, want);
        // Frames the ring no longer holds are not given, and are not counted as lost.
        let first = run.window(&run.channels()[0], 0, frame, &mut w);
        assert_eq!(first, frame - cap);
        assert_eq!(w.len() as u64, cap);
        assert_eq!(w[0], (frame - cap) as f32);
        assert_eq!(run.lost_frames(), 0);
    }

    #[test]
    fn a_reader_lapped_by_the_writer_drops_and_counts_what_was_overwritten() {
        let run = TapRun::new(1, 100.0, vec![input(1, 0)]);
        let cap = run.inputs.cap as usize;
        let block: Vec<f32> = (0..cap).map(|f| f as f32).collect();
        run.put_inputs(&block, cap);
        // The reader takes its window's end, and before it copies, the writer writes a
        // further 40 frames: the window's first 40 frames are no longer there.
        let hi = run.now();
        let lo = hi - 100;
        let more: Vec<f32> = (0..40).map(|f| (cap + f) as f32).collect();
        run.put_inputs(&more, 40);
        let mut w = Vec::new();
        let lo_all = hi - cap as u64;
        let first = run.window(&run.channels()[0], lo_all, hi, &mut w);
        assert_eq!(first, lo_all + 40);
        assert_eq!(w.len(), cap - 40);
        assert_eq!(w[0], 40.0, "never a sample of a later frame");
        assert_eq!(run.lost_frames(), 40);
        // A window clear of the overwrite loses nothing.
        let first = run.window(&run.channels()[0], lo, hi, &mut w);
        assert_eq!((first, w.len()), (lo, 100));
        assert_eq!(run.lost_frames(), 40);
    }

    #[test]
    fn a_block_longer_than_the_ring_keeps_its_last_frames_and_counts_the_rest() {
        let run = TapRun::new(1, 10.0, vec![input(1, 0)]);
        let cap = run.inputs.cap as usize;
        let block: Vec<f32> = (0..cap + 9).map(|f| f as f32).collect();
        run.put_inputs(&block, cap + 9);
        assert_eq!(run.lost_frames(), 9);
        let mut w = Vec::new();
        let first = run.window(&run.channels()[0], 0, run.now(), &mut w);
        assert_eq!(first, 9);
        assert_eq!(w, block[9..]);
        // A block whose samples do not fill its frames is not taken.
        run.put_inputs(&[1.0, 2.0], 3);
        assert_eq!(run.lost_frames(), 12);
        assert_eq!(run.now(), (cap + 9) as u64);
    }

    /// After a stop the backend drops what it held, so the next drive starts at the
    /// input's place: back over drive written ahead, or forward over a gap of silence.
    #[test]
    fn a_drive_after_a_stop_starts_again_at_the_input() {
        let hub = Hub::new();
        let t = table(vec![
            row(1, ChannelRole::Control, "", Units::AccelG),
            row(2, ChannelRole::Drive, "", Units::VoltageV),
        ]);
        let mut b = Tapped::new(Counting::new(1), hub.clone());
        b.configure(&t, &AcquisitionConfig::new(100.0, 8)).unwrap();
        let mut out = [0f32; 8];
        // Drive written two blocks ahead of the reads, then a stop.
        b.write_drive(&[1.0; 24]).unwrap();
        b.read_input(&mut out).unwrap();
        let run = hub.current().unwrap();
        assert_eq!((run.now(), run.drive_head()), (8, 24));
        b.stop().unwrap();
        b.write_drive(&[2.0; 4]).unwrap();
        assert_eq!(run.drive_head(), 12, "back to the input's frame 8");
        let mut w = Vec::new();
        run.window(&run.channels()[1], 0, 12, &mut w);
        assert_eq!(w, [[1.0; 8].as_slice(), &[2.0; 4]].concat());
        // Reads run past the drive; after the next stop the drive starts after silence.
        for _ in 0..3 {
            b.read_input(&mut out).unwrap();
        }
        b.stop().unwrap();
        b.write_drive(&[3.0; 2]).unwrap();
        assert_eq!((run.now(), run.drive_head()), (32, 34));
        run.window(&run.channels()[1], 8, 34, &mut w);
        let want = [[2.0; 4].as_slice(), &[0.0; 20], &[3.0; 2]].concat();
        assert_eq!(w, want);
    }

    // --- the run's lifetime -------------------------------------------------------

    #[test]
    fn a_new_run_replaces_the_tap_and_an_ending_run_marks_it_idle() {
        let hub = Hub::new();
        let t = table(vec![row(1, ChannelRole::Control, "", Units::AccelG)]);
        let mut a = Tapped::new(Counting::new(1), hub.clone());
        a.configure(&t, &AcquisitionConfig::new(100.0, 8)).unwrap();
        let first = hub.current().unwrap();
        assert!(first.is_live());
        let mut b = Tapped::new(Counting::new(1), hub.clone());
        b.configure(&t, &AcquisitionConfig::new(100.0, 8)).unwrap();
        let second = hub.current().unwrap();
        assert_ne!(first.id(), second.id());
        assert!(!first.is_live(), "replaced");
        // The first run ending later leaves the second alone.
        drop(a);
        assert!(hub.current().unwrap().is_live());
        assert_eq!(hub.current().unwrap().id(), second.id());
        // Release does nothing to a live run.
        hub.release(second.id());
        assert!(hub.current().is_some());
        b.close().unwrap();
        assert!(!second.is_live());
        assert_eq!(
            hub.current().unwrap().id(),
            second.id(),
            "kept for its last frames"
        );
        hub.release(second.id());
        assert!(hub.current().is_none());
        drop(b);
        assert!(hub.current().is_none());
    }

    #[test]
    fn a_configure_the_backend_refuses_installs_nothing() {
        struct Refusing(Counting);
        impl Backend for Refusing {
            fn caps(&self) -> Caps {
                self.0.caps()
            }
            fn open(&mut self) -> Result<(), HalError> {
                Ok(())
            }
            fn close(&mut self) -> Result<(), HalError> {
                Ok(())
            }
            fn configure(
                &mut self,
                _: &ChannelTable,
                _: &AcquisitionConfig,
            ) -> Result<ConfiguredActual, HalError> {
                Err(HalError::Config("no".into()))
            }
            fn arm(&mut self) -> Result<(), HalError> {
                Ok(())
            }
            fn start(&mut self) -> Result<(), HalError> {
                Ok(())
            }
            fn stop(&mut self) -> Result<(), HalError> {
                Ok(())
            }
            fn read_input(&mut self, out: &mut [f32]) -> Result<InputRead, HalError> {
                self.0.read_input(out)
            }
            fn write_drive(&mut self, d: &[f32]) -> Result<usize, HalError> {
                Ok(d.len())
            }
            fn poll_status(&mut self) -> BackendStatus {
                BackendStatus::default()
            }
            fn emergency_stop(&mut self) -> Result<(), HalError> {
                Ok(())
            }
        }
        let hub = Hub::new();
        let mut b = Tapped::new(Refusing(Counting::new(1)), hub.clone());
        let t = table(vec![row(1, ChannelRole::Control, "", Units::AccelG)]);
        assert!(b.configure(&t, &AcquisitionConfig::new(100.0, 8)).is_err());
        assert!(hub.current().is_none());
        let mut out = [0f32; 8];
        assert_eq!(b.read_input(&mut out).unwrap(), InputRead::Frames(8));
    }

    // --- the run never waits --------------------------------------------------------

    /// The hub's lock is the only lock in the tap. Held by another thread for the whole
    /// test (as a reader finding the run would hold it), the run's reads and writes still
    /// finish. The wait only guards against a hang: the verdict is that they finish.
    #[test]
    fn reads_and_writes_finish_while_the_hub_is_locked() {
        let hub = Hub::new();
        let t = table(vec![
            row(1, ChannelRole::Control, "", Units::AccelG),
            row(2, ChannelRole::Drive, "", Units::VoltageV),
        ]);
        let mut b = Tapped::new(Counting::new(1), hub.clone());
        b.configure(&t, &AcquisitionConfig::new(1000.0, 64))
            .unwrap();
        let run = hub.current().unwrap();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let holder = {
            let hub = hub.clone();
            std::thread::spawn(move || {
                let _guard = hub.hold_lock();
                locked_tx.send(()).unwrap();
                let _ = done_rx.recv_timeout(Duration::from_secs(60));
            })
        };
        locked_rx.recv().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            let mut out = vec![0f32; 64];
            let drive = vec![0.1f32; 64];
            for _ in 0..1000 {
                b.write_drive(&drive).unwrap();
                b.read_input(&mut out).unwrap();
            }
            tx.send(()).unwrap();
            b
        });
        let finished = rx.recv_timeout(Duration::from_secs(30));
        done_tx.send(()).unwrap();
        holder.join().unwrap();
        assert!(finished.is_ok(), "the run waited on the hub's lock");
        let b = writer.join().unwrap();
        assert_eq!(run.now(), 64_000);
        assert_eq!(run.drive_head(), 64_000);
        drop(b);
    }

    /// Two readers copy windows over and over while the run reads 4000 blocks: the
    /// blocks are not held up by them (the slowest 1% still well under a millisecond's
    /// order, against a 20 ms block on hardware), and every window a reader copies is
    /// whole: consecutive frames, each sample its own frame's, across wraps and laps.
    /// That the run never waits on a reader is shown without the clock by
    /// `reads_and_writes_finish_while_the_hub_is_locked`; this one is judged on it.
    #[test]
    fn signals_reads_never_wait_on_readers_under_contention() {
        const BLOCK: usize = 256;
        const BLOCKS: usize = 4000;
        let hub = Hub::new();
        let t = table(vec![
            row(1, ChannelRole::Control, "", Units::AccelG),
            row(2, ChannelRole::Monitor, "", Units::AccelG),
            row(3, ChannelRole::Monitor, "", Units::AccelG),
            row(4, ChannelRole::Monitor, "", Units::AccelG),
        ]);
        // A small ring (200 Hz: about 2100 frames), so readers are lapped often.
        let mut b = Tapped::new(Counting::new(4), hub.clone());
        b.configure(&t, &AcquisitionConfig::new(200.0, BLOCK))
            .unwrap();
        let run = hub.current().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let readers: Vec<_> = (0..2)
            .map(|k| {
                let (run, stop) = (run.clone(), stop.clone());
                std::thread::spawn(move || {
                    let mut w = Vec::new();
                    let mut windows = 0u64;
                    while !stop.load(Ordering::Relaxed) {
                        let ch = &run.channels()[(k + windows as usize) % 4];
                        let hi = run.now();
                        let lo = hi.saturating_sub(run.window_frames());
                        let first = run.window(ch, lo, hi, &mut w);
                        for (i, x) in w.iter().enumerate() {
                            let want = (first + i as u64) as f32 + ch.col as f32 / 100.0;
                            assert_eq!(*x, want, "a torn window at frame {}", first + i as u64);
                        }
                        windows += 1;
                        std::thread::yield_now();
                    }
                    windows
                })
            })
            .collect();
        let mut out = vec![0f32; 4 * BLOCK];
        let mut took = Vec::with_capacity(BLOCKS);
        for _ in 0..BLOCKS {
            let t0 = Instant::now();
            b.read_input(&mut out).unwrap();
            took.push(t0.elapsed());
        }
        stop.store(true, Ordering::Relaxed);
        let windows: u64 = readers.into_iter().map(|r| r.join().unwrap()).sum();
        assert!(windows > 0);
        assert_eq!(run.now(), (BLOCK * BLOCKS) as u64);
        took.sort();
        let p99 = took[BLOCKS * 99 / 100];
        assert!(
            p99 < Duration::from_millis(2),
            "1% of block reads took {p99:?} or longer with readers copying"
        );
    }
}
