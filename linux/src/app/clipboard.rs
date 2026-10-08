//! GTK clipboard integration. Text changes are synchronized only to active
//! sessions; remote ownership suppresses echoes and stale reads are discarded.
use super::files;
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use vnilink::{Core, CoreEvent, SessionInfo, SessionPhase, SessionRole};

struct Write {
    text: String,
    reply: mpsc::Sender<Result<(), vnilink::Error>>,
}
struct Adapter(mpsc::SyncSender<Write>);
impl vnilink::HostClipboard for Adapter {
    fn set_text(&mut self, text: &str) -> Result<(), vnilink::Error> {
        let (reply, done) = mpsc::channel();
        self.0
            .try_send(Write {
                text: text.into(),
                reply,
            })
            .map_err(|_| vnilink::Error::Busy)?;
        done.recv_timeout(Duration::from_secs(2))
            .map_err(|_| vnilink::Error::ShutDown)?
    }
}
#[derive(Clone)]
struct Native {
    board: gtk::gdk::Clipboard,
    remote: Rc<RefCell<Option<gtk::gdk::ContentProvider>>>,
    revision: Rc<Cell<u64>>,
    reading: Rc<RefCell<Option<gtk::gio::Cancellable>>>,
}
impl Native {
    fn apply(&self, text: &str) -> Result<(), vnilink::Error> {
        let provider = gtk::gdk::ContentProvider::for_value(&text.to_value());
        // Mark the provider before GTK emits changed synchronously.
        self.remote.replace(Some(provider.clone()));
        self.board
            .set_content(Some(&provider))
            .map_err(|_| vnilink::Error::Unsupported("the desktop clipboard could not be written"))
    }
    fn is_remote(&self) -> bool {
        self.remote
            .borrow()
            .as_ref()
            .is_some_and(|p| self.board.content().as_ref() == Some(p))
    }
}
fn native_adapter(window: &adw::ApplicationWindow) -> (Adapter, Native) {
    let (sender, receiver) = mpsc::sync_channel::<Write>(1);
    let native = Native {
        board: window.clipboard(),
        remote: Rc::new(RefCell::new(None)),
        revision: Rc::new(Cell::new(0)),
        reading: Rc::new(RefCell::new(None)),
    };
    let owner = window.downgrade();
    let writer = native.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(16), move || {
        if owner.upgrade().is_none() {
            return gtk::glib::ControlFlow::Break;
        }
        match receiver.try_recv() {
            Ok(write) => {
                let _ = write.reply.send(writer.apply(&write.text));
            }
            Err(mpsc::TryRecvError::Disconnected) => return gtk::glib::ControlFlow::Break,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        gtk::glib::ControlFlow::Continue
    });
    (Adapter(sender), native)
}
fn same(core: &Core, session: &SessionInfo) -> bool {
    match session.role {
        SessionRole::Controller => core.controller_session(session.id).ok(),
        SessionRole::Host => core.snapshot().ok().and_then(|s| s.incoming),
    }
    .is_some_and(|s| {
        s.id == session.id && s.generation == session.generation && s.phase == SessionPhase::Active
    })
}
struct Copy {
    text: String,
    sessions: Vec<SessionInfo>,
}
#[cfg(test)]
pub(crate) fn install(window: &adw::ApplicationWindow, core: Arc<Core>) {
    install_selected(window, core, None);
}
pub(crate) fn install_for_tabs(
    window: &adw::ApplicationWindow,
    core: Arc<Core>,
    selected: Arc<AtomicU64>,
) {
    install_selected(window, core, Some(selected));
}
fn install_selected(
    window: &adw::ApplicationWindow,
    core: Arc<Core>,
    selected: Option<Arc<AtomicU64>>,
) {
    let (adapter, native) = native_adapter(window);
    // Install before hosting resumes: no network operation is involved here.
    if let Err(e) = core.set_clipboard(Box::new(adapter)) {
        files::feedback(window.upcast_ref(), &e.to_string());
        return;
    }
    let latest = Arc::new(Mutex::new(None::<Copy>));
    let pending = latest.clone();
    let (wake, work) = mpsc::sync_channel(1);
    let (error_tx, errors) = mpsc::sync_channel(1);
    let worker = Arc::downgrade(&core);
    let worker_selected = selected.clone();
    std::thread::spawn(move || {
        while work.recv().is_ok() {
            loop {
                let Some(copy) = pending.lock().unwrap().take() else {
                    break;
                };
                let Some(core) = worker.upgrade() else {
                    return;
                };
                for session in copy.sessions {
                    if !same(&core, &session)
                        || (session.role == SessionRole::Controller
                            && worker_selected
                                .as_ref()
                                .is_some_and(|id| id.load(Ordering::Relaxed) != session.id))
                    {
                        continue;
                    }
                    let result = match session.role {
                        SessionRole::Controller => {
                            core.submit_clipboard(session.id, session.generation, &copy.text)
                        }
                        SessionRole::Host => core.publish_clipboard(session.id, &copy.text),
                    };
                    if let Err(e) = result {
                        if !matches!(
                            e,
                            vnilink::Error::ClipboardDenied
                                | vnilink::Error::NoSession
                                | vnilink::Error::StaleSession
                        ) {
                            let _ = error_tx.try_send(e.to_string());
                        }
                    }
                }
            }
        }
    });
    let observer = native.clone();
    let local_core = Arc::downgrade(&core);
    let changed_selected = selected.clone();
    let handler = native.board.connect_changed(move |board| {
        let revision = observer.revision.get().wrapping_add(1);
        observer.revision.set(revision);
        if let Some(read) = observer.reading.borrow_mut().take() {
            read.cancel();
        }
        if observer.is_remote() {
            return;
        }
        let Some(core) = local_core.upgrade() else {
            return;
        };
        let Ok(snapshot) = core.snapshot() else {
            return;
        };
        let mut sessions = Vec::new();
        let outgoing = selected_controller(&core, &changed_selected);
        if let Some(s) = outgoing.clone() {
            sessions.push(s);
        }
        if let Some(s) = snapshot
            .incoming
            .filter(|s| s.phase == SessionPhase::Active)
        {
            if snapshot
                .devices
                .iter()
                .any(|d| d.endpoint_id == s.peer_id && d.grants.clipboard)
            {
                sessions.push(s);
            }
        }
        if sessions.is_empty() {
            return;
        }
        let observer = observer.clone();
        let latest = latest.clone();
        let wake = wake.clone();
        let read = gtk::gio::Cancellable::new();
        observer.reading.replace(Some(read.clone()));
        board.read_text_async(Some(&read), move |result| {
            if observer.revision.get() != revision || observer.is_remote() {
                return;
            }
            observer.reading.borrow_mut().take();
            let Ok(Some(text)) = result else {
                return;
            };
            if text.len() > vnilink::MAX_CLIPBOARD_TEXT {
                return;
            }
            *latest.lock().unwrap() = Some(Copy {
                text: text.into(),
                sessions,
            });
            let _ = wake.try_send(()); // A full wake queue already schedules the latest copy.
        });
    });
    let mut handler = Some(handler);
    let events = core.subscribe().ok();
    let weak_core = Arc::downgrade(&core);
    let owner = window.downgrade();
    let mut lagged = 0;
    let mut clipboard_session = None;
    gtk::glib::timeout_add_local(Duration::from_millis(30), move || {
        let alive = owner.upgrade().zip(weak_core.upgrade());
        let Some((window, core)) = alive else {
            if let Some(handler) = handler.take() {
                native.board.disconnect(handler);
            }
            return gtk::glib::ControlFlow::Break;
        };
        let Ok(_snapshot) = core.snapshot() else {
            if let Some(handler) = handler.take() {
                native.board.disconnect(handler);
            }
            return gtk::glib::ControlFlow::Break;
        };
        let outgoing = selected_controller(&core, &selected);
        let stamp = outgoing.as_ref().map(|s| (s.id, s.generation));
        let mut received = stamp != clipboard_session;
        clipboard_session = stamp;
        if let Some(events) = &events {
            while let Some(event) = events.try_recv() {
                if let CoreEvent::ClipboardReceived { id } = event {
                    received |= outgoing.as_ref().is_some_and(|s| s.id == id);
                }
            }
            let dropped = events.lagged();
            received |= dropped != lagged;
            lagged = dropped;
        }
        if received && outgoing.is_some() {
            let session = outgoing.as_ref().unwrap();
            if let Ok(Some(text)) = core.session_clipboard_text(session.id, session.generation) {
                let _ = native.apply(&text);
            }
        }
        if let Ok(e) = errors.try_recv() {
            files::feedback(window.upcast_ref(), &format!("Clipboard sync failed: {e}"));
        }
        gtk::glib::ControlFlow::Continue
    });
}

fn selected_controller(core: &Core, selected: &Option<Arc<AtomicU64>>) -> Option<SessionInfo> {
    match selected {
        Some(id) => core.controller_session(id.load(Ordering::Relaxed)).ok(),
        None => core.snapshot().ok().and_then(|s| s.outgoing),
    }
    .filter(|s| s.phase == SessionPhase::Active)
}

#[cfg(test)]
#[path = "clipboard_tests.rs"]
pub(super) mod tests;
