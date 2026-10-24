//! Display idle cookies and system sleep FDs have distinct native lifetimes.
use std::time::Duration;
use vnilink::power::{HostPowerFactory, PowerInhibitor, PowerKind};
use vnilink::{BackendStartup, Error};
use zbus::blocking::{connection::Builder, Connection, Proxy};

struct SystemInhibitor {
    _fd: std::os::fd::OwnedFd,
}
impl PowerInhibitor for SystemInhibitor {}
struct DisplayInhibitor {
    proxy: Option<Proxy<'static>>,
    cookie: u32,
    release: &'static str,
}
impl PowerInhibitor for DisplayInhibitor {}
impl Drop for DisplayInhibitor {
    fn drop(&mut self) {
        let Some(proxy) = self.proxy.take() else {
            return;
        };
        let (release, cookie) = (self.release, self.cookie);
        // zbus's blocking calls, and the proxy's own drop, block on a runtime
        // of their own, which Tokio refuses on a thread that drives one: the
        // core releases inhibitors from its runtime, and the panic in this
        // destructor aborted the engine. A thread of their own does both. The
        // release does not wait for the desktop's reply, and dropping the
        // private bus connection lets owner-loss cleanup run after a crash.
        // Should no thread start, the proxy is leaked rather than dropped
        // here, and the bus ends the inhibition with this process.
        let proxy = std::mem::ManuallyDrop::new(proxy);
        let _ = std::thread::Builder::new()
            .name("vnilink-display-inhibit".into())
            .spawn(move || {
                let proxy = std::mem::ManuallyDrop::into_inner(proxy);
                let _ = proxy.call_noreply(release, &cookie);
            });
    }
}
#[derive(Default)]
pub struct LinuxPowerFactory;
impl LinuxPowerFactory {
    fn acquire_on(
        kind: PowerKind,
        connection: Connection,
        startup: &BackendStartup,
    ) -> Result<Box<dyn PowerInhibitor>, Error> {
        startup.check()?;
        match kind {
            PowerKind::System => {
                let proxy = Proxy::new(
                    &connection,
                    "org.freedesktop.login1",
                    "/org/freedesktop/login1",
                    "org.freedesktop.login1.Manager",
                )
                .map_err(|_| Error::Unsupported("Linux sleep inhibition is unavailable"))?;
                startup.check()?;
                let fd: zbus::zvariant::OwnedFd = proxy
                    .call(
                        "Inhibit",
                        &("sleep", "VniLink", "Remote desktop session", "block"),
                    )
                    .map_err(|_| Error::Unsupported("Linux rejected sleep inhibition"))?;
                let owned = SystemInhibitor { _fd: fd.into() };
                startup.check()?;
                Ok(Box::new(owned))
            }
            PowerKind::Display => {
                let gnome = Proxy::new_owned(
                    connection.clone(),
                    "org.gnome.SessionManager",
                    "/org/gnome/SessionManager",
                    "org.gnome.SessionManager",
                )
                .map_err(|_| Error::Unsupported("Linux display inhibition is unavailable"))?;
                startup.check()?;
                let result: zbus::Result<u32> = gnome.call(
                    "Inhibit",
                    &("VniLink", 0u32, "Remote desktop session", 8u32),
                );
                let owned = match result {
                    Ok(cookie) => DisplayInhibitor {
                        proxy: Some(gnome),
                        cookie,
                        release: "Uninhibit",
                    },
                    Err(zbus::Error::MethodError(name, _, _))
                        if matches!(
                            name.as_str(),
                            "org.freedesktop.DBus.Error.ServiceUnknown"
                                | "org.freedesktop.DBus.Error.NameHasNoOwner"
                                | "org.freedesktop.DBus.Error.UnknownMethod"
                        ) =>
                    {
                        startup.check()?;
                        let proxy = Proxy::new_owned(
                            connection,
                            "org.freedesktop.ScreenSaver",
                            "/org/freedesktop/ScreenSaver",
                            "org.freedesktop.ScreenSaver",
                        )
                        .map_err(|_| {
                            Error::Unsupported("Linux display inhibition is unavailable")
                        })?;
                        let cookie: u32 = proxy
                            .call("Inhibit", &("VniLink", "Remote desktop session"))
                            .map_err(|_| Error::Unsupported("Linux rejected display inhibition"))?;
                        DisplayInhibitor {
                            proxy: Some(proxy),
                            cookie,
                            release: "UnInhibit",
                        }
                    }
                    Err(_) => return Err(Error::Unsupported("Linux rejected display inhibition")),
                };
                startup.check()?;
                Ok(Box::new(owned))
            }
        }
    }
}
impl HostPowerFactory for LinuxPowerFactory {
    fn acquire(
        &self,
        kind: PowerKind,
        startup: &BackendStartup,
    ) -> Result<Box<dyn PowerInhibitor>, Error> {
        startup.check()?;
        let builder = match kind {
            PowerKind::Display => Builder::session(),
            PowerKind::System => Builder::system(),
        }
        .map_err(|_| Error::Unsupported("Linux power bus is unavailable"))?;
        let connection = builder
            .method_timeout(Duration::from_secs(2))
            .build()
            .map_err(|_| Error::Unsupported("Linux power bus is unavailable"))?;
        Self::acquire_on(kind, connection, startup)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_power_startup_does_not_connect_to_a_bus() {
        let startup = BackendStartup::new(Duration::ZERO);
        assert!(LinuxPowerFactory
            .acquire(PowerKind::Display, &startup)
            .is_err());
        assert!(LinuxPowerFactory
            .acquire(PowerKind::System, &startup)
            .is_err());
    }
}

#[cfg(test)]
mod bus_tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    struct Display(Arc<AtomicUsize>);
    #[zbus::interface(name = "org.gnome.SessionManager")]
    impl Display {
        fn inhibit(&self, app: &str, xid: u32, reason: &str, flags: u32) -> u32 {
            assert_eq!(
                (app, xid, reason, flags),
                ("VniLink", 0, "Remote desktop session", 8)
            );
            self.0.fetch_add(1, Ordering::SeqCst);
            41
        }
        fn uninhibit(&self, cookie: u32) {
            assert_eq!(cookie, 41);
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    struct System(Mutex<Option<std::os::unix::net::UnixStream>>);
    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl System {
        fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::zvariant::OwnedFd {
            assert_eq!(
                (what, who, why, mode),
                ("sleep", "VniLink", "Remote desktop session", "block")
            );
            let fd: std::os::fd::OwnedFd = self.0.lock().unwrap().take().unwrap().into();
            fd.into()
        }
    }
    struct ScreenSaver(Arc<AtomicUsize>);
    #[zbus::interface(name = "org.freedesktop.ScreenSaver")]
    impl ScreenSaver {
        fn inhibit(&self, app: &str, reason: &str) -> u32 {
            assert_eq!((app, reason), ("VniLink", "Remote desktop session"));
            self.0.fetch_add(1, Ordering::SeqCst);
            62
        }
        fn un_inhibit(&self, cookie: u32) {
            assert_eq!(cookie, 62);
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    #[tokio::test]
    #[ignore = "run explicitly inside dbus-run-session; no real desktop service"]
    async fn private_bus_missing_gnome_uses_standard_screensaver_path() {
        let count = Arc::new(AtomicUsize::new(0));
        let _server = zbus::connection::Builder::session()
            .unwrap()
            .name("org.freedesktop.ScreenSaver")
            .unwrap()
            .serve_at("/org/freedesktop/ScreenSaver", ScreenSaver(count.clone()))
            .unwrap()
            .build()
            .await
            .unwrap();
        let lease = tokio::task::spawn_blocking(|| {
            LinuxPowerFactory
                .acquire(
                    PowerKind::Display,
                    &BackendStartup::new(Duration::from_secs(5)),
                )
                .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        tokio::task::spawn_blocking(move || drop(lease))
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while count.load(Ordering::SeqCst) != 0 {
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    /// The core releases an inhibitor on its runtime: on a worker thread, not
    /// in `spawn_blocking`. Released there, it used to panic in its destructor
    /// ("Cannot start a runtime from within a runtime") and abort the engine
    /// whenever a Linux viewer closed a remote screen.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "run explicitly inside dbus-run-session; no real desktop service"]
    async fn a_display_inhibition_is_released_from_the_runtime_itself() {
        let count = Arc::new(AtomicUsize::new(0));
        let _server = zbus::connection::Builder::session()
            .unwrap()
            .name("org.freedesktop.ScreenSaver")
            .unwrap()
            .serve_at("/org/freedesktop/ScreenSaver", ScreenSaver(count.clone()))
            .unwrap()
            .build()
            .await
            .unwrap();
        let lease = tokio::task::spawn_blocking(|| {
            LinuxPowerFactory
                .acquire(
                    PowerKind::Display,
                    &BackendStartup::new(Duration::from_secs(5)),
                )
                .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // Dropped here, on a runtime worker, as `Slot::clear` drops it.
        tokio::spawn(async move { drop(lease) }).await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while count.load(Ordering::SeqCst) != 0 {
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    #[tokio::test]
    #[ignore = "run explicitly inside dbus-run-session; validates D-Bus lifetime, not actual sleep"]
    async fn private_bus_cookie_and_fd_are_released_independently() {
        let display_count = Arc::new(AtomicUsize::new(0));
        let (mut reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let _server = zbus::connection::Builder::session()
            .unwrap()
            .name("org.gnome.SessionManager")
            .unwrap()
            .name("org.freedesktop.login1")
            .unwrap()
            .serve_at("/org/gnome/SessionManager", Display(display_count.clone()))
            .unwrap()
            .serve_at("/org/freedesktop/login1", System(Mutex::new(Some(writer))))
            .unwrap()
            .build()
            .await
            .unwrap();
        let (display, system) = tokio::task::spawn_blocking(|| {
            let startup = BackendStartup::new(Duration::from_secs(5));
            let display = LinuxPowerFactory::acquire_on(
                PowerKind::Display,
                Connection::session().unwrap(),
                &startup,
            )
            .unwrap();
            let system = LinuxPowerFactory::acquire_on(
                PowerKind::System,
                Connection::session().unwrap(),
                &startup,
            )
            .unwrap();
            (display, system)
        })
        .await
        .unwrap();
        use std::io::Read;
        let mut byte = [0];
        assert_eq!(display_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            reader.read(&mut byte).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        tokio::task::spawn_blocking(move || drop(display))
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while display_count.load(Ordering::SeqCst) != 0 {
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            reader.read(&mut byte).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        tokio::task::spawn_blocking(move || drop(system))
            .await
            .unwrap();
        assert_eq!(reader.read(&mut byte).unwrap(), 0);
    }
}
