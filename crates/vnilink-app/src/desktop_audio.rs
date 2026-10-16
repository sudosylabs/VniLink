//! Optional desktop audio. Failure and listening controls never retire the desktop.
use crate::{
    AudioService, DesktopService, FfiAudioConfig, FfiAudioKind, FfiError, FfiServiceKind,
    NativeAudioPlayback, VnilinkCore,
};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopAudioPhase {
    Starting,
    Playing,
    Muted,
    Unavailable,
    Stopped,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopAudioStatus {
    pub phase: DesktopAudioPhase,
    pub enabled: bool,
    pub muted: bool,
    pub error: Option<FfiError>,
}
trait Playback: Send + Sync {
    fn close(&self);
    fn is_cancelled(&self) -> bool;
    fn error(&self) -> Option<FfiError>;
}
impl Playback for NativeAudioPlayback {
    fn close(&self) {
        self.close();
    }
    fn is_cancelled(&self) -> bool {
        self.is_cancelled()
    }
    fn error(&self) -> Option<FfiError> {
        self.error()
    }
}
type StartPlayback =
    dyn Fn(&VnilinkCore, Arc<AudioService>) -> Result<Arc<dyn Playback>, FfiError> + Send + Sync;
struct State {
    status: DesktopAudioStatus,
    revision: u64,
    closed: bool,
    owner: Option<(u64, u64)>,
    audio: Option<Arc<AudioService>>,
    playback: Option<Arc<dyn Playback>>,
}
/// Owned by the desktop, never by the currently selected workbench tab.
pub struct DesktopAudio {
    core: Arc<VnilinkCore>,
    state: Arc<Mutex<State>>,
}
impl DesktopAudio {
    pub fn start(core: Arc<VnilinkCore>, desktop: &Arc<DesktopService>) -> Arc<Self> {
        Self::start_with_playback(
            core,
            desktop,
            Arc::new(|core, audio| {
                core.start_default_native_audio_playback(audio)
                    .map(|playback| playback as Arc<dyn Playback>)
            }),
        )
    }
    fn start_with_playback(
        core: Arc<VnilinkCore>,
        desktop: &Arc<DesktopService>,
        start_playback: Arc<StartPlayback>,
    ) -> Arc<Self> {
        let state = Arc::new(Mutex::new(State {
            status: DesktopAudioStatus {
                phase: DesktopAudioPhase::Starting,
                enabled: true,
                muted: false,
                error: None,
            },
            revision: 0,
            closed: false,
            owner: None,
            audio: None,
            playback: None,
        }));
        let result = Arc::new(Self {
            core: core.clone(),
            state: state.clone(),
        });
        let desktop = Arc::downgrade(desktop);
        let worker_state = state.clone();
        if std::thread::Builder::new()
            .name("vnilink-desktop-audio".into())
            .spawn(move || run(core, desktop, worker_state, start_playback))
            .is_err()
        {
            let mut state = lock(&state);
            state.closed = true;
            state.status.enabled = false;
            state.status.phase = DesktopAudioPhase::Unavailable;
            state.status.error = Some(FfiError::Unsupported {
                message: "desktop audio worker".into(),
            });
        }
        result
    }
    pub fn status(&self) -> DesktopAudioStatus {
        lock(&self.state).status.clone()
    }
    pub fn set_muted(&self, muted: bool) {
        let mut state = lock(&self.state);
        if state.closed {
            return;
        }
        state.status.muted = muted;
        if let Some(audio) = &state.audio {
            audio.set_muted(muted);
        }
        if matches!(
            state.status.phase,
            DesktopAudioPhase::Playing | DesktopAudioPhase::Muted
        ) {
            state.status.phase = if muted {
                DesktopAudioPhase::Muted
            } else {
                DesktopAudioPhase::Playing
            };
        }
    }
    /// Enabling after an unavailable result is an explicit retry, not an automatic loop.
    pub fn set_enabled(&self, enabled: bool) {
        let owner = {
            let mut state = lock(&self.state);
            if state.closed
                || (state.status.enabled == enabled
                    && state.status.phase != DesktopAudioPhase::Unavailable)
            {
                return;
            }
            state.revision = state.revision.saturating_add(1);
            state.status.enabled = enabled;
            state.status.error = None;
            state.status.phase = if enabled {
                DesktopAudioPhase::Starting
            } else {
                DesktopAudioPhase::Stopped
            };
            retire(&mut state)
        };
        close_owner(&self.core, owner);
    }
    pub fn close(&self) {
        let owner = {
            let mut state = lock(&self.state);
            state.closed = true;
            state.status.enabled = false;
            state.status.phase = DesktopAudioPhase::Stopped;
            retire(&mut state)
        };
        close_owner(&self.core, owner);
    }
}
impl Drop for DesktopAudio {
    fn drop(&mut self) {
        self.close();
    }
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
fn retire(state: &mut State) -> Option<(u64, u64)> {
    if let Some(playback) = state.playback.take() {
        playback.close();
    }
    if let Some(audio) = state.audio.take() {
        audio.close();
    }
    state.owner.take()
}
fn close_owner(core: &VnilinkCore, owner: Option<(u64, u64)>) {
    if let Some((id, generation)) = owner {
        if core.close_controller_service(id, generation).is_ok() {
            let _ = core.forget_controller_service(id);
        }
    }
}
fn desktop_active(desktop: &Weak<DesktopService>) -> bool {
    desktop
        .upgrade()
        .is_some_and(|desktop| !desktop.is_cancelled())
}

fn run(
    core: Arc<VnilinkCore>,
    weak_desktop: Weak<DesktopService>,
    state: Arc<Mutex<State>>,
    start_playback: Arc<StartPlayback>,
) {
    let mut attempted = None;
    loop {
        let Some(desktop) = weak_desktop
            .upgrade()
            .filter(|desktop| !desktop.is_cancelled())
        else {
            break;
        };
        let revision = {
            let mut state = lock(&state);
            if state.closed {
                break;
            }
            if let Some(playback) = state.playback.clone() {
                if playback.is_cancelled() {
                    state.status.phase = DesktopAudioPhase::Unavailable;
                    state.status.error = playback.error().or(Some(FfiError::NoSession));
                    let owner = retire(&mut state);
                    drop(state);
                    close_owner(&core, owner);
                    continue;
                }
            }
            if !state.status.enabled || attempted == Some(state.revision) {
                drop(state);
                drop(desktop);
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            state.revision
        };
        attempted = Some(revision);
        let info = desktop.info();
        drop(desktop);
        let outcome = (|| {
            let devices = core.remote_audio_devices(
                info.connection_id,
                info.generation,
                FfiAudioKind::System,
            )?;
            if !desktop_active(&weak_desktop) {
                return Err(FfiError::NoSession);
            }
            let device = devices
                .iter()
                .find(|device| device.id == "vnilink:default-system-audio")
                .or_else(|| devices.first())
                .ok_or(FfiError::Unsupported {
                    message: "remote system audio source".into(),
                })?;
            let owner = core.attach_controller_service(
                info.connection_id,
                info.generation,
                FfiServiceKind::SystemAudio,
            )?;
            {
                let mut state = lock(&state);
                if state.closed
                    || !state.status.enabled
                    || state.revision != revision
                    || !desktop_active(&weak_desktop)
                {
                    drop(state);
                    close_owner(&core, Some((owner.id, owner.generation)));
                    return Err(FfiError::NoSession);
                }
                state.owner = Some((owner.id, owner.generation));
            }
            let audio = core.open_audio_service(
                owner.id,
                owner.generation,
                FfiAudioConfig {
                    device_id: device.id.clone(),
                    kind: FfiAudioKind::System,
                    channels: 2,
                    voice: false,
                },
            )?;
            {
                let mut state = lock(&state);
                if state.closed || state.revision != revision || !desktop_active(&weak_desktop) {
                    audio.close();
                    return Err(FfiError::NoSession);
                }
                audio.set_muted(state.status.muted);
                state.audio = Some(audio.clone());
            }
            let playback = start_playback(&core, audio)?;
            let mut state = lock(&state);
            if state.closed || state.revision != revision || !desktop_active(&weak_desktop) {
                playback.close();
                return Err(FfiError::NoSession);
            }
            state.playback = Some(playback);
            state.status.phase = if state.status.muted {
                DesktopAudioPhase::Muted
            } else {
                DesktopAudioPhase::Playing
            };
            Ok(())
        })();
        if let Err(error) = outcome {
            let mut state = lock(&state);
            if state.revision == revision && !state.closed {
                state.status.phase = DesktopAudioPhase::Unavailable;
                state.status.error = Some(error);
                let owner = retire(&mut state);
                drop(state);
                close_owner(&core, owner);
            }
        }
    }
    let owner = {
        let mut state = lock(&state);
        state.closed = true;
        state.status.enabled = false;
        state.status.phase = DesktopAudioPhase::Stopped;
        retire(&mut state)
    };
    close_owner(&core, owner);
}

#[cfg(test)]
#[path = "desktop_audio_tests.rs"]
mod tests;
