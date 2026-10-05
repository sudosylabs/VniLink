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
        adw::init().expect("libadwaita");
        super::install_style();
    });
    let builder = gtk::Builder::from_string(include_str!("window.ui"));
    let root: gtk::Stack = super::widgets::widget(&builder, "root");
    assert!(root.child_by_name("startup").is_some());
    assert!(root.child_by_name("home").is_some());
    assert!(root.child_by_name("failure").is_some());
    root.set_visible_child_name("home");
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
        capabilities: Capabilities {
            h264_decode: true,
            ..Capabilities::UNSUPPORTED
        },
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
    a_missing_decoder_disables_connect();
    computer_names_refresh_without_changing_selection();
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
    assert_eq!(home.stack().visible_child_name().as_deref(), Some("list"));
    assert_eq!(home.title().title().as_str(), "This computer");
    assert!(!home.banner().is_revealed());
    assert!(home.empty_visible());
    assert_eq!(calls.get(), 0);
    let text = labels(home.window());
    let add = named::<gtk::Button>(home.window(), "open_file");
    assert_eq!(add.tooltip_text().as_deref(), Some("Add computer"));
    assert!(add.ancestor(adw::HeaderBar::static_type()).is_some());
    assert!(named::<gtk::Button>(home.window(), "settings")
        .ancestor(adw::HeaderBar::static_type())
        .is_some());
    assert!(text.iter().any(|label| label == "VniLink"));
    assert!(!text
        .iter()
        .any(|label| label.contains("Pairing starts here")));
    let page = labels(home.detail());
    assert!(page.iter().any(|label| label == "this-computer"));
    assert!(page.iter().any(|label| label == "Allow connections"));
    assert!(page
        .iter()
        .any(|label| label == "Let your other computers connect to this desktop."));
    assert!(page.iter().any(|label| label == "Pair another computer"));
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
    assert_eq!(home.title().title().as_str(), "Computer OTHER-");
    let text = labels(home.detail());
    assert!(text.iter().any(|label| label == "Connect"));
    assert!(text
        .iter()
        .any(|label| { label.contains("It may ask you to approve access.") }));
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
    assert_eq!(home.title().title().as_str(), "Add computer");
    let text = labels(home.detail());
    assert!(!text.iter().any(|label| label == "042"));
    assert!(text.iter().any(|label| label == "Add a computer"));
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
    assert_eq!(home.title().title().as_str(), "Access request");
    let text = labels(home.detail());
    assert!(!text.iter().any(|label| label == "847"));
    assert!(text
        .iter()
        .any(|label| label.contains("asking to control this desktop")));
    let entry = named::<gtk::Entry>(home.detail(), "digits_entry");
    entry.set_text("abc");
    assert!(!named::<gtk::Button>(home.detail(), "allow_control").is_sensitive());
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
    picture.incoming.as_mut().unwrap().phase = SessionPhase::Closed;
    home.show(&picture);
    assert!(!home.banner().is_revealed());
    picture.outgoing = picture.incoming.clone();
    picture.outgoing.as_mut().unwrap().role = SessionRole::Controller;
    picture.outgoing.as_mut().unwrap().phase = SessionPhase::Active;
    home.show(&picture);
    assert!(home.banner().is_revealed());
    assert!(home.banner().title().starts_with("Connected to"));
}

fn a_removed_computer_returns_to_this_computer() {
    let home = open_home(quiet());
    home.show(&snapshot(vec![peer(Grants::default())], false));
    home.rows().select_row(home.rows().row_at_index(1).as_ref());
    home.show(&snapshot(Vec::new(), false));
    assert_eq!(home.title().title().as_str(), "This computer");
    assert_eq!(home.stack().visible_child_name().as_deref(), Some("list"));
}

