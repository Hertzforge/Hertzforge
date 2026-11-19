//! `print_page`: a report's printable page opened in the system's browser, which prints it.
//!
//! The window's web view does not print a page that asks to be printed (WebKit leaves
//! `window.print()` to the app embedding it), so Print or Save as PDF did nothing in the
//! desktop app. The page is written to a file of its own and opened where printing works; the
//! page asks for the print dialog itself once its figures have drawn, and that dialog saves a PDF.

use std::path::{Path, PathBuf};

/// Write `html` as `<name>.html` in the system's temporary folder and open it in the default
/// browser. Returns the file's path.
#[tauri::command]
pub fn print_page(html: String, name: String) -> Result<String, String> {
    let folder = std::env::temp_dir().join("Hertzforge print");
    std::fs::create_dir_all(&folder)
        .map_err(|e| format!("The page to print could not be written: {e}"))?;
    let path = page_path(&folder, &name);
    std::fs::write(&path, html)
        .map_err(|e| format!("The page to print could not be written: {e}"))?;
    let mut child = open_command(&path)
        .spawn()
        .map_err(|e| format!("The browser could not be opened to print the page: {e}"))?;
    // reaped off the caller's thread, so nothing is left behind
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(path.to_string_lossy().into_owned())
}

/// The page's file in `folder`: the report's name with what a file name cannot hold replaced.
pub fn page_path(folder: &Path, name: &str) -> PathBuf {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_.()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = safe.trim().trim_matches('.');
    folder.join(format!(
        "{}.html",
        if stem.is_empty() { "Report" } else { stem }
    ))
}

/// The command that opens `file` in this system's default browser.
pub fn open_command(file: &Path) -> std::process::Command {
    if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(file);
        c
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]).arg(file);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(file);
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_name_becomes_a_file_name() {
        let folder = Path::new("/tmp/x");
        assert_eq!(
            page_path(folder, "Random Report"),
            folder.join("Random Report.html")
        );
        assert_eq!(page_path(folder, "a/b:c"), folder.join("a_b_c.html"));
        assert_eq!(page_path(folder, ".."), folder.join("Report.html"));
        assert_eq!(page_path(folder, ""), folder.join("Report.html"));
    }

    #[test]
    fn the_file_is_opened_by_the_system() {
        let cmd = open_command(Path::new("/tmp/x/Report.html"));
        let program = cmd.get_program().to_string_lossy().into_owned();
        assert!(
            ["open", "cmd", "xdg-open"].contains(&program.as_str()),
            "{program}"
        );
    }
}
