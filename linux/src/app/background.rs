//! Desktop lifecycle preferences. No identity or credentials live here.
use adw::prelude::*;
use ksni::blocking::TrayMethods;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        mpsc,
    },
};

pub(crate) fn keep_running() -> bool {
    fs::read_to_string(gtk::glib::user_config_dir().join("vnilink/background"))
        .map_or(true, |value| value.trim() != "false")
}
pub(crate) fn set_keep_running(enabled: bool) -> Result<(), String> {
    let path = gtk::glib::user_config_dir().join("vnilink/background");
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(path, if enabled { "true\n" } else { "false\n" }).map_err(|e| e.to_string())
}
pub(crate) fn resume_x11() -> bool {
    fs::read_to_string(gtk::glib::user_config_dir().join("vnilink/resume-x11"))
        .is_ok_and(|v| v.trim() == "true")
}
fn set_resume_x11(enabled: bool) -> Result<(), String> {
    let path = gtk::glib::user_config_dir().join("vnilink/resume-x11");
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(path, if enabled { "true\n" } else { "false\n" }).map_err(|e| e.to_string())
}
fn login_path() -> PathBuf {
    gtk::glib::user_config_dir().join("autostart/com.vnilink.VniLink.desktop")
}
pub(crate) fn login_enabled() -> bool {
    login_path().is_file()
}
fn desktop_entry(executable: &Path) -> Result<String, String> {
    let path = executable
        .to_str()
        .ok_or("The executable path is not UTF-8")?;
    if path.contains(['\n', '\r']) {
        return Err("The executable path contains a newline".into());
    }
    // Desktop Entry escaping has two layers: Exec quoting then string escaping.
    let quoted = path
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%");
    let quoted = quoted.replace('\\', "\\\\");
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=VniLink\nComment=Remote access to your paired computers\nExec=\"{quoted}\" --background\nTerminal=false\nIcon=computer\n"
    ))
}
pub(crate) fn set_login(enabled: bool) -> Result<(), String> {
    let path = login_path();
    if enabled {
        let entry = desktop_entry(&std::env::current_exe().map_err(|e| e.to_string())?)?;
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let pending = path.with_extension("desktop.tmp");
        fs::write(&pending, entry).map_err(|e| e.to_string())?;
        fs::rename(pending, path).map_err(|e| e.to_string())
    } else {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}
static STATUS: AtomicU8 = AtomicU8::new(0);
pub(crate) fn update_status(snapshot: &vnilink::Snapshot) {
    let active = snapshot
        .incoming
        .as_ref()
        .is_some_and(|s| s.phase == vnilink::SessionPhase::Active);
    STATUS.store(
        if active {
            2
        } else if snapshot.hosting {
            1
        } else {
            0
        },
        Ordering::Relaxed,
    );
}
#[derive(Debug)]
pub(crate) enum Action {
    Open,
    Settings,
    Stop,
    Quit,
    Unavailable,
}
struct Tray {
    sender: mpsc::Sender<Action>,
}
impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "com.vnilink.VniLink".into()
    }
    fn title(&self) -> String {
        match STATUS.load(Ordering::Relaxed) {
            2 => "VniLink · A computer is connected",
            1 => "VniLink · Remote access on",
            _ => "VniLink · Remote access stopped",
        }
        .into()
    }
    fn watcher_offline(&self, _: ksni::OfflineReason) -> bool {
        let _ = self.sender.send(Action::Unavailable);
        true
    }
    fn icon_name(&self) -> String {
        "computer".into()
    }
    fn activate(&mut self, _: i32, _: i32) {
        let _ = self.sender.send(Action::Open);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Open VniLink".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.sender.send(Action::Open);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Settings…".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.sender.send(Action::Settings);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Stop remote access".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.sender.send(Action::Stop);
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit VniLink".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.sender.send(Action::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}
pub(crate) fn install(app: &adw::Application) {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        match (Tray {
            sender: sender.clone(),
        })
        .spawn()
        {
            Ok(handle) => {
                // Lifetime is the process; shutdown ends its bus connection.
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    handle.update(|_| {});
                }
            }
            Err(_) => {
                let _ = sender.send(Action::Unavailable);
            }
        }
    });
    let weak = app.downgrade();
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        let Some(app) = weak.upgrade() else {
            return gtk::glib::ControlFlow::Break;
        };
        while let Ok(action) = receiver.try_recv() {
            let name = match action {
                Action::Open => "open",
                Action::Settings => "settings",
                Action::Stop => "stop-hosting",
                Action::Quit => "quit",
                Action::Unavailable => "tray-unavailable",
            };
            app.activate_action(name, None);
        }
        gtk::glib::ControlFlow::Continue
    });
}
pub(crate) fn page(window: &adw::Window) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Background")
        .name("background")
        .icon_name("preferences-system-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder().title("Background and startup")
        .description("Closing the window can keep current screen sharing running. Launch at login opens VniLink in the background; X11 can resume desktop sharing when enabled below. Wayland requires selecting a screen after restarting. On desktops without tray support, reopen VniLink from the application launcher.").build();
    let background = adw::SwitchRow::builder()
        .title("Keep running when the window closes")
        .active(keep_running())
        .build();
    let login = adw::SwitchRow::builder()
        .title("Launch at login")
        .active(login_enabled())
        .build();
    let resume = adw::SwitchRow::builder().title("Share X11 desktop and allow connections on launch").subtitle("Also enables keyboard and mouse control for authorized devices. Wayland still requires its screen permission portal.").active(resume_x11()).build();
    for (row, save) in [
        (
            &background,
            set_keep_running as fn(bool) -> Result<(), String>,
        ),
        (&login, set_login),
        (&resume, set_resume_x11),
    ] {
        let win = window.clone();
        let reverting = std::cell::Cell::new(false);
        row.connect_active_notify(move |row| {
            if reverting.get() {
                return;
            }
            if let Err(error) = save(row.is_active()) {
                reverting.set(true);
                row.set_active(!row.is_active());
                reverting.set(false);
                super::preferences::report(&win, &error);
            }
        });
        group.add(row);
    }
    let quit = gtk::Button::with_label("Quit VniLink");
    let win = window.clone();
    quit.connect_clicked(move |_| {
        if let Some(app) = win.transient_for().and_then(|p| p.application()) {
            app.quit();
        }
    });
    group.add(&quit);
    page.add(&group);
    page
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn autostart_exec_quotes_paths_and_rejects_line_injection() {
        let entry = desktop_entry(Path::new("/opt/My App/a\"$`%\\b")).unwrap();
        assert!(
            entry.contains("Exec=\"/opt/My App/a\\\\\"\\\\$\\\\`%%\\\\\\\\b\" --background\n"),
            "{entry}"
        );
        assert!(desktop_entry(Path::new("/tmp/a\nHidden=true")).is_err());
    }
}

