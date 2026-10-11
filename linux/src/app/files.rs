//! File sharing preferences and sending files to the connected computer.
//! Transfers move through the jobs engine under approved file roots;
//! received files land where the Files window directs them.
use super::{preferences, tasks};
use adw::prelude::*;
use std::sync::Arc;

fn send_to_controller(
    window: &impl IsA<gtk::Window>,
    core: Arc<vnilink::Core>,
    id: u64,
    generation: u64,
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
            send_path(&win, core, id, generation, path);
        },
    );
}

pub(crate) fn page(window: &adw::Window, core: Arc<vnilink::Core>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Files")
        .name("files")
        .icon_name("folder-symbolic")
        .build();
    let folders = adw::PreferencesGroup::builder().title("File manager")
        .description("The Files window browses these folders on this computer, and paired computers see only these folders. Narrow the list to share less.").build();
    let current = read_roots(&roots_file()).unwrap_or_else(default_roots);
    let subtitle = current
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    let roots_row = adw::ActionRow::builder()
        .title("Shared folders")
        .subtitle(&subtitle)
        .subtitle_lines(3)
        .build();
    let choose_roots = gtk::Button::with_label("Choose folders…");
    choose_roots.set_valign(gtk::Align::Center);
    roots_row.add_suffix(&choose_roots);
    folders.add(&roots_row);
    page.add(&folders);
    let roots_win = window.clone();
    let roots_core = core.clone();
    choose_roots.connect_clicked(move |_| {
        let dialog = gtk::FileDialog::builder()
            .title("Choose shared folders")
            .build();
        let win = roots_win.clone();
        let worker = roots_core.clone();
        let row = roots_row.clone();
        dialog.select_multiple_folders(
            Some(&win.clone()),
            None::<&gtk::gio::Cancellable>,
            move |r| {
                let Ok(model) = r else {
                    return;
                };
                let paths: Vec<std::path::PathBuf> = (0..model.n_items())
                    .filter_map(|index| model.item(index))
                    .filter_map(|item| item.downcast::<gtk::gio::File>().ok())
                    .filter_map(|folder| folder.path())
                    .collect();
                if paths.is_empty() {
                    preferences::report(&win, "Choose at least one folder.");
                    return;
                }
                let display = paths
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("\n");
                tasks::run(
                    move || {
                        worker
                            .set_file_roots(paths.clone())
                            .map_err(|e| e.to_string())?;
                        save_roots(&paths)
                    },
                    move |r| match r {
                        Ok(()) => row.set_subtitle(&display),
                        Err(e) => preferences::report(&win, &e),
                    },
                );
            },
        );
    });
    let share = adw::PreferencesGroup::builder().title("Share with the connected computer").description("Send a file to the computer controlling this desktop. The file must be inside this computer's shared folders; it is saved to the controller's first shared folder. Clipboard text syncs automatically; separate clipboard and file permissions are required.").build();
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
                Ok((worker, s)) => send_to_controller(&win, worker, s.id, s.generation),
                Err(e) => preferences::report(&win, &e),
            },
        );
    });
    share.add(&preferences::action_group(&[&send]));
    page.add(&share);
    page
}

pub(super) fn initialize(core: &vnilink::Core) -> Result<(), String> {
    core.set_blob_directory(blob_directory())
        .map_err(|e| e.to_string())
        .map_err(|e| format!("Blob storage is unavailable: {e}"))?;
    let roots = read_roots(&roots_file()).unwrap_or_else(default_roots);
    core.set_file_roots(roots).map_err(|e| e.to_string())?;
    Ok(())
}

fn blob_directory() -> std::path::PathBuf {
    gtk::glib::user_data_dir().join("vnilink/blobstore")
}

fn roots_file() -> std::path::PathBuf {
    gtk::glib::user_config_dir().join("vnilink/file-roots")
}

fn default_roots() -> Vec<std::path::PathBuf> {
    Vec::new()
}

fn read_roots(config: &std::path::Path) -> Option<Vec<std::path::PathBuf>> {
    let text = std::fs::read_to_string(config).ok()?;
    let roots: Vec<std::path::PathBuf> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect();
    // An existing (possibly empty) config is authoritative: an empty scope
    // stays empty and never falls back to the home folder. A missing file
    // also starts empty; the owner picks folders deliberately in Settings.
    Some(roots)
}

