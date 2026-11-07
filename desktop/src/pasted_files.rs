//! The files on the macOS clipboard, by path: a webview's paste gives a copied file's contents
//! at most, and files pasted from Finder are imported the way they are imported when dropped.

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL};
use objc2_foundation::NSURL;

/// The paths of the files on the clipboard, in order; empty when it holds none.
#[tauri::command]
pub fn pasted_files() -> Vec<String> {
    let board = NSPasteboard::generalPasteboard();
    let Some(items) = board.pasteboardItems() else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for item in items.iter() {
        let Some(text) = (unsafe { item.stringForType(NSPasteboardTypeFileURL) }) else {
            continue;
        };
        let Some(url) = NSURL::URLWithString(&text) else {
            continue;
        };
        if let Some(path) = url.path() {
            paths.push(path.to_string());
        }
    }
    paths
}
