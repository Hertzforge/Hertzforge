//! Runtime binding to the NI-DAQmx C API.
//!
//! There is NO link-time NI dependency and no bindgen: the ~35 entry points this
//! backend uses are hand-declared from the documented C API and resolved out of
//! the shared library at runtime with `libloading`. Consequences that matter:
//!
//! - The crate compiles on every host, including a Mac that has never seen an NI
//!   driver. An absent driver becomes a typed [`NiError::DriverNotAvailable`],
//!   which the UI shows as "no driver" - it can never silently become
//!   simulated data.
//! - Cross-building the Windows artifact needs no NI SDK and no import library.
//!
//! ## FFI discipline (the one that bites)
//!
//! DAQmx's `int32` status is an error **iff it is negative**. A POSITIVE status
//! is a WARNING and the call SUCCEEDED. Sample-rate coercion - the exact path
//! this design depends on, where the driver rounds a requested rate to one the
//! hardware can produce - arrives as a positive warning. Treating `!= 0` as
//! failure breaks that path. Warnings are logged with their driver text and the
//! call continues; see [`check_status`].
//!
//! ## Symbol resolution
//!
//! Symbols resolve LAZILY, one at a time, and are cached. A symbol this backend
//! never calls is never looked up, and a symbol that is missing produces a
//! per-symbol [`NiError::MissingSymbol`] naming it rather than failing the whole
//! loader. That matters because the optional attributes (ADC overload, task
//! control) are not present on every DAQmx generation, and their absence must
//! degrade to "not checked", never to "backend broken".
//!
//! ## Threading
//!
//! A `TaskHandle` is owned by exactly ONE thread. [`Task`] carries a manual
//! `Send` so the owning thread can be chosen at runtime (the engine's run
//! thread), but nothing in this crate shares a handle across threads
//! concurrently. Enumeration uses its own short-lived [`Driver`] and touches no
//! task.

use crate::error::NiError;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CString};
use std::sync::Mutex;

// NI-DAQmx's TaskHandle is a pointer on every 64-bit target. On 32-bit Windows
// it was historically a `uInt32`, which would silently mis-marshal here.
#[cfg(not(target_pointer_width = "64"))]
compile_error!(
    "hertzforge-hal-ni requires a 64-bit target: DAQmx's TaskHandle is pointer-sized on 64-bit \
     builds and was a 32-bit integer on legacy 32-bit builds, so the hand-declared \
     signatures in ffi.rs would mis-marshal."
);

// ---------------------------------------------------------------------------------------
// C types
// ---------------------------------------------------------------------------------------

/// DAQmx `int32` status. Negative = error, zero = ok, positive = WARNING.
pub type Status = i32;
/// DAQmx `bool32`.
pub type Bool32 = u32;
/// DAQmx `TaskHandle` (`void*` on 64-bit builds).
pub type TaskHandleRaw = *mut c_void;

/// A DAQmx task handle with single-owner-thread discipline.
///
/// # Safety of the `Send` impl
///
/// A `TaskHandle` is an opaque driver-side token; DAQmx permits using a task
/// from a different thread as long as only ONE thread uses it at a time. This
/// crate never shares a handle concurrently: `NiBackend` owns its handles and is
/// itself `&mut`-driven by a single run thread, and the abort path signals that
/// thread rather than calling into the handle from elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Task(pub TaskHandleRaw);

// SAFETY: see the type-level note above - moved between threads, never shared.
unsafe impl Send for Task {}

impl Task {
    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }
}

// ---------------------------------------------------------------------------------------
// Constants
//
// Values are the standard DAQmx `DAQmx_Val_*` enumeration constants. Those marked
// UNVERIFIED were not exercised by the 2026-07-25 live C smoke test on the Linux
// box; `ni-selftest` reads back every one for which DAQmx exposes a getter, so
// they get proven on the box rather than trusted from a doc.
// ---------------------------------------------------------------------------------------

pub mod val {
    /// Sample clock active edge: rising.
    pub const RISING: i32 = 10280;
    /// Acquire/generate continuously.
    pub const CONT_SAMPS: i32 = 10123;
    /// Finite number of samples. UNVERIFIED constant - not used by this backend
    /// today; kept for the selftest's negative controls only.
    pub const FINITE_SAMPS: i32 = 10178;
    /// Interleaved (frame-major) data layout: all channels for sample 0, then
    /// all channels for sample 1, ... This is the layout `hertzforge_hal::Backend`
    /// specifies for both `read_input` and `write_drive`.
    pub const GROUP_BY_SCAN_NUMBER: i32 = 1;
    /// Non-interleaved (channel-major). UNVERIFIED constant - never used here;
    /// present so a future reader does not have to guess which one is which.
    pub const GROUP_BY_CHANNEL: i32 = 0;
    /// Units: volts. **The only AI unit this backend ever asks DAQmx for.**
    ///
    /// There is deliberately no `ACCEL_UNIT_G` / `MVOLTS_PER_G` constant here,
    /// and no binding for `DAQmxCreateAIAccelChan`. Those exist to make the
    /// DRIVER apply a sensor sensitivity - and Hertzforge already does that
    /// host-side (`hertzforge_dsp::eu::volts_to_eu`, the live scope's path). Two
    /// owners would square the sensitivity into a silent `sensitivity^2` error,
    /// and the accel-channel call additionally bakes accelerometer semantics
    /// into a row that might be a force or velocity sensor. One owner, host
    /// side, always. See `config::AiAssign::to_eu`.
    pub const VOLTS: i32 = 10348;
    /// AO idle output behavior: drive 0 V when the task is not running.
    ///
    /// **Safety-critical, and NOT the device default.** Measured on the bench
    /// 2026-08-01 (gate 5, DMM6500 digitizing the AO0 node): with the default
    /// behavior, stopping the AO task leaves the last commanded sample ON THE
    /// WIRE indefinitely. An abort that tripped at 0.500 V left 0.5135 V driving
    /// the output for the rest of the capture, and it was still there when the
    /// NEXT run opened the device 20 seconds later - across process exit. On a
    /// real armature that is a DC command held at the trip level after an abort.
    pub const ZERO_VOLTS: i32 = 12526;
    /// AO idle output behavior: tri-state the output. Never set by this backend;
    /// present so a readback can be NAMED. UNVERIFIED constant.
    pub const HIGH_IMPEDANCE: i32 = 12527;
    /// AO idle output behavior: hold the last written value. **This is what the
    /// hardware does by default** - see [`ZERO_VOLTS`]. Never set by this
    /// backend; present so a readback can be NAMED.
    pub const MAINTAIN_EXISTING_VALUE: i32 = 12528;
    /// Terminal configuration: pseudo-differential (the DSA input shape).
    pub const PSEUDO_DIFF: i32 = 12529;
    /// Terminal configuration: let DAQmx pick the device default.
    pub const CFG_DEFAULT: i32 = -1;
    /// Input coupling: AC. IEPE sensors ride on a DC bias that must be blocked.
    /// UNVERIFIED constant - `ni-selftest` reads it back with `DAQmxGetAICoupling`.
    pub const COUPLING_AC: i32 = 10045;
    /// Input coupling: DC. UNVERIFIED constant - selftest reads it back.
    pub const COUPLING_DC: i32 = 10050;
    /// Input coupling: ground-referenced. UNVERIFIED constant; never set by this
    /// backend, present so a readback value can be NAMED rather than printed raw.
    pub const COUPLING_GND: i32 = 10066;
    /// Excitation source: the device's own internal current source.
    pub const INTERNAL: i32 = 10200;
    /// Excitation source: none.
    pub const NO_EXCITATION: i32 = 10230;
    /// Write regeneration DISALLOWED - the AO buffer is a one-shot stream and
    /// running dry is an underrun, not a silent replay of stale drive.
    pub const DO_NOT_ALLOW_REGEN: i32 = 10158;
    /// Write regeneration allowed. Present so the readback assertion can name
    /// what it is refusing, never set by this backend.
    pub const ALLOW_REGEN: i32 = 10097;

