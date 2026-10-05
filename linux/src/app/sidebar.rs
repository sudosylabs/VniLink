//! The paired-computer list. Owns the rows and the selection, not the pages.

use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use vnilink::Snapshot;

use crate::devices;

use super::widgets::{self, peer_id};

/// Selection callback after the row changes: the window clears any open
/// file and repaints the stored snapshot.
pub(crate) struct Sidebar {
    rows: gtk::ListBox,
    sidebar_stack: gtk::Stack,
    root: adw::NavigationSplitView,
    empty_hint: gtk::Box,
    selected: Rc<RefCell<Option<String>>>,
    painting: Rc<Cell<bool>>,
    row_data: RefCell<Vec<devices::DeviceRow>>,
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
            empty_hint: widgets::widget(builder, "empty"),
            selected: Rc::new(RefCell::new(None)),
            painting,
            row_data: RefCell::new(Vec::new()),
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
            let next = peer_id(row);
            if *sidebar.selected.borrow() == next {
                return;
            }
            *sidebar.selected.borrow_mut() = next;
            selected();
        });
        let weak = Rc::downgrade(&sidebar);
        rows.connect_row_activated(move |_, _| {
            on_select();
            if let Some(sidebar) = weak.upgrade() {
                sidebar.root.set_show_content(true);
            }
        });
        for id in ["open_file", "open_file_detail"] {
            let open_file: gtk::Button = widgets::widget(builder, id);
            let on_open_file = on_open_file.clone();
            open_file.connect_clicked(move |_| on_open_file());
        }
        sidebar.rows.set_header_func(|row, before| {
            let paired = row.widget_name().starts_with("peer-")
                && before.is_none_or(|previous| previous.widget_name() == "self");
            if !paired {
                row.set_header(None::<&gtk::Widget>);
                return;
            }
            let label = gtk::Label::builder()
                .label("Your computers")
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

    /// Keep existing widgets when only the main page changes. Recreating
    /// focused rows makes GTK select them again after the paint guard ends.
    pub(crate) fn rebuild(&self, snapshot: &Snapshot) {
        let data = devices::rows(&snapshot.devices, snapshot.hosting, |_| "Paired");
        if *self.row_data.borrow() != data {
            while let Some(child) = self.rows.first_child() {
                self.rows.remove(&child);
            }
            for row in &data {
                let widget = if row.name == "self" {
                    widgets::self_row(row)
                } else {
                    widgets::peer_row(row)
                };
                self.rows.append(&widget);
            }
            self.row_data.replace(data);
        }
        let wanted = devices::retain_selection(&self.selected.borrow(), &snapshot.devices);
        *self.selected.borrow_mut() = wanted.clone();
        let name = wanted
            .map(|id| format!("peer-{id}"))
            .unwrap_or_else(|| "self".into());
        let mut index = 0;
        while let Some(row) = self.rows.row_at_index(index) {
            if row.widget_name() == name {
                self.rows.select_row(Some(&row));
                break;
            }
            index += 1;
        }
        self.empty_hint.set_visible(snapshot.devices.is_empty());
    }

    pub(crate) fn stack(&self) -> gtk::Stack {
        self.sidebar_stack.clone()
    }

    pub(crate) fn rows(&self) -> gtk::ListBox {
        self.rows.clone()
    }

    pub(crate) fn empty_visible(&self) -> bool {
        self.empty_hint.get_visible()
    }
}
