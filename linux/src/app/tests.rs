//! Window tests. GTK must run on the process main thread, so this repeats
//! itself in a fresh process like before.

use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib::object::IsA;
use vnilink::{
    Capabilities, ControlRequest, DeviceInfo, Grants, SessionInfo, SessionPhase, SessionRole,
    Snapshot,
};

use super::widgets::{click, named};
use super::window::{Actions, Home};
use crate::events::LiveState;
use crate::pairing::PairingFlow;

fn open_home(actions: Actions) -> Rc<Home> {
    use std::sync::Once;
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        gtk::init().expect("gtk");
    });
    let builder = gtk::Builder::from_string(include_str!("window.ui"));
    let root: gtk::Stack = super::widgets::widget(&builder, "root");
    assert!(root.child_by_name("startup").is_some());
    assert!(root.child_by_name("home").is_some());
    assert!(root.child_by_name("failure").is_some());
    Home::from_builder(
        &builder,
        actions,
        Rc::new(RefCell::new(PairingFlow::default())),
        Rc::new(RefCell::new(LiveState::default())),
    )
}

fn quiet() -> Actions {
    Actions {
        hosting: Rc::new(|_| {}),
        stop: Rc::new(|| {}),
        create_file: Rc::new(|| {}),
        open_file: Rc::new(|| {}),
        save_computer: Rc::new(|_| {}),
        cancel_file: Rc::new(|| {}),
        connect: Rc::new(|_| {}),
        allow: Rc::new(|_| {}),
        refuse: Rc::new(|| {}),
    }
}

fn snapshot(devices: Vec<DeviceInfo>, hosting: bool) -> Snapshot {
    Snapshot {
        endpoint_id: "this-computer".to_string(),
        public_relay: true,
        relay_urls: Vec::new(),
        hosting,
        devices,
        incoming: None,
        outgoing: None,
        capabilities: Capabilities::UNSUPPORTED,
    }
}

fn peer(grants: Grants) -> DeviceInfo {
    DeviceInfo {
        hostname: None,
        pet_name: None,
        endpoint_id: "other-computer".to_string(),
        relays: Vec::new(),
        direct: Vec::new(),
        grants,
    }
}

fn labels(widget: &impl IsA<gtk::Widget>) -> Vec<String> {
    let mut found = Vec::new();
    fn walk(widget: &gtk::Widget, found: &mut Vec<String>) {
        if let Some(label) = widget.downcast_ref::<gtk::Label>() {
            found.push(label.label().to_string());
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            walk(&widget, found);
            child = widget.next_sibling();
        }
    }
    walk(widget.as_ref(), &mut found);
    found
}

/// GTK has to run on the process main thread. The test harness puts other
/// tests on worker threads, so this one repeats itself in a fresh process.
#[test]
fn the_window_matches_the_computer_and_the_paired_list() {
    if std::env::var_os("VNILINK_GTK_CHILD").is_none() {
        let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "app::tests::the_window_matches_the_computer_and_the_paired_list",
                "--test-threads=1",
            ])
            .env("VNILINK_GTK_CHILD", "1")
            .env("GTK_IM_MODULE", "gtk-im-context-simple")
            .status()
            .expect("respawn gtk test");
        assert!(status.success());
        return;
    }
    an_unpaired_computer_shows_the_empty_state_and_this_computer();
    choosing_a_paired_computer_offers_connect();
    a_pairing_file_asks_for_the_digits_on_the_other_screen();
    a_control_request_asks_for_the_digits_on_the_other_screen();
    an_active_session_is_a_banner_with_stop();
    a_removed_computer_returns_to_this_computer();
}

fn an_unpaired_computer_shows_the_empty_state_and_this_computer() {
    let calls = Rc::new(Cell::new(0));
    let created = Rc::new(Cell::new(0));
    let mut actions = quiet();
    actions.hosting = Rc::new({
        let calls = Rc::clone(&calls);
        move |_| calls.set(calls.get() + 1)
    });
    actions.create_file = Rc::new({
        let created = Rc::clone(&created);
        move || created.set(created.get() + 1)
    });
    let home = open_home(actions);
    home.show(&snapshot(Vec::new(), true));
    assert_eq!(home.stack().visible_child_name().as_deref(), Some("empty"));
    assert_eq!(home.title().title().as_str(), "This computer");
    assert!(!home.banner().is_revealed());
    assert!(!home.empty_visible());
    assert_eq!(calls.get(), 0);
    let text = labels(home.window());
    assert!(text.iter().any(|label| label == "Open a pairing file"));
    assert!(text.iter().any(|label| label == "Reach another computer"));
    assert!(!text
        .iter()
        .any(|label| label.contains("Pairing starts here")));
    let page = labels(home.detail());
    assert!(page.iter().any(|label| label == "this-computer"));
    assert!(page.iter().any(|label| label == "Allow control"));
    assert!(page
        .iter()
        .any(|label| label == "Let another computer control this one"));
    assert!(page.iter().any(|label| label == "Create a pairing file"));
    click(home.detail(), "create_file");
    assert_eq!(created.get(), 1);
    home.show_offer("042");
    let page = labels(home.detail());
    assert!(page.iter().any(|label| label == "042"));
}