    /// `DAQmxTaskControl` action: commit (reserve + program the hardware).
    /// UNVERIFIED constant - the selftest reports whether the commit call
    /// succeeded so the value gets exercised on the box.
    pub const TASK_COMMIT: i32 = 3;
    /// `DAQmxTaskControl` action: verify. UNVERIFIED constant.
    pub const TASK_VERIFY: i32 = 2;
    /// `DAQmxTaskControl` action: unreserve (release the hardware and reset
    /// buffer/transfer positions). From the NIDAQmx.h ladder Start=0 Stop=1
    /// Verify=2 Commit=3 Reserve=4 Unreserve=5 Abort=6; Verify/Commit above
    /// are the same ladder and are exercised on the box. Needed at `stop()`:
    /// a stopped-but-still-reserved non-regenerating AO task keeps its stale
    /// transfer position, and a subsequent arm/start "succeeds" while the
    /// generation never consumes a frame - `output_space()` frozen at 0,
    /// measured 2026-08-03 (and first recorded 2026-07-30, when it was worked
    /// around by never stopping mid-run).
    pub const TASK_UNRESERVE: i32 = 5;

    /// Default IEPE excitation current in AMPS (2 mA).
    ///
    /// This is a CURRENT, and it is a backend policy value, not a channel-table
    /// value. `hertzforge_channel_table::Channel::iepe_bias_v` is the sensor's bias
    /// VOLTAGE - a readback quantity used to tell a live sensor from an open or
    /// shorted one - and must never be handed to DAQmx as the excitation
    /// parameter. Override per config via `NiConfig::excitation_a`.
    pub const IEPE_EXCITATION_AMPS: f64 = 0.002;
}

/// DAQmx error codes this backend interprets rather than merely reports.
pub mod code {
    /// "Some or all of the samples requested have not yet been acquired" - the
    /// documented steady state of a timed fixed-N read on a continuous task that
    /// simply has not filled a block yet. NOT a fault: it maps to
    /// `InputRead::NoDataYet`. Proven by the read path on the box; the selftest
    /// prints `DAQmxGetErrorString(-200284)` so the mapping is checked against
    /// the driver's own text rather than against this comment.
    pub const OPERATION_TIMED_OUT: i32 = -200284;

    /// Codes that mean the DEVICE IS GONE (unplugged, powered down, pulled from
    /// the chassis) rather than "the driver refused this request".
    ///
    /// UNVERIFIED set. Only `-88705` is high-confidence ("Specified device is
    /// not present or is not active in the system"); the rest are included
    /// because their documented text describes a removed/absent device. The
    /// arrival-day unplug test (gate 7) is what proves this list:
    /// whatever code a real USB pull produces gets added here CITING that
    /// capture. Being wrong is not silent - a device-gone code that is missing
    /// from this list surfaces as `HalError::Io` instead of
    /// `HalError::Disconnected`, i.e. a stop with a less precise reason, never a
    /// continued run.
    pub const DEVICE_GONE: &[i32] = &[
        -88705,  // Specified device is not present or is not active in the system.
        -88710,  // Device removed / no longer accessible.        UNVERIFIED
        -50405,  // No transfer is in progress; transfer aborted. UNVERIFIED
        -200587, // The device is no longer connected.            UNVERIFIED
    ];
}

/// True when `code` is DAQmx's "requested samples not yet acquired" timeout.
pub fn is_timeout(code: i32) -> bool {
    code == code::OPERATION_TIMED_OUT
}

/// True when `code` means the device link is gone rather than merely refused.
pub fn is_device_gone(code: i32) -> bool {
    code::DEVICE_GONE.contains(&code)
}

// ---------------------------------------------------------------------------------------
// Status classification - the error/warning split, isolated so it is testable
// without any driver at all.
// ---------------------------------------------------------------------------------------

/// What a DAQmx status value means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusClass {
    /// Zero: the call succeeded with nothing to say.
    Ok,
    /// Positive: the call SUCCEEDED and the driver has a remark (rate coercion
    /// is the one this design depends on). Never a failure.
    Warning,
    /// Negative: the call failed.
    Error,
}

/// Classify a raw DAQmx status. The whole error/warning discipline is this one
/// function; everything else defers to it.
pub fn classify(status: Status) -> StatusClass {
    match status {
        0 => StatusClass::Ok,
        s if s < 0 => StatusClass::Error,
        _ => StatusClass::Warning,
    }
}

/// Something that can produce the driver's extended error text for the most
/// recent call on the current thread. [`Driver`] implements it by calling
/// `DAQmxGetExtendedErrorInfo`; tests implement it with a canned string so the
/// error/warning split is provable with no driver present.
pub trait ErrorText {
    fn extended_error_info(&self) -> String;
}

/// A retrievable log of driver WARNINGS (positive statuses).
///
/// The workspace has no `log`/`tracing` dependency at this layer, so warnings
/// accumulate here and are surfaced through `BackendStatus::message` and the
/// selftest's per-second report. The rate-coercion warning in particular must
/// reach a human: a silently coerced rate is a requested-vs-actual mismatch
/// that would otherwise be invisible.
#[derive(Debug, Default)]
pub struct WarningLog {
    inner: Mutex<Vec<String>>,
}

impl WarningLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, msg: impl Into<String>) {
        // A poisoned lock must not take the backend down; recover the guard.
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.push(msg.into());
    }

    /// A copy of every warning recorded so far.
    pub fn snapshot(&self) -> Vec<String> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Take every warning recorded so far, emptying the log.
    pub fn drain(&self) -> Vec<String> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *g)
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Apply the DAQmx status discipline to one call's result.
///
/// - `status == 0` -> `Ok(())`, nothing logged.
/// - `status > 0`  -> `Ok(())`, a warning carrying the driver's text is logged.
///   **The call succeeded.**
/// - `status < 0`  -> `Err(NiError::Daqmx { code, text, context })`.
pub fn check_status(
    src: &dyn ErrorText,
    status: Status,
    context: &str,
    warnings: &WarningLog,
) -> Result<(), NiError> {
    match classify(status) {
        StatusClass::Ok => Ok(()),
        StatusClass::Warning => {
            let text = src.extended_error_info();
            warnings.push(format!("DAQmx warning {status} during {context}: {text}"));
            Ok(())
        }
        StatusClass::Error => Err(NiError::Daqmx {
            code: status,
            text: src.extended_error_info(),
            context: context.to_string(),
        }),
    }
}

// ---------------------------------------------------------------------------------------
// String helpers
// ---------------------------------------------------------------------------------------

