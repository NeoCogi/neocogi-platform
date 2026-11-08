//! Independently generated COM caller tests, including ownership after report release.
use super::*;
use neocogi_com::{INcStringComPtrExt, INcBytesComPtrExt, NcCopyFromBufferOffset, NC_OK, NC_E_INVALID_ARG, NC_E_OUT_OF_RANGE};
use neocogi_platform::render::abi::GpuShaderStage;
/// Separate generated ABI consumer negotiates by interface identity, not a Rust object cast.
#[allow(dead_code, clippy::len_without_is_empty, reason = "independent generated caller for the minimal len/get ABI")]
mod client {
    include!(env!("IDL_NEOCOGI_SHADER"));
}
const SOURCE: &str = "struct U { value:vec4<f32> } @group(1) @binding(0) var<uniform> u:U; @vertex fn vs(@location(0) p:vec2<f32>)->@builtin(position) vec4<f32> {return vec4<f32>(p,0.25,1.0)+u.value;}";

use neocogi_platform::shader::testing as admission;

/// Inactive entries cannot change admission when a caller selects an otherwise valid entry.
#[test]
fn complete_module_profile_admission_precedes_selected_exports() {
    let root: neocogi_com::ComPtr<client::ShaderCompilerAbi> = CompilerObject::from_value(CompilerObject).query().unwrap();
    for case in admission::cases() {
        // Establish that these are portable-profile failures, not WGSL/Naga syntax failures.
        let module = naga::front::wgsl::parse_str(&case.source).unwrap();
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|error| panic!("{}: {error:?}", case.name));
        for (stage, entry) in [(GpuShaderStage::Vertex, "selected"), (case.stage, "outside")] {
            let report = root
                .compile_result(
                    client::ShaderCompileRequest { stage, max_output_bytes: 4194304 },
                    NcCopyFromBufferOffset::new(0, case.source.as_bytes()),
                    NcCopyFromBufferOffset::new(0, entry.as_bytes()),
                )
                .unwrap();
            assert_eq!(report.status().unwrap(), NC_E_INVALID_ARG.code(), "{} / {entry}", case.name);
            assert_eq!(report.wgsl().unwrap_err(), NC_E_INVALID_ARG);
            assert_eq!(report.spirv().unwrap_err(), NC_E_INVALID_ARG);
        }
    }
    let source = admission::valid_mixed_module();
    for (stage, entry, texture_count) in [(GpuShaderStage::Vertex, "selected", 0), (GpuShaderStage::Fragment, "outside", 1)] {
        let report = root
            .compile_result(
                client::ShaderCompileRequest { stage, max_output_bytes: 4194304 },
                NcCopyFromBufferOffset::new(0, source.as_bytes()),
                NcCopyFromBufferOffset::new(0, entry.as_bytes()),
            )
            .unwrap();
        assert_eq!(report.status().unwrap(), NC_OK.code(), "{}", report.diagnostic().unwrap().to_string().unwrap());
        assert_eq!(report.wgsl().unwrap().to_string().unwrap(), source);
        assert_eq!(report.sampled_textures().unwrap().len().unwrap(), texture_count);
    }
}
/// Independent generated callers receive complete native artifacts for exact mip-zero reads.
#[test]
fn exact_texel_reads_export_complete_owned_code() {
    let root: ComPtr<client::ShaderCompilerAbi> = CompilerObject::from_value(CompilerObject).query().unwrap();
    for coordinate in ["vec2<i32>(0)", "vec2<i32>(-1,0)", "vec2<u32>(0u)"] {
        let source = format!(
            "@group(2) @binding(0) var t:texture_2d<f32>; @group(2) @binding(1) var s:sampler; @fragment fn fs()->@location(0) vec4<f32>{{return textureLoad(t,{coordinate},0)+textureSample(t,s,vec2(.5));}}"
        );
        let report = root
            .compile_result(
                client::ShaderCompileRequest {
                    stage: GpuShaderStage::Fragment,
                    max_output_bytes: 4194304,
                },
                NcCopyFromBufferOffset::new(0, source.as_bytes()),
                NcCopyFromBufferOffset::new(0, b"fs"),
            )
            .unwrap();
        assert_eq!(report.status().unwrap(), NC_OK.code(), "{}", report.diagnostic().unwrap().to_string().unwrap());
        assert_eq!(report.wgsl().unwrap().to_string().unwrap(), source);
        assert_eq!(report.sampled_textures().unwrap().len().unwrap(), 1);
        let glsl = report.glsl().unwrap();
        let spirv = report.spirv().unwrap();
        drop(report);
        assert!(glsl.to_string().unwrap().contains("texelFetch"));
        assert!(!glsl.to_string().unwrap().contains("textureQueryLevels"));
        assert_eq!(&spirv.to_vec().unwrap()[..4], &0x07230203u32.to_le_bytes());
    }
}

/// Real COM getters do not require summary lookup, and item/name/code owners are independent.
#[test]
fn independent_owned_exports_and_items() {
    let root: neocogi_com::ComPtr<client::ShaderCompilerAbi> = CompilerObject::from_value(CompilerObject).query().unwrap();
    let report = root
        .compile_result(
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
            .compile_result(
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
    use neocogi_platform::shader::{abi::*, PreparedShader};
    let root: neocogi_com::ComPtr<ShaderCompilerAbi> = CompilerObject::from_value(CompilerObject).query().unwrap();
    let compile = |cap| {
        root.compile_result(
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

/// Ordinary Rust clients get their own failure, independent of later reports and provider lifetime.
#[test]
fn rust_compiler_errors_retain_status_and_text_after_provider_release() {
    let root: ComPtr<neocogi_platform::shader::abi::ShaderCompilerAbi> = CompilerObject::from_value(CompilerObject).query().unwrap();
    let request = neocogi_platform::shader::abi::ShaderCompileRequest {
        stage: GpuShaderStage::Vertex,
        max_output_bytes: 4194304,
    };
    let error = root
        .compile(request, NcCopyFromBufferOffset::new(0, b"invalid wgsl"), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap_err();
    assert_eq!(error.status, NC_E_INVALID_ARG);
    assert!(!error.message.is_empty());
    let text = error.message.clone();
    drop(
        root.compile(
            request,
            NcCopyFromBufferOffset::new(0, SOURCE.as_bytes()),
            NcCopyFromBufferOffset::new(0, b"vs"),
        )
        .unwrap(),
    );
    drop(root);
    assert_eq!(error.message, text);
}
