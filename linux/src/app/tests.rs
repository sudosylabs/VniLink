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
        files: Rc::new(|_| {}),
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
    pointer_motion_does_not_require_keyboard_focus();
    remote_tabs_are_independent();
    super::clipboard::tests::hidden_window_accepts_clipboard();
    super::clipboard::tests::automatic_text_contract();
    files_panes_show_both_computers();
    desktop_drop_stages_approved_item_without_uploading();
    files_sort_drop_and_restore_locations();
    files_navigate_mkdir_rename_touch_real_disks();
    files_upload_and_download_move_trees();
    files_conflict_waits_and_replace_wins();
    files_revoke_denies_the_remote_pane();
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
        view: true,
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
    assert!(!labels(&viewer.window).iter().any(|s| s == "Quality"));
    assert!(viewer
        .picture
        .observe_controllers()
        .iter::<gtk::glib::Object>()
        .any(|c| c.unwrap().is::<gtk::DropTarget>()));
    viewer.window.set_content(Some(&viewer.content));
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
    let frame = vnilink_media::DecodedFrame {
        frame: vnilink_media::RawFrame::bgra(960, 540, 0, pixels).unwrap(),
        display_id: 1,
        revision: 1,
    };
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
    viewer.retire();
    viewer.window.close();
    assert!(
        viewer.picture.paintable().is_none(),
        "closing clears screen pixels"
    );
    files_gallery();
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

/// Run the actual application lifecycle in an isolated desktop session/config.
#[test]
fn background_close_reopen_and_quit() {
    if std::env::var_os("VNILINK_BACKGROUND_CHILD").is_none() {
        let config =
            std::env::temp_dir().join(format!("vnilink-background-{}", std::process::id()));
        std::fs::create_dir_all(&config).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "app::tests::background_close_reopen_and_quit",
                "--test-threads=1",
            ])
            .env("VNILINK_BACKGROUND_CHILD", "1")
            .env("XDG_CONFIG_HOME", &config)
            .env("GTK_IM_MODULE", "gtk-im-context-simple")
            .status()
            .unwrap();
        std::fs::remove_dir_all(config).unwrap();
        assert!(status.success());
        return;
    }
    let app = super::application();
    let stopped = Rc::new(Cell::new(false));
    let flag = stopped.clone();
    app.connect_shutdown(move |_| flag.set(true));
    let weak = app.downgrade();
    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
        let app = weak.upgrade().unwrap();
        let main = app
            .windows()
            .into_iter()
            .find(|w| w.widget_name() == "vnilink-main")
            .unwrap();
        // A login launch may stay hidden when a tray host is available.
        // Explicit activation must reveal it regardless of the tray environment.
        app.activate();
        assert!(main.is_visible());
        super::background::set_keep_running(true).unwrap();
        main.close();
        assert!(!main.is_visible());
        assert_eq!(app.windows().len(), 1);
        app.activate();
        assert!(main.is_visible());
        assert_eq!(
            app.windows().len(),
            1,
            "reopening must reuse the existing core/window"
        );
        super::background::set_login(true).unwrap();
        assert!(super::background::login_enabled());
        let path = gtk::glib::user_config_dir().join("autostart/com.vnilink.VniLink.desktop");
        let entry = gtk::gio::DesktopAppInfo::from_filename(path).expect("valid desktop entry");
        let args =
            gtk::glib::shell_parse_argv(entry.commandline().unwrap().to_str().unwrap()).unwrap();
        assert_eq!(
            std::path::Path::new(&args[0]),
            std::env::current_exe().unwrap()
        );
        assert_eq!(args[1], "--background");
        assert!(entry
            .commandline()
            .unwrap()
            .to_string_lossy()
            .ends_with(" --background"));
        super::background::set_login(false).unwrap();
        assert!(!super::background::login_enabled());
        super::background::set_keep_running(false).unwrap();
        assert!(!super::background::keep_running());
        main.close();
    });
    assert_eq!(
        app.run_with_args(&["vnilink", "--background"]),
        gtk::glib::ExitCode::SUCCESS
    );
    assert!(
        stopped.get(),
        "closing with background mode off must shut down the app"
    );
}