/// Fixed size for every string-attribute fetch.
///
/// Buffer-sizing policy: **fixed 8 KiB buffers**, not the query-then-fetch
/// (call with NULL/0 to learn the required size) pattern. Rationale: the longest
/// string this backend reads is a device's full physical-channel list, which for
/// a fully populated 8-slot chassis of 32-channel modules is a few kilobytes;
/// 8 KiB has margin. Query-then-fetch doubles the driver round-trips on a path
/// that runs once per scan, and DAQmx signals truncation with a NEGATIVE status
/// (buffer-too-small), which [`check_status`] surfaces as a hard error - so an
/// undersized buffer FAILS LOUDLY rather than silently returning a truncated
/// device list. A silently short device list is exactly the class of bug this
/// project refuses to ship.
pub const STR_BUF: usize = 8192;

/// Convert a Rust string to a NUL-terminated C string, refusing interior NULs
/// rather than truncating at one.
pub fn cstr(s: &str) -> Result<CString, NiError> {
    CString::new(s).map_err(|_| NiError::config(format!("string contains an interior NUL: {s:?}")))
}

/// Read a NUL-terminated C buffer into a Rust `String`, lossily.
fn buf_to_string(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Split a DAQmx comma-separated name list into trimmed, non-empty names.
///
/// DAQmx returns `"Dev1, Dev2, cDAQ9178Sim"` - comma plus a space - and an empty
/// string when there is nothing. Both shapes must produce a clean list.
pub fn split_names(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect()
}

/// The device part of a physical-channel name (`"cDAQ1Mod1/ai0"` -> `"cDAQ1Mod1"`).
///
/// This parse is legal ONLY inside this crate: physical-channel names are the
/// NI backend's own opaque ids (see `hertzforge_hal::discovery`), and no consumer
/// outside this crate may split them.
pub fn device_of(physical: &str) -> &str {
    match physical.find('/') {
        Some(i) => &physical[..i],
        None => physical,
    }
}

/// The trailing channel part of a physical-channel name (`"cDAQ1Mod1/ai0"` ->
/// `"ai0"`); the whole string when there is no `/`. Used only to build the
/// human-readable `DiscoveredChannel::label`.
pub fn channel_label(physical: &str) -> &str {
    match physical.rfind('/') {
        Some(i) => &physical[i + 1..],
        None => physical,
    }
}

// ---------------------------------------------------------------------------------------
// Hand-declared DAQmx signatures
// ---------------------------------------------------------------------------------------

type FnGetSysDevNames = unsafe extern "C" fn(*mut c_char, u32) -> Status;
type FnGetDevStrAttr = unsafe extern "C" fn(*const c_char, *mut c_char, u32) -> Status;
type FnGetDevU32Attr = unsafe extern "C" fn(*const c_char, *mut u32) -> Status;
type FnGetDevI32Attr = unsafe extern "C" fn(*const c_char, *mut i32) -> Status;
type FnGetDevF64Attr = unsafe extern "C" fn(*const c_char, *mut f64) -> Status;
type FnGetDevBoolAttr = unsafe extern "C" fn(*const c_char, *mut Bool32) -> Status;

type FnCreateTask = unsafe extern "C" fn(*const c_char, *mut TaskHandleRaw) -> Status;
type FnTaskOnly = unsafe extern "C" fn(TaskHandleRaw) -> Status;
type FnTaskBoolOut = unsafe extern "C" fn(TaskHandleRaw, *mut Bool32) -> Status;
type FnTaskU32Out = unsafe extern "C" fn(TaskHandleRaw, *mut u32) -> Status;
type FnTaskI32Out = unsafe extern "C" fn(TaskHandleRaw, *mut i32) -> Status;
type FnTaskF64Out = unsafe extern "C" fn(TaskHandleRaw, *mut f64) -> Status;
type FnTaskI32In = unsafe extern "C" fn(TaskHandleRaw, i32) -> Status;
type FnTaskU32In = unsafe extern "C" fn(TaskHandleRaw, u32) -> Status;

// Per-channel AI attribute set/get. DAQmx spells these
// `int32 DAQmxSetAI<Attr>(TaskHandle, const char channel[], <T> data)` and
// `int32 DAQmxGetAI<Attr>(TaskHandle, const char channel[], <T>* data)`.
type FnSetAOChanI32 = unsafe extern "C" fn(TaskHandleRaw, *const c_char, i32) -> Status;
type FnGetAOChanI32 = unsafe extern "C" fn(TaskHandleRaw, *const c_char, *mut i32) -> Status;
type FnSetAIChanI32 = unsafe extern "C" fn(TaskHandleRaw, *const c_char, i32) -> Status;
type FnGetAIChanI32 = unsafe extern "C" fn(TaskHandleRaw, *const c_char, *mut i32) -> Status;
type FnSetAIChanF64 = unsafe extern "C" fn(TaskHandleRaw, *const c_char, f64) -> Status;
type FnGetAIChanF64 = unsafe extern "C" fn(TaskHandleRaw, *const c_char, *mut f64) -> Status;

type FnCreateAIVoltageChan = unsafe extern "C" fn(
    TaskHandleRaw,
    *const c_char, // physicalChannel
    *const c_char, // nameToAssignToChannel
    i32,           // terminalConfig
    f64,           // minVal
    f64,           // maxVal
    i32,           // units
    *const c_char, // customScaleName
) -> Status;

type FnCreateAOVoltageChan = unsafe extern "C" fn(
    TaskHandleRaw,
    *const c_char, // physicalChannel
    *const c_char, // nameToAssignToChannel
    f64,           // minVal
    f64,           // maxVal
    i32,           // units
    *const c_char, // customScaleName
) -> Status;

type FnCfgSampClkTiming = unsafe extern "C" fn(
    TaskHandleRaw,
    *const c_char, // source ("" = onboard)
    f64,           // rate
    i32,           // activeEdge
    i32,           // sampleMode
    u64,           // sampsPerChanToAcquire
) -> Status;

type FnReadAnalogF64 = unsafe extern "C" fn(
    TaskHandleRaw,
    i32,      // numSampsPerChan
    f64,      // timeout (seconds)
    Bool32,   // fillMode
    *mut f64, // readArray
    u32,      // arraySizeInSamps
    *mut i32, // sampsPerChanRead
    *mut u32, // reserved
) -> Status;

type FnWriteAnalogF64 = unsafe extern "C" fn(
    TaskHandleRaw,
    i32,        // numSampsPerChan
    Bool32,     // autoStart
    f64,        // timeout (seconds)
    Bool32,     // dataLayout
    *const f64, // writeArray
    *mut i32,   // sampsPerChanWritten
    *mut u32,   // reserved
) -> Status;

type FnGetExtendedErrorInfo = unsafe extern "C" fn(*mut c_char, u32) -> Status;
type FnGetErrorString = unsafe extern "C" fn(i32, *mut c_char, u32) -> Status;
type FnTaskControl = unsafe extern "C" fn(TaskHandleRaw, i32) -> Status;

// ---------------------------------------------------------------------------------------
// The loader
// ---------------------------------------------------------------------------------------

/// Library names tried, in order, on this platform.
pub fn library_candidates() -> &'static [&'static str] {
    #[cfg(target_os = "linux")]
    {
        &["libnidaqmx.so.1", "libnidaqmx.so"]
    }
    #[cfg(target_os = "windows")]
    {
        &["nicaiu.dll"]
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        &[]
    }
}