/// Render the actual GTK widgets and stylesheet for manual visual inspection.
#[test]
#[ignore = "requires a display and VNILINK_UI_SCREENSHOTS output directory"]
fn screenshot_tour() {
    if std::env::var_os("VNILINK_GTK_CHILD").is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "app::tests::screenshot_tour",
                "--ignored",
                "--test-threads=1",
            ])
            .env("VNILINK_GTK_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let home = open_home(quiet());
    let id = "a3d290cfa22e883f191b235917361ffaeabb872c11fbe54e4d9a12d667c882031";
    let device = DeviceInfo {
        endpoint_id: id.into(),
        ..peer(Grants::default())
    };
    let mut state = snapshot(vec![], true);
    state.endpoint_id = "bd8849156333295135190454381482920165151273221246639597e630138e49".into();
    home.show(&state);
    home.window().present();
    capture(&home, "01-this-computer");
    assert!(!named::<gtk::Button>(home.window(), "settings_sidebar").is_visible());
    assert!(!named::<gtk::Button>(home.window(), "open_file_detail").is_visible());
    home.show_offer("042");
    capture(&home, "02-pairing-code");
    home.show_file(id, "042");
    capture(&home, "03-add-computer");
    assert_eq!(
        home.title().title().as_str(),
        "Add computer",
        "pairing page must survive GTK selection events"
    );
    home.clear_file();
    state.devices.push(device);
    home.show(&state);
    home.rows().select_row(home.rows().row_at_index(1).as_ref());
    capture(&home, "04-paired-computer");
    home.show_file(id, "042");
    capture(&home, "04b-add-from-paired-list");
    assert_eq!(home.title().title().as_str(), "Add computer");
    home.clear_file();
    state.outgoing = Some(SessionInfo {
        id: 9,
        generation: 1,
        role: SessionRole::Controller,
        phase: SessionPhase::Connecting,
        peer_id: id.into(),
        quality: None,
        monitor_width: 0,
        monitor_height: 0,
    });
    home.set_waiting(Some("847".into()));
    home.show(&state);
    capture(&home, "05-waiting");
    home.set_waiting(None);
    state.outgoing = None;
    home.set_request(Some(ControlRequest {
        endpoint_id: id.into(),
        check: "847".into(),
    }));
    home.show(&state);
    capture(&home, "06-approval");
    let entry = named::<gtk::Entry>(home.detail(), "digits_entry");
    entry.set_text("847");
    capture(&home, "06b-approval-ready");
    home.window().set_default_size(360, 720);
    capture(&home, "06c-narrow-approval");
    home.set_request(None);
    home.show(&state);
    home.window().set_default_size(360, 720);
    home.root.set_show_content(true);
    capture(&home, "07-narrow-content");
    assert!(home.root.is_collapsed());
    assert!(named::<gtk::Button>(home.window(), "settings_sidebar").is_visible());
    assert!(named::<gtk::Button>(home.window(), "open_file_detail").is_visible());
    home.root.set_show_content(false);
    capture(&home, "08-narrow-sidebar");
    home.window().set_default_size(1000, 720);
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    capture(&home, "09-dark");
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceLight);
    state.incoming = Some(SessionInfo {
        id: 12,
        generation: 1,
        role: SessionRole::Host,
        phase: SessionPhase::Active,
        peer_id: id.into(),
        quality: None,
        monitor_width: 0,
        monitor_height: 0,
    });
    home.show(&state);
    capture(&home, "10-connected");
    settings_and_desktop_capture_tour(&home, &state);
    home.window().close();
}

fn capture(home: &Home, name: &str) {
    capture_widget(home.window(), name);
}

