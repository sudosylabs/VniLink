use pipewire as pw;
use pw::spa::param::video::VideoFormat;

use crate::dmabuf::{DRM_FORMAT_MOD_INVALID, DRM_FORMAT_MOD_LINEAR};
use crate::pipewire::{
    delivery_of, enum_formats, format_objects, PortalBuffer, PortalDelivery, PortalFormat,
};

#[test]
fn portal_negotiation_does_not_offer_gpu_modifiers() {
    let objects = format_objects(&[DRM_FORMAT_MOD_LINEAR]);
    let key = pw::spa::param::format::FormatProperties::VideoModifier.as_raw();
    assert!(
        objects
            .iter()
            .all(|object| object.properties.iter().all(|property| property.key != key)),
        "portal capture must not negotiate the GBM path that aborts during unmap"
    );
    assert_eq!(objects.len(), 1);
    assert_eq!(enum_formats(&[DRM_FORMAT_MOD_LINEAR]).unwrap().len(), 1);
}

#[test]
fn a_copied_frame_names_its_format_and_buffer() {
    assert_eq!(
        delivery_of(VideoFormat::BGRA, false, 0, false),
        Some(PortalDelivery {
            format: PortalFormat::Bgra,
            buffer: PortalBuffer::Memory,
        })
    );
    assert_eq!(
        delivery_of(VideoFormat::BGRx, true, DRM_FORMAT_MOD_LINEAR, true),
        Some(PortalDelivery {
            format: PortalFormat::Bgrx,
            buffer: PortalBuffer::DmaBuf {
                modifier: DRM_FORMAT_MOD_LINEAR,
                explicit: true,
            },
        })
    );
    assert_eq!(delivery_of(VideoFormat::RGBA, true, 0, true), None);
}

#[test]
fn portal_buffer_policy_excludes_dmabuf_and_rejects_unexpected_gpu_buffers() {
    use crate::pipewire::{accepts_memory_type, memory_buffer_object};
    use pw::spa::buffer::DataType;
    use pw::spa::utils::{Choice, ChoiceEnum};
    let object = memory_buffer_object();
    assert_eq!(
        object.type_,
        pw::spa::utils::SpaTypes::ObjectParamBuffers.as_raw()
    );
    let property = &object.properties[0];
    assert_eq!(property.key, pw::spa::sys::SPA_PARAM_BUFFERS_dataType);
    assert!(property
        .flags
        .contains(pw::spa::pod::PropertyFlags::MANDATORY));
    let pw::spa::pod::Value::Choice(pw::spa::pod::ChoiceValue::Int(Choice(
        _,
        ChoiceEnum::Flags { default, flags },
    ))) = &property.value
    else {
        panic!("buffer mask must be a flags choice")
    };
    assert_eq!(
        *default,
        (1 << DataType::MemFd.as_raw()) | (1 << DataType::MemPtr.as_raw())
    );
    assert!(flags.is_empty());
    assert!(accepts_memory_type(DataType::MemFd));
    assert!(accepts_memory_type(DataType::MemPtr));
    assert!(!accepts_memory_type(DataType::DmaBuf));
    assert!(!accepts_memory_type(DataType::Invalid));
}