/// A loaded NI-DAQmx runtime plus its lazily-resolved symbol table.
pub struct Driver {
    lib: libloading::Library,
    /// Resolved symbol addresses. `None` records a PROVEN-ABSENT symbol so a
    /// missing optional attribute is looked up once, not once per block.
    cache: Mutex<HashMap<&'static str, Option<usize>>>,
    warnings: WarningLog,
    loaded_from: String,
}

impl std::fmt::Debug for Driver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Driver")
            .field("loaded_from", &self.loaded_from)
            .field("warnings", &self.warnings.len())
            .finish_non_exhaustive()
    }
}

impl Driver {
    /// Load the NI-DAQmx runtime.
    ///
    /// On macOS this returns [`NiError::DriverNotAvailable`] WITHOUT attempting a
    /// `dlopen`: NI ships no DAQmx for macOS, so a load attempt could only ever
    /// find something that is not DAQmx. Refusing up front keeps the failure
    /// honest and fast.
    #[cfg(target_os = "macos")]
    pub fn load() -> Result<Driver, NiError> {
        Err(NiError::DriverNotAvailable {
            tried: "none - NI-DAQmx does not exist for macOS".into(),
            detail: "run the NI backend on the Linux or Windows node".into(),
        })
    }

    #[cfg(not(target_os = "macos"))]
    pub fn load() -> Result<Driver, NiError> {
        let candidates = library_candidates();
        let mut detail = String::new();
        for name in candidates {
            // SAFETY: loading a shared library runs its initializers. DAQmx is a
            // system runtime installed by NI's own package; this is the sanctioned
            // way to use it without a link-time dependency.
            match unsafe { libloading::Library::new(name) } {
                Ok(lib) => {
                    return Ok(Driver {
                        lib,
                        cache: Mutex::new(HashMap::new()),
                        warnings: WarningLog::new(),
                        loaded_from: (*name).to_string(),
                    })
                }
                Err(e) => {
                    if !detail.is_empty() {
                        detail.push_str("; ");
                    }
                    detail.push_str(&format!("{name}: {e}"));
                }
            }
        }
        Err(NiError::DriverNotAvailable {
            tried: candidates.join(", "),
            detail,
        })
    }

    /// Which library name actually loaded.
    pub fn loaded_from(&self) -> &str {
        &self.loaded_from
    }

    /// The warning log this driver writes into.
    pub fn warnings(&self) -> &WarningLog {
        &self.warnings
    }

    /// Resolve one symbol, caching both hits and PROVEN misses.
    ///
    /// # Safety
    ///
    /// The caller must instantiate `F` with the exact C signature DAQmx declares
    /// for `name`. Every call site in this crate does so from the hand-declared
    /// types above.
    unsafe fn sym<F: Copy>(&self, name: &'static str) -> Result<F, NiError> {
        debug_assert_eq!(
            std::mem::size_of::<F>(),
            std::mem::size_of::<usize>(),
            "symbol type must be a plain function pointer"
        );
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let entry = match cache.get(name) {
            Some(v) => *v,
            None => {
                let mut owned = name.as_bytes().to_vec();
                owned.push(0);
                let resolved = self.lib.get::<F>(&owned).ok().map(|s| {
                    let f: F = *s;
                    // A function pointer and a `usize` are the same size on
                    // every supported target (the 32-bit `compile_error!` above
                    // is unrelated but rules out the only odd case in practice).
                    std::mem::transmute_copy::<F, usize>(&f)
                });
                cache.insert(name, resolved);
                resolved
            }
        };
        match entry {
            Some(addr) => Ok(std::mem::transmute_copy::<usize, F>(&addr)),
            None => Err(NiError::MissingSymbol {
                symbol: name,
                detail: format!("not exported by {}", self.loaded_from),
            }),
        }
    }

    /// True when `name` resolves. Used to decide whether an OPTIONAL attribute
    /// can be polled at all, without turning its absence into an error.
    pub fn has_symbol(&self, name: &'static str) -> bool {
        // SAFETY: the probe never calls through the pointer; `fn()` is only a
        // size-correct placeholder for the resolution attempt.
        unsafe { self.sym::<unsafe extern "C" fn()>(name).is_ok() }
    }

    fn check(&self, status: Status, context: &str) -> Result<(), NiError> {
        check_status(self, status, context, &self.warnings)
    }

    // --- system / device attributes -----------------------------------------

    pub fn sys_dev_names(&self) -> Result<String, NiError> {
        let mut buf = vec![0 as c_char; STR_BUF];
        // SAFETY: signature matches `int32 DAQmxGetSysDevNames(char*, uInt32)`.
        let st = unsafe {
            let f: FnGetSysDevNames = self.sym("DAQmxGetSysDevNames")?;
            f(buf.as_mut_ptr(), STR_BUF as u32)
        };
        self.check(st, "DAQmxGetSysDevNames")?;
        Ok(buf_to_string(&buf))
    }

    fn dev_str(&self, sym: &'static str, dev: &str) -> Result<String, NiError> {
        let cdev = cstr(dev)?;
        let mut buf = vec![0 as c_char; STR_BUF];
        // SAFETY: every DAQmxGetDev*<string> attribute has this signature.
        let st = unsafe {
            let f: FnGetDevStrAttr = self.sym(sym)?;
            f(cdev.as_ptr(), buf.as_mut_ptr(), STR_BUF as u32)
        };
        self.check(st, &format!("{sym}({dev})"))?;
        Ok(buf_to_string(&buf))
    }

    fn dev_u32(&self, sym: &'static str, dev: &str) -> Result<u32, NiError> {
        let cdev = cstr(dev)?;
        let mut out: u32 = 0;
        // SAFETY: every DAQmxGetDev*<uInt32> attribute has this signature.
        let st = unsafe {
            let f: FnGetDevU32Attr = self.sym(sym)?;
            f(cdev.as_ptr(), &mut out)
        };
        self.check(st, &format!("{sym}({dev})"))?;
        Ok(out)
    }

    fn dev_i32(&self, sym: &'static str, dev: &str) -> Result<i32, NiError> {
        let cdev = cstr(dev)?;
        let mut out: i32 = 0;
        // SAFETY: every DAQmxGetDev*<int32> attribute has this signature.
        let st = unsafe {
            let f: FnGetDevI32Attr = self.sym(sym)?;
            f(cdev.as_ptr(), &mut out)
        };
        self.check(st, &format!("{sym}({dev})"))?;
        Ok(out)
    }

    fn dev_f64(&self, sym: &'static str, dev: &str) -> Result<f64, NiError> {
        let cdev = cstr(dev)?;
        let mut out: f64 = 0.0;
        // SAFETY: every DAQmxGetDev*<float64> attribute has this signature.
        let st = unsafe {
            let f: FnGetDevF64Attr = self.sym(sym)?;
            f(cdev.as_ptr(), &mut out)
        };
        self.check(st, &format!("{sym}({dev})"))?;
        Ok(out)
    }

    fn dev_bool(&self, sym: &'static str, dev: &str) -> Result<bool, NiError> {
        let cdev = cstr(dev)?;
        let mut out: Bool32 = 0;
        // SAFETY: every DAQmxGetDev*<bool32> attribute has this signature.
        let st = unsafe {
            let f: FnGetDevBoolAttr = self.sym(sym)?;
            f(cdev.as_ptr(), &mut out)
        };
        self.check(st, &format!("{sym}({dev})"))?;
        Ok(out != 0)
    }

