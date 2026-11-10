//! Saving a file the shell made itself (a plot's image, its lines as CSV) where the person
//! chooses: the app shows its save window and writes the bytes to the path chosen there. The
//! shell never names the path, so nothing is written anywhere the person did not pick.

use tauri::WebviewWindow;
use tauri_plugin_dialog::DialogExt;

/// Ask where to save `name` and write `contents` there. The answer is the path written, or none
/// when the window was cancelled.
#[tauri::command]
pub async fn save_file(
    window: WebviewWindow,
    title: String,
    name: String,
    filter_name: String,
    extensions: Vec<String>,
    contents: Vec<u8>,
) -> Result<Option<String>, String> {
    let extensions: Vec<&str> = extensions.iter().map(String::as_str).collect();
    let chosen = window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(title)
        .set_file_name(name)
        .add_filter(filter_name, &extensions)
        .blocking_save_file();
    let Some(chosen) = chosen else {
        return Ok(None);
    };
    let path = chosen.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, contents).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(path.display().to_string()))
}
