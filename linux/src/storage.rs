//! The installation record in the session keyring.
//!
//! The bytes hold the installation secret and the paired computers. This
//! module does not log them and does not write them to a file. The core calls
//! the store on the caller's thread, outside its own runtime. Secret Service
//! uses a blocking client, so it must stay on that caller thread.

use std::sync::{Arc, Mutex};

use vnilink::{Error, InstallationStore};

const APPLICATION_ID: &str = "com.vnilink.VniLink";
const RECORD: &str = "installation";
const LABEL: &str = "VniLink installation";

trait Keyring: Send + Sync {
    fn read(&self) -> Result<Option<Vec<u8>>, Error>;
    fn write(&self, record: &[u8]) -> Result<(), Error>;
}

pub struct SecretStore {
    keyring: Arc<dyn Keyring>,
}

impl SecretStore {
    pub fn connect() -> Result<Self, Error> {
        Ok(Self {
            keyring: Arc::new(SystemKeyring::connect()?),
        })
    }

    fn with_keyring(keyring: Arc<dyn Keyring>) -> Self {
        Self { keyring }
    }
}

impl InstallationStore for SecretStore {
    fn load(&self) -> Result<Option<Vec<u8>>, Error> {
        self.keyring.read()
    }

    fn save(&self, record: &[u8]) -> Result<(), Error> {
        self.keyring.write(record)
    }
}

impl std::fmt::Debug for SecretStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretStore")
    }
}

struct SystemKeyring;

impl SystemKeyring {
    fn connect() -> Result<Self, Error> {
        #[cfg(target_os = "linux")]
        {
            use secret_service::blocking::SecretService;
            use secret_service::EncryptionType;
            SecretService::connect(EncryptionType::Dh).map_err(map_error)?;
            return Ok(Self);
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(Error::StoreUnavailable)
        }
    }
}

impl Keyring for SystemKeyring {
    fn read(&self) -> Result<Option<Vec<u8>>, Error> {
        #[cfg(target_os = "linux")]
        {
            use secret_service::blocking::SecretService;
            use secret_service::EncryptionType;

            let service = SecretService::connect(EncryptionType::Dh).map_err(map_error)?;
            let found = service.search_items(attributes()).map_err(map_error)?;
            if !found.locked.is_empty() {
                return Err(Error::StoreLocked);
            }
            let mut items = found.unlocked.into_iter();
            let Some(item) = items.next() else {
                return Ok(None);
            };
            if items.next().is_some() {
                return Err(Error::CorruptRecord);
            }
            return item.get_secret().map(Some).map_err(map_error);
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(Error::StoreUnavailable)
        }
    }

    fn write(&self, record: &[u8]) -> Result<(), Error> {
        #[cfg(target_os = "linux")]
        {
            use secret_service::blocking::SecretService;
            use secret_service::EncryptionType;

            let service = SecretService::connect(EncryptionType::Dh).map_err(map_error)?;
            let collection = service.get_default_collection().map_err(map_error)?;
            if collection.is_locked().map_err(map_error)? {
                return Err(Error::StoreLocked);
            }
            collection
                .create_item(
                    LABEL,
                    attributes(),
                    record,
                    true,
                    "application/octet-stream",
                )
                .map_err(map_error)?;
            return Ok(());
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = record;
            Err(Error::StoreUnavailable)
        }
    }
}

#[cfg(target_os = "linux")]
fn attributes() -> std::collections::HashMap<&'static str, &'static str> {
    std::collections::HashMap::from([("application", APPLICATION_ID), ("vnilink-record", RECORD)])
}

#[cfg(target_os = "linux")]
fn map_error(error: secret_service::Error) -> Error {
    match error {
        secret_service::Error::Locked | secret_service::Error::Prompt => Error::StoreLocked,
        secret_service::Error::NoResult => Error::StoreUnavailable,
        _ => Error::StoreUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    enum Slot {
        Empty,
        Record(Vec<u8>),
        Locked,
    }

    struct MemoryKeyring {
        record: Mutex<Slot>,
    }

    impl Keyring for MemoryKeyring {
        fn read(&self) -> Result<Option<Vec<u8>>, Error> {
            match &*self.record.lock().expect("keyring") {
                Slot::Empty => Ok(None),
                Slot::Record(bytes) => Ok(Some(bytes.clone())),
                Slot::Locked => Err(Error::StoreLocked),
            }
        }

        fn write(&self, record: &[u8]) -> Result<(), Error> {
            let mut slot = self.record.lock().expect("keyring");
            if matches!(*slot, Slot::Locked) {
                return Err(Error::StoreLocked);
            }
            *slot = Slot::Record(record.to_vec());
            Ok(())
        }
    }

    #[test]
    fn a_new_keyring_has_no_record_and_then_keeps_one() {
        let store = SecretStore::with_keyring(Arc::new(MemoryKeyring {
            record: Mutex::new(Slot::Empty),
        }));
        assert_eq!(store.load().unwrap(), None);
        store.save(b"record").unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some(b"record".as_slice()));
        store.save(b"next").unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some(b"next".as_slice()));
    }

    #[test]
    fn a_locked_keyring_is_not_replaced_with_a_file() {
        let store = SecretStore::with_keyring(Arc::new(MemoryKeyring {
            record: Mutex::new(Slot::Locked),
        }));
        assert!(matches!(store.load(), Err(Error::StoreLocked)));
        assert!(matches!(store.save(b"record"), Err(Error::StoreLocked)));
    }
}
