//! The window people keep open. Composes the sidebar and the pages.
//!
//! This computer creates a pairing file and shows three digits. The sidebar
//! is the other direction: open that file, type the digits, then connect.
//! Connect shows a second set of digits for the first computer to type.
//! A wide window shows both sides. A narrow window shows one at a time.

use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use vnilink::Snapshot;

use crate::events::LiveState;
use crate::pairing::PairingFlow;
use crate::presentation::{self, role_name};

use super::pages::PageActions;
use super::pages::Pages;
use super::sidebar::Sidebar;
use super::widgets;

pub(crate) struct Actions {
    pub hosting: Rc<dyn Fn(bool)>,
    pub stop: Rc<dyn Fn()>,
    pub create_file: Rc<dyn Fn()>,
    pub open_file: Rc<dyn Fn()>,
    pub save_computer: Rc<dyn Fn(String)>,
    pub cancel_file: Rc<dyn Fn()>,
    pub connect: Rc<dyn Fn(String)>,
    pub allow: Rc<dyn Fn(String)>,
    pub refuse: Rc<dyn Fn()>,
}

impl Actions {
    fn pages(&self) -> PageActions {
        PageActions {
            hosting: Rc::clone(&self.hosting),
            create_file: Rc::clone(&self.create_file),
            save_computer: Rc::clone(&self.save_computer),
            cancel_file: Rc::clone(&self.cancel_file),
            connect: Rc::clone(&self.connect),
            allow: Rc::clone(&self.allow),
            refuse: Rc::clone(&self.refuse),
        }
    }
}

pub(crate) struct Home {
    /// Owns the builder window so dropping the builder does not destroy the pages.
    #[allow(dead_code)]
    window: adw::ApplicationWindow,
    pub root: adw::NavigationSplitView,
    banner: adw::Banner,
    sidebar: Rc<Sidebar>,
    pages: Pages,
    painting: Rc<Cell<bool>>,
    snapshot: Rc<RefCell<Option<Snapshot>>>,
    flow: Rc<RefCell<PairingFlow>>,
    live: Rc<RefCell<LiveState>>,
    paint_key: RefCell<Option<String>>,
    actions: Actions,
}

impl Home {
    pub(crate) fn from_builder(
        builder: &gtk::Builder,
        actions: Actions,
        flow: Rc<RefCell<PairingFlow>>,
        live: Rc<RefCell<LiveState>>,
    ) -> Rc<Self> {
        let painting = Rc::new(Cell::new(false));
        let weak_flow = Rc::clone(&flow);
        let home: Rc<Self> = Rc::new_cyclic(|weak| {
            let on_select = {
                let weak: std::rc::Weak<Self> = weak.clone();
                let weak_flow = Rc::clone(&weak_flow);
                Rc::new(move || {
                    weak_flow.borrow_mut().select_computer();
                    if let Some(home) = weak.upgrade() {
                        home.render_stored();
                    }
                })
            };
            let sidebar = Sidebar::from_builder(
                builder,
                Rc::clone(&painting),
                on_select,
                Rc::new({
                    let weak = weak.clone();
                    move || {
                        if let Some(home) = weak.upgrade() {
                            (home.actions.open_file)();
                        }
                    }
                }),
            );
            Self {
                window: widgets::widget(builder, "window"),
                root: widgets::widget(builder, "split"),
                banner: widgets::widget(builder, "banner"),
                sidebar,
                pages: Pages::from_builder(builder, Rc::clone(&painting)),
                painting,
                snapshot: Rc::new(RefCell::new(None)),
                flow: weak_flow,
                live,
                paint_key: RefCell::new(None),
                actions,
            }
        });
        let split: adw::NavigationSplitView = widgets::widget(&builder, "split");
        let breakpoint = adw::Breakpoint::new(
            adw::BreakpointCondition::parse("max-width: 700sp").expect("breakpoint"),
        );
        breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
        home.window.add_breakpoint(breakpoint);
        // Keep both titlebar actions available whichever narrow pane is visible.
        for id in ["settings_sidebar", "open_file_detail"] {
            let button: gtk::Button = widgets::widget(builder, id);
            split
                .bind_property("collapsed", &button, "visible")
                .sync_create()
                .build();
        }

        home.banner.set_use_markup(false);
        let weak = Rc::downgrade(&home);
        home.banner.connect_button_clicked(move |_| {
            if let Some(home) = weak.upgrade() {
                (home.actions.stop)();
            }
        });
        home
    }

    pub(crate) fn show_offer(&self, check: &str) {
        self.sidebar.select(None);
        self.root.set_show_content(true);
        self.flow.borrow_mut().offer(check.to_string());
        self.refresh();
    }

    pub(crate) fn show_file(&self, endpoint_id: &str, check: &str) {
        self.root.set_show_content(true);
        self.flow
            .borrow_mut()
            .open_file(endpoint_id.to_string(), check.to_string());
        self.refresh();
    }

    pub(crate) fn clear_file(&self) {
        self.flow.borrow_mut().clear_file();
        self.refresh();
    }

    pub(crate) fn set_request(&self, request: Option<vnilink::ControlRequest>) {
        self.live.borrow_mut().set_request(request);
    }

    pub(crate) fn set_waiting(&self, digits: Option<String>) {
        match digits {
            Some(check) => self.flow.borrow_mut().wait(check),
            None => self.flow.borrow_mut().clear_waiting(),
        }
    }

