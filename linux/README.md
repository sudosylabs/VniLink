# Native GNOME application

`vnilink-gnome` is a GTK 4 and libadwaita window linked directly to the VniLink core.

The installation record — this computer's identity and the computers paired with it — is one item in the session keyring (Secret Service, normally GNOME Keyring). The app does not write that record to a file. A locked keyring, a missing keyring, or more than one item is shown. Nothing is saved anywhere else.

The window follows the same shell as VniDrop: a toast overlay, a startup page, and a navigation split (sidebar about a third of the window, content clamped to 620). This computer creates a `.vnl` pairing file and shows three digits. The sidebar opens that file from another computer, checks the digits, and connects. Connect shows a second set of three digits, which this computer types to allow control. A narrow window collapses to one pane. An active session is a banner with Stop. This window does not draw a remote desktop.

## Run

GTK 4.10+, libadwaita 1.5+, and the workspace Rust toolchain:

```sh
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev
cargo run -p vnilink-gnome
```

The record is not printed.
