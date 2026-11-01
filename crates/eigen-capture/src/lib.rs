//! Capture + snapshots: the mode-generic run infrastructure, and the durability
//! guarantees that make a capture worth keeping.
//!
//! Lifted out of the Explorer service into `eigen-testkit` so every module could share it,
//! and out of `eigen-testkit` into this crate. The `eigen_testkit::capture` compatibility
//! re-export that carried callers across that second move is now GONE, along with the
//! `eigen-testkit` crate itself (which had nothing else in it): every caller now depends on
//! `eigen-capture` and spells the paths `eigen_capture::...`.
//!
//! # CaptureSink: writer thread + bounded ring
//!
//! [`CaptureSink`] streams interleaved f32 telemetry to disk on a DEDICATED writer
//! thread behind a BOUNDED ring. The reader/analysis side NEVER blocks on disk: on a
//! full ring the sink DROPS the chunk and bumps a drop count. Format: raw interleaved
//! f32 binary (`<run>/telemetry.f32`) + a JSON sidecar (`<run>/capture.json`).
//!
//! # The durability quartet
//!
//! Before this, a capture had four ways to lose data quietly. Each is now closed, and
//! each closure has a test that names the mutation that reds it:
//!
//! 1. **Free-space preflight.** [`free_space_bytes`] is a REAL `statvfs(2)` (unix) or
//!    `GetDiskFreeSpaceExW` (Windows) on the capture root, checked at start and again
//!    on every flush tick. Below the
//!    configured floor the capture refuses to start, or FAULTS mid-run - never
//!    silently truncates. Replaces the 256 GiB placeholder constant in
//!    `ui/src-tauri/src/recorder.rs` (which asked for exactly this).
//! 2. **Timed flush + `sync_data`.** The 8 KiB `BufWriter` used to flush only at
//!    `finish()`, so a crash lost everything since the last incidental buffer spill
//!    and the OS page cache held the rest. The writer now flushes and
//!    `File::sync_data`s on a cadence ([`DurabilityConfig::flush_interval`], default
//!    5 s), bounding the loss.
//! 3. **A PROVISIONAL sidecar.** `capture.json` used to be written only at
//!    `finish()`, so a `kill -9` left a binary file with NO way to interpret it -
//!    channel count, rate, and layout all lived in the process that died. It is now
//!    written at START with `complete: false` and rewritten on every flush tick, then
//!    finalized with `complete: true`. A killed capture leaves a READABLE file.
//! 4. **Gap accounting on every path.** The old `TrySendError::Disconnected` arm
//!    returned `false` WITHOUT counting, and a writer-thread `io::Error` ended the
//!    thread silently - so the two worst failures were the two that under-counted. A
//!    faulted writer now keeps draining and counting, the fault text reaches the
//!    sidecar, and both are queryable live ([`CaptureSink::dropped`],
//!    [`CaptureSink::fault`]).
//!
//! # Snapshots
//!
//! [`save_snapshot`] writes a single timestamped JSON bundle in the run folder. The
//! bundle ([`Snapshot`]) carries the mode-generic envelope - timestamp, an opaque
//! serialized settings string, and the operator marks - plus a `payload: P` the mode
//! supplies. This crate never names a mode payload; it only requires that it
//! serializes.

use eigen_channel_table::{ChannelRole, ChannelTable};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Environment variable that overrides the default data root (the highest-priority
/// source in [`default_data_root`]). The value is used verbatim; set it to an
/// absolute path. An empty value is treated as unset.
pub const DATA_ROOT_ENV: &str = "EIGEN_DATA_ROOT";

/// The directory name of the default data root under the user's home directory.
pub const DATA_ROOT_DIR_NAME: &str = "Eigen_Data";

/// Resolve the default LOCAL data root (the folder run directories are created
/// under). Resolution order:
///
/// 1. The [`DATA_ROOT_ENV`] (`EIGEN_DATA_ROOT`) environment variable, verbatim.
/// 2. `<home>/Eigen_Data`, with `<home>` resolved per-OS by [`dirs::home_dir`]:
///    `/Users/<user>` on macOS, `/home/<user>` on Linux, `C:\Users\<user>` on
///    Windows.
/// 3. `./Eigen_Data` relative to the process working directory, only if no home
///    directory can be determined (degenerate environments; never on a normal
///    desktop session).
///
/// History: this was a hard-coded macOS constant (`/Users/<user>/Eigen_Data`), which
/// cannot exist on Linux or Windows and blocked boot there. Rule 2 resolves to that SAME
/// path on the original dev Mac, so existing captures are still found without setting
/// anything. Only WHERE
/// the root lives is resolved here; the run-folder layout and file naming beneath
/// it are unchanged.
///
/// There is currently NO UI setting for the data root: the env var is the only
/// end-user override. Services and [`CaptureSink::start`] additionally accept an
/// explicit `data_root` parameter, so this default is never hard-coded into the
/// sink logic.
pub fn default_data_root() -> PathBuf {
    if let Some(root) = std::env::var_os(DATA_ROOT_ENV) {
        if !root.is_empty() {
            return PathBuf::from(root);
        }
    }
    match dirs::home_dir() {
        Some(home) => home.join(DATA_ROOT_DIR_NAME),
        None => PathBuf::from(DATA_ROOT_DIR_NAME),
    }
}

/// The decimated default capture rate (default 5120 Hz).
pub const DEFAULT_CAPTURE_RATE_HZ: f64 = 5120.0;

/// The raw device rate (opt-in capture; ~64 kHz).
pub const RAW_CAPTURE_RATE_HZ: f64 = 65_536.0;

/// The file name of the raw interleaved-f32 binary inside a run directory.
pub const TELEMETRY_FILE: &str = "telemetry.f32";

/// The file name of the JSON sidecar inside a run directory.
pub const SIDECAR_FILE: &str = "capture.json";

// =====================================================================================
// Configuration
// =====================================================================================

/// Configuration for a capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureConfig {
    /// Number of channels per interleaved frame.
    pub n_channels: usize,
    /// The sample rate the captured frames are AT (decimated or raw).
    pub sample_rate_hz: f64,
    /// The decimation factor from the raw device rate to `sample_rate_hz` (1 for raw).
    pub decimation: u32,
    /// Bounded ring depth in chunks (reader never blocks; drops on overflow).
    pub ring_capacity: usize,
}

