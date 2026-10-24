use crate::ScreenGrant;

/// A directory of its own under the system's temporary one, removed after.
struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("vnilink-grant-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// No file is no grant; a kept token is the next start's, replaced whole,
/// in a file only this user can read.
#[test]
fn a_kept_token_is_offered_on_the_next_start() {
    let scratch = Scratch::new("kept");
    let grant = ScreenGrant::at(scratch.0.join("screen-grant"));
    assert_eq!(grant.token(), None);
    grant.keep("first-token").unwrap();
    assert_eq!(grant.token().as_deref(), Some("first-token"));
    grant.keep("second-token").unwrap();
    assert_eq!(grant.token().as_deref(), Some("second-token"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(scratch.0.join("screen-grant"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // Only the token is left: no temporary file beside it.
    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 1);
}

/// What the portal never sends is not kept, and an empty file grants nothing.
#[test]
fn an_empty_or_malformed_token_is_no_grant() {
    let scratch = Scratch::new("empty");
    let path = scratch.0.join("screen-grant");
    let grant = ScreenGrant::at(&path);
    assert!(grant.keep("").is_err());
    assert!(grant.keep("two\nlines").is_err());
    assert_eq!(grant.token(), None);
    std::fs::write(&path, "\n").unwrap();
    assert_eq!(grant.token(), None);
}
