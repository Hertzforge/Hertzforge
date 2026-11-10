//! The Hertzforge desktop app: the shell in a window of its own, with the control
//! and analysis services started beside it on loopback (`services.rs`).
//!
//! The window opens once both services listen, at the shell's own address with the
//! services' addresses in its query (`?analysis=ws://...&control=ws://...`), which is
//! how the shell already reads them in a browser. A display page opened in a window
//! of its own inherits the query.
//!
//! Quitting (closing the main window, or Quit in the menu) never kills the control
//! service. If a run is live, the operator is asked first; then the app closes the
//! services' inputs, the control service ends the run with its controlled stop and
//! exits, and only then does the app exit. If the app dies instead, the operating
//! system closes those inputs and the control service does the same on its own.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
mod pasted_files;
mod probe;
mod save_file;
#[cfg(target_os = "macos")]
mod save_panel;
mod services;

#[cfg(feature = "e2e")]
mod e2e;

use services::{AppLog, Layout, Services};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

/// The main window's label; closing it quits.
const MAIN: &str = "main";
/// The menu item that quits through [`Quit::request`].
#[cfg(target_os = "macos")]
const QUIT_ID: &str = "quit";
/// The app menu's Check for Updates: the shell asks the updater, as its palette does.
#[cfg(target_os = "macos")]
const UPDATE_ID: &str = "check-for-updates";

/// The app's state once the services have started.
struct Running {
    services: Arc<Services>,
}

/// Where the quit stands.
#[derive(Default)]
struct Quit {
    /// A quit is under way (asking, or stopping the services).
    busy: AtomicBool,
    /// The services have stopped; the app may exit.
    done: AtomicBool,
}

/// Display windows opened from the shell, for unique labels.
static DISPLAYS: AtomicU64 = AtomicU64::new(0);

fn log(app: &AppHandle) -> tauri::State<'_, AppLog> {
    app.state::<AppLog>()
}

impl Quit {
    /// Quit as the operator asked: if a run is live, ask first; then stop the
    /// services (the control service ends its run with its controlled stop) and
    /// exit once they have.
    fn request(app: &AppHandle) {
        let quit = app.state::<Quit>();
        if quit.done.load(Ordering::SeqCst) {
            app.exit(0);
            return;
        }
        if quit.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = app.clone();
        std::thread::spawn(move || {
            let services = app.try_state::<Running>().map(|r| r.services.clone());
            if let Some(services) = &services {
                match probe::live_run(services.control.port) {
                    Ok(Some(what)) => {
                        log(&app).say(&format!("quit asked while live: {what}"));
                        if !confirm_stop(&app, &what) {
                            log(&app).say("the operator kept the run going");
                            app.state::<Quit>().busy.store(false, Ordering::SeqCst);
                            return;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => log(&app).say(&format!("could not ask about a live run: {e}")),
                }
                log(&app).say("stopping the services");
                services.stop(|line| log(&app).say(line));
            }
            app.state::<Quit>().done.store(true, Ordering::SeqCst);
            log(&app).say("exiting");
            app.exit(0);
        });
    }
}

/// Ask whether to stop the live run and quit. True: stop it.
fn confirm_stop(app: &AppHandle, what: &str) -> bool {
    let question = format!(
        "{what}\n\nQuitting stops it with its shutdown ramp, the same controlled stop as \
         the Stop button, and Hertzforge closes once the run has ended."
    );
    #[cfg(feature = "e2e")]
    if let Some(answer) = e2e::answer(app, &question) {
        return answer;
    }
    let mut dialog = app
        .dialog()
        .message(question)
        .title("Stop the Test and Quit?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Stop the Test and Quit".into(),
            "Keep Running".into(),
        ));
    if let Some(w) = app.get_webview_window(MAIN) {
        dialog = dialog.parent(&w);
    }
    dialog.blocking_show()
}

fn fail(app: &AppHandle, message: &str) {
    log(app).say(&format!("cannot start: {message}"));
    app.dialog()
        .message(message)
        .title("Hertzforge Could Not Start")
        .kind(MessageDialogKind::Error)
        .blocking_show();
    app.state::<Quit>().done.store(true, Ordering::SeqCst);
    app.exit(1);
}

/// Start the services, then open the main window on them.
fn startup(app: AppHandle) {
    let paths = app.path();
    let layout = match (
        std::env::current_exe(),
        paths.resource_dir(),
        paths.app_log_dir(),
    ) {
        (Ok(exe), Ok(res), Ok(logs)) => Layout::development(logs.clone())
            .unwrap_or_else(|| Layout::installed(exe.parent().unwrap_or(&exe), &res, logs)),
        _ => return fail(&app, "The app's own folders could not be found."),
    };
    let services = match Services::start(&layout) {
        Ok(s) => Arc::new(s),
        Err(e) => return fail(&app, &e),
    };
    log(&app).open(&services.log_dir);
    log(&app).say(&format!(
        "control service on {}, analysis service on {}, logs in {}",
        services.control.url(),
        services.analysis.url(),
        services.log_dir.display()
    ));
    app.manage(Running {
        services: services.clone(),
    });
    // Quit chosen while the services were starting: stop them now.
    if app.state::<Quit>().busy.load(Ordering::SeqCst) {
        services.stop(|line| log(&app).say(line));
        app.state::<Quit>().done.store(true, Ordering::SeqCst);
        app.exit(0);
        return;
    }
    let query = format!(
        "?analysis={}&control={}{}",
        encode(&services.analysis.url()),
        encode(&services.control.url()),
        run_log_query(std::env::args().skip(1))
    );
    if let Err(e) = main_window(&app, &query) {
        fail(&app, &format!("The window could not be opened: {e}"));
    }
}

fn main_window(app: &AppHandle, query: &str) -> tauri::Result<()> {
    let opener = app.clone();
    let builder = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App(query.into()))
        .title("Hertzforge")
        .inner_size(1440.0, 900.0)
        .min_inner_size(960.0, 600.0)
        // A display page opened from the shell, for another monitor: a window of
        // the app's own, in the same webview context so the page can be its opener.
        .on_new_window(move |_url, features| {
            let n = DISPLAYS.fetch_add(1, Ordering::Relaxed) + 1;
            let built = WebviewWindowBuilder::new(
                &opener,
                format!("display-{n}"),
                WebviewUrl::External("about:blank".parse().expect("a valid address")),
            )
            .window_features(features)
            .title("Hertzforge")
            .on_document_title_changed(|w, title| {
                let _ = w.set_title(&title);
            })
            .build();
            match built {
                Ok(window) => NewWindowResponse::Create { window },
                Err(_) => NewWindowResponse::Deny,
            }
        });
    #[cfg(feature = "e2e")]
    let builder = e2e::attach(app, builder);
    builder.build()?;
    Ok(())
}

