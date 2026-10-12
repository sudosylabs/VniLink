//! Node-API control bridge. No media transport is exposed.
//! All core calls run on libuv workers, outside the JavaScript event loop.
use napi::{bindgen_prelude::AsyncTask, Env, Error, Result, Status, Task};
use napi_derive::napi;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use vnilink_app::{CredentialStore, FfiError, VnilinkCore};

struct NativeStore;
impl CredentialStore for NativeStore {
    fn load(&self) -> std::result::Result<Option<Vec<u8>>, FfiError> {
        let entry =
            keyring::Entry::new("com.vnilink.installation", "primary").map_err(store_error)?;
        match entry.get_secret() {
            Ok(bytes) => Ok(Some(bytes)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(store_error(error)),
        }
    }
    fn save(&self, record: Vec<u8>) -> std::result::Result<(), FfiError> {
        keyring::Entry::new("com.vnilink.installation", "primary")
            .and_then(|entry| entry.set_secret(&record))
            .map_err(store_error)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum Command {
    Snapshot,
    CreateInvitation {
        #[serde(rename = "ttlSeconds")]
        ttl_seconds: u32,
    },
    AcceptInvitation {
        token: String,
    },
    ExportPairingFile {
        #[serde(rename = "ttlSeconds")]
        ttl_seconds: u32,
        #[serde(rename = "allowControl")]
        allow_control: bool,
    },
    OpenPairingFile {
        bytes: Vec<u8>,
    },
    ConfirmPairing {
        #[serde(rename = "previewId")]
        preview_id: String,
        digits: String,
    },
    Authorize {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
        view: bool,
        control: bool,
        clipboard: bool,
        transfer: bool,
    },
    Revoke {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
    Unpair {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
    SetPeerName {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
        name: String,
    },
    Shutdown,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Reply {
    Snapshot {
        snapshot: vnilink_app::FfiSnapshot,
    },
    Invitation {
        token: String,
    },
    PairingFile {
        file: vnilink_app::FfiPairingFile,
    },
    PairingPreview {
        preview: vnilink_app::FfiPairingPreview,
    },
    Device {
        device: vnilink_app::FfiDeviceInfo,
    },
    Done,
}

// Commands serialize state mutations and keychain writes. Shutdown marks the
// instance closed; no later task can restart or mutate the old installation.
struct State {
    core: VnilinkCore,
    closed: bool,
}
#[napi]
pub struct Engine {
    state: Arc<Mutex<State>>,
}

pub struct OpenTask {
    temporary: bool,
}
impl Task for OpenTask {
    type Output = VnilinkCore;
    type JsValue = Engine;
    fn compute(&mut self) -> Result<Self::Output> {
        let result = if self.temporary {
            VnilinkCore::create(true, vec![])
        } else {
            VnilinkCore::open_persistent(Box::new(NativeStore))
        };
        result.map_err(core_error)
    }
    fn resolve(&mut self, _: Env, core: Self::Output) -> Result<Engine> {
        Ok(Engine {
            state: Arc::new(Mutex::new(State {
                core,
                closed: false,
            })),
        })
    }
}
#[napi]
pub fn open_engine() -> AsyncTask<OpenTask> {
    AsyncTask::new(OpenTask { temporary: false })
}
// Explicit isolated test/preview mode; never a fallback for unavailable storage.
#[napi]
pub fn open_temporary_engine() -> AsyncTask<OpenTask> {
    AsyncTask::new(OpenTask { temporary: true })
}

pub struct CommandTask {
    state: Arc<Mutex<State>>,
    command: Command,
}
#[napi]
impl Engine {
    #[napi]
    pub fn execute(&self, command: String) -> Result<AsyncTask<CommandTask>> {
        if command.len() > 256 * 1024 {
            return Err(Error::new(Status::InvalidArg, "Command too large"));
        }
        let decoded: Command = serde_json::from_str(&command)
            .map_err(|_| Error::new(Status::InvalidArg, "Invalid engine command"))?;
        // Serde unit variants otherwise accept extra fields despite the enum's
        // deny_unknown_fields setting. Reject those before entering the core.
        let raw: serde_json::Value = serde_json::from_str(&command)
            .map_err(|_| Error::new(Status::InvalidArg, "Invalid engine command"))?;
        if serde_json::to_value(&decoded)
            .map_err(|_| Error::from_reason("Invalid engine command"))?
            != raw
        {
            return Err(Error::new(Status::InvalidArg, "Invalid engine command"));
        }
        let command = decoded;
        Ok(AsyncTask::new(CommandTask {
            state: self.state.clone(),
            command,
        }))
    }
}
impl Task for CommandTask {
    type Output = String;
    type JsValue = String;
    fn compute(&mut self) -> Result<String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::from_reason("Engine unavailable"))?;
        if state.closed {
            return Err(core_error(FfiError::ShutDown));
        }
        let core = &state.core;
        let reply = match &self.command {
            Command::Snapshot => Reply::Snapshot {
                snapshot: core.snapshot().map_err(core_error)?,
            },
            Command::CreateInvitation { ttl_seconds } => Reply::Invitation {
                token: core
                    .create_invitation(u64::from(*ttl_seconds))
                    .map_err(core_error)?,
            },
            Command::AcceptInvitation { token } => Reply::Device {
                device: core.accept_invitation(token.clone()).map_err(core_error)?,
            },
            Command::ExportPairingFile {
                ttl_seconds,
                allow_control,
            } => Reply::PairingFile {
                file: core
                    .export_pairing_file(u64::from(*ttl_seconds), *allow_control)
                    .map_err(core_error)?,
            },
            Command::OpenPairingFile { bytes } => Reply::PairingPreview {
                preview: core.open_pairing_file(bytes.clone()).map_err(core_error)?,
            },
            Command::ConfirmPairing { preview_id, digits } => {
                let id = preview_id
                    .parse::<u64>()
                    .map_err(|_| Error::new(Status::InvalidArg, "Invalid preview ID"))?;
                Reply::Device {
                    device: core
                        .confirm_pairing(id, digits.clone())
                        .map_err(core_error)?,
                }
            }
            Command::Authorize {
                endpoint_id,
                view,
                control,
                clipboard,
                transfer,
            } => {
                core.authorize(endpoint_id.clone(), *view, *control, *clipboard, *transfer)
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::Revoke { endpoint_id } => {
                core.revoke(endpoint_id.clone()).map_err(core_error)?;
                Reply::Done
            }
            Command::Unpair { endpoint_id } => {
                core.unpair(endpoint_id.clone()).map_err(core_error)?;
                Reply::Done
            }
            Command::SetPeerName { endpoint_id, name } => {
                core.set_peer_pet_name(endpoint_id.clone(), Some(name.clone()))
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::Shutdown => {
                state.closed = true;
                state.core.shutdown();
                Reply::Done
            }
        };
        serde_json::to_string(&reply).map_err(|_| Error::from_reason("Cannot encode engine reply"))
    }
    fn resolve(&mut self, _: Env, output: String) -> Result<String> {
        Ok(output)
    }
}
fn core_error(error: FfiError) -> Error {
    Error::from_reason(error.to_string())
}

fn store_error(error: keyring::Error) -> FfiError {
    match error {
        keyring::Error::NoStorageAccess(_) => FfiError::StoreLocked,
        _ => FfiError::StoreUnavailable,
    }
}
