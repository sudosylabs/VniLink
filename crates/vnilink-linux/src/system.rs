//! Linux OS actions over D-Bus. Workstation lock goes to the session
//! screensaver; restart and shutdown go to logind, which may demand
//! interactive polkit authorization. Display privacy uses probed X11 scanout
//! gamma ramps. Wayland display privacy and global input blocking remain unsupported.

use vnilink::system::{HostSessionState, HostSystemActions, SystemSupport};
use vnilink::Error;
use zbus::blocking::{Connection, Proxy};

#[derive(Default)]
pub struct LinuxSystemActions {
    privacy: crate::privacy::Privacy,
}

impl LinuxSystemActions {
    fn lock_via(
        connection: &Connection,
        bus: &str,
        path: &str,
        interface: &str,
    ) -> Result<(), zbus::Error> {
        let proxy = Proxy::new(connection, bus, path, interface)?;
        proxy.call_method("Lock", &())?;
        Ok(())
    }

    fn login1(&self, method: &str) -> Result<(), Error> {
        let connection =
            Connection::system().map_err(|_| Error::Unsupported("logind is not reachable"))?;
        let proxy = Proxy::new(
            &connection,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )
        .map_err(|_| Error::Unsupported("logind is not reachable"))?;
        proxy
            .call_method(method, &true)
            .map_err(|_| Error::Unsupported("logind refused the request"))?;
        Ok(())
    }
}

impl HostSystemActions for LinuxSystemActions {
    fn support(&self) -> SystemSupport {
        SystemSupport {
            lock: true,
            restart: true,
            shutdown: true,
            privacy: crate::privacy::supported(),
            input_block: false,
        }
    }

    fn lock_workstation(&self) -> Result<(), Error> {
        let connection = Connection::session().map_err(|_| Error::Unsupported("no session bus"))?;
        Self::lock_via(
            &connection,
            "org.freedesktop.ScreenSaver",
            "/ScreenSaver",
            "org.freedesktop.ScreenSaver",
        )
        .or_else(|_| {
            Self::lock_via(
                &connection,
                "org.gnome.ScreenSaver",
                "/org/gnome/ScreenSaver",
                "org.gnome.ScreenSaver",
            )
        })
        .map_err(|_| Error::Unsupported("no screensaver answered the lock request"))
    }

    fn restart(&self) -> Result<(), Error> {
        self.login1("Reboot")
    }

    fn shutdown(&self) -> Result<(), Error> {
        self.login1("PowerOff")
    }

    fn set_display_private(&self, private: bool) -> Result<(), Error> {
        self.privacy.set(private)
    }

    fn set_input_blocked(&self, _blocked: bool) -> Result<(), Error> {
        Err(Error::Unsupported("input blocking is not available"))
    }

    fn observed_privacy(&self) -> Option<(bool, bool)> {
        Some((self.privacy.observed(), false))
    }

    fn session_state(&self) -> HostSessionState {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let x11 = std::env::var_os("DISPLAY").is_some();
        HostSessionState {
            locked: None,
            login_screen: None,
            headless: !wayland && !x11,
            session: if wayland {
                "wayland"
            } else if x11 {
                "x11"
            } else {
                "linux"
            }
            .into(),
        }
    }
}