    pub fn dev_product_type(&self, dev: &str) -> Result<String, NiError> {
        self.dev_str("DAQmxGetDevProductType", dev)
    }
    pub fn dev_product_category(&self, dev: &str) -> Result<i32, NiError> {
        self.dev_i32("DAQmxGetDevProductCategory", dev)
    }
    pub fn dev_serial_num(&self, dev: &str) -> Result<u32, NiError> {
        self.dev_u32("DAQmxGetDevSerialNum", dev)
    }
    pub fn dev_is_simulated(&self, dev: &str) -> Result<bool, NiError> {
        self.dev_bool("DAQmxGetDevIsSimulated", dev)
    }
    pub fn dev_chassis_name(&self, dev: &str) -> Result<String, NiError> {
        self.dev_str("DAQmxGetDevCompactDAQChassisDevName", dev)
    }
    pub fn dev_slot_num(&self, dev: &str) -> Result<u32, NiError> {
        self.dev_u32("DAQmxGetDevCompactDAQSlotNum", dev)
    }
    pub fn dev_ai_physical_chans(&self, dev: &str) -> Result<String, NiError> {
        self.dev_str("DAQmxGetDevAIPhysicalChans", dev)
    }
    pub fn dev_ao_physical_chans(&self, dev: &str) -> Result<String, NiError> {
        self.dev_str("DAQmxGetDevAOPhysicalChans", dev)
    }
    pub fn dev_ai_max_multi_chan_rate(&self, dev: &str) -> Result<f64, NiError> {
        self.dev_f64("DAQmxGetDevAIMaxMultiChanRate", dev)
    }
    pub fn dev_ai_max_single_chan_rate(&self, dev: &str) -> Result<f64, NiError> {
        self.dev_f64("DAQmxGetDevAIMaxSingleChanRate", dev)
    }
    pub fn dev_ao_max_rate(&self, dev: &str) -> Result<f64, NiError> {
        self.dev_f64("DAQmxGetDevAOMaxRate", dev)
    }

    // --- task lifecycle ------------------------------------------------------

    pub fn create_task(&self, name: &str) -> Result<Task, NiError> {
        let cname = cstr(name)?;
        let mut h: TaskHandleRaw = std::ptr::null_mut();
        // SAFETY: `int32 DAQmxCreateTask(const char[], TaskHandle*)`.
        let st = unsafe {
            let f: FnCreateTask = self.sym("DAQmxCreateTask")?;
            f(cname.as_ptr(), &mut h)
        };
        self.check(st, &format!("DAQmxCreateTask({name})"))?;
        Ok(Task(h))
    }

    fn task_only(&self, sym: &'static str, t: Task) -> Result<(), NiError> {
        // SAFETY: `int32 DAQmx<Start|Stop|Clear>Task(TaskHandle)`.
        let st = unsafe {
            let f: FnTaskOnly = self.sym(sym)?;
            f(t.0)
        };
        self.check(st, sym)
    }

    pub fn start_task(&self, t: Task) -> Result<(), NiError> {
        self.task_only("DAQmxStartTask", t)
    }
    pub fn stop_task(&self, t: Task) -> Result<(), NiError> {
        self.task_only("DAQmxStopTask", t)
    }
    pub fn clear_task(&self, t: Task) -> Result<(), NiError> {
        self.task_only("DAQmxClearTask", t)
    }

    pub fn is_task_done(&self, t: Task) -> Result<bool, NiError> {
        let mut out: Bool32 = 0;
        // SAFETY: `int32 DAQmxIsTaskDone(TaskHandle, bool32*)`.
        let st = unsafe {
            let f: FnTaskBoolOut = self.sym("DAQmxIsTaskDone")?;
            f(t.0, &mut out)
        };
        self.check(st, "DAQmxIsTaskDone")?;
        Ok(out != 0)
    }

    /// Optional: `DAQmxTaskControl`. Absence is not an error to the caller.
    pub fn task_control(&self, t: Task, action: i32) -> Result<(), NiError> {
        // SAFETY: `int32 DAQmxTaskControl(TaskHandle, int32)`.
        let st = unsafe {
            let f: FnTaskControl = self.sym("DAQmxTaskControl")?;
            f(t.0, action)
        };
        self.check(st, &format!("DAQmxTaskControl(action {action})"))
    }

    // --- channel creation ----------------------------------------------------

    /// Raw voltage input - the ONLY AI channel type this backend creates.
    ///
    /// Samples arrive in VOLTS and stay volts until the host converts them with
    /// the channel's own sensitivity. There is no `create_ai_accel_chan`
    /// counterpart by design: see [`val::VOLTS`].
    pub fn create_ai_voltage_chan(
        &self,
        t: Task,
        physical: &str,
        terminal_config: i32,
        min_v: f64,
        max_v: f64,
    ) -> Result<(), NiError> {
        let cphys = cstr(physical)?;
        let empty = cstr("")?;
        // SAFETY: matches `DAQmxCreateAIVoltageChan`'s 8-argument declaration.
        let st = unsafe {
            let f: FnCreateAIVoltageChan = self.sym("DAQmxCreateAIVoltageChan")?;
            f(
                t.0,
                cphys.as_ptr(),
                empty.as_ptr(),
                terminal_config,
                min_v,
                max_v,
                val::VOLTS,
                empty.as_ptr(),
            )
        };
        self.check(st, &format!("DAQmxCreateAIVoltageChan({physical})"))
    }

    // --- per-channel AI attributes (coupling + excitation) -------------------

    fn set_ai_chan_i32(
        &self,
        sym: &'static str,
        t: Task,
        chan: &str,
        v: i32,
    ) -> Result<(), NiError> {
        let cchan = cstr(chan)?;
        // SAFETY: `int32 DAQmxSetAI<Attr>(TaskHandle, const char[], int32)`.
        let st = unsafe {
            let f: FnSetAIChanI32 = self.sym(sym)?;
            f(t.0, cchan.as_ptr(), v)
        };
        self.check(st, &format!("{sym}({chan}, {v})"))
    }

    fn get_ai_chan_i32(&self, sym: &'static str, t: Task, chan: &str) -> Result<i32, NiError> {
        let cchan = cstr(chan)?;
        let mut out: i32 = 0;
        // SAFETY: `int32 DAQmxGetAI<Attr>(TaskHandle, const char[], int32*)`.
        let st = unsafe {
            let f: FnGetAIChanI32 = self.sym(sym)?;
            f(t.0, cchan.as_ptr(), &mut out)
        };
        self.check(st, &format!("{sym}({chan})"))?;
        Ok(out)
    }

    // --- per-channel AO attributes (idle output behavior) --------------------

    /// Set what the output DRIVES while the task is not running.
    ///
    /// Applied at configure time rather than on the abort path on purpose: the
    /// abort path is the one place that must not depend on a call succeeding,
    /// and an attribute already committed to the task needs no cooperation from
    /// a task that may already be erroring.
    pub fn set_ao_idle_output_behavior(&self, t: Task, chan: &str, v: i32) -> Result<(), NiError> {
        let cchan = cstr(chan)?;
        // SAFETY: `int32 DAQmxSetAOIdleOutputBehavior(TaskHandle, const char[], int32)`.
        let st = unsafe {
            let f: FnSetAOChanI32 = self.sym("DAQmxSetAOIdleOutputBehavior")?;
            f(t.0, cchan.as_ptr(), v)
        };
        self.check(st, &format!("DAQmxSetAOIdleOutputBehavior({chan}, {v})"))
    }

