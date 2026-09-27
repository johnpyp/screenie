//! What screenie asks a cast for: the formats it reads, and the buffers and metadata it
//! wants with them, as SPA pods.

use pipewire::spa;
use screenie_core::PixelFormat;
use spa::param::ParamType;
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::video::VideoFormat;
use spa::pod::{ChoiceValue, Object, Property, PropertyFlags, Value};
use spa::sys;
use spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Id, Rectangle, SpaTypes};

/// The formats a picture can come in, and what they are in memory.
const FORMATS: [(VideoFormat, PixelFormat); 4] = [
    (VideoFormat::BGRx, PixelFormat::Bgrx),
    (VideoFormat::BGRA, PixelFormat::Bgra),
    (VideoFormat::RGBx, PixelFormat::Rgbx),
    (VideoFormat::RGBA, PixelFormat::Rgba),
];

/// The byte order of `format`, if screenie reads it.
pub(crate) fn pixel_format(format: VideoFormat) -> Option<PixelFormat> {
    FORMATS.iter().find(|(f, _)| *f == format).map(|(_, p)| *p)
}

/// The largest cursor sprite asked for, and the one the cast starts with.
const CURSOR_MAX: u32 = 256;
const CURSOR_DEFAULT: u32 = 64;

fn property(key: u32, value: Value) -> Property {
    Property {
        key,
        flags: PropertyFlags::empty(),
        value,
    }
}

fn serialize(object: Object) -> Vec<u8> {
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .expect("a pod in memory always serializes")
    .0
    .into_inner()
}

/// Raw video in the formats screenie reads, of any size, at most `max_fps` frames a
/// second (any rate, when `None`).
pub(crate) fn enum_format(max_fps: Option<u32>) -> Vec<u8> {
    let formats: Vec<Id> = FORMATS.iter().map(|(f, _)| Id(f.as_raw())).collect();
    let mut properties = vec![
        property(
            FormatProperties::MediaType.as_raw(),
            Value::Id(Id(MediaType::Video.as_raw())),
        ),
        property(
            FormatProperties::MediaSubtype.as_raw(),
            Value::Id(Id(MediaSubtype::Raw.as_raw())),
        ),
        property(
            FormatProperties::VideoFormat.as_raw(),
            Value::Choice(ChoiceValue::Id(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Enum {
                    default: formats[0],
                    alternatives: formats,
                },
            ))),
        ),
        property(
            FormatProperties::VideoSize.as_raw(),
            Value::Choice(ChoiceValue::Rectangle(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Range {
                    default: Rectangle {
                        width: 1920,
                        height: 1080,
                    },
                    min: Rectangle {
                        width: 1,
                        height: 1,
                    },
                    max: Rectangle {
                        width: 16384,
                        height: 16384,
                    },
                },
            ))),
        ),
        // Screen casts send a frame when the screen changes: a variable rate.
        property(
            FormatProperties::VideoFramerate.as_raw(),
            Value::Choice(ChoiceValue::Fraction(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Range {
                    default: Fraction { num: 0, denom: 1 },
                    min: Fraction { num: 0, denom: 1 },
                    max: Fraction {
                        num: 1000,
                        denom: 1,
                    },
                },
            ))),
        ),
    ];
    let max = max_fps.map_or(Fraction { num: 1000, denom: 1 }, |fps| Fraction {
        num: fps.max(1),
        denom: 1,
    });
    properties.push(property(
        FormatProperties::VideoMaxFramerate.as_raw(),
        Value::Choice(ChoiceValue::Fraction(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Range {
                default: max,
                min: Fraction { num: 1, denom: 1 },
                max,
            },
        ))),
    ));
    serialize(Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties,
    })
}

/// Buffers in memory screenie can map (the compositor copies each frame into one).
pub(crate) fn buffers() -> Vec<u8> {
    let types = (1 << sys::SPA_DATA_MemFd) | (1 << sys::SPA_DATA_MemPtr);
    serialize(Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: ParamType::Buffers.as_raw(),
        properties: vec![property(
            sys::SPA_PARAM_BUFFERS_dataType,
            Value::Choice(ChoiceValue::Int(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Flags {
                    default: types,
                    flags: vec![types],
                },
            ))),
        )],
    })
}

/// A metadata block of `kind`, `size` bytes (or a range of sizes).
pub(crate) fn meta(kind: u32, size: ChoiceEnum<i32>) -> Vec<u8> {
    serialize(Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: ParamType::Meta.as_raw(),
        properties: vec![
            property(sys::SPA_PARAM_META_type, Value::Id(Id(kind))),
            property(
                sys::SPA_PARAM_META_size,
                Value::Choice(ChoiceValue::Int(Choice(ChoiceFlags::empty(), size))),
            ),
        ],
    })
}

/// The metadata screenie reads: timestamps, the crop to the content, and the pointer.
pub(crate) fn metas(pointer: bool) -> Vec<Vec<u8>> {
    let size = |n: usize| ChoiceEnum::None(n as i32);
    let mut metas = vec![
        meta(
            sys::SPA_META_Header,
            size(std::mem::size_of::<sys::spa_meta_header>()),
        ),
        meta(
            sys::SPA_META_VideoCrop,
            size(std::mem::size_of::<sys::spa_meta_region>()),
        ),
    ];
    if pointer {
        let cursor = |edge: u32| {
            (std::mem::size_of::<sys::spa_meta_cursor>()
                + std::mem::size_of::<sys::spa_meta_bitmap>()
                + (edge * edge * 4) as usize) as i32
        };
        metas.push(meta(
            sys::SPA_META_Cursor,
            ChoiceEnum::Range {
                default: cursor(CURSOR_DEFAULT),
                min: cursor(1),
                max: cursor(CURSOR_MAX),
            },
        ));
    }
    metas
}
