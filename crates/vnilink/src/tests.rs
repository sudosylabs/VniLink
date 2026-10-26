#[path = "tests/admission.rs"]
mod admission_tests;
#[path = "tests/filesystem_location.rs"]
pub(crate) mod filesystem_location;
#[path = "tests/hostname.rs"]
mod hostname_tests;
#[path = "tests/identity.rs"]
mod identity_tests;
#[path = "tests/impaired_transfer.rs"]
mod impaired_transfer_tests;
#[path = "tests/job.rs"]
mod job_tests;
#[path = "tests/pairing.rs"]
mod pairing_tests;
#[path = "tests/persistence.rs"]
mod persistence_tests;
#[path = "tests/reconnect.rs"]
mod reconnect_tests;
#[path = "tests/support.rs"]
pub(crate) mod support;

#[path = "tests/services.rs"]
mod service_tests;

#[path = "tests/sharing.rs"]
mod sharing_tests;