    /// Read the idle output behavior back. The SET call returning OK is not by
    /// itself proof the device honors it, so the caller reads back and compares.
    pub fn get_ao_idle_output_behavior(&self, t: Task, chan: &str) -> Result<i32, NiError> {
        let cchan = cstr(chan)?;
        let mut out: i32 = 0;
        // SAFETY: `int32 DAQmxGetAOIdleOutputBehavior(TaskHandle, const char[], int32*)`.
        let st = unsafe {
            let f: FnGetAOChanI32 = self.sym("DAQmxGetAOIdleOutputBehavior")?;
            f(t.0, cchan.as_ptr(), &mut out)
        };
        self.check(st, &format!("DAQmxGetAOIdleOutputBehavior({chan})"))?;
        Ok(out)
    }

    fn set_ai_chan_f64(
        &self,
        sym: &'static str,
        t: Task,
        chan: &str,
        v: f64,
    ) -> Result<(), NiError> {
        let cchan = cstr(chan)?;
        // SAFETY: `int32 DAQmxSetAI<Attr>(TaskHandle, const char[], float64)`.
        let st = unsafe {
            let f: FnSetAIChanF64 = self.sym(sym)?;
            f(t.0, cchan.as_ptr(), v)
        };
        self.check(st, &format!("{sym}({chan}, {v})"))
    }

    fn get_ai_chan_f64(&self, sym: &'static str, t: Task, chan: &str) -> Result<f64, NiError> {
        let cchan = cstr(chan)?;
        let mut out: f64 = 0.0;
        // SAFETY: `int32 DAQmxGetAI<Attr>(TaskHandle, const char[], float64*)`.
        let st = unsafe {
            let f: FnGetAIChanF64 = self.sym(sym)?;
            f(t.0, cchan.as_ptr(), &mut out)
        };
        self.check(st, &format!("{sym}({chan})"))?;
        Ok(out)
    }

    /// AC or DC input coupling. IEPE sensors ride a DC bias that AC coupling
    /// blocks; a DC-coupled IEPE channel rails at the bias voltage.
    pub fn set_ai_coupling(&self, t: Task, chan: &str, coupling: i32) -> Result<(), NiError> {
        self.set_ai_chan_i32("DAQmxSetAICoupling", t, chan, coupling)
    }
    pub fn get_ai_coupling(&self, t: Task, chan: &str) -> Result<i32, NiError> {
        self.get_ai_chan_i32("DAQmxGetAICoupling", t, chan)
    }

    /// Excitation SOURCE ([`val::INTERNAL`] or [`val::NO_EXCITATION`]).
    pub fn set_ai_excit_src(&self, t: Task, chan: &str, src: i32) -> Result<(), NiError> {
        self.set_ai_chan_i32("DAQmxSetAIExcitSrc", t, chan, src)
    }
    pub fn get_ai_excit_src(&self, t: Task, chan: &str) -> Result<i32, NiError> {
        self.get_ai_chan_i32("DAQmxGetAIExcitSrc", t, chan)
    }

    /// Excitation VALUE in amps (a current, not the sensor's bias voltage).
    pub fn set_ai_excit_val(&self, t: Task, chan: &str, amps: f64) -> Result<(), NiError> {
        self.set_ai_chan_f64("DAQmxSetAIExcitVal", t, chan, amps)
    }
    pub fn get_ai_excit_val(&self, t: Task, chan: &str) -> Result<f64, NiError> {
        self.get_ai_chan_f64("DAQmxGetAIExcitVal", t, chan)
    }

    pub fn create_ao_voltage_chan(
        &self,
        t: Task,
        physical: &str,
        min_v: f64,
        max_v: f64,
    ) -> Result<(), NiError> {
        let cphys = cstr(physical)?;
        let empty = cstr("")?;
        // SAFETY: matches `DAQmxCreateAOVoltageChan`'s 7-argument declaration.
        let st = unsafe {
            let f: FnCreateAOVoltageChan = self.sym("DAQmxCreateAOVoltageChan")?;
            f(
                t.0,
                cphys.as_ptr(),
                empty.as_ptr(),
                min_v,
                max_v,
                val::VOLTS,
                empty.as_ptr(),
            )
        };
        self.check(st, &format!("DAQmxCreateAOVoltageChan({physical})"))
    }

    // --- timing / buffers ----------------------------------------------------

    /// Continuous sample-clock timing. `source` is `""` for the task's own
    /// onboard clock, or a routed terminal such as `/<chassis>/ai/SampleClock`.
    ///
    /// `samps_per_chan` is DAQmx's buffer-size HINT in continuous mode; the
    /// explicit `cfg_input_buffer`/`cfg_output_buffer` calls override it, and
    /// this backend always makes them.
    pub fn cfg_samp_clk_timing(
        &self,
        t: Task,
        source: &str,
        rate_hz: f64,
        samps_per_chan: u64,
    ) -> Result<(), NiError> {
        let csrc = cstr(source)?;
        // SAFETY: matches `DAQmxCfgSampClkTiming`'s 6-argument declaration.
        let st = unsafe {
            let f: FnCfgSampClkTiming = self.sym("DAQmxCfgSampClkTiming")?;
            f(
                t.0,
                csrc.as_ptr(),
                rate_hz,
                val::RISING,
                val::CONT_SAMPS,
                samps_per_chan,
            )
        };
        self.check(
            st,
            &format!("DAQmxCfgSampClkTiming(source={source:?}, rate={rate_hz})"),
        )
    }

    fn task_f64_out(&self, sym: &'static str, t: Task) -> Result<f64, NiError> {
        let mut out: f64 = 0.0;
        // SAFETY: `int32 DAQmxGet<attr>(TaskHandle, float64*)`.
        let st = unsafe {
            let f: FnTaskF64Out = self.sym(sym)?;
            f(t.0, &mut out)
        };
        self.check(st, sym)?;
        Ok(out)
    }

    fn task_u32_out(&self, sym: &'static str, t: Task) -> Result<u32, NiError> {
        let mut out: u32 = 0;
        // SAFETY: `int32 DAQmxGet<attr>(TaskHandle, uInt32*)`.
        let st = unsafe {
            let f: FnTaskU32Out = self.sym(sym)?;
            f(t.0, &mut out)
        };
        self.check(st, sym)?;
        Ok(out)
    }

    fn task_i32_out(&self, sym: &'static str, t: Task) -> Result<i32, NiError> {
        let mut out: i32 = 0;
        // SAFETY: `int32 DAQmxGet<attr>(TaskHandle, int32*)`.
        let st = unsafe {
            let f: FnTaskI32Out = self.sym(sym)?;
            f(t.0, &mut out)
        };
        self.check(st, sym)?;
        Ok(out)
    }

    fn task_u32_in(&self, sym: &'static str, t: Task, v: u32) -> Result<(), NiError> {
        // SAFETY: `int32 DAQmxCfg<In|Out>putBuffer(TaskHandle, uInt32)`.
        let st = unsafe {
            let f: FnTaskU32In = self.sym(sym)?;
            f(t.0, v)
        };
        self.check(st, &format!("{sym}({v})"))
    }

