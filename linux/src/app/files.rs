//! Explicit file selection and an installation-scoped receive directory.
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
            tasks::run(
                move || {
                    let file =
                        std::fs::File::open(&path).map_err(|_| "The file could not be read.")?;
                    let mut bytes = Vec::new();
                    use std::io::Read;
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
                        core.publish_file(id, name, &bytes, false)
                            .map_err(|e| e.to_string())
                    } else {
                        core.submit_file(id, generation, name, &bytes, false)
                            .map_err(|e| e.to_string())
                    }
                },
                move |r| match r {
                    Ok(()) => {
                        let dialog = adw::MessageDialog::new(
                            Some(&win),
                            Some("File sent"),
                            Some("The remote computer accepted the file."),
                        );
                        dialog.add_response("close", "Close");
                        dialog.present();
                    }
                    Err(e) => preferences::report(&win, &e),
                },
            );
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
        .description("Choose a folder for this launch before receiving files. The sender also needs your file-transfer permission. Files are limited to 8 MiB; existing files are kept.").build();
    let row = adw::ActionRow::builder()
        .title("Receive folder")
        .subtitle("Not selected for this launch")
        .subtitle_lines(2)
        .build();
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
                            .set_transfer_directory(path)
                            .map_err(|e| e.to_string())
                    },
                    move |r| match r {
                        Ok(()) => row.set_subtitle(&display),
                        Err(e) => preferences::report(&win, &e),
                    },
                );
            },
        );
    });
    let share = adw::PreferencesGroup::builder().title("Share with the connected computer").description("These actions send from this desktop to its current controller. Separate clipboard and file permissions are required.").build();
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
    let clipboard = gtk::Button::with_label("Share clipboard text with controller");
    clipboard.set_halign(gtk::Align::Start);

    share.add(&preferences::action_group(&[&send, &clipboard]));
    let win = window.clone();
    clipboard.connect_clicked(move |_| {
        let worker = core.clone();
        let win = win.clone();
        win.clipboard()
            .read_text_async(None::<&gtk::gio::Cancellable>, move |r| match r {
                Ok(Some(text)) => tasks::run(
                    move || {
                        let session = worker
                            .snapshot()
                            .map_err(|e| e.to_string())?
                            .incoming
                            .filter(|s| s.phase == vnilink::SessionPhase::Active)
                            .ok_or("No computer is connected to this desktop.")?;
                        worker
                            .publish_clipboard(session.id, &text)
                            .map_err(|e| e.to_string())
                    },
                    move |r| {
                        if let Err(e) = r {
                            preferences::report(&win, &e);
                        }
                    },
                ),
                _ => preferences::report(&win, "The local clipboard has no readable text."),
            });
    });
    page.add(&share);
    page
}
