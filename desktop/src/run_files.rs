//! Run folders as files a person manages: where runs are kept, showing a run's folder in
//! the file manager, and the files the operating system asks the app to open.
//!
//! **The data folder.** The control service writes each run's folder under its data folder,
//! the folder `HERTZFORGE_DATA_ROOT` names, else `Hertzforge_Data` in the home folder
//! (`docs/run-folder.md`). The app passes the folder to both services it starts, in that
//! variable, so the analysis service names the same one. A folder chosen in the app is kept in
//! the app's settings file and wins over the variable; it takes effect the next time the
//! services start. Choosing one is refused while a run is live and when the folder cannot be
//! written, and never moves or deletes a run already kept. Deleting a run is the analysis
//! service's (`runs.delete`), not the app's.
//!
//! **Opening a run.** A plain folder cannot be associated with an app on Windows or Linux, so a
//! run folder opens through its `run.json`, as a `.hforge` project does by its own suffix: the
//! paths the app is opened with (its arguments, or the operating system's open event on macOS)
//! are queued here, and the shell takes them and opens each as File > Open does.

use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// The variable the control service reads its data folder from.
pub const DATA_ROOT_ENV: &str = "HERTZFORGE_DATA_ROOT";
/// The data folder's name in the home folder when nothing names one.
pub const DATA_ROOT_DIR_NAME: &str = "Hertzforge_Data";
/// The app's settings file, in its configuration folder.
const SETTINGS_FILE: &str = "settings.json";
/// The settings key of the data folder chosen in the app.
const DATA_FOLDER_KEY: &str = "data_folder";

/// Where a data folder comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Chosen in the app.
    Setting,
    /// `HERTZFORGE_DATA_ROOT`, set where the app was started.
    Environment,
    /// `Hertzforge_Data` in the home folder.
    Default,
}

impl Source {
    fn word(self) -> &'static str {
        match self {
            Source::Setting => "setting",
            Source::Environment => "environment",
            Source::Default => "default",
        }
    }
}

/// The data folder: the one chosen in the app, else the one the variable names (an empty
/// value is unset, as the control service reads it), else `Hertzforge_Data` in the home
/// folder, else in the working folder when there is no home folder.
pub fn resolve(
    chosen: Option<&Path>,
    env: Option<OsString>,
    home: Option<PathBuf>,
) -> (PathBuf, Source) {
    if let Some(chosen) = chosen {
        return (chosen.to_path_buf(), Source::Setting);
    }
    if let Some(named) = env.filter(|v| !v.is_empty()) {
        return (PathBuf::from(named), Source::Environment);
    }
    match home.filter(|h| !h.as_os_str().is_empty()) {
        Some(home) => (home.join(DATA_ROOT_DIR_NAME), Source::Default),
        None => (PathBuf::from(DATA_ROOT_DIR_NAME), Source::Default),
    }
}

/// The folder resolved where the app runs.
pub fn resolve_here(chosen: Option<&Path>) -> (PathBuf, Source) {
    resolve(
        chosen,
        std::env::var_os(DATA_ROOT_ENV),
        std::env::home_dir(),
    )
}

