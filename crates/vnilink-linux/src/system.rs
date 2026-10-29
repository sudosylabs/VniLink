//! Linux OS actions over D-Bus. Workstation lock goes to the session
//! screensaver; restart and shutdown go to logind, without asking for a
//! password no one may be there to type. Display privacy uses probed X11
//! scanout gamma ramps. Wayland display privacy and global input blocking
//! remain unsupported.

use std::time::Duration;

use vnilink::system::{HostSessionState, HostSystemActions, SystemSupport};
use vnilink::Error;
use zbus::blocking::{connection::Builder, Connection, Proxy};

#[derive(Default)]
pub struct LinuxSystemActions {
    privacy: crate::privacy::Privacy,
}

/// The longest one D-Bus call may take. GNOME answers `Lock` only once its
/// lock screen is up, and never while it already is: unbounded, one call
/// held the host's system service, and every later action was refused.
/// The bound and the lock sequence were measured on GNOME only. KDE's,
/// Xfce's and other screensavers behind the same freedesktop interface are
/// unverified: one that locks later than this may do so after the host has
/// reported that no screensaver answered.
const CALL_TIMEOUT: Duration = Duration::from_secs(3);

/// The screensavers that lock a session, freedesktop's first.
const SCREENSAVERS: [(&str, &str, &str); 2] = [
    (
        "org.freedesktop.ScreenSaver",
        "/ScreenSaver",
        "org.freedesktop.ScreenSaver",
    ),
    (
        "org.gnome.ScreenSaver",
        "/org/gnome/ScreenSaver",
        "org.gnome.ScreenSaver",
    ),
];

impl LinuxSystemActions {
    fn bus(builder: zbus::Result<Builder<'static>>) -> zbus::Result<Connection> {
        builder?.method_timeout(CALL_TIMEOUT).build()
    }

    /// Whether this session is locked, as logind knows it; `None` when it
    /// cannot say. The screensavers do not: GNOME's `GetActive` answers false
    /// on a locked screen, and freedesktop's does not implement it.
    fn locked() -> Option<bool> {
        let connection = Self::bus(Builder::system()).ok()?;
        let proxy = Proxy::new(
            &connection,
            "org.freedesktop.login1",
            "/org/freedesktop/login1/session/auto",
            "org.freedesktop.login1.Session",
        )
        .ok()?;
        proxy.get_property("LockedHint").ok()
    }

    fn lock_via(
        connection: &Connection,
        (bus, path, interface): (&str, &str, &str),
    ) -> Result<(), zbus::Error> {
        let proxy = Proxy::new(connection, bus, path, interface)?;
        proxy.call_method("Lock", &())?;
        Ok(())
    }

    /// Restarts or powers off through logind when it allows it without a
    /// password: `can` is its question (`CanReboot`), `method` the action.
    fn login1(&self, can: &str, method: &str) -> Result<(), Error> {
        let connection = Self::bus(Builder::system())
            .map_err(|_| Error::Unsupported("logind is not reachable"))?;
        let proxy = Proxy::new(
            &connection,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )
        .map_err(|_| Error::Unsupported("logind is not reachable"))?;
        let allowed: String = proxy
            .call(can, &())
            .map_err(|_| Error::Unsupported("logind is not reachable"))?;
        match allowed.as_str() {
            "yes" => {}
            // A password typed on this computer: the person asking is not here.
            "challenge" => return Err(Error::Unauthorized),
            _ => return Err(Error::Unsupported("logind refused the request")),
        }
        proxy
            .call_method(method, &false)
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
        // Already locked is locked: no request to wait on.
        if Self::locked() == Some(true) {
            return Ok(());
        }
        let connection =
            Self::bus(Builder::session()).map_err(|_| Error::Unsupported("no session bus"))?;
        for screensaver in SCREENSAVERS {
            let sent = Self::lock_via(&connection, screensaver);
            // An answer that timed out may still have locked the screen.
            if sent.is_ok() || Self::locked() == Some(true) {
                return Ok(());
            }
        }
        Err(Error::Unsupported(
            "no screensaver answered the lock request",
        ))
    }

    fn restart(&self) -> Result<(), Error> {
        self.login1("CanReboot", "Reboot")
    }

    fn shutdown(&self) -> Result<(), Error> {
        self.login1("CanPowerOff", "PowerOff")
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
        let wayland = crate::session::is_wayland();
        let x11 = crate::session::is_x11();
        HostSessionState {
            locked: Self::locked(),
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