impl CaptureConfig {
    /// A capture that records exactly the frames pushed to the sink, at `sample_rate_hz`
    /// (`decimation = 1`). This is the HONEST default for a caller that pushes its own
    /// source-rate frames (the sink does NOT itself drop samples - see [`CaptureSink`]).
    /// Use this with the ACTUAL rate of the frames you push (e.g. a Sim plant's `fs_hz`,
    /// or the decoded telemetry rate), so the sidecar matches the data on disk.
    ///
    /// Bug history: `decimated()` previously stored `5120 Hz` with `decimation = 13`
    /// while `65536/13 = 5041 Hz`, and callers pushed un-decimated source frames - so the
    /// sidecar disagreed with both its own stride and the data. `at_rate` is the fix:
    /// declare the true rate, no phantom decimation.
    pub fn at_rate(n_channels: usize, sample_rate_hz: f64) -> Self {
        Self {
            n_channels,
            sample_rate_hz,
            decimation: 1,
            ring_capacity: 256,
        }
    }

    /// A decimated capture of a RAW-device-rate ([`RAW_CAPTURE_RATE_HZ`]) stream by
    /// `decimation`, recorded at the CONSISTENT `RAW/decimation` rate.
    ///
    /// IMPORTANT: [`CaptureSink`] writes every frame it is given - it does NOT drop
    /// samples. This constructor is therefore only honest if the CALLER pushes frames
    /// already decimated to `sample_rate_hz` (1 of every `decimation` raw frames). If you
    /// are pushing source-rate frames directly, use [`CaptureConfig::at_rate`] with the
    /// true rate instead. `sample_rate_hz` is derived as `RAW/decimation` so the two
    /// fields never contradict each other.
    pub fn decimated(n_channels: usize) -> Self {
        let decimation = (RAW_CAPTURE_RATE_HZ / DEFAULT_CAPTURE_RATE_HZ).round() as u32;
        Self {
            n_channels,
            // Derived so sample_rate_hz == RAW/decimation exactly (was hard-coded 5120,
            // which did not equal 65536/13 = 5041.23).
            sample_rate_hz: RAW_CAPTURE_RATE_HZ / decimation as f64,
            decimation,
            ring_capacity: 256,
        }
    }

    /// A raw 64 kHz capture (opt-in) for `n_channels` channels.
    pub fn raw(n_channels: usize) -> Self {
        Self {
            n_channels,
            sample_rate_hz: RAW_CAPTURE_RATE_HZ,
            decimation: 1,
            ring_capacity: 512,
        }
    }

    /// The rate of the SOURCE stream these frames came from: `sample_rate_hz *
    /// decimation`. An honest config satisfies this invariant by construction.
    pub fn source_rate_hz(&self) -> f64 {
        self.sample_rate_hz * self.decimation as f64
    }

    /// Bytes per second on disk at these settings: `n_channels * 4 bytes * rate`.
    pub fn bytes_per_second(&self) -> f64 {
        self.n_channels as f64 * 4.0 * self.sample_rate_hz
    }
}

/// The absolute minimum free-space floor, regardless of how short the run is. A
/// capture that leaves less than this behind has filled the volume for everything else
/// on the machine (logs, reports, the OS), which is a different and worse failure than
/// a truncated capture.
pub const MIN_FREE_FLOOR_BYTES: u64 = 1024 * 1024 * 1024;

/// The default headroom multiple over the ESTIMATED run size. Two, not one: the
/// estimate is the operator's stated duration, and a run that is stopped late, or
/// re-armed, must not be the thing that fills the volume.
pub const DEFAULT_FLOOR_MULTIPLE: f64 = 2.0;

/// The default flush + `sync_data` cadence. Five seconds bounds a crash's loss to five
/// seconds of capture; a tighter cadence trades throughput for a tighter bound.
pub const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// How the free-space floor is computed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreeSpaceFloor {
    /// `max(multiple x estimated run bytes, MIN_FREE_FLOOR_BYTES)`, where the estimate
    /// is `bytes_per_second() * estimated_seconds`. An `estimated_seconds` of 0 (an
    /// open-ended run) degrades to the minimum floor - the honest answer, since there
    /// is no estimate to double.
    Auto {
        /// The operator's stated run duration, seconds. 0 = open-ended.
        estimated_seconds: f64,
        /// Headroom multiple over the estimate ([`DEFAULT_FLOOR_MULTIPLE`]).
        multiple: f64,
    },
    /// An explicit byte floor, used verbatim.
    Bytes(u64),
}

impl FreeSpaceFloor {
    /// The floor in bytes for `cfg`.
    pub fn bytes(&self, cfg: &CaptureConfig) -> u64 {
        match *self {
            FreeSpaceFloor::Bytes(b) => b,
            FreeSpaceFloor::Auto {
                estimated_seconds,
                multiple,
            } => {
                let est = cfg.bytes_per_second() * estimated_seconds.max(0.0) * multiple.max(0.0);
                let est = if est.is_finite() { est as u64 } else { 0 };
                est.max(MIN_FREE_FLOOR_BYTES)
            }
        }
    }
}

