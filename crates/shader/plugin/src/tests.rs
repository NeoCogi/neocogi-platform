//! Independently generated COM caller tests, including ownership after report release.
use super::*;
use neocogi_com::{INcStringComPtrExt, INcBytesComPtrExt, NcCopyFromBufferOffset, NC_OK, NC_E_INVALID_ARG, NC_E_OUT_OF_RANGE};
use neocogi_render_core::abi::GpuShaderStage;
/// Separate generated ABI consumer negotiates by interface identity, not a Rust object cast.
#[allow(dead_code)]
mod client {
    include!(env!("IDL_NEOCOGI_SHADER"));
}
const SOURCE: &str = "struct U { value:vec4<f32> } @group(1) @binding(0) var<uniform> u:U; @vertex fn vs(@location(0) p:vec2<f32>)->@builtin(position) vec4<f32> {return vec4<f32>(p,0.25,1.0)+u.value;}";
/// Real COM getters do not require summary lookup, and item/name/code owners are independent.
#[test]
fn independent_owned_exports_and_items() {
    let root: neocogi_com::ComPtr<client::ShaderCompilerAbi> = CompilerObject::from_value(CompilerObject).query().unwrap();
    let report = root
        .compile(
            client::ShaderCompileRequest {
                stage: GpuShaderStage::Vertex,
                max_output_bytes: 4 * 1024 * 1024,
            },
            NcCopyFromBufferOffset::new(0, SOURCE.as_bytes()),
            NcCopyFromBufferOffset::new(0, b"vs"),
        )
        .unwrap();
    assert_eq!(report.status().unwrap(), NC_OK.code());
    let wgsl = report.wgsl().unwrap();
    let spirv = report.spirv().unwrap();
    let glsl = report.glsl().unwrap();
    let inputs = report.inputs().unwrap();
    assert_eq!(inputs.len().unwrap(), 1);
    assert_eq!(inputs.get(1).unwrap_err(), NC_E_OUT_OF_RANGE);
    let location = inputs.get(0).unwrap();
    let uniforms = report.uniforms().unwrap();
    let uniform = uniforms.get(0).unwrap();
    let name = uniform.glsl_block_name().unwrap();
    let info = report.info().unwrap();
    assert_eq!(info.uniform_count, uniforms.len().unwrap());
    assert_eq!(info.wgsl_byte_count, wgsl.byte_len().unwrap());
    assert_eq!(info.spirv_word_count * 4, spirv.len().unwrap());
    drop((root, report, inputs, uniforms));
    assert_eq!(wgsl.to_string().unwrap(), SOURCE);
    assert_eq!(location.info().unwrap().location, 0);
    assert_eq!(uniform.info().unwrap().byte_len, 16);
    assert!(glsl.to_string().unwrap().contains(&name.to_string().unwrap()));
    assert_eq!(&spirv.to_vec().unwrap()[..4], &0x07230203u32.to_le_bytes());
    let parsed = naga::front::wgsl::parse_str(&wgsl.to_string().unwrap()).unwrap();
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(&parsed)
        .unwrap();
}
/// Rejection has no artifacts, preserves error categories, and text is per-call and bounded.
#[test]
fn rejection_is_complete_and_diagnostics_are_per_call() {
    let root: neocogi_com::ComPtr<client::ShaderCompilerAbi> = CompilerObject::from_value(CompilerObject).query().unwrap();
    for (source, entry, cap, expected) in [
        (b"invalid".as_slice(), b"vs".as_slice(), 4194304, NC_E_INVALID_ARG),
        (SOURCE.as_bytes(), b"missing".as_slice(), 4194304, NC_E_INVALID_ARG),
        (SOURCE.as_bytes(), b"vs".as_slice(), 1, NC_E_OUT_OF_RANGE),
    ] {
        let report = root
            .compile(
                client::ShaderCompileRequest {
                    stage: GpuShaderStage::Vertex,
                    max_output_bytes: cap,
                },
                NcCopyFromBufferOffset::new(0, source),
                NcCopyFromBufferOffset::new(0, entry),
            )
            .unwrap();
        assert_eq!(report.status().unwrap(), expected.code());
        assert_eq!(report.wgsl().unwrap_err(), expected);
        assert_eq!(report.spirv().unwrap_err(), expected);
        assert_eq!(report.uniforms().unwrap_err(), expected);
        let text = report.diagnostic().unwrap().to_string_bounded(4096).unwrap();
        assert!(!text.is_empty());
    }
}
/// Exact combined payload admission is independent of optional reflection shortcuts.
#[test]
fn exact_payload_limit_and_bounded_snapshot() {
    use neocogi_shader_core::{abi::*, PreparedShader};
    let root: neocogi_com::ComPtr<ShaderCompilerAbi> = CompilerObject::from_value(CompilerObject).query().unwrap();
    let compile = |cap| {
        root.compile(
            ShaderCompileRequest {
                stage: GpuShaderStage::Vertex,
                max_output_bytes: cap,
            },
            NcCopyFromBufferOffset::new(0, SOURCE.as_bytes()),
            NcCopyFromBufferOffset::new(0, b"vs"),
        )
        .unwrap()
    };
    let report = compile(4194304);
    let prepared = PreparedShader::new(report, GpuShaderStage::Vertex, b"vs", 4194304).unwrap();
    let size = prepared.byte_len();
    assert_eq!(prepared.copy().unwrap().byte_len(), size);
    assert_eq!(compile(size as u64).status().unwrap(), NC_OK.code());
    assert_eq!(compile(size as u64 - 1).status().unwrap(), NC_E_OUT_OF_RANGE.code());
}
