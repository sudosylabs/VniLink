//! Pairing file dialogs. Owns picking, saving, and opening files.

use adw::prelude::*;
use std::cell::Cell;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::rc::Rc;

use vnilink::{Core, PairingPreview, PAIRING_FILE_EXTENSION};

use super::notifications;

/// Saves a fresh pairing file and shows its digits. Creating the file
/// turns hosting on for 15 minutes.
pub(crate) fn create_pairing_file(
    core: std::sync::Arc<Core>,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    showed: Rc<dyn Fn(String)>,
    repaint: Rc<dyn Fn()>,
    notice: Rc<dyn Fn(&vnilink::Error)>,
    saving: Rc<Cell<bool>>,
) {
    if saving.get() {
        return;
    }
    if let Err(err) = core.start_hosting() {
        notice(&err);
        return;
    }
    let file = match core.export_pairing_file(std::time::Duration::from_secs(15 * 60), true) {
        Ok(file) => file,
        Err(err) => {
            notice(&err);
            return;
        }
    };
    saving.set(true);
    repaint();
    let dialog = gtk::FileDialog::builder()
        .title("Save the pairing file")
        .initial_name("computer.vnl")
        .accept_label("Save")
        .build();
    let bytes = file.bytes;
    let check = file.check;
    gtk::glib::spawn_future_local(async move {
        let saved = dialog.save_future(Some(&window)).await;
        saving.set(false);
        match saved {
            Ok(file) => {
                let Some(path) = file.path() else {
                    notifications::note(&toasts, "Could not save the pairing file.");
                    return;
                };
                if write_private(&path, &bytes).is_err() {
                    notifications::note(&toasts, "Could not save the pairing file.");
                    return;
                }
                showed(check);
            }
            Err(error) if error.matches(gtk::DialogError::Dismissed) => {}
            Err(_) => notifications::note(&toasts, "Could not save the pairing file."),
        }
    });
}

/// Opens a `.vnl` file and shows its digits for confirmation.
pub(crate) fn open_pairing_file(
    core: std::sync::Arc<Core>,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    opened: Rc<dyn Fn(PairingPreview)>,
    notice: Rc<dyn Fn(&vnilink::Error)>,
    saving: Rc<Cell<bool>>,
) {
    if saving.get() {
        return;
    }
    saving.set(true);
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("VniLink pairing file"));
    filter.add_suffix(PAIRING_FILE_EXTENSION);
    let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title("Open a pairing file")
        .accept_label("Open")
        .default_filter(&filter)
        .filters(&filters)
        .build();
    gtk::glib::spawn_future_local(async move {
        let picked = dialog.open_future(Some(&window)).await;
        saving.set(false);
        let file = match picked {
            Ok(file) => file,
            Err(error) if error.matches(gtk::DialogError::Dismissed) => return,
            Err(_) => {
                notifications::note(&toasts, "Could not open that file.");
                return;
            }
        };
        let Some(path) = file.path() else {
            notifications::note(&toasts, "Could not open that file.");
            return;
        };
        let Some(bytes) = read_limited(&path) else {
            notifications::note(&toasts, "That file is not a pairing file.");
            return;
        };
        match core.open_pairing_file(&bytes) {
            Ok(preview) => opened(preview),
            Err(err) => notice(&err),
        }
    });
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

fn read_limited(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() == 0 || meta.len() > 20_000 {
        return None;
    }
    std::fs::read(path).ok()
}
