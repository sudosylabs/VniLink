//! Two-pane Files window: this computer and the remote computer side by
//! side, with a transfer queue. Listings are revision-fenced so a late
//! reply can never paint a navigated pane; every transfer is a job the
//! queue drives, conflicts included.
use super::{preferences, service_files::ServiceFiles, tasks};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use vnilink::{Core, DirPage, JobSnapshot, JobState, SessionPhase};

/// Caller-side navigation revisions. Only the newest outstanding listing
/// may paint its pane.
#[derive(Debug, Default)]
pub(crate) struct Revision {
    current: u64,
}

impl Revision {
    pub(crate) fn next(&mut self) -> u64 {
        self.current = self.current.saturating_add(1);
        self.current
    }

    pub(crate) fn accept(&self, revision: u64) -> bool {
        revision != 0 && revision == self.current
    }
}

pub(crate) struct Pane {
    pub(crate) roots: gtk::ComboBoxText,
    pub(crate) sort: gtk::ComboBoxText,
    pub(crate) path: gtk::Label,
    pub(crate) back: gtk::Button,
    pub(crate) refresh: gtk::Button,
    pub(crate) mkdir: gtk::Button,
    pub(crate) rename: gtk::Button,
    pub(crate) list: gtk::ListBox,
    pub(crate) message: gtk::Label,
    pub(crate) revision: Rc<Cell<u64>>,
    pub(crate) root: Rc<Cell<u64>>,
    pub(crate) rel: Rc<RefCell<Vec<String>>>,
    pub(crate) entries: Rc<RefCell<Vec<vnilink::FsEntry>>>,
}

impl Pane {
    fn rel_string(&self) -> String {
        self.rel.borrow().join("/")
    }
}

pub(crate) struct FileManager {
    pub(crate) window: adw::ApplicationWindow,
    driving: Rc<Cell<bool>>,
    pub(crate) local: Pane,
    pub(crate) remote: Pane,
    pub(crate) queue: gtk::ListBox,
    pub(crate) status: gtk::Label,
    pub(crate) core: Arc<Core>,
    files: Arc<ServiceFiles>,
    roots_ready: Cell<bool>,
    pending_upload: RefCell<Option<std::path::PathBuf>>,
    upload_selection: RefCell<Option<String>>,
    pub(crate) peer_id: String,
    pub(crate) stop: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) remote_generation: Rc<Cell<Option<(u64, u64)>>>,
}

thread_local! {
    static WINDOWS: RefCell<std::collections::HashMap<(String, String), Rc<FileManager>>> = RefCell::new(std::collections::HashMap::new());
}

impl FileManager {
    /// The first job waiting for a conflict decision, if any.
    pub(crate) fn waiting_job(&self) -> Option<u64> {
        self.files
            .job_list()
            .ok()?
            .into_iter()
            .find_map(|job| (job.state == vnilink::JobState::WaitingDecision).then_some(job.id))
    }
}