    fn task_i32_in(&self, sym: &'static str, t: Task, v: i32) -> Result<(), NiError> {
        // SAFETY: `int32 DAQmxSet<attr>(TaskHandle, int32)`.
        let st = unsafe {
            let f: FnTaskI32In = self.sym(sym)?;
            f(t.0, v)
        };
        self.check(st, &format!("{sym}({v})"))
    }

    /// The rate the driver ACTUALLY programmed. May differ from the request; a
    /// coercion arrives as a positive warning on `cfg_samp_clk_timing`, and this
    /// is the value that must propagate through caps/engine/DSP.
    pub fn samp_clk_rate(&self, t: Task) -> Result<f64, NiError> {
        self.task_f64_out("DAQmxGetSampClkRate", t)
    }

    pub fn cfg_input_buffer(&self, t: Task, samps_per_chan: u32) -> Result<(), NiError> {
        self.task_u32_in("DAQmxCfgInputBuffer", t, samps_per_chan)
    }

    pub fn cfg_output_buffer(&self, t: Task, samps_per_chan: u32) -> Result<(), NiError> {
        self.task_u32_in("DAQmxCfgOutputBuffer", t, samps_per_chan)
    }

    pub fn set_write_regen_mode(&self, t: Task, mode: i32) -> Result<(), NiError> {
        self.task_i32_in("DAQmxSetWriteRegenMode", t, mode)
    }

    pub fn get_write_regen_mode(&self, t: Task) -> Result<i32, NiError> {
        self.task_i32_out("DAQmxGetWriteRegenMode", t)
    }

    pub fn read_avail_samp_per_chan(&self, t: Task) -> Result<u32, NiError> {
        self.task_u32_out("DAQmxGetReadAvailSampPerChan", t)
    }

    pub fn write_space_avail(&self, t: Task) -> Result<u32, NiError> {
        self.task_u32_out("DAQmxGetWriteSpaceAvail", t)
    }

    /// Optional: sample mode readback, for verifying `CONT_SAMPS` on the box.
    /// UNVERIFIED symbol name - absence degrades to "not checked".
    pub fn samp_quant_samp_mode(&self, t: Task) -> Result<i32, NiError> {
        self.task_i32_out("DAQmxGetSampQuantSampMode", t)
    }

    /// Optional: active-edge readback, for verifying `RISING` on the box.
    /// UNVERIFIED symbol name - absence degrades to "not checked".
    pub fn samp_clk_active_edge(&self, t: Task) -> Result<i32, NiError> {
        self.task_i32_out("DAQmxGetSampClkActiveEdge", t)
    }

    /// Optional: does any channel in the task currently report ADC overload?
    /// The NI-9234 reports overrange out of band, so this is the only way the
    /// engine learns a channel is railed. UNVERIFIED symbol name - absence
    /// degrades to "not checked", never to an error.
    pub fn read_overloaded_chans_exist(&self, t: Task) -> Result<bool, NiError> {
        let mut out: Bool32 = 0;
        // SAFETY: `int32 DAQmxGetReadOverloadedChansExist(TaskHandle, bool32*)`.
        let st = unsafe {
            let f: FnTaskBoolOut = self.sym("DAQmxGetReadOverloadedChansExist")?;
            f(t.0, &mut out)
        };
        self.check(st, "DAQmxGetReadOverloadedChansExist")?;
        Ok(out != 0)
    }

    // --- data movement -------------------------------------------------------

    /// Fixed-N interleaved read. Returns the RAW status and the frames actually
    /// read, WITHOUT applying [`check_status`] - the caller must decide, because
    /// a timeout with zero frames is the documented steady state (`NoDataYet`),
    /// not a fault.
    ///
    /// # Safety contract
    ///
    /// `out.len()` must be at least `frames * n_channels`.
    pub fn read_analog_f64_raw(
        &self,
        t: Task,
        frames: i32,
        timeout_s: f64,
        out: &mut [f64],
    ) -> Result<(Status, i32), NiError> {
        let mut got: i32 = 0;
        // SAFETY: matches `DAQmxReadAnalogF64`'s 8-argument declaration; `out`
        // is `arraySizeInSamps` long by construction at the call site.
        let st = unsafe {
            let f: FnReadAnalogF64 = self.sym("DAQmxReadAnalogF64")?;
            f(
                t.0,
                frames,
                timeout_s,
                val::GROUP_BY_SCAN_NUMBER as Bool32,
                out.as_mut_ptr(),
                out.len() as u32,
                &mut got,
                std::ptr::null_mut(),
            )
        };
        Ok((st, got))
    }

    /// Interleaved write with `autoStart = 0`. Returns the RAW status and the
    /// frames accepted; the caller decides, because a zero-frame timeout is
    /// legitimate backpressure rather than a fault.
    /// Frames the device has actually GENERATED on this task so far.
    ///
    /// The only way to know that drive written into the AO buffer has reached
    /// the pin. Used by the shutdown taper: on a USB cDAQ the queued drive
    /// cannot be preempted (see `NiBackend::ramp_drive_to_zero`), so the taper
    /// is appended and the abort has to WAIT for generation to catch up to it
    /// rather than stopping the task and truncating it.
    pub fn write_total_samp_per_chan_generated(&self, t: Task) -> Result<u64, NiError> {
        let mut out: u64 = 0;
        // SAFETY: `int32 DAQmxGetWriteTotalSampPerChanGenerated(TaskHandle, uInt64*)`.
        let st = unsafe {
            let f: unsafe extern "C" fn(TaskHandleRaw, *mut u64) -> Status =
                self.sym("DAQmxGetWriteTotalSampPerChanGenerated")?;
            f(t.0, &mut out)
        };
        self.check(st, "DAQmxGetWriteTotalSampPerChanGenerated")?;
        Ok(out)
    }

    pub fn write_analog_f64_raw(
        &self,
        t: Task,
        frames: i32,
        timeout_s: f64,
        data: &[f64],
    ) -> Result<(Status, i32), NiError> {
        let mut written: i32 = 0;
        // SAFETY: matches `DAQmxWriteAnalogF64`'s 8-argument declaration; `data`
        // holds `frames * n_channels` values by construction at the call site.
        let st = unsafe {
            let f: FnWriteAnalogF64 = self.sym("DAQmxWriteAnalogF64")?;
            f(
                t.0,
                frames,
                0, // autoStart = false: start() owns task startup ordering
                timeout_s,
                val::GROUP_BY_SCAN_NUMBER as Bool32,
                data.as_ptr(),
                &mut written,
                std::ptr::null_mut(),
            )
        };
        Ok((st, written))
    }

    /// Apply the status discipline to a status the caller obtained from one of
    /// the `*_raw` entry points.
    pub fn check_raw(&self, status: Status, context: &str) -> Result<(), NiError> {
        self.check(status, context)
    }

    // --- error text ----------------------------------------------------------

    /// The driver's text for an arbitrary code (not just the most recent call).
    /// Used by the selftest to check this crate's interpreted codes against the
    /// driver's own words.
    pub fn error_string(&self, code: i32) -> Result<String, NiError> {
        let mut buf = vec![0 as c_char; STR_BUF];
        // SAFETY: `int32 DAQmxGetErrorString(int32, char*, uInt32)`.
        let st = unsafe {
            let f: FnGetErrorString = self.sym("DAQmxGetErrorString")?;
            f(code, buf.as_mut_ptr(), STR_BUF as u32)
        };
        // A failure to look up text is not itself worth failing over.
        if classify(st) == StatusClass::Error {
            return Err(NiError::Daqmx {
                code: st,
                text: String::new(),
                context: "DAQmxGetErrorString".into(),
            });
        }
        Ok(buf_to_string(&buf))
    }
}

