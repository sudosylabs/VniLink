//! Authorized incoming clipboard writes are acknowledged after GTK applies
//! them. Outgoing clipboard sharing is explicit, never an automatic monitor.
use super::{preferences, tasks};
use adw::prelude::*;
use std::sync::{mpsc, Arc};
use std::time::Duration;
struct Write {
    text: String,
    reply: mpsc::Sender<Result<(), vnilink::Error>>,
}
struct Adapter(mpsc::SyncSender<Write>);
impl vnilink::HostClipboard for Adapter {
    fn set_text(&mut self, text: &str) -> Result<(), vnilink::Error> {
        let (reply, done) = mpsc::channel();
        self.0
            .try_send(Write {
                text: text.into(),
                reply,
            })
            .map_err(|_| vnilink::Error::Busy)?;
        done.recv_timeout(Duration::from_secs(2))
            .map_err(|_| vnilink::Error::ShutDown)?
    }
}
pub(crate) fn install(window: &adw::ApplicationWindow, core: Arc<vnilink::Core>) {
    let (sender, receiver) = mpsc::sync_channel::<Write>(1);
    let window = window.downgrade();
    gtk::glib::timeout_add_local(Duration::from_millis(16), move || {
        let Some(window) = window.upgrade().filter(|w| w.get_visible()) else {
            return gtk::glib::ControlFlow::Break;
        };
        if let Ok(write) = receiver.try_recv() {
            window.clipboard().set_text(&write.text);
            let _ = write.reply.send(Ok(()));
        }
        gtk::glib::ControlFlow::Continue
    });
    tasks::run(
        move || {
            core.set_clipboard(Box::new(Adapter(sender)))
                .map_err(|e| e.to_string())
        },
        |_| {},
    );
}
pub(crate) fn send(
    window: &adw::ApplicationWindow,
    core: Arc<vnilink::Core>,
    id: u64,
    generation: u64,
) {
    let win = window.clone();
    window.clipboard().read_text_async(
        None::<&gtk::gio::Cancellable>,
        move |result| match result {
            Ok(Some(text)) => tasks::run(
                move || {
                    core.submit_clipboard(id, generation, &text)
                        .map_err(|e| e.to_string())
                },
                move |r| {
                    if let Err(e) = r {
                        preferences::report(&win, &e);
                    }
                },
            ),
            Ok(None) => preferences::report(&win, "The local clipboard has no text to send."),
            Err(_) => preferences::report(&win, "The local clipboard could not be read."),
        },
    );
}
pub(crate) fn receive(window: &adw::ApplicationWindow, core: Arc<vnilink::Core>) {
    let win = window.clone();
    tasks::run(
        move || core.clipboard_text().map_err(|e| e.to_string()),
        move |r| match r {
            Ok(Some(text)) => win.clipboard().set_text(&text),
            Ok(None) => preferences::report(
                &win,
                "The remote computer has not shared any clipboard text.",
            ),
            Err(e) => preferences::report(&win, &e),
        },
    );
}