impl FileManager {
    pub(crate) fn open(
        parent: &adw::ApplicationWindow,
        core: Arc<Core>,
        peer_id: String,
        local_name: String,
        remote_name: String,
    ) -> Rc<Self> {
        let key = (core.endpoint_id(), peer_id.clone());
        if let Some(existing) = WINDOWS.with(|windows| windows.borrow().get(&key).cloned()) {
            if !existing.stop.load(std::sync::atomic::Ordering::Relaxed) {
                existing.window.present();
                return existing;
            }
        }
        let window = adw::ApplicationWindow::builder()
            .title(&format!("Files — {remote_name}"))
            .default_width(1020)
            .default_height(640)
            .build();
        if let Some(application) = parent.application() {
            window.set_application(Some(&application));
        }
        let view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::WindowTitle::new(
            &format!("Files — {remote_name}"),
            "Browse both computers and copy files either way",
        )));
        view.add_top_bar(&header);
        let panes = gtk::Paned::new(gtk::Orientation::Horizontal);
        panes.set_shrink_start_child(false);
        panes.set_shrink_end_child(false);
        panes.set_position(500);
        let local = build_pane(&local_name, "This computer", "files_local");
        let remote = build_pane(&remote_name, "Remote computer", "files_remote");
        panes.set_start_child(Some(&local.frame));
        panes.set_end_child(Some(&remote.frame));
        panes.set_vexpand(true);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&panes);
        let transfer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        transfer.set_margin_start(12);
        transfer.set_margin_end(12);
        transfer.set_margin_top(8);
        transfer.set_margin_bottom(8);
        let send = gtk::Button::with_label("Send to remote →");
        send.set_widget_name("files_send");
        send.set_tooltip_text(Some("Copy the selected local items into the remote folder"));
        let receive = gtk::Button::with_label("← Receive here");
        receive.set_widget_name("files_receive");
        receive.set_tooltip_text(Some("Copy the selected remote items into this folder"));
        transfer.append(&send);
        transfer.append(&receive);
        body.append(&transfer);
        let queue_label = gtk::Label::builder()
            .label("Transfer queue")
            .xalign(0.0)
            .margin_start(12)
            .build();
        queue_label.add_css_class("heading");
        body.append(&queue_label);
        let queue = gtk::ListBox::new();
        queue.set_widget_name("files_queue");
        queue.set_selection_mode(gtk::SelectionMode::None);
        let scroll = gtk::ScrolledWindow::builder()
            .min_content_height(140)
            .hexpand(true)
            .vexpand(false)
            .build();
        scroll.set_child(Some(&queue));
        body.append(&scroll);
        view.set_content(Some(&body));
        let status = gtk::Label::builder()
            .xalign(0.0)
            .margin_start(16)
            .margin_end(16)
            .margin_top(8)
            .margin_bottom(8)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        status.add_css_class("caption");
        view.add_bottom_bar(&status);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&view));
        window.set_content(Some(&toasts));

        let manager = Rc::new(Self {
            window,
            driving: Rc::new(Cell::new(false)),
            local: local.pane,
            remote: remote.pane,
            queue,
            status,
            files: Arc::new(ServiceFiles::new(core.clone(), peer_id.clone())),
            roots_ready: Cell::new(false),
            pending_upload: RefCell::new(None),
            upload_selection: RefCell::new(None),
            core,
            peer_id,
            stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            remote_generation: Rc::new(Cell::new(None)),
        });
        WINDOWS.with(|windows| {
            let mut windows = windows.borrow_mut();
            windows.retain(|_, manager| !manager.stop.load(std::sync::atomic::Ordering::Relaxed));
            windows.insert(key.clone(), manager.clone());
        });
        restore_location(&manager, false);
        restore_location(&manager, true);
        wire_pane(&manager, false);
        wire_pane(&manager, true);
        wire_transfers(&manager);
        manager.refresh_local_roots();
        manager.discover_remote_roots();
        manager.refresh_remote();
        manager.refresh_queue();
        let weak = Rc::downgrade(&manager);
        gtk::glib::timeout_add_local(Duration::from_millis(1000), move || {
            let Some(manager) = weak.upgrade() else {
                return gtk::glib::ControlFlow::Break;
            };
            if manager.stop.load(std::sync::atomic::Ordering::Relaxed) {
                return gtk::glib::ControlFlow::Break;
            }
            manager.tick_session();
            // Refresh progress while open; the application driver also
            // advances jobs when this window is closed.
            drive_jobs(&manager);
            gtk::glib::ControlFlow::Continue
        });
        let stop = manager.stop.clone();
        let files = manager.files.clone();
        let owner = Rc::downgrade(&manager);
        manager.window.connect_close_request(move |_| {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            files.close();
            WINDOWS.with(|windows| {
                let mut windows = windows.borrow_mut();
                if windows
                    .get(&key)
                    .is_some_and(|current| owner.ptr_eq(&Rc::downgrade(current)))
                {
                    windows.remove(&key);
                }
            });
            gtk::glib::Propagation::Proceed
        });
        manager.window.present();
        manager
    }

    pub(crate) fn live_session(&self) -> Option<vnilink::SessionInfo> {
        self.core.controller_sessions().ok().and_then(|sessions| {
            sessions
                .into_iter()
                .find(|s| s.phase == SessionPhase::Active && s.peer_id == self.peer_id)
        })
    }

    /// A reconnect mints a new generation: invalidate the remote pane so
    /// late replies to the old one cannot paint it. The first observation
    /// only adopts the session without invalidating: nothing is in flight
    /// from before the window opened.
    pub(crate) fn tick_session(self: &Rc<Self>) {
        let current = self.live_session().map(|s| (s.id, s.generation));
        let known = self.remote_generation.get();
        if known.is_some() && current != known {
            self.remote_generation.set(current);
            // Fence out any in-flight reply for the previous session.
            let revision = self.remote.revision.get().saturating_add(1);
            self.remote.revision.set(revision);
            clear_list(&self.remote.list);
            self.remote.entries.borrow_mut().clear();
            self.remote.message.set_label(if current.is_some() {
                "The connection changed. Refresh the remote folder."
            } else {
                "Not connected."
            });
        } else if known.is_none() {
            self.remote_generation.set(current);
        }
    }

    pub(crate) fn select_upload(self: &Rc<Self>, path: std::path::PathBuf) {
        if self.stop.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        if !self.roots_ready.get() {
            *self.pending_upload.borrow_mut() = Some(path);
            return;
        }
        let revision = self.local.revision.get().saturating_add(1);
        self.local.revision.set(revision);
        let core = self.core.clone();
        let weak = Rc::downgrade(self);
        tasks::run(
            move || selected_upload(&path, &core.file_roots().map_err(|e| e.to_string())?),
            move |result| {
                let Some(manager) = weak.upgrade() else {
                    return;
                };
                if manager.stop.load(std::sync::atomic::Ordering::Relaxed)
                    || manager.local.revision.get() != revision
                {
                    return;
                }
                match result {
                    Ok((root, parent, name)) => {
                        manager.local.roots.set_active_id(Some(&root.to_string()));
                        manager.local.root.set(root);
                        *manager.local.rel.borrow_mut() = parent;
                        manager.local.list.unselect_all();
                        *manager.upload_selection.borrow_mut() = Some(name);
                        manager
                            .status
                            .set_label("Choose the remote destination, then Send.");
                        manager.refresh_local();
                    }
                    Err(error) => manager.status.set_label(&error),
                }
            },
        );
    }

    pub(crate) fn refresh_local_roots(self: &Rc<Self>) {
        let worker = self.files.clone();
        let combo = self.local.roots.clone();
        let root = self.local.root.clone();
        let weak = Rc::downgrade(self);
        tasks::run(
            move || worker.file_roots().map_err(|e| e.to_string()),
            move |result| {
                let Some(manager) = weak.upgrade() else {
                    return;
                };
                if manager.stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                match result {
                    Ok(roots) => {
                        combo.remove_all();
                        for (id, path) in &roots {
                            combo.append(
                                Some(&id.to_string()),
                                &path.to_string_lossy().into_owned(),
                            );
                        }
                        if roots.iter().all(|(id, _)| *id != root.get()) {
                            if let Some((id, _)) = roots.first() {
                                root.set(*id);
                                manager.local.rel.borrow_mut().clear();
                            }
                        }
                        combo.set_active_id(Some(&root.get().to_string()));
                        manager.roots_ready.set(true);
                        let pending = manager.pending_upload.borrow_mut().take();
                        if let Some(path) = pending {
                            manager.select_upload(path);
                        } else {
                            manager.refresh_local();
                        }
                    }
                    Err(error) => {
                        manager.local.message.set_label(&error);
                    }
                }
            },
        );
    }

    /// Discovers the remote host's approved folders by probing ids
    /// `0..16` with a minimal listing. Host paths stay on the host; the
    /// dropdown shows `Shared folder {id}` so every configured root can be
    /// browsed and targeted instead of hardcoding zero.
    pub(crate) fn discover_remote_roots(self: &Rc<Self>) {
        let Some(session) = self.live_session() else {
            return;
        };
        let worker = self.files.clone();
        let combo = self.remote.roots.clone();
        let root = self.remote.root.clone();
        let weak = Rc::downgrade(self);
        tasks::run(
            move || {
                let mut valid = Vec::new();
                for id in 0..16u64 {
                    match worker.fs_list(session.id, session.generation, id, "", 0, 1) {
                        Ok(_) => valid.push(id),
                        Err(e) => {
                            let text = e.to_string();
                            // Unknown roots are skipped; denial/absence ends
                            // discovery with what was found (empty stays empty).
                            if text.contains("unknown")
                                || text.contains("Unknown")
                                || text.contains("no file roots")
                            {
                                continue;
                            }
                            // Denied/stale/closed: stop probing, keep found.
                            if text.contains("denied")
                                || text.contains("Denied")
                                || text.contains("TransferDenied")
                                || text.contains("NoSession")
                                || text.contains("Stale")
                            {
                                break;
                            }
                            continue;
                        }
                    }
                }
                Ok::<Vec<u64>, String>(valid)
            },
            move |result| {
                let Some(manager) = weak.upgrade() else {
                    return;
                };
                if manager.stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                match result {
                    Ok(ids) => {
                        combo.remove_all();
                        for id in &ids {
                            combo.append(Some(&id.to_string()), &format!("Shared folder {id}"));
                        }
                        if !ids.is_empty() {
                            if !ids.contains(&root.get()) {
                                root.set(ids[0]);
                                manager.remote.rel.borrow_mut().clear();
                            }
                            combo.set_active_id(Some(&root.get().to_string()));
                            manager.refresh_remote();
                        } else {
                            manager
                                .remote
                                .message
                                .set_label("The remote computer shares no folders.");
                        }
                    }
                    Err(error) => {
                        manager.remote.message.set_label(&error);
                    }
                }
            },
        );
    }

    pub(crate) fn refresh_local(self: &Rc<Self>) {
        let worker = self.files.clone();
        let revision = self.local.revision.get().saturating_add(1);
        self.local.revision.set(revision);
        let root = self.local.root.get();
        let rel = self.local.rel_string();
        let list = self.local.list.clone();
        let entries = self.local.entries.clone();
        let message = self.local.message.clone();
        let rev = self.local.revision.clone();
        let path_label = self.local.path.clone();
        let weak = Rc::downgrade(self);
        let path_text = if rel.is_empty() {
            "/".to_owned()
        } else {
            format!("/{rel}")
        };
        tasks::run(
            move || {
                // Fetch every page so entries beyond the first 200 stay
                // visible. Pages stay bounded (500 rows each, up to the
                // core's 5000-entry cap); `truncated` is preserved.
                let mut all = Vec::new();
                let mut offset = 0u64;
                let mut total = 0u64;
                let mut truncated = false;
                let mut skipped = 0u64;
                let mut first = true;
                loop {
                    let page = worker
                        .list_dir(root, &rel, offset, 500)
                        .map_err(|e| e.to_string())?;
                    if first {
                        total = page.total;
                        truncated = page.truncated;
                        skipped = page.non_utf8_skipped;
                        first = false;
                    }
                    all.extend(page.entries.iter().cloned());
                    match page.next_offset {
                        Some(next) => {
                            offset = next;
                            if all.len() >= 5000 {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                Ok(vnilink::DirPage {
                    entries: all,
                    total,
                    next_offset: None,
                    truncated,
                    non_utf8_skipped: skipped,
                })
            },
            move |result| {
                if rev.get() != revision {
                    return;
                }
                match result {
                    Ok(page) => {
                        if page.truncated {
                            message.set_label(&format!(
                                "Showing first {} entries.",
                                page.entries.len()
                            ));
                        } else {
                            message.set_label("");
                        }
                        path_label.set_label(&path_text);
                        entries.borrow_mut().clear();
                        entries.borrow_mut().extend(page.entries.iter().cloned());
                        if let Some(manager) = weak.upgrade() {
                            let remote = list == manager.remote.list;
                            remember_location(&manager, remote);
                            paint_sorted(&manager, remote);
                        }
                    }
                    Err(error) => {
                        entries.borrow_mut().clear();
                        clear_list(&list);
                        message.set_label(&error);
                        if let Some(manager) = weak.upgrade() {
                            let remote = list == manager.remote.list;
                            let pane = if remote {
                                &manager.remote
                            } else {
                                &manager.local
                            };
                            if !pane.rel.borrow().is_empty()
                                && (error.contains("folder cannot")
                                    || error.contains("directory cannot")
                                    || error.contains("source folder"))
                            {
                                pane.rel.borrow_mut().pop();
                                if remote {
                                    manager.refresh_remote();
                                } else {
                                    manager.refresh_local();
                                }
                            }
                        }
                    }
                }
            },
        );
    }

    pub(crate) fn refresh_remote(self: &Rc<Self>) {
        let Some(session) = self.live_session() else {
            self.remote.message.set_label("Not connected.");
            clear_list(&self.remote.list);
            self.remote.entries.borrow_mut().clear();
            return;
        };
        let worker = self.files.clone();
        let revision = self.remote.revision.get().saturating_add(1);
        self.remote.revision.set(revision);
        let root = self.remote.root.get();
        let rel = self.remote.rel_string();
        let list = self.remote.list.clone();
        let entries = self.remote.entries.clone();
        let message = self.remote.message.clone();
        let rev = self.remote.revision.clone();
        let path_label = self.remote.path.clone();
        let weak = Rc::downgrade(self);
        let path_text = if rel.is_empty() {
            "/ (shared folder)".to_owned()
        } else {
            format!("/{rel}")
        };
        tasks::run(
            move || {
                let mut all = Vec::new();
                let mut offset = 0u64;
                let mut total = 0u64;
                let mut truncated = false;
                let mut skipped = 0u64;
                let mut first = true;
                loop {
                    let page = worker
                        .fs_list(session.id, session.generation, root, &rel, offset, 500)
                        .map_err(|e| e.to_string())?;
                    if first {
                        total = page.total;
                        truncated = page.truncated;
                        skipped = page.non_utf8_skipped;
                        first = false;
                    }
                    all.extend(page.entries.iter().cloned());
                    match page.next_offset {
                        Some(next) => {
                            offset = next;
                            if all.len() >= 5000 {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                Ok(vnilink::DirPage {
                    entries: all,
                    total,
                    next_offset: None,
                    truncated,
                    non_utf8_skipped: skipped,
                })
            },
            move |result| {
                if rev.get() != revision {
                    return;
                }
                match result {
                    Ok(page) => {
                        if page.truncated {
                            message.set_label(&format!(
                                "Showing first {} entries.",
                                page.entries.len()
                            ));
                        } else {
                            message.set_label("");
                        }
                        path_label.set_label(&path_text);
                        entries.borrow_mut().clear();
                        entries.borrow_mut().extend(page.entries.iter().cloned());
                        if let Some(manager) = weak.upgrade() {
                            let remote = list == manager.remote.list;
                            remember_location(&manager, remote);
                            paint_sorted(&manager, remote);
                        }
                    }
                    Err(error) => {
                        // A failed refresh cannot vouch for old rows
                        // (revocation must read as denied, not stale).
                        entries.borrow_mut().clear();
                        clear_list(&list);
                        message.set_label(&error);
                        if let Some(manager) = weak.upgrade() {
                            let remote = list == manager.remote.list;
                            let pane = if remote {
                                &manager.remote
                            } else {
                                &manager.local
                            };
                            if !pane.rel.borrow().is_empty()
                                && (error.contains("folder cannot")
                                    || error.contains("directory cannot")
                                    || error.contains("source folder"))
                            {
                                pane.rel.borrow_mut().pop();
                                if remote {
                                    manager.refresh_remote();
                                } else {
                                    manager.refresh_local();
                                }
                            }
                        }
                    }
                }
            },
        );
    }

    pub(crate) fn refresh_queue(self: &Rc<Self>) {
        let worker = self.files.clone();
        let queue = self.queue.clone();
        let status = self.status.clone();
        let core = self.files.clone();
        let weak = Rc::downgrade(self);
        tasks::run(
            move || {
                let stamp = worker.owner_stamp();
                worker
                    .job_list()
                    .map(|jobs| (stamp, jobs))
                    .map_err(|e| e.to_string())
            },
            move |result| {
                let Some(manager) = weak.upgrade() else {
                    return;
                };
                if manager.stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                match result {
                    Ok((stamp, jobs)) if stamp == manager.files.owner_stamp() => {
                        paint_queue(&manager, &queue, &status, &core, &jobs)
                    }
                    Ok(_) => {}
                    Err(error) => status.set_label(&error),
                }
            },
        );
    }
}

struct BuiltPane {
    frame: gtk::Box,
    pane: Pane,
}

fn build_pane(title: &str, subtitle: &str, prefix: &str) -> BuiltPane {
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.set_margin_start(12);
    head.set_margin_end(12);
    head.set_margin_top(8);
    head.set_margin_bottom(4);
    let names = gtk::Box::new(gtk::Orientation::Vertical, 0);
    names.set_hexpand(true);
    let heading = gtk::Label::builder().label(title).xalign(0.0).build();
    heading.add_css_class("heading");
    let sub = gtk::Label::builder().label(subtitle).xalign(0.0).build();
    sub.add_css_class("caption");
    names.append(&heading);
    names.append(&sub);
    head.append(&names);
    let back = gtk::Button::builder()
        .icon_name("go-previous-symbolic")
        .tooltip_text("Parent folder")
        .build();
    back.set_widget_name(&format!("{prefix}_back"));
    let refresh = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("Refresh this folder")
        .build();
    refresh.set_widget_name(&format!("{prefix}_refresh"));
    let mkdir = gtk::Button::builder()
        .icon_name("folder-new-symbolic")
        .tooltip_text("New folder here")
        .build();
    mkdir.set_widget_name(&format!("{prefix}_mkdir"));
    let rename = gtk::Button::with_label("Rename…");
    rename.set_widget_name(&format!("{prefix}_rename"));
    rename.set_tooltip_text(Some("Rename the selected entry"));
    head.append(&back);
    head.append(&refresh);
    head.append(&mkdir);
    head.append(&rename);
    frame.append(&head);
    let roots = gtk::ComboBoxText::new();
    roots.set_widget_name(&format!("{prefix}_roots"));
    roots.set_margin_start(12);
    roots.set_margin_end(12);
    frame.append(&roots);
    let sort = gtk::ComboBoxText::new();
    sort.set_widget_name(&format!("{prefix}_sort"));
    sort.set_tooltip_text(Some("Sort files and folders"));
    for (id, label) in [
        ("name", "Name"),
        ("descending", "Name, descending"),
        ("size", "Size"),
        ("modified", "Modified"),
    ] {
        sort.append(Some(id), label);
    }
    sort.set_active_id(Some("name"));
    head.append(&sort);
    let path = gtk::Label::builder().label("/").xalign(0.0).build();
    path.add_css_class("caption");
    path.set_margin_start(12);
    frame.append(&path);
    let message = gtk::Label::builder().xalign(0.5).wrap(true).build();
    message.set_widget_name(&format!("{prefix}_message"));
    message.add_css_class("dim-label");
    frame.append(&message);
    let list = gtk::ListBox::new();
    list.set_widget_name(&format!("{prefix}_list"));
    list.set_selection_mode(gtk::SelectionMode::Multiple);
    list.set_vexpand(true);
    let scroll = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .build();
    scroll.set_child(Some(&list));
    scroll.set_margin_start(12);
    scroll.set_margin_end(12);
    scroll.set_margin_bottom(8);
    frame.append(&scroll);
    BuiltPane {
        frame,
        pane: Pane {
            roots,
            sort,
            path,
            back,
            refresh,
            mkdir,
            rename,
            list,
            message,
            revision: Rc::new(Cell::new(0)),
            root: Rc::new(Cell::new(0)),
            rel: Rc::new(RefCell::new(Vec::new())),
            entries: Rc::new(RefCell::new(Vec::new())),
        },
    }
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(row) = list.first_child() {
        list.remove(&row);
    }
}

fn paint_page(list: &gtk::ListBox, page: &DirPage) {
    clear_list(list);
    for entry in &page.entries {
        let row = adw::ActionRow::builder().title(&entry.name).build();
        row.set_widget_name(&entry.name);
        row.set_activatable(entry.kind == vnilink::EntryKind::Dir);
        let detail = match entry.kind {
            vnilink::EntryKind::File => format!("{} bytes", entry.size),
            vnilink::EntryKind::Dir => "Folder".into(),
            vnilink::EntryKind::Symlink => "Link".into(),
            vnilink::EntryKind::Other => "Other".into(),
        };
        row.set_subtitle(&detail);
        let icon = gtk::Image::from_icon_name(if entry.kind == vnilink::EntryKind::Dir {
            "folder-symbolic"
        } else {
            "text-x-generic-symbolic"
        });
        row.add_prefix(&icon);
        list.append(&row);
    }
}

fn selected_names(list: &gtk::ListBox) -> Vec<String> {
    list.selected_rows()
        .iter()
        .filter_map(|row| {
            row.clone()
                .downcast::<adw::ActionRow>()
                .ok()
                .or_else(|| {
                    row.first_child()
                        .and_then(|child| child.downcast::<adw::ActionRow>().ok())
                })
                .map(|row| row.title().to_string())
        })
        .collect()
}

fn paint_queue(
    manager: &Rc<FileManager>,
    queue: &gtk::ListBox,
    status: &gtk::Label,
    core: &Arc<ServiceFiles>,
    jobs: &[JobSnapshot],
) {
    clear_list(queue);
    if jobs.is_empty() {
        status.set_label("No transfers yet. Select files above and send or receive them.");
        return;
    }
    for job in jobs {
        let row = adw::ActionRow::builder()
            .title(if job.label.is_empty() {
                format!("Transfer #{}", job.id)
            } else {
                job.label.clone()
            })
            .subtitle(&format!(
                "{} · {} · {}/{} files",
                if job.direction == vnilink::JobDirection::Upload {
                    "Upload"
                } else {
                    "Download"
                },
                job_state_label(job.state),
                job.files_done,
                job.files_total
            ))
            .build();
        row.set_widget_name(&format!("job-{}", job.id));
        if job.total_bytes > 0 {
            let bar = gtk::ProgressBar::new();
            bar.set_fraction(job.done_bytes as f64 / job.total_bytes.max(1) as f64);
            bar.set_valign(gtk::Align::Center);
            row.add_suffix(&bar);
        }
        let worker = core.clone();
        let id = job.id;
        let weak = Rc::downgrade(manager);
        match job.state {
            JobState::WaitingDecision => {
                let decide = gtk::Button::with_label("Decide…");
                decide.set_widget_name(&format!("job-{id}-decide"));
                let file = job.waiting_file.clone().unwrap_or_default();
                decide.connect_clicked(move |_| {
                    if let Some(manager) = weak.upgrade() {
                        decide_dialog(&manager, &worker, id, &file);
                    }
                });
                row.add_suffix(&decide);
            }
            JobState::Transferring | JobState::Serving | JobState::Preparing => {
                let cancel = gtk::Button::with_label("Cancel");
                cancel.set_widget_name(&format!("job-{id}-cancel"));
                cancel.connect_clicked(move |_| {
                    if let Some(manager) = weak.upgrade() {
                        let worker = worker.clone();
                        tasks::run(
                            move || worker.job_cancel(id).map_err(|e| e.to_string()),
                            move |_| manager.refresh_queue(),
                        );
                    }
                });
                row.add_suffix(&cancel);
            }
            JobState::Interrupted => {
                let resume = gtk::Button::with_label("Resume");
                resume.set_widget_name(&format!("job-{id}-resume"));
                resume.connect_clicked(move |_| {
                    if let Some(manager) = weak.upgrade() {
                        let worker = worker.clone();
                        tasks::run(
                            move || worker.job_resume(id).map_err(|e| e.to_string()),
                            move |_| manager.refresh_queue(),
                        );
                    }
                });
                row.add_suffix(&resume);
            }
            JobState::Cancelled | JobState::Failed => {
                let resume = gtk::Button::with_label("Resume");
                resume.set_widget_name(&format!("job-{id}-resume"));
                let worker_resume = worker.clone();
                let weak_resume = Rc::downgrade(manager);
                resume.connect_clicked(move |_| {
                    if let Some(manager) = weak_resume.upgrade() {
                        let worker = worker_resume.clone();
                        tasks::run(
                            move || worker.job_resume(id).map_err(|e| e.to_string()),
                            move |_| manager.refresh_queue(),
                        );
                    }
                });
                row.add_suffix(&resume);
                let forget = gtk::Button::with_label("Forget");
                forget.set_widget_name(&format!("job-{id}-forget"));
                forget.connect_clicked(move |_| {
                    if let Some(manager) = weak.upgrade() {
                        let worker = worker.clone();
                        tasks::run(
                            move || worker.job_forget(id).map_err(|e| e.to_string()),
                            move |_| manager.refresh_queue(),
                        );
                    }
                });
                row.add_suffix(&forget);
            }
            JobState::Complete => {
                let forget = gtk::Button::with_label("Forget");
                forget.set_widget_name(&format!("job-{id}-forget"));
                forget.connect_clicked(move |_| {
                    if let Some(manager) = weak.upgrade() {
                        let worker = worker.clone();
                        tasks::run(
                            move || worker.job_forget(id).map_err(|e| e.to_string()),
                            move |_| manager.refresh_queue(),
                        );
                    }
                });
                row.add_suffix(&forget);
            }
        }
        if let Some(error) = job.error.as_ref() {
            let subtitle = row.subtitle();
            row.set_subtitle(&format!("{} · {error}", subtitle.as_deref().unwrap_or("")));
        }
        queue.append(&row);
    }
    status.set_label(&format!(
        "{} of {} transfers complete",
        jobs.iter()
            .filter(|job| job.state == JobState::Complete)
            .count(),
        jobs.len()
    ));
}

fn job_state_label(state: JobState) -> &'static str {
    match state {
        JobState::Preparing => "Preparing",
        JobState::Serving => "Serving",
        JobState::Transferring => "Transferring",
        JobState::WaitingDecision => "Needs a decision",
        JobState::Interrupted => "Interrupted",
        JobState::Cancelled => "Cancelled",
        JobState::Failed => "Failed",
        JobState::Complete => "Complete",
    }
}

fn decide_dialog(manager: &Rc<FileManager>, core: &Arc<ServiceFiles>, id: u64, file: &str) {
    let dialog = adw::MessageDialog::new(
        Some(&manager.window),
        Some("Name already exists"),
        Some(&format!(
            "“{file}” already exists at the destination. The original stays untouched unless you replace it."
        )),
    );
    dialog.add_responses(&[
        ("skip", "Skip"),
        ("replace", "Replace"),
        ("both", "Keep both"),
    ]);
    dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
    let check = gtk::CheckButton::with_label("Apply to the remaining conflicts");
    check.set_widget_name(&format!("job-{id}-apply-remaining"));
    dialog.set_extra_child(Some(&check));
    let worker = core.clone();
    let weak = Rc::downgrade(manager);
    dialog.connect_response(None, move |dialog, response| {
        let choice = match response {
            "replace" => vnilink::ConflictChoice::Replace,
            "both" => vnilink::ConflictChoice::KeepBoth,
            _ => vnilink::ConflictChoice::Skip,
        };
        let apply = check.is_active();
        let worker = worker.clone();
        let weak = weak.clone();
        tasks::run(
            move || {
                worker
                    .job_resolve(id, choice, apply)
                    .map_err(|e| e.to_string())
            },
            move |_| {
                if let Some(manager) = weak.upgrade() {
                    manager.refresh_queue();
                }
            },
        );
        dialog.close();
    });
    dialog.present();
}

fn name_dialog(
    parent: &adw::ApplicationWindow,
    title: &str,
    initial: &str,
    name: &str,
    on_name: impl Fn(String) + 'static,
) {
    let dialog = adw::MessageDialog::new(Some(parent), Some(title), None);
    let entry = gtk::Entry::builder().text(initial).build();
    entry.set_widget_name(name);
    entry.set_activates_default(true);
    dialog.set_extra_child(Some(&entry));
    dialog.add_responses(&[("cancel", "Cancel"), ("ok", "OK")]);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("ok"));
    dialog.connect_response(None, move |dialog, response| {
        if response == "ok" {
            on_name(entry.text().to_string());
        }
        dialog.close();
    });
    dialog.present();
}

fn wire_pane(manager: &Rc<FileManager>, remote: bool) {
    let pane = if remote {
        &manager.remote
    } else {
        &manager.local
    };
    let weak = Rc::downgrade(manager);
    pane.sort.connect_changed(move |_| {
        if let Some(manager) = weak.upgrade() {
            paint_sorted(&manager, remote);
        }
    });
    wire_drag(manager, remote);
    let weak = Rc::downgrade(manager);
    let back = if remote {
        manager.remote.back.clone()
    } else {
        manager.local.back.clone()
    };
    back.connect_clicked(move |_| {
        if let Some(manager) = weak.upgrade() {
            if remote {
                manager.remote.rel.borrow_mut().pop();
                manager.refresh_remote();
            } else {
                manager.local.rel.borrow_mut().pop();
                manager.refresh_local();
            }
        }
    });
    let weak = Rc::downgrade(manager);
    let refresh = if remote {
        manager.remote.refresh.clone()
    } else {
        manager.local.refresh.clone()
    };
    refresh.connect_clicked(move |_| {
        if let Some(manager) = weak.upgrade() {
            if remote {
                manager.refresh_remote();
            } else {
                manager.refresh_local();
            }
        }
    });
    let weak = Rc::downgrade(manager);
    let mkdir = if remote {
        manager.remote.mkdir.clone()
    } else {
        manager.local.mkdir.clone()
    };
    mkdir.connect_clicked(move |_| {
        let Some(manager) = weak.upgrade() else {
            return;
        };
        let window = manager.window.clone();
        let name = if remote {
            "files_remote_mkdir_name"
        } else {
            "files_local_mkdir_name"
        };
        name_dialog(&window, "New folder", "", name, move |folder| {
            if folder.is_empty() {
                return;
            }
            if remote {
                mkdir_remote(&manager, &folder);
            } else {
                mkdir_local(&manager, &folder);
            }
        });
    });
    let weak = Rc::downgrade(manager);
    let rename = if remote {
        manager.remote.rename.clone()
    } else {
        manager.local.rename.clone()
    };
    rename.connect_clicked(move |_| {
        if let Some(manager) = weak.upgrade() {
            rename_selected(&manager, remote);
        }
    });
    let weak = Rc::downgrade(manager);
    let roots = if remote {
        manager.remote.roots.clone()
    } else {
        manager.local.roots.clone()
    };
    // Both panes switch roots: local via `file_roots`, remote via probed
    // approved folders (ids only; host paths stay on the host).
    roots.connect_changed(move |combo| {
        let Some(manager) = weak.upgrade() else {
            return;
        };
        if let Some(id) = combo.active_id() {
            if let Ok(root) = id.parse::<u64>() {
                if remote {
                    if manager.remote.root.get() != root {
                        manager.remote.rel.borrow_mut().clear();
                    }
                    manager.remote.root.set(root);
                    manager.refresh_remote();
                } else {
                    if manager.local.root.get() != root {
                        manager.local.rel.borrow_mut().clear();
                    }
                    manager.local.root.set(root);
                    manager.refresh_local();
                }
            }
        }
    });
    let weak = Rc::downgrade(manager);
    let list = if remote {
        manager.remote.list.clone()
    } else {
        manager.local.list.clone()
    };
    let list_weak = list.downgrade();
    list.connect_row_activated(move |_, _| {
        let Some(manager) = weak.upgrade() else {
            return;
        };
        let Some(list) = list_weak.upgrade() else {
            return;
        };
        // Double-clicking a folder enters it; files stay selected for
        // the transfer buttons.
        let entries = if remote {
            manager.remote.entries.borrow().clone()
        } else {
            manager.local.entries.borrow().clone()
        };
        let selected = selected_names(&list);
        for name in selected {
            if entries
                .iter()
                .any(|entry| entry.name == name && entry.kind == vnilink::EntryKind::Dir)
            {
                enter_folder(&manager, remote, &name);
                break;
            }
        }
    });
}

fn mkdir_local(manager: &Rc<FileManager>, name: &str) {
    let worker = manager.files.clone();
    let root = manager.local.root.get();
    let rel = manager.local.rel_string();
    let name = name.to_owned();
    let weak = Rc::downgrade(manager);
    tasks::run(
        move || {
            worker
                .create_dir(root, &rel, &name)
                .map_err(|e| e.to_string())
        },
        move |result| {
            if let Some(manager) = weak.upgrade() {
                match result {
                    Ok(()) => manager.refresh_local(),
                    Err(error) => manager.local.message.set_label(&error),
                }
            }
        },
    );
}

fn mkdir_remote(manager: &Rc<FileManager>, name: &str) {
    let Some(session) = manager.live_session() else {
        manager.remote.message.set_label("Not connected.");
        return;
    };
    let worker = manager.files.clone();
    let root = manager.remote.root.get();
    let rel = manager.remote.rel_string();
    let name = name.to_owned();
    let weak = Rc::downgrade(manager);
    tasks::run(
        move || {
            worker
                .fs_mkdir(session.id, session.generation, root, &rel, &name)
                .map_err(|e| e.to_string())
        },
        move |result| {
            if let Some(manager) = weak.upgrade() {
                match result {
                    Ok(()) => manager.refresh_remote(),
                    Err(error) => manager.remote.message.set_label(&error),
                }
            }
        },
    );
}

fn rename_selected(manager: &Rc<FileManager>, remote: bool) {
    let list = if remote {
        manager.remote.list.clone()
    } else {
        manager.local.list.clone()
    };
    let names = selected_names(&list);
    if names.len() != 1 {
        return;
    }
    let name = names[0].clone();
    let window = manager.window.clone();
    let entry = if remote {
        "files_remote_rename_name"
    } else {
        "files_local_rename_name"
    };
    let weak = Rc::downgrade(manager);
    let current = name.clone();
    name_dialog(&window, "Rename", &name, entry, move |new_name| {
        let Some(manager) = weak.upgrade() else {
            return;
        };
        if new_name.is_empty() || new_name == current {
            return;
        }
        rename_entry(&manager, remote, &current, &new_name);
    });
}

fn rename_entry(manager: &Rc<FileManager>, remote: bool, name: &str, new_name: &str) {
    // The listing token fences the rename against a replaced source.
    let token = entry_token(manager, remote, name);
    if remote {
        let Some(session) = manager.live_session() else {
            manager.remote.message.set_label("Not connected.");
            return;
        };
        let worker = manager.files.clone();
        let root = manager.remote.root.get();
        let rel = manager.remote.rel_string();
        let (name, new_name) = (name.to_owned(), new_name.to_owned());
        let weak = Rc::downgrade(manager);
        tasks::run(
            move || {
                worker
                    .fs_rename(
                        session.id,
                        session.generation,
                        root,
                        &rel,
                        &name,
                        token,
                        &new_name,
                    )
                    .map_err(|e| e.to_string())
            },
            move |result| {
                if let Some(manager) = weak.upgrade() {
                    match result {
                        Ok(()) => manager.refresh_remote(),
                        Err(error) => manager.remote.message.set_label(&error),
                    }
                }
            },
        );
    } else {
        let worker = manager.files.clone();
        let root = manager.local.root.get();
        let rel = manager.local.rel_string();
        let (name, new_name) = (name.to_owned(), new_name.to_owned());
        let weak = Rc::downgrade(manager);
        tasks::run(
            move || {
                worker
                    .rename_entry(root, &rel, &name, token, &new_name)
                    .map_err(|e| e.to_string())
            },
            move |result| {
                if let Some(manager) = weak.upgrade() {
                    match result {
                        Ok(()) => manager.refresh_local(),
                        Err(error) => manager.local.message.set_label(&error),
                    }
                }
            },
        );
    }
}

fn entry_token(manager: &FileManager, remote: bool, name: &str) -> Option<vnilink::EntryToken> {
    // Tokens come from the last painted page; a missing entry renames
    // unfenced rather than refusing outright.
    let entries = if remote {
        manager.remote.entries.borrow()
    } else {
        manager.local.entries.borrow()
    };
    entries
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.token)
}

fn wire_transfers(manager: &Rc<FileManager>) {
    let weak = Rc::downgrade(manager);
    let send = super::widgets::named::<gtk::Button>(&manager.window, "files_send");
    send.connect_clicked(move |_| {
        if let Some(manager) = weak.upgrade() {
            submit_selection(&manager, false);
        }
    });
    let weak = Rc::downgrade(manager);
    let receive = super::widgets::named::<gtk::Button>(&manager.window, "files_receive");
    receive.connect_clicked(move |_| {
        if let Some(manager) = weak.upgrade() {
            submit_selection(&manager, true);
        }
    });
}

fn enter_folder(manager: &Rc<FileManager>, remote: bool, name: &str) {
    if remote {
        manager.remote.rel.borrow_mut().push(name.to_owned());
        manager.refresh_remote();
    } else {
        manager.local.rel.borrow_mut().push(name.to_owned());
        manager.refresh_local();
    }
}

fn submit_selection(manager: &Rc<FileManager>, download: bool) {
    if manager.live_session().is_none() {
        manager.status.set_label("Not connected.");
        return;
    }
    let (names, src_root, src_rel, dst_root, dst_rel) = if download {
        (
            selected_names(&manager.remote.list),
            manager.remote.root.get(),
            manager.remote.rel_string(),
            manager.local.root.get(),
            manager.local.rel_string(),
        )
    } else {
        (
            selected_names(&manager.local.list),
            manager.local.root.get(),
            manager.local.rel_string(),
            manager.remote.root.get(),
            manager.remote.rel_string(),
        )
    };
    if names.is_empty() {
        manager.status.set_label("Select files or folders first.");
        return;
    }
    submit_items(
        manager, download, names, src_root, src_rel, dst_root, dst_rel,
    );
}

fn submit_items(
    manager: &Rc<FileManager>,
    download: bool,
    names: Vec<String>,
    src_root: u64,
    src_rel: String,
    dst_root: u64,
    dst_rel: String,
) {
    let Some(session) = manager.live_session() else {
        manager.status.set_label("Not connected.");
        return;
    };
    let worker = manager.files.clone();
    let weak = Rc::downgrade(manager);
    tasks::run(
        move || {
            for name in &names {
                let rel = if src_rel.is_empty() {
                    name.clone()
                } else {
                    format!("{src_rel}/{name}")
                };
                if download {
                    worker
                        .download(
                            session.id,
                            session.generation,
                            src_root,
                            &rel,
                            dst_root,
                            &dst_rel,
                        )
                        .map_err(|e| e.to_string())?;
                } else {
                    worker
                        .upload(
                            session.id,
                            session.generation,
                            src_root,
                            &rel,
                            dst_root,
                            &dst_rel,
                        )
                        .map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        },
        move |result: Result<(), String>| {
            if let Some(manager) = weak.upgrade() {
                match result {
                    Ok(()) => {
                        manager.status.set_label(if download {
                            "Download queued."
                        } else {
                            "Upload queued."
                        });
                        manager.refresh_queue();
                    }
                    Ok(_) => {}
                    Err(error) => manager.status.set_label(&error),
                }
            }
        },
    );
}

fn paint_sorted(manager: &FileManager, remote: bool) {
    let pane = if remote {
        &manager.remote
    } else {
        &manager.local
    };
    let selection = if remote {
        selected_names(&pane.list)
    } else {
        manager
            .upload_selection
            .borrow_mut()
            .take()
            .map(|name| vec![name])
            .unwrap_or_else(|| selected_names(&pane.list))
    };
    let mut entries = pane.entries.borrow().clone();
    let sort = pane
        .sort
        .active_id()
        .map(|id| id.to_string())
        .unwrap_or_default();
    entries.sort_by(|a, b| {
        let folders = (b.kind == vnilink::EntryKind::Dir).cmp(&(a.kind == vnilink::EntryKind::Dir));
        let order = match sort.as_str() {
            "size" => b.size.cmp(&a.size),
            "modified" => b.mtime_ms.cmp(&a.mtime_ms),
            "descending" => b.name.to_lowercase().cmp(&a.name.to_lowercase()),
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };
        folders.then(order).then(a.name.cmp(&b.name))
    });
    paint_page(
        &pane.list,
        &DirPage {
            entries,
            total: pane.entries.borrow().len() as u64,
            next_offset: None,
            truncated: false,
            non_utf8_skipped: 0,
        },
    );
    let mut child = pane.list.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if selection.contains(&widget.widget_name().to_string()) {
            if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
                pane.list.select_row(Some(&row));
            }
        }
    }
}

fn location_file(manager: &FileManager, remote: bool) -> std::path::PathBuf {
    let peer: String = manager
        .peer_id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    gtk::glib::user_config_dir()
        .join("vnilink/files-locations")
        .join(format!(
            "{peer}-{}",
            if remote { "remote" } else { "local" }
        ))
}

fn remember_location(manager: &FileManager, remote: bool) {
    let pane = if remote {
        &manager.remote
    } else {
        &manager.local
    };
    let keyfile = gtk::glib::KeyFile::new();
    keyfile.set_uint64("pane", "root", pane.root.get());
    keyfile.set_string("pane", "path", &pane.rel_string());
    let path = location_file(manager, remote);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = keyfile.save_to_file(path);
}

fn restore_location(manager: &FileManager, remote: bool) {
    let keyfile = gtk::glib::KeyFile::new();
    if keyfile
        .load_from_file(
            location_file(manager, remote),
            gtk::glib::KeyFileFlags::NONE,
        )
        .is_err()
    {
        return;
    }
    let (Ok(root), Ok(rel)) = (
        keyfile.uint64("pane", "root"),
        keyfile.string("pane", "path"),
    ) else {
        return;
    };
    if rel.split('/').any(|s| s == "." || s == "..") || rel.starts_with('/') {
        return;
    }
    let pane = if remote {
        &manager.remote
    } else {
        &manager.local
    };
    pane.root.set(root);
    *pane.rel.borrow_mut() = rel
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
}

#[derive(Clone)]
struct FilesDrag {
    window: adw::ApplicationWindow,
    remote: bool,
    root: u64,
    path: String,
    names: Vec<String>,
}

fn wire_drag(manager: &Rc<FileManager>, remote: bool) {
    let pane = if remote {
        &manager.remote
    } else {
        &manager.local
    };
    let source = gtk::DragSource::new();
    source.set_actions(gtk::gdk::DragAction::COPY);
    let weak = Rc::downgrade(manager);
    source.connect_prepare(move |_, _, y| {
        let manager = weak.upgrade()?;
        let pane = if remote {
            &manager.remote
        } else {
            &manager.local
        };
        let row = pane.list.row_at_y(y as i32)?;
        if !row.is_selected() {
            pane.list.unselect_all();
            pane.list.select_row(Some(&row));
        }
        let names = selected_names(&pane.list);
        if names.is_empty() {
            return None;
        }
        let payload = gtk::glib::BoxedAnyObject::new(FilesDrag {
            window: manager.window.clone(),
            remote,
            root: pane.root.get(),
            path: pane.rel_string(),
            names,
        });
        Some(gtk::gdk::ContentProvider::for_value(&payload.to_value()))
    });
    pane.list.add_controller(source);
    let target = gtk::DropTarget::new(
        gtk::glib::BoxedAnyObject::static_type(),
        gtk::gdk::DragAction::COPY,
    );
    let weak = Rc::downgrade(manager);
    target.connect_drop(move |_, value, _, y| {
        let Some(manager) = weak.upgrade() else {
            return false;
        };
        let Ok(object) = value.get::<gtk::glib::BoxedAnyObject>() else {
            return false;
        };
        let Ok(payload) = object.try_borrow::<FilesDrag>() else {
            return false;
        };
        if payload.window != manager.window || payload.remote == remote {
            return false;
        }
        let pane = if remote {
            &manager.remote
        } else {
            &manager.local
        };
        let mut destination = pane.rel_string();
        if let Some(row) = pane.list.row_at_y(y as i32) {
            let name = row.widget_name().to_string();
            if pane
                .entries
                .borrow()
                .iter()
                .any(|e| e.name == name && e.kind == vnilink::EntryKind::Dir)
            {
                if !destination.is_empty() {
                    destination.push('/');
                }
                destination.push_str(&name);
            }
        }
        submit_items(
            &manager,
            payload.remote,
            payload.names.clone(),
            payload.root,
            payload.path.clone(),
            pane.root.get(),
            destination,
        );
        true
    });
    pane.list.add_controller(target);
}

fn drive_jobs(manager: &Rc<FileManager>) {
    if manager.driving.get() {
        return;
    }
    manager.driving.set(true);
    let worker = manager.files.clone();
    let weak = Rc::downgrade(manager);
    tasks::run(
        move || {
            let stamp = worker.owner_stamp();
            let jobs = worker.job_list().map_err(|e| e.to_string())?;
            for job in jobs.iter().filter(|job| {
                !matches!(
                    job.state,
                    JobState::Complete | JobState::Cancelled | JobState::Failed
                ) && (job.sender || job.state != JobState::WaitingDecision)
            }) {
                worker.job_poll(job.id).map_err(|e| e.to_string())?;
            }
            worker
                .job_list()
                .map(|jobs| (stamp, jobs))
                .map_err(|e| e.to_string())
        },
        move |result| {
            if let Some(manager) = weak.upgrade() {
                manager.driving.set(false);
                match result {
                    Ok((stamp, jobs)) if stamp == manager.files.owner_stamp() => {
                        paint_queue(
                            &manager,
                            &manager.queue,
                            &manager.status,
                            &manager.files,
                            &jobs,
                        );
                        if jobs.iter().any(|job| job.state == JobState::Complete) {
                            manager.refresh_local();
                            manager.refresh_remote();
                        }
                    }
                    Ok(_) => {}
                    Err(error) => manager.status.set_label(&error),
                }
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::Revision;

    #[test]
    fn revisions_fence_stale_replies() {
        let mut gate = Revision::default();
        assert!(!gate.accept(0));
        let first = gate.next();
        let second = gate.next();
        assert_ne!(first, second);
        assert!(!gate.accept(first));
        assert!(gate.accept(second));
    }
}

/// Locate one item inside an approved root without reading file bytes.
pub(crate) fn selected_upload(
    path: &std::path::Path,
    roots: &[(u64, std::path::PathBuf)],
) -> Result<(u64, Vec<String>, String), String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| "The selected item could not be read.")?;
    if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
        return Err("Choose a regular file or folder. Links cannot be sent.".into());
    }
    let path = path
        .canonicalize()
        .map_err(|_| "The selected item could not be read.")?;
    let mut selected = None;
    for (id, root) in roots {
        let Ok(root) = root.canonicalize() else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(&root) else {
            continue;
        };
        let parts = relative
            .components()
            .map(|c| c.as_os_str().to_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or("The selected name is unsupported.")?;
        if parts.is_empty() {
            continue;
        }
        let candidate = (
            *id,
            parts[..parts.len() - 1].to_vec(),
            parts.last().unwrap().clone(),
        );
        if selected
            .as_ref()
            .is_none_or(|old: &(u64, Vec<String>, String)| candidate.1.len() < old.1.len())
        {
            selected = Some(candidate);
        }
    }
    selected.ok_or_else(|| {
        "Choose an item inside a shared folder, or add its folder in Settings → Files.".into()
    })
}

#[cfg(test)]
mod upload_selection_tests {
    use super::*;
    #[test]
    fn approved_selection_checks_boundaries_without_reading_payload() {
        let directory =
            std::env::temp_dir().join(format!("vnilink-upload-selection-{}", std::process::id()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(directory.clone());
        let approved = directory.join("approved");
        let nested = approved.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let file = nested.join("large.bin");
        std::fs::File::create(&file)
            .unwrap()
            .set_len(vnilink::MAX_FILE_BYTES as u64 + 1)
            .unwrap();
        assert_eq!(
            selected_upload(&file, &[(4, approved.clone())]).unwrap(),
            (4, vec!["nested".into()], "large.bin".into())
        );
        assert_eq!(
            selected_upload(&file, &[(4, approved.clone()), (8, nested.clone())]).unwrap(),
            (8, vec![], "large.bin".into())
        );
        assert_eq!(
            selected_upload(&nested, &[(4, approved.clone())]).unwrap(),
            (4, vec![], "nested".into())
        );
        assert!(selected_upload(&approved, &[(4, approved.clone())]).is_err());
        let outside = directory.join("approved-sibling");
        std::fs::write(&outside, []).unwrap();
        assert!(selected_upload(&outside, &[(4, approved.clone())]).is_err());
        let link = nested.join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(selected_upload(&link, &[(4, approved)]).is_err());
    }
}
