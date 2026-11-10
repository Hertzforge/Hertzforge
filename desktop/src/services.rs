//! The two services the shell talks to, started by the app as child processes on
//! loopback, each on a port the operating system picks (`--port 0`).
//!
//! - The control service (`sds-control`), shipped beside the app's own executable.
//! - The analysis service, `python -I -B -m hertzforge serve`, from the Python runtime
//!   shipped in the app's resources.
//!
//! A development build started by `scripts/dev_app.sh` takes both from the checkout instead
//! ([`Layout::development`]), so a change is tried without packaging the app.
//!
//! Each child's standard output and error go to a log file of this launch, never to a
//! pipe: a pipe would close if the app died, and a write to it would then fail inside
//! the service. Each child's standard input is a pipe the app holds and never writes
//! to (`--watch-stdin`): closing it is how the app stops a service, and if the app
//! dies the operating system closes it, so a service never outlives the app. The app
//! never kills the control service: a killed control service can leave an NI output
//! holding its last sample. It stops any run with its controlled stop when its input
//! closes, and exits once the run has ended.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long a service may take to start listening. The first launch after an
/// install can be slow while the operating system checks every library it loads.
const START_WAIT: Duration = Duration::from_secs(180);
/// How long the app waits for the control service to stop its run and exit. Past
/// this the app exits anyway and leaves the service to finish on its own: its input
/// is closed, so its stop is under way, and killing it is the one thing never done.
pub const CONTROL_STOP_WAIT: Duration = Duration::from_secs(300);
/// How long the analysis service has to exit before it is ended. It drives nothing.
const ANALYSIS_STOP_WAIT: Duration = Duration::from_secs(10);
/// How many launches' logs are kept.
const LOGS_KEPT: usize = 10;

/// Where the parts the app starts are, and where they keep their files.
pub struct Layout {
    pub control_exe: PathBuf,
    pub python_exe: PathBuf,
    pub log_root: PathBuf,
}

impl Layout {
    /// The parts as the installed app lays them out: the control service beside the
    /// app's executable, the Python runtime in its resources.
    pub fn installed(exe_dir: &Path, resource_dir: &Path, log_root: PathBuf) -> Layout {
        let control = if cfg!(windows) {
            "sds-control.exe"
        } else {
            "sds-control"
        };
        let python = if cfg!(windows) {
            resource_dir.join("python").join("python.exe")
        } else {
            resource_dir.join("python").join("bin").join("python3")
        };
        Layout {
            control_exe: exe_dir.join(control),
            python_exe: python,
            log_root,
        }
    }

    /// The parts a development build runs, from the checkout: the control service
    /// `cargo` built and the Python the package is installed in for development, named
    /// by `HERTZFORGE_DEV_CONTROL` and `HERTZFORGE_DEV_PYTHON` (`scripts/dev_app.sh` sets
    /// both). None when either is unset. A release build has no such route.
    #[cfg(debug_assertions)]
    pub fn development(log_root: PathBuf) -> Option<Layout> {
        Some(Layout {
            control_exe: std::env::var_os("HERTZFORGE_DEV_CONTROL")?.into(),
            python_exe: std::env::var_os("HERTZFORGE_DEV_PYTHON")?.into(),
            log_root,
        })
    }

    #[cfg(not(debug_assertions))]
    pub fn development(_log_root: PathBuf) -> Option<Layout> {
        None
    }
}

/// One running service.
pub struct Service {
    pub name: &'static str,
    pub port: u16,
    pub log: PathBuf,
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
}