impl ErrorText for Driver {
    fn extended_error_info(&self) -> String {
        let mut buf = vec![0 as c_char; STR_BUF];
        // SAFETY: `int32 DAQmxGetExtendedErrorInfo(char*, uInt32)`.
        let res = unsafe {
            self.sym::<FnGetExtendedErrorInfo>("DAQmxGetExtendedErrorInfo")
                .map(|f| f(buf.as_mut_ptr(), STR_BUF as u32))
        };
        match res {
            // Do NOT recurse through `check` here: this IS the error-text path.
            Ok(st) if classify(st) != StatusClass::Error => buf_to_string(&buf),
            Ok(st) => format!("(DAQmxGetExtendedErrorInfo itself failed: {st})"),
            Err(e) => format!("({e})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the driver's extended-error text, so the status discipline
    /// is provable with no driver present.
    struct FakeText(&'static str);
    impl ErrorText for FakeText {
        fn extended_error_info(&self) -> String {
            self.0.to_string()
        }
    }

    #[test]
    fn classify_is_the_whole_discipline() {
        assert_eq!(classify(0), StatusClass::Ok);
        // POSITIVE = warning. This is the assertion that keeps rate coercion working.
        assert_eq!(classify(200015), StatusClass::Warning);
        assert_eq!(classify(1), StatusClass::Warning);
        assert_eq!(classify(i32::MAX), StatusClass::Warning);
        assert_eq!(classify(-200077), StatusClass::Error);
        assert_eq!(classify(-1), StatusClass::Error);
        assert_eq!(classify(i32::MIN), StatusClass::Error);
    }

    #[test]
    fn zero_status_logs_nothing() {
        let w = WarningLog::new();
        let src = FakeText("should not be read");
        check_status(&src, 0, "CfgSampClkTiming", &w).unwrap();
        assert!(w.is_empty(), "a clean status must not log a warning");
    }

    #[test]
    fn positive_status_is_a_logged_warning_and_the_call_succeeded() {
        let w = WarningLog::new();
        let src = FakeText("Sample clock rate coerced to 51200.000000");
        // The whole point: this returns Ok, not Err.
        check_status(&src, 200015, "DAQmxCfgSampClkTiming", &w)
            .expect("a positive status must NOT be an error");
        let got = w.snapshot();
        assert_eq!(got.len(), 1);
        assert!(got[0].contains("200015"), "{}", got[0]);
        assert!(got[0].contains("coerced"), "{}", got[0]);
        assert!(got[0].contains("DAQmxCfgSampClkTiming"), "{}", got[0]);
    }

    #[test]
    fn negative_status_is_an_error_carrying_code_text_and_context() {
        let w = WarningLog::new();
        let src = FakeText("Requested value is not a supported value for this property.");
        let err = check_status(&src, -200077, "DAQmxCreateAIVoltageChan(Dev1/ai0)", &w)
            .expect_err("a negative status must be an error");
        match err {
            NiError::Daqmx {
                code,
                text,
                context,
            } => {
                assert_eq!(code, -200077);
                assert!(text.contains("not a supported value"), "{text}");
                assert!(context.contains("Dev1/ai0"), "{context}");
            }
            other => panic!("expected Daqmx, got {other:?}"),
        }
        assert!(
            w.is_empty(),
            "an ERROR must not also be logged as a warning"
        );
    }

    #[test]
    fn warning_log_drains_and_reports_length() {
        let w = WarningLog::new();
        assert!(w.is_empty());
        w.push("a");
        w.push("b");
        assert_eq!(w.len(), 2);
        assert_eq!(w.drain(), vec!["a".to_string(), "b".to_string()]);
        assert!(w.is_empty(), "drain must empty the log");
    }

    #[test]
    fn timeout_code_is_recognized_and_nothing_else_is() {
        assert!(is_timeout(-200284));
        assert!(!is_timeout(-200077));
        assert!(!is_timeout(0));
        assert!(
            !is_timeout(200284),
            "a POSITIVE 200284 is a warning, not the timeout"
        );
    }

    #[test]
    fn device_gone_set_is_exactly_the_listed_codes() {
        assert!(is_device_gone(-88705));
        assert!(is_device_gone(-200587));
        // A plain "unsupported value" refusal is NOT a lost link.
        assert!(!is_device_gone(-200077));
        assert!(!is_device_gone(code::OPERATION_TIMED_OUT));
        assert!(!is_device_gone(0));
    }

    #[test]
    fn cstr_refuses_interior_nul_rather_than_truncating() {
        assert!(cstr("cDAQ1Mod1/ai0").is_ok());
        let e = cstr("bad\0name").expect_err("interior NUL must be refused");
        assert!(matches!(e, NiError::Config(_)), "{e:?}");
    }

    #[test]
    fn split_names_handles_the_daqmx_comma_space_shape() {
        assert_eq!(
            split_names("Dev1, Dev2, cDAQ9178Sim"),
            vec!["Dev1", "Dev2", "cDAQ9178Sim"]
        );
        assert_eq!(split_names("Dev1"), vec!["Dev1"]);
        // Empty and whitespace-only lists produce an empty vec, never [""].
        assert!(split_names("").is_empty());
        assert!(split_names("   ").is_empty());
        assert!(split_names(",,").is_empty());
        assert_eq!(split_names("A,,B, "), vec!["A", "B"]);
    }

    #[test]
    fn physical_channel_name_parts() {
        assert_eq!(device_of("cDAQ9178SimMod1/ai0"), "cDAQ9178SimMod1");
        assert_eq!(channel_label("cDAQ9178SimMod1/ai0"), "ai0");
        // No slash: the whole string is both the device and the label.
        assert_eq!(device_of("Dev1"), "Dev1");
        assert_eq!(channel_label("Dev1"), "Dev1");
        // Multiple slashes: device is the FIRST segment, label the LAST.
        assert_eq!(device_of("a/b/c"), "a");
        assert_eq!(channel_label("a/b/c"), "c");
    }

    #[test]
    fn buf_to_string_stops_at_the_first_nul() {
        let mut buf = vec![0 as c_char; 16];
        for (i, b) in b"Dev1".iter().enumerate() {
            buf[i] = *b as c_char;
        }
        assert_eq!(buf_to_string(&buf), "Dev1");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_returns_driver_not_available_without_attempting_a_load() {
        let e = Driver::load().expect_err("macOS has no NI-DAQmx; load must refuse");
        match e {
            NiError::DriverNotAvailable { tried, detail } => {
                assert!(tried.contains("macOS"), "{tried}");
                assert!(
                    detail.contains("Linux") || detail.contains("Windows"),
                    "{detail}"
                );
            }
            other => panic!("expected DriverNotAvailable, got {other:?}"),
        }
        // And the candidate list is empty, proving no dlopen could have happened.
        assert!(library_candidates().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_tries_the_soname_before_the_devel_link() {
        assert_eq!(library_candidates(), &["libnidaqmx.so.1", "libnidaqmx.so"]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_tries_the_documented_dll() {
        assert_eq!(library_candidates(), &["nicaiu.dll"]);
    }
}
