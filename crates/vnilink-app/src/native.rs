//! Native runtime registration. This installs factories, never starts capture or a shell.
use crate::{error::map_error, FfiError, VnilinkCore};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct FfiNativeBackend {
    pub name: String,
    pub ready: bool,
    pub error: Option<String>,
}
impl VnilinkCore {
    pub fn initialize_native_backends(&self) -> Vec<FfiNativeBackend> {
        let operations = [
            ("system", self.enable_native_system_actions()),
            ("terminal", self.enable_default_native_terminal()),
            ("power", self.enable_native_power(true)),
            ("clipboard", self.enable_native_clipboard()),
            ("video", self.prepare_native_video()),
            ("filesystem", self.enable_filesystem_access()),
            ("snapshot", self.enable_native_snapshot()),
        ];
        operations
            .into_iter()
            .map(|(name, result)| FfiNativeBackend {
                name: name.into(),
                ready: result.is_ok(),
                // Backend diagnostics never contain source paths, device payloads or secrets.
                error: result
                    .err()
                    .map(|_| format!("The native {name} backend could not be initialized")),
            })
            .collect()
    }
    pub fn enable_native_clipboard(&self) -> Result<(), FfiError> {
        self.install_clipboard(Box::new(crate::native_clipboard::NativeClipboard::open()?))
    }
    pub fn prepare_native_video(&self) -> Result<(), FfiError> {
        #[cfg(target_os = "macos")]
        {
            self.prepare_mac_video()?;
            // Electron currently consumes video frames without a separate cursor channel.
            self.inner.set_cursor_receiver(false).map_err(map_error)
        }
        #[cfg(target_os = "linux")]
        {
            let mut caps = vnilink_linux::video_codec_caps();
            caps.additional = vnilink_media::probe_software_codecs();
            if caps.can_encode() && (caps.max_width == 0 || caps.max_height == 0) {
                caps.max_width = caps.max_width.max(4096);
                caps.max_height = caps.max_height.max(4096);
            }
            let supported = caps.can_encode() || caps.can_decode();
            self.inner.set_video_codecs(caps).map_err(map_error)?;
            if supported {
                Ok(())
            } else {
                Err(map_error(vnilink::Error::Unsupported(
                    "no native video codec is available",
                )))
            }
        }
        #[cfg(windows)]
        {
            let mut caps = vnilink_windows::probe_video_codecs();
            caps.additional = vnilink_media::probe_software_codecs();
            if caps.can_encode() && (caps.max_width == 0 || caps.max_height == 0) {
                caps.max_width = caps.max_width.max(4096);
                caps.max_height = caps.max_height.max(4096);
            }
            let supported = caps.can_encode() || caps.can_decode();
            self.inner.set_video_codecs(caps).map_err(map_error)?;
            if supported {
                Ok(())
            } else {
                Err(map_error(vnilink::Error::Unsupported(
                    "no native video codec is available",
                )))
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            Err(map_error(vnilink::Error::Unsupported(
                "native video backend",
            )))
        }
    }
    /// Installs the one-shot screenshot capture. Registration does not prompt or capture.
    pub fn enable_native_snapshot(&self) -> Result<(), FfiError> {
        #[cfg(target_os = "macos")]
        let factory = Arc::new(vnilink_macos::MacSnapshot);
        #[cfg(target_os = "linux")]
        let factory = Arc::new(vnilink_linux::LinuxSnapshot);
        #[cfg(windows)]
        let factory = Arc::new(vnilink_windows::WindowsSnapshot);
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        {
            self.inner
                .set_snapshot_factory(Some(factory))
                .map_err(map_error)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        {
            let _ = self;
            Err(map_error(vnilink::Error::Unsupported("screen snapshot")))
        }
    }
    /// OS consent is handled by Electron, and this path must never open an OS prompt.
    pub fn enable_native_input_without_prompt(&self) -> Result<(), FfiError> {
        self.inner
            .enable_os_input_without_prompt()
            .map_err(map_error)
    }
}
