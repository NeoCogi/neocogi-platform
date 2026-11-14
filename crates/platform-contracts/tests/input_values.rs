//! Logical key payloads are validated and preserved through the generated COM boundary.
use neocogi_com::{ComPtr, NC_E_INVALID_ARG, NC_E_INVALID_TAG, NcStatus, NcCopyFromBufferOffset, com_object, decode_inline, encode_inline};
use neocogi_platform_contracts::render::{
    abi::{RenderSurfaceDesc, Size},
    platform::*,
};

#[com_object(interfaces(IInputSink))]
/// Immutable expected transition; comparison observes the values decoded by a real COM call.
struct ExpectedInput {
    /// Exact event the generated client must deliver.
    expected: KeyStroke,
}
impl IInputSink for ExpectedInput {
    /// Check semantic key bounds before comparing the transition decoded by the generated server.
    fn input(&self, event: UiInput, text: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        let UiInput::Key(key) = event else { return Err(NC_E_INVALID_ARG) };
        key.validate()?;
        if key == self.expected && text.offset() == 0 && text.as_slice().is_empty() {
            Ok(())
        } else {
            Err(NC_E_INVALID_ARG)
        }
    }
}

/// Each command and payload family survives nested enum encoding on press and release.
#[test]
fn key_identity_and_action_survive_real_com_calls() {
    let mut keys = vec![KeyCode::Character(u32::from('é')), KeyCode::Character(u32::from('🦀'))];
    keys.extend((1..=24).map(KeyCode::Function));
    keys.extend([
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Enter,
        KeyCode::Escape,
        KeyCode::Space,
        KeyCode::Tab,
        KeyCode::Insert,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::ArrowUp,
        KeyCode::ArrowDown,
        KeyCode::ArrowLeft,
        KeyCode::ArrowRight,
        KeyCode::Alt,
        KeyCode::Control,
        KeyCode::Shift,
        KeyCode::Super,
    ]);
    for key in keys {
        for action in [KeyAction::Pressed { repeat: false }, KeyAction::Pressed { repeat: true }, KeyAction::Released] {
            let expected = KeyStroke { key, action, modifiers: 15 };
            let sink: ComPtr<IInputSinkAbi> = ExpectedInput::from_value(ExpectedInput { expected }).query().unwrap();
            sink.input(UiInput::Key(expected), NcCopyFromBufferOffset::new(0, &[])).unwrap();
            assert_eq!(KeyStroke { modifiers: 16, ..expected }.validate(), Err(NC_E_INVALID_ARG));
            assert_eq!(
                sink.input(UiInput::Key(KeyStroke { modifiers: 16, ..expected }), NcCopyFromBufferOffset::new(0, &[])),
                Err(NC_E_INVALID_ARG)
            );
            for key in [
                KeyCode::Character(0xd800),
                KeyCode::Character(0x110000),
                KeyCode::Function(0),
                KeyCode::Function(25),
            ] {
                assert_eq!(KeyStroke { key, ..expected }.validate(), Err(NC_E_INVALID_ARG));
                assert_eq!(
                    sink.input(UiInput::Key(KeyStroke { key, ..expected }), NcCopyFromBufferOffset::new(0, &[])),
                    Err(NC_E_INVALID_ARG)
                );
            }
        }
    }
}

/// Corrupt only raw integer tags, never a Rust enum; inactive fields are not read.
#[test]
fn malformed_key_action_and_surface_wire_tags_reject() {
    let key = KeyStroke {
        key: KeyCode::Function(24),
        action: KeyAction::Released,
        modifiers: 0,
    };
    let mut wire = encode_inline(&key).unwrap();
    wire.key.tag = 255;
    // SAFETY: generated initialized inline wire, with only an unrestricted integer tag changed.
    assert_eq!(unsafe { decode_inline::<KeyStroke>(&wire) }, Err(NC_E_INVALID_TAG));
    let mut wire = encode_inline(&key).unwrap();
    wire.action.tag = 255;
    // SAFETY: generated initialized inline wire, with only an unrestricted integer tag changed.
    assert_eq!(unsafe { decode_inline::<KeyStroke>(&wire) }, Err(NC_E_INVALID_TAG));
    for desc in [RenderSurfaceDesc::Framebuffer, RenderSurfaceDesc::Image(Size { width: 32, height: 16 })] {
        let mut wire = encode_inline(&desc).unwrap();
        // SAFETY: the encoder supplies fully initialized self-contained wire storage.
        assert_eq!(unsafe { decode_inline::<RenderSurfaceDesc>(&wire) }.unwrap(), desc);
        wire.tag = 255;
        // SAFETY: only the unrestricted raw integer tag was changed.
        assert_eq!(unsafe { decode_inline::<RenderSurfaceDesc>(&wire) }, Err(NC_E_INVALID_TAG));
    }
}
