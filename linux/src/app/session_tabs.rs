//! One application window; each tab retains its own viewer across selection.
use super::viewer::Viewer;
use adw::prelude::*;
use gtk::glib::{self, ControlFlow, Propagation};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use vnilink::{Core, SessionInfo, SessionPhase};

struct SessionTab {
    page: adw::TabPage,
    container: gtk::Box,
    waiting: adw::StatusPage,
    viewer: Option<Rc<Viewer>>,
    peer: String,
}

pub(crate) struct SessionTabs {
    pub view: adw::TabView,
    pub selected: Arc<AtomicU64>,
    home: adw::TabPage,
    window: adw::ApplicationWindow,
    core: Arc<Core>,
    entries: RefCell<BTreeMap<u64, SessionTab>>,
}
impl SessionTabs {
    pub(crate) fn install(window: &adw::ApplicationWindow, core: Arc<Core>) -> Rc<Self> {
        let content = window.content().expect("main window content");
        window.set_content(None::<&gtk::Widget>);
        hide_window_controls(&content);
        let view = adw::TabView::new();
        view.set_vexpand(true);
        let home = view.append(&content);
        home.set_title("Home");
        view.set_page_pinned(&home, true);
        home.set_icon(Some(&gtk::gio::ThemedIcon::new("go-home-symbolic")));
        let bar = adw::TabBar::new();
        bar.set_view(Some(&view));
        bar.set_autohide(false);
        let outer = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::WindowTitle::new("VniLink", "")));
        outer.add_top_bar(&header);
        outer.add_top_bar(&bar);
        outer.set_content(Some(&view));
        window.set_content(Some(&outer));
        let tabs = Rc::new(Self {
            view,
            home,
            window: window.clone(),
            core,
            selected: Arc::new(AtomicU64::new(0)),
            entries: RefCell::new(BTreeMap::new()),
        });
        let weak = Rc::downgrade(&tabs);
        tabs.view.connect_close_page(move |view, page| {
            let Some(tabs) = weak.upgrade() else {
                return Propagation::Proceed;
            };
            if *page == tabs.home {
                view.close_page_finish(page, false);
                return Propagation::Stop;
            }
            let id = tabs
                .entries
                .borrow()
                .iter()
                .find_map(|(id, tab)| (tab.page == *page).then_some(*id));
            if let Some(id) = id {
                if let Some(tab) = tabs.entries.borrow_mut().remove(&id) {
                    if let Some(viewer) = tab.viewer {
                        viewer.retire();
                    }
                }
                // The closed tab owns Desktop only; Files/Terminal keep the parent.
            }
            view.close_page_finish(page, true);
            Propagation::Stop
        });
        let weak = Rc::downgrade(&tabs);
        tabs.view.connect_selected_page_notify(move |_| {
            if let Some(tabs) = weak.upgrade() {
                let selected = tabs.view.selected_page();
                let id = tabs
                    .entries
                    .borrow()
                    .iter()
                    .find_map(|(id, t)| (Some(&t.page) == selected.as_ref()).then_some(*id))
                    .unwrap_or(0);
                let previous = tabs.selected.swap(id, Ordering::Relaxed);
                if previous != id {
                    if let Some(viewer) = tabs
                        .entries
                        .borrow()
                        .get(&previous)
                        .and_then(|t| t.viewer.as_ref())
                    {
                        viewer.deactivate();
                    }
                }
            }
        });
        let weak = Rc::downgrade(&tabs);
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(tabs) = weak.upgrade() else {
                return ControlFlow::Break;
            };
            if tabs.core.controller_sessions().is_err() {
                return ControlFlow::Break;
            }
            tabs.refresh();
            ControlFlow::Continue
        });
        let shortcut = gtk::ShortcutController::new();
        shortcut.set_scope(gtk::ShortcutScope::Managed);
        shortcut.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&tabs);
        shortcut.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>w"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(tabs) = weak.upgrade() {
                    if let Some(page) = tabs.view.selected_page().filter(|p| *p != tabs.home) {
                        tabs.view.close_page(&page);
                    }
                }
                Propagation::Stop
            })),
        ));
        window.add_controller(shortcut);
        tabs
    }

    pub(crate) fn connect(self: &Rc<Self>, peer: &str) -> Result<(), vnilink::Error> {
        let existing = self
            .entries
            .borrow()
            .iter()
            .find(|(_, t)| t.peer == peer)
            .map(|(id, t)| (*id, t.page.clone()));
        if let Some((id, page)) = existing {
            if self
                .entries
                .borrow()
                .get(&id)
                .and_then(|t| t.viewer.as_ref())
                .is_some_and(|v| v.is_retired())
                || self.core.controller_session(id).is_ok_and(|s| {
                    matches!(s.phase, SessionPhase::Closed | SessionPhase::Failed(_))
                })
            {
                self.view.close_page(&page);
            } else {
                self.view.set_selected_page(&page);
                return Ok(());
            }
        }
        let session = self.core.begin_connect_services(peer)?;
        self.add(session);
        Ok(())
    }
    pub(crate) fn add(&self, session: SessionInfo) {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let waiting = adw::StatusPage::builder()
            .title("Connecting…")
            .vexpand(true)
            .build();
        container.append(&waiting);
        let page = self.view.append(&container);
        page.set_title(&self.name(&session.peer_id));
        page.set_loading(true);
        self.entries.borrow_mut().insert(
            session.id,
            SessionTab {
                page: page.clone(),
                container,
                waiting,
                viewer: None,
                peer: session.peer_id,
            },
        );
        self.view.set_selected_page(&page);
    }
    fn name(&self, peer: &str) -> String {
        self.core
            .snapshot()
            .ok()
            .and_then(|s| s.devices.into_iter().find(|d| d.endpoint_id == peer))
            .map(|d| crate::presentation::device_name(&d))
            .unwrap_or_else(|| crate::presentation::computer_name(peer))
    }
    pub(crate) fn refresh(&self) {
        for (id, tab) in self.entries.borrow_mut().iter_mut() {
            let Ok(session) = self.core.controller_session(*id) else {
                continue;
            };
            tab.page.set_title(&self.name(&session.peer_id));
            tab.page.set_loading(matches!(
                session.phase,
                SessionPhase::Connecting
                    | SessionPhase::Authenticated
                    | SessionPhase::Authorized
                    | SessionPhase::Negotiating
                    | SessionPhase::Reconnecting
            ));
            tab.page.set_tooltip(&phase_label(&session.phase));
            let same = tab.viewer.as_ref().is_some_and(|v| {
                v.session.generation == session.generation && session.phase == SessionPhase::Active
            });
            if !same {
                if let Some(viewer) = tab.viewer.take() {
                    viewer.retire();
                    tab.container.remove(&viewer.content);
                    tab.container.append(&tab.waiting);
                }
                if session.phase == SessionPhase::Active {
                    let viewer = Viewer::build(&self.window, self.core.clone(), session.clone());
                    tab.container.remove(&tab.waiting);
                    tab.container.append(&viewer.content);
                    viewer.start(self.core.clone());
                    tab.viewer = Some(viewer);
                } else {
                    tab.waiting.set_title(&phase_label(&session.phase));
                    if matches!(
                        session.phase,
                        SessionPhase::Connecting | SessionPhase::Authenticated
                    ) {
                        tab.waiting.set_description(Some(&format!(
                            "If approval is requested, enter {} on {}.",
                            self.core.control_check(),
                            self.name(&session.peer_id)
                        )));
                    } else {
                        tab.waiting.set_description(None);
                    }
                }
            }
        }
    }
}
fn phase_label(phase: &SessionPhase) -> String {
    match phase {
        SessionPhase::Connecting => "Connecting…".into(),
        SessionPhase::Authenticated | SessionPhase::Authorized | SessionPhase::Negotiating => {
            "Waiting for approval…".into()
        }
        SessionPhase::Active => "Connected".into(),
        SessionPhase::Reconnecting => "Reconnecting…".into(),
        SessionPhase::Stopping => "Disconnecting…".into(),
        SessionPhase::Closed => "Disconnected".into(),
        SessionPhase::Failed(message) => message.clone(),
    }
}
fn hide_window_controls(widget: &impl IsA<gtk::Widget>) {
    if let Some(header) = widget.as_ref().downcast_ref::<adw::HeaderBar>() {
        header.set_show_start_title_buttons(false);
        header.set_show_end_title_buttons(false);
    }
    let mut child = widget.as_ref().first_child();
    while let Some(current) = child {
        hide_window_controls(&current);
        child = current.next_sibling();
    }
}

#[cfg(test)]
impl SessionTabs {
    pub(crate) fn page(&self, id: u64) -> adw::TabPage {
        self.entries.borrow().get(&id).unwrap().page.clone()
    }
    pub(crate) fn viewer(&self, id: u64) -> Option<Rc<Viewer>> {
        self.entries
            .borrow()
            .get(&id)
            .and_then(|t| t.viewer.clone())
    }
}