impl Default for FreeSpaceFloor {
    fn default() -> Self {
        FreeSpaceFloor::Auto {
            estimated_seconds: 0.0,
            multiple: DEFAULT_FLOOR_MULTIPLE,
        }
    }
}

/// Durability settings for one capture (the durability quartet).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DurabilityConfig {
    /// Flush + `sync_data` cadence, and the free-space re-check cadence.
    pub flush_interval: Duration,
    /// The free-space floor enforced at start and on every flush tick.
    pub free_space_floor: FreeSpaceFloor,
    /// Whether a capture may start when free space cannot be DETERMINED (a platform
    /// with no implementation, or a query that failed; see [`free_space_bytes`]).
    ///
    /// Defaults to `false`: an unknown answer refuses, because the alternative is a
    /// preflight that silently passes on exactly the platform where it was never
    /// implemented. Set it explicitly, with a logged reason, to proceed anyway.
    pub allow_unknown_free_space: bool,
}

impl Default for DurabilityConfig {
    fn default() -> Self {
        DurabilityConfig {
            flush_interval: DEFAULT_FLUSH_INTERVAL,
            free_space_floor: FreeSpaceFloor::default(),
            allow_unknown_free_space: false,
        }
    }
}

// =====================================================================================
// Free space (quartet rule 1)
// =====================================================================================

/// Why a free-space query could not answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreeSpaceError {
    /// The platform has no implementation in this crate. NAMED rather than silently
    /// treated as "plenty of space" - see
    /// [`DurabilityConfig::allow_unknown_free_space`].
    Unsupported {
        /// The target family that has no implementation.
        platform: String,
    },
    /// The syscall failed (path does not exist, permission, transient I/O).
    Query {
        /// The path queried.
        path: String,
        /// The OS error text.
        error: String,
    },
}

impl std::fmt::Display for FreeSpaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FreeSpaceError::Unsupported { platform } => write!(
                f,
                "free space cannot be determined on {platform} (no free-space \
                 implementation in eigen-capture for this platform); the preflight refuses \
                 rather than assuming space exists"
            ),
            FreeSpaceError::Query { path, error } => {
                write!(f, "free space query failed for {path}: {error}")
            }
        }
    }
}

impl std::error::Error for FreeSpaceError {}

/// Bytes available to an UNPRIVILEGED process on the filesystem containing `path`.
///
/// `statvfs(2)`'s `f_bavail x f_frsize`, not `f_bfree`: `f_bfree` includes the
/// root-reserved blocks, which a capture running as the operator cannot use, and
/// reporting them would overstate the headroom by exactly the reserve (5% on a default
/// ext4 - enough to turn a passing preflight into a full disk).
///
/// Windows: `GetDiskFreeSpaceExW`'s `lpFreeBytesAvailableToCaller`, the analog of
/// `f_bavail` for the same reason - it honors the caller's disk quota, where the
/// volume-wide `lpTotalNumberOfFreeBytes` would overstate what this process can
/// write. Until this arm existed every capture on Windows refused at the preflight
/// (the `Unsupported` answer below), which stopped the Recorder and every module's
/// capture there.
///
/// Any other target answers [`FreeSpaceError::Unsupported`]. That is a deliberate,
/// NAMED gap rather than an optimistic default: the preflight refuses there unless
/// [`DurabilityConfig::allow_unknown_free_space`] is set on purpose.
pub fn free_space_bytes(path: &Path) -> Result<u64, FreeSpaceError> {
    #[cfg(unix)]
    {
        match nix::sys::statvfs::statvfs(path) {
            Ok(s) => {
                // `fsblkcnt_t` and `c_ulong` are u64 on 64-bit macOS/Linux and u32 on
                // 32-bit targets, so the widen is required for portability even where
                // clippy sees it as a same-type cast on THIS host.
                #[allow(clippy::unnecessary_cast)]
                let avail = s.blocks_available() as u64;
                #[allow(clippy::unnecessary_cast)]
                let frag = s.fragment_size() as u64;
                Ok(avail.saturating_mul(frag))
            }
            Err(e) => Err(FreeSpaceError::Query {
                path: path.display().to_string(),
                error: e.to_string(),
            }),
        }
    }
    #[cfg(windows)]
    {
        windows_free_bytes_available(path).map_err(|e| FreeSpaceError::Query {
            path: path.display().to_string(),
            error: e.to_string(),
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(FreeSpaceError::Unsupported {
            platform: std::env::consts::FAMILY.to_string(),
        })
    }
}

/// `GetDiskFreeSpaceExW` for the volume holding `path`.
///
/// The call takes a DIRECTORY, where `statvfs` takes any path: a file is asked
/// about through its parent, so both platforms answer for the same inputs. A UNC
/// share root must end in a backslash, so one is appended when the directory does
/// not already end in a separator (harmless on a local path). A path that does not
/// exist fails the call, which is an error, never a number - same as the unix arm.
#[cfg(windows)]
fn windows_free_bytes_available(path: &Path) -> std::io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    let dir = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let mut wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
    let sep = |c: u16| c == u16::from(b'\\') || c == u16::from(b'/');
    if !wide.last().is_some_and(|&c| sep(c)) {
        wide.push(u16::from(b'\\'));
    }
    wide.push(0);
    let mut avail: u64 = 0;
    // SAFETY: `wide` is NUL-terminated and outlives the call; `avail` is a valid
    // out-pointer; the two optional out-parameters are passed as null, which the
    // API documents as "not requested".
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut avail,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(avail)
}

/// Free bytes on the volume that WOULD hold `path`, walking up to the nearest existing
/// ancestor when `path` has not been created yet.
///
/// This is what a UI readout wants: a recordings folder that does not exist yet still
/// has a volume behind it, and reporting "unknown" there would show an operator a zero
/// budget for a disk that is in fact empty. [`free_space_bytes`] stays exact about the
/// path it was handed; this one is explicit about walking up.
pub fn free_space_bytes_for(path: &Path) -> Result<u64, FreeSpaceError> {
    free_space_bytes(&nearest_existing(path))
}

/// How many whole seconds a capture at `cfg` can run before it fills `free_bytes` of
/// disk. Returns [`u64::MAX`] when the settings write nothing per second.
pub fn disk_budget_max_seconds(cfg: &CaptureConfig, free_bytes: u64) -> u64 {
    let bps = cfg.bytes_per_second();
    if bps <= 0.0 {
        return u64::MAX;
    }
    (free_bytes as f64 / bps).floor() as u64
}

/// How many whole seconds a capture at `cfg` can run on `root` before free space
/// reaches `floor_bytes`. The live-disk counterpart of [`disk_budget_max_seconds`];
/// this is what replaces `ui/src-tauri/src/recorder.rs`'s 256 GiB placeholder.
pub fn disk_budget_seconds_on(
    root: &Path,
    cfg: &CaptureConfig,
    floor_bytes: u64,
) -> Result<u64, FreeSpaceError> {
    let free = free_space_bytes(root)?;
    Ok(disk_budget_max_seconds(
        cfg,
        free.saturating_sub(floor_bytes),
    ))
}

/// Why a capture could not start.
#[derive(Debug)]
pub enum CaptureStartError {
    /// The free-space preflight refused. The capture never opened a file.
    NotEnoughSpace {
        /// Bytes free on the capture volume at the check.
        free_bytes: u64,
        /// The floor that was required.
        floor_bytes: u64,
    },
    /// Free space could not be determined and
    /// [`DurabilityConfig::allow_unknown_free_space`] was not set.
    FreeSpaceUnknown(FreeSpaceError),
    /// Filesystem error creating the run directory, the binary, or the sidecar.
    Io(io::Error),
}

impl std::fmt::Display for CaptureStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureStartError::NotEnoughSpace {
                free_bytes,
                floor_bytes,
            } => write!(
                f,
                "capture refused: {free_bytes} bytes free on the capture volume, floor is \
                 {floor_bytes}. Free space or lower the floor; the capture will not start \
                 and silently truncate."
            ),
            CaptureStartError::FreeSpaceUnknown(e) => write!(f, "capture refused: {e}"),
            CaptureStartError::Io(e) => write!(f, "capture could not start: {e}"),
        }
    }
}

