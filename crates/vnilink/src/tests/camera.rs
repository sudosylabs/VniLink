use crate::camera::*;
#[test]
fn camera_approval_names_or_allows_every_camera() {
    let only = CameraApproval::Only(vec!["a".into()]);
    assert!(only.allows("a") && only.names("a"));
    assert!(!only.allows("b") && !only.names("b"));
    assert!(CameraApproval::All.allows("later") && !CameraApproval::All.names("later"));
    assert!(CameraApproval::All.validate().is_ok());
    assert!(CameraApproval::Only(vec![String::new()])
        .validate()
        .is_err());
    assert!(CameraApproval::Only(vec!["x".into(); 33])
        .validate()
        .is_err());
}
