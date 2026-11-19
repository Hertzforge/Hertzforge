//! THE MUTUAL-EXCLUSION ADVISORY LOCK - one process owns the NI hardware at a time.
//!
//! # It lives in `hertzforge-station` because BOTH sides read it
//!
//! `hertzforge-backendd` WRITES it; the Tauri desktop app READS it, to turn "the NI station
//! will not arm" into "hertzforge-backendd (pid 4711) on bench-host has it; stop it". Two
//! copies of a JSON shape that must agree is a drift hazard with a guaranteed
//! symptom - a lock one side cannot parse reads as "nobody is holding it", which is
//! precisely the wrong answer. So the shape and both operations live once, in the
//! crate both front ends already depend on.
//!
//! # The problem this solves
//!
//! The daemon and the desktop app are two programs that both want the SAME DAQmx
//! devices, and DAQmx's own refusal when the second one tries is a negative status
//! code three layers down, at arm, naming a resource - not naming the program that
//! has it. The operator sees "the NI station will not arm" and has no way to know
//! that the daemon they started this morning is holding it.
//!
//! So the holder writes a lockfile that IDENTIFIES ITSELF, and the other program
//! reads it and says who to go stop.
//!
//! # Why a lockfile and not an OS lock
//!
//! An `flock` would be tidier and would say nothing. The whole value here is the
//! CONTENT - pid, host, bind address, start time - because the error message the
//! operator needs is "hertzforge-backendd (pid 4711) on bench-host has the NI hardware;
//! stop it with `hertzforge-backendd --stop`", and no kernel primitive carries that.
//!
//! # And what it is NOT
//!
//! **ADVISORY.** It cannot stop a program that does not check. It is not a safety
//! interlock, and nothing in the arm chain consults it - DAQmx's own reservation is
//! the real exclusion, and it holds whether or not this file exists. This is a
//! DIAGNOSTIC that turns an unexplainable refusal into an actionable one.
//!
//! The desktop app's half of this (`ui/src-tauri/src/hardware.rs`) offers "stop the
//! daemon" and NEVER offers a Sim path. Falling back to the software plant because
//! hardware is busy is the exact substitution this design exists to make
//! impossible - a plant is not a measurement, and "the hardware was busy" is not a
//! reason to produce data that looks like one.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The lockfile's name inside the data root. The desktop app looks for exactly this.
pub const LOCK_FILE: &str = "hertzforge-backendd.lock";

/// What the holder writes about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockHolder {
    /// The holder's process id, so a reader can check whether it is still alive.
    pub pid: u32,
    /// The host it runs on. Present because a data root can be on a share, and a
    /// stale lock from a different machine must be recognizable as such.
    pub host: String,
    /// What it is serving on, so the operator can find it.
    pub bind: String,
    /// Unix seconds at acquisition.
    pub since_unix: u64,
    /// The station it claimed (`"sim"` / `"ni"`). A Sim holder does NOT block the
    /// app's NI station - it holds no devices - and saying which it is lets the
    /// reader make that distinction.
    pub station: String,
}

impl LockHolder {
    /// The sentence the other program shows the operator.
    pub fn advice(&self) -> String {
        format!(
            "hertzforge-backendd (pid {}) on {} has the {} station, serving {} since unix \
             {}. Stop it with Ctrl-C in its console, or on Linux and macOS with \
             `hertzforge-backendd --stop --config <its config>` (or `kill {}` on that host), \
             then rescan. A forced kill (`kill -9`, `taskkill /F`) skips the safe stop.",
            self.pid, self.host, self.station, self.bind, self.since_unix, self.pid
        )
    }

    /// Does this holder claim HARDWARE? A Sim holder blocks nothing on the DAQmx
    /// side, and reporting it as a blocker would train the operator to ignore the
    /// message that matters.
    pub fn holds_hardware(&self) -> bool {
        self.station == "ni"
    }
}

/// Where the lockfile lives for a given data root.
pub fn lock_path(data_root: &Path) -> PathBuf {
    data_root.join(LOCK_FILE)
}

/// Read the lock, if a live one exists.
///
/// A lock whose pid is NOT running is STALE and answers `None`: a `kill -9`d daemon
/// must not lock the hardware out forever. A lock from another HOST is reported as
/// present (we cannot check its pid) - the operator is shown who claims it and
/// decides.
pub fn read_live(data_root: &Path) -> Option<LockHolder> {
    let text = std::fs::read_to_string(lock_path(data_root)).ok()?;
    let holder: LockHolder = serde_json::from_str(&text).ok()?;
    if holder.host == hostname() && !pid_is_alive(holder.pid) {
        return None;
    }
    Some(holder)
}

/// An acquired lock. Releases on drop.
#[derive(Debug)]
pub struct LockGuard {
    path: PathBuf,
    holder: LockHolder,
}

