//! Forgets the local installation after preserving its database files.
//! Refuses to run unless `--confirm-forget-installation` is present.

fn main() {
    std::process::exit(vnilink_node::reset_installation_cli());
}