impl std::error::Error for CaptureStartError {}

impl From<io::Error> for CaptureStartError {
    fn from(e: io::Error) -> Self {
        CaptureStartError::Io(e)
    }
}

impl From<CaptureStartError> for io::Error {
    /// Callers that still thread `io::Result` get a preserved, readable message.
    fn from(e: CaptureStartError) -> io::Error {
        match e {
            CaptureStartError::Io(io) => io,
            other => io::Error::other(other.to_string()),
        }
    }
}

/// Enforce the free-space floor on `root`. `Ok(Some(free))` when the check passed with
/// a known figure; `Ok(None)` when free space was unknown and the config explicitly
/// allows proceeding anyway.
pub fn preflight_free_space(
    root: &Path,
    cfg: &CaptureConfig,
    durability: &DurabilityConfig,
) -> Result<Option<u64>, CaptureStartError> {
    let floor = durability.free_space_floor.bytes(cfg);
    match free_space_bytes(root) {
        Ok(free) if free >= floor => Ok(Some(free)),
        Ok(free) => Err(CaptureStartError::NotEnoughSpace {
            free_bytes: free,
            floor_bytes: floor,
        }),
        Err(e) if durability.allow_unknown_free_space => {
            // Explicitly allowed: proceed, but the sidecar records that the floor was
            // never actually verified (`free_space_at_start_bytes: null`).
            let _ = e;
            Ok(None)
        }
        Err(e) => Err(CaptureStartError::FreeSpaceUnknown(e)),
    }
}

// =====================================================================================
// The sidecar
// =====================================================================================

/// How samples are arranged in `telemetry.f32`.
///
/// One variant, deliberately: the writer produces exactly one layout, and naming a
/// second would claim a capability that does not exist. The field is here so a READER
/// validates the layout instead of assuming it - which is the point of a sidecar that
/// survives a `kill -9`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Interleave {
    /// Frame-major: all `n_channels` samples of frame 0, then all of frame 1, ...
    /// Little-endian `f32`, 4 bytes each, no header and no padding.
    FrameInterleaved,
}

/// Where a sidecar's `channel_order` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelOrderSource {
    /// DERIVED from the capture's channel table (its enabled non-drive rows, in table
    /// order) after checking that the count matches `n_channels`. Derived, not asserted
    /// by the producer - a reader should treat it as a strong hint.
    ChannelTableEnabledInputs,
    /// Not determinable: the table's enabled-input count did not match `n_channels`, so
    /// no order is claimed rather than a plausible one guessed.
    Unknown,
}

