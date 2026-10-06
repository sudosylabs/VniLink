//! Bounded file transfers and a persistent receive directory.
use super::{preferences, tasks};
use adw::prelude::*;
use std::sync::Arc;

pub(crate) fn send(
    window: &adw::ApplicationWindow,
    core: Arc<vnilink::Core>,
    id: u64,
    generation: u64,
) {
    send_selected(window, core, id, generation, false);
}

fn send_selected(
    window: &impl IsA<gtk::Window>,
    core: Arc<vnilink::Core>,
    id: u64,
    generation: u64,
    hosted: bool,
) {
    let window: &gtk::Window = window.as_ref();
    let dialog = gtk::FileDialog::builder().title("Send a file").build();
    let win = window.clone();
    dialog.open(
        Some(window),
        None::<&gtk::gio::Cancellable>,
        move |result| {
            let Ok(file) = result else {
                return;
            };
            let Some(path) = file.path() else {
                preferences::report(&win, "Choose a local file.");
                return;
            };
            send_path(&win, core, id, generation, hosted, path);
        },
    );
}

pub(crate) fn page(window: &adw::Window, core: Arc<vnilink::Core>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Files")
        .name("files")
        .icon_name("folder-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder().title("Receive files")
        .description("Received files are saved here automatically when file-transfer permission is enabled. Files are limited to 8 MiB; existing files are kept.").build();
    let row = adw::ActionRow::builder()
        .title("Receive folder")
        .subtitle(receive_folder().display().to_string())
        .subtitle_lines(2)
        .build();
    let open = gtk::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("Open receive folder")
        .valign(gtk::Align::Center)
        .build();
    let win = window.clone();
    open.connect_clicked(move |_| {
        let uri = gtk::gio::File::for_path(receive_folder()).uri();
        if gtk::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk::gio::AppLaunchContext>)
            .is_err()
        {
            preferences::report(&win, "The receive folder could not be opened.");
        }
    });
    open.set_margin_end(6);
    row.add_suffix(&open);
    let choose = gtk::Button::with_label("Choose folder…");
    choose.set_valign(gtk::Align::Center);
    row.add_suffix(&choose);
    group.add(&row);
    page.add(&group);
    let win = window.clone();
    let folder_core = core.clone();
    choose.connect_clicked(move |_| {
        let dialog = gtk::FileDialog::builder()
            .title("Choose a receive folder")
            .build();
        let win = win.clone();
        let worker = folder_core.clone();
        let row = row.clone();
        dialog.select_folder(
            Some(&win.clone()),
            None::<&gtk::gio::Cancellable>,
            move |r| {
                let Ok(folder) = r else {
                    return;
                };
                let Some(path) = folder.path() else {
                    preferences::report(&win, "Choose a local folder.");
                    return;
                };
                let display = path.display().to_string();
                tasks::run(
                    move || {
                        worker
                            .set_transfer_directory(&path)
                            .map_err(|e| e.to_string())?;
                        save_folder(&path)
                    },
                    move |r| match r {
                        Ok(()) => row.set_subtitle(&display),
                        Err(e) => preferences::report(&win, &e),
                    },
                );
            },
        );
    });
    let share = adw::PreferencesGroup::builder().title("Share with the connected computer").description("Send a file to the computer controlling this desktop. Clipboard text syncs automatically; separate clipboard and file permissions are required.").build();
    let send = gtk::Button::with_label("Send file to controller…");
    send.set_halign(gtk::Align::Start);

    let worker = core.clone();
    let win = window.clone();
    send.connect_clicked(move |_| {
        let worker = worker.clone();
        let win = win.clone();
        tasks::run(
            move || {
                worker.snapshot().map_err(|e| e.to_string()).and_then(|s| {
                    s.incoming
                        .filter(|s| s.phase == vnilink::SessionPhase::Active)
                        .ok_or("No computer is connected to this desktop.".into())
                        .map(|session| (worker, session))
                })
            },
            move |r| match r {
                Ok((worker, s)) => send_selected(&win, worker, s.id, s.generation, true),
                Err(e) => preferences::report(&win, &e),
            },
        );
    });
    share.add(&preferences::action_group(&[&send]));
    page.add(&share);
    page
}