impl LockGuard {
    /// Who this guard says holds the lock.
    pub fn holder(&self) -> &LockHolder {
        &self.holder
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Best effort. A lock left behind by a hard kill is recognized as stale by
        // `read_live` (dead pid), so failing to remove it is recoverable; panicking
        // in a Drop during shutdown is not.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Why the lock could not be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// Someone else has it.
    Held(LockHolder),
    /// The file could not be written.
    Io(String),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Held(h) => write!(
                f,
                "another Hertzforge process already holds this data root: {}",
                h.advice()
            ),
            LockError::Io(m) => write!(f, "could not write the advisory lock: {m}"),
        }
    }
}

impl std::error::Error for LockError {}

/// Take the lock for this process.
pub fn acquire(data_root: &Path, bind: &str, station: &str) -> Result<LockGuard, LockError> {
    if let Some(existing) = read_live(data_root) {
        return Err(LockError::Held(existing));
    }
    std::fs::create_dir_all(data_root).map_err(|e| LockError::Io(e.to_string()))?;
    let holder = LockHolder {
        pid: std::process::id(),
        host: hostname(),
        bind: bind.to_string(),
        since_unix: now_unix(),
        station: station.to_string(),
    };
    let path = lock_path(data_root);
    let json = serde_json::to_string_pretty(&holder).map_err(|e| LockError::Io(e.to_string()))?;
    std::fs::write(&path, json).map_err(|e| LockError::Io(e.to_string()))?;
    Ok(LockGuard { path, holder })
}

/// Is this pid a live process?
///
/// Unix: `/proc/<pid>` where it exists, else `ps -p <pid>`. Windows: open the
/// process for a limited query and ask whether it is still running (below). Any
/// other platform, and any answer that cannot be read, is TRUE (assume alive),
/// which is the conservative direction: a false "alive" costs the operator one
/// manual file deletion, a false "dead" costs two processes fighting over a DAQmx
/// device mid-run.
///
/// Windows used to take the "any other platform" branch, so on Windows a daemon
/// that was killed or crashed left a lock that read as LIVE forever - the stale
/// case this function exists to detect never fired there.
fn pid_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY-FREE: no unsafe here. `/proc` is Linux-only, so use the portable
        // trick of asking the OS through std where possible; on unix without /proc
        // we fall back to assuming alive (see the doc comment).
        let proc_entry = std::path::Path::new("/proc").join(pid.to_string());
        if std::path::Path::new("/proc").is_dir() {
            return proc_entry.exists();
        }
        // macOS and the BSDs: no /proc. `ps -p <pid>` exits 0 when it exists.
        match std::process::Command::new("ps")
            .arg("-p")
            .arg(pid.to_string())
            .output()
        {
            Ok(out) => out.status.success(),
            Err(_) => true,
        }
    }
    #[cfg(windows)]
    {
        windows_pid_is_alive(pid)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

/// Windows liveness, asked of the OS the same way the macOS arm asks `ps`: run
/// `tasklist /FI "PID eq <pid>" /FO CSV /NH` and look for the pid in the PID column.
///
/// A subprocess rather than `OpenProcess`, on purpose: this crate is
/// `#![forbid(unsafe_code)]` and this module is safety-free by design, and a lock
/// read happens at acquire and on an operator rescan, never on a hot path.
/// `CREATE_NO_WINDOW` keeps the probe from flashing a console over the desktop
/// app, which reads this lock too.
///
/// - pid 0 is the System Idle Process, a kernel pseudo-process that `tasklist`
///   DOES list but that no program can run as, so a lock naming it is stale by
///   construction (unix answers the same for pid 0, which is why the stale-lock
///   test uses it).
/// - `tasklist` cannot run, or exits non-zero: cannot tell, so alive.
/// - It ran: alive exactly when a CSV row carries the pid in its second column.
///   A no-match run prints a localized `INFO:` line instead of a row, which this
///   never parses as a match.
#[cfg(windows)]
fn windows_pid_is_alive(pid: u32) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if pid == 0 {
        return false;
    }
    match std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(out) if out.status.success() => {
            tasklist_csv_lists_pid(&String::from_utf8_lossy(&out.stdout), pid)
        }
        _ => true,
    }
}