/// The on-disk layout of `telemetry.f32`, carried by the sidecar so a reader never has
/// to infer it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutDescriptor {
    /// How samples are arranged.
    pub interleave: Interleave,
    /// Frames per pushed chunk, when every chunk carried the same count. `None` means
    /// the producer pushed variable-length chunks - which the format allows (the file
    /// is a flat stream), and which a reader must therefore not assume away.
    pub block_frames: Option<usize>,
    /// Channel ids in interleave-slot order: slot `i` of every frame is
    /// `channel_order[i]`. Empty when [`LayoutDescriptor::channel_order_source`] is
    /// [`ChannelOrderSource::Unknown`].
    pub channel_order: Vec<u32>,
    /// Provenance of `channel_order`.
    pub channel_order_source: ChannelOrderSource,
}

impl LayoutDescriptor {
    /// The claim-nothing layout, used when deserializing a pre-item-12 sidecar with no
    /// `layout` field. The interleave is known (the format never changed); the block
    /// size and channel order are not, and are not guessed.
    pub fn unknown() -> Self {
        LayoutDescriptor {
            interleave: Interleave::FrameInterleaved,
            block_frames: None,
            channel_order: Vec::new(),
            channel_order_source: ChannelOrderSource::Unknown,
        }
    }

    /// Derive the channel order from `table`'s enabled non-drive rows, in table order.
    /// If the count does not match `n_channels` the order is left UNKNOWN - a mismatch
    /// means the producer's interleave is not the table's enabled-input set, and a
    /// plausible-but-wrong order is worse than no order at all.
    pub fn from_table(table: &ChannelTable, n_channels: usize) -> Self {
        let inputs: Vec<u32> = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .map(|c| c.index)
            .collect();
        if inputs.len() == n_channels && n_channels > 0 {
            LayoutDescriptor {
                interleave: Interleave::FrameInterleaved,
                block_frames: None,
                channel_order: inputs,
                channel_order_source: ChannelOrderSource::ChannelTableEnabledInputs,
            }
        } else {
            LayoutDescriptor::unknown()
        }
    }
}

/// The JSON sidecar written alongside the raw binary. Everything a later reader needs
/// to interpret `telemetry.f32`.
///
/// Written repeatedly over a capture's life: at start (`complete: false`), on every
/// flush tick (refreshed counters, still `complete: false`), and at `finish()`
/// (`complete: true`). A file with `complete: false` is a capture that was killed or is
/// still running - and is still fully interpretable, which is the point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureSidecar {
    /// The capture settings (channels, rate, decimation).
    pub config: CaptureConfig,
    /// The channel table (EU / sensitivities / roles) at capture time.
    pub channel_table: ChannelTable,
    /// ISO-ish start timestamp (seconds since the Unix epoch; the UI formats it).
    pub start_unix_secs: u64,
    /// An opaque, serialized snapshot of the run config (band, cal, mode) as JSON text,
    /// so the sidecar is self-contained without this crate depending on the service
    /// struct layout.
    pub run_config_json: String,
    /// Total chunks written to disk.
    pub chunks_written: u64,
    /// Total chunks dropped - ring full, writer faulted, or writer gone. Counted on
    /// EVERY path (quartet rule 4).
    pub chunks_dropped: u64,
    /// Total frames written.
    pub frames_written: u64,
    /// **`false` while the capture is live or if it was killed; `true` only after a
    /// clean `finish()`.** The single field that tells a reader whether the binary is a
    /// complete record.
    #[serde(default)]
    pub complete: bool,
    /// The on-disk layout (scale-8).
    #[serde(default = "LayoutDescriptor::unknown")]
    pub layout: LayoutDescriptor,
    /// Flush + `sync_data` cycles completed. A crash loses at most the data written
    /// since the last one.
    #[serde(default)]
    pub flushes: u64,
    /// Free bytes on the capture volume at start; `None` when free space could not be
    /// determined and the capture was explicitly allowed to proceed anyway.
    #[serde(default)]
    pub free_space_at_start_bytes: Option<u64>,
    /// The free-space floor this capture was required to leave.
    #[serde(default)]
    pub free_space_floor_bytes: u64,
    /// The fault that stopped writing, if any: an I/O error, or the free-space floor
    /// crossed mid-run. `None` on a healthy capture. **A capture that faulted says so
    /// here rather than just ending short.**
    #[serde(default)]
    pub fault: Option<String>,
}

// =====================================================================================
// The sink
// =====================================================================================

/// A chunk handed to the writer thread.
struct WriteChunk {
    samples: Vec<f32>,
    frames: usize,
}

/// Shared counters + fault state between the sink handle and its writer thread.
struct CaptureShared {
    chunks_written: AtomicU64,
    chunks_dropped: AtomicU64,
    frames_written: AtomicU64,
    flushes: AtomicU64,
    /// Set once the writer can no longer write (I/O error, or the free-space floor
    /// crossed). The sink keeps accepting pushes and counting them as drops, so the
    /// producer never blocks and the accounting stays complete.
    faulted: AtomicBool,
    fault_text: Mutex<Option<String>>,
    /// Frames per chunk, if constant. `Some(n)` while every chunk matched; cleared to
    /// `None` by the first differing chunk.
    block_frames: Mutex<Option<usize>>,
    block_frames_seen: AtomicBool,
}

impl CaptureShared {
    fn fault(&self, text: impl Into<String>) {
        // First fault wins: the later ones are usually consequences of the first.
        if !self.faulted.swap(true, Ordering::Relaxed) {
            *self.fault_text.lock().expect("fault text lock") = Some(text.into());
        }
    }

    fn fault_text(&self) -> Option<String> {
        self.fault_text.lock().expect("fault text lock").clone()
    }