fn read_settings(config_dir: &Path) -> Map<String, Value> {
    fs::read_to_string(config_dir.join(SETTINGS_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// The data folder chosen in the app, or none.
pub fn read_chosen(config_dir: &Path) -> Option<PathBuf> {
    read_settings(config_dir)
        .get(DATA_FOLDER_KEY)
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

/// Keep `chosen` as the data folder (none: back to the variable or the default), leaving the
/// file's other settings as they were. Written whole to a temporary name and renamed into place.
pub fn write_chosen(config_dir: &Path, chosen: Option<&Path>) -> Result<(), String> {
    let mut settings = read_settings(config_dir);
    match chosen {
        Some(p) => settings.insert(
            DATA_FOLDER_KEY.into(),
            Value::String(p.to_string_lossy().into_owned()),
        ),
        None => settings.remove(DATA_FOLDER_KEY),
    };
    fs::create_dir_all(config_dir).map_err(|e| {
        format!(
            "The settings folder {} cannot be made: {e}",
            config_dir.display()
        )
    })?;
    let file = config_dir.join(SETTINGS_FILE);
    let partial = config_dir.join(format!(".{SETTINGS_FILE}.{}.partial", std::process::id()));
    let text = serde_json::to_string_pretty(&Value::Object(settings)).map_err(|e| e.to_string())?;
    let written = fs::write(&partial, text).and_then(|_| fs::rename(&partial, &file));
    if let Err(e) = written {
        let _ = fs::remove_file(&partial);
        return Err(format!(
            "The settings file {} cannot be written: {e}",
            file.display()
        ));
    }
    Ok(())
}

/// Whether a run's record could be written in `folder`: it is a whole path, it exists or can
/// be made, and a file can be made in it. The error says why not, naming the folder.
pub fn check_writable(folder: &Path) -> Result<(), String> {
    if !folder.is_absolute() {
        return Err(format!(
            "{} is not a whole path; choose a folder by its full path.",
            folder.display()
        ));
    }
    if folder.exists() && !folder.is_dir() {
        return Err(format!("{} is a file, not a folder.", folder.display()));
    }
    fs::create_dir_all(folder).map_err(|e| format!("{} cannot be made: {e}", folder.display()))?;
    let probe = folder.join(format!(".hertzforge-write-check-{}", std::process::id()));
    let made = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .and_then(|mut f| f.write_all(b"check").and_then(|_| f.sync_all()));
    let _ = fs::remove_file(&probe);
    made.map_err(|e| format!("{} cannot be written: {e}", folder.display()))
}

/// Whether the data folder may be changed now: never while a run is live, nor when the
/// control service cannot be asked; then the folder (none: the default) must be writable.
pub fn may_change(
    live: Result<Option<String>, String>,
    folder: Option<&Path>,
) -> Result<(), String> {
    match live {
        Ok(Some(what)) => {
            return Err(format!(
                "{what} The data folder is not changed while a run is live; change it once the run has ended."
            ))
        }
        Ok(None) => {}
        Err(e) => {
            return Err(format!(
                "The control service could not be asked whether a run is live ({e}), so the data folder is not changed."
            ))
        }
    }
    match folder {
        Some(f) => check_writable(f),
        None => Ok(()),
    }
}

/// The data folder of this launch, and where the app keeps its settings.
pub struct DataFolder {
    config_dir: Option<PathBuf>,
    /// The folder the services were started with, and where it came from.
    running: Mutex<Option<(PathBuf, Source)>>,
}

impl DataFolder {
    pub fn new(config_dir: Option<PathBuf>) -> DataFolder {
        DataFolder {
            config_dir,
            running: Mutex::new(None),
        }
    }

    /// The folder to start the services with: chosen, named, or the default.
    pub fn for_start(&self) -> (PathBuf, Source) {
        let chosen = self.config_dir.as_deref().and_then(read_chosen);
        let resolved = resolve_here(chosen.as_deref());
        if let Ok(mut r) = self.running.lock() {
            *r = Some(resolved.clone());
        }
        resolved
    }

    /// What the shell shows: the folder in use, where it came from, and the folder the next
    /// start will use when that differs.
    pub fn describe(&self) -> Value {
        let chosen = self.config_dir.as_deref().and_then(read_chosen);
        let next = resolve_here(chosen.as_deref());
        let running = self.running.lock().ok().and_then(|r| r.clone());
        let (path, source) = running.clone().unwrap_or_else(|| next.clone());
        let mut out = json!({
            "path": path.to_string_lossy(),
            "source": source.word(),
            "exists": path.is_dir(),
            "settable": self.config_dir.is_some(),
            "chosen": chosen.as_ref().map(|c| c.to_string_lossy().into_owned()),
            "default": resolve_here(None).0.to_string_lossy(),
        });
        if next.0 != path {
            out["next"] = Value::String(next.0.to_string_lossy().into_owned());
        }
        if path.is_dir() {
            match check_writable(&path) {
                Ok(()) => out["writable"] = Value::Bool(true),
                Err(why) => {
                    out["writable"] = Value::Bool(false);
                    out["reason"] = Value::String(why);
                }
            }
        }
        out
    }

    fn choose(&self, folder: Option<&Path>) -> Result<(), String> {
        let dir = self.config_dir.as_deref().ok_or(
            "The app's settings folder could not be found, so the data folder cannot be kept.",
        )?;
        write_chosen(dir, folder)
    }
}

/// `data_folder`: where runs are kept (`DataFolder::describe`).
#[tauri::command]
pub fn data_folder(state: tauri::State<'_, DataFolder>) -> Value {
    state.describe()
}

/// `set_data_folder`: keep `path` as the data folder from the next start (none: back to the
/// variable or the default). Refused while a run is live and when the folder cannot be written;
/// the runs already kept stay where they are.
#[tauri::command]
pub async fn set_data_folder(app: AppHandle, path: Option<String>) -> Result<Value, String> {
    let folder = path.filter(|p| !p.is_empty()).map(PathBuf::from);
    let live = match app.try_state::<crate::Running>() {
        Some(r) => crate::probe::live_any(r.services.control.port),
        None => Err("the control service has not started yet".into()),
    };
    may_change(live, folder.as_deref())?;
    let state = app.state::<DataFolder>();
    state.choose(folder.as_deref())?;
    Ok(state.describe())
}

/// `show_folder`: the folder shown in the file manager (on macOS and Windows selected in the
/// folder that holds it). Only a folder that exists is shown.
#[tauri::command]
pub fn show_folder(path: String) -> Result<(), String> {
    let folder = PathBuf::from(&path);
    if !folder.is_absolute() || !folder.is_dir() {
        return Err(format!("There is no folder at {path}."));
    }
    let mut cmd = show_command(&folder);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("The file manager could not be opened: {e}"))?;
    // reaped off the caller's thread, so nothing is left behind
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// The command that shows `folder` in this system's file manager.
pub fn show_command(folder: &Path) -> std::process::Command {
    if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(folder);
        c
    } else if cfg!(windows) {
        let mut select = OsString::from("/select,");
        select.push(folder.as_os_str());
        let mut c = std::process::Command::new("explorer");
        c.arg(select);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(folder);
        c
    }
}

/// The files the app was asked to open, waiting for the shell.
#[derive(Default)]
pub struct Opened(Mutex<Vec<String>>);

impl Opened {
    pub fn push(&self, paths: impl IntoIterator<Item = String>) {
        if let Ok(mut q) = self.0.lock() {
            q.extend(paths);
        }
    }

    fn take(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }
}

/// `opened_files`: the files the app was asked to open since the shell last asked, in order.
#[tauri::command]
pub fn opened_files(state: tauri::State<'_, Opened>) -> Vec<String> {
    state.take()
}

/// The files among the app's launch arguments: every argument that is not a flag, nor the
/// file `--run-log` takes, made whole from where the app was launched.
pub fn launch_paths(
    args: impl Iterator<Item = String>,
    whole: impl Fn(&str) -> String,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        if arg == "--run-log" {
            if args.peek().is_some_and(|a| !a.starts_with('-')) {
                args.next();
            }
            continue;
        }
        if !arg.starts_with('-') && !arg.is_empty() {
            out.push(whole(&arg));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hf-desktop-runfiles-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_data_folder_is_chosen_then_named_then_the_default() {
        let home = Some(PathBuf::from("/home/someone"));
        let chosen = PathBuf::from("/data/runs");
        assert_eq!(
            resolve(Some(&chosen), Some("/env/runs".into()), home.clone()),
            (chosen.clone(), Source::Setting)
        );
        assert_eq!(
            resolve(None, Some("/env/runs".into()), home.clone()),
            (PathBuf::from("/env/runs"), Source::Environment)
        );
        assert_eq!(
            resolve(None, Some("".into()), home.clone()),
            (
                PathBuf::from("/home/someone/Hertzforge_Data"),
                Source::Default
            )
        );
        assert_eq!(
            resolve(None, None, None),
            (PathBuf::from("Hertzforge_Data"), Source::Default)
        );
    }

    #[test]
    fn the_chosen_data_folder_is_kept_beside_the_other_settings() {
        let dir = scratch("settings");
        fs::write(dir.join(SETTINGS_FILE), r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(read_chosen(&dir), None);
        write_chosen(&dir, Some(Path::new("/data/runs"))).unwrap();
        assert_eq!(read_chosen(&dir), Some(PathBuf::from("/data/runs")));
        let kept: Value =
            serde_json::from_str(&fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap()).unwrap();
        assert_eq!(kept["theme"], "dark");
        write_chosen(&dir, None).unwrap();
        assert_eq!(read_chosen(&dir), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_data_folder_change_is_refused_while_a_run_is_live() {
        let dir = scratch("live");
        let live = Ok(Some(
            "The Random test \"Bench\" is running on the simulator.".into(),
        ));
        let said = may_change(live, Some(&dir)).unwrap_err();
        assert!(
            said.contains("\"Bench\"") && said.contains("while a run is live"),
            "{said}"
        );
        let unknown = may_change(Err("refused".into()), Some(&dir)).unwrap_err();
        assert!(unknown.contains("could not be asked"), "{unknown}");
        may_change(Ok(None), Some(&dir)).unwrap();
        may_change(Ok(None), None).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_data_folder_that_cannot_be_written_is_refused_with_the_reason() {
        let dir = scratch("unwritable");
        let file = dir.join("a file");
        fs::write(&file, "x").unwrap();
        let said = check_writable(&file).unwrap_err();
        assert!(said.contains("is a file, not a folder"), "{said}");
        let below = check_writable(&file.join("runs")).unwrap_err();
        assert!(below.contains("cannot be made"), "{below}");
        let relative = check_writable(Path::new("runs")).unwrap_err();
        assert!(relative.contains("not a whole path"), "{relative}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let locked = dir.join("locked");
            fs::create_dir(&locked).unwrap();
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
            // a user who may write anywhere (root) writes here too
            if fs::write(locked.join("probe"), "x").is_err() {
                let said = check_writable(&locked).unwrap_err();
                assert!(said.contains("cannot be written"), "{said}");
            }
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let fresh = dir.join("new").join("runs");
        check_writable(&fresh).unwrap();
        assert!(fresh.is_dir());
        assert_eq!(
            fs::read_dir(&fresh).unwrap().count(),
            0,
            "the check leaves nothing"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_new_data_folder_takes_effect_at_the_next_start_and_moves_no_run() {
        let dir = scratch("next");
        let old = dir.join("old");
        fs::create_dir_all(old.join("20261114-120000_random_1")).unwrap();
        let folder = DataFolder::new(Some(dir.join("config")));
        let (started, _) = folder.for_start();
        write_chosen(&dir.join("config"), Some(&dir.join("new"))).unwrap();
        let said = folder.describe();
        assert_eq!(said["path"], started.to_string_lossy().as_ref());
        assert_eq!(said["next"], dir.join("new").to_string_lossy().as_ref());
        assert!(old.join("20261114-120000_random_1").is_dir());
        assert_eq!(folder.for_start().0, dir.join("new"));
        assert!(folder.describe().get("next").is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn launch_arguments_name_the_files_to_open() {
        let args =
            |a: &[&str]| launch_paths(a.iter().map(|s| s.to_string()), |p| format!("/at/{p}"));
        assert_eq!(args(&[]), Vec::<String>::new());
        assert_eq!(
            args(&["run/run.json", "plate.hforge"]),
            vec!["/at/run/run.json", "/at/plate.hforge"]
        );
        assert_eq!(
            args(&["--run-log", "a.hflog", "x.hforge"]),
            vec!["/at/x.hforge"]
        );
        assert_eq!(
            args(&["--run-log", "-psn_0_1", "x.hforge"]),
            vec!["/at/x.hforge"]
        );
    }

    #[test]
    fn a_folder_is_shown_in_this_systems_file_manager() {
        let c = show_command(Path::new("/data/runs/one"));
        let program = c.get_program().to_string_lossy().into_owned();
        let args: Vec<String> = c
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        if cfg!(target_os = "macos") {
            assert_eq!(
                (program.as_str(), args),
                ("open", vec!["-R".into(), "/data/runs/one".into()])
            );
        } else if cfg!(windows) {
            assert_eq!(program, "explorer");
        } else {
            assert_eq!(program, "xdg-open");
        }
        assert!(show_folder("/no/such/folder/here".into()).is_err());
        assert!(show_folder("relative".into()).is_err());
    }
}
