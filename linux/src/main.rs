mod present;
mod secret;

#[cfg(target_os = "linux")]
mod app;
#[cfg(target_os = "linux")]
mod home;

#[cfg(target_os = "linux")]
fn main() -> gtk::glib::ExitCode {
    app::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("The GNOME application runs on Linux.");
    std::process::exit(1);
}
