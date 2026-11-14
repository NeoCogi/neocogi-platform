//! Nested renderer values preserve only active data through generated clients and checked wire codecs.
use neocogi_com::{NC_E_INVALID_TAG, NcOutputBuffer, decode_inline, encode_inline};
use neocogi_platform_contracts::render::{abi::*, source::PipelineDesc};

/// All equation shapes survive copied results and typed bulk binding outputs.
#[test]
fn blend_equations_and_binding_payloads_survive_real_com_calls() {
    let equations = [
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
    ];
    let bindings = [
        GpuShaderBinding {
            stage: GpuShaderStage::Fragment,
            slot: 0,
            kind: GpuBindingKind::Uniform(16),
        },
        GpuShaderBinding {
            stage: GpuShaderStage::Fragment,
            slot: 0,
            kind: GpuBindingKind::SampledTexture,
        },
    ];
    let mut states = vec![GpuBlendState::Replace];
    for color in equations {
        states.extend(equations.map(|alpha| GpuBlendState::Blend { color, alpha }));
    }
    assert_eq!(GpuBlendState::default(), GpuBlendState::Replace);
    for blend in states {
        let source = PipelineDesc {
            state: GpuPipelineInfo { blend, ..Default::default() },
            bindings: bindings.to_vec(),
            ..Default::default()
        }
        .source()
        .unwrap();
        assert_eq!(source.info().unwrap().blend, blend);
        let mut output = NcOutputBuffer::new();
        source.bindings(output.output(0, 2)).unwrap();
        assert_eq!(output.as_slice(), bindings);
    }
}

/// Invalid integer tags never become invalid Rust enums or reach native mappings.
#[test]
fn malformed_blend_and_binding_wire_values_reject() {
    let mut wire = encode_inline(&GpuBlendState::Blend {
        color: GpuBlendEquation::Min,
        alpha: GpuBlendEquation::Max,
    })
    .unwrap();
    wire.tag = 255;
    // SAFETY: the encoder initialized this self-contained wire; only its raw integer tag changed.
    assert_eq!(unsafe { decode_inline::<GpuBlendState>(&wire) }, Err(NC_E_INVALID_TAG));
    let mut wire = encode_inline(&GpuBlendEquation::Min).unwrap();
    wire.tag = 255;
    // SAFETY: valid generated storage, with only the unrestricted tag changed.
    assert_eq!(unsafe { decode_inline::<GpuBlendEquation>(&wire) }, Err(NC_E_INVALID_TAG));
    let mut wire = encode_inline(&GpuBindingKind::Uniform(16)).unwrap();
    wire.tag = 255;
    // SAFETY: valid generated storage, with only the unrestricted tag changed.
    assert_eq!(unsafe { decode_inline::<GpuBindingKind>(&wire) }, Err(NC_E_INVALID_TAG));
    for equation in [
        GpuBlendEquation::Add {
            src: GpuBlendFactor::from_raw(255),
            dst: GpuBlendFactor::One,
        },
        GpuBlendEquation::Subtract {
            src: GpuBlendFactor::One,
            dst: GpuBlendFactor::from_raw(255),
        },
        GpuBlendEquation::ReverseSubtract {
            src: GpuBlendFactor::from_raw(255),
            dst: GpuBlendFactor::Zero,
        },
    ] {
        // This is an ordinary open scalar wrapper, not an invalid Rust enum discriminant.
        assert_eq!(encode_inline(&equation).err(), Some(NC_E_INVALID_TAG));
    }
}