fn config_file() -> std::path::PathBuf {
    gtk::glib::user_config_dir().join("vnilink/receive-folder")
}
pub(super) fn receive_folder() -> std::path::PathBuf {
    read_folder(&config_file()).unwrap_or_else(|| {
        gtk::glib::user_special_dir(gtk::glib::UserDirectory::Downloads)
            .unwrap_or_else(gtk::glib::home_dir)
            .join("VniLink")
    })
}
fn read_folder(config: &std::path::Path) -> Option<std::path::PathBuf> {
    let uri = std::fs::read_to_string(config).ok()?;
    if !uri.starts_with("file://") {
        return None;
    }
    gtk::gio::File::for_uri(&uri)
        .path()
        .filter(|p| p.is_absolute())
}
fn save_folder(path: &std::path::Path) -> Result<(), String> {
    save_folder_to(&config_file(), path)
}
fn save_folder_to(config: &std::path::Path, path: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(config.parent().unwrap())
        .map_err(|_| "The receive folder preference could not be saved.")?;
    let temporary = config.with_extension("new");
    std::fs::write(&temporary, gtk::gio::File::for_path(path).uri().as_bytes())
        .and_then(|_| std::fs::rename(&temporary, config))
        .map_err(|_| "The receive folder preference could not be saved.".into())
}
pub(super) fn initialize(core: &vnilink::Core) -> Result<(), String> {
    core.set_transfer_directory(receive_folder())
        .map_err(|e| e.to_string())
}
fn read_file(path: &std::path::Path) -> Result<(String, Vec<u8>), String> {
    use std::io::Read;
    if !std::fs::metadata(path)
        .map_err(|_| "The file could not be read.")?
        .is_file()
    {
        return Err("Choose a regular file.".into());
    }
    let file = std::fs::File::open(path).map_err(|_| "The file could not be read.")?;
    if !file
        .metadata()
        .map_err(|_| "The file could not be read.")?
        .is_file()
    {
        return Err("Choose a regular file.".into());
    }
    let mut bytes = Vec::new();
    file.take(vnilink::MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The file could not be read.")?;
    if bytes.len() > vnilink::MAX_FILE_BYTES {
        return Err("Files must be 8 MiB or smaller.".into());
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("The file name is unsupported.")?;
    Ok((name.into(), bytes))
}
thread_local! {
    static SENDING: std::cell::RefCell<std::collections::BTreeSet<(u64,u64,bool)>> = const { std::cell::RefCell::new(std::collections::BTreeSet::new()) };
}
pub(super) fn feedback(window: &gtk::Window, text: &str) {
    fn overlay(widget: &gtk::Widget) -> Option<adw::ToastOverlay> {
        if let Ok(toasts) = widget.clone().downcast::<adw::ToastOverlay>() {
            return Some(toasts);
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            if let Some(t) = overlay(&c) {
                return Some(t);
            }
            child = c.next_sibling();
        }
        None
    }
    if let Some(toasts) = overlay(window.upcast_ref()) {
        toasts.add_toast(adw::Toast::new(text));
    } else {
        preferences::report(window, text);
    }
}
fn send_path(
    window: &gtk::Window,
    core: Arc<vnilink::Core>,
    id: u64,
    generation: u64,
    hosted: bool,
    path: std::path::PathBuf,
) {
    let key = (id, generation, hosted);
    if !SENDING.with(|s| s.borrow_mut().insert(key)) {
        feedback(window, "A file is already being sent.");
        return;
    }
    feedback(window, "Sending file…");
    let win = window.clone();
    tasks::run(
        move || {
            let (name, bytes) = read_file(&path)?;
            if hosted {
                if !core
                    .snapshot()
                    .map_err(|e| e.to_string())?
                    .incoming
                    .is_some_and(|s| {
                        s.id == id
                            && s.generation == generation
                            && s.phase == vnilink::SessionPhase::Active
                    })
                {
                    return Err("The hosted session ended.".into());
                }
                core.publish_file(id, &name, &bytes, false)
                    .map_err(|e| e.to_string())
            } else {
                core.submit_file(id, generation, &name, &bytes, false)
                    .map_err(|e| e.to_string())
            }
        },
        move |result| {
            SENDING.with(|s| s.borrow_mut().remove(&key));
            match result {
                Ok(()) => feedback(&win, "File saved on the remote computer."),
                Err(e) => preferences::report(&win, &e),
            }
        },
    );
}
pub(super) fn install_drop(
    picture: &gtk::Picture,
    window: &adw::ApplicationWindow,
    core: Arc<vnilink::Core>,
    id: u64,
    generation: u64,
) {
    let drop = gtk::DropTarget::new(
        gtk::gdk::FileList::static_type(),
        gtk::gdk::DragAction::COPY,
    );
    let win = window.clone();
    drop.connect_drop(move |_, value, _, _| {
        let Ok(files) = value.get::<gtk::gdk::FileList>() else {
            return false;
        };
        let files = files.files();
        if files.len() != 1 {
            preferences::report(&win, "Send one file at a time (up to 8 MiB).");
            return false;
        }
        let Some(path) = files[0].path() else {
            preferences::report(&win, "Choose a local file.");
            return false;
        };
        send_path(win.upcast_ref(), core.clone(), id, generation, false, path);
        true
    });
    picture.add_controller(drop);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receive_folder_survives_restart_and_reads_are_bounded() {
        let root = std::env::temp_dir().join(format!("vnilink-file-ui-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("config");
        let folder = root.join("my received files");
        save_folder_to(&config, &folder).unwrap();
        assert_eq!(read_folder(&config), Some(folder));
        let empty = root.join("empty");
        std::fs::write(&empty, []).unwrap();
        assert_eq!(read_file(&empty).unwrap(), ("empty".into(), vec![]));
        let large = root.join("large");
        let file = std::fs::File::create(&large).unwrap();
        file.set_len(vnilink::MAX_FILE_BYTES as u64 + 1).unwrap();
        assert_eq!(
            read_file(&large).unwrap_err(),
            "Files must be 8 MiB or smaller."
        );
        assert!(read_file(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