fn save_roots(paths: &[std::path::PathBuf]) -> Result<(), String> {
    let config = roots_file();
    std::fs::create_dir_all(config.parent().unwrap())
        .map_err(|_| "The shared folders preference could not be saved.")?;
    let temporary = config.with_extension("new");
    let text = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&temporary, text)
        .and_then(|_| std::fs::rename(&temporary, config))
        .map_err(|_| "The shared folders preference could not be saved.".into())
}
/// Resolve a picked path to a jobs upload: the approved source root, the
/// root-relative source path, and the file name the receiver saves.
fn upload_plan(
    path: &std::path::Path,
    roots: &[(u64, std::path::PathBuf)],
) -> Result<(u64, String, String), String> {
    let (root, mut parent, name) = super::filemanager::selected_upload(path, roots)?;
    parent.push(name.clone());
    Ok((root, parent.join("/"), name))
}

fn incoming_live(core: &vnilink::Core, id: u64, generation: u64) -> Option<String> {
    core.snapshot()
        .ok()?
        .incoming
        .filter(|session| {
            session.id == id
                && session.generation == generation
                && session.phase == vnilink::SessionPhase::Active
        })
        .map(|session| session.peer_id)
}

fn send_file(
    core: &vnilink::Core,
    id: u64,
    generation: u64,
    path: &std::path::Path,
) -> Result<(), String> {
    let peer = incoming_live(core, id, generation).ok_or("The hosted session ended.")?;
    let roots = core.file_roots().map_err(|e| e.to_string())?;
    let (src_root, src_rel, name) = upload_plan(path, &roots)?;
    let job = core
        .job_submit_upload(&peer, src_root, &src_rel, 0, &name)
        .map_err(|e| e.to_string())?;
    loop {
        if incoming_live(core, id, generation).is_none() {
            let _ = core.job_cancel(job);
            return Err("The hosted session ended.".into());
        }
        let snapshot = core.job_poll(job).map_err(|e| e.to_string())?;
        match snapshot.state {
            vnilink::JobState::Complete => return Ok(()),
            vnilink::JobState::Failed => {
                return Err(snapshot
                    .error
                    .unwrap_or_else(|| "The transfer failed.".into()));
            }
            vnilink::JobState::Cancelled => return Err("The transfer was cancelled.".into()),
            vnilink::JobState::WaitingDecision => {
                // A name conflict waits on the receiver's decision in its
                // Files window; keep polling rather than failing the send.
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}
thread_local! {
    static SENDING: std::cell::RefCell<std::collections::BTreeSet<(u64,u64)>> = const { std::cell::RefCell::new(std::collections::BTreeSet::new()) };
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
    path: std::path::PathBuf,
) {
    let key = (id, generation);
    if !SENDING.with(|s| s.borrow_mut().insert(key)) {
        feedback(window, "A file is already being sent.");
        return;
    }
    feedback(window, "Sending file…");
    let win = window.clone();
    tasks::run(
        move || send_file(&core, id, generation, &path),
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
            preferences::report(&win, "Choose one file or folder at a time.");
            return false;
        }
        let Some(path) = files[0].path() else {
            preferences::report(&win, "Choose a local file.");
            return false;
        };
        let Ok(session) = core.controller_session(id) else {
            return false;
        };
        if session.generation != generation || session.phase != vnilink::SessionPhase::Active {
            return false;
        }
        let peer = session.peer_id;
        let manager = super::filemanager::FileManager::open(
            &win,
            core.clone(),
            peer.clone(),
            "This computer".into(),
            crate::presentation::computer_name(&peer),
        );
        manager.select_upload(path);
        true
    });
    picture.add_controller(drop);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upload_plan_names_the_root_relative_source_and_receiver_name() {
        let root = std::env::temp_dir().join(format!("vnilink-file-ui-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        let approved = root.join("approved");
        let nested = approved.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let file = nested.join("large.bin");
        // Sparse and unread: the plan resolves names, never payload bytes.
        std::fs::File::create(&file)
            .unwrap()
            .set_len(9 * 1024 * 1024)
            .unwrap();
        assert_eq!(
            upload_plan(&file, &[(4, approved.clone())]).unwrap(),
            (4, "nested/large.bin".into(), "large.bin".into())
        );
        // A file directly under the root keeps a flat relative path.
        let top = approved.join("top.txt");
        std::fs::write(&top, []).unwrap();
        assert_eq!(
            upload_plan(&top, &[(4, approved.clone())]).unwrap(),
            (4, "top.txt".into(), "top.txt".into())
        );
        // Outside every root, or a bare root itself, is refused.
        let outside = root.join("approved-sibling");
        std::fs::write(&outside, []).unwrap();
        assert!(upload_plan(&outside, &[(4, approved.clone())]).is_err());
        assert!(upload_plan(&approved, &[(4, approved.clone())]).is_err());
    }
}
