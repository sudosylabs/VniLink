use pipewire as pw;
use pw::spa::param::video::VideoFormat;

use crate::dmabuf::DRM_FORMAT_MOD_LINEAR;
use crate::gpu_picture::{
    MODIFIER_I915_X_TILED, MODIFIER_I915_Y_TILED, MODIFIER_LINEAR, PREFERRED_MODIFIERS,
};
use crate::pipewire::{
    damage_meta_object, delivery_of, dma_buf_object, enum_formats, fixated_format, format_objects,
    offered_modifiers, PortalBuffer, PortalDelivery, PortalFormat,
};

#[test]
fn shared_memory_negotiation_offers_no_modifier() {
    let key = pw::spa::param::format::FormatProperties::VideoModifier.as_raw();
    let objects = format_objects(&[]);
    assert_eq!(objects.len(), 1);
    assert!(
        objects[0]
            .properties
            .iter()
            .all(|property| property.key != key),
        "a capture that keeps pictures in memory must not offer GPU layouts"
    );
    assert_eq!(enum_formats(&[]).unwrap().len(), 1);
}

#[test]
fn gpu_negotiation_offers_layouts_unfixed_and_then_shared_memory() {
    let key = pw::spa::param::format::FormatProperties::VideoModifier.as_raw();
    let objects = format_objects(&PREFERRED_MODIFIERS);
    assert_eq!(objects.len(), 2);
    let modifier = objects[0]
        .properties
        .iter()
        .find(|property| property.key == key)
        .expect("the first format names layouts");
    // Mandatory so a compositor that cannot draw in one leaves it for the
    // second format; unfixed so it is chosen in a second round.
    assert!(modifier.flags.contains(
        pw::spa::pod::PropertyFlags::MANDATORY | pw::spa::pod::PropertyFlags::DONT_FIXATE
    ));
    assert!(objects[1]
        .properties
        .iter()
        .all(|property| property.key != key));
    assert_eq!(enum_formats(&PREFERRED_MODIFIERS).unwrap().len(), 2);
}

#[test]
fn a_fixated_format_names_one_layout_and_one_pixel_format() {
    let object = fixated_format(VideoFormat::BGRx, MODIFIER_I915_Y_TILED);
    let key = pw::spa::param::format::FormatProperties::VideoModifier.as_raw();
    let modifier = object
        .properties
        .iter()
        .find(|property| property.key == key)
        .unwrap();
    assert!(matches!(
        modifier.value,
        pw::spa::pod::Value::Long(value) if value as u64 == MODIFIER_I915_Y_TILED
    ));
    assert!(!modifier
        .flags
        .contains(pw::spa::pod::PropertyFlags::DONT_FIXATE));
    let format = object
        .properties
        .iter()
        .find(|property| {
            property.key == pw::spa::param::format::FormatProperties::VideoFormat.as_raw()
        })
        .unwrap();
    assert!(matches!(
        format.value,
        pw::spa::pod::Value::Id(id) if id.0 == VideoFormat::BGRx.as_raw()
    ));
}

#[test]
fn offered_layouts_are_read_from_the_compositors_format() {
    let object = format_objects(&[MODIFIER_LINEAR, MODIFIER_I915_X_TILED]).remove(0);
    let bytes = crate::pipewire::serialize_object(object).unwrap();
    let pod = pw::spa::pod::Pod::from_bytes(&bytes).unwrap();
    assert_eq!(
        offered_modifiers(pod),
        [MODIFIER_LINEAR, MODIFIER_LINEAR, MODIFIER_I915_X_TILED]
    );
}

#[test]
fn gpu_buffers_ask_for_dmabuf_only_and_damage_has_room_for_regions() {
    use pw::spa::buffer::DataType;
    use pw::spa::utils::{Choice, ChoiceEnum};
    let object = dma_buf_object();
    let pw::spa::pod::Value::Choice(pw::spa::pod::ChoiceValue::Int(Choice(
        _,
        ChoiceEnum::Flags { default, .. },
    ))) = &object.properties[0].value
    else {
        panic!("buffer mask must be a flags choice")
    };
    assert_eq!(*default, 1 << DataType::DmaBuf.as_raw());
    let meta = damage_meta_object();
    assert!(matches!(
        meta.properties[1].value,
        pw::spa::pod::Value::Int(size) if size % 16 == 0 && size >= 16
    ));
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