impl Service {
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}/", self.port)
    }

    /// Close the pipe on the service's standard input: the service stops.
    pub fn close_input(&self) {
        if let Ok(mut s) = self.stdin.lock() {
            s.take();
        }
    }

    /// Wait up to `wait` for the process to exit; true when it has.
    pub fn wait_exit(&self, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        loop {
            if let Ok(mut c) = self.child.lock() {
                match c.try_wait() {
                    Ok(Some(_)) | Err(_) => return true,
                    Ok(None) => {}
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn has_exited(&self) -> bool {
        self.child
            .lock()
            .ok()
            .and_then(|mut c| c.try_wait().ok())
            .is_some_and(|s| s.is_some())
    }

    /// End the process. Only ever called for the analysis service, which drives
    /// nothing; the control service is never killed.
    fn kill(&self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Both services, and this launch's log folder.
pub struct Services {
    pub control: Service,
    pub analysis: Service,
    pub log_dir: PathBuf,
}

impl Services {
    /// Start both services and wait until each says where it listens.
    pub fn start(layout: &Layout) -> Result<Services, String> {
        let log_dir = new_log_dir(&layout.log_root)?;
        for (what, path) in [
            ("the control service", &layout.control_exe),
            ("the Python runtime", &layout.python_exe),
        ] {
            if !path.is_file() {
                return Err(format!(
                    "This installation is incomplete: {what} is missing ({}).",
                    path.display()
                ));
            }
        }
        let mut control = Command::new(&layout.control_exe);
        control.args(["--port", "0", "--watch-stdin"]);
        let control = spawn("control", control, &log_dir)?;

        let mut analysis = Command::new(&layout.python_exe);
        // -I: this runtime's own packages only, whatever PYTHONPATH or a user's
        // site folder says; -B: never write bytecode into the installed app.
        analysis
            .args(["-I", "-B", "-m", "hertzforge", "serve"])
            .args(["--port", "0", "--watch-stdin"]);
        let analysis = match spawn("analysis", analysis, &log_dir) {
            Ok(a) => a,
            Err(e) => {
                control.close_input();
                return Err(e);
            }
        };
        let services = Services {
            control,
            analysis,
            log_dir,
        };
        let ready = wait_listening(&services.control)
            .and_then(|p| Ok((p, wait_listening(&services.analysis)?)));
        match ready {
            Ok((c, a)) => Ok(Services {
                control: Service {
                    port: c,
                    ..services.control
                },
                analysis: Service {
                    port: a,
                    ..services.analysis
                },
                log_dir: services.log_dir,
            }),
            Err(e) => {
                services.stop(|_| {});
                Err(e)
            }
        }
    }

    /// Stop both: close their inputs, wait for the control service to stop any run
    /// and exit (never killing it), and give the analysis service a moment before
    /// ending it. `say` hears what happens, for the app's log.
    pub fn stop(&self, say: impl Fn(&str)) {
        self.control.close_input();
        self.analysis.close_input();
        if self.control.wait_exit(CONTROL_STOP_WAIT) {
            say("the control service has exited");
        } else {
            say(&format!(
                "the control service had not exited after {} s; leaving it to finish its stop",
                CONTROL_STOP_WAIT.as_secs()
            ));
        }
        if !self.analysis.wait_exit(ANALYSIS_STOP_WAIT) {
            say("the analysis service did not exit; ending it");
            self.analysis.kill();
        }
    }
}

/// A folder for this launch's logs, under `root`; the oldest beyond
/// [`LOGS_KEPT`] are removed.
fn new_log_dir(root: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(root)
        .map_err(|e| format!("Cannot make the log folder {}: {e}", root.display()))?;
    let mut old: Vec<PathBuf> = fs::read_dir(root)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("launch-"))
                })
                .collect()
        })
        .unwrap_or_default();
    old.sort();
    while old.len() >= LOGS_KEPT {
        let _ = fs::remove_dir_all(old.remove(0));
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dir = root.join(format!("launch-{secs}-{}", std::process::id()));
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Cannot make the log folder {}: {e}", dir.display()))?;
    Ok(dir)
}

fn spawn(name: &'static str, mut cmd: Command, log_dir: &Path) -> Result<Service, String> {
    let log = log_dir.join(format!("{name}.log"));
    let out =
        File::create(&log).map_err(|e| format!("Cannot write the log {}: {e}", log.display()))?;
    let err = out
        .try_clone()
        .map_err(|e| format!("Cannot write the log {}: {e}", log.display()))?;
    cmd.current_dir(log_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    #[cfg(unix)]
    {
        // A group of its own: a Ctrl-C typed at a terminal that started the app
        // reaches the app alone, and the service learns of it from its input.
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        // No console window for the service.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Cannot start the {name} service: {e}"))?;
    let stdin = child.stdin.take();
    Ok(Service {
        name,
        port: 0,
        log,
        child: Mutex::new(child),
        stdin: Mutex::new(stdin),
    })
}

/// The port in a service's "listening on ws://127.0.0.1:<port>/" line.
pub fn listening_port(log: &str) -> Option<u16> {
    log.lines().find_map(|line| {
        let at = line.split("listening on ws://").nth(1)?;
        let host_port = &at[..at.find('/')?];
        let (host, port) = host_port.rsplit_once(':')?;
        if host != "127.0.0.1" {
            return None;
        }
        port.parse().ok()
    })
}

fn wait_listening(s: &Service) -> Result<u16, String> {
    let deadline = Instant::now() + START_WAIT;
    loop {
        let text = fs::read_to_string(&s.log).unwrap_or_default();
        if let Some(port) = listening_port(&text) {
            return Ok(port);
        }
        if s.has_exited() {
            return Err(format!(
                "The {} service stopped as it started. Its log, {}, ends:\n\n{}",
                s.name,
                s.log.display(),
                tail(&text, 12)
            ));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "The {} service did not start within {} s. Its log is {}.",
                s.name,
                START_WAIT.as_secs(),
                s.log.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The app's own log of a launch: what it started, and how it stopped.
pub struct AppLog(Mutex<Option<File>>);

impl AppLog {
    pub fn new() -> AppLog {
        AppLog(Mutex::new(None))
    }

    pub fn open(&self, dir: &Path) {
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("app.log"))
            .ok();
        if let Ok(mut slot) = self.0.lock() {
            *slot = f;
        }
    }

    pub fn say(&self, line: &str) {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let line = format!("{secs:.3} {line}");
        if let Ok(mut slot) = self.0.lock() {
            if let Some(f) = slot.as_mut() {
                let _ = writeln!(f, "{line}");
            }
        }
        // A terminal that started the app sees it too; a closed one is ignored.
        let _ = writeln!(std::io::stderr(), "hertzforge: {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_port_is_read_from_either_services_line() {
        let control = "sds-control 1 listening on ws://127.0.0.1:61234/ (protocol sds/1)\n";
        assert_eq!(listening_port(control), Some(61234));
        let analysis = "2026-11-02 INFO x\nhertzforge analysis service listening on \
                        ws://127.0.0.1:55001/\n";
        assert_eq!(listening_port(analysis), Some(55001));
        assert_eq!(listening_port("starting\n"), None);
        // Only loopback, and only a whole line's port.
        assert_eq!(listening_port("listening on ws://0.0.0.0:80/\n"), None);
        assert_eq!(listening_port("listening on ws://127.0.0.1:99999/\n"), None);
        assert_eq!(listening_port("listening on ws://127.0.0.1:12"), None);
    }

    #[test]
    fn old_launch_logs_are_pruned_to_the_newest() {
        let root = std::env::temp_dir().join(format!("hf-desktop-logs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for i in 0..(LOGS_KEPT + 3) {
            fs::create_dir_all(root.join(format!("launch-{:010}-1", i))).unwrap();
        }
        fs::create_dir_all(root.join("other")).unwrap();
        let dir = new_log_dir(&root).unwrap();
        let mut names: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("launch-"))
            .collect();
        names.sort();
        assert_eq!(names.len(), LOGS_KEPT);
        assert!(!names.contains(&format!("launch-{:010}-1", 0)));
        assert!(dir.is_dir());
        assert!(
            root.join("other").is_dir(),
            "only launch folders are pruned"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