fn pointer_motion_does_not_require_keyboard_focus() {
    let window = gtk::Window::new();
    let picture = gtk::Picture::builder().focusable(true).build();
    window.set_child(Some(&picture));
    let sent = Rc::new(RefCell::new(Vec::new()));
    let output = sent.clone();
    let source = Rc::new(Cell::new(vnilink::DisplayInfo {
        id: 1,
        origin_x: 0,
        origin_y: 0,
        width: 1920,
        height: 1080,
        rotation_deg: 0,
        scale_percent: 100,
        revision: 3,
    }));
    let resolve_source = Rc::clone(&source);
    let motion = super::viewer::install_pointer_motion(
        &picture,
        Rc::new(Cell::new((960, 540))),
        Rc::new(move || Some(resolve_source.get())),
        move |x, y, _display| output.borrow_mut().push((x, y)),
    );
    // Allocate the image without presenting a focused desktop. Emit the same
    // GTK motion signal the native event controller delivers to the callback.
    picture.allocate(960, 540, -1, None);
    assert!(!picture.has_focus());
    motion.emit_by_name::<()>("motion", &[&480.0f64, &270.0f64]);
    assert_eq!(&*sent.borrow(), &[(960, 540)]);
    // GTK can emit motion again when a new paintable changes hit testing.
    // An unchanged controller pointer must not keep warping the host pointer.
    motion.emit_by_name::<()>("motion", &[&480.0f64, &270.0f64]);
    assert_eq!(sent.borrow().len(), 1, "stationary pointer was resent");
    motion.emit_by_name::<()>("motion", &[&481.0f64, &270.0f64]);
    assert_eq!(&*sent.borrow(), &[(960, 540), (962, 540)]);
    // Letterboxing still excludes movement outside the actual remote image.
    picture.allocate(960, 700, -1, None);
    motion.emit_by_name::<()>("motion", &[&480.0f64, &10.0f64]);
    assert_eq!(sent.borrow().len(), 2);
    // SPEC17: a source change rescales the next event to the new
    // extent instead of the handshake monitor.
    source.set(vnilink::DisplayInfo {
        id: 2,
        origin_x: 1920,
        origin_y: 0,
        width: 1280,
        height: 720,
        rotation_deg: 0,
        scale_percent: 100,
        revision: 4,
    });
    picture.allocate(960, 540, -1, None);
    motion.emit_by_name::<()>("motion", &[&480.0f64, &270.0f64]);
    assert_eq!(&*sent.borrow(), &[(960, 540), (962, 540), (640, 360)]);
    window.close();
}