    fn observe_block(&self, frames: usize) {
        let mut slot = self.block_frames.lock().expect("block frames lock");
        if !self.block_frames_seen.swap(true, Ordering::Relaxed) {
            *slot = Some(frames);
        } else if *slot != Some(frames) {
            *slot = None;
        }
    }

    fn block_frames(&self) -> Option<usize> {
        *self.block_frames.lock().expect("block frames lock")
    }
}

/// The immutable description of a capture, shared with the writer thread so it can
/// rewrite the sidecar without the sink handle.
struct SidecarBase {
    run_dir: PathBuf,
    cfg: CaptureConfig,
    channel_table: ChannelTable,
    run_config_json: String,
    start_unix_secs: u64,
    layout: LayoutDescriptor,
    free_space_at_start_bytes: Option<u64>,
    free_space_floor_bytes: u64,
}

impl SidecarBase {
    fn snapshot(&self, shared: &CaptureShared, complete: bool) -> CaptureSidecar {
        let mut layout = self.layout.clone();
        layout.block_frames = shared.block_frames();
        CaptureSidecar {
            config: self.cfg.clone(),
            channel_table: self.channel_table.clone(),
            start_unix_secs: self.start_unix_secs,
            run_config_json: self.run_config_json.clone(),
            chunks_written: shared.chunks_written.load(Ordering::Relaxed),
            chunks_dropped: shared.chunks_dropped.load(Ordering::Relaxed),
            frames_written: shared.frames_written.load(Ordering::Relaxed),
            complete,
            layout,
            flushes: shared.flushes.load(Ordering::Relaxed),
            free_space_at_start_bytes: self.free_space_at_start_bytes,
            free_space_floor_bytes: self.free_space_floor_bytes,
            fault: shared.fault_text(),
        }
    }

    /// Write (or rewrite) the sidecar.
    fn write(&self, shared: &CaptureShared, complete: bool) -> io::Result<CaptureSidecar> {
        let sidecar = self.snapshot(shared, complete);
        let json = serde_json::to_string_pretty(&sidecar)
            .map_err(|e| io::Error::other(format!("sidecar serialize: {e}")))?;
        std::fs::write(self.run_dir.join(SIDECAR_FILE), json)?;
        Ok(sidecar)
    }
}

/// A running capture: a dedicated writer thread draining a bounded ring to disk. The
/// reader pushes chunks with [`CaptureSink::push`], which NEVER blocks (drop-on-full).
/// [`CaptureSink::finish`] flushes, syncs, writes the FINAL sidecar, and joins.
pub struct CaptureSink {
    tx: Option<SyncSender<WriteChunk>>,
    join: Option<JoinHandle<()>>,
    shared: Arc<CaptureShared>,
    base: Arc<SidecarBase>,
}

impl CaptureSink {
    /// Start a capture under `data_root/<run_name>/` with the DEFAULT durability
    /// settings. The `io::Result` signature is preserved for every pre-item-12 caller;
    /// a preflight refusal arrives as an `io::Error` whose message names the free bytes
    /// and the floor.
    pub fn start(
        data_root: &Path,
        run_name: &str,
        cfg: CaptureConfig,
        channel_table: ChannelTable,
        run_config_json: String,
    ) -> io::Result<Self> {
        Self::start_durable(
            data_root,
            run_name,
            cfg,
            channel_table,
            run_config_json,
            DurabilityConfig::default(),
        )
        .map_err(io::Error::from)
    }

    /// Start a capture with explicit durability settings.
    ///
    /// Order of operations, and it matters: the free-space preflight runs BEFORE the
    /// run directory is created, so a refused capture leaves no half-made artifacts;
    /// then the binary is opened, the PROVISIONAL sidecar written (`complete: false`),
    /// and only then the writer spawned. From the first instant the capture exists on
    /// disk, it is interpretable.
    pub fn start_durable(
        data_root: &Path,
        run_name: &str,
        cfg: CaptureConfig,
        channel_table: ChannelTable,
        run_config_json: String,
        durability: DurabilityConfig,
    ) -> Result<Self, CaptureStartError> {
        // Preflight against the volume that will hold the capture. `data_root` may not
        // exist yet, so `nearest_existing` walks up - the filesystem is the same either
        // way, and creating the directory to ask about it would leave debris behind a
        // refusal.
        let probe = nearest_existing(data_root);
        let free_at_start = preflight_free_space(&probe, &cfg, &durability)?;
        let floor_bytes = durability.free_space_floor.bytes(&cfg);

        let run_dir = data_root.join(run_name);
        std::fs::create_dir_all(&run_dir)?;
        let bin_path = run_dir.join(TELEMETRY_FILE);
        let file = std::fs::File::create(&bin_path)?;

        let shared = Arc::new(CaptureShared {
            chunks_written: AtomicU64::new(0),
            chunks_dropped: AtomicU64::new(0),
            frames_written: AtomicU64::new(0),
            flushes: AtomicU64::new(0),
            faulted: AtomicBool::new(false),
            fault_text: Mutex::new(None),
            block_frames: Mutex::new(None),
            block_frames_seen: AtomicBool::new(false),
        });
        let base = Arc::new(SidecarBase {
            run_dir: run_dir.clone(),
            layout: LayoutDescriptor::from_table(&channel_table, cfg.n_channels),
            cfg: cfg.clone(),
            channel_table,
            run_config_json,
            start_unix_secs: unix_secs(),
            free_space_at_start_bytes: free_at_start,
            free_space_floor_bytes: floor_bytes,
        });

        // The PROVISIONAL sidecar, before any data is pushed (quartet rule 3). A failure here
        // IS fatal: a capture whose sidecar cannot be written is exactly the
        // uninterpretable-binary case this exists to prevent.
        base.write(&shared, false)?;

        let (tx, rx): (SyncSender<WriteChunk>, Receiver<WriteChunk>) =
            mpsc::sync_channel(cfg.ring_capacity);
        let thread_shared = Arc::clone(&shared);
        let thread_base = Arc::clone(&base);
        let join = thread::Builder::new()
            .name("eigen-capture-writer".into())
            .spawn(move || writer_loop(file, rx, thread_shared, thread_base, durability))?;

        Ok(Self {
            tx: Some(tx),
            join: Some(join),
            shared,
            base,
        })
    }