/// Does `tasklist /FO CSV /NH` output carry a row whose PID column is `pid`?
/// Rows look like `"cmd.exe","1234","Console","1","3,456 K"`; the PID is the
/// second quoted field. Pure, so it is tested on every host.
#[cfg(any(windows, test))]
fn tasklist_csv_lists_pid(csv: &str, pid: u32) -> bool {
    let want = pid.to_string();
    csv.lines().any(|line| {
        let line = line.trim();
        line.starts_with('"')
            && line
                .split("\",\"")
                .nth(1)
                .map(|field| field.trim_matches('"'))
                == Some(want.as_str())
    })
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "hertzforge-backendd-lock-{}-{}-{}",
            name,
            std::process::id(),
            now_unix()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// THE EXCLUSION. A second acquire fails, and the failure NAMES the holder -
    /// which is the entire reason this is a file with content rather than an flock.
    ///
    /// RED-MUTATION: make `acquire` skip the `read_live` check and this reds; the
    /// second daemon starts, and the operator's NI station fails at arm with a DAQmx
    /// status code that names a resource instead of a program.
    #[test]
    fn a_second_holder_is_refused_and_the_refusal_names_the_first() {
        let root = tmp("exclusion");
        let first = acquire(&root, "192.168.4.20:8765", "ni").expect("first acquires");
        assert_eq!(first.holder().pid, std::process::id());

        let err = acquire(&root, "192.168.4.20:8766", "ni").unwrap_err();
        match &err {
            LockError::Held(h) => {
                assert_eq!(h.pid, std::process::id());
                assert_eq!(h.bind, "192.168.4.20:8765");
                assert!(h.holds_hardware());
            }
            other => panic!("expected Held, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(msg.contains("hertzforge-backendd"), "{msg}");
        assert!(msg.contains("--stop"), "names the remedy: {msg}");
        assert!(
            !msg.to_lowercase().contains("sim"),
            "the remedy must never be a Sim fallback: {msg}"
        );

        // Releasing lets the next one in.
        drop(first);
        assert!(acquire(&root, "x:1", "ni").is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A lock left behind by a hard kill (dead pid, same host) is STALE and does not
    /// lock the hardware out forever.
    #[test]
    fn a_dead_holders_lock_is_stale_and_ignored() {
        let root = tmp("stale");
        // pid 0 is never a live user process; the reader treats it as dead.
        let dead = LockHolder {
            pid: 0,
            host: hostname(),
            bind: "x:1".into(),
            since_unix: 1,
            station: "ni".into(),
        };
        std::fs::write(lock_path(&root), serde_json::to_string(&dead).unwrap()).unwrap();
        assert!(
            read_live(&root).is_none(),
            "a dead holder must not hold the lock"
        );
        assert!(acquire(&root, "x:2", "ni").is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The liveness probe itself, on every platform: this process is alive, and a
    /// child that has exited and been waited for is dead.
    ///
    /// RED-MUTATION: make `pid_is_alive` answer `true` unconditionally - which is
    /// what Windows did before it had its own arm - and the exited-child half reds,
    /// as does `a_dead_holders_lock_is_stale_and_ignored` (the failure the Windows
    /// bench recorded).
    #[test]
    fn the_liveness_probe_tells_a_running_process_from_an_exited_one() {
        assert!(pid_is_alive(std::process::id()), "this process is running");
        let spawned = if cfg!(windows) {
            std::process::Command::new("cmd")
                .args(["/C", "exit 0"])
                .spawn()
        } else {
            std::process::Command::new("true").spawn()
        };
        let mut child = spawned.expect("spawn a short-lived child");
        let pid = child.id();
        child.wait().expect("the child exits");
        drop(child);
        assert!(
            !pid_is_alive(pid),
            "an exited, waited-for child must read dead (pid {pid})"
        );
    }

    /// The Windows probe's parser, on every host. A row with the pid in the PID
    /// column is a match; the localized no-match `INFO:` line, a different pid, and a
    /// pid that merely appears inside another field (here the image name) are not.
    ///
    /// RED-MUTATION: match on `line.contains(&want)` instead of the PID column and
    /// the image-name case reds.
    #[test]
    fn the_tasklist_parser_reads_only_the_pid_column() {
        let row = "\"hertzforge-backendd.exe\",\"4711\",\"Console\",\"1\",\"12,345 K\"\r\n";
        assert!(tasklist_csv_lists_pid(row, 4711));
        assert!(!tasklist_csv_lists_pid(row, 471));
        assert!(!tasklist_csv_lists_pid(row, 12));
        let name_only = "\"svc4711.exe\",\"8\",\"Services\",\"0\",\"9,120 K\"\r\n";
        assert!(!tasklist_csv_lists_pid(name_only, 4711));
        assert!(!tasklist_csv_lists_pid(
            "INFO: No tasks are running which match the specified criteria.\r\n",
            4711
        ));
        assert!(!tasklist_csv_lists_pid(
            "INFORMATION: Es werden keine Aufgaben mit den angegebenen Kriterien ausgef\u{fc}hrt.\r\n",
            4711
        ));
        assert!(!tasklist_csv_lists_pid("", 4711));
    }

    /// A holder from ANOTHER host is reported (we cannot check its pid), so the
    /// operator sees who claims it rather than having it silently ignored.
    #[test]
    fn a_foreign_hosts_lock_is_reported_not_ignored() {
        let root = tmp("foreign");
        let far = LockHolder {
            pid: 1,
            host: "some-other-box".into(),
            bind: "10.0.0.9:8765".into(),
            since_unix: 1,
            station: "ni".into(),
        };
        std::fs::write(lock_path(&root), serde_json::to_string(&far).unwrap()).unwrap();
        let seen = read_live(&root).expect("a foreign lock is visible");
        assert_eq!(seen.host, "some-other-box");
        assert!(seen.advice().contains("some-other-box"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A SIM holder holds no DAQmx devices, and the app must not report it as an
    /// NI blocker - a warning that is usually wrong is a warning that gets ignored.
    #[test]
    fn a_sim_holder_does_not_claim_hardware() {
        let sim = LockHolder {
            pid: 1,
            host: "h".into(),
            bind: "x:1".into(),
            since_unix: 1,
            station: "sim".into(),
        };
        assert!(!sim.holds_hardware());
    }
}
