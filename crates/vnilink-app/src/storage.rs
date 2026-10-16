//! Persistence adapter between the core and the process that owns the bytes.
//!
//! The trait name is historical. Callers still pass the whole installation
//! record, but the native adapter no longer stores that record in the
//! platform credential store. It may keep the record in its own encrypted
//! database and use the credential store only for the key. `load` returns
//! `None` only when this adapter has just created an empty installation.
//! Corruption, a missing key, and an uncertain commit are errors.
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
        FfiError::CorruptRecord => vnilink::Error::CorruptRecord,
        FfiError::StoreRecoveryRequired => vnilink::Error::StoreRecoveryRequired,
        FfiError::StoreUnavailable => vnilink::Error::StoreUnavailable,
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

#[cfg(test)]
mod tests {
    use super::*;
    use vnilink::InstallationStore;

    struct Fault(FfiError);

    impl CredentialStore for Fault {
        fn load(&self) -> Result<Option<Vec<u8>>, FfiError> {
            Err(self.0.clone())
        }

        fn save(&self, _record: Vec<u8>) -> Result<(), FfiError> {
            Err(self.0.clone())
        }
    }

    #[test]
    fn native_store_preserves_corruption_and_recovery() {
        let cases = [
            (FfiError::StoreLocked, vnilink::Error::StoreLocked),
            (FfiError::CorruptRecord, vnilink::Error::CorruptRecord),
            (
                FfiError::StoreRecoveryRequired,
                vnilink::Error::StoreRecoveryRequired,
            ),
            (FfiError::StoreUnavailable, vnilink::Error::StoreUnavailable),
            (FfiError::ShutDown, vnilink::Error::StoreUnavailable),
        ];
        for (ffi, expected) in cases {
            let store = NativeStore(Arc::new(Fault(ffi)));
            assert_eq!(
                std::mem::discriminant(&store.load().unwrap_err()),
                std::mem::discriminant(&expected)
            );
            assert_eq!(
                std::mem::discriminant(&store.save(b"record").unwrap_err()),
                std::mem::discriminant(&expected)
            );
        }
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
