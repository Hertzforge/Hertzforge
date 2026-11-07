//! The macOS save window with a list of file types under the name. The dialog plugin's save window on macOS merges every type it is given and shows
//! no list, and on any system it does not say which type was chosen, which matters when
//! two formats share a suffix (a sdynpy data array and a Rattlesnake specification are
//! both `.npz`).

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSModalResponseOK, NSPopUpButton, NSSavePanel, NSTextField, NSView};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString};
use serde::{Deserialize, Serialize};
use tauri::WebviewWindow;

#[derive(Deserialize)]
pub struct FileType {
    value: String,
    label: String,
    /// With its dot; empty for a folder.
    suffix: String,
}

#[derive(Serialize)]
pub struct Chosen {
    path: String,
    format: String,
}

/// `name` with the suffix of `suffix`, in place of any of `known` it ends with.
fn renamed(name: &str, known: &[String], suffix: &str) -> String {
    let lower = name.to_lowercase();
    let stem = known
        .iter()
        .filter(|s| !s.is_empty() && lower.ends_with(&s.to_lowercase()))
        .map(|s| &name[..name.len() - s.len()])
        .next()
        .unwrap_or(name);
    format!("{stem}{suffix}")
}

fn allow(panel: &NSSavePanel, suffix: &str) {
    let types = (!suffix.is_empty()).then(|| {
        NSArray::from_retained_slice(&[NSString::from_str(suffix.trim_start_matches('.'))])
    });
    #[allow(deprecated)]
    panel.setAllowedFileTypes(types.as_deref());
}

struct Ivars {
    panel: Retained<NSSavePanel>,
    suffixes: Vec<String>,
}

define_class!(
    // The list's target: choosing a type changes the name's suffix.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct FileTypeList;

    impl FileTypeList {
        #[unsafe(method(typeChanged:))]
        fn type_changed(&self, sender: &NSPopUpButton) {
            let Ivars { panel, suffixes } = self.ivars();
            let suffix = &suffixes[sender.indexOfSelectedItem() as usize];
            let name = panel.nameFieldStringValue().to_string();
            allow(panel, suffix);
            panel.setNameFieldStringValue(&NSString::from_str(&renamed(&name, suffixes, suffix)));
        }
    }
);

fn run(mtm: MainThreadMarker, title: &str, name: &str, types: &[FileType]) -> Option<Chosen> {
    let panel = NSSavePanel::savePanel(mtm);
    panel.setTitle(Some(&NSString::from_str(title)));
    panel.setCanCreateDirectories(true);
    panel.setExtensionHidden(false);
    allow(&panel, &types[0].suffix);
    panel.setNameFieldStringValue(&NSString::from_str(&format!("{name}{}", types[0].suffix)));

    let frame = |x, width| NSRect::new(NSPoint::new(x, 8.0), NSSize::new(width, 26.0));
    let list = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        frame(90.0, 370.0),
        false,
    );
    for file_type in types {
        list.addItemWithTitle(&NSString::from_str(&file_type.label));
    }
    let target = FileTypeList::alloc(mtm).set_ivars(Ivars {
        panel: panel.clone(),
        suffixes: types.iter().map(|t| t.suffix.clone()).collect(),
    });
    let target: Retained<FileTypeList> = unsafe { msg_send![super(target), init] };
    unsafe {
        list.setTarget(Some(&target));
        list.setAction(Some(sel!(typeChanged:)));
    }
    let label = NSTextField::labelWithString(&NSString::from_str("File type:"), mtm);
    label.setFrame(NSRect::new(
        NSPoint::new(16.0, 12.0),
        NSSize::new(70.0, 18.0),
    ));
    let accessory = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(476.0, 42.0)),
    );
    accessory.addSubview(&label);
    accessory.addSubview(&list);
    panel.setAccessoryView(Some(&accessory));

    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    let path = panel.URL()?.path()?.to_string();
    let format = types[list.indexOfSelectedItem() as usize].value.clone();
    Some(Chosen { path, format })
}

/// Where to write an object and in which format, or null when cancelled. Only the shell's
/// window asks: the display windows get nothing (capabilities/main.json).
#[tauri::command]
pub async fn save_with_file_types(
    window: WebviewWindow,
    title: String,
    name: String,
    types: Vec<FileType>,
) -> Result<Option<Chosen>, String> {
    if window.label() != crate::MAIN {
        return Err("only the shell's window can ask for the save window".into());
    }
    if types.is_empty() {
        return Err("there is no file type to save as".into());
    }
    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    window
        .run_on_main_thread(move || {
            let mtm = MainThreadMarker::new().expect("run on the main thread");
            // nobody to tell when the shell has gone
            let _ = sender.try_send(run(mtm, &title, &name, &types));
        })
        .map_err(|error| error.to_string())?;
    receiver
        .recv()
        .await
        .ok_or_else(|| "the save window closed without an answer".to_string())
}

#[cfg(test)]
mod tests {
    use super::renamed;

    #[test]
    fn a_new_type_replaces_the_suffix_and_keeps_the_rest_of_the_name() {
        let known = [".hforge".to_string(), ".unv".to_string(), String::new()];
        assert_eq!(renamed("Plate v2.hforge", &known, ".unv"), "Plate v2.unv");
        assert_eq!(renamed("Plate.UNV", &known, ".hforge"), "Plate.hforge");
        assert_eq!(renamed("Plate v2", &known, ".unv"), "Plate v2.unv");
        assert_eq!(renamed("Plate.hforge", &known, ""), "Plate");
    }
}