    /// Push an interleaved chunk. NEVER blocks: on a full ring the chunk is DROPPED and
    /// the drop count bumped (the reader must not stall on disk). Returns `true` if the
    /// chunk was queued, `false` if it was dropped.
    ///
    /// **Every rejection path counts** (quartet rule 4). The earlier code counted only
    /// the ring-full case; a `Disconnected` channel - i.e. the writer thread had died -
    /// returned `false` silently, so the worst failure mode was the one that left the
    /// drop count reading zero.
    pub fn push(&self, samples: &[f32], frames: usize) -> bool {
        let Some(tx) = self.tx.as_ref() else {
            // The sink has been torn down (the ring's sender is released by `finish`
            // and by `Drop`). A push after that is a genuinely lost chunk, so it is
            // counted AND explained - the pre-item-12 code returned a bare `false`.
            self.shared.chunks_dropped.fetch_add(1, Ordering::Relaxed);
            self.shared
                .fault("push after the capture sink was closed; the chunk was discarded");
            return false;
        };
        match tx.try_send(WriteChunk {
            samples: samples.to_vec(),
            frames,
        }) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.shared.chunks_dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                self.shared.chunks_dropped.fetch_add(1, Ordering::Relaxed);
                self.shared.fault("capture writer thread is gone");
                false
            }
        }
    }

    /// The run directory for this capture (where the binary + sidecar + snapshots land).
    pub fn run_dir(&self) -> &Path {
        &self.base.run_dir
    }

    /// Chunks dropped so far, on every path. Queryable LIVE (quartet rule 4).
    pub fn dropped(&self) -> u64 {
        self.shared.chunks_dropped.load(Ordering::Relaxed)
    }

    /// Chunks written so far.
    pub fn written(&self) -> u64 {
        self.shared.chunks_written.load(Ordering::Relaxed)
    }

    /// Frames written so far.
    pub fn frames_written(&self) -> u64 {
        self.shared.frames_written.load(Ordering::Relaxed)
    }

    /// Flush + `sync_data` cycles completed so far.
    pub fn flushes(&self) -> u64 {
        self.shared.flushes.load(Ordering::Relaxed)
    }

    /// The fault that stopped writing, if any. Queryable LIVE, so a UI can surface a
    /// full disk while the run is still going rather than after the fact.
    pub fn fault(&self) -> Option<String> {
        self.shared.fault_text()
    }

    /// A live snapshot of what the sidecar currently says (without rewriting it).
    pub fn status(&self) -> CaptureSidecar {
        self.base.snapshot(&self.shared, false)
    }

    /// Finish the capture: close the ring, join the writer (which does the final flush +
    /// `sync_data`), and write the FINAL sidecar with `complete: true`.
    pub fn finish(mut self) -> io::Result<CaptureSidecar> {
        // Drop the sender to end the writer loop.
        self.tx = None;
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|_| io::Error::other("capture writer thread panicked"))?;
        }
        self.base.write(&self.shared, true)
    }
}

