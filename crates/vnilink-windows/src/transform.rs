//! The H.264 encoder transforms Media Foundation offers on this machine:
//! hardware ones (NVIDIA NVENC, Intel Quick Sync, AMD) in the order Media
//! Foundation ranks them, then Microsoft's software transform, which every
//! Windows with Media Foundation has.

use windows::core::{GUID, PWSTR};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFTransform, MFMediaType_Video, MFTEnumEx, MFT_FRIENDLY_NAME_Attribute,
    MFVideoFormat_H264, MFVideoFormat_NV12, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_ASYNCMFT,
    MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER, MFT_REGISTER_TYPE_INFO,
    MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};

const CLSID_MSH264_ENCODER: GUID = GUID::from_u128(0x6ca5_0344_051a_4ded_9779_a433_0516_5e35);

/// One H.264 encoder transform, not yet created.
pub(crate) struct H264Transform {
    source: Source,
    pub(crate) name: String,
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
