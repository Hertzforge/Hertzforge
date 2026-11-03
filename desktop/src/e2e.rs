//! The local proof of the built app, compiled only with the `e2e` feature and never
//! into a shipped build. With `HERTZFORGE_E2E_SCRIPT` naming a script, the script
//! runs in the main window once the shell has loaded and drives the shell through
//! its test ids, as an operator would (packaging/e2e/random_run.js). It reports
//! through `e2e_log` and quits through `e2e_quit`, which takes the same path as
//! closing the window. `HERTZFORGE_E2E_ANSWER` (`stop` or `keep`) answers the
//! question asked when quitting during a run, since nothing can click it; the
//! question is logged word for word.

use crate::{log, Quit};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, WebviewWindowBuilder, Wry};

static RAN: AtomicBool = AtomicBool::new(false);

pub fn attach<'a>(
    app: &AppHandle,
    builder: WebviewWindowBuilder<'a, Wry, AppHandle>,
) -> WebviewWindowBuilder<'a, Wry, AppHandle> {
    let Some(path) = std::env::var_os("HERTZFORGE_E2E_SCRIPT") else {
        return builder;
    };
    let script = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            log(app).say(&format!("e2e: cannot read the script: {e}"));
            return builder;
        }
    };
    builder.on_page_load(move |window, payload| {
        if payload.event() == PageLoadEvent::Finished && !RAN.swap(true, Ordering::SeqCst) {
            let _ = window.eval(&script);
        }
    })
}

/// The answer the environment gives to the quit question, if any.
pub fn answer(app: &AppHandle, question: &str) -> Option<bool> {
    let a = std::env::var("HERTZFORGE_E2E_ANSWER").ok()?;
    log(app).say(&format!("e2e: asked {question:?}; answering {a}"));
    Some(a == "stop")
}

#[tauri::command]
pub fn e2e_log(app: AppHandle, line: String) {
    log(&app).say(&format!("e2e: {line}"));
}

#[tauri::command]
pub fn e2e_quit(app: AppHandle) {
    log(&app).say("e2e: quit, as closing the window does");
    Quit::request(&app);
}
