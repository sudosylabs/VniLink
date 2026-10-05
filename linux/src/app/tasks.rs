//! Marshal blocking core calls back to GTK without running them on its thread.
use gtk::glib::{self, ControlFlow};
use std::time::Duration;

pub(crate) fn run<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: impl FnOnce(Result<T, String>) + 'static,
) {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    let mut done = Some(done);
    glib::timeout_add_local(Duration::from_millis(30), move || {
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return ControlFlow::Continue,
            Err(_) => Err("The operation could not finish.".into()),
        };
        if let Some(done) = done.take() {
            done(result);
        }
        ControlFlow::Break
    });
}