fn capture_widget(window: &impl IsA<gtk::Widget>, name: &str) {
    let window: &gtk::Widget = window.as_ref();
    let frames = Rc::new(Cell::new(0));
    let ticks = frames.clone();
    window.add_tick_callback(move |_, _| {
        ticks.set(ticks.get() + 1);
        if ticks.get() >= 12 {
            gtk::glib::ControlFlow::Break
        } else {
            gtk::glib::ControlFlow::Continue
        }
    });
    window.queue_draw();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let context = gtk::glib::MainContext::default();
    while frames.get() < 12 && std::time::Instant::now() < until {
        context.iteration(false);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(frames.get() >= 12, "window did not render");
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
    let node = snapshot.to_node().expect("render node");
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer.realize(None::<&gtk::gdk::Surface>).unwrap();
    let texture = renderer.render_texture(&node, None);
    renderer.unrealize();
    let directory = std::path::PathBuf::from(
        std::env::var_os("VNILINK_UI_SCREENSHOTS").expect("output directory"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    texture
        .save_to_png(directory.join(format!("{name}.png")))
        .unwrap();
}

fn a_missing_decoder_disables_connect() {
    let home = open_home(quiet());
    let mut state = snapshot(vec![peer(Grants::default())], false);
    state.capabilities.h264_decode = false;
    home.show(&state);
    home.rows().select_row(home.rows().row_at_index(1).as_ref());
    assert!(!named::<gtk::Button>(home.detail(), "connect").is_sensitive());
    assert!(labels(home.detail())
        .iter()
        .any(|text| text == "Desktop viewing unavailable"));
}

fn pump_until(mut ready: impl FnMut() -> bool) {
    let context = gtk::glib::MainContext::default();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !ready() && std::time::Instant::now() < until {
        context.iteration(false);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(ready(), "GTK operation did not complete");
}

fn settings_and_desktop_capture_tour(home: &Home, state: &Snapshot) {
    use std::sync::Arc;
    let core = Arc::new(
        vnilink::Core::open(
            Arc::new(vnilink::MemoryStore::empty()),
            vnilink::OpenConfig {
                listen: vnilink::Listen::Loopback,
                initial_relay: vnilink::RelaySettings::custom(vec![vnilink::CustomRelay::new(
                    "http://127.0.0.1:9",
                )
                .unwrap()])
                .unwrap(),
                ..vnilink::OpenConfig::default()
            },
        )
        .unwrap(),
    );
    let saved =
        vnilink::RelaySettings::custom(vec![vnilink::CustomRelay::new("https://relay.example/")
            .unwrap()
            .with_auth_token("fixture-token")
            .unwrap()])
        .unwrap();
    core.set_relay(saved.clone()).unwrap();
    let settings = super::preferences::shell(home.window());
    super::preferences::populate(
        &settings,
        core.clone(),
        state,
        core.relay_configuration().unwrap(),
    );
    settings.present();
    capture_widget(&settings, "11-settings-relays");
    settings.set_default_size(640, 760);
    capture_widget(&settings, "11b-settings-narrow");
    let sections = named::<gtk::ListBox>(&settings, "settings_sections");
    let pages = named::<gtk::Stack>(&settings, "settings_pages");
    let sidebar_bounds = sections.compute_bounds(&settings).unwrap();
    let page_bounds = pages.compute_bounds(&settings).unwrap();
    assert!(sidebar_bounds.x() + sidebar_bounds.width() <= page_bounds.x());
    assert!(sections.width() >= 160);
    settings.set_default_size(900, 760);
    let token = named::<adw::PasswordEntryRow>(&settings, "relay_token_0");
    assert!(token.text().is_empty(), "saved tokens must stay opaque");
    let save = named::<gtk::Button>(&settings, "save_relays");
    save.emit_clicked();
    pump_until(|| save.is_sensitive());
    assert_eq!(
        core.relay_configuration().unwrap(),
        saved,
        "saving an unchanged form must preserve the token"
    );
    token.set_text("updated-fixture-token");
    save.emit_clicked();
    pump_until(|| save.is_sensitive());
    let updated = core.relay_configuration().unwrap();
    token.set_text("");
    save.emit_clicked();
    pump_until(|| save.is_sensitive());
    assert_eq!(
        core.relay_configuration().unwrap(),
        updated,
        "empty replacement preserves the latest saved token"
    );
    named::<adw::SwitchRow>(&settings, "relay_auth_0").set_active(false);
    save.emit_clicked();
    pump_until(|| save.is_sensitive());
    match core.relay_configuration().unwrap() {
        vnilink::RelaySettings::Custom(relays) => assert!(!relays[0].has_auth_token()),
        _ => panic!("custom relay expected"),
    }
    for (page, name) in [
        ("keyboard", "12-settings-keyboard"),
        ("computers", "13-settings-computers"),
        ("files", "14-settings-files"),
        ("pairing", "15-settings-pairing"),
    ] {
        super::preferences::show_page(&settings, page);
        capture_widget(&settings, name);
    }
    let other = vnilink::Core::open(
        Arc::new(vnilink::MemoryStore::empty()),
        vnilink::OpenConfig {
            listen: vnilink::Listen::Loopback,
            initial_relay: saved.clone(),
            ..vnilink::OpenConfig::default()
        },
    )
    .unwrap();
    let invitation = other
        .create_invitation(std::time::Duration::from_secs(60))
        .unwrap();
    let device = core.accept_invitation(&invitation).unwrap();
    let row = adw::ActionRow::builder()
        .title(crate::presentation::device_name(&device))
        .build();
    let manage =
        super::preferences::manage_computer(&settings, core.clone(), device.clone(), row.clone());
    let name = named::<adw::EntryRow>(&manage, "peer_pet_name");
    let save_name = named::<gtk::Button>(&manage, "save_peer_name");
    name.set_text("My Linux laptop");
    save_name.emit_clicked();
    pump_until(|| save_name.is_sensitive());
    assert_eq!(row.title().as_str(), "My Linux laptop");
    let renamed = core
        .snapshot()
        .unwrap()
        .devices
        .into_iter()
        .find(|d| d.endpoint_id == device.endpoint_id)
        .unwrap();
    assert_eq!(renamed.pet_name.as_deref(), Some("My Linux laptop"));
    assert_eq!(renamed.grants, device.grants);
    capture_widget(&manage, "18-computer-name");
    name.set_text("");
    save_name.emit_clicked();
    pump_until(|| save_name.is_sensitive());
    assert_eq!(
        row.title().as_str(),
        crate::presentation::device_name(&device)
    );
    manage.close();
    other.shutdown();
    settings.close();
    let session = SessionInfo {
        id: 42,
        generation: 1,
        role: SessionRole::Controller,
        phase: SessionPhase::Active,
        peer_id: state.devices[0].endpoint_id.clone(),
        quality: Some(vnilink::Quality::new(8000, 30, 1920, 1080).unwrap()),
        monitor_width: 1920,
        monitor_height: 1080,
    };
    let viewer = super::viewer::Viewer::build(home.window(), core.clone(), session);
    viewer.window.present();
    capture_widget(&viewer.window, "16-desktop-waiting");
    // Synthetic pixels exercise the real texture upload; this is not a remote session.
    let mut pixels = Vec::with_capacity(960 * 540 * 4);
    for y in 0..540 {
        for x in 0..960 {
            let value = if (x / 80 + y / 80) % 2 == 0 { 56 } else { 72 };
            pixels.extend_from_slice(&[value, value, value, 0]);
        }
    }
    let frame = vnilink_media::RawFrame::bgra(960, 540, 0, pixels).unwrap();
    viewer.show_frame(&frame);
    viewer.status.set_label("Synthetic test frame · 960 × 540");
    let texture = viewer
        .picture
        .paintable()
        .unwrap()
        .downcast::<gtk::gdk::Texture>()
        .unwrap();
    let mut uploaded = vec![0; 960 * 540 * 4];
    texture.download(&mut uploaded, 960 * 4);
    assert_eq!(&uploaded[0..4], &[56, 56, 56, 255]);
    capture_widget(&viewer.window, "17-desktop-pixels");
    viewer.window.close();
    assert!(
        viewer.picture.paintable().is_none(),
        "closing clears screen pixels"
    );
    core.shutdown();
}

fn computer_names_refresh_without_changing_selection() {
    let home = open_home(quiet());
    let mut state = snapshot(vec![peer(Grants::default())], true);
    state.devices[0].hostname = Some("home-mini".into());
    home.show(&state);
    home.rows().select_row(home.rows().row_at_index(1).as_ref());
    assert_eq!(home.title().title().as_str(), "home-mini");
    state.devices[0].pet_name = Some("My desk".into());
    home.show(&state);
    assert_eq!(home.title().title().as_str(), "My desk");
    assert!(labels(&home.rows()).iter().any(|s| s == "My desk"));
    state.devices[0].hostname = Some("renamed-mini".into());
    home.show(&state);
    assert_eq!(home.title().title().as_str(), "My desk");
    state.devices[0].pet_name = None;
    home.show(&state);
    assert_eq!(home.title().title().as_str(), "renamed-mini");
}
