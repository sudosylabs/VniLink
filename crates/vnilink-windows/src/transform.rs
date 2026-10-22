//! The H.264 encoder transforms Media Foundation offers on this machine:
//! hardware ones (NVIDIA NVENC, Intel Quick Sync, AMD) in the order Media
//! Foundation ranks them, then Microsoft's software transform, which every
//! Windows with Media Foundation has.
//!
//! What a transform does with a recovery keyframe, a still screen and a new
//! bitrate is measured per vendor (`recovery_tests`), not assumed from its
//! controls: every transform measured accepts `AVEncVideoMinQP` or reports
//! it unsupported, and none applies it.

use windows::core::{GUID, PWSTR};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFTransform, MFMediaType_Video, MFTEnumEx, MFT_ENUM_HARDWARE_VENDOR_ID_Attribute,
    MFT_FRIENDLY_NAME_Attribute, MFVideoFormat_H264, MFVideoFormat_NV12,
    MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_ASYNCMFT, MFT_ENUM_FLAG_HARDWARE,
    MFT_ENUM_FLAG_SORTANDFILTER, MFT_REGISTER_TYPE_INFO, MF_TRANSFORM_ASYNC,
    MF_TRANSFORM_ASYNC_UNLOCK,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};

const CLSID_MSH264_ENCODER: GUID = GUID::from_u128(0x6ca5_0344_051a_4ded_9779_a433_0516_5e35);

/// Who made a transform, from its hardware vendor ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Vendor {
    /// Microsoft's software transform.
    Microsoft,
    Nvidia,
    Intel,
    Amd,
    /// A hardware transform of a vendor not measured here.
    Other,
}

/// Measured 2026-10-22 on Windows 11 26H2 with NVIDIA's (RTX 5060 Laptop),
/// Intel's (UHD, Raptor Lake) and Microsoft's transforms; AMD and other
/// hardware are not measured and keep the settings that are harmless where
/// ignored.
impl Vendor {
    /// Whether a quantiser floor (`AVEncVideoMinQP`) may shrink a recovery
    /// keyframe. NVIDIA's and Microsoft's take it and ignore it, alone or
    /// with a reconfiguring bitrate write; Intel's does not support it;
    /// `AVEncVideoEncodeQP` and a smaller buffer do nothing either.
    /// NVIDIA's keyframe shrinks only under a cut mean bitrate, and its
    /// rate control spends what was saved on the next pictures.
    pub(crate) fn takes_qp_floor(self) -> bool {
        matches!(self, Self::Amd | Self::Other)
    }

    /// Whether pictures of an unchanged screen bring a keyframe to full
    /// quality: Intel's took a page of text from 16 to 41 dB in two
    /// seconds at 720p 2 Mb/s; NVIDIA's stayed between 17 and 31 dB and
    /// Microsoft's between 25 and 39 dB.
    pub(crate) fn refines_still_pictures(self) -> bool {
        self == Self::Intel
    }

    /// Whether a new mean bitrate takes effect while encoding. Halving it
    /// on Intel's left the bytes per second within 7%; NVIDIA's and
    /// Microsoft's followed it.
    pub(crate) fn retunes(self) -> bool {
        self != Self::Intel
    }

    /// PCI vendor IDs as Media Foundation reports them, `VEN_10DE`.
    fn of(id: &str) -> Self {
        match id.to_ascii_uppercase().as_str() {
            "VEN_10DE" => Self::Nvidia,
            "VEN_8086" => Self::Intel,
            "VEN_1002" | "VEN_1022" => Self::Amd,
            _ => Self::Other,
        }
    }
}

/// One H.264 encoder transform, not yet created.
pub(crate) struct H264Transform {
    source: Source,
    pub(crate) name: String,
    pub(crate) vendor: Vendor,
}

enum Source {
    Hardware(IMFActivate),
    Software,
}

impl H264Transform {
    /// Microsoft's software transform.
    pub(crate) fn software() -> Self {
        Self {
            source: Source::Software,
            name: "Microsoft H.264 Encoder MFT".into(),
            vendor: Vendor::Microsoft,
        }
    }

    pub(crate) fn hardware(&self) -> bool {
        matches!(self.source, Source::Hardware(_))
    }

    /// Creates the transform, unlocked if it is asynchronous. The caller
    /// owns COM and Media Foundation startup on this thread. One transform
    /// of a hardware encoder is live at a time: the next is created once
    /// the last has been dropped.
    pub(crate) unsafe fn create(&self) -> windows::core::Result<CreatedTransform> {
        let Source::Hardware(activate) = &self.source else {
            return Ok(CreatedTransform {
                transform: CoCreateInstance(&CLSID_MSH264_ENCODER, None, CLSCTX_INPROC_SERVER)?,
                activate: None,
            });
        };
        let transform: IMFTransform = activate.ActivateObject()?;
        let created = CreatedTransform {
            transform,
            activate: Some(activate.clone()),
        };
        if let Ok(attributes) = created.GetAttributes() {
            if attributes.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0 {
                attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
            }
        }
        Ok(created)
    }
}

/// A transform from [`H264Transform::create`]. A hardware one is shut down
/// through its activation object when dropped (these do not implement
/// `DetachObject`), which ends its events and lets that object create
/// another; activating again before that returns the same transform.
pub(crate) struct CreatedTransform {
    transform: IMFTransform,
    activate: Option<IMFActivate>,
}

impl std::ops::Deref for CreatedTransform {
    type Target = IMFTransform;

    fn deref(&self) -> &IMFTransform {
        &self.transform
    }
}

impl Drop for CreatedTransform {
    fn drop(&mut self) {
        if let Some(activate) = &self.activate {
            // SAFETY: the activation object created this transform.
            let _ = unsafe { activate.ShutdownObject() };
        }
    }
}

/// The H.264 encoders that take NV12, hardware first, Microsoft's software
/// transform last. A machine whose hardware cannot be enumerated still gets
/// the software transform.
pub(crate) unsafe fn h264_encoders() -> Vec<H264Transform> {
    let mut encoders = hardware_encoders().unwrap_or_default();
    encoders.push(H264Transform::software());
    encoders
}

unsafe fn hardware_encoders() -> windows::core::Result<Vec<H264Transform>> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut activates = std::ptr::null_mut::<Option<IMFActivate>>();
    let mut count = 0u32;
    MFTEnumEx(
        MFT_CATEGORY_VIDEO_ENCODER,
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
        Some(&input),
        Some(&output),
        &mut activates,
        &mut count,
    )?;
    if activates.is_null() {
        return Ok(Vec::new());
    }
    // The array is ours to free; each element is taken, so released once.
    let encoders = std::slice::from_raw_parts_mut(activates, count as usize)
        .iter_mut()
        .filter_map(Option::take)
        .map(|activate| H264Transform {
            name: string(&activate, &MFT_FRIENDLY_NAME_Attribute).unwrap_or_default(),
            vendor: string(&activate, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute)
                .map_or(Vendor::Other, |id| Vendor::of(&id)),
            source: Source::Hardware(activate),
        })
        .collect();
    CoTaskMemFree(Some(activates as *const _));
    Ok(encoders)
}

unsafe fn string(activate: &IMFActivate, key: &GUID) -> Option<String> {
    let mut value = PWSTR::null();
    let mut length = 0u32;
    activate
        .GetAllocatedString(key, &mut value, &mut length)
        .ok()?;
    let text = value.to_string().ok();
    CoTaskMemFree(Some(value.0 as *const _));
    text
}