/// `--run-log [file]`: the window with the log alone in it, for the keeper's own computer,
/// Its own flag, not a file to import.
fn run_log_query(args: impl Iterator<Item = String>) -> String {
    let args: Vec<String> = args.collect();
    let Some(at) = args.iter().position(|a| a == "--run-log") else {
        return String::new();
    };
    match args.get(at + 1).filter(|a| !a.starts_with('-')) {
        Some(file) => format!("&runlog=1&runlog_file={}", encode(&from_launch(file))),
        None => "&runlog=1".to_string(),
    }
}

/// `file` whole, from where the app was launched: the service runs in the log directory, so a
/// relative path means nothing there. A leading `~` is left for the service to expand.
fn from_launch(file: &str) -> String {
    if file.starts_with('~') {
        return file.to_string();
    }
    std::path::absolute(file)
        .map(|whole| whole.to_string_lossy().into_owned())
        .unwrap_or_else(|_| file.to_string())
}

/// Percent-encode a query value (the services' addresses).
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem as P, Submenu};
    // The standard menus, with Quit replaced: the predefined Quit ends the process
    // at once, past the question and the controlled stop.
    let quit = MenuItem::with_id(app, QUIT_ID, "Quit Hertzforge", true, Some("CmdOrCtrl+Q"))?;
    let update = MenuItem::with_id(app, UPDATE_ID, "Check for Updates...", true, None::<&str>)?;
    let app_menu = Submenu::with_items(
        app,
        "Hertzforge",
        true,
        &[
            &P::about(app, None, None)?,
            &update,
            &P::separator(app)?,
            &P::services(app, None)?,
            &P::separator(app)?,
            &P::hide(app, None)?,
            &P::hide_others(app, None)?,
            &P::show_all(app, None)?,
            &P::separator(app)?,
            &quit,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &P::undo(app, None)?,
            &P::redo(app, None)?,
            &P::separator(app)?,
            &P::cut(app, None)?,
            &P::copy(app, None)?,
            &P::paste(app, None)?,
            &P::select_all(app, None)?,
        ],
    )?;
    // The project's files: the shell answers, since the dialogs and the service are its.
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &MenuItem::with_id(app, "file-new", "New Project", true, Some("CmdOrCtrl+N"))?,
            &MenuItem::with_id(app, "file-open", "Open...", true, Some("CmdOrCtrl+O"))?,
            &MenuItem::with_id(app, "file-import", "Import...", true, Some("CmdOrCtrl+I"))?,
            &P::separator(app)?,
            &MenuItem::with_id(app, "file-save", "Save", true, Some("CmdOrCtrl+S"))?,
            &MenuItem::with_id(
                app,
                "file-save-as",
                "Save As...",
                true,
                Some("CmdOrCtrl+Shift+S"),
            )?,
        ],
    )?;
    let view = Submenu::with_items(app, "View", true, &[&P::fullscreen(app, None)?])?;
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &P::minimize(app, None)?,
            &P::maximize(app, None)?,
            &P::separator(app)?,
            &P::close_window(app, None)?,
        ],
    )?;
    Menu::with_items(app, &[&app_menu, &file, &edit, &view, &window])
}