impl Drop for CaptureSink {
    fn drop(&mut self) {
        // If finish() was not called, still stop the writer cleanly. The writer's own
        // exit path flushes + syncs and rewrites the sidecar, which stays
        // `complete: false` - the honest record of a capture that was never finished.
        self.tx = None;
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// The writer thread body: drain the ring to disk, flushing + `sync_data`ing and
/// rewriting the sidecar on the durability cadence, and re-checking free space on the
/// same tick.
///
/// It does NOT exit on an I/O error. On a fault it stops writing, records the reason,
/// and KEEPS DRAINING - counting every further chunk as a drop - so the producer never
/// blocks and the accounting is complete when `finish()` reads it (quartet rule 4).
fn writer_loop(
    file: std::fs::File,
    rx: Receiver<WriteChunk>,
    shared: Arc<CaptureShared>,
    base: Arc<SidecarBase>,
    durability: DurabilityConfig,
) {
    let mut writer = io::BufWriter::new(file);
    let interval = durability.flush_interval.max(Duration::from_millis(1));
    let floor = durability.free_space_floor.bytes(&base.cfg);
    let mut next_flush = Instant::now() + interval;
    let mut dirty = false;

    loop {
        let wait = next_flush.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(chunk) => {
                shared.observe_block(chunk.frames);
                if shared.faulted.load(Ordering::Relaxed) {
                    // Faulted: keep the ring moving, and COUNT what we discard.
                    shared.chunks_dropped.fetch_add(1, Ordering::Relaxed);
                } else {
                    let bytes: &[u8] = f32_as_le_bytes(&chunk.samples);
                    match writer.write_all(bytes) {
                        Ok(()) => {
                            dirty = true;
                            shared.chunks_written.fetch_add(1, Ordering::Relaxed);
                            shared
                                .frames_written
                                .fetch_add(chunk.frames as u64, Ordering::Relaxed);
                        }
                        Err(e) => {
                            shared.fault(format!("capture write failed: {e}"));
                            shared.chunks_dropped.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if Instant::now() >= next_flush {
            next_flush = Instant::now() + interval;
            flush_tick(&mut writer, &shared, &base, floor, &mut dirty);
        }
    }

    // Final flush + sync on the way out - including the un-finished (dropped-sink)
    // path, so a capture abandoned without `finish()` still has its bytes on the
    // platter. The sidecar rewritten here stays `complete: false`.
    flush_tick(&mut writer, &shared, &base, floor, &mut dirty);
}

/// One durability tick: flush the buffer, `sync_data` the file, re-check free space,
/// and rewrite the provisional sidecar.
fn flush_tick(
    writer: &mut io::BufWriter<std::fs::File>,
    shared: &CaptureShared,
    base: &SidecarBase,
    floor: u64,
    dirty: &mut bool,
) {
    if !shared.faulted.load(Ordering::Relaxed) {
        if let Err(e) = writer.flush() {
            shared.fault(format!("capture flush failed: {e}"));
        } else if *dirty {
            // `sync_data`, not `sync_all`: the metadata that matters (the length) is
            // covered, and skipping the inode/timestamp sync is materially cheaper at
            // this cadence. The sidecar carries everything else a reader needs.
            if let Err(e) = writer.get_ref().sync_data() {
                shared.fault(format!("capture fsync failed: {e}"));
            } else {
                *dirty = false;
                shared.flushes.fetch_add(1, Ordering::Relaxed);
            }
        }

        // Mid-run free-space check. Crossing the floor FAULTS the capture (quartet rule 1:
        // "never silently"): writing stops here, with the reason on disk, rather than
        // running the volume to zero and failing somewhere less recoverable.
        if let Ok(free) = free_space_bytes(&base.run_dir) {
            if free < floor {
                shared.fault(format!(
                    "capture stopped: free space {free} bytes fell below the {floor}-byte \
                     floor mid-run"
                ));
            }
        }
    }

    // Best effort: a sidecar rewrite failure must not take the capture down - the
    // counters are still live in memory for `finish()`.
    let _ = base.write(shared, false);
}

/// The nearest existing ancestor of `path` (itself, if it exists). Used to probe the
/// filesystem for a directory that has not been created yet.
fn nearest_existing(path: &Path) -> PathBuf {
    let mut p = path;
    loop {
        if p.exists() {
            return p.to_path_buf();
        }
        match p.parent() {
            Some(parent) => p = parent,
            // No existing ancestor at all (an empty or fully-missing root): "." is the
            // only thing guaranteed to exist for a running process.
            None => return PathBuf::from("."),
        }
    }
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// =====================================================================================
// Snapshots
// =====================================================================================

/// A saved snapshot bundle: the mode-generic envelope (timestamp + opaque settings +
/// operator marks) plus a mode-supplied `payload`. Written by [`save_snapshot`] as a
/// timestamped JSON file in the run folder.
///
/// `P` is the mode's payload type. The pre-lift `Snapshot` mixed generic fields with
/// SMART-sine-specific ones (live spectra, resonance profiles, the frequency/drive
/// estimates); the lift keeps the generic envelope here and pushes every mode-specific
/// field into `payload: P`. This crate never names a mode payload - it only requires
/// that the payload serializes (and derives `Deserialize` too when `P` does, so a
/// reader can round-trip the whole bundle).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot<P> {
    /// Seconds since the Unix epoch when the snapshot was taken.
    pub unix_secs: u64,
    /// Opaque serialized run settings (band, cal, mode) as JSON text.
    pub settings_json: String,
    /// Operator marks passed in from the UI (freeform label + freq/value points).
    pub marks: Vec<Mark>,
    /// The mode-specific payload (SMART: spectra + profiles + estimates; other modes
    /// define their own).
    pub payload: P,
}

/// An operator-placed mark (cursor point) - passed IN from the UI. The service does not
/// create marks; it only records the ones the UI hands it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    /// 1-based channel index the mark is on.
    pub ch: u32,
    /// Frequency of the marked point (Hz).
    pub freq_hz: f64,
    /// Value at the marked point (EU for the trace's domain).
    pub value: f64,
    /// A freeform operator label.
    pub label: String,
}

/// Write a timestamped snapshot bundle into `run_dir` as `snapshot_<unix_secs>.json`.
/// Returns the path written. Generic over the payload type (only `Serialize` is
/// required).
pub fn save_snapshot<P: Serialize>(run_dir: &Path, snapshot: &Snapshot<P>) -> io::Result<PathBuf> {
    std::fs::create_dir_all(run_dir)?;
    let path = run_dir.join(format!("snapshot_{}.json", snapshot.unix_secs));
    let json = serde_json::to_string_pretty(snapshot)
        .map_err(|e| io::Error::other(format!("snapshot serialize: {e}")))?;
    std::fs::write(&path, json)?;
    Ok(path)
}

/// Reinterpret an `&[f32]` slice as its little-endian bytes. Std-only (no `bytemuck`
/// crate): on all supported targets `f32` is 4 bytes and this platform is little-endian,
/// which is the on-disk contract (`telemetry.f32` is LE f32, matching the device).
fn f32_as_le_bytes(samples: &[f32]) -> &[u8] {
    // SAFETY: `[f32]` and `[u8]` have compatible layout; we produce a read-only view of
    // exactly `4 * len` bytes over the same allocation. f32 has no padding/invalid bit
    // patterns for a byte view. The result borrows `samples`, so lifetimes are sound.
    unsafe {
        std::slice::from_raw_parts(
            samples.as_ptr() as *const u8,
            std::mem::size_of_val(samples),
        )
    }
}

#[cfg(test)]
mod tests;