struct FilePair {
    host: std::sync::Arc<vnilink::Core>,
    controller: std::sync::Arc<vnilink::Core>,
    host_root: std::path::PathBuf,
    controller_root: std::path::PathBuf,
    _scratch: std::path::PathBuf,
}
fn paired_file_cores(tag: &str) -> FilePair {
    use std::sync::Arc;
    let scratch =
        std::env::temp_dir().join(format!("vnilink-gtk-files-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let host_root = scratch.join("host-root");
    let controller_root = scratch.join("controller-root");
    std::fs::create_dir_all(&host_root).unwrap();
    std::fs::create_dir_all(&controller_root).unwrap();
    let open = || {
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
        .unwrap()
    };
    let host = Arc::new(open());
    let controller = Arc::new(open());
    host.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .unwrap();
    controller
        .set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .unwrap();
    host.set_blob_directory(scratch.join("host-blobs")).unwrap();
    controller
        .set_blob_directory(scratch.join("controller-blobs"))
        .unwrap();
    host.set_file_roots(vec![host_root.clone()]).unwrap();
    controller
        .set_file_roots(vec![controller_root.clone()])
        .unwrap();
    host.start_hosting().unwrap();
    let token = host
        .create_invitation(std::time::Duration::from_secs(60))
        .unwrap();
    controller.accept_invitation(&token).unwrap();
    let back = controller
        .create_invitation(std::time::Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&back).unwrap();
    let grants = vnilink::Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: true,
    };
    host.authorize(&controller.endpoint_id(), grants).unwrap();
    controller.authorize(&host.endpoint_id(), grants).unwrap();
    let _session = controller.connect(&host.endpoint_id()).unwrap();
    host.start_job_driver();
    FilePair {
        host,
        controller,
        host_root,
        controller_root,
        _scratch: scratch,
    }
}

fn open_files(pair: &FilePair) -> Rc<super::filemanager::FileManager> {
    open_files_for(&pair.controller, &pair.host, "Remote computer")
}

fn open_host_files(pair: &FilePair) -> Rc<super::filemanager::FileManager> {
    open_files_for(&pair.host, &pair.controller, "Controller computer")
}

fn open_files_for(
    core: &std::sync::Arc<vnilink::Core>,
    peer: &vnilink::Core,
    remote_name: &str,
) -> Rc<super::filemanager::FileManager> {
    let parent = adw::ApplicationWindow::builder().build();
    parent.present();
    super::filemanager::FileManager::open(
        &parent,
        core.clone(),
        peer.endpoint_id(),
        "This computer".into(),
        remote_name.into(),
    )
}

fn files_fixture(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::create_dir_all(root.join("empty")).unwrap();
    std::fs::write(root.join("top.txt"), b"top-bytes").unwrap();
    std::fs::write(root.join("docs/guide-ß.txt"), b"guide-bytes").unwrap();
}

/// An appended `adw::ActionRow` already is a `gtk::ListBoxRow`: the
/// list hands back the row itself, not a wrapper around it.
fn entry_of(row: &gtk::Widget) -> Option<adw::ActionRow> {
    row.clone().downcast::<adw::ActionRow>().ok().or_else(|| {
        row.first_child()
            .and_then(|child| child.downcast::<adw::ActionRow>().ok())
    })
}

fn list_names(list: &gtk::ListBox) -> Vec<String> {
    let mut names = Vec::new();
    let mut child = list.first_child();
    while let Some(row) = child {
        if let Some(entry) = entry_of(&row) {
            names.push(entry.title().to_string());
        }
        child = row.next_sibling();
    }
    names
}

fn row_named(list: &gtk::ListBox, name: &str) -> Option<gtk::ListBoxRow> {
    let mut child = list.first_child();
    while let Some(row) = child {
        let next = row.next_sibling();
        let matches = entry_of(&row).is_some_and(|entry| entry.title() == name);
        if matches {
            return row.downcast::<gtk::ListBoxRow>().ok();
        }
        child = next;
    }
    None
}

fn select_named(list: &gtk::ListBox, name: &str) {
    let row = row_named(list, name).expect("row exists");
    list.select_row(Some(&row));
}

fn has_widget(manager: &Rc<super::filemanager::FileManager>, name: &str) -> bool {
    fn walk(widget: &gtk::Widget, name: &str) -> bool {
        if widget.widget_name() == name {
            return true;
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            if walk(&widget, name) {
                return true;
            }
            child = widget.next_sibling();
        }
        false
    }
    walk(manager.window.upcast_ref::<gtk::Widget>(), name)
}

fn files_dialog() -> adw::MessageDialog {
    pump_until(|| {
        gtk::Window::list_toplevels()
            .into_iter()
            .any(|window| window.downcast::<adw::MessageDialog>().is_ok())
    });
    gtk::Window::list_toplevels()
        .into_iter()
        .find_map(|window| window.downcast::<adw::MessageDialog>().ok())
        .expect("dialog is open")
}

fn queue_text(manager: &Rc<super::filemanager::FileManager>) -> Vec<String> {
    labels(&manager.queue)
}

fn files_panes_show_both_computers() {
    let pair = paired_file_cores("panes");
    files_fixture(&pair.host_root);
    files_fixture(&pair.controller_root);
    let manager = open_files(&pair);
    let existing = open_files(&pair);
    assert!(
        Rc::ptr_eq(&manager, &existing),
        "Device and viewer entry points must reuse one Files owner"
    );
    pump_until(|| list_names(&manager.local.list).len() == 3);
    pump_until(|| list_names(&manager.remote.list).len() == 3);
    assert_eq!(
        list_names(&manager.local.list),
        list_names(&manager.remote.list)
    );
    assert!(list_names(&manager.local.list).contains(&"top.txt".to_string()));
    let text = labels(&manager.window);
    assert!(text.iter().any(|label| label == "This computer"));
    assert!(text.iter().any(|label| label == "Remote computer"));
    assert!(text.iter().any(|label| label == "Transfer queue"));
    assert_eq!(manager.local.roots.active_id().as_deref(), Some("0"));
    manager.window.close();
    let reopened = open_files(&pair);
    assert!(!Rc::ptr_eq(&manager, &reopened));
    pump_until(|| list_names(&reopened.remote.list).len() == 3);
    reopened.window.close();
}

fn files_sort_drop_and_restore_locations() {
    let pair = paired_file_cores("sort-drop");
    std::fs::create_dir_all(pair.controller_root.join("folder")).unwrap();
    std::fs::write(pair.controller_root.join("a.txt"), b"small").unwrap();
    std::fs::write(pair.controller_root.join("z.txt"), b"larger-file").unwrap();
    std::fs::create_dir_all(pair.host_root.join("target")).unwrap();
    let manager = open_files(&pair);
    let host_manager = open_host_files(&pair);
    pump_until(|| list_names(&manager.local.list).len() == 3);
    pump_until(|| list_names(&manager.remote.list).len() == 1);
    manager.local.sort.set_active_id(Some("size"));
    assert_eq!(
        list_names(&manager.local.list),
        ["folder", "z.txt", "a.txt"]
    );
    select_named(&manager.local.list, "z.txt");
    manager.local.sort.set_active_id(Some("descending"));
    assert_eq!(manager.local.list.selected_rows().len(), 1);
    assert!(!has_widget(&manager, "files_drive"));

    pump_until(|| {
        row_named(&manager.local.list, "z.txt").is_some_and(|row| row.height() > 0)
            && row_named(&manager.remote.list, "target").is_some_and(|row| row.height() > 0)
    });
    let controllers = manager.local.list.observe_controllers();
    let source = (0..controllers.n_items())
        .find_map(|i| controllers.item(i)?.downcast::<gtk::DragSource>().ok())
        .unwrap();
    let row = row_named(&manager.local.list, "z.txt").unwrap();
    let y = f64::from(row.allocation().y() + 2);
    let provider = source
        .emit_by_name::<Option<gtk::gdk::ContentProvider>>("prepare", &[&2f64, &y])
        .unwrap();
    let value = provider
        .value(gtk::glib::BoxedAnyObject::static_type())
        .unwrap();
    let controllers = manager.remote.list.observe_controllers();
    let target = (0..controllers.n_items())
        .find_map(|i| controllers.item(i)?.downcast::<gtk::DropTarget>().ok())
        .unwrap();
    let row = row_named(&manager.remote.list, "target").unwrap();
    let y = f64::from(row.allocation().y() + 2);
    assert!(target.emit_by_name::<bool>("drop", &[&gtk::glib::BoxedValue(value), &2f64, &y]));
    pump_until(|| pair.host_root.join("target/z.txt").is_file());
    assert_eq!(
        std::fs::read(pair.host_root.join("target/z.txt")).unwrap(),
        b"larger-file"
    );

    select_named(&manager.local.list, "folder");
    let row = row_named(&manager.local.list, "folder").unwrap();
    manager
        .local
        .list
        .emit_by_name::<()>("row-activated", &[&row]);
    pump_until(|| {
        manager.local.rel.borrow().as_slice() == ["folder"]
            && manager.local.entries.borrow().is_empty()
    });
    manager.window.close();
    let reopened = open_files(&pair);
    pump_until(|| reopened.local.path.label() == "/folder");
    assert_eq!(reopened.local.rel.borrow().as_slice(), ["folder"]);
    reopened.window.close();
    host_manager.window.close();
}

fn files_navigate_mkdir_rename_touch_real_disks() {
    let pair = paired_file_cores("mutate");
    files_fixture(&pair.host_root);
    files_fixture(&pair.controller_root);
    let manager = open_files(&pair);
    pump_until(|| list_names(&manager.local.list).len() == 3);

    // Local mkdir through the real dialog wiring.
    click(&manager.window, "files_local_mkdir");
    let dialog = files_dialog();
    named::<gtk::Entry>(&dialog, "files_local_mkdir_name").set_text("docs2");
    dialog.response("ok");
    pump_until(|| pair.controller_root.join("docs2").is_dir());

    // Local rename of a real file, fenced by its listing token.
    select_named(&manager.local.list, "top.txt");
    click(&manager.window, "files_local_rename");
    let dialog = files_dialog();
    named::<gtk::Entry>(&dialog, "files_local_rename_name").set_text("renamed.txt");
    dialog.response("ok");
    pump_until(|| pair.controller_root.join("renamed.txt").is_file());
    assert_eq!(
        std::fs::read(pair.controller_root.join("renamed.txt")).unwrap(),
        b"top-bytes"
    );

    // Remote mkdir and rename land on the paired core's disk.
    click(&manager.window, "files_remote_mkdir");
    let dialog = files_dialog();
    named::<gtk::Entry>(&dialog, "files_remote_mkdir_name").set_text("incoming");
    dialog.response("ok");
    pump_until(|| pair.host_root.join("incoming").is_dir());
    select_named(&manager.remote.list, "top.txt");
    click(&manager.window, "files_remote_rename");
    let dialog = files_dialog();
    named::<gtk::Entry>(&dialog, "files_remote_rename_name").set_text("moved.txt");
    dialog.response("ok");
    pump_until(|| pair.host_root.join("moved.txt").is_file());
    assert_eq!(
        std::fs::read(pair.host_root.join("moved.txt")).unwrap(),
        b"top-bytes"
    );

    // Entering a folder narrows the pane; the parent button restores it.
    select_named(&manager.local.list, "docs");
    if let Some(row) = row_named(&manager.local.list, "docs") {
        manager
            .local
            .list
            .emit_by_name::<()>("row-activated", &[&row]);
    }
    pump_until(|| list_names(&manager.local.list) == vec!["guide-ß.txt".to_string()]);
    click(&manager.window, "files_local_back");
    pump_until(|| list_names(&manager.local.list).len() == 4);
    manager.window.close();
}

fn files_upload_and_download_move_trees() {
    let pair = paired_file_cores("trees");
    files_fixture(&pair.controller_root);
    files_fixture(&pair.host_root);
    let manager = open_files(&pair);
    pump_until(|| list_names(&manager.local.list).len() == 3);

    // Upload one file, then drive until it lands and completes. Both
    // windows tick once a second and advance their own side. The file is
    // fresh on the controller so nothing collides on the host.
    let host_manager = open_host_files(&pair);
    std::fs::write(pair.controller_root.join("fresh-up.txt"), b"fresh-bytes").unwrap();
    click(&manager.window, "files_local_refresh");
    pump_until(|| list_names(&manager.local.list).contains(&"fresh-up.txt".to_string()));
    select_named(&manager.local.list, "fresh-up.txt");
    click(&manager.window, "files_send");
    // The queue repaints asynchronously after the submit lands; wait for
    // rows, and report the status line if they never come.
    pump_until(|| !queue_text(&manager).is_empty());
    assert!(
        !queue_text(&manager).is_empty(),
        "send failed: {}",
        manager.status.label()
    );
    pump_until(|| pair.host_root.join("fresh-up.txt").is_file());
    pump_until(|| {
        queue_text(&manager)
            .iter()
            .any(|line| line.contains("Complete"))
            && queue_text(&host_manager).is_empty()
    });
    assert_eq!(
        std::fs::read(pair.host_root.join("fresh-up.txt")).unwrap(),
        b"fresh-bytes"
    );

    // Download the remote folder into a fresh local folder: make it,
    // enter it, then receive there so nothing collides.
    click(&manager.window, "files_local_mkdir");
    let dialog = files_dialog();
    named::<gtk::Entry>(&dialog, "files_local_mkdir_name").set_text("got");
    dialog.response("ok");
    pump_until(|| pair.controller_root.join("got").is_dir());
    pump_until(|| list_names(&manager.local.list).contains(&"got".to_string()));
    select_named(&manager.local.list, "got");
    if let Some(row) = row_named(&manager.local.list, "got") {
        manager
            .local
            .list
            .emit_by_name::<()>("row-activated", &[&row]);
    }
    pump_until(|| list_names(&manager.local.list).is_empty());
    select_named(&manager.remote.list, "docs");
    click(&manager.window, "files_receive");
    pump_until(|| pair.controller_root.join("got/docs/guide-ß.txt").is_file());
    assert_eq!(
        std::fs::read(pair.controller_root.join("got/docs/guide-ß.txt")).unwrap(),
        b"guide-bytes"
    );
    manager.window.close();
    host_manager.window.close();
    pump_until(|| {
        pair.controller
            .controller_services()
            .unwrap()
            .iter()
            .all(|service| {
                service.kind != vnilink::ServiceKind::Files
                    || matches!(service.phase, vnilink::ServicePhase::Closed(_))
            })
    });
    assert!(pair
        .controller
        .controller_sessions()
        .unwrap()
        .iter()
        .all(|session| session.phase == SessionPhase::Active));
}

fn files_conflict_waits_and_replace_wins() {
    let pair = paired_file_cores("conflict");
    std::fs::write(pair.controller_root.join("a.txt"), b"new-bytes").unwrap();
    std::fs::write(pair.host_root.join("a.txt"), b"old-bytes").unwrap();
    let manager = open_files(&pair);
    let host_manager = open_host_files(&pair);
    pump_until(|| list_names(&manager.local.list) == vec!["a.txt".to_string()]);
    select_named(&manager.local.list, "a.txt");
    click(&manager.window, "files_send");
    // The initiating owner mirrors the host conflict and forwards its decision.
    // The other window must not adopt this job.
    pump_until(|| manager.waiting_job().is_some());
    let waiting = manager.waiting_job().unwrap();
    pump_until(|| has_widget(&manager, &format!("job-{waiting}-decide")));
    click(&manager.window, &format!("job-{waiting}-decide"));
    let dialog = files_dialog();
    dialog.response("replace");
    pump_until(|| {
        std::fs::read(pair.host_root.join("a.txt"))
            .ok()
            .is_some_and(|bytes| bytes == b"new-bytes")
    });
    assert_eq!(
        std::fs::read(pair.host_root.join("a.txt")).unwrap(),
        b"new-bytes"
    );
    manager.window.close();
    host_manager.window.close();
}

fn files_revoke_denies_the_remote_pane() {
    let pair = paired_file_cores("revoke");
    files_fixture(&pair.host_root);
    let manager = open_files(&pair);
    pump_until(|| !list_names(&manager.remote.list).is_empty());
    pair.host.revoke(&pair.controller.endpoint_id()).unwrap();
    manager.refresh_remote();
    pump_until(|| !manager.remote.message.label().is_empty());
    assert!(list_names(&manager.remote.list).is_empty());
    manager.window.close();
}

/// Files gallery: real panes, queue states and dialogs on loopback cores,
/// captured for visual inspection alongside the widget contracts.
fn files_gallery() {
    let pair = paired_file_cores("gallery");
    files_fixture(&pair.host_root);
    files_fixture(&pair.controller_root);
    let manager = open_files(&pair);
    manager.window.present();
    pump_until(|| list_names(&manager.local.list).len() == 3);
    pump_until(|| list_names(&manager.remote.list).len() == 3);
    capture_widget(&manager.window, "19-files-populated");

    // Empty folder state on the remote pane.
    select_named(&manager.remote.list, "empty");
    if let Some(row) = row_named(&manager.remote.list, "empty") {
        manager
            .remote
            .list
            .emit_by_name::<()>("row-activated", &[&row]);
    }
    pump_until(|| list_names(&manager.remote.list).is_empty());
    capture_widget(&manager.window, "20-files-empty");
    click(&manager.window, "files_remote_back");
    pump_until(|| list_names(&manager.remote.list).len() == 3);

    // A completed job plus a waiting conflict in the queue. The upload
    // uses a fresh file so it completes; the conflict is staged after.
    std::fs::write(pair.controller_root.join("fresh.txt"), b"fresh-bytes").unwrap();
    click(&manager.window, "files_local_refresh");
    pump_until(|| list_names(&manager.local.list).contains(&"fresh.txt".to_string()));
    let host_manager = open_host_files(&pair);
    host_manager.window.present();
    select_named(&manager.local.list, "fresh.txt");
    click(&manager.window, "files_send");
    pump_until(|| pair.host_root.join("fresh.txt").is_file());
    std::fs::write(pair.host_root.join("keep.txt"), b"remote-original").unwrap();
    std::fs::write(pair.controller_root.join("keep.txt"), b"local-copy").unwrap();
    click(&manager.window, "files_local_refresh");
    pump_until(|| list_names(&manager.local.list).contains(&"keep.txt".to_string()));
    // Refresh preserves selection now; this scenario sends only keep.txt.
    manager.local.list.unselect_all();
    select_named(&manager.local.list, "keep.txt");
    click(&manager.window, "files_send");
    pump_until(|| manager.waiting_job().is_some());
    let waiting = manager.waiting_job().unwrap();
    pump_until(|| has_widget(&manager, &format!("job-{waiting}-decide")));
    capture_widget(&manager.window, "21-files-queue");
    click(&manager.window, &format!("job-{waiting}-decide"));
    let dialog = files_dialog();
    capture_widget(&dialog, "22-files-conflict");
    dialog.response("both");
    pump_until(|| {
        std::fs::read_dir(&pair.host_root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .any(|name| name.starts_with("keep (2)"))
    });

    // Revoked access reads as a denied pane, not an empty one.
    pair.host.revoke(&pair.controller.endpoint_id()).unwrap();
    manager.refresh_remote();
    pump_until(|| !manager.remote.message.label().is_empty());
    capture_widget(&manager.window, "23-files-denied");

    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    capture_widget(&manager.window, "24-files-dark");
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceLight);
    manager.window.close();
}

/// Real paired connections, native AdwTabView routing and retained viewer owners.
fn remote_tabs_are_independent() {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use vnilink::{Core, CustomRelay, Listen, MemoryStore, OpenConfig, RelaySettings};
    struct Clipboard(Arc<Mutex<Option<String>>>);
    impl vnilink::HostClipboard for Clipboard {
        fn set_text(&mut self, text: &str) -> Result<(), vnilink::Error> {
            *self.0.lock().unwrap() = Some(text.into());
            Ok(())
        }
    }
    struct Capture;
    impl vnilink::HostCapture for Capture {
        fn monitor(&self) -> (u16, u16) {
            (1280, 720)
        }
    }
    struct Desktop(Arc<Mutex<Vec<vnilink::InputEvent>>>);
    impl vnilink::HostDesktop for Desktop {
        fn monitor(&self) -> (u16, u16) {
            (1280, 720)
        }
        fn apply(&mut self, event: vnilink::InputEvent) -> Result<(), vnilink::Error> {
            self.0.lock().unwrap().push(event);
            Ok(())
        }
    }
    let make_core = || {
        Arc::new(
            Core::open(
                Arc::new(MemoryStore::empty()),
                OpenConfig {
                    listen: Listen::Loopback,
                    initial_relay: RelaySettings::custom(vec![CustomRelay::new(
                        "http://127.0.0.1:9",
                    )
                    .unwrap()])
                    .unwrap(),
                    ..OpenConfig::default()
                },
            )
            .unwrap(),
        )
    };
    let controller = make_core();
    let a = make_core();
    let b = make_core();
    let pending = make_core();
    controller
        .set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .unwrap();
    let clipboard_a = Arc::new(Mutex::new(None));
    let clipboard_b = Arc::new(Mutex::new(None));
    a.set_clipboard(Box::new(Clipboard(clipboard_a.clone())))
        .unwrap();
    b.set_clipboard(Box::new(Clipboard(clipboard_b.clone())))
        .unwrap();
    let input_a = Arc::new(Mutex::new(Vec::new()));
    let input_b = Arc::new(Mutex::new(Vec::new()));
    for (host, input) in [(&a, &input_a), (&b, &input_b)] {
        host.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
        host.set_capture(Box::new(Capture)).unwrap();
        host.set_desktop(Box::new(Desktop(input.clone()))).unwrap();
    }
    pending
        .set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .unwrap();
    pending
        .set_desktop(Box::new(Desktop(Arc::new(Mutex::new(Vec::new())))))
        .unwrap();
    pending.set_capture(Box::new(Capture)).unwrap();
    for (host, name) in [
        (&a, "Computer A"),
        (&b, "Computer B"),
        (&pending, "Approval pending"),
    ] {
        controller
            .accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
            .unwrap();
        host.accept_invitation(
            &controller
                .create_invitation(Duration::from_secs(60))
                .unwrap(),
        )
        .unwrap();
        controller
            .set_peer_pet_name(&host.endpoint_id(), Some(name))
            .unwrap();
        if name != "Approval pending" {
            host.authorize(
                &controller.endpoint_id(),
                Grants {
                    view: true,
                    control: true,
                    clipboard: true,
                    transfer: false,
                },
            )
            .unwrap();
        }
        controller
            .authorize(
                &host.endpoint_id(),
                Grants {
                    view: false,
                    control: false,
                    clipboard: true,
                    transfer: false,
                },
            )
            .unwrap();
        host.start_hosting().unwrap();
    }
    let home = open_home(quiet());
    home.show(&controller.snapshot().unwrap());
    let tabs = super::session_tabs::SessionTabs::install(home.window(), controller.clone());
    super::clipboard::install_for_tabs(home.window(), controller.clone(), tabs.selected.clone());
    home.window().present();
    tabs.connect(&a.endpoint_id()).unwrap();
    tabs.connect(&b.endpoint_id()).unwrap();
    let sessions = controller.controller_sessions().unwrap();
    let sa = sessions
        .iter()
        .find(|s| s.peer_id == a.endpoint_id())
        .unwrap()
        .id;
    let sb = sessions
        .iter()
        .find(|s| s.peer_id == b.endpoint_id())
        .unwrap()
        .id;
    pump_until(|| {
        tabs.refresh();
        tabs.viewer(sa)
            .and_then(|v| v.desktop_owner())
            .is_some_and(|o| {
                o.control_state().ok() == Some(vnilink::DesktopControlState::Controlling)
            })
            && tabs
                .viewer(sb)
                .and_then(|v| v.desktop_owner())
                .is_some_and(|o| {
                    o.control_state().ok() == Some(vnilink::DesktopControlState::Controlling)
                })
    });
    let va = tabs.viewer(sa).unwrap();
    let vb = tabs.viewer(sb).unwrap();
    assert_eq!(tabs.view.n_pages(), 3);
    assert_eq!(tabs.page(sa).title().as_str(), "Computer A");
    assert_eq!(tabs.page(sb).title().as_str(), "Computer B");
    tabs.connect(&a.endpoint_id()).unwrap();
    assert_eq!(
        tabs.view.n_pages(),
        3,
        "connecting twice focuses the existing tab"
    );
    assert_eq!(tabs.selected.load(std::sync::atomic::Ordering::Relaxed), sa);
    assert!(Rc::ptr_eq(&tabs.viewer(sa).unwrap(), &va));
    // Exercise real native textures and a focused keyboard controller.
    va.show_frame(&vnilink_media::DecodedFrame {
        frame: vnilink_media::RawFrame::bgra(2, 2, 0, vec![40; 16]).unwrap(),
        display_id: 1,
        revision: 1,
    });
    vb.show_frame(&vnilink_media::DecodedFrame {
        frame: vnilink_media::RawFrame::bgra(2, 2, 0, vec![90; 16]).unwrap(),
        display_id: 2,
        revision: 1,
    });
    va.desktop_owner()
        .unwrap()
        .set_keyboard_mode(vnilink::KeyboardMode::Map)
        .unwrap();
    va.picture.grab_focus();
    let keys = va
        .picture
        .observe_controllers()
        .iter::<gtk::glib::Object>()
        .filter_map(Result::ok)
        .find_map(|c| c.downcast::<gtk::EventControllerKey>().ok())
        .unwrap();
    keys.emit_by_name::<bool>(
        "key-pressed",
        &[&gtk::gdk::Key::a, &38u32, &gtk::gdk::ModifierType::empty()],
    );
    pump_until(|| !input_a.lock().unwrap().is_empty());
    tabs.view.set_selected_page(&tabs.page(sb));
    pump_until(|| {
        input_a
            .lock()
            .unwrap()
            .iter()
            .any(|e| *e == vnilink::InputEvent::key_up(4))
    });
    assert!(
        input_b.lock().unwrap().is_empty(),
        "focus loss must not release keys on the other computer"
    );
    assert!(Rc::ptr_eq(&tabs.viewer(sa).unwrap(), &va));
    assert!(
        va.picture.paintable().is_some(),
        "hidden page retains its picture"
    );
    // Only the selected remote receives local copies or updates the native board.
    let board = home.window().clipboard();
    tabs.view.set_selected_page(&tabs.page(sa));
    board.set_text("copy for A");
    pump_until(|| clipboard_a.lock().unwrap().as_deref() == Some("copy for A"));
    assert!(clipboard_b.lock().unwrap().is_none());
    let host_b = b
        .hosted_services()
        .unwrap()
        .into_iter()
        .find(|s| {
            s.kind == vnilink::ServiceKind::Desktop && s.phase == vnilink::ServicePhase::Active
        })
        .unwrap();
    let publish_b = b.clone();
    let (published, result) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = published.send(publish_b.publish_desktop_clipboard(
            host_b.id,
            host_b.generation,
            "copy from B",
        ));
    });
    pump_until(|| {
        vb.desktop_owner()
            .unwrap()
            .clipboard_text()
            .unwrap()
            .is_some()
    });
    result
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
        .unwrap();
    assert!(board.content().is_some());
    tabs.view.set_selected_page(&tabs.page(sb));
    let copied = Rc::new(RefCell::new(None::<String>));
    pump_until(|| {
        let callback = copied.clone();
        board.read_text_async(None::<&gtk::gio::Cancellable>, move |result| {
            if let Ok(Some(text)) = result {
                *callback.borrow_mut() = Some(text.to_string());
            }
        });
        copied.borrow().as_deref() == Some("copy from B")
    });
    board.set_text("copy for B");
    pump_until(|| clipboard_b.lock().unwrap().as_deref() == Some("copy for B"));
    assert_eq!(clipboard_a.lock().unwrap().as_deref(), Some("copy for A"));
    let home_page = tabs.view.nth_page(0);
    tabs.view.set_selected_page(&home_page);
    assert_eq!(tabs.selected.load(std::sync::atomic::Ordering::Relaxed), 0);
    tabs.view.close_page(&home_page);
    assert_eq!(tabs.view.n_pages(), 3, "Home cannot be closed");
    tabs.view.set_selected_page(&tabs.page(sb));
    if std::env::var_os("VNILINK_UI_SCREENSHOTS").is_some() {
        capture_widget(home.window(), "tabs-two-computers");
    }
    tabs.view.close_page(&tabs.page(sa));
    pump_until(|| {
        tabs.refresh();
        va.desktop_owner().is_none()
    });
    assert!(va.picture.paintable().is_none());
    assert_eq!(
        controller.controller_session(sa).unwrap().phase,
        SessionPhase::Active
    );
    assert!(controller
        .controller_services()
        .unwrap()
        .iter()
        .all(|s| s.connection_id != sa
            || s.kind != vnilink::ServiceKind::Desktop
            || matches!(s.phase, vnilink::ServicePhase::Closed(_))));
    assert_eq!(
        controller.controller_session(sb).unwrap().phase,
        SessionPhase::Active
    );
    assert!(Rc::ptr_eq(&tabs.viewer(sb).unwrap(), &vb));
    assert_eq!(tabs.view.n_pages(), 2);
    pending
        .authorize(
            &controller.endpoint_id(),
            Grants {
                view: true,
                control: true,
                clipboard: true,
                transfer: true,
            },
        )
        .unwrap();
    let files_root =
        std::env::temp_dir().join(format!("vnilink-neutral-tab-files-{}", std::process::id()));
    std::fs::create_dir_all(&files_root).unwrap();
    std::fs::write(files_root.join("survives.txt"), b"owned by Files").unwrap();
    pending.set_file_roots(vec![files_root.clone()]).unwrap();
    let files = controller
        .begin_connect_services(&pending.endpoint_id())
        .unwrap();
    pump_until(|| controller.controller_session(files.id).unwrap().phase == SessionPhase::Active);
    let parent = controller.controller_session(files.id).unwrap();
    let files_info = controller
        .attach_controller_service(parent.id, parent.generation, vnilink::ServiceKind::Files)
        .unwrap();
    let files_lease = controller
        .service_lease(files_info.id, files_info.generation)
        .unwrap();
    let files_service = controller
        .open_files_service(files_info.id, files_info.generation)
        .unwrap();
    assert_eq!(files_service.list(0, "", 0, 16).unwrap().entries.len(), 1);
    tabs.connect(&pending.endpoint_id()).unwrap();
    pump_until(|| {
        tabs.refresh();
        tabs.viewer(files.id)
            .and_then(|v| v.desktop_owner())
            .is_some()
    });
    assert_eq!(
        controller
            .begin_connect_services(&pending.endpoint_id())
            .unwrap()
            .id,
        files.id
    );
    assert!(
        !files_lease.is_cancelled(),
        "opening Desktop replaced the Files owner"
    );
    tabs.view.close_page(&tabs.page(files.id));
    assert!(
        !files_lease.is_cancelled(),
        "closing Desktop cancelled its sibling Files owner"
    );
    assert_eq!(
        controller.controller_session(files.id).unwrap().phase,
        SessionPhase::Active
    );
    assert_eq!(
        controller.controller_session(sb).unwrap().phase,
        SessionPhase::Active
    );
    assert_eq!(
        files_service.list(0, "", 0, 16).unwrap().entries[0].name,
        "survives.txt"
    );
    files_service.close();
    for core in [controller, a, b, pending] {
        core.shutdown();
    }
    home.window().close();
    std::fs::remove_dir_all(files_root).unwrap();
}