fn main() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Check for Updates, from the app menu or the shell's palette: the updater reads the latest
        // release's manifest, verifies the archive against the public key in
        // tauri.conf.json, installs it, and the process plugin relaunches.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppLog::new())
        .manage(Quit::default())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_menu(menu(app.handle())?)?;
            let handle = app.handle().clone();
            std::thread::spawn(move || startup(handle));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == MAIN {
                    api.prevent_close();
                    Quit::request(window.app_handle());
                }
            }
        });
    #[cfg(target_os = "macos")]
    let builder = builder.on_menu_event(|app, event| {
        if event.id() == QUIT_ID {
            Quit::request(app);
        } else if event.id() == UPDATE_ID {
            let _ = tauri::Emitter::emit(app, "update-menu", ());
        } else if let Some(act) = event.id().as_ref().strip_prefix("file-") {
            // nothing to tell when the window is gone
            let _ = tauri::Emitter::emit(app, "file-menu", act);
        }
    });
    #[cfg(all(target_os = "macos", not(feature = "e2e")))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        pasted_files::pasted_files,
        save_panel::save_with_file_types,
        save_file::save_file
    ]);
    #[cfg(all(target_os = "macos", feature = "e2e"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        e2e::e2e_log,
        e2e::e2e_quit,
        pasted_files::pasted_files,
        save_panel::save_with_file_types,
        save_file::save_file
    ]);
    #[cfg(all(not(target_os = "macos"), feature = "e2e"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        e2e::e2e_log,
        e2e::e2e_quit,
        save_file::save_file
    ]);
    #[cfg(all(not(target_os = "macos"), not(feature = "e2e")))]
    let builder = builder.invoke_handler(tauri::generate_handler![save_file::save_file]);
    let app = builder
        .build(tauri::generate_context!())
        .expect("the app could not be built");
    app.run(|app, event| match event {
        // Every way out that the app hears of goes through the quit.
        RunEvent::ExitRequested { api, .. } if !app.state::<Quit>().done.load(Ordering::SeqCst) => {
            api.prevent_exit();
            Quit::request(app);
        }
        // Ended without the quit (the operating system ending the app): close the
        // services' inputs so each stops by itself, the control service with its
        // controlled stop.
        RunEvent::Exit => {
            if let Some(r) = app.try_state::<Running>() {
                r.services.control.close_input();
                r.services.analysis.close_input();
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn service_addresses_travel_whole_in_the_query() {
        assert_eq!(
            encode("ws://127.0.0.1:61234/"),
            "ws%3A%2F%2F127.0.0.1%3A61234%2F"
        );
    }
}

#[cfg(test)]
mod run_log_tests {
    use super::run_log_query;

    fn query(args: &[&str]) -> String {
        run_log_query(args.iter().map(|a| a.to_string()))
    }

    #[test]
    fn the_flag_asks_for_the_log_alone_and_takes_a_file() {
        assert_eq!(query(&[]), "");
        assert_eq!(query(&["--run-log"]), "&runlog=1");
        assert_eq!(query(&["--run-log", "--other"]), "&runlog=1");
        assert_eq!(
            query(&["--run-log", "/tmp/a b.hflog"]),
            "&runlog=1&runlog_file=%2Ftmp%2Fa%20b.hflog"
        );
    }

    #[test]
    fn a_relative_file_is_whole_from_where_the_app_was_launched() {
        let here = std::env::current_dir().unwrap().join("a.hflog");
        assert_eq!(
            query(&["--run-log", "a.hflog"]),
            format!("&runlog=1&runlog_file={}", super::encode(&here.to_string_lossy()))
        );
        assert_eq!(query(&["--run-log", "~/a.hflog"]), "&runlog=1&runlog_file=~%2Fa.hflog");
    }
}