    pub(crate) fn set_connecting(&self, connecting: bool) {
        self.live.borrow_mut().set_connecting(connecting);
        self.refresh();
    }

    pub(crate) fn select(&self, endpoint_id: Option<String>) {
        self.sidebar.select(endpoint_id);
        self.paint_key.borrow_mut().take();
    }

    pub(crate) fn show(&self, snapshot: &Snapshot) {
        self.snapshot.replace(Some(snapshot.clone()));
        if self.live.borrow().request().is_some() {
            self.sidebar.select(None);
        }
        let selected = self.sidebar.selected();
        if selected.as_ref().is_some_and(|id| {
            !snapshot
                .devices
                .iter()
                .any(|device| &device.endpoint_id == id)
        }) {
            self.sidebar.select(None);
        }
        let key = self.key(snapshot);
        if self.paint_key.borrow().as_ref() == Some(&key) {
            return;
        }
        if self.live.borrow().request().is_some() {
            self.root.set_show_content(true);
        }
        self.painting.set(true);
        self.sidebar_stack_of(snapshot);
        self.sidebar.rebuild(snapshot);
        self.render_pages(snapshot);
        self.paint_key.replace(Some(key));
        self.painting.set(false);
    }

    fn sidebar_stack_of(&self, _snapshot: &Snapshot) {
        self.stack().set_visible_child_name("list");
    }

    fn refresh(&self) {
        self.paint_key.borrow_mut().take();
        let snapshot = self.snapshot.borrow().as_ref().cloned();
        if let Some(snapshot) = snapshot {
            self.show(&snapshot);
        }
    }

    fn key(&self, snapshot: &Snapshot) -> String {
        let devices = snapshot
            .devices
            .iter()
            .map(|device| {
                format!(
                    "{}:{:?}:{:?}:{}{}{}",
                    device.endpoint_id,
                    device.hostname,
                    device.pet_name,
                    u8::from(device.grants.control),
                    u8::from(device.grants.clipboard),
                    u8::from(device.grants.transfer)
                )
            })
            .collect::<Vec<_>>()
            .join("|");
        let session = |info: &Option<vnilink::SessionInfo>| {
            info.as_ref()
                .map(|info| {
                    format!(
                        "{}:{}:{}",
                        info.peer_id,
                        role_name(info.role),
                        presentation::phase_name(&info.phase)
                    )
                })
                .unwrap_or_default()
        };
        let flow = self.flow.borrow();
        let live = self.live.borrow();
        format!(
            "{}\n{:?}\n{:?}\n{:?}\n{}\n{}\n{}\n{}\n{}\n{}\n{}{}{}",
            self.sidebar.selected().unwrap_or_default(),
            flow.offer_check(),
            live.request().as_ref().map(|ask| ask.check.clone()),
            flow.file(),
            flow.waiting().unwrap_or_default(),
            live.connecting(),
            snapshot.hosting,
            devices,
            session(&snapshot.incoming),
            session(&snapshot.outgoing),
            u8::from(snapshot.capabilities.desktop_capture),
            u8::from(snapshot.capabilities.h264_encode),
            u8::from(snapshot.capabilities.h264_decode),
        )
    }

    fn render_stored(&self) {
        let snapshot = self.snapshot.borrow().as_ref().cloned();
        if let Some(snapshot) = snapshot {
            self.painting.set(true);
            self.render_pages(&snapshot);
            self.paint_key.replace(Some(self.key(&snapshot)));
            self.painting.set(false);
        }
    }

    fn render_pages(&self, snapshot: &Snapshot) {
        if let Some(session) = snapshot
            .incoming
            .as_ref()
            .filter(|session| {
                !matches!(
                    session.phase,
                    vnilink::SessionPhase::Closed | vnilink::SessionPhase::Failed(_)
                )
            })
            .or_else(|| {
                snapshot.outgoing.as_ref().filter(|session| {
                    !matches!(
                        session.phase,
                        vnilink::SessionPhase::Closed | vnilink::SessionPhase::Failed(_)
                    )
                })
            })
        {
            let title = snapshot
                .devices
                .iter()
                .find(|d| d.endpoint_id == session.peer_id)
                .map(presentation::device_name);
            self.banner.set_title(
                &title
                    .map(|name| presentation::session_banner_named(session, &name))
                    .unwrap_or_else(|| presentation::session_banner(session)),
            );
            self.banner.set_revealed(true);
        } else {
            self.banner.set_revealed(false);
        }
        let chosen = self.sidebar.selected().and_then(|id| {
            snapshot
                .devices
                .iter()
                .find(|device| device.endpoint_id == id)
                .cloned()
        });
        let flow = self.flow.borrow();
        let live = self.live.borrow();
        let actions = self.actions.pages();
        self.pages.render(snapshot, &flow, &live, chosen, &actions);
    }

    pub(crate) fn window(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    pub(crate) fn banner(&self) -> &adw::Banner {
        &self.banner
    }

    pub(crate) fn stack(&self) -> gtk::Stack {
        self.sidebar.stack()
    }

    pub(crate) fn rows(&self) -> gtk::ListBox {
        self.sidebar.rows()
    }

    pub(crate) fn empty_visible(&self) -> bool {
        self.sidebar.empty_visible()
    }

    pub(crate) fn detail(&self) -> &gtk::Box {
        self.pages.detail()
    }

    pub(crate) fn title(&self) -> &adw::WindowTitle {
        self.pages.title()
    }
}
