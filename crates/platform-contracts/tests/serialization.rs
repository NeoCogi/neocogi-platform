//! Generated value persistence uses the SDK's COM format protocol without a renderer.
use neocogi_com::{
    ComDeserialize, ComSerialize,
    serialization::{Format, Limits},
};
use neocogi_com_serialization::{json, ncb};
use neocogi_platform_contracts::{
    audio::{PcmByteOrder, PcmFormat, PcmType},
    render::{
        abi::*,
        platform::{KeyAction, KeyCode, KeyStroke, UiInput},
    },
};

/// Independent COM factories exercise the same generated traits through binary and named formats.
fn formats() -> [Format; 2] {
    [ncb::format(Limits::default()), Format::new(json::factory(), Limits::default())]
}

/// Successful persistence reconstructs the caller's exact generated Rust value.
fn round_trip<T: ComSerialize + ComDeserialize + PartialEq + std::fmt::Debug>(format: &Format, value: T) {
    let bytes = format.serialize(&value).unwrap();
    assert_eq!(format.deserialize::<T>(&bytes).unwrap(), value);
}

/// Records, nested payload enums and scalar wrappers need no hand-authored codecs.
#[test]
fn generated_renderer_input_and_audio_values_round_trip() {
    for format in formats() {
        round_trip(&format, Color { r: 1, g: 2, b: 3, a: 4 });
        for surface in [RenderSurfaceDesc::Framebuffer, RenderSurfaceDesc::Image(Size { width: 320, height: 180 })] {
            round_trip(&format, surface);
        }
        for equation in [
            GpuBlendEquation::Add {
                src: GpuBlendFactor::SrcAlpha,
                dst: GpuBlendFactor::OneMinusSrcAlpha,
            },
            GpuBlendEquation::Subtract {
                src: GpuBlendFactor::One,
                dst: GpuBlendFactor::DstColor,
            },
            GpuBlendEquation::ReverseSubtract {
                src: GpuBlendFactor::SrcColor,
                dst: GpuBlendFactor::Zero,
            },
            GpuBlendEquation::Min,
            GpuBlendEquation::Max,
        ] {
            round_trip(
                &format,
                GpuBlendState::Blend {
                    color: equation,
                    alpha: GpuBlendEquation::Max,
                },
            );
        }
        round_trip(&format, GpuBlendState::Replace);
        // Open integer wrappers preserve unknown values; semantic GPU admission remains separate.
        round_trip(&format, GpuBlendFactor::from_raw(255));
        for key in [KeyCode::Character(u32::from('🦀')), KeyCode::Function(24), KeyCode::ArrowLeft] {
            for action in [KeyAction::Pressed { repeat: true }, KeyAction::Released] {
                round_trip(&format, UiInput::Key(KeyStroke { key, action, modifiers: 15 }));
            }
        }
        for sample_type in [
            PcmType::UnsignedInteger(8),
            PcmType::SignedInteger(24),
            PcmType::FixedPoint(32, 16),
            PcmType::FloatingPoint(64),
        ] {
            round_trip(
                &format,
                PcmFormat {
                    sample_type,
                    byte_order: PcmByteOrder::LittleEndian,
                },
            );
        }
    }
}

/// NCB saves declared fields, never layout padding or the old declaration version.
#[test]
fn color_ncb_bytes_are_ordered_and_incomplete_documents_reject() {
    let color = Color { r: 1, g: 2, b: 3, a: 4 };
    let format = ncb::format(Limits::default());
    let bytes = format.serialize(&color).unwrap();
    // NCB header, little-endian field count, then one byte per declared channel.
    assert_eq!(bytes, b"NCB\x01\x04\x00\x00\x00\x01\x02\x03\x04");
    for end in 0..bytes.len() {
        assert!(format.deserialize::<Color>(&bytes[..end]).is_err(), "accepted prefix of {end} bytes");
    }
    let mut trailing = bytes;
    trailing.push(0);
    assert!(format.deserialize::<Color>(&trailing).is_err());
}

/// Named decoding admits reordered fields while rejecting missing, unknown and mistyped data.
#[test]
fn json_record_fields_are_required_and_checked() {
    let format = Format::new(json::factory(), Limits::default());
    assert_eq!(
        format.deserialize::<Color>(br#"{"a":4,"b":3,"g":2,"r":1}"#).unwrap(),
        Color { r: 1, g: 2, b: 3, a: 4 }
    );
    for bytes in [
        br#"{"r":1,"g":2,"b":3}"#.as_slice(),
        br#"{"r":1,"g":2,"b":3,"a":4,"extra":5}"#,
        br#"{"r":256,"g":2,"b":3,"a":4}"#,
        br#"{"r":"red","g":2,"b":3,"a":4}"#,
        br#"{"r":1,"g":2,"b":3,"a":4} true"#,
    ] {
        assert!(format.deserialize::<Color>(bytes).is_err(), "accepted {}", String::from_utf8_lossy(bytes));
    }
}

/// Caller-owned format errors retain syntax coordinates and typed field paths after session release.
#[test]
fn json_errors_preserve_source_and_field_context() {
    let format = Format::new(json::factory(), Limits::default());
    let source = b"{\n  \"r\": ?\n}";
    let error = format.deserialize::<Color>(source).unwrap_err();
    assert_eq!(error.byte_offset, Some(source.iter().position(|byte| *byte == b'?').unwrap() as u64));
    assert_eq!((error.line, error.column), (Some(2), Some(8)));
    assert!(!error.message.is_empty());
    let error = format.deserialize::<Color>(br#"{"r":256,"g":2,"b":3,"a":4}"#).unwrap_err();
    assert_eq!(error.path, "$.r");
}
