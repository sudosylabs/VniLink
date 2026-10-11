//! Native service ownership. Backend operations remain in their own modules.

use crate::{error::map_error, FfiError, FfiSessionInfo, VnilinkCore};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiServiceKind {
    Desktop,
    Files,
    Terminal,
    Camera,
    SystemAudio,
    Microphone,
    System,
}

impl From<FfiServiceKind> for vnilink::ServiceKind {
    fn from(value: FfiServiceKind) -> Self {
        match value {
            FfiServiceKind::Desktop => Self::Desktop,
            FfiServiceKind::Files => Self::Files,
            FfiServiceKind::Terminal => Self::Terminal,
            FfiServiceKind::Camera => Self::Camera,
            FfiServiceKind::SystemAudio => Self::SystemAudio,
            FfiServiceKind::Microphone => Self::Microphone,
            FfiServiceKind::System => Self::System,
        }
    }
}

impl From<vnilink::ServiceKind> for FfiServiceKind {
    fn from(value: vnilink::ServiceKind) -> Self {
        match value {
            vnilink::ServiceKind::Desktop => Self::Desktop,
            vnilink::ServiceKind::Files => Self::Files,
            vnilink::ServiceKind::Terminal => Self::Terminal,
            vnilink::ServiceKind::Camera => Self::Camera,
            vnilink::ServiceKind::SystemAudio => Self::SystemAudio,
            vnilink::ServiceKind::Microphone => Self::Microphone,
            vnilink::ServiceKind::System => Self::System,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiDesktopControlState {
    Viewing,
    Controlling,
    Contended,
}
impl From<vnilink::DesktopControlState> for FfiDesktopControlState {
    fn from(value: vnilink::DesktopControlState) -> Self {
        match value {
            vnilink::DesktopControlState::Viewing => Self::Viewing,
            vnilink::DesktopControlState::Controlling => Self::Controlling,
            vnilink::DesktopControlState::Contended => Self::Contended,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiCloseCause {
    OwnerClose,
    ParentLoss,
    Revoked,
    Replaced,
    DeviceLoss,
    Shutdown,
    PeerClose,
    IdleExpired,
}

impl From<vnilink::CloseCause> for FfiCloseCause {
    fn from(value: vnilink::CloseCause) -> Self {
        match value {
            vnilink::CloseCause::OwnerClose => Self::OwnerClose,
            vnilink::CloseCause::ParentLoss => Self::ParentLoss,
            vnilink::CloseCause::Revoked => Self::Revoked,
            vnilink::CloseCause::Replaced => Self::Replaced,
            vnilink::CloseCause::DeviceLoss => Self::DeviceLoss,
            vnilink::CloseCause::Shutdown => Self::Shutdown,
            vnilink::CloseCause::PeerClose => Self::PeerClose,
            vnilink::CloseCause::IdleExpired => Self::IdleExpired,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FfiServicePhase {
    Waiting,
    Active,
    Suspended,
    Closed { cause: FfiCloseCause },
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiServiceInfo {
    pub id: u64,
    pub connection_id: u64,
    pub generation: u64,
    pub peer_id: String,
    pub kind: FfiServiceKind,
    pub phase: FfiServicePhase,
}

impl From<vnilink::ServiceInfo> for FfiServiceInfo {
    fn from(value: vnilink::ServiceInfo) -> Self {
        Self {
            id: value.id,
            connection_id: value.connection_id,
            generation: value.generation,
            peer_id: value.peer_id,
            kind: value.kind.into(),
            phase: match value.phase {
                vnilink::ServicePhase::Waiting => FfiServicePhase::Waiting,
                vnilink::ServicePhase::Active => FfiServicePhase::Active,
                vnilink::ServicePhase::Suspended => FfiServicePhase::Suspended,
                vnilink::ServicePhase::Closed(cause) => FfiServicePhase::Closed {
                    cause: cause.into(),
                },
                vnilink::ServicePhase::Failed(message) => FfiServicePhase::Failed { message },
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FfiServiceSupport {
    pub desktop: bool,
    pub files: bool,
    pub terminal: bool,
    pub camera: bool,
    pub system_audio: bool,
    pub microphone: bool,
    pub system_actions: bool,
}

/// Permissions this installation grants the paired peer; not remote support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FfiServicePermissions {
    pub recording: bool,
    pub desktop: bool,
    pub files: bool,
    pub terminal: bool,
    pub camera: bool,
    pub system_audio: bool,
    pub microphone: bool,
    pub system_actions: bool,
}

/// Pollable cancellation for one service generation. An old lease stays
/// cancelled after reconnection. Holding it never keeps the core alive.
#[derive(uniffi::Object)]
pub struct ServiceLeasePoller {
    inner: vnilink::ServiceLease,
}

#[uniffi::export]
impl ServiceLeasePoller {
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
    pub fn info(&self) -> FfiServiceInfo {
        self.inner.info().clone().into()
    }
}

#[uniffi::export]
impl VnilinkCore {
    pub fn transfer_desktop_control(
        &self,
        from_id: u64,
        from_generation: u64,
        to_id: u64,
        to_generation: u64,
    ) -> Result<(), FfiError> {
        self.inner
            .transfer_desktop_control(from_id, from_generation, to_id, to_generation)
            .map_err(map_error)
    }
    pub fn host_desktop_active(&self, id: u64, generation: u64) -> Result<bool, FfiError> {
        self.inner
            .host_desktop_active(id, generation)
            .map_err(map_error)
    }
    pub fn begin_connect_services(&self, endpoint_id: String) -> Result<FfiSessionInfo, FfiError> {
        self.inner
            .begin_connect_services(&endpoint_id)
            .map(crate::types::session_info)
            .map_err(map_error)
    }

    pub fn authorize_recording(&self, endpoint_id: String, allowed: bool) -> Result<(), FfiError> {
        self.inner
            .authorize_recording(&endpoint_id, allowed)
            .map_err(map_error)
    }
    pub fn service_permissions(
        &self,
        endpoint_id: String,
    ) -> Result<FfiServicePermissions, FfiError> {
        self.inner
            .service_permissions(&endpoint_id)
            .map(|p| FfiServicePermissions {
                desktop: p.desktop,
                files: p.files,
                terminal: p.terminal,
                camera: p.camera,
                system_audio: p.system_audio,
                microphone: p.microphone,
                recording: p.recording,
                system_actions: p.system_actions,
            })
            .map_err(map_error)
    }
    pub fn authorize_service(
        &self,
        endpoint_id: String,
        kind: FfiServiceKind,
        allowed: bool,
    ) -> Result<(), FfiError> {
        self.inner
            .authorize_service(&endpoint_id, kind.into(), allowed)
            .map_err(map_error)
    }

    pub fn files_service_upload(
        &self,
        id: u64,
        generation: u64,
        src_root: u64,
        src_rel: String,
        dst_root: u64,
        dst_rel: String,
    ) -> Result<u64, FfiError> {
        self.inner
            .files_service_upload(id, generation, src_root, &src_rel, dst_root, &dst_rel)
            .map_err(map_error)
    }
    pub fn files_service_download(
        &self,
        id: u64,
        generation: u64,
        src_root: u64,
        src_rel: String,
        dst_root: u64,
        dst_rel: String,
    ) -> Result<u64, FfiError> {
        self.inner
            .files_service_download(id, generation, src_root, &src_rel, dst_root, &dst_rel)
            .map_err(map_error)
    }
    pub fn files_service_jobs(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<Vec<crate::FfiJobSnapshot>, FfiError> {
        self.inner
            .files_service_jobs(id, generation)
            .map(|jobs| jobs.into_iter().map(crate::files::job_snapshot).collect())
            .map_err(map_error)
    }

    pub fn remote_service_support(
        &self,
        connection_id: u64,
        generation: u64,
    ) -> Result<FfiServiceSupport, FfiError> {
        let support = self
            .inner
            .remote_service_support(connection_id, generation)
            .map_err(map_error)?;
        Ok(FfiServiceSupport {
            desktop: support.desktop,
            files: support.files,
            terminal: support.terminal,
            camera: support.camera,
            system_audio: support.system_audio,
            microphone: support.microphone,
            system_actions: support.system_actions,
        })
    }

    pub fn hosted_services(&self) -> Result<Vec<FfiServiceInfo>, FfiError> {
        self.inner
            .hosted_services()
            .map(|records| records.into_iter().map(Into::into).collect())
            .map_err(map_error)
    }

    pub fn close_hosted_service(&self, id: u64, generation: u64) -> Result<(), FfiError> {
        self.inner
            .close_hosted_service(id, generation)
            .map_err(map_error)
    }

    pub fn open_files_service(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<Arc<FilesService>, FfiError> {
        self.inner
            .open_files_service(id, generation)
            .map(|inner| {
                Arc::new(FilesService {
                    inner,
                    core: Arc::downgrade(&self.inner),
                })
            })
            .map_err(map_error)
    }

    pub fn open_desktop_service(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<Arc<DesktopService>, FfiError> {
        self.inner
            .open_desktop_service(id, generation)
            .map(|inner| {
                Arc::new(DesktopService {
                    inner: Arc::new(inner),
                    core: Arc::downgrade(&self.inner),
                })
            })
            .map_err(map_error)
    }
    pub fn controller_service_support(
        &self,
        connection_id: u64,
    ) -> Result<FfiServiceSupport, FfiError> {
        let support = self
            .inner
            .controller_service_support(connection_id)
            .map_err(map_error)?;
        Ok(FfiServiceSupport {
            desktop: support.desktop,
            files: support.files,
            terminal: support.terminal,
            camera: support.camera,
            system_audio: support.system_audio,
            microphone: support.microphone,
            system_actions: support.system_actions,
        })
    }

    pub fn attach_controller_service(
        &self,
        connection_id: u64,
        generation: u64,
        kind: FfiServiceKind,
    ) -> Result<FfiServiceInfo, FfiError> {
        self.inner
            .attach_controller_service(connection_id, generation, kind.into())
            .map(Into::into)
            .map_err(map_error)
    }

    pub fn controller_services(&self) -> Result<Vec<FfiServiceInfo>, FfiError> {
        self.inner
            .controller_services()
            .map(|records| records.into_iter().map(Into::into).collect())
            .map_err(map_error)
    }

    pub fn controller_service(&self, id: u64) -> Result<FfiServiceInfo, FfiError> {
        self.inner
            .controller_service(id)
            .map(Into::into)
            .map_err(map_error)
    }

    pub fn service_connection(&self, id: u64, generation: u64) -> Result<FfiSessionInfo, FfiError> {
        self.inner
            .service_connection(id, generation)
            .map(crate::types::session_info)
            .map_err(map_error)
    }

    pub fn service_lease(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<Arc<ServiceLeasePoller>, FfiError> {
        self.inner
            .service_lease(id, generation)
            .map(|inner| Arc::new(ServiceLeasePoller { inner }))
            .map_err(map_error)
    }

    pub fn close_controller_service(&self, id: u64, generation: u64) -> Result<(), FfiError> {
        self.inner
            .close_controller_service(id, generation)
            .map_err(map_error)
    }

    pub fn forget_controller_service(&self, id: u64) -> Result<(), FfiError> {
        self.inner.forget_controller_service(id).map_err(map_error)
    }
}

#[derive(uniffi::Object)]
pub struct FilesService {
    inner: vnilink::RemoteFilesService,
    core: std::sync::Weak<vnilink::Core>,
}

#[derive(uniffi::Object)]
pub struct DesktopService {
    pub(crate) inner: Arc<vnilink::RemoteDesktopService>,
    pub(crate) core: std::sync::Weak<vnilink::Core>,
}

#[uniffi::export]
impl DesktopService {
    pub fn control_state(&self) -> Result<FfiDesktopControlState, FfiError> {
        self.inner
            .control_state()
            .map(Into::into)
            .map_err(map_error)
    }
    pub fn acquire_control(&self) -> Result<(), FfiError> {
        self.inner.acquire_control().map_err(map_error)
    }
    pub fn yield_control(&self) -> Result<(), FfiError> {
        self.inner.yield_control().map_err(map_error)
    }
    pub fn submit_clipboard(&self, text: String) -> Result<(), FfiError> {
        self.inner.submit_clipboard(&text).map_err(map_error)
    }
    pub fn clipboard_text(&self) -> Result<Option<String>, FfiError> {
        self.inner.clipboard_text().map_err(map_error)
    }
    pub fn keyboard_mode(&self) -> Result<crate::FfiKeyboardMode, FfiError> {
        self.inner
            .keyboard_mode()
            .map(crate::types::keyboard_mode)
            .map_err(map_error)
    }
    pub fn set_keyboard_mode(&self, mode: crate::FfiKeyboardMode) -> Result<(), FfiError> {
        self.inner
            .set_keyboard_mode(crate::types::to_keyboard_mode(mode))
            .map_err(map_error)
    }
    pub fn swap_control_and_command(&self) -> Result<bool, FfiError> {
        self.inner.swap_control_and_command().map_err(map_error)
    }
    pub fn set_swap_control_and_command(&self, enabled: bool) -> Result<(), FfiError> {
        self.inner
            .set_swap_control_and_command(enabled)
            .map_err(map_error)
    }
    pub fn submit_local_key(&self, usage: u16, down: bool, text: String) -> Result<(), FfiError> {
        self.inner
            .submit_local_key(vnilink::LocalKey {
                usage,
                down,
                text: &text,
                composing: false,
            })
            .map_err(map_error)
    }
    pub fn info(&self) -> FfiServiceInfo {
        self.inner.info().clone().into()
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
    pub fn close(&self) {
        self.inner.close();
    }
}

impl FilesService {
    fn core(&self) -> Result<Arc<vnilink::Core>, FfiError> {
        self.core.upgrade().ok_or(FfiError::ShutDown)
    }
}
#[uniffi::export]
impl FilesService {
    pub fn upload(
        &self,
        src_root: u64,
        src_rel: String,
        dst_root: u64,
        dst_rel: String,
    ) -> Result<u64, FfiError> {
        let info = self.inner.info();
        self.core()?
            .files_service_upload(
                info.id,
                info.generation,
                src_root,
                &src_rel,
                dst_root,
                &dst_rel,
            )
            .map_err(map_error)
    }
    pub fn download(
        &self,
        src_root: u64,
        src_rel: String,
        dst_root: u64,
        dst_rel: String,
    ) -> Result<u64, FfiError> {
        let info = self.inner.info();
        self.core()?
            .files_service_download(
                info.id,
                info.generation,
                src_root,
                &src_rel,
                dst_root,
                &dst_rel,
            )
            .map_err(map_error)
    }
    pub fn jobs(&self) -> Result<Vec<crate::FfiJobSnapshot>, FfiError> {
        let info = self.inner.info();
        self.core()?
            .files_service_jobs(info.id, info.generation)
            .map(|jobs| jobs.into_iter().map(crate::files::job_snapshot).collect())
            .map_err(map_error)
    }
    pub fn job_snapshot(&self, job: u64) -> Result<crate::FfiJobSnapshot, FfiError> {
        let info = self.inner.info();
        self.core()?
            .files_service_job_snapshot(info.id, info.generation, job)
            .map(crate::files::job_snapshot)
            .map_err(map_error)
    }
    pub fn job_poll(&self, job: u64) -> Result<crate::FfiJobSnapshot, FfiError> {
        let info = self.inner.info();
        self.core()?
            .files_service_job_poll(info.id, info.generation, job)
            .map(crate::files::job_snapshot)
            .map_err(map_error)
    }
    pub fn job_cancel(&self, job: u64) -> Result<crate::FfiJobSnapshot, FfiError> {
        let info = self.inner.info();
        self.core()?
            .files_service_job_cancel(info.id, info.generation, job)
            .map(crate::files::job_snapshot)
            .map_err(map_error)
    }
    pub fn job_resume(&self, job: u64) -> Result<crate::FfiJobSnapshot, FfiError> {
        let info = self.inner.info();
        self.core()?
            .files_service_job_resume(info.id, info.generation, job)
            .map(crate::files::job_snapshot)
            .map_err(map_error)
    }
    pub fn job_resolve(
        &self,
        job: u64,
        choice: crate::FfiConflictChoice,
        apply_to_remaining: bool,
    ) -> Result<crate::FfiJobSnapshot, FfiError> {
        let info = self.inner.info();
        self.core()?
            .files_service_job_resolve(
                info.id,
                info.generation,
                job,
                crate::files::to_conflict_choice(choice),
                apply_to_remaining,
            )
            .map(crate::files::job_snapshot)
            .map_err(map_error)
    }
    pub fn job_forget(&self, job: u64) -> Result<(), FfiError> {
        let info = self.inner.info();
        self.core()?
            .files_service_job_forget(info.id, info.generation, job)
            .map_err(map_error)
    }
    pub fn info(&self) -> FfiServiceInfo {
        self.inner.info().clone().into()
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
    pub fn close(&self) {
        self.inner.close();
    }
    pub fn list(
        &self,
        root: u64,
        path: String,
        offset: u64,
        limit: u16,
    ) -> Result<crate::FfiDirPage, FfiError> {
        self.inner
            .list(root, &path, offset, limit)
            .map(crate::files::dir_page)
            .map_err(map_error)
    }
    pub fn mkdir(&self, root: u64, path: String, name: String) -> Result<(), FfiError> {
        self.inner.mkdir(root, &path, &name).map_err(map_error)
    }
    pub fn rename(
        &self,
        root: u64,
        path: String,
        name: String,
        expected: Option<crate::FfiEntryToken>,
        new_name: String,
    ) -> Result<(), FfiError> {
        let expected = expected.map(|token| vnilink::EntryToken {
            dev: token.dev,
            ino: token.ino,
        });
        self.inner
            .rename(root, &path, &name, expected, &new_name)
            .map_err(map_error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiDesktopInfo {
    pub service: FfiServiceInfo,
    pub monitor_width: u16,
    pub monitor_height: u16,
    pub codec: u8,
    pub quality: crate::FfiQuality,
    pub separate_cursor: bool,
}
impl From<vnilink::DesktopInfo> for FfiDesktopInfo {
    fn from(value: vnilink::DesktopInfo) -> Self {
        Self {
            service: value.service.into(),
            monitor_width: value.monitor_width,
            monitor_height: value.monitor_height,
            codec: value.codec,
            quality: crate::types::quality_record(&value.quality),
            separate_cursor: value.separate_cursor,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiDesktopCursorImage {
    pub width: u16,
    pub height: u16,
    pub hotspot_x: u16,
    pub hotspot_y: u16,
    pub pixels: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiDesktopCursor {
    pub x: i32,
    pub y: i32,
    pub visible: bool,
    pub image: Option<FfiDesktopCursorImage>,
}
impl From<&vnilink::CursorState> for FfiDesktopCursor {
    fn from(value: &vnilink::CursorState) -> Self {
        Self {
            x: value.position.0,
            y: value.position.1,
            visible: value.visible,
            image: value.image.as_ref().map(|image| FfiDesktopCursorImage {
                width: image.width(),
                height: image.height(),
                hotspot_x: image.hotspot().0,
                hotspot_y: image.hotspot().1,
                pixels: image.pixels().to_vec(),
            }),
        }
    }
}
#[uniffi::export]
impl VnilinkCore {
    pub fn hosted_desktop_info(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<FfiDesktopInfo, FfiError> {
        self.inner
            .hosted_desktop_info(id, generation)
            .map(Into::into)
            .map_err(map_error)
    }
    pub fn publish_desktop_clipboard(
        &self,
        id: u64,
        generation: u64,
        text: String,
    ) -> Result<(), FfiError> {
        self.inner
            .publish_desktop_clipboard(id, generation, &text)
            .map_err(map_error)
    }
    pub fn publish_desktop_video(
        &self,
        id: u64,
        generation: u64,
        keyframe: bool,
        timestamp_us: u64,
        payload: Vec<u8>,
    ) -> Result<(), FfiError> {
        let frame =
            vnilink::EncodedFrame::h264(keyframe, timestamp_us, payload).map_err(map_error)?;
        self.inner
            .publish_desktop_video(id, generation, frame)
            .map_err(map_error)
    }
    pub fn publish_desktop_cursor(
        &self,
        id: u64,
        generation: u64,
        cursor: FfiDesktopCursor,
    ) -> Result<(), FfiError> {
        let image = cursor
            .image
            .map(|image| {
                vnilink::CursorImage::new(
                    image.width,
                    image.height,
                    (image.hotspot_x, image.hotspot_y),
                    image.pixels,
                )
                .map(Arc::new)
            })
            .transpose()
            .map_err(map_error)?;
        self.inner
            .publish_desktop_cursor(
                id,
                generation,
                vnilink::CursorState {
                    position: (cursor.x, cursor.y),
                    visible: cursor.visible,
                    image,
                },
            )
            .map_err(map_error)
    }
    pub fn take_desktop_keyframe_request(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<bool, FfiError> {
        self.inner
            .take_desktop_keyframe_request(id, generation)
            .map_err(map_error)
    }
    /// Reports measured native capture-read and encode costs for one hosted
    /// Desktop owner. Capture read may include waiting for a frame; it is
    /// diagnostic, not a CPU congestion signal. Feeds the diagnostics
    /// pipeline section and the host feedback the controller adapts on.
    pub fn report_desktop_producer_duration(
        &self,
        id: u64,
        generation: u64,
        capture_us: u64,
        encode_us: u64,
    ) -> Result<(), FfiError> {
        self.inner
            .report_desktop_producer_duration(id, generation, capture_us, encode_us)
            .map_err(map_error)
    }
}
#[uniffi::export]
impl DesktopService {
    pub fn desktop_info(&self) -> Result<FfiDesktopInfo, FfiError> {
        self.inner.desktop_info().map(Into::into).map_err(map_error)
    }
    pub fn subscribe_video(&self) -> Result<crate::VideoPoller, FfiError> {
        self.inner
            .subscribe_video()
            .map(crate::VideoPoller::new)
            .map_err(map_error)
    }
    pub fn cursor(&self) -> Result<Option<FfiDesktopCursor>, FfiError> {
        self.inner
            .cursor()
            .map(|c| c.map(|c| c.as_ref().into()))
            .map_err(map_error)
    }
    pub fn submit_key(&self, usage: u16, down: bool) -> Result<(), FfiError> {
        self.inner
            .submit_input(vnilink::InputEvent::Key { usage, down })
            .map_err(map_error)
    }
    pub fn topology(&self) -> Result<Vec<crate::types::FfiDisplay>, FfiError> {
        self.inner
            .topology()
            .map(|displays| displays.iter().map(crate::types::display_record).collect())
            .map_err(map_error)
    }

    pub fn select_displays(&self, ids: Vec<u64>) -> Result<(), FfiError> {
        self.inner.select_displays(&ids).map_err(map_error)
    }

    pub fn submit_pointer(
        &self,
        x: i32,
        y: i32,
        display: u64,
        revision: u64,
    ) -> Result<(), FfiError> {
        self.inner
            .submit_input(vnilink::InputEvent::pointer(x, y, display, revision))
            .map_err(map_error)
    }
    pub fn submit_button(
        &self,
        button: crate::FfiPointerButton,
        down: bool,
    ) -> Result<(), FfiError> {
        let button = match button {
            crate::FfiPointerButton::Left => vnilink::PointerButton::Left,
            crate::FfiPointerButton::Right => vnilink::PointerButton::Right,
            crate::FfiPointerButton::Middle => vnilink::PointerButton::Middle,
            crate::FfiPointerButton::Back => vnilink::PointerButton::Back,
            crate::FfiPointerButton::Forward => vnilink::PointerButton::Forward,
        };
        self.inner
            .submit_input(vnilink::InputEvent::Button { button, down })
            .map_err(map_error)
    }
    pub fn submit_wheel(&self, horizontal: i16, vertical: i16) -> Result<(), FfiError> {
        self.inner
            .submit_input(vnilink::InputEvent::Wheel {
                dx: horizontal,
                dy: vertical,
            })
            .map_err(map_error)
    }
    pub fn pointer_settings(&self) -> Result<crate::FfiPointerSettings, FfiError> {
        self.inner
            .pointer_settings()
            .map(Into::into)
            .map_err(map_error)
    }
    pub fn set_pointer_settings(
        &self,
        settings: crate::FfiPointerSettings,
    ) -> Result<(), FfiError> {
        self.inner
            .set_pointer_settings(settings.into())
            .map_err(map_error)
    }
    pub fn submit_precise_wheel(&self, dx: i32, dy: i32) -> Result<(), FfiError> {
        self.inner
            .submit_input(vnilink::InputEvent::precise_wheel(dx, dy))
            .map_err(map_error)
    }
    pub fn submit_motion(&self, dx: i16, dy: i16) -> Result<(), FfiError> {
        self.inner
            .submit_input(vnilink::InputEvent::motion(dx, dy))
            .map_err(map_error)
    }
    pub fn submit_text(&self, text: String) -> Result<(), FfiError> {
        self.inner.submit_text(&text).map_err(map_error)
    }
    pub fn release_input(&self) -> Result<(), FfiError> {
        self.inner.release_input().map_err(map_error)
    }
    pub fn request_keyframe(&self) -> Result<(), FfiError> {
        self.inner.request_keyframe().map_err(map_error)
    }
}

#[uniffi::export]
impl VnilinkCore {
    pub fn files_service_adopt_job(
        &self,
        id: u64,
        generation: u64,
        job: u64,
    ) -> Result<crate::FfiJobSnapshot, FfiError> {
        self.inner
            .files_service_adopt_job(id, generation, job)
            .map(crate::files::job_snapshot)
            .map_err(map_error)
    }
}

#[uniffi::export]
impl FilesService {
    pub fn adopt_job(&self, job: u64) -> Result<crate::FfiJobSnapshot, FfiError> {
        let info = self.inner.info();
        self.core()?
            .files_service_adopt_job(info.id, info.generation, job)
            .map(crate::files::job_snapshot)
            .map_err(map_error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiAdaptationReason {
    WarmingUp,
    Stable,
    ReceiverPressure,
    EncoderPressure,
    NetworkPressure,
    Recovering,
    Idle,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FfiDesktopMediaStatus {
    pub reason: FfiAdaptationReason,
    pub received_kbps: u32,
    pub received_fps: u16,
    pub receiver_queue_ms: u64,
    pub host_queue_ms: u64,
    pub send_loss_per_mille: u16,
    pub rtt_ms: u64,
    pub decode_us: u64,
    pub encode_us: u64,
    pub capture_us: u64,
}
impl From<vnilink::DesktopMediaStatus> for FfiDesktopMediaStatus {
    fn from(status: vnilink::DesktopMediaStatus) -> Self {
        use vnilink::AdaptationReason as Reason;
        Self {
            reason: match status.reason {
                Reason::WarmingUp => FfiAdaptationReason::WarmingUp,
                Reason::Stable => FfiAdaptationReason::Stable,
                Reason::ReceiverPressure => FfiAdaptationReason::ReceiverPressure,
                Reason::EncoderPressure => FfiAdaptationReason::EncoderPressure,
                Reason::NetworkPressure => FfiAdaptationReason::NetworkPressure,
                Reason::Recovering => FfiAdaptationReason::Recovering,
                Reason::Idle => FfiAdaptationReason::Idle,
            },
            received_kbps: status.received_kbps,
            received_fps: status.received_fps,
            receiver_queue_ms: status.receiver_queue_ms,
            host_queue_ms: status.host_queue_ms,
            send_loss_per_mille: status.send_loss_per_mille,
            rtt_ms: status.rtt_ms,
            decode_us: status.decode_us,
            encode_us: status.encode_us,
            capture_us: status.capture_us,
        }
    }
}
#[uniffi::export]
impl DesktopService {
    pub fn report_decode_duration(&self, microseconds: u64) -> Result<(), FfiError> {
        self.inner
            .report_decode_duration(microseconds)
            .map_err(map_error)
    }
    pub fn media_status(&self) -> Result<FfiDesktopMediaStatus, FfiError> {
        self.inner.media_status().map(Into::into).map_err(map_error)
    }
}