fn desktop_drop_stages_approved_item_without_uploading() {
    let pair = paired_file_cores("desktop-drop");
    files_fixture(&pair.controller_root);
    let parent = adw::ApplicationWindow::builder().build();
    let session = pair
        .controller
        .controller_sessions()
        .unwrap()
        .into_iter()
        .find(|s| s.phase == SessionPhase::Active)
        .unwrap();
    let viewer = super::viewer::Viewer::build(&parent, pair.controller.clone(), session);
    let drop = viewer
        .picture
        .observe_controllers()
        .iter::<gtk::glib::Object>()
        .filter_map(Result::ok)
        .find_map(|c| c.downcast::<gtk::DropTarget>().ok())
        .unwrap();
    let list = gtk::gdk::FileList::from_array(&[gtk::gio::File::for_path(
        pair.controller_root.join("docs"),
    )]);
    assert!(drop.emit_by_name::<bool>(
        "drop",
        &[&gtk::glib::BoxedValue(list.to_value()), &0f64, &0f64]
    ));
    let manager = open_files(&pair);
    pump_until(|| selected_file_names(&manager.local.list) == vec!["docs".to_owned()]);
    assert!(pair.controller.job_list().unwrap().is_empty());
    assert_eq!(
        manager.status.label(),
        "Choose the remote destination, then Send."
    );
    click(&manager.window, "files_send");
    pump_until(|| pair.host_root.join("docs/guide-ß.txt").exists());
    assert_eq!(
        std::fs::read(pair.host_root.join("docs/guide-ß.txt")).unwrap(),
        b"guide-bytes"
    );
    manager.window.close();
    viewer.window.close();
}
fn selected_file_names(list: &gtk::ListBox) -> Vec<String> {
    list.selected_rows()
        .iter()
        .map(|row| row.widget_name().to_string())
        .collect()
}