#[cfg(test)]
mod bus_tests {
    use super::*;
    #[test]
    fn tray_exports_menu_and_dispatches_activation_over_dbus() {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let handle = Tray { sender }
            .assume_sni_available(true)
            .spawn()
            .expect("tray bus service");
        fn call(destination: &str, path: &str, method: &str, args: &[&str]) -> String {
            let output = std::process::Command::new("gdbus")
                .env("LC_ALL", "C.UTF-8")
                .args([
                    "call",
                    "--session",
                    "--dest",
                    destination,
                    "--object-path",
                    path,
                    "--method",
                    method,
                    "--",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        }
        let names = call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus.ListNames",
            &[],
        );
        let prefix = format!("org.kde.StatusNotifierItem-{}-", std::process::id());
        let name = names
            .split('\'')
            .find(|name| name.starts_with(&prefix))
            .expect("exported tray name");
        let layout = call(
            name,
            "/MenuBar",
            "com.canonical.dbusmenu.GetLayout",
            &["0", "-1", "[]"],
        );
        for label in [
            "Open VniLink",
            "Settings…",
            "Stop remote access",
            "Quit VniLink",
        ] {
            assert!(layout.contains(label), "{layout}");
        }
        call(
            name,
            "/StatusNotifierItem",
            "org.kde.StatusNotifierItem.Activate",
            &["0", "0"],
        );
        // Watcher absence may also have queued the reachability fallback.
        loop {
            match receiver
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap()
            {
                Action::Open => break,
                Action::Unavailable => continue,
                other => panic!("unexpected tray action: {other:?}"),
            }
        }
        handle.shutdown().wait();
    }
}
