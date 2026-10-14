//! Native credential stores keep the opaque record; Rust owns its format.
use crate::FfiError;
use std::sync::Arc;

pub trait CredentialStore: Send + Sync {
    fn load(&self) -> Result<Option<Vec<u8>>, FfiError>;
    fn save(&self, record: Vec<u8>) -> Result<(), FfiError>;
}

pub(crate) struct NativeStore(pub Arc<dyn CredentialStore>);
fn store_error(error: FfiError) -> vnilink::Error {
    match error {
        FfiError::StoreLocked => vnilink::Error::StoreLocked,
        _ => vnilink::Error::StoreUnavailable,
    }
}
impl vnilink::InstallationStore for NativeStore {
    fn load(&self) -> Result<Option<Vec<u8>>, vnilink::Error> {
        self.0.load().map_err(store_error)
    }
    fn save(&self, record: &[u8]) -> Result<(), vnilink::Error> {
        self.0.save(record.to_vec()).map_err(store_error)
    }
}

#[derive(Clone, Debug)]
pub struct FfiRelay {
    pub url: String,
    pub authenticated: bool,
}
#[derive(Clone)]
pub struct FfiRelayInput {
    pub url: String,
    pub authenticated: bool,
    /// Empty preserves the existing credential for the same URL.
    pub replacement_token: String,
}
