use pipewire as pw;
use pw::spa::param::video::VideoFormat;
use pw::spa::utils::{Choice, ChoiceEnum};

use crate::dmabuf::{DRM_FORMAT_MOD_INVALID, DRM_FORMAT_MOD_LINEAR};
use crate::pipewire::{
    delivery_of, enum_formats, format_objects, PortalBuffer, PortalDelivery, PortalFormat,
};

#[test]
fn the_stream_offers_dma_buf_modifiers_and_a_memory_fallback() {
    let objects = format_objects(&[DRM_FORMAT_MOD_LINEAR]);
    assert_eq!(objects.len(), 2);
    let key = pw::spa::param::format::FormatProperties::VideoModifier.as_raw();
    let modifier = objects[0]
        .properties
        .iter()
        .find(|property| property.key == key)
        .expect("modifier");
    assert!(modifier.flags.contains(
        pw::spa::pod::PropertyFlags::MANDATORY | pw::spa::pod::PropertyFlags::DONT_FIXATE
    ));
    let pw::spa::pod::Value::Choice(pw::spa::pod::ChoiceValue::Long(Choice(
        _,
        ChoiceEnum::Enum {
            default,
            alternatives,
        },
    ))) = &modifier.value
    else {
        panic!("modifier is not a long choice");
    };
    assert_eq!(*default, DRM_FORMAT_MOD_LINEAR as i64);
    assert!(alternatives.contains(&(DRM_FORMAT_MOD_INVALID as i64)));
    assert!(objects[1]
        .properties
        .iter()
        .all(|property| property.key != key));
    assert_eq!(enum_formats(&[DRM_FORMAT_MOD_LINEAR]).unwrap().len(), 2);
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
