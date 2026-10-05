//! The paired-computer list. Owns the rows and the selection, not the pages.

use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use vnilink::Snapshot;

use crate::devices;
use crate::presentation::peer_status;

use super::widgets::{self, peer_id};

/// Selection callback after the row changes: the window clears any open
/// file and repaints the stored snapshot.
pub(crate) struct Sidebar {
    rows: gtk::ListBox,
    sidebar_stack: gtk::Stack,
    root: adw::NavigationSplitView,
    empty_action: gtk::Button,
    selected: Rc<RefCell<Option<String>>>,
    painting: Rc<Cell<bool>>,
}

impl Sidebar {
    pub(crate) fn from_builder(
        builder: &gtk::Builder,
        painting: Rc<Cell<bool>>,
        on_select: Rc<dyn Fn()>,
        on_open_file: Rc<dyn Fn()>,
    ) -> Rc<Self> {
        let sidebar = Rc::new(Self {
            rows: widgets::widget(builder, "rows"),
            sidebar_stack: widgets::widget(builder, "sidebar_stack"),
            root: widgets::widget(builder, "split"),
            empty_action: widgets::widget(builder, "show_computer"),
            selected: Rc::new(RefCell::new(None)),
            painting,
        });
        let rows = sidebar.rows.clone();
        let weak = Rc::downgrade(&sidebar);
        let selected = Rc::clone(&on_select);
        rows.connect_row_selected(move |_, row| {
            let Some(sidebar) = weak.upgrade() else {
                return;
            };
            if sidebar.painting.get() {
                return;
            }
            *sidebar.selected.borrow_mut() = peer_id(row);
            selected();
        });
        let weak = Rc::downgrade(&sidebar);
        rows.connect_row_activated(move |_, _| {
            if let Some(sidebar) = weak.upgrade() {
                sidebar.root.set_show_content(true);
            }
        });
        let weak = Rc::downgrade(&sidebar);
        let on_empty = Rc::clone(&on_select);
        sidebar.empty_action.connect_clicked(move |_| {
            if let Some(sidebar) = weak.upgrade() {
                *sidebar.selected.borrow_mut() = None;
                on_empty();
                sidebar.root.set_show_content(true);
            }
        });
        let weak = Rc::downgrade(&sidebar);
        sidebar.root.connect_collapsed_notify(move |_| {
            if let Some(sidebar) = weak.upgrade() {
                sidebar.update_empty_action();
            }
        });
        let open_file: gtk::Button = widgets::widget(builder, "open_file");
        let open_header: gtk::Button = widgets::widget(builder, "open_file_header");
        let open = Rc::clone(&on_open_file);
        open_file.connect_clicked(move |_| open());
        let open = Rc::clone(&on_open_file);
        open_header.connect_clicked(move |_| open());
        sidebar.rows.set_header_func(|row, before| {
            let paired = row.widget_name().starts_with("peer-")
                && before.is_none_or(|previous| previous.widget_name() == "self");
            if !paired {
                row.set_header(None::<&gtk::Widget>);
                return;
            }
            let label = gtk::Label::builder()
                .label("Paired")
                .xalign(0.0)
                .margin_top(18)
                .margin_bottom(6)
                .margin_start(14)
                .margin_end(14)
                .build();
            label.add_css_class("caption-heading");
            row.set_header(Some(&label));
        });
        sidebar
    }

    pub(crate) fn selected(&self) -> Option<String> {
        self.selected.borrow().clone()
    }

    pub(crate) fn select(&self, endpoint_id: Option<String>) {
        *self.selected.borrow_mut() = endpoint_id;
    }

    /// Rebuilds rows from the snapshot, keeping the selection when its
    /// computer is still paired. The empty/list switch is owned by the
    /// window, which knows the full snapshot before delegating here.
    pub(crate) fn rebuild(&self, snapshot: &Snapshot) {
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        let wanted = crate::devices::retain_selection(&self.selected.borrow(), &snapshot.devices);
        *self.selected.borrow_mut() = wanted.clone();
        let mut matched = None;
        for row in devices::rows(&snapshot.devices, snapshot.hosting, peer_status) {
            let widget = if row.name == "self" {
                widgets::self_row(&row)
            } else {
                widgets::peer_row(&row)
            };
            let is_match = match &wanted {
                None => row.name == "self",
                Some(id) => row.name == format!("peer-{id}"),
            };
            let list_row: gtk::ListBoxRow = widget.upcast();
            self.rows.append(&list_row);
            if is_match {
                matched = Some(list_row);
            }
        }
        if let Some(row) = &matched {
            self.rows.select_row(Some(row));
        }
        self.update_empty_action();
    }

    pub(crate) fn stack(&self) -> gtk::Stack {
        self.sidebar_stack.clone()
    }

    pub(crate) fn rows(&self) -> gtk::ListBox {
        self.rows.clone()
    }

    pub(crate) fn empty_visible(&self) -> bool {
        self.empty_action.is_visible()
    }

    fn update_empty_action(&self) {
        let empty = self.sidebar_stack.visible_child_name().as_deref() == Some("empty");
        self.empty_action
            .set_visible(empty && self.root.is_collapsed());
    }
}
