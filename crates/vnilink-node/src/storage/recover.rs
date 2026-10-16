//! Developer reset for one installation directory.
//!
//! The database, journal, and any unexpected WAL or SHM files are renamed into
//! a private recovery directory on the same file system. The installation key
//! is deleted only after that preservation and its directory sync succeed.
//! `installation.lock` stays. Quarantined ciphertext is not a working copy
//! once the key is gone.

use std::fs;
use std::io::ErrorKind;

use super::key::KeyVault;
use super::paths::{self, InstallationPaths};
use super::StoreFailure;

#[derive(Debug)]
pub(crate) struct Recovered {
    pub(crate) moved: Vec<String>,
    /// Read by the reset command and by tests. Normal application builds omit both.
    #[cfg_attr(not(any(test, feature = "reset-installation")), allow(dead_code))]
    pub(crate) key_deleted: bool,
}

#[derive(Debug)]
pub(crate) enum RecoverFailure {
    Stopped {
        step: &'static str,
    },
    Partial {
        step: &'static str,
        moved: Vec<String>,
    },
}

impl std::fmt::Display for RecoverFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped { step } => {
                write!(formatter, "stopped at {step} before deleting the key")
            }
            Self::Partial { step, moved } => write!(
                formatter,
                "stopped at {step} after moving {}; the key was kept",
                moved.len()
            ),
        }
    }
}

pub(crate) fn recover(
    paths: &InstallationPaths,
    vault: &dyn KeyVault,
) -> Result<Recovered, RecoverFailure> {
    let _lease = paths::InstallationLease::acquire(paths)
        .map_err(|_| RecoverFailure::Stopped { step: "lease" })?;
    paths::validate_dir(&paths.engine_dir)
        .map_err(|_| RecoverFailure::Stopped { step: "namespace" })?;
    let artifacts = [&paths.database, &paths.journal, &paths.wal, &paths.shm];
    let mut present = Vec::new();
    for path in artifacts {
        match paths::file_len(path) {
            Ok(None) => {}
            Ok(Some(_)) => {
                paths::validate_file(path)
                    .map_err(|_| RecoverFailure::Stopped { step: "validate" })?;
                present.push(path);
            }
            Err(StoreFailure::Corrupt) | Err(StoreFailure::Unavailable) => {
                return Err(RecoverFailure::Stopped { step: "validate" });
            }
            Err(_) => return Err(RecoverFailure::Stopped { step: "validate" }),
        }
    }
    if present.is_empty() {
        delete_key(vault, Vec::new())?;
        return Ok(Recovered {
            moved: Vec::new(),
            key_deleted: true,
        });
    }
    let recovery_dir = paths.engine_dir.join(format!(
        "installation-recovery-{}",
        recovery_id().map_err(|_| RecoverFailure::Stopped { step: "preserve" })?
    ));
    paths::ensure_private_dir(&recovery_dir)
        .map_err(|_| RecoverFailure::Stopped { step: "preserve" })?;
    let mut moved = Vec::new();
    for path in &present {
        let name =
            path.file_name()
                .and_then(|name| name.to_str())
                .ok_or(RecoverFailure::Partial {
                    step: "preserve",
                    moved: moved.clone(),
                })?;
        let target = recovery_dir.join(name);
        if let Err(error) = fs::rename(path, &target) {
            let step = if error.kind() == ErrorKind::CrossesDevices {
                "cross-device"
            } else {
                "preserve"
            };
            return Err(RecoverFailure::Partial { step, moved });
        }
        moved.push(name.to_owned());
    }
    for name in &moved {
        if paths::sync_file(&recovery_dir.join(name)).is_err() {
            return Err(RecoverFailure::Partial {
                step: "sync",
                moved,
            });
        }
    }
    if paths::sync_dir(&recovery_dir).is_err() || paths::sync_dir(&paths.engine_dir).is_err() {
        return Err(RecoverFailure::Partial {
            step: "sync",
            moved,
        });
    }
    delete_key(vault, moved.clone())?;
    Ok(Recovered {
        moved,
        key_deleted: true,
    })
}

fn delete_key(vault: &dyn KeyVault, moved: Vec<String>) -> Result<(), RecoverFailure> {
    vault.delete().map_err(|_| {
        if moved.is_empty() {
            RecoverFailure::Stopped { step: "delete-key" }
        } else {
            RecoverFailure::Partial {
                step: "delete-key",
                moved,
            }
        }
    })
}

fn recovery_id() -> Result<String, StoreFailure> {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).map_err(|_| StoreFailure::Unavailable)?;
    let mut text = String::with_capacity(16);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    Ok(text)
}