fn choosing_a_paired_computer_offers_connect() {
    let connected = Rc::new(RefCell::new(None));
    let mut actions = quiet();
    actions.connect = Rc::new({
        let connected = Rc::clone(&connected);
        move |id| *connected.borrow_mut() = Some(id)
    });
    let home = open_home(actions);
    let grants = Grants {
        control: true,
        clipboard: false,
        transfer: false,
    };
    home.show(&snapshot(vec![peer(grants)], false));
    assert_eq!(home.stack().visible_child_name().as_deref(), Some("list"));
    let row = home.rows().row_at_index(1).expect("peer row");
    assert_eq!(row.widget_name().as_str(), "peer-other-computer");
    home.rows().select_row(Some(&row));
    assert_eq!(home.title().title().as_str(), "other-computer");
    let text = labels(home.detail());
    assert!(text.iter().any(|label| label == "Connect"));
    assert!(text.iter().any(|label| {
        label == "Control is allowed. Connect when that computer is on. The clipboard and files stay off."
    }));
    click(home.detail(), "connect");
    assert_eq!(connected.borrow().as_deref(), Some("other-computer"));
}

fn a_pairing_file_asks_for_the_digits_on_the_other_screen() {
    let saved = Rc::new(RefCell::new(None));
    let mut actions = quiet();
    actions.save_computer = Rc::new({
        let saved = Rc::clone(&saved);
        move |digits| *saved.borrow_mut() = Some(digits)
    });
    let home = open_home(actions);
    home.show(&snapshot(Vec::new(), false));
    home.show_file("other-computer", "042");
    assert_eq!(home.title().title().as_str(), "Pairing file");
    let text = labels(home.detail());
    assert!(text.iter().any(|label| label == "042"));
    assert!(text.iter().any(|label| label == "Check the file"));
    let entry = named::<gtk::Entry>(home.detail(), "digits_entry");
    entry.set_text("042");
    click(home.detail(), "save_computer");
    assert_eq!(saved.borrow().as_deref(), Some("042"));
}

fn a_control_request_asks_for_the_digits_on_the_other_screen() {
    let allowed = Rc::new(RefCell::new(None));
    let mut actions = quiet();
    actions.allow = Rc::new({
        let allowed = Rc::clone(&allowed);
        move |digits| *allowed.borrow_mut() = Some(digits)
    });
    let home = open_home(actions);
    home.show(&snapshot(vec![peer(Grants::default())], true));
    home.rows().select_row(home.rows().row_at_index(1).as_ref());
    home.set_request(Some(ControlRequest {
        endpoint_id: "other-computer".to_string(),
        check: "847".to_string(),
    }));
    home.show(&snapshot(vec![peer(Grants::default())], true));
    assert_eq!(home.title().title().as_str(), "This computer");
    let text = labels(home.detail());
    assert!(text.iter().any(|label| label == "847"));
    assert!(text
        .iter()
        .any(|label| label.contains("wants to control this desktop")));
    let entry = named::<gtk::Entry>(home.detail(), "digits_entry");
    entry.set_text("847");
    click(home.detail(), "allow_control");
    assert_eq!(allowed.borrow().as_deref(), Some("847"));
}

fn an_active_session_is_a_banner_with_stop() {
    let home = open_home(quiet());
    let mut picture = snapshot(Vec::new(), true);
    picture.incoming = Some(SessionInfo {
        id: 7,
        generation: 1,
        role: SessionRole::Host,
        phase: SessionPhase::Active,
        peer_id: "abcdefghijklmnopqrstuvwxyz012345".to_string(),
        quality: None,
        monitor_width: 0,
        monitor_height: 0,
    });
    home.show(&picture);
    assert!(home.banner().is_revealed());
    assert_eq!(
        home.banner().title().as_str(),
        "abcdefgh…012345 is connected to this desktop"
    );
    assert_eq!(home.banner().button_label().as_deref(), Some("Stop"));
}

fn a_removed_computer_returns_to_this_computer() {
    let home = open_home(quiet());
    home.show(&snapshot(vec![peer(Grants::default())], false));
    home.rows().select_row(home.rows().row_at_index(1).as_ref());
    home.show(&snapshot(Vec::new(), false));
    assert_eq!(home.title().title().as_str(), "This computer");
    assert_eq!(home.stack().visible_child_name().as_deref(), Some("empty"));
}
